//! Connect command implementation

use std::{
    collections::VecDeque,
    net::SocketAddr,
    time::{Duration, Instant},
};

use iroh::{Endpoint, EndpointAddr, EndpointId, RelayUrl};
use iroh_doctor_core::monitor::{derive_state, snapshot_paths};
use iroh_doctor_core::probe::ProbeClient;
use n0_future::StreamExt;
use tokio_util::task::AbortOnDropHandle;

use crate::commands::monitor_view::{format_path_lines, MonitorView, HISTORY_LEN};
use crate::doctor::{log_connection_changes, passive_side, Gui};

/// Connects to a [`EndpointId`].
///
/// By default this runs a live connection monitor against the peer's probe
/// protocol (state, paths, latency over time, periodic throughput, ttfdb).
/// With `test` set it runs the legacy doctor throughput test as the passive
/// side, pairing with `iroh-doctor accept`.
pub async fn connect(
    endpoint_id: EndpointId,
    direct_addresses: Vec<SocketAddr>,
    relay_url: Option<RelayUrl>,
    endpoint: Endpoint,
    test: bool,
) -> anyhow::Result<()> {
    let res = run(
        endpoint_id,
        direct_addresses,
        relay_url,
        endpoint.clone(),
        test,
    )
    .await;
    // Close the endpoint gracefully on every exit path; otherwise iroh logs
    // "Endpoint dropped without calling `Endpoint::close`. Aborting
    // ungracefully." as the process tears down.
    endpoint.close().await;
    res
}

async fn run(
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

    let (alpn, alpn_label) = if test {
        (iroh_doctor_core::doctor::ALPN, "doctor test")
    } else {
        (iroh_doctor_core::probe::ALPN, "monitor")
    };

    eprintln!("dialing {endpoint_id} ({alpn_label})...");
    let dial = tokio::time::timeout(
        Duration::from_secs(30),
        endpoint.connect(endpoint_addr, alpn),
    )
    .await;
    let connection = match dial {
        Ok(Ok(c)) => c,
        Ok(Err(cause)) => {
            eprintln!("unable to connect to {endpoint_id}: {cause:#}");
            return Ok(());
        }
        Err(_) => {
            eprintln!(
                "timed out dialing {endpoint_id} after 30s.\n\
                 Try passing --relay-url and/or --remote-endpoint so the peer is reachable."
            );
            return Ok(());
        }
    };
    eprintln!("connected; starting {alpn_label}...");

    let gui = Gui::new(endpoint, endpoint_id);
    let close_reason = connection
        .close_reason()
        .map(|e| format!(" (reason: {e})"))
        .unwrap_or_default();

    if test {
        log_connection_changes(gui.mp.clone(), endpoint_id, connection.clone());
        if let Err(cause) = passive_side(gui, &connection).await {
            eprintln!("error handling connection: {cause:#}{close_reason}");
        } else {
            eprintln!("Connection closed{close_reason}");
        }
    } else if let Err(cause) = monitor(&gui, endpoint_id, &connection).await {
        eprintln!("error monitoring connection: {cause:#}{close_reason}");
    } else {
        eprintln!("Connection closed{close_reason}");
    }

    Ok(())
}

/// Watches the connection's path stream and updates the dashboard's
/// state and paths lines as paths come and go. TTFDB is handled by a
/// separate spawn that calls [`iroh_doctor_core::monitor::ttfdb_watch`].
fn spawn_paths_watcher(
    connection: iroh::endpoint::Connection,
    view: MonitorView,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut paths = connection.paths_stream();
        // Re-read the current paths on every change notification and refresh
        // the state and path-table rows from the shared core snapshot.
        while paths.next().await.is_some() {
            let snaps = snapshot_paths(&connection);
            view.set_state(derive_state(&snaps));
            view.set_paths(format_path_lines(&snaps));
        }
    })
}

/// Runs the live connection monitor: maintains a dashboard via the
/// existing `Gui`'s `MultiProgress`, repeatedly pings the peer's probe
/// responder for latency-over-time, and periodically uploads to measure
/// throughput. Returns `Ok(())` cleanly once the peer goes away.
async fn monitor(
    gui: &Gui,
    endpoint_id: EndpointId,
    connection: &iroh::endpoint::Connection,
) -> anyhow::Result<()> {
    let view = MonitorView::new(&gui.mp, MonitorView::monitoring_header(endpoint_id));
    let started = Instant::now();
    // Both background tasks are owned here via AbortOnDropHandle: when the
    // monitor returns they are aborted rather than left running on the
    // still-cloned connection.
    let _watcher = AbortOnDropHandle::new(spawn_paths_watcher(connection.clone(), view.clone()));
    // Time-to-first-direct-byte is computed by core so both the cli and the
    // app report the same number for the same physical holepunch.
    let _ttfdb = {
        let conn = connection.clone();
        let view = view.clone();
        AbortOnDropHandle::new(tokio::spawn(async move {
            if let Some(elapsed) = iroh_doctor_core::monitor::ttfdb_watch(&conn, started).await {
                view.set_ttfdb(elapsed);
            }
        }))
    };

    let mut client =
        match tokio::time::timeout(Duration::from_secs(10), ProbeClient::connect(connection)).await
        {
            Ok(Ok(c)) => c,
            Ok(Err(cause)) => {
                view.set_probe_ended("setup", cause);
                return Ok(());
            }
            Err(_) => {
                view.set_probe_ended("setup", "timed out opening probe stream after 10s");
                return Ok(());
            }
        };

    let mut nonce: u32 = 0;
    let mut history: VecDeque<Duration> = VecDeque::with_capacity(HISTORY_LEN);

    loop {
        match client.ping(nonce).await {
            Ok(rtt) => {
                if history.len() == HISTORY_LEN {
                    history.pop_front();
                }
                history.push_back(rtt);
                view.set_latency(rtt, &history);
            }
            Err(cause) => {
                view.set_probe_ended("latency", cause);
                break;
            }
        }

        // Every tenth tick, starting at the first, so the user gets an
        // immediate throughput sample and then one roughly every 10s.
        if nonce.is_multiple_of(10) {
            const UPLOAD_BYTES: u64 = 1024 * 1024;
            match client.upload(UPLOAD_BYTES).await {
                Ok(elapsed) => view.set_throughput(UPLOAD_BYTES, elapsed),
                Err(cause) => {
                    view.set_probe_ended("throughput", cause);
                    break;
                }
            }
        }

        nonce = nonce.wrapping_add(1);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    Ok(())
}
