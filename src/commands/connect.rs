//! Connect command implementation

use std::net::SocketAddr;

use iroh::{Endpoint, NodeAddr, NodeId, RelayUrl};

use crate::doctor::{log_connection_changes, passive_side, Gui, DR_RELAY_ALPN};

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
