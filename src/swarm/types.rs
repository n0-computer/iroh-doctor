//! Types for swarm test statistics and results

use std::fmt;

use iroh::endpoint::ConnectionType;
use portable_atomic::{AtomicU64, Ordering};
use serde::{Deserialize, Serialize};

/// Custom serde module for u128 duration in milliseconds
/// Handles both integer and string values for backward compatibility
mod serde_duration_ms {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(value: &u128, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        value.to_string().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u128, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error;
        use serde_json::Value;

        let value = Value::deserialize(deserializer)?;
        match value {
            Value::Number(n) => {
                if let Some(int_val) = n.as_u64() {
                    Ok(int_val as u128)
                } else {
                    Err(Error::custom("Invalid number format"))
                }
            }
            Value::String(s) => s.parse().map_err(Error::custom),
            _ => Err(Error::custom("Expected number or string")),
        }
    }
}

/// Test types supported by doctor nodes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestType {
    Connectivity,
    Throughput,
    Latency,
    RelayPerformance,
    /// Combined test: connectivity + throughput + latency
    Fingerprint,
}

impl fmt::Display for TestType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TestType::Connectivity => write!(f, "connectivity"),
            TestType::Throughput => write!(f, "throughput"),
            TestType::Latency => write!(f, "latency"),
            TestType::RelayPerformance => write!(f, "relay_performance"),
            TestType::Fingerprint => write!(f, "fingerprint"),
        }
    }
}

/// Capability declaration for a specific test type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCapability {
    pub test_type: TestType,
    pub max_bandwidth_mbps: Option<u32>,
}

/// Configuration for a test run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConfig {
    pub test_type: TestType,
    pub duration_secs: Option<u64>,
    pub size_bytes: Option<u64>,
    pub iterations: Option<u32>,
    /// Advanced configuration options
    pub advanced: Option<AdvancedTestConfig>,
}

/// Advanced configuration for fine-tuning test parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThroughputAdvancedConfig {
    /// Number of parallel streams (default: 4, range: 1-16)
    pub parallel_streams: Option<u32>,
    /// Chunk size in KB (default: 64, range: 16-512)
    pub chunk_size_kb: Option<u32>,
}

/// Advanced configuration for latency tests
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyAdvancedConfig {
    /// Interval between pings in milliseconds (default: 10)
    pub ping_interval_ms: Option<u32>,
    /// Timeout for individual pings in milliseconds (default: 1000)
    pub ping_timeout_ms: Option<u32>,
}

/// Advanced network configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
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

impl DoctorCaps {
    /// Create capabilities for a test node
    pub fn test_node() -> Self {
        Self {
            can_register: true,
            can_report_results: true,
        }
    }
}

/// Statistics for a single stream in a throughput test
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamStats {
    /// Stream identifier (0-based index)
    pub stream_id: usize,
    /// Total bytes sent on this stream
    pub bytes_sent: u64,
    /// Total bytes received on this stream
    pub bytes_received: u64,
    /// Duration of the stream test in milliseconds
    pub duration_ms: f64,
    /// Calculated throughput in Mbps
    pub throughput_mbps: f64,
}

/// QUIC connection statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuicStats {
    /// Round-trip time in milliseconds
    pub rtt_ms: Option<f64>,
    /// Smoothed RTT in milliseconds
    pub smoothed_rtt_ms: Option<f64>,
    /// RTT variance
    pub rtt_variance_ms: Option<f64>,
    /// Number of congestion events
    pub congestion_events: Option<u64>,
    /// Packet loss rate (0.0 to 1.0)
    pub packet_loss_rate: Option<f64>,
    /// Connection-level throughput in Mbps
    pub connection_throughput_mbps: Option<f64>,
    /// Congestion window size in bytes
    pub congestion_window: Option<u64>,
    /// Number of packets sent
    pub packets_sent: Option<u64>,
    /// Number of packets lost
    pub packets_lost: Option<u64>,
    /// Number of bytes sent
    pub bytes_sent: Option<u64>,
    /// Number of bytes received  
    pub bytes_received: Option<u64>,
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
    /// QUIC connection statistics (optional)
    pub quic_stats: Option<QuicStats>,
}

