//! Throughput test implementations

use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::protocol::{TestProtocolHeader, TestProtocolType};
use crate::swarm::types::{QuicStats, StreamStats, TestStats};

/// Result of a bidirectional throughput test
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BidirectionalThroughputResult {
    pub upload_mbps: f64,
    pub download_mbps: f64,
    pub upload_duration_ms: u64,
    pub download_duration_ms: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub duration_ms: u64, // Total test duration
    /// Detailed statistics
    pub statistics: Option<TestStats>,
}

/// Default number of parallel streams if not configured
const DEFAULT_PARALLEL_STREAMS: usize = 4;
/// Default chunk size in bytes (64KB)
const DEFAULT_CHUNK_SIZE: usize = 65536;

/// Run a bidirectional throughput test using parallel streams
pub async fn run_bidirectional_throughput_test(
    conn: &iroh::endpoint::Connection,
    data_size: u64,
    _is_initiator: bool,
) -> Result<BidirectionalThroughputResult> {
    run_bidirectional_throughput_test_with_config(
        conn,
        data_size,
        _is_initiator,
        DEFAULT_PARALLEL_STREAMS,
        DEFAULT_CHUNK_SIZE,
    )
    .await
}

/// Run a bidirectional throughput test with custom configuration
pub async fn run_bidirectional_throughput_test_with_config(
    conn: &iroh::endpoint::Connection,
    data_size: u64,
    _is_initiator: bool,
    parallel_streams: usize,
    chunk_size: usize,
) -> Result<BidirectionalThroughputResult> {
    let test_start = Instant::now();

    info!(
        "Starting bidirectional throughput test (size: {} bytes, {} parallel streams, chunk: {} bytes)",
        data_size, parallel_streams, chunk_size
    );

    // Calculate data size per stream
    let data_per_stream = data_size / parallel_streams as u64;
    let last_stream_extra = data_size % parallel_streams as u64;

    // Create tasks for parallel streams
    let mut stream_tasks = Vec::new();

    for stream_idx in 0..parallel_streams {
        let conn = conn.clone();
        let stream_data_size = if stream_idx == parallel_streams - 1 {
            data_per_stream + last_stream_extra
        } else {
            data_per_stream
        };

        let task = tokio::spawn(async move {
            run_single_stream_test_with_config(conn, stream_idx, stream_data_size, chunk_size).await
        });

        stream_tasks.push(task);
    }

    // Wait for all streams to complete
    let mut total_bytes_sent = 0u64;
    let mut total_bytes_received = 0u64;
    let mut max_upload_duration = Duration::ZERO;
    let mut max_download_duration = Duration::ZERO;
    let mut errors = Vec::new();
    let mut stream_stats = Vec::new();

    for (idx, task) in stream_tasks.into_iter().enumerate() {
        match task.await {
            Ok(Ok((bytes_sent, bytes_received, upload_duration, download_duration))) => {
                total_bytes_sent += bytes_sent;
                total_bytes_received += bytes_received;
                max_upload_duration = max_upload_duration.max(upload_duration);
                max_download_duration = max_download_duration.max(download_duration);

                // Calculate stream throughput
                let stream_duration_ms =
                    upload_duration.max(download_duration).as_secs_f64() * 1000.0;
                let stream_throughput_mbps = if stream_duration_ms > 0.0 {
                    let total_bytes = bytes_sent + bytes_received;
                    (total_bytes as f64 * 8.0) / (stream_duration_ms / 1000.0) / 1_000_000.0
                } else {
                    0.0
                };

                stream_stats.push(StreamStats {
                    stream_id: idx,
                    bytes_sent,
                    bytes_received,
                    duration_ms: stream_duration_ms,
                    throughput_mbps: stream_throughput_mbps,
                });

                debug!(
                    "Stream {} completed: sent={}, received={}, upload_duration={:?}, download_duration={:?}, throughput={:.2} Mbps",
                    idx, bytes_sent, bytes_received, upload_duration, download_duration, stream_throughput_mbps
                );
            }
            Ok(Err(e)) => {
                warn!("Stream {} failed: {}", idx, e);
                errors.push(format!("Stream {idx} error: {e}"));
            }
            Err(e) => {
                warn!("Stream {} task panicked: {}", idx, e);
                errors.push(format!("Stream {idx} panic: {e}"));
            }
        }
    }

    // Check if too many streams failed
    if errors.len() > parallel_streams / 2 {
        return Err(anyhow::anyhow!(
            "Too many streams failed: {}",
            errors.join(", ")
        ));
    }

    // Calculate speeds based on the longest stream duration (since they run in parallel)
    let upload_mbps = if max_upload_duration.as_secs_f64() > 0.0 {
        (total_bytes_sent as f64 * 8.0) / max_upload_duration.as_secs_f64() / 1_000_000.0
    } else {
        0.0
    };

    let download_mbps = if max_download_duration.as_secs_f64() > 0.0 {
        (total_bytes_received as f64 * 8.0) / max_download_duration.as_secs_f64() / 1_000_000.0
    } else {
        0.0
    };

    let total_duration = test_start.elapsed();

    // Calculate test statistics if we have stream data
    let statistics = if !stream_stats.is_empty() {
        let mut stats = TestStats::from_streams(stream_stats);

        // Collect QUIC stats
        let quic_stats = QuicStats {
            rtt_ms: None,
            smoothed_rtt_ms: None,
            rtt_variance_ms: None,
            congestion_events: None,
            packet_loss_rate: if total_bytes_sent > 0 {
                // Estimate packet loss from difference between sent and received
                // Expected total: data_size sent to server + data_size received from server
                let expected_total = data_size * 2;
                let actual_total = total_bytes_sent + total_bytes_received;
                if actual_total < expected_total {
                    Some(1.0 - (actual_total as f64 / expected_total as f64))
                } else {
                    Some(0.0)
                }
            } else {
                None
            },
            connection_throughput_mbps: Some(
                ((total_bytes_sent + total_bytes_received) as f64 * 8.0)
                    / total_duration.as_secs_f64()
                    / 1_000_000.0,
            ),
            congestion_window: None,
            packets_sent: None,
            packets_lost: None,
            bytes_sent: Some(total_bytes_sent),
            bytes_received: Some(total_bytes_received),
        };

        stats.quic_stats = Some(quic_stats);
        Some(stats)
    } else {
        None
    };

    info!(
        "Bidirectional test complete - Upload: {:.2} Mbps ({:?}), Download: {:.2} Mbps ({:?})",
        upload_mbps, max_upload_duration, download_mbps, max_download_duration
    );

    if let Some(ref stats) = statistics {
        info!(
            "Stream statistics - Balance: {:.2}, Min: {:.2} Mbps, Max: {:.2} Mbps, Variance: {:.2}",
            stats.stream_balance_score,
            stats.min_stream_throughput,
            stats.max_stream_throughput,
            stats.throughput_variance
        );

        if let Some(ref quic) = stats.quic_stats {
            info!(
                "QUIC statistics - Connection throughput: {:.2} Mbps, Packet loss: {:.2}%, Bytes sent: {}, Bytes received: {}",
                quic.connection_throughput_mbps.unwrap_or(0.0),
                quic.packet_loss_rate.unwrap_or(0.0) * 100.0,
                quic.bytes_sent.unwrap_or(0),
                quic.bytes_received.unwrap_or(0)
            );
        }
    }

    if !errors.is_empty() {
        warn!("Test completed with errors: {}", errors.join(", "));
    }

    Ok(BidirectionalThroughputResult {
        upload_mbps,
        download_mbps,
        upload_duration_ms: max_upload_duration.as_millis() as u64,
        download_duration_ms: max_download_duration.as_millis() as u64,
        bytes_sent: total_bytes_sent,
        bytes_received: total_bytes_received,
        duration_ms: total_duration.as_millis() as u64,
        statistics,
    })
}

