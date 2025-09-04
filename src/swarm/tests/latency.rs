//! Latency test implementation

use std::time::{Duration, Instant};

use anyhow::Result;
use iroh::{Endpoint, NodeId};
use tracing::{info, trace, warn};

use super::{
    connectivity::{get_connection_type, resolve_node_addr},
    protocol::{LatencyMessage, TestProtocolHeader, TestProtocolType, DOCTOR_SWARM_ALPN},
};
use crate::swarm::types::{LatencyResult, TestResult};

/// Run a latency test between two nodes
pub async fn run_latency_test(
    endpoint: &Endpoint,
    node_id: NodeId,
    iterations: u32,
) -> Result<TestResult<LatencyResult>> {
    run_latency_test_with_config(
        endpoint,
        node_id,
        iterations,
        Duration::from_millis(10),   // default interval
        Duration::from_millis(1000), // default timeout
    )
    .await
}

/// Run a latency test between two nodes with configurable timing
pub async fn run_latency_test_with_config(
    endpoint: &Endpoint,
    node_id: NodeId,
    iterations: u32,
    ping_interval: Duration,
    ping_timeout: Duration,
) -> Result<TestResult<LatencyResult>> {
    let start = Instant::now();

    // Resolve the node address
    let node_addr = resolve_node_addr(endpoint, node_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Failed to resolve node address for {}", node_id))?;

    info!(
        "Starting latency test to {} ({} iterations)",
        node_id, iterations
    );

    let conn = endpoint.connect(node_addr, DOCTOR_SWARM_ALPN).await?;

    let mut latencies = Vec::new();
    let mut failures = 0;

    for i in 0..iterations {
        match conn.open_bi().await {
            Ok((mut send, mut recv)) => {
                let ping_start = Instant::now();

                let ping_data = LatencyMessage::ping(i);
                let header =
                    TestProtocolHeader::new(TestProtocolType::Latency, ping_data.len() as u64);
                let header_bytes = header.to_bytes();

                if let Err(e) = send.write_all(&header_bytes).await {
                    warn!("Failed to send protocol header: {}", e);
                    failures += 1;
                    continue;
                }

                // Send ping
                if let Err(e) = send.write_all(ping_data.as_bytes()).await {
                    warn!("Failed to send ping {}: {}", i, e);
                    failures += 1;
                    continue;
                }

                if let Err(e) = send.finish() {
                    warn!("Failed to finish send stream: {}", e);
                    failures += 1;
                    continue;
                }

                // Read pong with timeout
                let mut buf = vec![0u8; 1024];
                match tokio::time::timeout(ping_timeout, recv.read(&mut buf)).await {
                    Ok(Ok(Some(n))) => {
                        let response = String::from_utf8_lossy(&buf[..n]);
                        if LatencyMessage::is_pong_response(&response) {
                            let latency = ping_start.elapsed();
                            latencies.push(latency);
                            trace!("Ping {} completed in {:?}", i, latency);
                        } else {
                            warn!("Unexpected response: {}", response);
                            failures += 1;
                        }
                    }
                    Ok(Ok(None)) => {
                        warn!("Stream closed without response");
                        failures += 1;
                    }
                    Ok(Err(e)) => {
                        warn!("Failed to read pong: {}", e);
                        failures += 1;
                    }
                    Err(_) => {
                        warn!("Ping {} timed out after {:?}", i, ping_timeout);
                        failures += 1;
                    }
                }
            }
            Err(e) => {
                warn!("Failed to open stream for ping {}: {}", i, e);
                failures += 1;
            }
        }

        // Configurable delay between pings
        if i < iterations - 1 {
            tokio::time::sleep(ping_interval).await;
        }
    }

    conn.close(0u32.into(), b"latency test complete");

    if latencies.is_empty() {
        return Ok(TestResult::failure(LatencyResult {
            avg_latency_ms: None,
            min_latency_ms: None,
            max_latency_ms: None,
            std_dev_ms: None,
            successful_pings: 0,
            failed_pings: failures,
            total_iterations: iterations,
            success_rate: None,
            duration: start.elapsed(),
            error: Some("No successful ping measurements".to_string()),
            connection_type: None, // Connection may have failed
        }));
    }

    let sum: Duration = latencies.iter().sum();
    let avg_latency = sum / latencies.len() as u32;
    let min_latency = latencies.iter().min().cloned().unwrap_or_default();
    let max_latency = latencies.iter().max().cloned().unwrap_or_default();

    // Calculate variance and std dev
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
        "Latency test complete: avg={:?}, min={:?}, max={:?}, success_rate={}%",
        avg_latency,
        min_latency,
        max_latency,
        (latencies.len() as f64 / iterations as f64) * 100.0
    );

    Ok(TestResult::success(LatencyResult {
        avg_latency_ms: Some(avg_latency.as_secs_f64() * 1000.0),
        min_latency_ms: Some(min_latency.as_secs_f64() * 1000.0),
        max_latency_ms: Some(max_latency.as_secs_f64() * 1000.0),
        std_dev_ms: Some(std_dev),
        successful_pings: latencies.len(),
        failed_pings: failures,
        total_iterations: iterations,
        success_rate: Some(latencies.len() as f64 / iterations as f64),
        duration: start.elapsed(),
        error: None,
        connection_type: get_connection_type(endpoint, node_id),
            ..Default::default()
    }))
}
