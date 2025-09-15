//! Swarm runner implementation

use std::{collections::HashSet, path::Path, sync::Arc, time::Duration};

use anyhow::Result;
use futures_lite::future::race;
use iroh::{endpoint::StreamId, Endpoint, NodeId};
use iroh_n0des;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

use crate::{
    doctor::close_endpoint_on_ctrl_c,
    swarm::{
        client::SwarmClient,
        config::SwarmConfig,
        execution::perform_test_assignment,
        net_report_ext::probe_port_variation,
        tests::protocol::{
            LatencyMessage, TestProtocolHeader, TestProtocolType, DOCTOR_SWARM_ALPN,
        },
        transfer_utils::handle_bidirectional_transfer,
        types::{ErrorResult, SwarmStats, TestAssignmentResult, TestResultType},
    },
};
async fn assignment_processing_task(
    client: Arc<SwarmClient>,
    completed_assignments: Arc<RwLock<HashSet<(Uuid, NodeId, i32)>>>,
    node_id: NodeId,
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
                                let key = (a.test_run_id, a.node_id, a.retry_count);
                                !completed.contains(&key)
                            })
                            .collect();

                        if !new_assignments.is_empty() {
                            info!("Received {} new test assignments", new_assignments.len());

                            if let Some(assignment) = new_assignments.into_iter().next() {
                                let test_run_id = assignment.test_run_id;
                                let retry_count = assignment.retry_count;
                                let target_node_id = assignment.node_id;

                                if let Err(e) = client
                                    .mark_test_started(test_run_id, node_id, target_node_id)
                                    .await
                                {
                                    warn!("Failed to mark test as started: {}", e);
                                }

                                let client = client.clone();
                                let endpoint = endpoint.clone();
                                let stats = stats.clone();
                                let completed = completed_assignments.clone();

                                tokio::spawn(async move {
                                    info!("Executing test assignment: {} -> {}", node_id, target_node_id);

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
                                                TestAssignmentResult {
                                                    result_type: TestResultType::Error,
                                                    error: Some(ErrorResult {
                                                        error: e.to_string(),
                                                        duration: Duration::from_secs(0),
                                                        test_type: None,
                                                        node_id: None,
                                                    }),
                                                    ..Default::default()
                                                },
                                            )
                                        }
                                    };

                                    if let Err(e) = client
                                            .submit_result(
                                                test_run_id,
                                                node_id,
                                                target_node_id,
                                                success,
                                                result_data,
                                            )
                                            .await
                                        {
                                            error!("Failed to submit result: {}", e);
                                        } else {
                                            completed.write().await.insert((
                                                test_run_id,
                                                target_node_id,
                                                retry_count,
                                            ));

                                            if success {
                                                stats.increment_completed();
                                            } else {
                                                stats.increment_failed();
                                            }

                                            info!("Test completed, entering 0.5-second cool-down period");
                                            tokio::time::sleep(Duration::from_millis(500)).await;
                                        }
                                });
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
            #[allow(clippy::needless_return)]
            return;
        }
    }
}

/// Background task to periodically update port variation detection
async fn port_variation_update_task(
    client: Arc<SwarmClient>,
    config: SwarmConfig,
    mut shutdown_rx: broadcast::Receiver<()>,
) {
    // Only run this task if port variation detection is enabled
    if !config.port_variation.enabled {
        debug!("Port variation detection disabled, task not starting");
        return;
    }

    debug!("Starting periodic port variation detection updates");
    let mut interval = tokio::time::interval(Duration::from_secs(300)); // Check every 5 minutes
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                debug!("Running periodic port variation detection update");
                match probe_port_variation(&config.port_variation, config.coordinator_node_id).await {
                    Ok(result) => {
                        client.update_port_variation(Some(result)).await;
                        debug!("Updated port variation detection results");
                    }
                    Err(e) => {
                        debug!("Periodic port variation detection failed: {}", e);
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                info!("Port variation update task received shutdown signal");
                break;
            }
        }
    }
}

