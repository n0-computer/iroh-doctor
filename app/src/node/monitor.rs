//! The active connection monitor: dial the probe ALPN, drive the shared
//! core monitor, and snapshot QUIC paths for the UI.

use std::sync::Arc;

use iroh::endpoint;
use iroh::{Endpoint, EndpointAddr};
use iroh_doctor_core::monitor::{self, MonitorConfig, MonitorEvent};
use tokio::sync::Mutex;

use super::*;

/// Projects core's path snapshots onto the app's render-facing [`PathInfo`].
///
/// The path classification and RTT reading live in
/// [`iroh_doctor_core::monitor::snapshot_paths`]; this only maps them to the
/// UI type (RTT in milliseconds, the app's [`PathKind`] naming).
pub(crate) fn snapshot_paths(conn: &endpoint::Connection) -> Vec<PathInfo> {
    iroh_doctor_core::monitor::snapshot_paths(conn)
        .into_iter()
        .map(|p| PathInfo {
            addr: p.addr,
            kind: match p.kind {
                iroh_doctor_core::monitor::PathKind::Direct => PathKind::Ip,
                iroh_doctor_core::monitor::PathKind::Relay => PathKind::Relay,
                iroh_doctor_core::monitor::PathKind::Custom => PathKind::Custom,
            },
            selected: p.selected,
            rtt_ms: p.rtt.as_secs_f64() * 1000.0,
        })
        .collect()
}

/// Dials `addr` on the iroh-doctor probe ALPN and runs the active monitor
/// against the peer until the connection ends.
///
/// On a successful dial the connection is published into `conn_slot` so the
/// long-lived paths sampler drives the latency graph and path table. The
/// shared [`iroh_doctor_core::monitor::run`] composition then reports
/// time-to-first-direct-byte and drives the probe client; its throughput
/// samples are surfaced via `on_throughput`. We leave `watch_paths` off
/// because the app's periodic sampler already feeds the paths view, and the
/// graph's latency comes from QUIC's smoothed RTT (matching
/// `iroh-doctor accept`), so the client-side latency samples are ignored.
pub(crate) async fn run_monitor(
    endpoint: Endpoint,
    addr: EndpointAddr,
    conn_slot: Arc<Mutex<Option<endpoint::Connection>>>,
    on_state: StateCb,
    on_throughput: ThroughputCb,
    on_ttfdb: TtfdbCb,
) {
    let started = std::time::Instant::now();
    let conn = match endpoint.connect(addr, iroh_doctor_core::probe::ALPN).await {
        Ok(conn) => conn,
        Err(e) => {
            on_state(ConnectionState::Error(format!("connect failed: {e:#}")));
            return;
        }
    };
    let peer_id = conn.remote_id().to_string();
    let peer_short_id: String = peer_id.chars().take(10).collect();

    // Take over conn_slot for the paths sampler and announce the connection.
    {
        let mut slot = conn_slot.lock().await;
        *slot = Some(conn.clone());
    }
    on_state(ConnectionState::Connected {
        peer_id: peer_id.clone(),
        peer_short_id: peer_short_id.clone(),
    });

    let config = MonitorConfig {
        watch_paths: false,
        ..MonitorConfig::default()
    };
    let mut mon = monitor::run(&conn, config, started);
    while let Some(event) = mon.events.recv().await {
        match event {
            MonitorEvent::Throughput { bytes, elapsed } => {
                on_throughput(ThroughputSnapshot {
                    bytes,
                    elapsed,
                    mbps: iroh_doctor_core::probe::throughput_mbps(bytes, elapsed),
                });
            }
            MonitorEvent::Ttfdb(elapsed) => on_ttfdb(Some(elapsed)),
            MonitorEvent::Ended(_) => break,
            // Latency/State/Paths: the app's periodic sampler feeds those.
            _ => {}
        }
    }

    // The loop ended: the peer went away or the stream broke. Clear the slot
    // if it still holds our connection (a later dial may have replaced it)
    // and tell the UI.
    {
        let mut slot = conn_slot.lock().await;
        if slot
            .as_ref()
            .is_some_and(|c| c.remote_id().to_string() == peer_id)
        {
            *slot = None;
        }
    }
    on_state(ConnectionState::PeerDisconnected { peer_short_id });
}
