//! Main SwarmClient implementation

use std::{path::Path, time::Duration};

use anyhow::{Context, Result};
use iroh::{endpoint, Endpoint, EndpointId, RelayMode, Watcher};
use tracing::info;
use uuid::Uuid;

// Client timeout constants
const COORDINATOR_CONNECT_TIMEOUT_SECS: u64 = 10;

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

        let mut builder = endpoint::QuicTransportConfig::builder();
        // Always enable keep-alive for swarm connections
        builder = builder.keep_alive_interval(Duration::from_secs(1));

        if let Some(ref transport) = config.transport {
            if let Some(max_streams) = transport.max_concurrent_bidi_streams {
                builder = builder.max_concurrent_bidi_streams(max_streams.into());
            }

            if let Some(send_window_kb) = transport.send_window_kb {
                builder = builder.send_window((send_window_kb * 1024) as u64);
            }

            if let Some(receive_window_kb) = transport.receive_window_kb {
                builder = builder.receive_window((receive_window_kb * 1024).into());
            }
        }

        let transport_config = builder.build();

        let endpoint = Endpoint::builder()
            .secret_key(config.secret_key.clone())
            .alpns(vec![DOCTOR_SWARM_ALPN.to_vec(), N0DES_DOCTOR_ALPN.to_vec()])
            .relay_mode(relay_mode)
            .transport_config(transport_config)
            .bind()
            .await?;

        {
            let mut registry = metrics_registry.write().expect("poisoned");
            registry.register_all(endpoint.metrics());
        }

        let mut net_report_watcher = endpoint.net_report();
        net_report_watcher.initialized().await;
        let base_network_report = net_report_watcher.get();

        let extended_network_report = ExtendedNetworkReport::from_base_report(base_network_report);
        let doctor_client = match tokio::time::timeout(
            Duration::from_secs(COORDINATOR_CONNECT_TIMEOUT_SECS),
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
    pub fn get_extended_network_report(&self) -> ExtendedNetworkReport {
        let base_report = self.endpoint.net_report().get();
        ExtendedNetworkReport::from_base_report(base_report)
    }

    pub async fn get_assignments(&self) -> Result<Vec<TestAssignment>> {
        let network_report = self.get_extended_network_report();

        let response = self.doctor_client.get_assignments(network_report).await?;

        Ok(response.assignments)
    }

    /// Submit test result to coordinator (node_b is the peer)
    pub async fn submit_result(
        &self,
        test_run_id: Uuid,
        node_b: EndpointId,
        result_data: TestAssignmentResult,
    ) -> Result<()> {
        let report = TestResultReport {
            test_run_id,
            node_a: self.endpoint.id(),
            node_b,
            request_id: Some(Uuid::new_v4()),
            result_data,
        };

        self.doctor_client.report_result(report).await?;
        Ok(())
    }

    /// Mark a test as started to prevent duplicate assignments
    pub async fn mark_test_started(&self, test_run_id: Uuid, node_b: EndpointId) -> Result<bool> {
        let response = self
            .doctor_client
            .mark_test_started(test_run_id, self.endpoint.id(), node_b)
            .await?;
        Ok(response.success)
    }

    /// Get the endpoint
    pub fn endpoint(&self) -> Endpoint {
        self.endpoint.clone()
    }

    /// Get the node ID
    pub fn id(&self) -> EndpointId {
        self.endpoint.id()
    }
}
