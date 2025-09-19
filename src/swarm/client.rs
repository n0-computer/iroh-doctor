//! Main SwarmClient implementation

use std::{path::Path, time::Duration};

use anyhow::{Context, Result};
use iroh::{endpoint, Endpoint, NodeId, RelayMode, Watcher};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    metrics::IrohMetricsRegistry,
    swarm::{
        config::SwarmConfig,
        net_report_ext::ExtendedNetworkReport,
        rpc::{DoctorClient, TestAssignment, TestResultReport},
        tests::protocol::DOCTOR_SWARM_ALPN,
        types::TestAssignmentResult,
    },
};

/// ALPN protocol identifier for doctor RPC
pub const N0DES_DOCTOR_ALPN: &[u8] = b"n0/n0des-doctor/1";

/// Swarm client for network testing
#[derive(Debug)]
pub struct SwarmClient {
    endpoint: Endpoint,
    project_id: Uuid,
    doctor_client: DoctorClient,
}

impl SwarmClient {
    pub fn project_id(&self) -> Uuid {
        self.project_id
    }

    /// Connect to a coordinator node using RCAN authentication
    pub async fn connect(
        config: SwarmConfig,
        ssh_key_path: &Path,
        metrics_registry: IrohMetricsRegistry,
    ) -> Result<Self> {

        let relay_mode = if let Some(relay_map) = &config.relay_map {
            RelayMode::Custom(relay_map.clone())
        } else {
            RelayMode::Default
        };

        let mut transport_config = endpoint::TransportConfig::default();

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
            .alpns(vec![DOCTOR_SWARM_ALPN.to_vec()])
            .relay_mode(relay_mode)
            .transport_config(transport_config)
            .discovery_n0()
            .bind()
            .await?;

        {
            let mut registry = metrics_registry.write().expect("poisoned");
            registry.register_all(endpoint.metrics());
        }


        let mut direct_addrs = endpoint.direct_addresses();
        tokio::time::timeout(Duration::from_secs(10), direct_addrs.initialized())
            .await
            .ok();

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
                report
            }
            Err(_) => {
                None
            }
        };

        let extended_network_report = ExtendedNetworkReport::from_base_report(base_network_report);
        let doctor_client = match tokio::time::timeout(
            Duration::from_secs(30),
            DoctorClient::with_ssh_key(&endpoint, config.coordinator_node_id, ssh_key_path),
        )
        .await
        {
            Ok(client) => client.context("Failed to connect and authenticate with coordinator")?,
            Err(_) => {
                return Err(anyhow::anyhow!("Timeout connecting to coordinator"));
            }
        };

        let register_response = doctor_client
            .register(config.name.clone(), Some(extended_network_report.clone()))
            .await
            .context("Failed to register with coordinator")?;

        let project_id = register_response.project_id;
        info!("Registered with coordinator, project_id: {}", project_id);

        Ok(Self {
            endpoint,
            project_id,
            doctor_client,
        })
    }

    /// Get the current network report as an ExtendedNetworkReport
    pub async fn get_extended_network_report(&self) -> ExtendedNetworkReport {
        let base_report = self.endpoint.net_report().get();
        ExtendedNetworkReport::from_base_report(base_report)
    }

    pub async fn get_assignments(&self) -> Result<Vec<TestAssignment>> {
        let mut retries = 3;
        let mut last_error = None;

        let network_report = self.get_extended_network_report().await;

        while retries > 0 {
            match self.doctor_client.get_assignments(network_report.clone()).await {
                Ok(response) => return Ok(response.assignments),
                Err(e) => {
                    warn!("Failed to get assignments: {}", e);
                    last_error = Some(e);
                    retries -= 1;
                    if retries > 0 {
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
        result_data: TestAssignmentResult,
    ) -> Result<()> {
        let (success, error) = match &result_data {
            TestAssignmentResult::Error(e) => (false, Some(e.error.clone())),
            _ => (true, None),
        };

        let report = TestResultReport {
            test_run_id,
            node_a_id,
            node_b_id,
            success,
            error,
            request_id: Some(Uuid::new_v4()),
            result_data,
        };

        self.doctor_client.report_result(report).await?;
        Ok(())
    }

    /// Mark a test as started to prevent duplicate assignments
    pub async fn mark_test_started(
        &self,
        test_run_id: Uuid,
        node_a_id: NodeId,
        node_b_id: NodeId,
    ) -> Result<bool> {
        let response = self.doctor_client
            .mark_test_started(test_run_id, node_a_id, node_b_id)
            .await?;
        Ok(response.success)
    }

    /// Get the endpoint
    pub fn endpoint(&self) -> Endpoint {
        self.endpoint.clone()
    }

    /// Get the node ID
    pub fn node_id(&self) -> NodeId {
        self.endpoint.node_id()
    }
}
