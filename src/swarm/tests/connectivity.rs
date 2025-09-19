//! Connectivity test implementation

use std::time::Instant;

use anyhow::Result;
use iroh::{endpoint::ConnectionType, Endpoint, NodeId, Watcher};
use tracing::{info, warn};

use super::protocol::DOCTOR_SWARM_ALPN;
use crate::swarm::types::ConnectivityResult;

/// Helper function to get the real connection type from the endpoint
pub(crate) fn get_connection_type(endpoint: &Endpoint, node_id: NodeId) -> Option<ConnectionType> {
    endpoint.conn_type(node_id).map(|mut watcher| watcher.get())
}


/// Run a connectivity test between two nodes
pub async fn run_connectivity_test(
    endpoint: &Endpoint,
    node_id: NodeId,
) -> Result<ConnectivityResult> {
    let start = Instant::now();

    // The endpoint handles discovery internally, including DNS resolution,
    // retries, and caching. We just need to connect directly with the NodeId.
    info!("Attempting to connect to {}", node_id);

    match endpoint.connect(node_id, DOCTOR_SWARM_ALPN).await {
        Ok(conn) => {
            let connection_time = start.elapsed();
            info!(
                "Successfully connected to {} in {:?}",
                node_id, connection_time
            );

            conn.close(0u32.into(), b"connectivity test complete");

            Ok(ConnectivityResult {
                connected: true,
                connection_time_ms: Some(connection_time.as_millis() as u64),
                node_id,
                duration: connection_time,
                error: None,
                connection_type: get_connection_type(endpoint, node_id),
                ..Default::default()
            })
        }
        Err(e) => {
            warn!("Failed to connect to {}: {}", node_id, e);
            Ok(ConnectivityResult {
                connected: false,
                connection_time_ms: None,
                node_id,
                duration: start.elapsed(),
                error: Some(e.to_string()),
                connection_type: None,
                ..Default::default()
            })
        }
    }
}
