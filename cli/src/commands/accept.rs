//! Accept command implementation

use std::{sync::Arc, time::Instant};

use iroh::{Endpoint, SecretKey};
use portable_atomic::AtomicU64;
use tracing::warn;

use crate::doctor::{active_side, format_addr, log_connection_changes, Gui, TestConfig};

/// Accepts the connections.
pub async fn accept(
    secret_key: SecretKey,
    config: TestConfig,
    endpoint: Endpoint,
) -> anyhow::Result<()> {
    let endpoint_addr = endpoint.addr();

    let remote_addrs = endpoint_addr
        .ip_addrs()
        .map(|addr| format!("--remote-endpoint {}", format_addr(*addr)))
        .collect::<Vec<_>>()
        .join(" ");
    // `accept` drives the throughput test, which pairs with the passive
    // side of `connect --test`. A plain `connect` runs the live monitor
    // instead, so the test instructions must pass `--test`.
    println!("Run the throughput test against this node with one of the following commands:\n");
    println!(
        "\tUsing the relay url and direct connections:\niroh-doctor connect --test {} {}\n",
        secret_key.public(),
        remote_addrs,
    );
    if let Some(relay_url) = endpoint_addr.relay_urls().next() {
        println!(
            "\tUsing just the relay url:\niroh-doctor connect --test {} --relay-url {}\n",
            secret_key.public(),
            relay_url,
        );
    }
    if !endpoint.address_lookup()?.is_empty() {
        println!(
            "\tUsing just the node id:\niroh-doctor connect --test {}\n",
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
            let connection = match connecting.await {
                Ok(connection) => connection,
                Err(cause) => {
                    eprintln!("error accepting connection {cause}");
                    return;
                }
            };

            // Probe connections (the live monitor) just need the passive
            // responder; they do not participate in the doctor-test counter.
            if connection.alpn() == iroh_doctor_core::probe::ALPN {
                if let Err(cause) = iroh_doctor_core::probe::handle_connection(connection).await {
                    warn!("probe connection failed: {cause:#}");
                }
                return;
            }

            // Doctor ALPN: run the throughput test. The first connection
            // drives with a Gui, the rest run silently.
            let n = connections.fetch_add(1, portable_atomic::Ordering::SeqCst);
            if n == 0 {
                let remote_peer_id = connection.remote_id();
                println!("Accepted connection from {remote_peer_id}");
                let t0 = Instant::now();
                let gui = Gui::new(endpoint.clone(), remote_peer_id);
                log_connection_changes(gui.mp.clone(), remote_peer_id, connection.clone());
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
            connections.sub(1, portable_atomic::Ordering::SeqCst);
        });
    }

    Ok(())
}
