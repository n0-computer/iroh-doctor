//! Configuration for swarm client

use std::time::Duration;

use iroh::{NodeId, RelayMap, SecretKey};

use crate::swarm::types::TestCapability;

/// Configuration for swarm client
#[derive(Debug, Clone)]
pub struct SwarmConfig {
    /// The node ID of the coordinator that this swarm client will connect to
    pub coordinator_node_id: NodeId,
    /// The secret key used for cryptographic operations and node identity
    pub secret_key: SecretKey,
    /// List of test capabilities that this swarm client supports
    pub capabilities: Vec<TestCapability>,
    /// How often to send heartbeat messages to maintain connection with coordinator
    pub heartbeat_interval: Duration,
    /// Optional relay map for NAT traversal and connectivity, None defaults to default relay map
    pub relay_map: Option<RelayMap>,
    /// Optional human-readable name for this swarm client
    pub name: Option<String>,
    /// Transport configuration for throughput optimization
    pub transport: Option<TransportConfig>,
}

/// Transport configuration for QUIC connection tuning
#[derive(Debug, Clone)]
pub struct TransportConfig {
    /// Maximum number of concurrent bidirectional streams (default: 10)
    pub max_concurrent_bidi_streams: Option<u32>,
    /// Send window size in KB (default: 10240 = 10MB)
    pub send_window_kb: Option<u32>,
    /// Receive window size in KB (default: 10240 = 10MB)
    pub receive_window_kb: Option<u32>,
    /// Idle timeout in seconds (default: 60)
    pub idle_timeout_secs: Option<u32>,
    /// Whether to enable keep-alive (default: false for throughput tests)
    pub keep_alive: Option<bool>,
}
