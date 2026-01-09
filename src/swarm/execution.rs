//! Test execution and swarm runner implementation

use std::time::{Duration, Instant};

use anyhow::Result;
use iroh::{endpoint::Connection, Endpoint, EndpointId, TransportAddr, Watcher};
use serde::{Deserialize, Serialize};
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
        ThroughputResult, DEFAULT_DATA_SIZE,
    },
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum ConnectionType {
    Ip,
    Relay,
}

impl From<&TransportAddr> for ConnectionType {
    fn from(value: &TransportAddr) -> Self {
        match value {
            TransportAddr::Ip(_) => ConnectionType::Ip,
            TransportAddr::Relay(_) => ConnectionType::Relay,
            _ => panic!(),
        }
    }
}

/// Helper function to get the real connection type from the endpoint
pub(crate) fn get_connection_type(conn: &Connection) -> Option<ConnectionType> {
    conn.paths()
        .get()
        .into_iter()
        .find(|path| path.is_selected())
        .map(|path| path.remote_addr().into())
}

pub async fn perform_test_assignment(
    assignment: TestAssignment,
    endpoint: Endpoint,
    _node_id: EndpointId,
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
    endpoint: Endpoint,
    start: Instant,
    data_transfer_timeout: Duration,
) -> Result<TestAssignmentResult> {
    info!("Starting throughput test with node {}", assignment.node_id);

    info!("Attempting to connect to node {}", assignment.node_id);
    match endpoint
        .connect(assignment.node_id, DOCTOR_SWARM_ALPN)
        .await
    {
        Ok(conn) => {
            let data_size = assignment
                .test_config
                .size_bytes
                .unwrap_or(DEFAULT_DATA_SIZE);

            let (parallel_streams, chunk_size_bytes) = assignment.throughput_config();

            info!(
                "Running throughput test with {} parallel streams and {} KiB chunks",
                parallel_streams,
                chunk_size_bytes / 1024
            );

            let transfer_result = tokio::time::timeout(
                data_transfer_timeout,
                run_bidirectional_throughput_test_with_config(
                    &conn,
                    data_size,
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

                    let throughput_result = ThroughputResult::new(
                        assignment.test_type,
                        assignment.node_id,
                        start.elapsed(),
                        data_size,
                        parallel_streams,
                        chunk_size_bytes,
                    )
                    .with_connection_type(get_connection_type(&conn))
                    .with_transfer_results(
                        result.bytes_sent,
                        result.bytes_received,
                        result.duration,
                        result.upload_mbps,
                        result.download_mbps,
                    )
                    .with_statistics(result.statistics);

                    Ok(TestAssignmentResult::Throughput(throughput_result))
                }
                Ok(Err(e)) => {
                    warn!("Throughput test error: {}", e);
                    Ok(TestAssignmentResult::Error(ErrorResult {
                        error: e.to_string(),
                        duration: start.elapsed(),
                        test_type: Some(assignment.test_type),
                        remote_id: Some(assignment.node_id),
                        connection_type: get_connection_type(&conn),
                    }))
                }
                Err(_) => {
                    warn!("Throughput test timed out");
                    Ok(TestAssignmentResult::Error(ErrorResult {
                        error: "Test timed out after 30 seconds".to_string(),
                        duration: start.elapsed(),
                        test_type: Some(assignment.test_type),
                        remote_id: Some(assignment.node_id),
                        connection_type: get_connection_type(&conn),
                    }))
                }
            }
        }
        Err(e) => {
            warn!("Failed to connect: {}", e);
            Ok(TestAssignmentResult::Error(ErrorResult {
                error: format!("Failed to connect: {e}"),
                duration: start.elapsed(),
                test_type: Some(assignment.test_type),
                remote_id: Some(assignment.node_id),
                connection_type: None, // No connection established
            }))
        }
    }
}

async fn execute_latency_test(
    assignment: TestAssignment,
    endpoint: Endpoint,
    start: Instant,
) -> Result<TestAssignmentResult> {
    let iterations = assignment.test_config.iterations.unwrap_or(10);

    let (ping_interval, ping_timeout) = assignment.latency_config();
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
        Ok(result) => Ok(TestAssignmentResult::Latency(result)),
        Err(e) => {
            warn!("Latency test failed: {}", e);
            Ok(TestAssignmentResult::Error(ErrorResult {
                error: e.to_string(),
                duration: start.elapsed(),
                test_type: Some(assignment.test_type),
                remote_id: Some(assignment.node_id),
                connection_type: None, // Test failed before connection info available
            }))
        }
    }
}

async fn execute_fingerprint_test(
    assignment: TestAssignment,
    endpoint: Endpoint,
    start: Instant,
    data_transfer_timeout: Duration,
) -> Result<TestAssignmentResult> {
    info!("Starting fingerprint test with node {}", assignment.node_id);

    info!("Phase 1/2: Testing latency...");
    let iterations = 10;
    let (ping_interval, ping_timeout) = assignment.latency_config();
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
            warn!("Latency test failed: {}", e);
            return Ok(TestAssignmentResult::Fingerprint(FingerprintResult {
                test_type: assignment.test_type,
                remote_id: assignment.node_id,
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
                connection_type: None,
            }));
        }
    };

    info!("Phase 2/2: Testing throughput...");

    let connection = match endpoint
        .connect(assignment.node_id, DOCTOR_SWARM_ALPN)
        .await
    {
        Ok(conn) => conn,
        Err(e) => {
            warn!("Failed to connect for throughput test: {}", e);
            return Ok(TestAssignmentResult::Fingerprint(FingerprintResult {
                test_type: assignment.test_type,
                remote_id: assignment.node_id,
                duration: start.elapsed(),
                latency: latency_result,
                throughput: None,
                error: Some(format!("Throughput test skipped: connection failed: {e}")),
                connection_type: None,
            }));
        }
    };

    let data_size = assignment
        .test_config
        .size_bytes
        .unwrap_or(DEFAULT_DATA_SIZE);

    let (parallel_streams, chunk_size_bytes) = assignment.throughput_config();

    info!(
        "Running fingerprint throughput test with {} parallel streams and {} KiB chunks",
        parallel_streams,
        chunk_size_bytes / 1024
    );

    let mut throughput_result = ThroughputResult::new(
        TestType::Throughput,
        assignment.node_id,
        start.elapsed(),
        data_size,
        parallel_streams,
        chunk_size_bytes,
    )
    .with_connection_type(get_connection_type(&connection));

    match tokio::time::timeout(
        data_transfer_timeout,
        run_bidirectional_throughput_test_with_config(
            &connection,
            data_size,
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
            throughput_result = throughput_result
                .with_transfer_results(
                    result.bytes_sent,
                    result.bytes_received,
                    result.duration,
                    result.upload_mbps,
                    result.download_mbps,
                )
                .with_statistics(result.statistics);
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

    connection.close(0u32.into(), b"fingerprint test complete");

    let overall_success = latency_result.is_some();

    let fingerprint_result = FingerprintResult {
        test_type: assignment.test_type,
        remote_id: assignment.node_id,
        duration: start.elapsed(),
        latency: latency_result,
        throughput: throughput_result,
        error: None,
        connection_type: get_connection_type(&connection),
    };

    info!("Fingerprint test completed. Success: {}", overall_success);
    Ok(TestAssignmentResult::Fingerprint(fingerprint_result))
}