async fn incoming_connection_handler_task(
    endpoint: Arc<Endpoint>,
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
                            let remote = conn.remote_node_id().ok();
                            info!("Accepted test connection from {:?}", remote);

                            tokio::spawn(async move {
                                handle_incoming_test_connection(conn).await;
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

async fn handle_incoming_test_connection(conn: iroh::endpoint::Connection) {
    loop {
        match conn.accept_bi().await {
            Ok((send, recv)) => {
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
    debug!("Connection handler complete");
}

/// Handle an individual test stream
async fn handle_incoming_test_stream(
    send: iroh::endpoint::SendStream,
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
                    let stream_id = recv.id();
                    handle_throughput_stream(send, recv, header, stream_id)
                        .await
                        .ok();
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
            if let Some(ping_msg) = LatencyMessage::from_bytes(&ping_buf) {
                if let LatencyMessage::Ping(n) = ping_msg {
                    let pong_msg = LatencyMessage::pong(n);
                    let pong_data = pong_msg.to_bytes();
                    send.write_all(&pong_data).await.ok();
                    trace!("Sent PONG response for ping {}", n);
                }
            } else {
                warn!("Failed to parse ping message");
            }
        }
        Err(e) => {
            warn!("Failed to read ping data: {}", e);
        }
    }
    send.finish().ok();
}

async fn handle_throughput_stream(
    send: iroh::endpoint::SendStream,
    recv: iroh::endpoint::RecvStream,
    header: TestProtocolHeader,
    stream_id: StreamId,
) -> Result<()> {
    let data_size = header.data_size;
    let chunk_size = header.chunk_size.unwrap_or(1024 * 1024); // Default to 1MB

    debug!(
        "Handling throughput stream {} with {} bytes, chunk size {} bytes",
        stream_id, data_size, chunk_size
    );

    let (bytes_received, bytes_sent, duration) =
        handle_bidirectional_transfer(send, recv, data_size, chunk_size)
            .await
            .map_err(|e| anyhow::anyhow!("Stream {} transfer failed: {}", stream_id, e))?;

    debug!(
        "Stream {} completed: received {} bytes, sent {} bytes in {:?}",
        stream_id, bytes_received, bytes_sent, duration
    );
    Ok(())
}

/// Run the swarm client
pub async fn run_swarm_client(
    config: SwarmConfig,
    ssh_key_path: &Path,
    metrics_registry: crate::metrics::IrohMetricsRegistry,
) -> Result<()> {
    let client =
        Arc::new(SwarmClient::connect(config.clone(), ssh_key_path, metrics_registry).await?);
    let endpoint = client.endpoint();

    let endpoint_for_shutdown = (*endpoint).clone();
    race(
        close_endpoint_on_ctrl_c(endpoint_for_shutdown),
        run_swarm_client_inner(client, config, endpoint, ssh_key_path),
    )
    .await;

    Ok(())
}

/// Inner function that runs the actual swarm client logic
async fn run_swarm_client_inner(
    client: Arc<SwarmClient>,
    config: SwarmConfig,
    endpoint: Arc<Endpoint>,
    ssh_key_path: &Path,
) {
    let node_id = client.node_id();

    // Create n0des client for metrics collection using coordinator
    info!(
        "Creating iroh_n0des::Client for metrics collection to coordinator {}",
        config.coordinator_node_id
    );

    let _rpc_client = match iroh_n0des::Client::builder(&endpoint)
        .ssh_key_from_file(ssh_key_path)
        .await
    {
        Ok(builder) => match builder.build(config.coordinator_node_id).await {
            Ok(client) => client,
            Err(e) => {
                error!("Failed to build n0des client: {}", e);
                return;
            }
        },
        Err(e) => {
            error!("Failed to create n0des client builder: {}", e);
            return;
        }
    };

    info!("Connected to coordinator, starting swarm operations as a client node");

    // Create shutdown broadcast channel
    let (shutdown_tx, _) = broadcast::channel(1);

    // Spawn background task to periodically update port variation detection
    let _port_variation_handle = tokio::spawn(port_variation_update_task(
        client.clone(),
        config.clone(),
        shutdown_tx.subscribe(),
    ));

    // Wait for DNS discovery to be ready
    info!("Waiting 10 seconds for DNS discovery to be ready...");
    tokio::time::sleep(Duration::from_secs(10)).await;

    // Track completed assignments to avoid duplicates
    // Key: (test_run_id, node_id, retry_count)
    let completed_assignments = Arc::new(RwLock::new(
        HashSet::<(uuid::Uuid, iroh::NodeId, i32)>::new(),
    ));

    let _connection_handler_handle = tokio::spawn(incoming_connection_handler_task(
        endpoint.clone(),
        shutdown_tx.subscribe(),
    ));

    // Main loop for getting and executing test assignments
    let stats = Arc::new(SwarmStats::default());
    let mut assignment_interval = tokio::time::interval(config.assignment_interval);
    assignment_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = assignment_interval.tick() => {
                let client = client.clone();
                let completed_assignments = completed_assignments.clone();
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
}
