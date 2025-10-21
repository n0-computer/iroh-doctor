//! Swarm runner implementation

use std::{path::Path, sync::Arc, time::Duration};

use anyhow::Result;
use futures_lite::future::race;
use iroh::{
    endpoint::{Connection, StreamId},
    protocol::{AcceptError, ProtocolHandler, Router},
    Endpoint, EndpointId,
};
use iroh_n0des;
use tokio::{sync::broadcast, task::JoinSet};
use tracing::{debug, error, info, trace, warn};

use crate::{
    doctor::close_endpoint_on_ctrl_c,
    metrics::IrohMetricsRegistry,
    swarm::{
        client::SwarmClient,
        config::SwarmConfig,
        execution::perform_test_assignment,
        tests::protocol::{
            LatencyMessage, TestProtocolHeader, TestProtocolType, DOCTOR_SWARM_ALPN,
        },
        transfer_utils::handle_bidirectional_transfer,
        types::{
            ErrorResult, SwarmStats, TestAssignmentResult, DEFAULT_CHUNK_SIZE,
            DEFAULT_CONNECTION_TIMEOUT, DEFAULT_DATA_TRANSFER_TIMEOUT,
        },
    },
};

/// Protocol handler for incoming swarm test connections
#[derive(Debug, Clone)]
struct SwarmProtocolHandler;

impl ProtocolHandler for SwarmProtocolHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let remote = connection.remote_id()?;
        debug!("Accepted test connection from {:?}", remote);

        // Track stream handling tasks
        let mut stream_tasks = JoinSet::new();

        // Handle all streams on this connection
        loop {
            tokio::select! {
                result = connection.accept_bi() => {
                    match result {
                        Ok((send, recv)) => {
                            stream_tasks.spawn(async move {
                                handle_incoming_test_stream(send, recv).await;
                            });
                        }
                        Err(e) => {
                            debug!("No more streams on connection from {}: {}", remote, e);
                            break;
                        }
                    }
                }
                _ = stream_tasks.join_next(), if !stream_tasks.is_empty() => {
                }
            }
        }

        // Wait for all remaining stream tasks to complete
        while stream_tasks.join_next().await.is_some() {}

        Ok(())
    }
}

async fn assignment_processing_task(
    client: Arc<SwarmClient>,
    node_id: EndpointId,
    endpoint: Endpoint,
    stats: Arc<SwarmStats>,
    mut shutdown_rx: broadcast::Receiver<()>,
    data_transfer_timeout: Duration,
) -> Option<tokio::task::JoinHandle<()>> {
    tokio::select! {
        result = client.get_assignments() => {
            match result {
                Ok(assignments) => {
                    if let Some(assignment) = assignments.into_iter().next() {
                        let test_run_id = assignment.test_run_id;
                        let target_node_id = assignment.node_id;

                        if let Err(e) = client
                            .mark_test_started(test_run_id, target_node_id)
                            .await
                        {
                            warn!("Failed to mark test as started: {}", e);
                        }

                        let client = client.clone();
                        let endpoint = endpoint.clone();
                        let stats = stats.clone();
                        let mut shutdown_rx = shutdown_rx;

                        return Some(tokio::spawn(async move {
                            debug!("Executing test assignment: {} -> {}", node_id, target_node_id);

                            tokio::select! {
                                result = perform_test_assignment(
                                    assignment,
                                    endpoint.clone(),
                                    node_id,
                                    data_transfer_timeout,
                                ) => {
                                    let result_data = match result {
                                        Ok(result) => result,
                                        Err(e) => {
                                            error!("Failed to perform test: {}", e);
                                            TestAssignmentResult::Error(ErrorResult {
                                                error: e.to_string(),
                                                duration: Duration::from_secs(0),
                                                test_type: None,
                                                remote_id: None,
                                                connection_type: None,
                                            })
                                        }
                                    };

                                    let result_success = !matches!(&result_data, TestAssignmentResult::Error(_));

                                    if let Err(e) = client
                                        .submit_result(
                                            test_run_id,
                                            target_node_id,
                                            result_data,
                                        )
                                        .await
                                    {
                                        error!("Failed to submit result: {}", e);
                                    } else if result_success {
                                        stats.increment_completed();
                                    } else {
                                        stats.increment_failed();
                                    }
                                }
                                _ = shutdown_rx.recv() => {
                                    debug!("Test execution task received shutdown signal");
                                }
                            }
                        }));
                    }
                }
                Err(e) => {
                    error!("Failed to get assignments: {}", e);
                }
            }
            None
        }
        _ = shutdown_rx.recv() => {
            debug!("Assignment processing task received shutdown signal");
            None
        }
    }
}

/// Handle an individual test stream
async fn handle_incoming_test_stream(
    send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
) {
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
    let chunk_size = header.chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE);

    let (_bytes_received, _bytes_sent, _duration) =
        handle_bidirectional_transfer(send, recv, data_size, chunk_size)
            .await
            .map_err(|e| anyhow::anyhow!("Stream {stream_id} transfer failed: {e}"))?;
    Ok(())
}

/// Run the swarm client
pub async fn run_swarm_client(
    config: SwarmConfig,
    ssh_key_path: &Path,
    metrics_registry: IrohMetricsRegistry,
) -> Result<()> {
    let client =
        Arc::new(SwarmClient::connect(config.clone(), ssh_key_path, metrics_registry).await?);
    let endpoint = client.endpoint();

    // Set up the protocol router to handle incoming test connections
    let router = Router::builder(endpoint.clone())
        .accept(DOCTOR_SWARM_ALPN, SwarmProtocolHandler)
        .spawn();

    race(
        async {
            close_endpoint_on_ctrl_c(router.endpoint().clone()).await;
            router.shutdown().await.ok();
        },
        run_swarm_client_inner(client, config, endpoint, ssh_key_path),
    )
    .await;

    Ok(())
}

/// Inner function that runs the actual swarm client logic
async fn run_swarm_client_inner(
    client: Arc<SwarmClient>,
    config: SwarmConfig,
    endpoint: Endpoint,
    ssh_key_path: &Path,
) {
    let node_id = client.id();

    // Keep the n0des client alive for metrics collection
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

    info!("Starting swarm operations");

    let (shutdown_tx, _) = broadcast::channel(1);

    tokio::time::sleep(Duration::from_secs(1)).await;

    let stats = Arc::new(SwarmStats::default());
    let mut assignment_interval = tokio::time::interval(config.assignment_interval);
    assignment_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let data_transfer_timeout = config
        .data_transfer_timeout
        .unwrap_or(DEFAULT_DATA_TRANSFER_TIMEOUT);

    let mut test_execution_tasks = JoinSet::new();

    loop {
        tokio::select! {
            _ = assignment_interval.tick() => {
                debug!("Checking for new assignments");
                let client = client.clone();
                let endpoint = endpoint.clone();
                let stats = stats.clone();
                let shutdown_rx = shutdown_tx.subscribe();

                test_execution_tasks.spawn(async move {
                    match tokio::time::timeout(
                        DEFAULT_CONNECTION_TIMEOUT,
                        assignment_processing_task(
                            client,
                            node_id,
                            endpoint,
                            stats,
                            shutdown_rx,
                            data_transfer_timeout,
                        )
                    ).await {
                        Ok(Some(handle)) => {
                            handle.await.ok();
                        }
                        Ok(None) => {
                        }
                        Err(_) => {
                            warn!("Assignment processing timed out");
                        }
                    }
                });
            }
            Some(_) = test_execution_tasks.join_next() => {
            }
        }
    }
}
