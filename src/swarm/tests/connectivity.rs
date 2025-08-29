//! Connectivity test implementation

use std::time::{Duration, Instant};

use anyhow::Result;
use futures_lite::StreamExt;
use iroh::{endpoint::ConnectionType, Endpoint, NodeAddr, NodeId, Watcher};
use tracing::{info, warn};

use super::protocol::DOCTOR_SWARM_ALPN;
use crate::swarm::types::{ConnectivityResult, TestResult};

/// Helper function to get the real connection type from the endpoint
pub(crate) fn get_connection_type(endpoint: &Endpoint, peer_id: NodeId) -> Option<ConnectionType> {
    endpoint.conn_type(peer_id).map(|mut watcher| {
        watcher.get()
    })
}

/// Helper function to resolve node address using discovery
pub(crate) async fn resolve_node_addr(
    endpoint: &Endpoint,
    node_id: NodeId,
) -> Result<Option<NodeAddr>> {
    // Use the discovery mechanism to find the node
    if let Some(discovery) = endpoint.discovery() {
        if let Some(mut stream) = discovery.resolve(node_id) {
            let timeout = Duration::from_secs(5);
            let start = Instant::now();

            while start.elapsed() < timeout {
                match stream.next().await {
                    Some(Ok(info)) => {
                        let node_addr = info.to_node_addr();
                        if node_addr.direct_addresses().count() > 0
                            || node_addr.relay_url().is_some()
                        {
                            return Ok(Some(node_addr));
                        }
                    }
                    Some(Err(e)) => {
                        warn!("Discovery error for {}: {}", node_id, e);
                    }
                    None => break,
                }
            }

            // If we didn't find anything through discovery, return a basic NodeAddr
            // The endpoint will try to use any cached information it has
            Ok(Some(NodeAddr::new(node_id)))
        } else {
            Ok(Some(NodeAddr::new(node_id)))
        }
    } else {
        Ok(Some(NodeAddr::new(node_id)))
    }
}

/// Run a connectivity test between two nodes
pub async fn run_connectivity_test(
    endpoint: &Endpoint,
    peer_node_id: NodeId,
) -> Result<TestResult<ConnectivityResult>> {
    let start = Instant::now();

    // Try to resolve with retries and DNS propagation delays
    let mut node_addr = None;
    let max_dns_attempts = 3;

    for attempt in 1..=max_dns_attempts {
        if attempt > 1 {
            info!(
                "Waiting 10 seconds for DNS propagation before retry {} of {}",
                attempt, max_dns_attempts
            );
            tokio::time::sleep(Duration::from_secs(10)).await;
        }

        match resolve_node_addr(endpoint, peer_node_id).await {
            Ok(Some(addr)) => {
                info!(
                    "Successfully resolved {} to {:?} on attempt {}",
                    peer_node_id, addr, attempt
                );
                node_addr = Some(addr);
                break;
            }
            Ok(None) => {
                warn!(
                    "No address found for {} on attempt {}",
                    peer_node_id, attempt
                );
            }
            Err(e) => {
                warn!(
                    "Error resolving {} on attempt {}: {}",
                    peer_node_id, attempt, e
                );
            }
        }
    }

    let node_addr = match node_addr {
        Some(addr) => addr,
        None => {
            return Ok(TestResult::failure(ConnectivityResult {
                connected: false,
                connection_time_ms: None,
                peer: peer_node_id.to_string(),
                duration_ms: start.elapsed().as_millis(),
                error: Some(format!(
                    "Failed to resolve node address after {max_dns_attempts} attempts"
                )),
                connection_type: None, // No connection established
            }));
        }
    };

    info!(
        "Attempting to connect to {} at {:?}",
        peer_node_id, node_addr
    );

    match endpoint.connect(node_addr, DOCTOR_SWARM_ALPN).await {
        Ok(conn) => {
            let connection_time = start.elapsed();
            info!(
                "Successfully connected to {} in {:?}",
                peer_node_id, connection_time
            );

            conn.close(0u32.into(), b"connectivity test complete");

            Ok(TestResult::success(ConnectivityResult {
                connected: true,
                connection_time_ms: Some(connection_time.as_millis() as u64),
                peer: peer_node_id.to_string(),
                duration_ms: connection_time.as_millis(),
                error: None,
                connection_type: get_connection_type(endpoint, peer_node_id),
            }))
        }
        Err(e) => {
            warn!("Failed to connect to {}: {}", peer_node_id, e);
            Ok(TestResult::failure(ConnectivityResult {
                connected: false,
                connection_time_ms: None,
                peer: peer_node_id.to_string(),
                duration_ms: start.elapsed().as_millis(),
                error: Some(e.to_string()),
                connection_type: None, // No connection established
            }))
        }
    }
}