impl TestStats {
    /// Calculate statistics from stream results
    pub fn from_streams(streams: Vec<StreamStats>) -> Self {
        if streams.is_empty() {
            return Self {
                per_stream_stats: vec![],
                aggregate_throughput_mbps: 0.0,
                throughput_variance: 0.0,
                stream_balance_score: 1.0,
                min_stream_throughput: 0.0,
                max_stream_throughput: 0.0,
                quic_stats: None,
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
            quic_stats: None, // Will be populated by the throughput test
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

/// Base result type for all test outcomes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult<T> {
    pub success: bool,
    pub data: T,
}

impl<T> TestResult<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data,
        }
    }

    pub fn failure(data: T) -> Self {
        Self {
            success: false,
            data,
        }
    }
}

/// Result for connectivity tests
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectivityResult {
    pub connected: bool,
    pub connection_time_ms: Option<u64>,
    pub peer: String,
    #[serde(with = "serde_duration_ms")]
    pub duration_ms: u128,
    pub error: Option<String>,
    /// Real connection type determined by iroh (only available when connected=true)
    pub connection_type: Option<ConnectionType>,
}

/// Result for latency tests
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyResult {
    pub avg_latency_ms: Option<f64>,
    pub min_latency_ms: Option<f64>,
    pub max_latency_ms: Option<f64>,
    pub std_dev_ms: Option<f64>,
    pub successful_pings: usize,
    pub failed_pings: u32,
    pub total_iterations: u32,
    pub success_rate: Option<f64>,
    #[serde(with = "serde_duration_ms")]
    pub duration_ms: u128,
    pub error: Option<String>,
    /// Real connection type determined by iroh
    pub connection_type: Option<ConnectionType>,
}

/// Result for throughput tests
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThroughputResult {
    pub test_type: TestType,
    pub peer: String,
    #[serde(with = "serde_duration_ms")]
    pub duration_ms: u128,
    pub data_size_mb: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub transfer_duration_ms: Option<u64>,
    pub throughput_mbps: f64, // For backward compatibility
    pub upload_mbps: f64,
    pub download_mbps: f64,
    pub parallel_streams: usize,
    pub chunk_size_kb: usize,
    pub statistics: Option<TestStats>,
    pub error: Option<String>,
    /// Real connection type determined by iroh
    pub connection_type: Option<ConnectionType>,
}

/// Result for fingerprint tests (combined)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FingerprintResult {
    pub test_type: TestType,
    pub peer: String,
    #[serde(with = "serde_duration_ms")]
    pub duration_ms: u128,
    pub connectivity: Option<ConnectivityResult>,
    pub latency: Option<LatencyResult>,
    pub throughput: Option<ThroughputResult>,
    pub error: Option<String>,
}

/// Generic error result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResult {
    pub error: String,
    #[serde(with = "serde_duration_ms")]
    pub duration_ms: u128,
    pub test_type: Option<TestType>,
    pub peer: Option<String>,
}

/// Internal skip result (when acting as responder)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InternalSkipResult {
    pub reason: String,
}

/// Unified result type for all test assignments
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result_type")]
#[allow(clippy::large_enum_variant)]
pub enum TestAssignmentResult {
    Connectivity(ConnectivityResult),
    Latency(LatencyResult),
    Throughput(ThroughputResult),
    Fingerprint(FingerprintResult),
    InternalSkip(InternalSkipResult),
    Error(ErrorResult),
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
                duration_ms: 1000.0,
                throughput_mbps: 80.0,
            },
            StreamStats {
                stream_id: 1,
                bytes_sent: 10_000_000,
                bytes_received: 10_000_000,
                duration_ms: 1000.0,
                throughput_mbps: 80.0,
            },
        ];

        let stats = TestStats::from_streams(streams);
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
                duration_ms: 1000.0,
                throughput_mbps: 100.0,
            },
            StreamStats {
                stream_id: 1,
                bytes_sent: 5_000_000,
                bytes_received: 5_000_000,
                duration_ms: 1000.0,
                throughput_mbps: 40.0,
            },
        ];

        let stats = TestStats::from_streams(streams);
        assert_eq!(stats.aggregate_throughput_mbps, 140.0);
        assert!(stats.throughput_variance > 0.0);
        assert!(stats.stream_balance_score < 1.0);
        assert!(stats.stream_balance_score > 0.0);
        assert_eq!(stats.min_stream_throughput, 40.0);
        assert_eq!(stats.max_stream_throughput, 100.0);
    }

    #[test]
    fn test_empty_stats() {
        let stats = TestStats::from_streams(vec![]);
        assert_eq!(stats.aggregate_throughput_mbps, 0.0);
        assert_eq!(stats.throughput_variance, 0.0);
        assert_eq!(stats.stream_balance_score, 1.0);
    }
}
