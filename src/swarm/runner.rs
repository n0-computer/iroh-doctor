//! Swarm runner implementation

use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::sync::broadcast;

use anyhow::Result;
use futures_lite::StreamExt;
use iroh::{Endpoint, NodeId};
use iroh_n0des;
use n0_watcher::Watcher;
use portable_atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::RwLock;
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

use crate::swarm::{
    client::SwarmClient,
    config::SwarmConfig,
    execution::perform_test_assignment,
    tests::protocol::{LatencyMessage, TestProtocolHeader, TestProtocolType, DOCTOR_SWARM_ALPN},
    types::{ErrorResult, NetworkReport, SwarmStats, TestAssignmentResult, TestType},
};
/// Background task to process test assignments from the coordinator
async fn assignment_processing_task(
    client: Arc<SwarmClient>,
    completed_assignments: Arc<RwLock<HashSet<(Uuid, NodeId, i32)>>>,
    node_id: NodeId,
    is_testing_initiator: Arc<AtomicBool>,
    endpoint: Arc<Endpoint>,
    stats: Arc<SwarmStats>,
    mut shutdown_rx: broadcast::Receiver<()>,
) {
    // Get new assignments
    tokio::select! {
        result = client.get_assignments() => {
            match result {
        Ok(assignments) => {
            if !assignments.is_empty() {
                let completed = completed_assignments.read().await;

                // Filter out completed assignments
                let new_assignments: Vec<_> = assignments
                    .into_iter()
                    .filter(|a| {
                        let key = (a.test_run_id, a.peer_node_id, a.retry_count);
                        !completed.contains(&key)
                    })
                    .collect();

                if !new_assignments.is_empty() {
                    info!("Received {} new test assignments", new_assignments.len());

                    // Execute each new assignment
                    for assignment in new_assignments {
                        let test_run_id = assignment.test_run_id;
                        let peer_node_id = assignment.peer_node_id;

                        // Check if we're already initiating a test
                        if is_testing_initiator.load(Ordering::Acquire) {
                            info!(
                                "Skipping assignment {} -> {} - already initiating another test",
                                node_id, peer_node_id
                            );
                            continue;
                        }

                        // Check if we should initiate this test
                        let should_initiate = match assignment.test_type {
                            TestType::Fingerprint | TestType::Throughput => node_id < peer_node_id,
                            _ => true,
                        };

                        // Only mark test as started if we're the initiator
                        if should_initiate {
                            match client
                                .mark_test_started(test_run_id, node_id, peer_node_id)
                                .await
                            {
                                Ok(success) => {
                                    if !success {
                                        warn!("Test already started by another node, skipping");
                                        continue;
                                    }
                                }
                                Err(e) => {
                                    warn!("Failed to mark test as started: {}", e);
                                    // Continue anyway - might be a transient error
                                }
                            }
                        } else {
                            info!("Not marking test as started - we're the responder");
                        }

                        // Mark this node as actively initiating a test
                        is_testing_initiator.store(true, Ordering::Release);

                        let retry_count = assignment.retry_count;
                        let client = client.clone();
                        let endpoint = endpoint.clone();
                        let stats = stats.clone();
                        let completed = completed_assignments.clone();
                        let is_testing_task = is_testing_initiator.clone();

                        tokio::spawn(async move {
                            info!("Executing test assignment: {} -> {}", node_id, peer_node_id);

                            let (success, result_data) = match perform_test_assignment(
                                assignment,
                                endpoint.clone(),
                                node_id,
                            )
                            .await
                            {
                                Ok(result) => result,
                                Err(e) => {
                                    error!("Failed to perform test: {}", e);
                                    (
                                        false,
                                        TestAssignmentResult::Error(ErrorResult {
                                            error: e.to_string(),
                                            duration_ms: 0,
                                            test_type: None,
                                            peer: None,
                                        }),
                                    )
                                }
                            };

                            // Check if this is an internal skip (we're just the responder)
                            if matches!(result_data, TestAssignmentResult::InternalSkip(_)) {
                                // Don't submit result - we're acting as responder only
                                // Mark as completed so we don't try again
                                completed.write().await.insert((
                                    test_run_id,
                                    peer_node_id,
                                    retry_count,
                                ));
                                debug!("Skipped submitting result - acting as responder only");
                            } else {
                                // Submit result
                                if let Err(e) = client
                                    .submit_result(
                                        test_run_id,
                                        node_id,
                                        peer_node_id,
                                        success,
                                        result_data,
                                    )
                                    .await
                                {
                                    error!("Failed to submit result: {}", e);
                                } else {
                                    // Mark as completed
                                    completed.write().await.insert((
                                        test_run_id,
                                        peer_node_id,
                                        retry_count,
                                    ));

                                    // Update stats
                                    if success {
                                        stats.increment_completed();
                                    } else {
                                        stats.increment_failed();
                                    }

                                    // Cool-down period between tests
                                    info!("Test completed, entering 1-second cool-down period");
                                    tokio::time::sleep(Duration::from_secs(1)).await;
                                }
                            }

                            // Clear test active flag
                            is_testing_task.store(false, Ordering::Release);
                        });

                        // Break after starting one test to prevent concurrent execution
                        break;
                    }
                }
            }
            }
            Err(e) => {
                warn!("Failed to get assignments: {}", e);
            }
        }
        }
        _ = shutdown_rx.recv() => {
            debug!("Assignment processing task received shutdown signal");
            return;
        }
    }
}

