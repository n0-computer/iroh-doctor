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
use iroh_doctor_core::probe::{handle_connection, handle_connection_with, ProbeEvent};
use n0_future::StreamExt;
use portable_atomic::AtomicU64;
use tracing::warn;

use crate::{
    commands::monitor_view::{MonitorView, StateKind, HISTORY_LEN},
    doctor::{active_side, log_connection_changes, Gui, TestConfig},
};

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
                    run_probe_monitor(endpoint, connection).await;
                    monitor_active.store(false, Ordering::SeqCst);
                } else if let Err(cause) = handle_connection(connection).await {
                    warn!("probe connection failed: {cause:#}");
                }
                return;
            }

            // Doctor ALPN: the throughput test. The first concurrent
            // connection drives with a Gui, the rest run silently.
            let n = connections.fetch_add(1, portable_atomic::Ordering::SeqCst);
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
            connections.sub(1, portable_atomic::Ordering::SeqCst);
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

    let _watcher = spawn_paths_watcher(connection.clone(), view.clone());
    {
        let conn = connection.clone();
        let view = view.clone();
        tokio::spawn(async move {
            if let Some(elapsed) = iroh_doctor_core::monitor::ttfdb_watch(&conn, started).await {
                view.set_ttfdb(elapsed);
            }
        });
    }

    // Surface throughput from the responder's perspective on each upload.
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel::<ProbeEvent>(8);
    {
        let view = view.clone();
        tokio::spawn(async move {
            while let Some(ev) = events_rx.recv().await {
                match ev {
                    ProbeEvent::UploadCompleted { bytes, elapsed } => {
                        view.set_throughput(bytes, elapsed);
                    }
                }
            }
        });
    }

    let close_reason = connection
        .close_reason()
        .map(|e| format!(" (reason: {e})"))
        .unwrap_or_default();
    if let Err(cause) = handle_connection_with(connection, events_tx).await {
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
        while let Some(path_list) = paths.next().await {
            let (label, kind) = match path_list.iter().find(|p| p.is_selected()) {
                Some(p) if p.remote_addr().is_ip() => ("direct", StateKind::Direct),
                Some(p) if p.remote_addr().is_relay() => ("relay", StateKind::Relay),
                Some(_) => ("custom", StateKind::Custom),
                None => ("no path", StateKind::Unknown),
            };
            view.set_state(label, kind);

            if let Some(p) = path_list.iter().find(|p| p.is_selected()) {
                if history.len() == HISTORY_LEN {
                    history.pop_front();
                }
                history.push_back(p.rtt());
                view.set_latency(p.rtt(), &history);
            }

            let lines: Vec<String> = path_list
                .iter()
                .map(|p| {
                    let sel = if p.is_selected() { '*' } else { ' ' };
                    let kind_str = if p.remote_addr().is_ip() {
                        "direct"
                    } else if p.remote_addr().is_relay() {
                        "relay "
                    } else {
                        "custom"
                    };
                    let rtt_ms = p.rtt().as_secs_f64() * 1000.0;
                    format!(
                        "{sel} {kind_str}  {:<44}  rtt {rtt_ms:>6.1} ms",
                        p.remote_addr().to_string(),
                    )
                })
                .collect();
            view.set_paths(lines);
        }
    })
}
