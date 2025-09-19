//! Test execution and swarm runner implementation

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use iroh::{endpoint::ConnectionType, Endpoint, NodeId, Watcher};
use rand::Rng;
use tracing::{info, warn};

use crate::swarm::{
    rpc::TestAssignment,
    tests::{
        latency::{self, run_latency_test_with_config},
        protocol::DOCTOR_SWARM_ALPN,
        throughput::run_bidirectional_throughput_test_with_config,
    },
    types::{
        ErrorResult, FingerprintResult, LatencyResult, TestAssignmentResult, TestType,
        ThroughputResult,
    },
};

/// Helper function to get the real connection type from the endpoint
fn get_connection_type(endpoint: &Endpoint, node_id: NodeId) -> Option<ConnectionType> {
    endpoint.conn_type(node_id).map(|mut watcher| {
        // Get the current connection type from the watcher
        watcher.get()
    })
}

/// Extract throughput configuration with defaults
fn extract_throughput_config(assignment: &TestAssignment) -> (usize, usize) {
    let throughput_config = &assignment
        .test_config
        .advanced
        .as_ref()
        .and_then(|c| c.throughput.as_ref());

    let parallel_streams = throughput_config
        .as_ref()
        .and_then(|c| c.parallel_streams)
        .unwrap_or(4) // default: 4 streams
        .clamp(1, 16) as usize; // enforce range: 1-16

    let chunk_size_kb = throughput_config
        .as_ref()
        .and_then(|c| c.chunk_size_kb)
        .unwrap_or(1024) // default: 1MB
        .clamp(1, 16 * 1024); // enforce range: 1KB-16MB

    let chunk_size_bytes = (chunk_size_kb as usize) * 1024;

    (parallel_streams, chunk_size_bytes)
}

/// Extract connection timeout from network config
fn extract_connection_timeout(assignment: &TestAssignment) -> Duration {
    assignment
        .test_config
        .advanced
        .as_ref()
        .and_then(|a| a.network.as_ref())
        .and_then(|n| n.connection_timeout_secs.as_ref())
        .map(|secs| Duration::from_secs(*secs as u64))
        .unwrap_or_else(|| Duration::from_secs(20)) // default: 20 seconds
}

/// Extract latency test configuration with defaults
fn extract_latency_config(assignment: &TestAssignment) -> (Duration, Duration) {
    let advanced_config = assignment
        .test_config
        .advanced
        .as_ref()
        .and_then(|a| a.latency.as_ref());

    let ping_interval_ms = advanced_config
        .as_ref()
        .and_then(|c| c.ping_interval_ms)
        .unwrap_or(10) // default: 10ms
        .clamp(1, 1000); // enforce range: 1-1000ms

    let ping_timeout_ms = advanced_config
        .as_ref()
        .and_then(|c| c.ping_timeout_ms)
        .unwrap_or(1000) // default: 1000ms (1 second)
        .clamp(100, 10000); // enforce range: 100ms-10s

    (
        Duration::from_millis(ping_interval_ms as u64),
        Duration::from_millis(ping_timeout_ms as u64),
    )
}

// Function to perform test assignment
pub async fn perform_test_assignment(
    assignment: TestAssignment,
    endpoint: Arc<Endpoint>,
    _node_id: NodeId,
    data_transfer_timeout: Duration,
) -> Result<TestAssignmentResult> {
    let start = Instant::now();

    match assignment.test_type {
        TestType::Throughput => {
            execute_throughput_test(assignment, endpoint, start, data_transfer_timeout).await
        }
        TestType::Latency => execute_latency_test(assignment, endpoint, start).await,
        TestType::Fingerprint => {
            execute_fingerprint_test(assignment, endpoint, start, data_transfer_timeout).await
        }
    }
}

