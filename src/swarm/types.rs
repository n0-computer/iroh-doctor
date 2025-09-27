//! Types for swarm test statistics and results

use std::{fmt, time::Duration};

use iroh::{endpoint::ConnectionType, NodeId};
use portable_atomic::{AtomicU64, Ordering};
use serde::{Deserialize, Serialize};

// Common default configuration values used across the swarm module
pub const DEFAULT_DATA_TRANSFER_TIMEOUT_SECS: u64 = 5 * 60; // 5 minutes
pub const DEFAULT_DATA_SIZE: u64 = 10 * 1024 * 1024; // 10MB
pub const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024; // 1MB
pub const DEFAULT_PARALLEL_STREAMS: u32 = 4;
pub const DEFAULT_CONNECTION_TIMEOUT_SECS: u32 = 20;
pub const DEFAULT_PING_INTERVAL_MS: u32 = 10;
pub const DEFAULT_PING_TIMEOUT_MS: u32 = 3000;

/// Test types supported by doctor nodes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TestType {
    #[default]
    Latency,
    Throughput,
    /// Combined test: connectivity + throughput + latency
    Fingerprint,
}

impl fmt::Display for TestType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TestType::Throughput => write!(f, "throughput"),
            TestType::Latency => write!(f, "latency"),
            TestType::Fingerprint => write!(f, "fingerprint"),
        }
    }
}

/// Configuration for a test run
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TestConfig {
    pub test_type: TestType,
    pub duration_secs: Option<u64>,
    pub size_bytes: Option<u64>,
    pub iterations: Option<u32>,
    pub advanced: Option<AdvancedTestConfig>,
}

/// Advanced configuration for fine-tuning test parameters
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AdvancedTestConfig {
    /// Throughput-specific settings
    pub throughput: Option<ThroughputAdvancedConfig>,
    /// Latency-specific settings
    pub latency: Option<LatencyAdvancedConfig>,
    /// Network-level settings
    pub network: Option<NetworkAdvancedConfig>,
}

/// Advanced configuration for throughput tests
///
/// Note: Transport-level buffer settings (send/receive windows) are configured
/// globally via SwarmConfig.transport, not per-test
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ThroughputAdvancedConfig {
    /// Number of parallel streams (default: 4, range: 1-16)
    pub parallel_streams: Option<u32>,
    /// Chunk size in KB (default: 64, range: 16-512)
    pub chunk_size_kb: Option<u32>,
}

/// Advanced configuration for latency tests
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LatencyAdvancedConfig {
    /// Interval between pings in milliseconds (default: 10)
    pub ping_interval_ms: Option<u32>,
    /// Timeout for individual pings in milliseconds (default: 1000)
    pub ping_timeout_ms: Option<u32>,
}

/// Advanced network configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NetworkAdvancedConfig {
    /// Connection establishment timeout in seconds (default: 20)
    pub connection_timeout_secs: Option<u32>,
}

/// Doctor-specific capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorCaps {
    /// Can register as doctor node
    pub can_register: bool,
    /// Can report test results
    pub can_report_results: bool,
}

impl Default for DoctorCaps {
    fn default() -> Self {
        Self {
            can_register: true,
            can_report_results: true,
        }
    }
}

/// Statistics for a single stream in a throughput test
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StreamStats {
    /// Stream identifier (0-based index)
    pub stream_id: usize,
    /// Total bytes sent on this stream
    pub bytes_sent: u64,
    /// Total bytes received on this stream
    pub bytes_received: u64,
    /// Upload duration
    pub upload_duration: Duration,
    /// Download duration
    pub download_duration: Duration,
    /// Calculated throughput in Mbps
    pub throughput_mbps: f64,
}

