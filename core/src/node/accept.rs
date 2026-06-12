//! The probe protocol's accept side, registered on the node's
//! [`iroh::protocol::Router`]: serve each incoming peer probe, capped at
//! [`MAX_CONCURRENT_PROBE_SERVERS`] in parallel.

use std::sync::Arc;

use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use tokio::sync::{mpsc, Semaphore};
use tokio_util::task::AbortOnDropHandle;
use tracing::warn;

use crate::probe;

use super::{ConnectionState, Events, SharedConn, ThroughputSnapshot};

/// Maximum number of peer-probe server handlers we will run in parallel.
/// The probe ALPN has no auth layer; without this cap a peer that learned
/// our endpoint id could open arbitrarily many parallel probe sessions.
const MAX_CONCURRENT_PROBE_SERVERS: usize = 4;

/// Handles connections on [`crate::probe::ALPN`]: serves the probe
/// responder, surfaces the connection through the shared `conn_slot` so the
/// paths sampler covers incoming probes, and reports state, throughput, and
/// time-to-first-direct-byte through the node's event stream.
#[derive(Debug, Clone)]
pub(crate) struct ProbeProtocol {
    limit: Arc<Semaphore>,
    conn_slot: SharedConn,
    events: Events,
}

impl ProbeProtocol {
    pub(crate) fn new(conn_slot: SharedConn, events: Events) -> Self {
        Self {
            limit: Arc::new(Semaphore::new(MAX_CONCURRENT_PROBE_SERVERS)),
            conn_slot,
            events,
        }
    }
}

impl ProtocolHandler for ProbeProtocol {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        // At capacity: close with an application error code so the client
        // knows why it was rejected and can show it, rather than seeing an
        // anonymous connection loss.
        let Ok(_permit) = self.limit.try_acquire() else {
            warn!("peer-probe rejected: at capacity");
            conn.close(
                probe::AT_CAPACITY_CLOSE_CODE.into(),
                probe::AT_CAPACITY_CLOSE_REASON,
            );
            return Ok(());
        };

        // Surface the probe connection through the same conn_slot the paths
        // sampler reads, so the diagnostics view shows the connection state,
        // paths, and live RTT for an incoming `iroh-doctor connect` monitor.
        // If an outgoing monitor already owns the slot we respond silently
        // in the background.
        let peer_id = conn.remote_id().to_string();
        let peer_short_id: String = peer_id.chars().take(10).collect();
        let started = std::time::Instant::now();
        let claimed = {
            let mut slot = self.conn_slot.lock().await;
            if slot.is_none() {
                *slot = Some(conn.clone());
                self.events.state(ConnectionState::Connected {
                    peer_id: peer_id.clone(),
                    peer_short_id: peer_short_id.clone(),
                });
                true
            } else {
                false
            }
        };

        // When we own the slot, report time-to-first-direct-byte for this
        // incoming probe just like an outgoing dial does. Owned via
        // AbortOnDropHandle so it stops when this handler returns.
        let _ttfdb_task = claimed.then(|| {
            let conn = conn.clone();
            let events = self.events.clone();
            AbortOnDropHandle::new(tokio::spawn(async move {
                if let Some(elapsed) = crate::monitor::ttfdb_watch(&conn, started).await {
                    events.ttfdb(Some(elapsed));
                }
            }))
        });

        // Bounded channel: the responder emits one event per upload, which
        // clients pace by waiting for `UploadDone`, so 8 slots is more than
        // enough headroom for the drainer to keep up.
        let (tx, mut rx) = mpsc::channel::<probe::ProbeEvent>(8);
        let drain = {
            let events = self.events.clone();
            tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    match event {
                        probe::ProbeEvent::UploadCompleted { bytes, elapsed } => {
                            events.throughput(ThroughputSnapshot {
                                bytes,
                                elapsed,
                                mbps: probe::throughput_mbps(bytes, elapsed),
                            });
                        }
                    }
                }
            })
        };

        if let Err(e) = probe::handle_connection_with(conn, tx).await {
            warn!(err = %e, "peer-probe accept failed");
        }
        // Sender drops when handle_connection_with returns; the drainer's
        // `rx.recv()` then returns None and the task ends. Await it so we
        // don't leak a JoinHandle.
        let _ = drain.await;

        if claimed {
            let mut slot = self.conn_slot.lock().await;
            // Only clear the slot if it still holds our connection; an
            // outgoing monitor could have replaced it while we ran.
            if slot
                .as_ref()
                .is_some_and(|c| c.remote_id().to_string() == peer_id)
            {
                *slot = None;
            }
            self.events
                .state(ConnectionState::PeerDisconnected { peer_short_id });
        }
        Ok(())
    }
}
