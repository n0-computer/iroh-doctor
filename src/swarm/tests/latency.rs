//! Latency test implementation

use std::time::{Duration, Instant};

use anyhow::Result;
use iroh::{endpoint::Connection, Endpoint, NodeId};
use tracing::{info, trace};

use super::protocol::{LatencyMessage, TestProtocolHeader, TestProtocolType, DOCTOR_SWARM_ALPN};
use crate::swarm::{execution::get_connection_type, types::LatencyResult};

/// Run a latency test between two nodes with configurable timing
pub async fn run_latency_test_with_config(
    endpoint: &Endpoint,
    node_id: NodeId,
    iterations: u32,
    ping_interval: Duration,
    ping_timeout: Duration,
) -> Result<LatencyResult> {
    info!(
        "Starting latency test to {} ({} iterations)",
        node_id, iterations
    );

    let conn = endpoint.connect(node_id, DOCTOR_SWARM_ALPN).await?;

    run_latency_test_on_connection(
        &conn,
        endpoint,
        node_id,
        iterations,
        ping_interval,
        ping_timeout,
    )
    .await
}

/// Run a latency test on an existing connection
pub async fn run_latency_test_on_connection(
    conn: &Connection,
    endpoint: &Endpoint,
    node_id: NodeId,
    iterations: u32,
    ping_interval: Duration,
    ping_timeout: Duration,
) -> Result<LatencyResult> {
    let start = Instant::now();
    let mut latencies = Vec::new();

    for i in 0..iterations {
        let (mut send, mut recv) = conn
            .open_bi()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to open stream for ping {}: {}", i, e))?;

        let ping_start = Instant::now();

        let ping_msg = LatencyMessage::ping(i);
        let ping_data = ping_msg.to_bytes();
        let header = TestProtocolHeader::new(TestProtocolType::Latency, ping_data.len() as u64);
        let header_bytes = header.to_bytes();

        send.write_all(&header_bytes)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send protocol header for ping {}: {}", i, e))?;

        send.write_all(&ping_data)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send ping {}: {}", i, e))?;

        send.finish()
            .map_err(|e| anyhow::anyhow!("Failed to finish send stream for ping {}: {}", i, e))?;

        let mut buf = vec![0u8; 1024];
        let n = match tokio::time::timeout(ping_timeout, recv.read(&mut buf)).await {
            Err(_) => {
                return Err(anyhow::anyhow!(
                    "Ping {} timed out after {:?}",
                    i,
                    ping_timeout
                ))
            }
            Ok(Err(e)) => return Err(anyhow::anyhow!("Connection closed, cannot continue: {}", e)),
            Ok(Ok(Some(n))) => n,
            Ok(Ok(None)) => {
                return Err(anyhow::anyhow!(
                    "Connection closed without response for ping {}",
                    i
                ))
            }
        };

        LatencyMessage::from_bytes(&buf[..n])
            .ok_or_else(|| anyhow::anyhow!("Failed to parse response for ping {}", i))?;

        let latency = ping_start.elapsed();
        latencies.push(latency);
        trace!("Ping {} completed in {:?}", i, latency);

        if i < iterations - 1 {
            tokio::time::sleep(ping_interval).await;
        }
    }

    conn.close(0u32.into(), b"latency test complete");

    let sum: Duration = latencies.iter().sum();
    let avg_latency = sum / latencies.len() as u32;
    let min_latency = latencies.iter().min().cloned().unwrap_or_default();
    let max_latency = latencies.iter().max().cloned().unwrap_or_default();

    // Use as_secs_f64() * 1000.0 instead of as_millis() to preserve sub-millisecond precision.
    // Network latencies can often be less than 1ms, and as_millis() would round them to 0 or 1.
    let avg_ms = avg_latency.as_secs_f64() * 1000.0;
    let variance = latencies
        .iter()
        .map(|l| {
            let ms = l.as_secs_f64() * 1000.0;
            (ms - avg_ms).powi(2)
        })
        .sum::<f64>()
        / latencies.len() as f64;
    let std_dev = variance.sqrt();

    info!(
        "Latency test complete: avg={:?}, min={:?}, max={:?}, {} pings",
        avg_latency, min_latency, max_latency, iterations
    );

    Ok(LatencyResult {
        avg_latency_ms: Some(avg_latency.as_secs_f64() * 1000.0),
        min_latency_ms: Some(min_latency.as_secs_f64() * 1000.0),
        max_latency_ms: Some(max_latency.as_secs_f64() * 1000.0),
        std_dev_ms: Some(std_dev),
        successful_pings: iterations as usize,
        total_iterations: iterations,
        duration: start.elapsed(),
        error: None,
        connection_type: get_connection_type(endpoint, node_id),
    })
}