/// Background task to continuously update cached network report
async fn network_report_caching_task(
    client: Arc<SwarmClient>, 
    endpoint: Arc<iroh::Endpoint>,
    mut shutdown_rx: broadcast::Receiver<()>,
) {
    debug!("Starting continuous network report caching (updates every 5 seconds)");
    let mut net_report_stream = endpoint.net_report().stream();
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Helper function to convert report to structured format
    let convert_report_to_network_report =
        |report: iroh::net_report::Report| NetworkReport::from_iroh_report(report);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                // Just wait for the stream-based updates
                debug!("Network report cache tick (waiting for stream updates)");
            }
            // Listen for updates from the stream
            next_report = net_report_stream.next() => {
                match next_report {
                    Some(Some(report)) => {
                        debug!("Got network report update: udp_v4={:?}, udp_v6={:?}",
                            report.udp_v4, report.udp_v6);

                        let network_report = convert_report_to_network_report(report);

                        // Update cached network report
                        client.set_cached_network_report(Some(network_report)).await;
                        debug!("Successfully updated cached network report from stream");
                    }
                    Some(None) => {
                        debug!("Network report stream received None");
                    }
                    None => {
                        warn!("Network report stream ended");
                        break;
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                info!("Network report caching task received shutdown signal");
                break;
            }
        }
    }
}

/// Background task to handle incoming test connections
async fn incoming_connection_handler_task(
    endpoint: Arc<iroh::Endpoint>,
    _node_id: iroh::NodeId,
    is_testing_responder: Arc<AtomicBool>,
    mut shutdown_rx: broadcast::Receiver<()>,
) {
    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                match incoming {
                    Some(incoming) => {
                let connecting = match incoming.accept() {
                    Ok(c) => c,
                    Err(e) => {
                        warn!("Failed to accept incoming connection: {}", e);
                        continue;
                    }
                };

                match connecting.await {
                    Ok(conn) => {
                        let remote = conn.remote_node_id();
                        debug!("Accepted connection from {:?}", remote);

                        // Get ALPN from the connection
                        let alpn = conn.alpn();

                        if alpn.as_deref() == Some(DOCTOR_SWARM_ALPN) {
                            // Handle test connection
                            let remote = conn.remote_node_id().ok();
                            info!("Accepted test connection from {:?}", remote);

                            // For incoming connections, we should only reject if we're actively
                            // INITIATING a test (not just responding). The is_testing flag is for
                            // initiating tests, while incoming connections are handled separately.
                            // We still use is_testing_incoming to prevent handling multiple incoming
                            // connections simultaneously.
                            if is_testing_responder
                                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                                .is_err()
                            {
                                warn!("Rejecting connection from {:?} - already handling another incoming test connection", remote);
                                conn.close(1u32.into(), b"busy");
                                continue;
                            }

                            // Handle different test types
                            let is_testing_handler = is_testing_responder.clone();
                            tokio::spawn(async move {
                                handle_incoming_test_connection(conn, is_testing_handler).await;
                            });
                        }
                    }
                    Err(e) => {
                        warn!("Failed to establish connection: {}", e);
                    }
                }
                    }
                    None => {
                        debug!("Endpoint closed, stopping accept loop");
                        break;
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                info!("Connection handler task received shutdown signal");
                break;
            }
        }
    }
}

/// Handle an individual incoming test connection
async fn handle_incoming_test_connection(
    conn: iroh::endpoint::Connection,
    is_testing_handler: Arc<AtomicBool>,
) {
    // Accept multiple streams for different test types
    loop {
        match conn.accept_bi().await {
            Ok((send, recv)) => {
                // Clone connection for potential parallel streams
                let _conn_clone = conn.clone();

                // Spawn a task to handle this stream
                tokio::spawn(async move {
                    handle_incoming_test_stream(send, recv).await;
                });
            }
            Err(e) => {
                debug!("No more streams: {}", e);
                break;
            }
        }
    }

    // Cleanup when responder finishes - mark as not testing
    is_testing_handler.store(false, Ordering::Release);
    debug!("Responder cleanup complete");
}

