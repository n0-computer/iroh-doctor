//! SwarmClient command implementation

use std::{path::PathBuf, time::Duration};

use anyhow::{bail, Result};
use iroh::{NodeId, SecretKey};
use tracing::{info, warn};

use crate::{
    metrics::IrohMetricsRegistry,
    swarm::{SwarmConfig, TestCapability, TestType},
};

/// Run the swarm client command
pub async fn run_swarm_client(
    ssh_key: PathBuf,
    coordinator: NodeId,
    capabilities: String,
    heartbeat_interval: u64,
    name: Option<String>,
    metrics: IrohMetricsRegistry,
) -> Result<()> {
    // Verify SSH key exists for authentication
    if !ssh_key.exists() {
        bail!("SSH key not found at {:?}", ssh_key);
    }

    // Always generate a unique secret key for this node
    let secret_key = SecretKey::generate(rand::rngs::OsRng);
    info!("Generated new node with ID: {}", secret_key.public());

    // Parse capabilities string
    let test_capabilities = capabilities
        .split(',')
        .filter_map(|cap| match cap.trim().to_lowercase().as_str() {
            "connectivity" => Some(TestCapability {
                test_type: TestType::Connectivity,
                max_bandwidth_mbps: None,
            }),
            "throughput" => Some(TestCapability {
                test_type: TestType::Throughput,
                max_bandwidth_mbps: Some(10000),
            }),
            "latency" => Some(TestCapability {
                test_type: TestType::Latency,
                max_bandwidth_mbps: None,
            }),
            "fingerprint" => Some(TestCapability {
                test_type: TestType::Fingerprint,
                max_bandwidth_mbps: Some(10000),
            }),
            _ => {
                warn!("Unknown capability: {}", cap);
                None
            }
        })
        .collect();

    let swarm_config = SwarmConfig {
        coordinator_node_id: coordinator,
        secret_key,
        capabilities: test_capabilities,
        heartbeat_interval: Duration::from_secs(heartbeat_interval),
        relay_map: None, // Use default relay configuration
        name,
        transport: None, // Use default transport configuration
    };

    crate::swarm::run_swarm_client(swarm_config, &ssh_key, metrics).await
}
