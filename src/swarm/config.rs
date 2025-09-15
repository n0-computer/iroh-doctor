//! Configuration for swarm client

use std::time::Duration;

use iroh::{defaults::DEFAULT_RELAY_QUIC_PORT, NodeId, RelayMap, SecretKey};

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
    /// Port variation detection configuration
    pub port_variation: PortVariationConfig,
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

/// Configuration for port variation detection
///
/// The probe service runs on the coordinator node and listens on multiple ports.
/// We connect via n0 discovery and default relays, only needing the coordinator's node ID.
#[derive(Debug, Clone)]
pub struct PortVariationConfig {
    /// Whether port variation detection is enabled
    pub enabled: bool,
    /// Primary port to probe
    pub primary_port: u16,
    /// Alternate ports to probe
    pub alternate_ports: Vec<u16>,
    /// Timeout for probe connections
    pub probe_timeout: Duration,
}

impl Default for PortVariationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            primary_port: DEFAULT_RELAY_QUIC_PORT,
            alternate_ports: vec![DEFAULT_RELAY_QUIC_PORT + 1],
            probe_timeout: Duration::from_secs(3),
        }
    }
}
