//! Connect command implementation

use std::{net::SocketAddr, time::Duration};

use iroh::{Endpoint, EndpointAddr, EndpointId, RelayUrl};

use iroh_doctor_core::probe::{throughput_mbps, ProbeClient};
use n0_future::StreamExt;

use crate::doctor::{log_connection_changes, passive_side, Gui};

/// Connects to a [`EndpointId`].
///
/// By default this runs a live connection monitor against the peer's probe
/// protocol (latency over time, periodic throughput). With `test` set it runs
/// the legacy doctor throughput test as the passive side, pairing with
/// `iroh-doctor accept`.
pub async fn connect(
    endpoint_id: EndpointId,
    direct_addresses: Vec<SocketAddr>,
    relay_url: Option<RelayUrl>,
    endpoint: Endpoint,
    test: bool,
) -> anyhow::Result<()> {
    tracing::info!("dialing {:?}", endpoint_id);
    let mut endpoint_addr = EndpointAddr::new(endpoint_id);
    if let Some(relay_url) = relay_url {
        endpoint_addr = endpoint_addr.with_relay_url(relay_url);
    }
    for ip_addr in direct_addresses {
        endpoint_addr = endpoint_addr.with_ip_addr(ip_addr);
    }

    let alpn = if test {
        iroh_doctor_core::doctor::ALPN
    } else {
        iroh_doctor_core::probe::ALPN
    };

    let conn = endpoint.connect(endpoint_addr, alpn).await;
    match conn {
        Ok(connection) => {
            let gui = Gui::new(endpoint, endpoint_id);
            log_connection_changes(gui.mp.clone(), endpoint_id, connection.clone());

            let close_reason = connection
                .close_reason()
                .map(|e| format!(" (reason: {e})"))
                .unwrap_or_default();

            if test {
                if let Err(cause) = passive_side(gui, &connection).await {
                    eprintln!("error handling connection: {cause}{close_reason}");
                } else {
                    eprintln!("Connection closed{close_reason}");
                }
            } else if let Err(cause) = monitor(&gui, &connection).await {
                eprintln!("error monitoring connection: {cause}{close_reason}");
            } else {
                eprintln!("Connection closed{close_reason}");
            }
        }
        Err(cause) => {
            eprintln!("unable to connect to {endpoint_id}: {cause}");
        }
    }

    Ok(())
}

/// Runs the live connection monitor: repeatedly pings the peer's probe
/// responder, tracking latency stats, and periodically measures upload
/// throughput. Returns `Ok(())` cleanly once the peer goes away (e.g. on
/// ctrl_c the dispatch closes the endpoint, the next ping errors, and we
/// exit).
async fn monitor(gui: &Gui, connection: &iroh::endpoint::Connection) -> anyhow::Result<()> {
    // Watch for the first selected direct (ip) path and report time-to-first
    // direct byte once.
    spawn_ttfdb(connection.clone(), gui.mp.clone());

    let mut client = ProbeClient::connect(connection).await?;

    let mut nonce: u32 = 0;
    let mut count: u64 = 0;
    let mut min = Duration::MAX;
    let mut max = Duration::ZERO;
    let mut total = Duration::ZERO;

    loop {
        match client.ping(nonce).await {
            Ok(rtt) => {
                count += 1;
                min = min.min(rtt);
                max = max.max(rtt);
                total += rtt;
                let avg = total / count as u32;
                gui.mp
                    .println(format!(
                        "latency: {:.2} ms (min {:.2} / avg {:.2} / max {:.2}, n={count})",
                        rtt.as_secs_f64() * 1000.0,
                        min.as_secs_f64() * 1000.0,
                        avg.as_secs_f64() * 1000.0,
                        max.as_secs_f64() * 1000.0,
                    ))
                    .ok();
            }
            Err(cause) => {
                gui.mp.println(format!("latency probe ended: {cause}")).ok();
                break;
            }
        }

        if nonce.is_multiple_of(10) {
            const UPLOAD_BYTES: u64 = 1024 * 1024;
            match client.upload(UPLOAD_BYTES).await {
                Ok(elapsed) => {
                    let mbps = throughput_mbps(UPLOAD_BYTES, elapsed)
                        .map(|m| format!("{m:.2}"))
                        .unwrap_or_else(|| "?".to_string());
                    gui.mp.println(format!("throughput: {mbps} Mbps")).ok();
                }
                Err(cause) => {
                    gui.mp
                        .println(format!("throughput probe ended: {cause}"))
                        .ok();
                    break;
                }
            }
        }

        nonce = nonce.wrapping_add(1);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    Ok(())
}

/// Watches the connection's paths and prints "time to first direct byte" once,
/// when a selected direct (ip) path first appears.
fn spawn_ttfdb(connection: iroh::endpoint::Connection, mp: indicatif::MultiProgress) {
    tokio::spawn(async move {
        let start = std::time::Instant::now();
        let mut paths = connection.paths_stream();
        while let Some(path_list) = paths.next().await {
            let has_direct = path_list
                .iter()
                .any(|p| p.is_selected() && p.remote_addr().is_ip());
            if has_direct {
                mp.println(format!("time to first direct byte: {:?}", start.elapsed()))
                    .ok();
                break;
            }
        }
    });
}
