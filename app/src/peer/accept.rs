//! The accept loop: dispatch each incoming connection by ALPN to the
//! gossip handler or the probe monitor.

use tokio::sync::mpsc;
use tokio_util::task::AbortOnDropHandle;
use tracing::warn;

use super::*;

pub(crate) async fn run_accept_loop(ctx: AcceptCtx) {
    loop {
        let Some(incoming) = ctx.endpoint.accept().await else {
            break;
        };
        let mut accepting = match incoming.accept() {
            Ok(a) => a,
            Err(_) => continue,
        };
        let alpn = match accepting.alpn().await {
            Ok(a) => a,
            Err(_) => continue,
        };
        let conn = match accepting.await {
            Ok(c) => c,
            Err(_) => continue,
        };

        let alpn_bytes: &[u8] = alpn.as_ref();
        if alpn_bytes == iroh_gossip::ALPN {
            let handler = ctx.gossip.clone();
            tokio::spawn(async move {
                if let Err(e) = handler.handle_connection(conn).await {
                    warn!(err = ?e, "iroh-gossip accept failed");
                }
            });
        } else if alpn_bytes == iroh_doctor_core::probe::ALPN {
            // Try to grab a probe-server permit. If we are at capacity
            // we drop the connection on the floor; the active side will
            // see a clean QUIC error and can retry.
            let limit = ctx.probe_limit.clone();
            let permit = match limit.try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    warn!("peer-probe rejected: at capacity");
                    drop(conn);
                    continue;
                }
            };
            // Surface the probe connection through the same conn_slot the
            // paths sampler reads, so the diagnostics view shows the
            // connection state, paths, and live RTT for an incoming
            // `iroh-doctor connect` monitor. If an outgoing monitor already
            // owns the slot we respond silently in the background.
            let conn_slot = ctx.conn_slot.clone();
            let on_state = ctx.on_state.clone();
            let on_throughput = ctx.on_throughput.clone();
            let on_ttfdb = ctx.on_ttfdb.clone();
            tokio::spawn(async move {
                let peer_id = conn.remote_id().to_string();
                let peer_short_id: String = peer_id.chars().take(10).collect();
                let started = std::time::Instant::now();
                let claimed = {
                    let mut slot = conn_slot.lock().await;
                    if slot.is_none() {
                        *slot = Some(conn.clone());
                        on_state(ConnectionState::Connected {
                            peer_id: peer_id.clone(),
                            peer_short_id: peer_short_id.clone(),
                        });
                        true
                    } else {
                        false
                    }
                };

                // When we own the slot, report time-to-first-direct-byte for
                // this incoming probe just like an outgoing dial does. Owned
                // via AbortOnDropHandle so it stops when this handler returns.
                let _ttfdb_task = claimed.then(|| {
                    let conn = conn.clone();
                    let on_ttfdb = on_ttfdb.clone();
                    AbortOnDropHandle::new(tokio::spawn(async move {
                        if let Some(elapsed) =
                            iroh_doctor_core::monitor::ttfdb_watch(&conn, started).await
                        {
                            on_ttfdb(Some(elapsed));
                        }
                    }))
                });

                // Bounded channel: the responder emits one event per
                // upload, which clients pace by waiting for `UploadDone`,
                // so 8 slots is more than enough headroom for the
                // drainer to keep up.
                let (tx, mut rx) = mpsc::channel::<iroh_doctor_core::probe::ProbeEvent>(8);
                let drain = tokio::spawn(async move {
                    while let Some(event) = rx.recv().await {
                        match event {
                            iroh_doctor_core::probe::ProbeEvent::UploadCompleted {
                                bytes,
                                elapsed,
                            } => {
                                on_throughput(ThroughputSnapshot {
                                    bytes,
                                    elapsed,
                                    mbps: iroh_doctor_core::probe::throughput_mbps(bytes, elapsed),
                                });
                            }
                        }
                    }
                });

                if let Err(e) = iroh_doctor_core::probe::handle_connection_with(conn, tx).await {
                    warn!(err = %e, "peer-probe accept failed");
                }
                // Sender drops here when handle_connection_with returns;
                // the drainer's `rx.recv()` then returns None and the
                // task ends. Await it so we don't leak a JoinHandle.
                let _ = drain.await;
                drop(permit);

                if claimed {
                    let mut slot = conn_slot.lock().await;
                    // Only clear the slot if it still holds our connection;
                    // an outgoing monitor could have replaced it while we ran.
                    if slot
                        .as_ref()
                        .is_some_and(|c| c.remote_id().to_string() == peer_id)
                    {
                        *slot = None;
                    }
                    on_state(ConnectionState::PeerDisconnected { peer_short_id });
                }
            });
        }
    }
}
