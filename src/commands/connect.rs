//! Connect command implementation

use std::net::SocketAddr;

use iroh::{Endpoint, EndpointAddr, EndpointId, RelayUrl};

use crate::doctor::{log_connection_changes, passive_side, Gui, DR_RELAY_ALPN};

/// Connects to a [`EndpointId`].
pub async fn connect(
    endpoint_id: EndpointId,
    direct_addresses: Vec<SocketAddr>,
    relay_url: Option<RelayUrl>,
    endpoint: Endpoint,
) -> anyhow::Result<()> {
    tracing::info!("dialing {:?}", endpoint_id);
    let mut endpoint_addr = EndpointAddr::new(endpoint_id);
    if let Some(relay_url) = relay_url {
        endpoint_addr = endpoint_addr.with_relay_url(relay_url);
    }
    for ip_addr in direct_addresses {
        endpoint_addr = endpoint_addr.with_ip_addr(ip_addr);
    }
    let conn = endpoint.connect(endpoint_addr, &DR_RELAY_ALPN).await;
    match conn {
        Ok(connection) => {
            let maybe_conn_type = endpoint.conn_type(endpoint_id);
            let gui = Gui::new(endpoint, endpoint_id);
            if let Some(conn_type) = maybe_conn_type {
                log_connection_changes(gui.mp.clone(), endpoint_id, conn_type);
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
            eprintln!("unable to connect to {endpoint_id}: {cause}");
        }
    }

    Ok(())
}
