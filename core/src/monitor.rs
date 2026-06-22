//! Connection-monitor helpers reused by both the cli's live `connect`
//! dashboard and the app's diagnostics view.
//!
//! The shape of a "live monitor" view is the same on either side: classify
//! the selected path (relay vs direct vs custom), list the paths and their
//! RTTs, and report the time it took for the first direct path to appear
//! (time-to-first-direct-byte). These helpers operate on an
//! `iroh::endpoint::Connection`, which both the dialing and the accepting
//! side hold, so the implementation is symmetric.

use std::time::{Duration, Instant};

use iroh::endpoint::Connection;
use n0_future::StreamExt;
use tokio::sync::mpsc;
use tokio_util::task::AbortOnDropHandle;

use crate::client::{Client, ClientConfig, ClientEnd, ClientSample};

/// Transport kind for one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// A direct IP path: holepunched UDP.
    Direct,
    /// A relayed path through a coordinator.
    Relay,
    /// A pluggable custom transport (BLE, Tor, ...).
    Custom,
}

/// Snapshot of one path of an iroh connection.
#[derive(Debug, Clone, PartialEq)]
pub struct PathSnapshot {
    /// `Display` form of the remote address.
    pub addr: String,
    /// What transport carries this path.
    pub kind: PathKind,
    /// Whether iroh is currently routing traffic through this path.
    pub selected: bool,
    /// Last-known smoothed RTT for this path.
    pub rtt: Duration,
}

/// High-level connection state derived from the path list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateKind {
    /// A direct IP path is selected.
    Direct,
    /// A relayed path is selected.
    Relay,
    /// A custom transport path is selected.
    Custom,
    /// No path is currently selected.
    NoPath,
}

/// Captures the connection's current paths.
#[must_use]
pub fn snapshot_paths(conn: &Connection) -> Vec<PathSnapshot> {
    conn.paths()
        .iter()
        .map(|p| {
            let addr = p.remote_addr();
            let kind = if addr.is_ip() {
                PathKind::Direct
            } else if addr.is_relay() {
                PathKind::Relay
            } else {
                PathKind::Custom
            };
            PathSnapshot {
                addr: addr.to_string(),
                kind,
                selected: p.is_selected(),
                rtt: p.rtt(),
            }
        })
        .collect()
}

/// Classifies a paths snapshot into a single high-level state.
#[must_use]
pub fn derive_state(paths: &[PathSnapshot]) -> StateKind {
    match paths.iter().find(|p| p.selected).map(|p| p.kind) {
        Some(PathKind::Direct) => StateKind::Direct,
        Some(PathKind::Relay) => StateKind::Relay,
        Some(PathKind::Custom) => StateKind::Custom,
        None => StateKind::NoPath,
    }
}

/// Resolves with the time it took for the connection to first select a
/// direct IP path, measured from `started`. Returns `None` if the path
/// stream ends without one ever being selected.
///
/// The same helper works for the dialing side (start the timer when the
/// dial begins) and the accepting side (start it when the connection is
/// accepted), so both crates report a comparable number for the same
/// physical holepunch.
#[must_use = "the resolved time-to-first-direct-byte is the point of awaiting this"]
pub async fn ttfdb_watch(conn: &Connection, started: Instant) -> Option<Duration> {
    let mut paths = conn.paths_stream();
    while let Some(path_list) = paths.next().await {
        if path_list
            .iter()
            .any(|p| p.is_selected() && p.remote_addr().is_ip())
        {
            return Some(started.elapsed());
        }
    }
    None
}

/// Configuration for the composed live monitor [`run`].
#[derive(Debug, Clone, Copy)]
pub struct MonitorConfig {
    /// Pacing for the active probe client (ping interval, upload cadence).
    pub client: ClientConfig,
    /// Also watch the connection's path stream and emit [`MonitorEvent::State`]
    /// and [`MonitorEvent::Paths`]. Callers that already drive their own paths
    /// view (e.g. a periodic sampler) leave this `false` to avoid a second
    /// reader of the same stream.
    pub watch_paths: bool,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            client: ClientConfig::default(),
            watch_paths: true,
        }
    }
}

/// One update from the composed monitor [`run`].
#[derive(Debug, Clone)]
pub enum MonitorEvent {
    /// The selected-path state changed. Only emitted when `watch_paths` is set.
    State(StateKind),
    /// A fresh path snapshot. Only emitted when `watch_paths` is set.
    Paths(Vec<PathSnapshot>),
    /// A ping round-trip sample from the active probe client.
    Latency { nonce: u32, rtt: Duration },
    /// A completed upload sample from the active probe client.
    Throughput { bytes: u64, elapsed: Duration },
    /// Time-to-first-direct-byte resolved.
    Ttfdb(Duration),
    /// The probe client loop ended; no more samples will arrive.
    Ended(ClientEnd),
}

