//! Throughput test implementations

use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use super::protocol::{TestProtocolHeader, TestProtocolType};
use crate::swarm::{
    transfer_utils::{drain_stream, send_data_on_stream},
    types::{StreamStats, TestStats},
};

/// Result of a bidirectional throughput test
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BidirectionalThroughputResult {
    pub upload_mbps: f64,
    pub download_mbps: f64,
    pub upload_duration: Duration,
    pub download_duration: Duration,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub duration: Duration,
    pub statistics: Option<TestStats>,
}

/// Run a bidirectional throughput test with custom configuration
pub async fn run_bidirectional_throughput_test_with_config(
    conn: &iroh::endpoint::Connection,
    data_size: u64,
    parallel_streams: usize,
    chunk_size: usize,
) -> Result<BidirectionalThroughputResult> {
    let test_start = Instant::now();

    info!(
        "Starting bidirectional throughput test (size: {} bytes, {} parallel streams, chunk: {} bytes)",
        data_size, parallel_streams, chunk_size
    );

    let data_per_stream = data_size / parallel_streams as u64;
    let last_stream_extra = data_size % parallel_streams as u64;

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

    let mut total_bytes_sent = 0u64;
    let mut total_bytes_received = 0u64;
    let mut max_upload_duration = Duration::ZERO;
    let mut max_download_duration = Duration::ZERO;
    let mut stream_stats = Vec::new();

    for (idx, task) in stream_tasks.into_iter().enumerate() {
        match task.await {
            Ok(Ok((bytes_sent, bytes_received, upload_duration, download_duration))) => {
                total_bytes_sent += bytes_sent;
                total_bytes_received += bytes_received;
                max_upload_duration = max_upload_duration.max(upload_duration);
                max_download_duration = max_download_duration.max(download_duration);

                // Since upload and download are sequential, sum the durations
                let stream_duration = upload_duration + download_duration;
                let stream_throughput_mbps = if !stream_duration.is_zero() {
                    let total_bytes = bytes_sent + bytes_received;
                    (total_bytes as f64 * 8.0) / stream_duration.as_secs_f64() / 1_000_000.0
                } else {
                    0.0
                };

                stream_stats.push(StreamStats {
                    stream_id: idx,
                    bytes_sent,
                    bytes_received,
                    upload_duration,
                    download_duration,
                    throughput_mbps: stream_throughput_mbps,
                });

                debug!(
                    "Stream {} completed: sent={}, received={}, upload_duration={:?}, download_duration={:?}, throughput={:.2} Mbps",
                    idx, bytes_sent, bytes_received, upload_duration, download_duration, stream_throughput_mbps
                );
            }
            Ok(Err(e)) => {
                return Err(anyhow::anyhow!("Stream {} failed: {}", idx, e));
            }
            Err(e) => {
                return Err(anyhow::anyhow!("Stream {} task panicked: {}", idx, e));
            }
        }
    }

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

    let statistics = if !stream_stats.is_empty() {
        let connection_stats = conn.stats().into();
        let stats = TestStats::from_streams(stream_stats, connection_stats);
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

        let conn_stats = &stats.connection_stats;
        info!(
            "Connection stats - RTT: {}ms, Congestion window: {} bytes, Packets sent: {}, Packets lost: {}, Loss rate: {:.2}%",
            conn_stats.rtt_ms,
            conn_stats.cwnd,
            conn_stats.sent_packets,
            conn_stats.lost_packets,
            if conn_stats.sent_packets > 0 {
                (conn_stats.lost_packets as f64 / conn_stats.sent_packets as f64) * 100.0
            } else {
                0.0
            }
        );
        info!(
            "Connection bytes - Sent: {} bytes, Received: {} bytes",
            conn_stats.sent_bytes, conn_stats.recv_bytes
        );
    }

    Ok(BidirectionalThroughputResult {
        upload_mbps,
        download_mbps,
        upload_duration: max_upload_duration,
        download_duration: max_download_duration,
        bytes_sent: total_bytes_sent,
        bytes_received: total_bytes_received,
        duration: total_duration,
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

    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to open stream {}: {}", stream_idx, e))?;

    let header =
        TestProtocolHeader::with_config(TestProtocolType::Throughput, data_size, 1, chunk_size);
    let header_bytes = header.to_bytes();
    send.write_all(&header_bytes)
        .await
        .map_err(|e| anyhow::anyhow!("Stream {} failed to send header: {}", stream_idx, e))?;

    let upload_start = Instant::now();
    send_data_on_stream(&mut send, data_size, chunk_size)
        .await
        .map_err(|e| anyhow::anyhow!("Stream {} upload failed: {}", stream_idx, e))?;
    let upload_duration = upload_start.elapsed();

    let download_start = Instant::now();
    let (bytes_received, _time_to_first_byte, _num_chunks) =
        drain_stream(&mut recv, false)
            .await
            .map_err(|e| anyhow::anyhow!("Stream {} download failed: {}", stream_idx, e))?;
    let download_duration = download_start.elapsed();

    let bytes_sent = data_size;
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
        bytes_received as u64,
        upload_duration,
        download_duration,
    ))
}