/// Run a single stream test with custom config
async fn run_single_stream_test_with_config(
    conn: iroh::endpoint::Connection,
    stream_idx: usize,
    data_size: u64,
    chunk_size: usize,
) -> Result<(u64, u64, Duration, Duration)> {
    debug!(
        "Starting stream {} with {} bytes, chunk size {} bytes",
        stream_idx, data_size, chunk_size
    );

    // Open a bidirectional stream
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to open stream {}: {}", stream_idx, e))?;

    // Send protocol header with chunk size config
    // Note: parallel_streams is not needed per-stream since each stream is independent
    let header =
        TestProtocolHeader::with_config(TestProtocolType::Throughput, data_size, 1, chunk_size);
    let header_bytes = header.to_bytes();
    send.write_all(&header_bytes)
        .await
        .map_err(|e| anyhow::anyhow!("Stream {} failed to send header: {}", stream_idx, e))?;

    // Sequential bidirectional test: first upload, then download

    // Upload phase
    let upload_start = Instant::now();
    let send_chunk = vec![0u8; chunk_size];
    let mut bytes_sent = 0u64;

    while bytes_sent < data_size {
        let to_send = chunk_size.min((data_size - bytes_sent) as usize);
        match send.write_all(&send_chunk[..to_send]).await {
            Ok(_) => {
                bytes_sent += to_send as u64;

                // Log progress periodically
                if bytes_sent % (10 * 1024 * 1024) == 0 || bytes_sent >= data_size {
                    debug!(
                        "Stream {} upload progress: {:.1} MB sent ({:.1}%)",
                        stream_idx,
                        bytes_sent as f64 / (1024.0 * 1024.0),
                        (bytes_sent as f64 / data_size as f64) * 100.0
                    );
                }
            }
            Err(e) => {
                warn!("Stream {} upload error: {}", stream_idx, e);
                break;
            }
        }
    }

    // Signal end of upload
    send.finish().ok();
    let upload_duration = upload_start.elapsed();
    debug!(
        "Stream {} upload complete: {} bytes in {:?}",
        stream_idx, bytes_sent, upload_duration
    );

    // Download phase
    let download_start = Instant::now();
    let mut recv_buf = vec![0u8; chunk_size];
    let mut bytes_received = 0u64;

    while bytes_received < data_size {
        match recv.read(&mut recv_buf).await {
            Ok(Some(n)) => {
                bytes_received += n as u64;

                // Log progress periodically
                if bytes_received % (10 * 1024 * 1024) == 0 || bytes_received >= data_size {
                    debug!(
                        "Stream {} download progress: {:.1} MB received ({:.1}%)",
                        stream_idx,
                        bytes_received as f64 / (1024.0 * 1024.0),
                        (bytes_received as f64 / data_size as f64) * 100.0
                    );
                }
            }
            Ok(None) => {
                debug!(
                    "Stream {} closed after receiving {} bytes",
                    stream_idx, bytes_received
                );
                break;
            }
            Err(e) => {
                warn!("Stream {} download error: {}", stream_idx, e);
                break;
            }
        }
    }

    let download_duration = download_start.elapsed();
    debug!(
        "Stream {} download complete: {} bytes in {:?}",
        stream_idx, bytes_received, download_duration
    );

    debug!(
        "Stream {} completed: sent={} bytes in {:?}, received={} bytes in {:?}",
        stream_idx, bytes_sent, upload_duration, bytes_received, download_duration
    );

    Ok((
        bytes_sent,
        bytes_received,
        upload_duration,
        download_duration,
    ))
}
