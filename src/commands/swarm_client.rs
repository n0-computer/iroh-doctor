//! SwarmClient command implementation

use std::{path::PathBuf, time::Duration};

use anyhow::{bail, Result};
use iroh::{NodeId, SecretKey};
use tracing::info;

use crate::{
    metrics::IrohMetricsRegistry,
    swarm::{self, SwarmConfig},
};

/// Run the swarm client command
pub async fn run_swarm_client(
    ssh_key: PathBuf,
    coordinator: NodeId,
    assignment_interval: u64,
    name: Option<String>,
    secret_key: SecretKey,
    metrics: IrohMetricsRegistry,
) -> Result<()> {
    if !ssh_key.exists() {
        bail!("SSH key file does not exist: {:?}", ssh_key);
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
        data_transfer_timeout: None,
    };

    swarm::run_swarm_client(swarm_config, &ssh_key, metrics).await
}