/// Handle an individual test stream
async fn handle_incoming_test_stream(
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
) {
    // Read protocol header directly
    match TestProtocolHeader::read_from(&mut recv).await {
        Ok(header) => {
            debug!(
                "Received test header: type={}, size={}, chunk_size={:?}",
                header.test_type, header.data_size, header.chunk_size
            );

            match header.test_type {
                TestProtocolType::Latency => {
                    handle_latency_test(send, recv, header).await;
                }
                TestProtocolType::Throughput => {
                    static STREAM_COUNTER: AtomicU64 = AtomicU64::new(0);
                    let stream_idx = STREAM_COUNTER.fetch_add(1, Ordering::Relaxed) as usize;
                    handle_throughput_stream(send, recv, header, stream_idx)
                        .await
                        .ok();
                }
                TestProtocolType::Connectivity => {
                    // Simple connectivity test - just respond with success
                    debug!("Handling connectivity test");
                    send.write_all(b"OK").await.ok();
                    send.finish().ok();
                }
            }
        }
        Err(e) => {
            debug!("Failed to read protocol header: {}", e);
        }
    }
}

/// Handle a latency test stream
async fn handle_latency_test(
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    header: TestProtocolHeader,
) {
    debug!("Handling latency test");
    let mut ping_buf = vec![0u8; header.data_size as usize];
    match recv.read_exact(&mut ping_buf).await {
        Ok(()) => {
            let ping_str = String::from_utf8_lossy(&ping_buf);
            if let Some(pong_response) = LatencyMessage::pong_from_ping(&ping_str) {
                send.write_all(pong_response.as_bytes()).await.ok();
                trace!("Sent PONG response");
            }
        }
        Err(e) => {
            warn!("Failed to read ping data: {}", e);
        }
    }
    send.finish().ok();
}

/// Handle a throughput test stream
async fn handle_throughput_stream(
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    header: TestProtocolHeader,
    stream_idx: usize,
) -> Result<()> {
    let data_size = header.data_size;
    let chunk_size = header.chunk_size.unwrap_or(65536);

    debug!(
        "Handling throughput stream {} with {} bytes, chunk size {} bytes",
        stream_idx, data_size, chunk_size
    );

    // Ready to receive data - no confirmation needed

    // First, receive all incoming data
    let mut recv_buf = vec![0u8; chunk_size];
    let mut total_received = 0u64;

    while total_received < data_size {
        match recv.read(&mut recv_buf).await {
            Ok(Some(n)) => {
                total_received += n as u64;

                // Log progress periodically
                if total_received % (10 * 1024 * 1024) == 0 || total_received >= data_size {
                    debug!(
                        "Stream {} receive progress: {:.1} MB received ({:.1}%)",
                        stream_idx,
                        total_received as f64 / (1024.0 * 1024.0),
                        (total_received as f64 / data_size as f64) * 100.0
                    );
                }
            }
            Ok(None) => {
                debug!(
                    "Stream {} closed after receiving {} bytes",
                    stream_idx, total_received
                );
                break;
            }
            Err(e) => {
                warn!("Stream {} error reading data: {}", stream_idx, e);
                break;
            }
        }
    }

    debug!(
        "Stream {} finished receiving {} bytes, now sending response",
        stream_idx, total_received
    );

    // Now send back the same amount of data using the configured chunk size
    let send_chunk = vec![0u8; chunk_size];
    let mut total_sent = 0u64;

    while total_sent < data_size {
        let to_send = chunk_size.min((data_size - total_sent) as usize);
        if let Err(e) = send.write_all(&send_chunk[..to_send]).await {
            warn!("Stream {} failed to send response data: {}", stream_idx, e);
            break;
        }
        total_sent += to_send as u64;

        // Log progress periodically
        if total_sent % (10 * 1024 * 1024) == 0 || total_sent >= data_size {
            debug!(
                "Stream {} send progress: {:.1} MB sent ({:.1}%)",
                stream_idx,
                total_sent as f64 / (1024.0 * 1024.0),
                (total_sent as f64 / data_size as f64) * 100.0
            );
        }
    }

    send.finish().ok();
    debug!(
        "Stream {} completed: received {} bytes, sent {} bytes",
        stream_idx, total_received, total_sent
    );
    Ok(())
}