/// Connection statistics extracted from quinn::ConnectionStats
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConnectionStats {
    /// Round-trip time in milliseconds
    pub rtt_ms: u32,
    /// Smoothed round-trip time in milliseconds  
    pub smoothed_rtt_ms: u32,
    /// Latest RTT sample in milliseconds
    pub latest_rtt_ms: u32,
    /// RTT variance in milliseconds
    pub rtt_variance_ms: u32,
    /// Congestion window in bytes
    pub cwnd: u64,
    /// Total packets sent on this path
    pub sent_packets: u64,
    /// Total packets lost on this path
    pub lost_packets: u64,
    /// Total bytes sent on this path
    pub sent_bytes: u64,
    /// Total bytes received on this path
    pub recv_bytes: u64,
    /// Number of congestion events on this path
    pub congestion_events: u64,
    /// Number of packets sent containing only ACK frames
    pub sent_ack_only_packets: u64,
    /// Number of packets sent with PLPMTU probe frames
    pub sent_plpmtu_probes: u64,
    /// Number of packets lost with PLPMTU probe frames
    pub lost_plpmtu_probes: u64,
    /// Number of black hole events detected (when packets suddenly stop being delivered)
    pub black_hole_detected: u64,
}

impl From<quinn::ConnectionStats> for ConnectionStats {
    fn from(stats: quinn::ConnectionStats) -> Self {
        Self {
            rtt_ms: stats.path.rtt.as_millis() as u32,
            smoothed_rtt_ms: stats.path.rtt.as_millis() as u32, // PathStats.rtt is the smoothed RTT estimate
            latest_rtt_ms: 0,   // Not separately tracked in iroh-quinn
            rtt_variance_ms: 0, // Not separately tracked in iroh-quinn
            cwnd: stats.path.cwnd,
            sent_packets: stats.path.sent_packets,
            lost_packets: stats.path.lost_packets,
            sent_bytes: stats.udp_tx.bytes,
            recv_bytes: stats.udp_rx.bytes,
            congestion_events: stats.path.congestion_events,
            sent_ack_only_packets: 0, // Not available in iroh-quinn
            sent_plpmtu_probes: stats.path.sent_plpmtud_probes,
            lost_plpmtu_probes: stats.path.lost_plpmtud_probes,
            black_hole_detected: stats.path.black_holes_detected,
        }
    }
}

/// Aggregate test statistics across all streams
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestStats {
    /// Per-stream statistics
    pub per_stream_stats: Vec<StreamStats>,
    /// Total aggregate throughput across all streams (Mbps)
    pub aggregate_throughput_mbps: f64,
    /// Variance in throughput across streams
    pub throughput_variance: f64,
    /// Stream balance score (0-1, where 1 = perfectly balanced)
    pub stream_balance_score: f64,
    /// Minimum throughput observed across streams (Mbps)
    pub min_stream_throughput: f64,
    /// Maximum throughput observed across streams (Mbps)
    pub max_stream_throughput: f64,
    /// Connection-level statistics from quinn
    pub connection_stats: ConnectionStats,
}

impl TestStats {
    /// Calculate statistics from stream results and connection stats
    pub fn from_streams(streams: Vec<StreamStats>, connection_stats: ConnectionStats) -> Self {
        if streams.is_empty() {
            return Self {
                per_stream_stats: vec![],
                aggregate_throughput_mbps: 0.0,
                throughput_variance: 0.0,
                stream_balance_score: 1.0,
                min_stream_throughput: 0.0,
                max_stream_throughput: 0.0,
                connection_stats,
            };
        }

        // Calculate aggregate throughput (sum of all streams)
        let aggregate_throughput_mbps: f64 = streams.iter().map(|s| s.throughput_mbps).sum();

        // Find min and max throughput
        let min_stream_throughput = streams
            .iter()
            .map(|s| s.throughput_mbps)
            .min_by(|a, b| a.partial_cmp(b).unwrap())
            .unwrap_or(0.0);

        let max_stream_throughput = streams
            .iter()
            .map(|s| s.throughput_mbps)
            .max_by(|a, b| a.partial_cmp(b).unwrap())
            .unwrap_or(0.0);

        // Calculate mean throughput per stream
        let mean_throughput = aggregate_throughput_mbps / streams.len() as f64;

        // Calculate variance
        let throughput_variance = if streams.len() > 1 {
            let sum_squared_diff: f64 = streams
                .iter()
                .map(|s| (s.throughput_mbps - mean_throughput).powi(2))
                .sum();
            sum_squared_diff / streams.len() as f64
        } else {
            0.0
        };

        // Calculate balance score
        // Balance score is 1.0 if all streams have identical throughput
        // It decreases as the coefficient of variation increases
        let stream_balance_score = if mean_throughput > 0.0 && streams.len() > 1 {
            // Use coefficient of variation (CV) to measure relative variability
            let std_dev = throughput_variance.sqrt();
            let cv = std_dev / mean_throughput;
            // Convert CV to a 0-1 score where lower CV = higher score
            // Using exponential decay: score = e^(-2*CV)
            // This gives: CV=0 -> score=1, CV=0.35 -> score≈0.5, CV=1 -> score≈0.14
            (-2.0 * cv).exp().clamp(0.0, 1.0)
        } else {
            1.0
        };

        Self {
            per_stream_stats: streams,
            aggregate_throughput_mbps,
            throughput_variance,
            stream_balance_score,
            min_stream_throughput,
            max_stream_throughput,
            connection_stats,
        }
    }
}

