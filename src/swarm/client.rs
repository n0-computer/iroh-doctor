//! Main SwarmClient implementation

use std::{path::Path, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use iroh::{endpoint, Endpoint, NodeId, RelayMode, Watcher};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::swarm::{
    config::SwarmConfig,
    net_report_ext::{self, probe_port_variation, ExtendedNetworkReport},
    rpc::{DoctorClient, TestAssignment, TestResultReport},
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
    doctor_client: Arc<Mutex<DoctorClient>>,
    /// Port variation detection results (stored separately as they're not part of iroh's report)
    port_variation_result: Arc<RwLock<Option<net_report_ext::PortVariationResult>>>,
}

impl SwarmClient {
    pub fn project_id(&self) -> Uuid {
        self.project_id
    }

    /// Connect to a coordinator node using RCAN authentication
    pub async fn connect(
        config: SwarmConfig,
        ssh_key_path: &Path,
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

        // Start with quinn/iroh default transport config
        let mut transport_config = endpoint::TransportConfig::default();

        // Only modify transport config when explicitly requested
        if let Some(ref transport) = config.transport {
            if let Some(max_streams) = transport.max_concurrent_bidi_streams {
                transport_config.max_concurrent_bidi_streams(max_streams.into());
            }

            if let Some(send_window_kb) = transport.send_window_kb {
                transport_config.send_window((send_window_kb * 1024) as u64);
            }

            if let Some(receive_window_kb) = transport.receive_window_kb {
                transport_config.receive_window((receive_window_kb * 1024).into());
            }

            if let Some(keep_alive) = transport.keep_alive {
                if keep_alive {
                    transport_config.keep_alive_interval(Some(Duration::from_secs(30)));
                } else {
                    transport_config.keep_alive_interval(None);
                }
            }

            if let Some(idle_timeout_secs) = transport.idle_timeout_secs {
                transport_config.max_idle_timeout(Some(
                    Duration::from_secs(idle_timeout_secs as u64)
                        .try_into()
                        .unwrap(),
                ));
            }
        }

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
        info!(
            "Connecting to coordinator at {}",
            config.coordinator_node_id
        );

        // Wait for endpoint to initialize (relay connection, direct addresses, etc.)
        info!("Waiting for endpoint initialization...");
        let mut direct_addrs = endpoint.direct_addresses();
        match tokio::time::timeout(Duration::from_secs(10), direct_addrs.initialized()).await {
            Ok(_) => info!("Endpoint initialized successfully"),
            Err(_) => warn!("Timeout waiting for endpoint initialization"),
        }

        // Wait for initial network report
        let mut net_report_watcher = endpoint.net_report();
        let initialized_watcher = net_report_watcher.initialized();
        let base_network_report = match tokio::time::timeout(
            Duration::from_secs(3),
            initialized_watcher,
        )
        .await
        {
            Ok(_) => {
                let report = net_report_watcher.get();
                if let Some(ref r) = report {
                    info!("Collected network report for NAT detection");
                    info!("Network report details: udp_v4={:?}, udp_v6={:?}, mapping_varies_by_dest_ipv4={:?}, mapping_varies_by_dest_ipv6={:?}",
                        r.udp_v4, r.udp_v6, r.mapping_varies_by_dest_ipv4, r.mapping_varies_by_dest_ipv6);
                }
                report
            }
            Err(_) => {
                warn!("Timeout waiting for network report, proceeding without NAT detection");
                None
            }
        };

        // Run port variation detection if enabled
        let port_variation_result = if config.port_variation.enabled {
            debug!("Running port variation detection probe");
            match probe_port_variation(&config.port_variation, config.coordinator_node_id).await {
                Ok(result) => {
                    debug!(
                        "Port variation detection complete: IPv4 varies: {:?}, IPv6 varies: {:?}",
                        result.ipv4_varies, result.ipv6_varies
                    );
                    Some(result)
                }
                Err(e) => {
                    warn!("Port variation detection failed: {}", e);
                    None
                }
            }
        } else {
            debug!("Port variation detection disabled");
            None
        };

        // Create extended network report for registration
        let extended_network_report = if let Some(ref port_result) = port_variation_result {
            ExtendedNetworkReport::from_base_report(base_network_report)
                .with_port_variation(port_result.ipv4_varies, port_result.ipv6_varies)
        } else {
            ExtendedNetworkReport::from_base_report(base_network_report)
        };

        // Create doctor client with SSH key authentication and connect to coordinator
        let doctor_client = match tokio::time::timeout(
            Duration::from_secs(30),
            DoctorClient::with_ssh_key(&endpoint, config.coordinator_node_id, ssh_key_path),
        )
        .await
        {
            Ok(client) => client.context("Failed to connect and authenticate with coordinator")?,
            Err(_) => {
                warn!("Timeout connecting to coordinator, coordinator may be offline");
                return Err(anyhow::anyhow!("Timeout connecting to coordinator"));
            }
        };

        // Register with coordinator
        let register_response = doctor_client
            .register(config.name.clone(), Some(extended_network_report.clone()))
            .await
            .context("Failed to register with coordinator")?;

        let project_id = register_response.project_id;
        info!("Registered with coordinator, project_id: {}", project_id);

        Ok(Self {
            endpoint: Arc::new(endpoint),
            node_id,
            project_id,
            doctor_client: Arc::new(Mutex::new(doctor_client)),
            port_variation_result: Arc::new(RwLock::new(port_variation_result)),
        })
    }

    /// Get the current network report as an ExtendedNetworkReport
    pub async fn get_extended_network_report(&self) -> ExtendedNetworkReport {
        let base_report = self.endpoint.net_report().get();
        let port_variation = self.port_variation_result.read().await;

        if let Some(ref port_result) = *port_variation {
            ExtendedNetworkReport::from_base_report(base_report)
                .with_port_variation(port_result.ipv4_varies, port_result.ipv6_varies)
        } else {
            ExtendedNetworkReport::from_base_report(base_report)
        }
    }

    /// Update the port variation result
    pub async fn update_port_variation(&self, result: Option<net_report_ext::PortVariationResult>) {
        let mut port_variation = self.port_variation_result.write().await;
        *port_variation = result;
    }

    /// Get assignments from coordinator with retry logic
    /// Always includes network report in the request (acts as heartbeat)
    pub async fn get_assignments(&self) -> Result<Vec<TestAssignment>> {
        let mut retries = 3;
        let mut last_error = None;

        // Get network report to send with request (acts as heartbeat)
        let network_report = self.get_extended_network_report().await;

        while retries > 0 {
            let mut client = self.doctor_client.lock().await;

            // Try to ensure connection is alive
            match client.ensure_connected().await {
                Ok(()) => {
                    // Connection is good, try to get assignments with network report
                    match client.get_assignments(network_report.clone()).await {
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
            error: if success {
                None
            } else {
                Some("Test failed".to_string())
            },
            request_id: Some(Uuid::new_v4()), // For idempotency
            result_data: result_data.clone(),
        };

        client.report_result(report).await?;
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