/// A running monitor. Read [`MonitorEvent`]s from `events` until an
/// [`MonitorEvent::Ended`] arrives, then drop this handle to abort the
/// background tasks.
pub struct Monitor {
    /// Stream of monitor updates.
    pub events: mpsc::Receiver<MonitorEvent>,
    /// Background tasks (ttfdb watcher, optional paths watcher, probe client
    /// driver), aborted on drop so a finished or replaced monitor leaves
    /// nothing running on the connection.
    _tasks: Vec<AbortOnDropHandle<()>>,
}

/// Composes the live connection monitor: a time-to-first-direct-byte watcher,
/// the active probe client (latency + throughput), and optionally a path
/// watcher, all multiplexed onto one channel. `started` is the dial (or
/// accept) instant the ttfdb measurement is relative to.
///
/// This is the shared assembly behind both `iroh-doctor connect` and the
/// app's Connect view, so both report a connection the same way. Spawns onto
/// the current Tokio runtime.
#[must_use]
pub fn run(conn: &Connection, config: MonitorConfig, started: Instant) -> Monitor {
    let (tx, events) = mpsc::channel(32);
    let mut tasks = Vec::new();

    // Time-to-first-direct-byte.
    {
        let conn = conn.clone();
        let tx = tx.clone();
        tasks.push(AbortOnDropHandle::new(tokio::spawn(async move {
            if let Some(elapsed) = ttfdb_watch(&conn, started).await {
                let _ = tx.send(MonitorEvent::Ttfdb(elapsed)).await;
            }
        })));
    }

    // Optional path watcher: re-snapshot on every path-stream change.
    if config.watch_paths {
        let conn = conn.clone();
        let tx = tx.clone();
        tasks.push(AbortOnDropHandle::new(tokio::spawn(async move {
            let mut paths = conn.paths_stream();
            while paths.next().await.is_some() {
                let snaps = snapshot_paths(&conn);
                if tx
                    .send(MonitorEvent::State(derive_state(&snaps)))
                    .await
                    .is_err()
                {
                    break;
                }
                if tx.send(MonitorEvent::Paths(snaps)).await.is_err() {
                    break;
                }
            }
        })));
    }

    // Active probe client: forward each sample, then a single `Ended`.
    {
        let conn = conn.clone();
        let tx = tx.clone();
        tasks.push(AbortOnDropHandle::new(tokio::spawn(async move {
            let (sample_tx, mut sample_rx) = mpsc::channel(16);
            let driver =
                tokio::spawn(async move { Client::new(conn).run(config.client, sample_tx).await });
            while let Some(sample) = sample_rx.recv().await {
                let event = match sample {
                    ClientSample::Latency { nonce, rtt } => MonitorEvent::Latency { nonce, rtt },
                    ClientSample::Throughput { bytes, elapsed } => {
                        MonitorEvent::Throughput { bytes, elapsed }
                    }
                };
                if tx.send(event).await.is_err() {
                    return;
                }
            }
            let end = driver.await.unwrap_or(ClientEnd {
                phase: "monitor",
                cause: "driver task panicked".to_string(),
            });
            let _ = tx.send(MonitorEvent::Ended(end)).await;
        })));
    }

    Monitor {
        events,
        _tasks: tasks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(addr: &str, kind: PathKind, selected: bool, rtt_ms: u64) -> PathSnapshot {
        PathSnapshot {
            addr: addr.into(),
            kind,
            selected,
            rtt: Duration::from_millis(rtt_ms),
        }
    }

    #[test]
    fn derive_state_picks_the_selected_path_kind() {
        let paths = vec![
            snap("1.2.3.4:1", PathKind::Direct, true, 12),
            snap("https://relay/", PathKind::Relay, false, 58),
        ];
        assert_eq!(derive_state(&paths), StateKind::Direct);

        let paths = vec![
            snap("1.2.3.4:1", PathKind::Direct, false, 12),
            snap("https://relay/", PathKind::Relay, true, 58),
        ];
        assert_eq!(derive_state(&paths), StateKind::Relay);
    }

    #[test]
    fn derive_state_no_path_when_nothing_selected() {
        let paths = vec![snap("1.2.3.4:1", PathKind::Direct, false, 12)];
        assert_eq!(derive_state(&paths), StateKind::NoPath);
        assert_eq!(derive_state(&[]), StateKind::NoPath);
    }
}