async fn execute_throughput_test(
    assignment: TestAssignment,
    endpoint: Arc<Endpoint>,
    start: Instant,
    data_transfer_timeout: Duration,
) -> Result<TestAssignmentResult> {
    info!("Starting throughput test with node {}", assignment.node_id);

    // Try multiple times to resolve and connect
    let mut last_error = String::from("No connection attempt made");
    for attempt in 1..=3 {
        if attempt > 1 {
            info!("Throughput test connection attempt {} of 3", attempt);
            tokio::time::sleep(Duration::from_secs(5)).await;
        }

        // Extract configuration
        let connection_timeout = extract_connection_timeout(&assignment);

        // Try to connect directly with NodeId - endpoint handles discovery
        info!(
            "Attempting to connect to node {} (attempt {})",
            assignment.node_id, attempt
        );
        match tokio::time::timeout(
            connection_timeout,
            endpoint.connect(assignment.node_id, DOCTOR_SWARM_ALPN),
        )
        .await
        {
            Ok(Ok(conn)) => {
                // Connection successful, proceed with test
                // Use configured size or generate random size
                let data_size = assignment
                    .test_config
                    .size_bytes
                    .unwrap_or_else(|| rand::thread_rng().gen_range(10..=50) * 1024 * 1024);

                // Extract advanced configuration
                let (parallel_streams, chunk_size_bytes) = extract_throughput_config(&assignment);

                info!(
                    "Running throughput test with {} parallel streams and {} KB chunks",
                    parallel_streams,
                    chunk_size_bytes / 1024
                );

                // Run the bidirectional throughput test with timeout using the connection
                let transfer_result = tokio::time::timeout(
                    data_transfer_timeout,
                    run_bidirectional_throughput_test_with_config(
                        &conn,
                        data_size,
                        true,
                        parallel_streams,
                        chunk_size_bytes,
                    ),
                )
                .await;

                match transfer_result {
                    Ok(Ok(result)) => {
                        conn.close(0u32.into(), b"test complete");

                        info!(
                                    "Bidirectional throughput test completed: Upload: {:.2} Mbps, Download: {:.2} Mbps",
                                    result.upload_mbps,
                                    result.download_mbps
                                );

                        let throughput_result = ThroughputResult {
                            test_type: assignment.test_type,
                            node_id: assignment.node_id,
                            duration: start.elapsed(),
                            data_size_mb: data_size / (1024 * 1024),
                            bytes_sent: result.bytes_sent,
                            bytes_received: result.bytes_received,
                            transfer_duration: Some(result.duration),
                            upload_mbps: result.upload_mbps,
                            download_mbps: result.download_mbps,
                            parallel_streams,
                            chunk_size_kb: chunk_size_bytes / 1024,
                            statistics: result.statistics,
                            connection_type: get_connection_type(&endpoint, assignment.node_id),
                            ..Default::default()
                        };

                        return Ok(TestAssignmentResult::Throughput(throughput_result));
                    }
                    Ok(Err(e)) => {
                        warn!("Throughput test error: {}", e);
                        return Ok(TestAssignmentResult::Error(ErrorResult {
                            error: e.to_string(),
                            duration: start.elapsed(),
                            test_type: Some(assignment.test_type),
                            node_id: Some(assignment.node_id),
                        }));
                    }
                    Err(_) => {
                        warn!("Throughput test timed out");
                        return Ok(TestAssignmentResult::Error(ErrorResult {
                            error: "Test timed out after 30 seconds".to_string(),
                            duration: start.elapsed(),
                            test_type: Some(assignment.test_type),
                            node_id: Some(assignment.node_id),
                        }));
                    }
                }
            }
            Ok(Err(e)) => {
                last_error = format!("Failed to connect: {e} (attempt {attempt})");
                warn!("{}", last_error);
                continue;
            }
            Err(_) => {
                last_error = format!("Connection attempt {attempt} timed out after 20 seconds");
                warn!("{}", last_error);
                continue;
            }
        }
    }

    // All attempts failed
    warn!("All throughput test connection attempts failed");
    Ok(TestAssignmentResult::Error(ErrorResult {
        error: last_error,
        duration: start.elapsed(),
        test_type: Some(assignment.test_type),
        node_id: Some(assignment.node_id),
    }))
}

async fn execute_latency_test(
    assignment: TestAssignment,
    endpoint: Arc<Endpoint>,
    start: Instant,
) -> Result<TestAssignmentResult> {
    // Add retry logic for latency tests
    let iterations = assignment.test_config.iterations.unwrap_or(10);

    let max_attempts = 3;
    let mut last_error: Option<TestAssignmentResult> = None;

    for attempt in 1..=max_attempts {
        if attempt > 1 {
            info!("Latency test attempt {} of {}", attempt, max_attempts);
            tokio::time::sleep(Duration::from_secs(3)).await;
        }

        // Extract latency configuration
        let (ping_interval, ping_timeout) = extract_latency_config(&assignment);
        info!(
            "Running latency test with {}ms interval and {}ms timeout",
            ping_interval.as_millis(),
            ping_timeout.as_millis()
        );

        match run_latency_test_with_config(
            &endpoint,
            assignment.node_id,
            iterations,
            ping_interval,
            ping_timeout,
        )
        .await
        {
            Ok(result) => {
                return Ok(TestAssignmentResult::Latency(result));
            }
            Err(e) => {
                last_error = Some(TestAssignmentResult::Error(ErrorResult {
                    error: e.to_string(),
                    duration: start.elapsed(),
                    test_type: Some(assignment.test_type),
                    node_id: Some(assignment.node_id),
                }));
                if attempt < max_attempts {
                    warn!("Latency test error: {}, retrying...", e);
                }
            }
        }
    }

    // All attempts failed
    warn!("All latency test attempts failed");
    Ok(last_error.unwrap_or_else(|| {
        TestAssignmentResult::Error(ErrorResult {
            error: "All latency test attempts failed".to_string(),
            duration: start.elapsed(),
            test_type: Some(assignment.test_type),
            node_id: Some(assignment.node_id),
        })
    }))
}

