//! Accept command implementation.
//!
//! `accept` prints the endpoint id and waits. The incoming ALPN decides the
//! mode for each connection: a probe stream opens a live monitor dashboard
//! (the same one `connect` shows); a doctor stream runs the throughput
//! test as the active side. There is no flag - the peer's choice picks the
//! mode automatically.

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use iroh::{Endpoint, SecretKey};
use iroh_doctor_core::{
    monitor::{derive_state, snapshot_paths},
    probe::{handle_connection, handle_connection_with, ProbeEvent},
};
use n0_future::StreamExt;
use portable_atomic::AtomicU64;
use tokio_util::task::AbortOnDropHandle;
use tracing::warn;

use crate::{
    commands::monitor_view::{format_path_lines, MonitorView, HISTORY_LEN},
    doctor::{active_side, log_connection_changes, Gui, TestConfig},
};

/// Runs a closure on drop. Used to reset a counter or flag even if the
/// guarded future panics, so a panicking handler cannot permanently wedge
/// the accept loop's "one dashboard at a time" or connection-count state.
struct OnDrop<F: FnMut()>(F);

impl<F: FnMut()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}

/// Accepts incoming connections. Probe connections drive a live monitor;
/// doctor connections drive the throughput test.
pub async fn accept(
    secret_key: SecretKey,
    config: TestConfig,
    endpoint: Endpoint,
) -> anyhow::Result<()> {
    println!("endpoint id: {}", secret_key.public());
    println!("waiting for connections... (Ctrl-C to stop)");

    let connections = Arc::new(AtomicU64::default());
    let monitor_active = Arc::new(AtomicBool::new(false));

    while let Some(incoming) = endpoint.accept().await {
        let connecting = match incoming.accept() {
            Ok(connecting) => connecting,
            Err(err) => {
                warn!("incoming connection failed: {err:#}");
                continue;
            }
        };
        let connections = connections.clone();
        let endpoint = endpoint.clone();
        let monitor_active = monitor_active.clone();
        tokio::task::spawn(async move {
            let connection = match connecting.await {
                Ok(connection) => connection,
                Err(cause) => {
                    eprintln!("error accepting connection: {cause:#}");
                    return;
                }
            };

            if connection.alpn() == iroh_doctor_core::probe::ALPN {
                // First probe gets the dashboard; concurrent probes respond
                // silently in the background so two dashboards never fight
                // for the terminal.
                if monitor_active
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    // Reset on drop so a panic in the monitor cannot leave
                    // the flag stuck and silence every later dashboard.
                    let _reset = OnDrop(|| monitor_active.store(false, Ordering::SeqCst));
                    run_probe_monitor(endpoint, connection).await;
                } else if let Err(cause) = handle_connection(connection).await {
                    warn!("probe connection failed: {cause:#}");
                }
                return;
            }

            // Doctor ALPN: the throughput test. The first concurrent
            // connection drives with a Gui, the rest run silently.
            let n = connections.fetch_add(1, portable_atomic::Ordering::SeqCst);
            // Decrement on drop so a panic in the test driver cannot leave
            // the counter stuck above zero and starve future tests of a Gui.
            let _dec = OnDrop(|| {
                connections.sub(1, portable_atomic::Ordering::SeqCst);
            });
            if n == 0 {
                let remote_peer_id = connection.remote_id();
                println!("accepted doctor test from {remote_peer_id}");
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
                    eprintln!("test finished after {dt}s: {cause:#}{close_reason}");
                } else {
                    eprintln!("test finished after {dt}s");
                }
            } else {
                active_side(&connection, &config, None).await.ok();
            }
        });
    }

    Ok(())
}

/// Renders the live monitor dashboard for one accepted probe connection.
/// Returns when the peer closes the connection or the handler errors.
async fn run_probe_monitor(endpoint: Endpoint, connection: iroh::endpoint::Connection) {
    let remote_peer = connection.remote_id();
    let gui = Gui::new(endpoint, remote_peer);
    let view = MonitorView::new(&gui.mp, MonitorView::accepted_header(remote_peer));
    let started = Instant::now();

    // All three background tasks are owned here via AbortOnDropHandle, so
    // they stop when this function returns rather than lingering on the
    // cloned connection.
    let _watcher = AbortOnDropHandle::new(spawn_paths_watcher(connection.clone(), view.clone()));
    let _ttfdb = {
        let conn = connection.clone();
        let view = view.clone();
        AbortOnDropHandle::new(tokio::spawn(async move {
            if let Some(elapsed) = iroh_doctor_core::monitor::ttfdb_watch(&conn, started).await {
                view.set_ttfdb(elapsed);
            }
        }))
    };

    // Surface throughput from the responder's perspective on each upload.
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel::<ProbeEvent>(8);
    let _pump = {
        let view = view.clone();
        AbortOnDropHandle::new(tokio::spawn(async move {
            while let Some(ev) = events_rx.recv().await {
                match ev {
                    ProbeEvent::UploadCompleted { bytes, elapsed } => {
                        view.set_throughput(bytes, elapsed);
                    }
                }
            }
        }))
    };

    // Read the close reason after the handler returns: before then the
    // connection is still open and would report `None`.
    let reason_conn = connection.clone();
    let result = handle_connection_with(connection, events_tx).await;
    let close_reason = reason_conn
        .close_reason()
        .map(|e| format!(" (reason: {e})"))
        .unwrap_or_default();
    if let Err(cause) = result {
        view.set_probe_ended("probe", format!("{cause:#}{close_reason}"));
    } else {
        view.set_probe_ended("probe", format!("closed{close_reason}"));
    }
}

/// Watches the connection's paths and refreshes state, paths, latency,
/// and the sparkline. Latency on the accept side comes from QUIC's
/// smoothed RTT on the selected path, since the peer drives the pings.
fn spawn_paths_watcher(
    connection: iroh::endpoint::Connection,
    view: MonitorView,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut paths = connection.paths_stream();
        let mut history: VecDeque<Duration> = VecDeque::with_capacity(HISTORY_LEN);
        // Re-read the current paths on every change notification. Latency on
        // this side is the selected path's smoothed RTT, since the peer
        // drives the pings.
        while paths.next().await.is_some() {
            let snaps = snapshot_paths(&connection);
            view.set_state(derive_state(&snaps));

            if let Some(p) = snaps.iter().find(|p| p.selected) {
                if history.len() == HISTORY_LEN {
                    history.pop_front();
                }
                history.push_back(p.rtt);
                view.set_latency(p.rtt, &history);
            }

            view.set_paths(format_path_lines(&snaps));
        }
    })
}
