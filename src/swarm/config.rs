//! Configuration for swarm client

use std::time::Duration;

use iroh::{NodeId, RelayMap, SecretKey};

/// Configuration for swarm client
#[derive(Debug, Clone)]
pub struct SwarmConfig {
    /// The node ID of the coordinator that this swarm client will connect to
    pub coordinator_node_id: NodeId,
    /// The secret key used for cryptographic operations and node identity
    pub secret_key: SecretKey,
    /// How often to poll for new assignments from the coordinator
    pub assignment_interval: Duration,
    /// Optional relay map for NAT traversal and connectivity, None defaults to default relay map
    pub relay_map: Option<RelayMap>,
    /// Optional human-readable name for this swarm client
    pub name: Option<String>,
    /// Transport configuration for throughput optimization
    pub transport: Option<TransportConfig>,
    /// Timeout for data transfer operations (default: 30 seconds)
    pub data_transfer_timeout: Option<Duration>,
}

/// Transport configuration for QUIC connection tuning
///
/// All fields are optional. When None, quinn/iroh defaults are used.
#[derive(Debug, Clone)]
pub struct TransportConfig {
    /// Maximum number of concurrent bidirectional streams
    pub max_concurrent_bidi_streams: Option<u32>,
    /// Send window size in KB
    pub send_window_kb: Option<u32>,
    /// Receive window size in KB
    pub receive_window_kb: Option<u32>,
    /// Idle timeout in seconds
    pub idle_timeout_secs: Option<u32>,
    /// Whether to enable keep-alive
    pub keep_alive: Option<bool>,
}