/// Run the swarm client
pub async fn run_swarm_client(
    config: SwarmConfig,
    ssh_key_path: &std::path::Path,
    metrics_registry: crate::metrics::IrohMetricsRegistry,
) -> Result<()> {
    let client =
        Arc::new(SwarmClient::connect(config.clone(), ssh_key_path, metrics_registry).await?);
    let endpoint = client.endpoint();
    let node_id = client.node_id();

    // Create n0des client for metrics collection using coordinator
    info!("Creating iroh_n0des::Client for metrics collection to coordinator {}", config.coordinator_node_id);
    let rpc_client = iroh_n0des::Client::builder(&endpoint)
        .ssh_key_from_file(ssh_key_path)
        .await?
        .build(config.coordinator_node_id)
        .await?;

    info!("Connected to coordinator, starting swarm operations as a client node");

    // Create shutdown broadcast channel
    let (shutdown_tx, _) = broadcast::channel(1);
    
    // Spawn background task to continuously update cached network report
    let network_report_handle = tokio::spawn(network_report_caching_task(
        client.clone(),
        endpoint.clone(),
        shutdown_tx.subscribe(),
    ));

    // Wait for DNS discovery to be ready
    info!("Waiting 10 seconds for DNS discovery to be ready...");
    tokio::time::sleep(Duration::from_secs(10)).await;

    // Track completed assignments to avoid duplicates
    // Key: (test_run_id, peer_node_id, retry_count)
    let completed_assignments = Arc::new(RwLock::new(
        HashSet::<(uuid::Uuid, iroh::NodeId, i32)>::new(),
    ));

    // Separate flags for initiating vs responding to tests
    // is_testing_initiator: tracks if we're actively initiating a test
    // is_testing_responder: tracks if we're currently responding to an incoming test
    // These need to be separate because a node can be a responder while also
    // needing to check for assignments (but not initiate)
    let is_testing_initiator = Arc::new(AtomicBool::new(false));
    let is_testing_responder = Arc::new(AtomicBool::new(false));

    // Spawn task to handle incoming test connections
    let connection_handler_handle = tokio::spawn(incoming_connection_handler_task(
        endpoint.clone(),
        node_id,
        is_testing_responder.clone(),
        shutdown_tx.subscribe(),
    ));

    // Main loop for getting and executing test assignments
    let stats = Arc::new(SwarmStats::default());
    let mut heartbeat_interval = tokio::time::interval(config.heartbeat_interval);
    heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut assignment_interval = tokio::time::interval(Duration::from_secs(10));
    assignment_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let result = {
        loop {
            tokio::select! {
                    _ = heartbeat_interval.tick() => {
                        // Spawn heartbeat task so it doesn't block Ctrl+C
                        let client = client.clone();
                        tokio::spawn(async move {
                            match tokio::time::timeout(Duration::from_secs(7), client.heartbeat()).await {
                                Ok(Ok(())) => {
                                    // Heartbeat successful
                                }
                                Ok(Err(e)) => {
                                    warn!("Failed to send heartbeat: {}", e);
                                }
                                Err(_) => {
                                    warn!("Heartbeat timed out after 7 seconds");
                                }
                            }
                        });
                    }
                    _ = tokio::signal::ctrl_c() => {
                        info!("Received shutdown signal, stopping swarm client");
                        // Broadcast shutdown to all tasks
                        let _ = shutdown_tx.send(());
                        break Ok(());
                    }
                    _ = assignment_interval.tick() => {
                        // Spawn assignment processing task so it doesn't block Ctrl+C
                        let client = client.clone();
                        let completed_assignments = completed_assignments.clone();
                        let is_testing_initiator = is_testing_initiator.clone();
                        let endpoint = endpoint.clone();
                        let stats = stats.clone();
                        let shutdown_rx = shutdown_tx.subscribe();
                        
                        tokio::spawn(async move {
                            match tokio::time::timeout(
                                Duration::from_secs(15),
                                assignment_processing_task(
                                    client,
                                    completed_assignments,
                                    node_id,
                                    is_testing_initiator,
                                    endpoint,
                                    stats,
                                    shutdown_rx,
                                )
                            ).await {
                                Ok(()) => {
                                    // Assignment processing completed
                                }
                                Err(_) => {
                                    warn!("Assignment processing timed out after 15 seconds");
                                }
                            }
                        });
                    }
            }
        }
    };

    // Cleanup: Abort background tasks
    info!("Shutting down background tasks...");
    network_report_handle.abort();
    connection_handler_handle.abort();

    // Wait for tasks to complete cleanup (with timeout)
    let cleanup_timeout = Duration::from_secs(5);
    let _ = tokio::time::timeout(cleanup_timeout, async {
        let _ = tokio::join!(network_report_handle, connection_handler_handle);
    })
    .await;

    info!("Swarm client shutdown complete");
    
    // Keep rpc_client alive until function end for metrics collection
    drop(rpc_client);
    
    result
}
