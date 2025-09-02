//! Test execution and swarm runner implementation

use std::{sync::Arc, time::Duration};

use anyhow::Result;
use iroh::{
    endpoint::{Connection, ConnectionType},
    Endpoint, NodeId, Watcher,
};
use rand::Rng;
use tracing::{info, trace, warn};

use crate::swarm::{
    rpc::TestAssignment,
    tests::{
        connectivity::resolve_node_addr,
        protocol::{LatencyMessage, TestProtocolHeader, TestProtocolType, DOCTOR_SWARM_ALPN},
        run_bidirectional_throughput_test_with_config, run_connectivity_test,
        run_latency_test_with_config,
    },
    types::{
        ConnectivityResult, ErrorResult, FingerprintResult, InternalSkipResult, LatencyResult,
        TestAssignmentResult, TestType, ThroughputResult,
    },
};

// Data transfer timeout constant
const DATA_TRANSFER_TIMEOUT: Duration = Duration::from_secs(30); // 30 seconds should be enough for most transfers

/// Helper function to get the real connection type from the endpoint
fn get_connection_type(endpoint: &Endpoint, peer_id: NodeId) -> Option<ConnectionType> {
    endpoint.conn_type(peer_id).map(|mut watcher| {
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
        .unwrap_or(64) // default: 64KB
        .clamp(16, 512); // enforce range: 16-512KB

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
    node_id: NodeId,
) -> Result<(bool, TestAssignmentResult)> {
    let start = std::time::Instant::now();

    // For bidirectional tests like Fingerprint, only the node with lower ID initiates
    // This prevents both nodes from trying to initiate the same test
    let should_initiate = match assignment.test_type {
        TestType::Fingerprint | TestType::Throughput => {
            let initiates = node_id < assignment.peer_node_id;
            info!(
                "Bidirectional test: my_id={} < peer_id={} = {} (I {} initiate)",
                node_id,
                assignment.peer_node_id,
                initiates,
                if initiates { "will" } else { "won't" }
            );
            initiates
        }
        _ => true, // Other tests can be initiated by any node
    };

    if !should_initiate {
        info!(
            "Skipping test initiation - peer {} will initiate (my ID: {})",
            assignment.peer_node_id, node_id
        );
        // Don't submit any result - we're just the responder
        // The test will be handled when the peer connects to us
        return Ok((
            true,
            TestAssignmentResult::InternalSkip(InternalSkipResult {
                reason: "acting_as_responder".to_string(),
            }),
        ));
    }

    match assignment.test_type {
        TestType::Connectivity => execute_connectivity_test(assignment, endpoint, start).await,
        TestType::Throughput => execute_throughput_test(assignment, endpoint, start).await,
        TestType::Latency => execute_latency_test(assignment, endpoint, start).await,
        TestType::RelayPerformance => {
            execute_relay_performance_test(assignment, endpoint, start).await
        }
        TestType::Fingerprint => execute_fingerprint_test(assignment, endpoint, start).await,
    }
}

async fn execute_connectivity_test(
    assignment: TestAssignment,
    endpoint: Arc<Endpoint>,
    start: std::time::Instant,
) -> Result<(bool, TestAssignmentResult)> {
    // Add retry logic for connectivity tests
    let max_attempts = 3;
    let mut last_error: Option<String> = None;

    for attempt in 1..=max_attempts {
        if attempt > 1 {
            info!("Connectivity test attempt {} of {}", attempt, max_attempts);
            tokio::time::sleep(Duration::from_secs(3)).await;
        }

        match run_connectivity_test(&endpoint, assignment.peer_node_id).await {
            Ok(test_result) => {
                if test_result.success {
                    return Ok((true, TestAssignmentResult::Connectivity(test_result.data)));
                } else {
                    last_error = Some(test_result.data.error.unwrap_or("Test failed".to_string()));
                    if attempt < max_attempts {
                        warn!("Connectivity test failed, retrying...");
                    }
                }
            }
            Err(e) => {
                last_error = Some(e.to_string());
                if attempt < max_attempts {
                    warn!("Connectivity test error: {}, retrying...", e);
                }
            }
        }
    }

    // All attempts failed
    warn!("All connectivity test attempts failed");
    Ok((
        false,
        TestAssignmentResult::Error(ErrorResult {
            error: last_error.unwrap_or_else(|| "All connection attempts failed".to_string()),
            duration_ms: start.elapsed().as_millis(),
            test_type: Some(assignment.test_type),
            peer: Some(assignment.peer_node_id.to_string()),
        }),
    ))
}

async fn execute_throughput_test(
    assignment: TestAssignment,
    endpoint: Arc<Endpoint>,
    start: std::time::Instant,
) -> Result<(bool, TestAssignmentResult)> {
    info!(
        "Starting throughput test with peer {}",
        assignment.peer_node_id
    );

    // Try multiple times to resolve and connect
    let mut last_error = String::from("No connection attempt made");
    for attempt in 1..=3 {
        if attempt > 1 {
            info!("Throughput test connection attempt {} of 3", attempt);
            tokio::time::sleep(Duration::from_secs(5)).await;
        }

        // Try to resolve the node address
        let node_addr = match resolve_node_addr(&endpoint, assignment.peer_node_id).await {
            Ok(Some(addr)) => {
                info!("Resolved peer address: {:?}", addr);
                addr
            }
            Ok(None) => {
                last_error = format!(
                            "Failed to resolve node address for {} (attempt {}). The peer may not be publishing its address yet.",
                            assignment.peer_node_id, attempt
                        );
                warn!("{}", last_error);
                continue;
            }
            Err(e) => {
                last_error = format!("Error resolving node address: {e} (attempt {attempt})");
                warn!("{}", last_error);
                continue;
            }
        };

        // Extract configuration
        let connection_timeout = extract_connection_timeout(&assignment);

        // Try to connect
        match tokio::time::timeout(
            connection_timeout,
            endpoint.connect(node_addr.clone(), DOCTOR_SWARM_ALPN),
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
                    DATA_TRANSFER_TIMEOUT,
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
                            peer: assignment.peer_node_id.to_string(),
                            duration_ms: start.elapsed().as_millis(),
                            data_size_mb: data_size / (1024 * 1024),
                            bytes_sent: result.bytes_sent,
                            bytes_received: result.bytes_received,
                            transfer_duration_ms: Some(result.duration_ms),
                            throughput_mbps: result.upload_mbps, // For backward compatibility
                            upload_mbps: result.upload_mbps,
                            download_mbps: result.download_mbps,
                            parallel_streams,
                            chunk_size_kb: chunk_size_bytes / 1024,
                            statistics: result.statistics,
                            error: None,
                            connection_type: get_connection_type(
                                &endpoint,
                                assignment.peer_node_id,
                            ),
                        };

                        return Ok((true, TestAssignmentResult::Throughput(throughput_result)));
                    }
                    Ok(Err(e)) => {
                        warn!("Throughput test error: {}", e);
                        return Ok((
                            false,
                            TestAssignmentResult::Error(ErrorResult {
                                error: e.to_string(),
                                duration_ms: start.elapsed().as_millis(),
                                test_type: Some(assignment.test_type),
                                peer: Some(assignment.peer_node_id.to_string()),
                            }),
                        ));
                    }
                    Err(_) => {
                        warn!("Throughput test timed out");
                        return Ok((
                            false,
                            TestAssignmentResult::Error(ErrorResult {
                                error: "Test timed out after 30 seconds".to_string(),
                                duration_ms: start.elapsed().as_millis(),
                                test_type: Some(assignment.test_type),
                                peer: Some(assignment.peer_node_id.to_string()),
                            }),
                        ));
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
    Ok((
        false,
        TestAssignmentResult::Error(ErrorResult {
            error: last_error,
            duration_ms: start.elapsed().as_millis(),
            test_type: Some(assignment.test_type),
            peer: Some(assignment.peer_node_id.to_string()),
        }),
    ))
}

async fn execute_latency_test(
    assignment: TestAssignment,
    endpoint: Arc<Endpoint>,
    start: std::time::Instant,
) -> Result<(bool, TestAssignmentResult)> {
    // Add retry logic for latency tests
    let iterations = assignment.test_config.iterations.unwrap_or(10);

    let max_attempts = 3;
    let mut last_error = None;

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
            assignment.peer_node_id,
            iterations,
            ping_interval,
            ping_timeout,
        )
        .await
        {
            Ok(test_result) => {
                if test_result.success {
                    return Ok((true, TestAssignmentResult::Latency(test_result.data)));
                } else {
                    last_error = Some(TestAssignmentResult::Latency(test_result.data));
                    if attempt < max_attempts {
                        warn!("Latency test failed, retrying...");
                    }
                }
            }
            Err(e) => {
                last_error = Some(TestAssignmentResult::Error(ErrorResult {
                    error: e.to_string(),
                    duration_ms: start.elapsed().as_millis(),
                    test_type: Some(assignment.test_type),
                    peer: Some(assignment.peer_node_id.to_string()),
                }));
                if attempt < max_attempts {
                    warn!("Latency test error: {}, retrying...", e);
                }
            }
        }
    }

    // All attempts failed
    warn!("All latency test attempts failed");
    Ok((
        false,
        last_error.unwrap_or_else(|| {
            TestAssignmentResult::Error(ErrorResult {
                error: "All latency test attempts failed".to_string(),
                duration_ms: start.elapsed().as_millis(),
                test_type: Some(assignment.test_type),
                peer: Some(assignment.peer_node_id.to_string()),
            })
        }),
    ))
}

async fn execute_relay_performance_test(
    assignment: TestAssignment,
    _endpoint: Arc<Endpoint>,
    start: std::time::Instant,
) -> Result<(bool, TestAssignmentResult)> {
    // Not implemented yet
    Ok((
        false,
        TestAssignmentResult::Error(ErrorResult {
            error: "Relay performance test not implemented".to_string(),
            duration_ms: start.elapsed().as_millis(),
            test_type: Some(assignment.test_type),
            peer: None,
        }),
    ))
}

async fn execute_fingerprint_test(
    assignment: TestAssignment,
    endpoint: Arc<Endpoint>,
    start: std::time::Instant,
) -> Result<(bool, TestAssignmentResult)> {
    // Combined test: connectivity + latency + throughput
    info!(
        "Starting fingerprint test with peer {}",
        assignment.peer_node_id
    );

    let mut connectivity_result: Option<ConnectivityResult> = None;

    // 1. Connectivity test with retries
    info!("Phase 1/3: Testing connectivity...");
    let mut connectivity_success = false;
    let mut connection: Option<Connection> = None;
    let mut last_error = String::from("No connection attempt made");

    for attempt in 1..=3 {
        if attempt > 1 {
            let wait_time = if attempt == 2 { 10 } else { 5 };
            info!(
                "Waiting {} seconds for DNS propagation before retry {} of 3",
                wait_time, attempt
            );
            tokio::time::sleep(Duration::from_secs(wait_time)).await;
        }

        // Try to resolve the node address
        let node_addr = match resolve_node_addr(&endpoint, assignment.peer_node_id).await {
            Ok(Some(addr)) => {
                info!("Resolved peer address: {:?}", addr);
                addr
            }
            Ok(None) => {
                last_error = format!(
                            "Failed to resolve node address for {} (attempt {}). DNS/pkarr may still be propagating.",
                            assignment.peer_node_id, attempt
                        );
                warn!("{}", last_error);
                continue;
            }
            Err(e) => {
                last_error = format!("Error resolving node address: {e} (attempt {attempt})");
                warn!("{}", last_error);
                continue;
            }
        };

        let conn_start = std::time::Instant::now();
        let connection_timeout = extract_connection_timeout(&assignment);
        match tokio::time::timeout(
            connection_timeout,
            endpoint.connect(node_addr, DOCTOR_SWARM_ALPN),
        )
        .await
        {
            Ok(Ok(conn)) => {
                connectivity_success = true;
                let connection_time = conn_start.elapsed();
                info!("Connectivity successful in {:?}", connection_time);
                connectivity_result = Some(ConnectivityResult {
                    connected: true,
                    connection_time_ms: Some(connection_time.as_millis() as u64),
                    peer: assignment.peer_node_id.to_string(),
                    duration_ms: connection_time.as_millis(),
                    error: None,
                    connection_type: get_connection_type(&endpoint, assignment.peer_node_id),
                });
                connection = Some(conn);
                break;
            }
            Ok(Err(e)) => {
                last_error = format!("Failed to connect: {e} (attempt {attempt})");
                warn!("{}", last_error);
            }
            Err(_) => {
                last_error = format!("Connection attempt {attempt} timed out after 20 seconds");
                warn!("{}", last_error);
            }
        }
    }

    if !connectivity_success {
        connectivity_result = Some(ConnectivityResult {
            connected: false,
            connection_time_ms: None,
            peer: assignment.peer_node_id.to_string(),
            duration_ms: start.elapsed().as_millis(),
            error: Some(last_error),
            connection_type: None, // No connection established
        });

        // Skip other tests if connectivity failed - return error result
        return Ok((
            false,
            TestAssignmentResult::Fingerprint(FingerprintResult {
                test_type: assignment.test_type,
                peer: assignment.peer_node_id.to_string(),
                duration_ms: start.elapsed().as_millis(),
                connectivity: connectivity_result,
                latency: None,
                throughput: None,
                error: Some("Connectivity test failed".to_string()),
            }),
        ));
    }

    let connection = connection.unwrap();

    // 2. Latency test
    info!("Phase 2/3: Testing latency...");
    let iterations = 10;
    let (ping_interval, ping_timeout) = extract_latency_config(&assignment);
    info!(
        "Running fingerprint latency test with {}ms interval and {}ms timeout",
        ping_interval.as_millis(),
        ping_timeout.as_millis()
    );
    let mut latencies = Vec::new();
    let mut latency_failures = 0;

    for i in 0..iterations {
        match connection.open_bi().await {
            Ok((mut send, mut recv)) => {
                let ping_start = std::time::Instant::now();

                // Send protocol header for latency test
                let ping_data = LatencyMessage::ping(i);
                let header =
                    TestProtocolHeader::new(TestProtocolType::Latency, ping_data.len() as u64);
                let header_bytes = header.to_bytes();

                if let Err(e) = send.write_all(&header_bytes).await {
                    warn!("Failed to send protocol header: {}", e);
                    latency_failures += 1;
                    continue;
                }

                // Send ping data
                if let Err(e) = send.write_all(ping_data.as_bytes()).await {
                    warn!("Failed to send ping {}: {}", i, e);
                    latency_failures += 1;
                    continue;
                }

                if let Err(e) = send.finish() {
                    warn!("Failed to finish send stream: {}", e);
                    latency_failures += 1;
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
                            latency_failures += 1;
                        }
                    }
                    Ok(Ok(None)) => {
                        warn!("Stream closed without response");
                        latency_failures += 1;
                    }
                    Ok(Err(e)) => {
                        warn!("Failed to read pong: {}", e);
                        latency_failures += 1;
                    }
                    Err(_) => {
                        warn!("Ping {} timed out after {:?}", i, ping_timeout);
                        latency_failures += 1;
                    }
                }
            }
            Err(e) => {
                warn!("Failed to open stream for ping {}: {}", i, e);
                latency_failures += 1;
            }
        }

        // Configurable delay between pings
        if i < iterations - 1 {
            tokio::time::sleep(ping_interval).await;
        }
    }

    let latency_result = if !latencies.is_empty() {
        let sum: Duration = latencies.iter().sum();
        let avg_latency = sum / latencies.len() as u32;
        let min_latency = latencies.iter().min().cloned().unwrap_or_default();
        let max_latency = latencies.iter().max().cloned().unwrap_or_default();

        // Calculate standard deviation
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
            "Latency test: avg={:?}, min={:?}, max={:?}",
            avg_latency, min_latency, max_latency
        );

        Some(LatencyResult {
            avg_latency_ms: Some(avg_latency.as_secs_f64() * 1000.0),
            min_latency_ms: Some(min_latency.as_secs_f64() * 1000.0),
            max_latency_ms: Some(max_latency.as_secs_f64() * 1000.0),
            std_dev_ms: Some(std_dev),
            successful_pings: latencies.len(),
            failed_pings: latency_failures,
            total_iterations: iterations,
            success_rate: Some(latencies.len() as f64 / iterations as f64),
            duration_ms: start.elapsed().as_millis(),
            error: None,
            connection_type: get_connection_type(&endpoint, assignment.peer_node_id),
        })
    } else {
        Some(LatencyResult {
            avg_latency_ms: None,
            min_latency_ms: None,
            max_latency_ms: None,
            std_dev_ms: None,
            successful_pings: 0,
            failed_pings: latency_failures,
            total_iterations: iterations,
            success_rate: None,
            duration_ms: start.elapsed().as_millis(),
            error: Some("No successful latency measurements".to_string()),
            connection_type: get_connection_type(&endpoint, assignment.peer_node_id),
        })
    };

    // 3. Throughput test - Use the same parallel streams implementation as regular throughput test
    info!("Phase 3/3: Testing throughput...");

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

    // Run the same parallel streams throughput test as regular throughput
    let throughput_result = match tokio::time::timeout(
        DATA_TRANSFER_TIMEOUT,
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

            Some(ThroughputResult {
                test_type: TestType::Throughput,
                peer: assignment.peer_node_id.to_string(),
                duration_ms: start.elapsed().as_millis(),
                data_size_mb: data_size / (1024 * 1024),
                bytes_sent: result.bytes_sent,
                bytes_received: result.bytes_received,
                transfer_duration_ms: Some(result.duration_ms),
                throughput_mbps: result.upload_mbps, // For backward compatibility
                upload_mbps: result.upload_mbps,
                download_mbps: result.download_mbps,
                parallel_streams,
                chunk_size_kb: chunk_size_bytes / 1024,
                statistics: result.statistics,
                error: None,
                connection_type: get_connection_type(&endpoint, assignment.peer_node_id),
            })
        }
        Ok(Err(e)) => {
            warn!("Throughput test error: {}", e);
            Some(ThroughputResult {
                test_type: TestType::Throughput,
                peer: assignment.peer_node_id.to_string(),
                duration_ms: start.elapsed().as_millis(),
                data_size_mb: data_size / (1024 * 1024),
                bytes_sent: 0,
                bytes_received: 0,
                transfer_duration_ms: None,
                throughput_mbps: 0.0,
                upload_mbps: 0.0,
                download_mbps: 0.0,
                parallel_streams,
                chunk_size_kb: chunk_size_bytes / 1024,
                statistics: None,
                error: Some(e.to_string()),
                connection_type: get_connection_type(&endpoint, assignment.peer_node_id),
            })
        }
        Err(_) => {
            warn!("Throughput test timed out");
            Some(ThroughputResult {
                test_type: TestType::Throughput,
                peer: assignment.peer_node_id.to_string(),
                duration_ms: start.elapsed().as_millis(),
                data_size_mb: data_size / (1024 * 1024),
                bytes_sent: 0,
                bytes_received: 0,
                transfer_duration_ms: None,
                throughput_mbps: 0.0,
                upload_mbps: 0.0,
                download_mbps: 0.0,
                parallel_streams,
                chunk_size_kb: chunk_size_bytes / 1024,
                statistics: None,
                error: Some("Test timed out after 30 seconds".to_string()),
                connection_type: get_connection_type(&endpoint, assignment.peer_node_id),
            })
        }
    };

    // Close the main connection
    connection.close(0u32.into(), b"fingerprint test complete");

    // Determine overall success (at least connectivity should work)
    let overall_success = connectivity_success;

    let fingerprint_result = FingerprintResult {
        test_type: assignment.test_type,
        peer: assignment.peer_node_id.to_string(),
        duration_ms: start.elapsed().as_millis(),
        connectivity: connectivity_result,
        latency: latency_result,
        throughput: throughput_result,
        error: None,
    };

    info!("Fingerprint test completed. Success: {}", overall_success);
    Ok((
        overall_success,
        TestAssignmentResult::Fingerprint(fingerprint_result),
    ))
}

/// Execute a test assignment and return the result
pub async fn execute_test(
    endpoint: &Arc<Endpoint>,
    assignment: TestAssignment,
    node_id: NodeId,
) -> Result<(bool, TestAssignmentResult)> {
    perform_test_assignment(assignment, endpoint.clone(), node_id).await
}