/// Statistics tracker for the swarm
#[derive(Debug, Default)]
pub struct SwarmStats {
    pub tests_completed: AtomicU64,
    pub tests_failed: AtomicU64,
    pub bytes_transferred: AtomicU64,
}

impl SwarmStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn increment_completed(&self) {
        self.tests_completed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn increment_failed(&self) {
        self.tests_failed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn add_bytes_transferred(&self, bytes: u64) {
        self.bytes_transferred.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn get_completed(&self) -> u64 {
        self.tests_completed.load(Ordering::Relaxed)
    }

    pub fn get_failed(&self) -> u64 {
        self.tests_failed.load(Ordering::Relaxed)
    }

    pub fn get_bytes_transferred(&self) -> u64 {
        self.bytes_transferred.load(Ordering::Relaxed)
    }
}

/// Result for latency tests
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LatencyResult {
    pub avg_latency_ms: Option<f64>,
    pub min_latency_ms: Option<f64>,
    pub max_latency_ms: Option<f64>,
    pub std_dev_ms: Option<f64>,
    pub successful_pings: usize,
    pub total_iterations: u32,
    pub duration: Duration,
    pub error: Option<String>,
    /// Real connection type determined by iroh
    pub connection_type: Option<ConnectionType>,
}

/// Result for throughput tests
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThroughputResult {
    pub test_type: TestType,
    pub node_id: NodeId,
    pub duration: Duration,
    pub data_size_mb: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub transfer_duration: Option<Duration>,
    pub upload_mbps: f64,
    pub download_mbps: f64,
    pub parallel_streams: usize,
    pub chunk_size_kb: usize,
    pub statistics: Option<TestStats>,
    pub error: Option<String>,
    pub connection_type: Option<ConnectionType>,
}

impl Default for ThroughputResult {
    fn default() -> Self {
        Self {
            test_type: TestType::default(),
            node_id: NodeId::from_bytes(&[0; 32]).unwrap(),
            duration: Duration::ZERO,
            data_size_mb: 0,
            bytes_sent: 0,
            bytes_received: 0,
            transfer_duration: None,
            upload_mbps: 0.0,
            download_mbps: 0.0,
            parallel_streams: 0,
            chunk_size_kb: 0,
            statistics: None,
            error: None,
            connection_type: None,
        }
    }
}

impl ThroughputResult {
    /// Create a new ThroughputResult with basic parameters
    pub fn new(
        test_type: TestType,
        node_id: NodeId,
        duration: Duration,
        data_size: u64,
        parallel_streams: usize,
        chunk_size_bytes: usize,
    ) -> Self {
        Self {
            test_type,
            node_id,
            duration,
            data_size_mb: data_size / (1024 * 1024),
            parallel_streams,
            chunk_size_kb: chunk_size_bytes / 1024,
            ..Default::default()
        }
    }

    /// Set the connection type
    pub fn with_connection_type(mut self, conn_type: Option<ConnectionType>) -> Self {
        self.connection_type = conn_type;
        self
    }

    /// Set the transfer results
    pub fn with_transfer_results(
        mut self,
        bytes_sent: u64,
        bytes_received: u64,
        transfer_duration: Duration,
        upload_mbps: f64,
        download_mbps: f64,
    ) -> Self {
        self.bytes_sent = bytes_sent;
        self.bytes_received = bytes_received;
        self.transfer_duration = Some(transfer_duration);
        self.upload_mbps = upload_mbps;
        self.download_mbps = download_mbps;
        self
    }

    /// Set statistics
    pub fn with_statistics(mut self, statistics: Option<TestStats>) -> Self {
        self.statistics = statistics;
        self
    }
}

/// Result for fingerprint tests (combined)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FingerprintResult {
    pub test_type: TestType,
    pub node_id: NodeId,
    pub duration: Duration,
    pub latency: Option<LatencyResult>,
    pub throughput: Option<ThroughputResult>,
    pub error: Option<String>,
    pub connection_type: Option<ConnectionType>,
}

impl Default for FingerprintResult {
    fn default() -> Self {
        Self {
            test_type: TestType::default(),
            node_id: NodeId::from_bytes(&[0; 32]).unwrap(),
            duration: Duration::ZERO,
            latency: None,
            throughput: None,
            error: None,
            connection_type: None,
        }
    }
}

/// Generic error result
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ErrorResult {
    pub error: String,
    pub duration: Duration,
    pub test_type: Option<TestType>,
    pub node_id: Option<NodeId>,
    pub connection_type: Option<ConnectionType>,
}

/// Unified result type for all test assignments using tagged enum
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum TestAssignmentResult {
    Latency(LatencyResult),
    Throughput(ThroughputResult),
    Fingerprint(FingerprintResult),
    Error(ErrorResult),
}

impl TestAssignmentResult {
    pub fn duration(&self) -> Duration {
        match self {
            TestAssignmentResult::Latency(r) => r.duration,
            TestAssignmentResult::Throughput(r) => r.duration,
            TestAssignmentResult::Fingerprint(r) => r.duration,
            TestAssignmentResult::Error(r) => r.duration,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stats_calculation() {
        // Test with perfectly balanced streams
        let streams = vec![
            StreamStats {
                stream_id: 0,
                bytes_sent: 10_000_000,
                bytes_received: 10_000_000,
                duration: Duration::from_millis(1000),
                throughput_mbps: 80.0,
            },
            StreamStats {
                stream_id: 1,
                bytes_sent: 10_000_000,
                bytes_received: 10_000_000,
                duration: Duration::from_millis(1000),
                throughput_mbps: 80.0,
            },
        ];

        let stats = TestStats::from_streams(streams, ConnectionStats::default());
        assert_eq!(stats.aggregate_throughput_mbps, 160.0);
        assert_eq!(stats.throughput_variance, 0.0);
        assert_eq!(stats.stream_balance_score, 1.0);
        assert_eq!(stats.min_stream_throughput, 80.0);
        assert_eq!(stats.max_stream_throughput, 80.0);
    }

    #[test]
    fn test_stats_with_imbalance() {
        // Test with imbalanced streams
        let streams = vec![
            StreamStats {
                stream_id: 0,
                bytes_sent: 10_000_000,
                bytes_received: 10_000_000,
                duration: Duration::from_millis(1000),
                throughput_mbps: 100.0,
            },
            StreamStats {
                stream_id: 1,
                bytes_sent: 5_000_000,
                bytes_received: 5_000_000,
                duration: Duration::from_millis(1000),
                throughput_mbps: 40.0,
            },
        ];

        let stats = TestStats::from_streams(streams, ConnectionStats::default());
        assert_eq!(stats.aggregate_throughput_mbps, 140.0);
        assert!(stats.throughput_variance > 0.0);
        assert!(stats.stream_balance_score < 1.0);
        assert!(stats.stream_balance_score > 0.0);
        assert_eq!(stats.min_stream_throughput, 40.0);
        assert_eq!(stats.max_stream_throughput, 100.0);
    }

    #[test]
    fn test_empty_stats() {
        let stats = TestStats::from_streams(vec![], ConnectionStats::default());
        assert_eq!(stats.aggregate_throughput_mbps, 0.0);
        assert_eq!(stats.throughput_variance, 0.0);
        assert_eq!(stats.stream_balance_score, 1.0);
    }
}
