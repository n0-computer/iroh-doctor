//! SwarmClient command implementation

use std::{path::PathBuf, time::Duration};

use anyhow::{bail, Result};
use iroh::{NodeId, SecretKey};
use tracing::info;

use crate::{metrics::IrohMetricsRegistry, swarm::SwarmConfig};

/// Run the swarm client command
pub async fn run_swarm_client(
    ssh_key: PathBuf,
    coordinator: NodeId,
    assignment_interval: u64,
    name: Option<String>,
    secret_key: SecretKey,
    metrics: IrohMetricsRegistry,
) -> Result<()> {
    // Verify SSH key exists for authentication
    if !ssh_key.exists() {
        bail!("SSH key not found at {:?}", ssh_key);
    }

    info!(
        "Starting swarm client with node ID: {}",
        secret_key.public()
    );

    let swarm_config = SwarmConfig {
        coordinator_node_id: coordinator,
        secret_key,
        assignment_interval: Duration::from_secs(assignment_interval),
        relay_map: None,
        name,
        transport: None,
        data_transfer_timeout: None, // Use default 30 seconds
    };

    crate::swarm::run_swarm_client(swarm_config, &ssh_key, metrics).await
}
