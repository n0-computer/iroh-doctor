//! The active connection monitor: dial the probe ALPN and drive the shared
//! core monitor, reporting samples through the node's event stream.

use iroh::{Endpoint, EndpointAddr};

use crate::monitor::{self, MonitorConfig, MonitorEvent};
use crate::probe;

use super::{ConnectionState, Events, SharedConn, ThroughputSnapshot};

/// Dials `addr` on the iroh-doctor probe ALPN and runs the active monitor
/// against the peer until the connection ends.
///
/// On a successful dial the connection is published into `conn_slot` so the
/// long-lived paths sampler drives the path table. The shared
/// [`crate::monitor::run`] composition then reports
/// time-to-first-direct-byte and drives the probe client; its throughput
/// samples and ping round-trips go out as node events, so the latency graph
/// plots the same series as `iroh-doctor connect`. We leave `watch_paths`
/// off because the node's periodic sampler already feeds the paths view
/// (and that sampler holds its path-RTT latency samples back while this
/// monitor runs).
pub(crate) async fn run_monitor(
    endpoint: Endpoint,
    addr: EndpointAddr,
    conn_slot: SharedConn,
    events: Events,
) {
    let started = std::time::Instant::now();
    let conn = match endpoint.connect(addr, probe::ALPN).await {
        Ok(conn) => conn,
        Err(e) => {
            events.state(ConnectionState::Error(format!("connect failed: {e:#}")));
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
    events.state(ConnectionState::Connected {
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
                events.throughput(ThroughputSnapshot {
                    bytes,
                    elapsed,
                    mbps: probe::throughput_mbps(bytes, elapsed),
                });
            }
            MonitorEvent::Latency { rtt, .. } => events.latency(rtt),
            MonitorEvent::Ttfdb(elapsed) => events.ttfdb(Some(elapsed)),
            MonitorEvent::Ended(_) => break,
            // State/Paths: the node's periodic sampler feeds those.
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
    events.state(ConnectionState::PeerDisconnected { peer_short_id });
}
