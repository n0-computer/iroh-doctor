//! Accept command implementation

use std::{sync::Arc, time::Instant};

use iroh::{Endpoint, SecretKey};
use portable_atomic::AtomicU64;
use tracing::warn;

use crate::doctor::{Gui, TestConfig, active_side, format_addr, log_connection_changes};

/// Accepts the connections.
pub async fn accept(
    secret_key: SecretKey,
    config: TestConfig,
    endpoint: Endpoint,
) -> anyhow::Result<()> {
    let endpoints = endpoint.node_addr().direct_addresses;

    let remote_addrs = endpoints
        .iter()
        .map(|addr| format!("--remote-endpoint {}", format_addr(*addr)))
        .collect::<Vec<_>>()
        .join(" ");
    println!("Connect to this node using one of the following commands:\n");
    println!(
        "\tUsing the relay url and direct connections:\niroh-doctor connect {} {}\n",
        secret_key.public(),
        remote_addrs,
    );
    if let Some(relay_url) = endpoint.node_addr().relay_url {
        println!(
            "\tUsing just the relay url:\niroh-doctor connect {} --relay-url {}\n",
            secret_key.public(),
            relay_url,
        );
    }
    if !endpoint.discovery().is_empty() {
        println!(
            "\tUsing just the node id:\niroh-doctor connect {}\n",
            secret_key.public(),
        );
    }
    let connections = Arc::new(AtomicU64::default());
    while let Some(incoming) = endpoint.accept().await {
        let connecting = match incoming.accept() {
            Ok(connecting) => connecting,
            Err(err) => {
                warn!("incoming connection failed: {err:#}");
                // we can carry on in these cases:
                // this can be caused by retransmitted datagrams
                continue;
            }
        };
        let connections = connections.clone();
        let endpoint = endpoint.clone();
        tokio::task::spawn(async move {
            let n = connections.fetch_add(1, portable_atomic::Ordering::SeqCst);
            match connecting.await {
                Ok(connection) => {
                    if n == 0 {
                        let Ok(remote_peer_id) = connection.remote_node_id() else {
                            return;
                        };
                        println!("Accepted connection from {remote_peer_id}");
                        let t0 = Instant::now();
                        let gui = Gui::new(endpoint.clone(), remote_peer_id);
                        if let Some(conn_type) = endpoint.conn_type(remote_peer_id) {
                            log_connection_changes(gui.mp.clone(), remote_peer_id, conn_type);
                        }
                        let res = active_side(&connection, &config, Some(&gui)).await;
                        gui.clear();
                        let dt = t0.elapsed().as_secs_f64();
                        if let Err(cause) = res {
                            let close_reason = connection
                                .close_reason()
                                .map(|e| format!(" (reason: {e})"))
                                .unwrap_or_default();
                            eprintln!("Test finished after {dt}s: {cause}{close_reason}",);
                        } else {
                            eprintln!("Test finished after {dt}s",);
                        }
                    } else {
                        // silent
                        active_side(&connection, &config, None).await.ok();
                    }
                }
                Err(cause) => {
                    eprintln!("error accepting connection {cause}");
                }
            };
            connections.sub(1, portable_atomic::Ordering::SeqCst);
        });
    }

    Ok(())
}
