//! Main SwarmClient implementation

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use futures_lite::StreamExt;
use iroh::{endpoint, net_report::Report, Endpoint, NodeAddr, NodeId, RelayMode};
use n0_watcher::Watcher;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::swarm::{
    config::SwarmConfig,
    rpc::{self, DoctorClient, TestAssignment, TestResultReport},
    tests::protocol::DOCTOR_SWARM_ALPN,
    types::TestAssignmentResult,
};

/// ALPN protocol identifier for doctor RPC
pub const DOCTOR_ALPN: &[u8] = b"n0/n0des-doctor/1";

/// Swarm client for network testing
#[derive(Debug)]
pub struct SwarmClient {
    endpoint: Arc<Endpoint>,
    node_id: NodeId,
    /// ID of the project in the coordinator
    project_id: Uuid,
    /// IRPC client for coordinator communication
    doctor_client: Arc<tokio::sync::Mutex<rpc::DoctorClient>>,
    /// Cached network report shared between tasks
    cached_network_report: Arc<RwLock<Option<Report>>>,
}

impl SwarmClient {
    pub fn project_id(&self) -> Uuid {
        self.project_id
    }

    /// Connect to a coordinator node using RCAN authentication
    pub async fn connect(
        config: SwarmConfig,
        ssh_key_path: &std::path::Path,
        metrics_registry: crate::metrics::IrohMetricsRegistry,
    ) -> Result<Self> {
        let node_id = config.secret_key.public();
        info!("Starting swarm client with NodeID: {}", node_id);
        debug!(
            "Secret key bytes (first 8): {:?}",
            &config.secret_key.to_bytes()[..8]
        );

        // Create endpoint with relay configuration
        let relay_mode = if let Some(relay_map) = &config.relay_map {
            RelayMode::Custom(relay_map.clone())
        } else {
            RelayMode::Default
        };

        // Configure transport for optimal throughput
        let mut transport_config = endpoint::TransportConfig::default();

        // Apply transport configuration from config or use defaults
        if let Some(ref transport) = config.transport {
            // Concurrent bidirectional streams (default: 10)
            let max_streams = transport.max_concurrent_bidi_streams.unwrap_or(10);
            transport_config.max_concurrent_bidi_streams(max_streams.into());

            // Send and receive windows (default: 10MB)
            let send_window_kb = transport.send_window_kb.unwrap_or(10240); // 10MB
            let receive_window_kb = transport.receive_window_kb.unwrap_or(10240); // 10MB
            transport_config.send_window((send_window_kb * 1024) as u64);
            transport_config.receive_window((receive_window_kb * 1024).into());

            // Keep-alive (default: disabled for throughput tests)
            if transport.keep_alive.unwrap_or(false) {
                transport_config.keep_alive_interval(Some(Duration::from_secs(30)));
            } else {
                transport_config.keep_alive_interval(None);
            }

            // Idle timeout (default: 60 seconds)
            let idle_timeout_secs = transport.idle_timeout_secs.unwrap_or(60);
            transport_config.max_idle_timeout(Some(
                Duration::from_secs(idle_timeout_secs as u64)
                    .try_into()
                    .unwrap(),
            ));
        } else {
            // Use existing defaults when no transport config is provided
            transport_config.max_concurrent_bidi_streams(10u32.into());
            transport_config.receive_window((10 * 1024 * 1024u32).into());
            transport_config.send_window(10 * 1024 * 1024);
            transport_config.keep_alive_interval(None);
            transport_config.max_idle_timeout(Some(Duration::from_secs(60).try_into().unwrap()));
        }

        // Note: BBR congestion control would need to be set at quinn level
        // think that iroh currently doesn't expose congestion control configuration

        let endpoint = Endpoint::builder()
            .secret_key(config.secret_key.clone())
            .alpns(vec![DOCTOR_SWARM_ALPN.to_vec(), DOCTOR_ALPN.to_vec()])
            .relay_mode(relay_mode)
            .transport_config(transport_config)
            .discovery_n0()
            .bind()
            .await?;

        // Register endpoint metrics
        {
            let mut registry = metrics_registry.write().expect("poisoned");
            registry.register_all(endpoint.metrics());
        }

        let actual_node_id = endpoint.node_id();
        info!("Swarm client listening with node ID: {}", actual_node_id);

        // Verify the endpoint is using our secret key
        if actual_node_id != node_id {
            error!(
                "MISMATCH: Expected node ID {} but endpoint has {}",
                node_id, actual_node_id
            );
        }

        // Connect to coordinator
        let coordinator_addr = NodeAddr::new(config.coordinator_node_id);
        info!(
            "Connecting to coordinator at {}",
            config.coordinator_node_id
        );

        // Wait for endpoint to initialize (relay connection, direct addresses, etc.)
        info!("Waiting for endpoint initialization...");
        let mut direct_addrs = endpoint.direct_addresses();
        tokio::select! {
            result = tokio::time::timeout(
                Duration::from_secs(10),
                direct_addrs.initialized(),
            ) => {
                match result {
                    Ok(_) => info!("Endpoint initialized successfully"),
                    Err(_) => warn!("Timeout waiting for endpoint initialization"),
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Received Ctrl+C during endpoint initialization, shutting down...");
                return Err(anyhow::anyhow!("Cancelled by user"));
            }
        }

        // Collect initial network report for NAT detection
        // We'll wait briefly for an initial report
        let network_report = {
            let mut net_report_stream = endpoint.net_report().stream();
            tokio::select! {
                result = tokio::time::timeout(Duration::from_secs(3), net_report_stream.next()) => {
                    match result {
                        Ok(Some(Some(report))) => {
                            info!("Collected network report for NAT detection");
                            info!("Network report details: udp_v4={:?}, udp_v6={:?}, mapping_varies_by_dest_ipv4={:?}, mapping_varies_by_dest_ipv6={:?}",
                                report.udp_v4, report.udp_v6, report.mapping_varies_by_dest_ipv4, report.mapping_varies_by_dest_ipv6);

                            // Use iroh's Report directly
                            Some(report)
                        }
                        Ok(Some(None)) => {
                            warn!("Network report stream returned None report");
                            None
                        }
                        Ok(None) => {
                            warn!("Network report stream ended unexpectedly");
                            None
                        }
                        Err(_) => {
                            warn!("Timeout waiting for network report, proceeding without NAT detection");
                            None
                        }
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    info!("Received Ctrl+C during network report collection, shutting down...");
                    return Err(anyhow::anyhow!("Cancelled by user"));
                }
            }
        };

        // Create doctor client with SSH key authentication and connect to coordinator
        let doctor_client = tokio::select! {
            result = tokio::time::timeout(
                Duration::from_secs(30),
                DoctorClient::with_ssh_key(&endpoint, coordinator_addr.clone(), ssh_key_path)
            ) => {
                match result {
                    Ok(client) => client.context("Failed to connect and authenticate with coordinator")?,
                    Err(_) => {
                        warn!("Timeout connecting to coordinator, coordinator may be offline");
                        return Err(anyhow::anyhow!("Timeout connecting to coordinator"));
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Received Ctrl+C during coordinator connection, shutting down...");
                return Err(anyhow::anyhow!("Cancelled by user"));
            }
        };

        // Register with coordinator
        let register_response = tokio::select! {
            result = doctor_client.register(
                config.capabilities.clone(),
                config.name.clone(),
                network_report.clone(),
            ) => {
                result.context("Failed to register with coordinator")?
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Received Ctrl+C during coordinator registration, shutting down...");
                return Err(anyhow::anyhow!("Cancelled by user"));
            }
        };

        let project_id = register_response.project_id;
        info!("Registered with coordinator, project_id: {}", project_id);

        Ok(Self {
            endpoint: Arc::new(endpoint),
            node_id,
            project_id,
            doctor_client: Arc::new(tokio::sync::Mutex::new(doctor_client)),
            cached_network_report: Arc::new(RwLock::new(network_report)),
        })
    }

    /// Set the cached network report
    pub async fn set_cached_network_report(&self, network_report: Option<Report>) {
        let mut cached = self.cached_network_report.write().await;
        *cached = network_report;
    }

    /// Get the cached network report
    pub async fn get_cached_network_report(&self) -> Option<Report> {
        let cached = self.cached_network_report.read().await;
        cached.clone()
    }

    /// Get assignments from coordinator with retry logic
    pub async fn get_assignments(&self) -> Result<Vec<TestAssignment>> {
        let mut retries = 3;
        let mut last_error = None;

        while retries > 0 {
            let mut client = self.doctor_client.lock().await;

            // Try to ensure connection is alive
            match client.ensure_connected().await {
                Ok(()) => {
                    // Connection is good, try to get assignments
                    match client.get_assignments().await {
                        Ok(response) => return Ok(response.assignments),
                        Err(e) => {
                            warn!("Failed to get assignments: {}", e);
                            last_error = Some(e);
                            retries -= 1;
                            if retries > 0 {
                                // Wait before retry
                                drop(client); // Release lock before sleeping
                                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to ensure connection: {}", e);
                    last_error = Some(e);
                    retries -= 1;
                    if retries > 0 {
                        // Wait before retry
                        drop(client); // Release lock before sleeping
                        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                    }
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| anyhow::anyhow!("Failed to get assignments after retries")))
    }

    /// Submit test result to coordinator
    pub async fn submit_result(
        &self,
        test_run_id: Uuid,
        node_a_id: NodeId,
        node_b_id: NodeId,
        success: bool,
        result_data: TestAssignmentResult,
    ) -> Result<()> {
        let mut client = self.doctor_client.lock().await;

        // Ensure connection is alive
        client.ensure_connected().await?;

        // Convert to TestResultReport format expected by backend
        let report = TestResultReport {
            test_run_id,
            node_a_id,
            node_b_id,
            success,
            error: if success { None } else { Some("Test failed".to_string()) },
            request_id: Some(Uuid::new_v4()), // For idempotency
            result_data: result_data.clone(),
        };

        client.report_result(report).await?;
        Ok(())
    }

    /// Send heartbeat to coordinator with retry logic
    pub async fn heartbeat(&self) -> Result<()> {
        let mut retries = 2; // Fewer retries for heartbeat since it's called frequently
        let mut last_error = None;

        // Get the cached network report to include in heartbeat
        let network_report = self.get_cached_network_report().await;

        while retries > 0 {
            let mut client = self.doctor_client.lock().await;

            // Try to ensure connection is alive
            match client.ensure_connected().await {
                Ok(()) => {
                    // Connection is good, try to send heartbeat with network report
                    match client
                        .heartbeat_with_network_report(network_report.clone())
                        .await
                    {
                        Ok(_response) => return Ok(()), // Ignore the heartbeat response
                        Err(e) => {
                            debug!("Failed to send heartbeat: {}", e);
                            last_error = Some(e);
                            retries -= 1;
                            if retries > 0 {
                                // Wait before retry
                                drop(client); // Release lock before sleeping
                                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                            }
                        }
                    }
                }
                Err(e) => {
                    debug!("Failed to ensure connection for heartbeat: {}", e);
                    last_error = Some(e);
                    retries -= 1;
                    if retries > 0 {
                        // Wait before retry
                        drop(client); // Release lock before sleeping
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    }
                }
            }
        }

        // Don't propagate heartbeat errors - just log them
        if let Some(e) = last_error {
            warn!("Failed to send heartbeat after retries: {}", e);
        }
        Ok(())
    }

    /// Mark a test as started to prevent duplicate assignments
    pub async fn mark_test_started(
        &self,
        test_run_id: Uuid,
        node_a_id: NodeId,
        node_b_id: NodeId,
    ) -> Result<bool> {
        let mut client = self.doctor_client.lock().await;

        // Ensure connection is alive
        client.ensure_connected().await?;

        let response = client
            .mark_test_started(test_run_id, node_a_id, node_b_id)
            .await?;
        Ok(response.success)
    }

    /// Get the endpoint
    pub fn endpoint(&self) -> Arc<Endpoint> {
        self.endpoint.clone()
    }

    /// Get the node ID
    pub fn node_id(&self) -> NodeId {
        self.node_id
    }
}
