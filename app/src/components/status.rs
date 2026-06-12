//! Text renderings of [`ConnectionState`] shared by the status indicator,
//! the connect bar, the event log, and the diagnostics export.

use crate::node::ConnectionState;

/// Compact label for the connection-event log and the export bundle.
pub fn short_event_label(state: &ConnectionState) -> String {
    match state {
        ConnectionState::Idle => "idle".into(),
        ConnectionState::Binding => "binding".into(),
        ConnectionState::Ready => "ready".into(),
        ConnectionState::Connecting => "connecting".into(),
        ConnectionState::Connected { peer_short_id, .. } => format!("connected: {peer_short_id}"),
        ConnectionState::PeerDisconnected { peer_short_id } => {
            format!("peer disconnected: {peer_short_id}")
        }
        ConnectionState::Error(msg) => format!("error: {msg}"),
    }
}

/// Sentence-style status for the global indicator and the connect bar.
pub fn status_line(state: &ConnectionState) -> String {
    match state {
        ConnectionState::Idle => "idle".into(),
        ConnectionState::Binding => "binding...".into(),
        ConnectionState::Ready => "ready".into(),
        ConnectionState::Connecting => "connecting...".into(),
        ConnectionState::Connected { peer_short_id, .. } => format!("connected to {peer_short_id}"),
        ConnectionState::PeerDisconnected { peer_short_id } => {
            format!("{peer_short_id} disconnected")
        }
        ConnectionState::Error(msg) => format!("error: {msg}"),
    }
}

/// The status indicator's `data-state` attribute value, also reused as the
/// event-log row kind.
pub fn status_kind(state: &ConnectionState) -> &'static str {
    match state {
        ConnectionState::Idle => "idle",
        ConnectionState::Binding => "pending",
        ConnectionState::Ready => "ready",
        ConnectionState::Connecting => "pending",
        ConnectionState::Connected { .. } => "connected",
        ConnectionState::PeerDisconnected { .. } => "disconnected",
        ConnectionState::Error(_) => "error",
    }
}
