//! Passive responder for the direct peer probe.
//!
//! The active/client side of this probe lived on the Diagnostics tab and
//! has been removed; this module now only keeps the responder so peers
//! running other iroh-pong builds can still measure latency and a small
//! upload against this endpoint. Both peers register the [`ALPN`]; the
//! accept loop in `peer.rs` dispatches to [`handle_connection`] when a
//! probe arrives. One bidi stream: echo pings, then drain one upload.

use std::time::Duration;

use anyhow::{Context, Result};
use iroh::endpoint;
use postcard::experimental::max_size::MaxSize;
use serde::{Deserialize, Serialize};
use tracing::warn;

/// Dedicated ALPN for the iroh-pong peer probe. Distinct from the pong
/// game ALPN (`iroh-helloiroh-pong/0`); only peers that also register
/// this exact ALPN respond.
//
// TODO(version): the trailing `/0` is the wire-format version. Any
// change to `Frame` (variants, fields, encoding) must bump to `/1`
// and accept both `/0` and `/1` on the passive side for one release
// so a mixed-version deployment does not hard-fail.
pub const ALPN: &[u8] = b"iroh-pong-probe/0";

/// Per-frame read budget. Each `read_frame` call may block up to this
/// long.
const FRAME_READ_TIMEOUT: Duration = Duration::from_secs(3);

/// Ceiling for the passive side's whole probe handler so a peer that
/// completes the QUIC handshake on this ALPN and then misbehaves cannot
/// pin a task forever.
const SERVER_PROBE_TIMEOUT: Duration = Duration::from_secs(35);

/// Expected number of latency round-trips a probing peer issues. The
/// responder treats more than twice this many pings as a flood and
/// closes the connection.
const PING_ITERATIONS: u32 = 5;

/// Upper bound on `UploadStart::bytes` that the passive side will
/// accept from a peer. Anything larger fails the probe immediately so
/// a misbehaving (or malicious) client cannot trickle bytes into a
/// long-running drain.
const MAX_UPLOAD_BYTES: u64 = 16 * 1024 * 1024;

/// Wire message: a ping/pong sequence followed by a single upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, MaxSize)]
pub enum Frame {
    /// Active side sends a ping carrying a sequence number.
    Ping(u32),
    /// Passive side echoes the ping's sequence number.
    Pong(u32),
    /// Active side announces it is about to upload `bytes` raw bytes.
    /// Passive side reads exactly `bytes` then sends `UploadDone`.
    UploadStart {
        /// Number of bytes the active side will write. Bounded by
        /// [`MAX_UPLOAD_BYTES`] on the passive side.
        bytes: u64,
    },
    /// Passive side acknowledges it received the full upload.
    UploadDone,
}

/// Server side of the probe. Accepts a single bidi stream, echoes pings,
/// and drains the upload.
///
/// The whole handler is bounded by [`SERVER_PROBE_TIMEOUT`]; a peer that
/// completes the QUIC handshake on this ALPN and then misbehaves cannot
/// pin a task longer than that. Concurrency is capped at the dispatcher
/// in `peer.rs` via a semaphore.
pub async fn handle_connection(conn: endpoint::Connection) -> Result<()> {
    tokio::time::timeout(SERVER_PROBE_TIMEOUT, handle_connection_inner(conn))
        .await
        .context("probe handler exceeded server timeout")?
}

