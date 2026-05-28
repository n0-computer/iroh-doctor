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
#[derive(Debug, Clone)]
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

/// Capture the connection's current paths.
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

/// Classify a paths snapshot into a single high-level state.
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