async fn execute_fingerprint_test(
    assignment: TestAssignment,
    endpoint: Arc<Endpoint>,
    start: Instant,
    data_transfer_timeout: Duration,
) -> Result<TestAssignmentResult> {
    // Combined test: latency + throughput
    info!("Starting fingerprint test with node {}", assignment.node_id);

    // 1. Latency test (also proves connectivity)
    info!("Phase 1/2: Testing latency...");
    let iterations = 10;
    let (ping_interval, ping_timeout) = extract_latency_config(&assignment);
    info!(
        "Running fingerprint latency test with {}ms interval and {}ms timeout",
        ping_interval.as_millis(),
        ping_timeout.as_millis()
    );

    let latency_result = match latency::run_latency_test_with_config(
        &endpoint,
        assignment.node_id,
        iterations,
        ping_interval,
        ping_timeout,
    )
    .await
    {
        Ok(result) => Some(result),
        Err(e) => {
            // If latency test fails, we can't establish connectivity, so skip throughput
            warn!("Latency test failed: {}", e);
            return Ok(TestAssignmentResult::Fingerprint(FingerprintResult {
                test_type: assignment.test_type,
                node_id: assignment.node_id,
                duration: start.elapsed(),
                latency: Some(LatencyResult {
                    avg_latency_ms: None,
                    min_latency_ms: None,
                    max_latency_ms: None,
                    std_dev_ms: None,
                    successful_pings: 0,
                    total_iterations: iterations,
                    duration: start.elapsed(),
                    error: Some(format!("Latency test failed: {e}")),
                    connection_type: None,
                }),
                throughput: None,
                error: Some(format!("Test failed during latency phase: {e}")),
            }));
        }
    };

    // 2. Throughput test - Need to establish a connection for this
    info!("Phase 2/2: Testing throughput...");

    // TODO: The latency test establishes its own connection internally but doesn't return it.
    // This means we create a second connection here, which is inefficient.
    // Future improvement: refactor latency test to optionally return its connection.
    // Connect for throughput test
    let connection = match endpoint
        .connect(assignment.node_id, DOCTOR_SWARM_ALPN)
        .await
    {
        Ok(conn) => conn,
        Err(e) => {
            warn!("Failed to connect for throughput test: {}", e);
            // Return with just latency results
            return Ok(TestAssignmentResult::Fingerprint(FingerprintResult {
                test_type: assignment.test_type,
                node_id: assignment.node_id,
                duration: start.elapsed(),
                latency: latency_result,
                throughput: None,
                error: Some(format!("Throughput test skipped: connection failed: {e}")),
            }));
        }
    };

    // Use configured size or default
    let data_size = assignment
        .test_config
        .size_bytes
        .unwrap_or(10 * 1024 * 1024); // 10MB default for fingerprint

    // Extract advanced configuration for consistent behavior with regular throughput test
    let (parallel_streams, chunk_size_bytes) = extract_throughput_config(&assignment);

    info!(
        "Running fingerprint throughput test with {} parallel streams and {} KB chunks",
        parallel_streams,
        chunk_size_bytes / 1024
    );

    let mut throughput_result = ThroughputResult {
        test_type: TestType::Throughput,
        node_id: assignment.node_id,
        duration: start.elapsed(),
        data_size_mb: data_size / (1024 * 1024),
        parallel_streams,
        chunk_size_kb: chunk_size_bytes / 1024,
        connection_type: get_connection_type(&endpoint, assignment.node_id),
        ..Default::default()
    };

    // Run test and update result based on outcome
    match tokio::time::timeout(
        data_transfer_timeout,
        run_bidirectional_throughput_test_with_config(
            &connection,
            data_size,
            true,
            parallel_streams,
            chunk_size_bytes,
        ),
    )
    .await
    {
        Ok(Ok(result)) => {
            info!(
                "Throughput test: Upload: {:.2} Mbps, Download: {:.2} Mbps",
                result.upload_mbps, result.download_mbps
            );
            throughput_result.bytes_sent = result.bytes_sent;
            throughput_result.bytes_received = result.bytes_received;
            throughput_result.transfer_duration = Some(result.duration);
            throughput_result.upload_mbps = result.upload_mbps;
            throughput_result.download_mbps = result.download_mbps;
            throughput_result.statistics = result.statistics;
        }
        Ok(Err(e)) => {
            warn!("Throughput test error: {}", e);
            throughput_result.error = Some(e.to_string());
        }
        Err(_) => {
            warn!("Throughput test timed out");
            throughput_result.error = Some("Test timed out after 30 seconds".to_string());
        }
    }

    let throughput_result = Some(throughput_result);

    // Close the connection
    connection.close(0u32.into(), b"fingerprint test complete");

    // Overall success means at least latency test succeeded
    let overall_success = latency_result.is_some();

    let fingerprint_result = FingerprintResult {
        test_type: assignment.test_type,
        node_id: assignment.node_id,
        duration: start.elapsed(),
        latency: latency_result,
        throughput: throughput_result,
        error: None,
    };

    info!("Fingerprint test completed. Success: {}", overall_success);
    Ok(TestAssignmentResult::Fingerprint(fingerprint_result))
}