async fn handle_connection_inner(conn: endpoint::Connection) -> Result<()> {
    let (mut send, mut recv) = tokio::time::timeout(Duration::from_secs(5), conn.accept_bi())
        .await
        .context("accept probe bidi stream: timeout")?
        .context("accept probe bidi stream")?;
    let mut pings_seen: u32 = 0;
    let max_pings = PING_ITERATIONS * 2;
    loop {
        let frame = match read_frame(&mut recv, FRAME_READ_TIMEOUT).await {
            Ok(f) => f,
            Err(e) => {
                warn!(err = %e, "probe server: read frame");
                break;
            }
        };
        match frame {
            Frame::Ping(n) => {
                pings_seen += 1;
                if pings_seen > max_pings {
                    warn!("probe server: ping flood, closing");
                    break;
                }
                write_frame(&mut send, &Frame::Pong(n)).await?;
            }
            Frame::UploadStart { bytes } => {
                if bytes > MAX_UPLOAD_BYTES {
                    warn!(bytes, "probe server: upload too large, refusing");
                    break;
                }
                let mut remaining = bytes;
                let mut buf = vec![0u8; 32 * 1024];
                while remaining > 0 {
                    let take = remaining.min(buf.len() as u64) as usize;
                    recv.read_exact(&mut buf[..take])
                        .await
                        .context("upload drain")?;
                    remaining -= take as u64;
                }
                write_frame(&mut send, &Frame::UploadDone).await?;
                break;
            }
            Frame::Pong(_) | Frame::UploadDone => {
                warn!(?frame, "probe server: unexpected frame");
                break;
            }
        }
    }
    send.finish().ok();
    Ok(())
}

async fn write_frame(send: &mut endpoint::SendStream, frame: &Frame) -> Result<()> {
    let mut buf = [0u8; Frame::POSTCARD_MAX_SIZE];
    let encoded = postcard::to_slice(frame, &mut buf).context("encode frame")?;
    let len = encoded.len() as u16;
    send.write_all(&len.to_le_bytes()).await?;
    send.write_all(encoded).await?;
    Ok(())
}

// This read is not cancel-safe: a timed-out length read may leave the
// stream mid-frame. Every caller drops the stream on timeout, so the
// desync never matters in practice.
async fn read_frame(recv: &mut endpoint::RecvStream, budget: Duration) -> Result<Frame> {
    let mut len_buf = [0u8; 2];
    let read = tokio::time::timeout(budget, recv.read_exact(&mut len_buf))
        .await
        .context("read frame length: timeout")?;
    read.context("read frame length")?;
    let len = u16::from_le_bytes(len_buf) as usize;
    if len > Frame::POSTCARD_MAX_SIZE {
        anyhow::bail!("frame too large: {len}");
    }
    let mut buf = vec![0u8; len];
    let read = tokio::time::timeout(budget, recv.read_exact(&mut buf))
        .await
        .context("read frame body: timeout")?;
    read.context("read frame body")?;
    let frame: Frame = postcard::from_bytes(&buf).context("decode frame")?;
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip every Frame variant through postcard so a future
    /// serialization-shape change (or a reorder of the enum) fails
    /// loudly here.
    #[test]
    fn frame_postcard_roundtrip() {
        let cases = [
            Frame::Ping(0),
            Frame::Ping(u32::MAX),
            Frame::Pong(42),
            Frame::UploadStart { bytes: 0 },
            Frame::UploadStart {
                bytes: MAX_UPLOAD_BYTES,
            },
            Frame::UploadDone,
        ];
        let mut buf = [0u8; Frame::POSTCARD_MAX_SIZE];
        for frame in cases {
            let encoded = postcard::to_slice(&frame, &mut buf).expect("encode");
            let decoded: Frame = postcard::from_bytes(encoded).expect("decode");
            assert_eq!(frame, decoded, "frame did not survive postcard roundtrip");
        }
    }

    /// The MAX_UPLOAD_BYTES ceiling is the only safeguard preventing a
    /// malicious peer from trickling an unbounded upload through the
    /// passive side. Pin the constant so a careless edit cannot relax
    /// it without flagging.
    #[test]
    fn max_upload_bytes_is_bounded() {
        // At least the 1 MiB a probing peer is expected to upload, and
        // well under a runaway ceiling.
        const _: () = assert!(MAX_UPLOAD_BYTES >= 1024 * 1024);
        const _: () = assert!(MAX_UPLOAD_BYTES <= 256 * 1024 * 1024);
    }
}
