//! Connect command implementation

use std::{net::SocketAddr, time::Duration};

use anyhow::Context;
use iroh::{endpoint::Connection, Endpoint, NodeAddr, NodeId, RelayMode, RelayUrl, SecretKey};

use crate::{
    config::NodeConfig,
    doctor::{log_connection_changes, passive_side, Gui, DR_RELAY_ALPN},
};

/// Connects to a [`NodeId`].
pub async fn connect(
    node_id: NodeId,
    direct_addresses: Vec<SocketAddr>,
    relay_url: Option<RelayUrl>,
    endpoint: Endpoint,
) -> anyhow::Result<()> {
    tracing::info!("dialing {:?}", node_id);
    let node_addr = NodeAddr::from_parts(node_id, relay_url, direct_addresses);
    let conn = endpoint.connect(node_addr, &DR_RELAY_ALPN).await;
    match conn {
        Ok(connection) => {
            let maybe_conn_type = endpoint.conn_type(node_id);
            let gui = Gui::new(endpoint, node_id);
            if let Some(conn_type) = maybe_conn_type {
                log_connection_changes(gui.mp.clone(), node_id, conn_type);
            }

            let close_reason = connection
                .close_reason()
                .map(|e| format!(" (reason: {e})"))
                .unwrap_or_default();

            if let Err(cause) = passive_side(gui, &connection).await {
                eprintln!("error handling connection: {cause}{close_reason}");
            } else {
                eprintln!("Connection closed{close_reason}");
            }
        }
        Err(cause) => {
            eprintln!("unable to connect to {node_id}: {cause}");
        }
    }

    Ok(())
}

/// Connects to a [`NodeId`] with a timeout.
pub async fn connect_with_timeout(
    config: &NodeConfig,
    node_id: NodeId,
    alpn: &[u8],
    timeout: Duration,
) -> anyhow::Result<Connection> {
    // Create a new endpoint for this connection
    let secret_key = SecretKey::generate(&mut rand::rngs::OsRng);

    let endpoint = if config.relay_nodes.is_empty() {
        Endpoint::builder()
            .alpns(vec![alpn.to_vec()])
            .secret_key(secret_key)
            .discovery_n0()
            .bind()
            .await?
    } else {
        let relay_map = config.relay_map()?;
        let relay_mode = match relay_map {
            Some(map) => RelayMode::Custom(map),
            None => RelayMode::Default,
        };
        Endpoint::builder()
            .alpns(vec![alpn.to_vec()])
            .secret_key(secret_key)
            .relay_mode(relay_mode)
            .discovery_n0()
            .bind()
            .await?
    };

    let node_addr = NodeAddr::new(node_id);
    tokio::time::timeout(timeout, endpoint.connect(node_addr, alpn))
        .await
        .context("Connection timeout")?
        .context("Failed to connect")
}
