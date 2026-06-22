//! The passive side of the probe: a [`Server`] serves request streams on a
//! connection and gates how many connections run at once.
//!
//! [`Server::serve`] accepts request streams and fulfills each via
//! [`crate::probe::serve_request`], concurrently, so a liveness ping is
//! answered while a bulk transfer is still draining; concurrency per
//! connection is capped at [`MAX_STREAMS_PER_CONN`]. [`Server::try_admit`] and
//! [`Server::reject`] are the per-connection capacity gate: the probe ALPN has
//! no auth, so a caller admits a connection or closes it with the at-capacity
//! code.

use std::sync::Arc;

use anyhow::Result;
use iroh::endpoint::Connection;
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;
use tracing::warn;

use crate::probe::{self, ProbeEvent};

/// Default number of probe connections served in parallel.
const MAX_CONNS: usize = 4;

/// Maximum number of request streams served at once on one connection. The
/// probe ALPN is unauthenticated, so this bounds how much work and memory a
/// single peer can pin (each transfer stream can be up to
/// [`crate::probe::MAX_TRANSFER_BYTES`]).
const MAX_STREAMS_PER_CONN: usize = 16;

/// Capacity gate plus the request-stream responder for the probe protocol.
/// Cheap to clone (the gate is shared); clone it to hand a copy to each accept
/// task.
#[derive(Debug, Clone)]
pub struct Server {
    limit: Arc<Semaphore>,
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

impl Server {
    /// A server admitting [`MAX_CONNS`] connections at once.
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(MAX_CONNS)
    }

    /// A server admitting `max_conns` connections at once.
    #[must_use]
    pub fn with_capacity(max_conns: usize) -> Self {
        Self {
            limit: Arc::new(Semaphore::new(max_conns)),
        }
    }

    /// Tries to admit one connection, returning a permit to hold for its
    /// lifetime, or `None` when already at capacity (the caller should
    /// [`reject`](Self::reject) it).
    #[must_use]
    pub fn try_admit(&self) -> Option<OwnedSemaphorePermit> {
        self.limit.clone().try_acquire_owned().ok()
    }

    /// Closes `conn` with the at-capacity code and reason, so the dialer sees
    /// why it was turned away rather than an anonymous connection loss.
    pub fn reject(&self, conn: &Connection) {
        conn.close(
            probe::AT_CAPACITY_CLOSE_CODE.into(),
            probe::AT_CAPACITY_CLOSE_REASON,
        );
    }

    /// Serves every request stream the client opens on `conn`, each
    /// concurrently, until the connection closes (iroh's idle timeout reaps a
    /// vanished peer). Pass an `events` channel to observe [`ProbeEvent`]s as
    /// they happen (the app surfaces them as throughput readouts); `None`
    /// serves silently.
    ///
    /// The serve tasks are owned by a `JoinSet` dropped when this returns, so
    /// they are aborted the moment the connection closes rather than lingering.
    pub async fn serve(conn: Connection, events: Option<mpsc::Sender<ProbeEvent>>) -> Result<()> {
        let limit = Arc::new(Semaphore::new(MAX_STREAMS_PER_CONN));
        let mut streams = JoinSet::new();
        loop {
            // Reap finished serve tasks so the set does not grow without bound.
            while streams.try_join_next().is_some() {}
            let (mut send, mut recv) = match conn.accept_bi().await {
                Ok(streams) => streams,
                // The connection closed; no more requests will arrive.
                Err(_) => break,
            };
            // Block for a slot before serving the next stream, bounding how
            // many an unauthenticated peer can pin at once.
            let Ok(permit) = limit.clone().acquire_owned().await else {
                break;
            };
            let events = events.clone();
            streams.spawn(async move {
                let _permit = permit;
                if let Err(e) = probe::serve_request(&mut send, &mut recv, events.as_ref()).await {
                    warn!(err = %e, "probe: serving request failed");
                }
                // Closing our send side acks an upload and ends a ping/download.
                let _ = send.finish();
            });
        }
        Ok(())
    }
}
