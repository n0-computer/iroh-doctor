//! Connect command implementation

use std::{
    collections::VecDeque,
    net::SocketAddr,
    time::{Duration, Instant},
};

use iroh::{Endpoint, EndpointAddr, EndpointId, RelayUrl};
use iroh_doctor_core::monitor::{derive_state, snapshot_paths};
use iroh_doctor_core::probe::{run_client, ClientConfig, ClientSample};
use n0_future::StreamExt;
use tokio_util::task::AbortOnDropHandle;

use crate::commands::monitor_view::{format_path_lines, MonitorView, HISTORY_LEN};
use crate::doctor::Gui;

/// Connects to a [`EndpointId`] and runs a live connection monitor against
/// the peer's probe protocol: state, paths, latency over time, periodic
/// throughput, and time-to-first-direct-byte.
pub async fn connect(
    endpoint_id: EndpointId,
    direct_addresses: Vec<SocketAddr>,
    relay_url: Option<RelayUrl>,
    endpoint: Endpoint,
) -> anyhow::Result<()> {
    let res = run(endpoint_id, direct_addresses, relay_url, endpoint.clone()).await;
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
) -> anyhow::Result<()> {
    tracing::info!("dialing {:?}", endpoint_id);
    let mut endpoint_addr = EndpointAddr::new(endpoint_id);
    if let Some(relay_url) = relay_url {
        endpoint_addr = endpoint_addr.with_relay_url(relay_url);
    }
    for ip_addr in direct_addresses {
        endpoint_addr = endpoint_addr.with_ip_addr(ip_addr);
    }

    eprintln!("dialing {endpoint_id} (monitor)...");
    let dial = tokio::time::timeout(
        Duration::from_secs(30),
        endpoint.connect(endpoint_addr, iroh_doctor_core::probe::ALPN),
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
    eprintln!("connected; starting monitor...");

    let gui = Gui::new(endpoint, endpoint_id);
    let close_reason = connection
        .close_reason()
        .map(|e| format!(" (reason: {e})"))
        .unwrap_or_default();

    if let Err(cause) = monitor(&gui, endpoint_id, &connection).await {
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

    // Drive the shared probe client loop in the background and render each
    // sample as it arrives. The loop owns a connection clone so it outlives
    // this borrow; AbortOnDropHandle stops it if `monitor` returns early.
    let (samples_tx, mut samples_rx) = tokio::sync::mpsc::channel::<ClientSample>(16);
    let driver = {
        let conn = connection.clone();
        AbortOnDropHandle::new(tokio::spawn(async move {
            run_client(&conn, ClientConfig::default(), samples_tx).await
        }))
    };

    let mut history: VecDeque<Duration> = VecDeque::with_capacity(HISTORY_LEN);
    while let Some(sample) = samples_rx.recv().await {
        match sample {
            ClientSample::Latency { rtt, .. } => {
                if history.len() == HISTORY_LEN {
                    history.pop_front();
                }
                history.push_back(rtt);
                view.set_latency(rtt, &history);
            }
            ClientSample::Throughput { bytes, elapsed } => view.set_throughput(bytes, elapsed),
        }
    }

    // The sender dropped, so the driver has finished. Surface why the probe
    // ended; a join error only happens if it was aborted, which has nothing
    // to report.
    if let Ok(end) = driver.await {
        view.set_probe_ended(end.phase, end.cause);
    }

    Ok(())
}
