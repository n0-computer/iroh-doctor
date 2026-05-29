//! Connect command implementation

use std::{
    collections::VecDeque,
    net::SocketAddr,
    time::{Duration, Instant},
};

use iroh::{Endpoint, EndpointAddr, EndpointId, RelayUrl};
use iroh_doctor_core::monitor::{self, MonitorConfig, MonitorEvent};

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

/// Runs the live connection monitor: drives the shared
/// [`iroh_doctor_core::monitor::run`] composition (path watcher, ttfdb, and
/// the probe client) and renders each event onto the `Gui`'s dashboard.
/// Returns `Ok(())` cleanly once the peer goes away.
async fn monitor(
    gui: &Gui,
    endpoint_id: EndpointId,
    connection: &iroh::endpoint::Connection,
) -> anyhow::Result<()> {
    let view = MonitorView::new(&gui.mp, MonitorView::monitoring_header(endpoint_id));
    let started = Instant::now();
    let mut monitor = monitor::run(connection, MonitorConfig::default(), started);

    let mut history: VecDeque<Duration> = VecDeque::with_capacity(HISTORY_LEN);
    while let Some(event) = monitor.events.recv().await {
        match event {
            MonitorEvent::State(state) => view.set_state(state),
            MonitorEvent::Paths(snaps) => view.set_paths(format_path_lines(&snaps)),
            MonitorEvent::Latency { rtt, .. } => {
                if history.len() == HISTORY_LEN {
                    history.pop_front();
                }
                history.push_back(rtt);
                view.set_latency(rtt, &history);
            }
            MonitorEvent::Throughput { bytes, elapsed } => view.set_throughput(bytes, elapsed),
            MonitorEvent::Ttfdb(elapsed) => view.set_ttfdb(elapsed),
            MonitorEvent::Ended(end) => {
                view.set_probe_ended(end.phase, end.cause);
                break;
            }
        }
    }

    Ok(())
}
