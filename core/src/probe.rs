//! Peer probe protocol (`iroh-pong-probe/0`): latency over time and upload
//! throughput against a peer.
//!
//! One bidi stream carries the whole exchange. The active side (the
//! [`ProbeClient`]) sends `Ping`s and times the matching `Pong`s, and may
//! send an `UploadStart` followed by raw bytes to measure upload
//! throughput. The passive side ([`handle_connection`]) echoes pings and
//! drains uploads.
//!
//! The responder is built for continuous monitoring: it echoes pings
//! without a count cap (a `Pong` is cheap) and keeps serving after an
//! upload, so a client can ping for as long as it likes and interleave the
//! occasional throughput measurement. A per-frame idle timeout bounds a
//! stalled or vanished client, and the upload size is capped so a peer
//! cannot make the responder drain an unbounded stream.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use iroh::endpoint;
use postcard::experimental::max_size::MaxSize;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tracing::warn;

/// ALPN for the peer probe. Must stay byte-stable for interop with deployed
/// peers (the app registers this same ALPN).
//
// TODO(version): the trailing `/0` is the wire-format version. Any change
// to `Frame` (variants, fields, encoding) must bump to `/1` and accept both
// `/0` and `/1` on the passive side for one release so a mixed-version
// deployment does not hard-fail.
pub const ALPN: &[u8] = b"iroh-pong-probe/0";

/// How long the responder waits for the next frame before treating the
/// client as gone. Comfortably longer than a client's ping interval.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the client waits for a `Pong` after sending a `Ping`.
const PING_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the client waits for `UploadDone` after sending the payload.
/// Generous because the peer is still draining the upload.
const UPLOAD_DONE_TIMEOUT: Duration = Duration::from_secs(30);

/// Upper bound on `UploadStart::bytes` the responder will accept. Anything
/// larger fails immediately so a peer cannot make us drain forever.
const MAX_UPLOAD_BYTES: u64 = 16 * 1024 * 1024;

/// Write granularity for the upload payload.
const UPLOAD_CHUNK: usize = 32 * 1024;

/// Wire message: ping/pong rounds interleaved with single uploads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, MaxSize)]
pub enum Frame {
    /// Active side sends a ping carrying a nonce.
    Ping(u32),
    /// Passive side echoes the ping's nonce.
    Pong(u32),
    /// Active side announces it is about to upload `bytes` raw bytes.
    UploadStart {
        /// Number of bytes that follow. Bounded by [`MAX_UPLOAD_BYTES`].
        bytes: u64,
    },
    /// Passive side acknowledges it received the full upload.
    UploadDone,
}

/// Megabits per second for `bytes` transferred in `elapsed`, or `None` when
/// `elapsed` is zero.
pub fn throughput_mbps(bytes: u64, elapsed: Duration) -> Option<f64> {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return None;
    }
    Some((bytes as f64 * 8.0) / secs / 1_000_000.0)
}

/// Serves the passive side of one probe stream until the client closes it
/// or goes idle for [`IDLE_TIMEOUT`].
pub async fn handle_connection(conn: endpoint::Connection) -> Result<()> {
    let (send, recv) = tokio::time::timeout(Duration::from_secs(5), conn.accept_bi())
        .await
        .context("accept probe bidi stream: timeout")?
        .context("accept probe bidi stream")?;
    serve_stream(send, recv).await
}

async fn serve_stream<S, R>(mut send: S, mut recv: R) -> Result<()>
where
    S: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    loop {
        let frame = match read_frame(&mut recv, IDLE_TIMEOUT).await {
            Ok(f) => f,
            // Idle, closed, or malformed: the client is done with us.
            Err(_) => break,
        };
        match frame {
            Frame::Ping(n) => write_frame(&mut send, &Frame::Pong(n)).await?,
            Frame::UploadStart { bytes } => {
                if bytes > MAX_UPLOAD_BYTES {
                    warn!(bytes, "probe: upload too large, refusing");
                    break;
                }
                let mut remaining = bytes;
                let mut buf = vec![0u8; UPLOAD_CHUNK];
                while remaining > 0 {
                    let take = remaining.min(buf.len() as u64) as usize;
                    recv.read_exact(&mut buf[..take])
                        .await
                        .context("upload drain")?;
                    remaining -= take as u64;
                }
                write_frame(&mut send, &Frame::UploadDone).await?;
            }
            Frame::Pong(_) | Frame::UploadDone => {
                warn!(?frame, "probe: unexpected client frame");
                break;
            }
        }
    }
    let _ = send.shutdown().await;
    Ok(())
}

/// Active side of the probe over a single bidi stream.
pub struct ProbeClient {
    send: endpoint::SendStream,
    recv: endpoint::RecvStream,
}

impl ProbeClient {
    /// Opens a probe stream on an existing connection.
    pub async fn connect(conn: &endpoint::Connection) -> Result<Self> {
        let (send, recv) = conn.open_bi().await.context("open probe bidi stream")?;
        Ok(Self { send, recv })
    }

    /// Sends one ping and returns the round-trip time.
    pub async fn ping(&mut self, nonce: u32) -> Result<Duration> {
        ping_once(&mut self.send, &mut self.recv, nonce).await
    }

    /// Uploads `bytes` bytes and returns how long it took the peer to
    /// acknowledge them. Pair with [`throughput_mbps`].
    pub async fn upload(&mut self, bytes: u64) -> Result<Duration> {
        upload_once(&mut self.send, &mut self.recv, bytes).await
    }
}

async fn ping_once<S, R>(send: &mut S, recv: &mut R, nonce: u32) -> Result<Duration>
where
    S: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    let started = Instant::now();
    write_frame(send, &Frame::Ping(nonce)).await?;
    match read_frame(recv, PING_TIMEOUT).await? {
        Frame::Pong(n) if n == nonce => Ok(started.elapsed()),
        other => anyhow::bail!("unexpected reply to ping {nonce}: {other:?}"),
    }
}

async fn upload_once<S, R>(send: &mut S, recv: &mut R, bytes: u64) -> Result<Duration>
where
    S: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    write_frame(send, &Frame::UploadStart { bytes }).await?;
    let payload = vec![0u8; UPLOAD_CHUNK];
    let mut written = 0u64;
    let started = Instant::now();
    while written < bytes {
        let take = (bytes - written).min(payload.len() as u64) as usize;
        send.write_all(&payload[..take])
            .await
            .context("upload write")?;
        written += take as u64;
    }
    match read_frame(recv, UPLOAD_DONE_TIMEOUT).await? {
        Frame::UploadDone => Ok(started.elapsed()),
        other => anyhow::bail!("unexpected reply to upload: {other:?}"),
    }
}

async fn write_frame<W: AsyncWrite + Unpin>(send: &mut W, frame: &Frame) -> Result<()> {
    let mut buf = [0u8; Frame::POSTCARD_MAX_SIZE];
    let encoded = postcard::to_slice(frame, &mut buf).context("encode frame")?;
    let len = encoded.len() as u16;
    send.write_all(&len.to_le_bytes()).await?;
    send.write_all(encoded).await?;
    Ok(())
}

// Not cancel-safe: a timed-out length read may leave the stream mid-frame.
// Every caller drops the stream on timeout, so the desync never matters.
async fn read_frame<R: AsyncRead + Unpin>(recv: &mut R, budget: Duration) -> Result<Frame> {
    let mut len_buf = [0u8; 2];
    tokio::time::timeout(budget, recv.read_exact(&mut len_buf))
        .await
        .context("read frame length: timeout")?
        .context("read frame length")?;
    let len = u16::from_le_bytes(len_buf) as usize;
    if len > Frame::POSTCARD_MAX_SIZE {
        anyhow::bail!("frame too large: {len}");
    }
    let mut buf = vec![0u8; len];
    tokio::time::timeout(budget, recv.read_exact(&mut buf))
        .await
        .context("read frame body: timeout")?
        .context("read frame body")?;
    postcard::from_bytes(&buf).context("decode frame")
}

#[cfg(test)]
mod tests {
    use tokio::io::duplex;

    use super::*;

    /// Round-trip every Frame variant through postcard so a future
    /// serialization-shape change fails loudly here.
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

    #[test]
    fn max_upload_bytes_is_bounded() {
        const _: () = assert!(MAX_UPLOAD_BYTES >= 1024 * 1024);
        const _: () = assert!(MAX_UPLOAD_BYTES <= 256 * 1024 * 1024);
    }

    #[test]
    fn throughput_mbps_is_none_on_zero_time() {
        assert!(throughput_mbps(1024, Duration::ZERO).is_none());
        // 1 MiB in 100ms = ~83.9 Mbps.
        let mbps = throughput_mbps(1024 * 1024, Duration::from_millis(100)).unwrap();
        assert!((80.0..90.0).contains(&mbps), "got {mbps}");
    }

    /// Drive the real client functions against the real responder over an
    /// in-memory duplex pair, exercising ping and upload without QUIC.
    #[tokio::test]
    async fn ping_and_upload_roundtrip_over_duplex() {
        let (mut client_send, server_recv) = duplex(64 * 1024);
        let (server_send, mut client_recv) = duplex(64 * 1024);
        let server = tokio::spawn(serve_stream(server_send, server_recv));

        let rtt = ping_once(&mut client_send, &mut client_recv, 7)
            .await
            .expect("ping");
        assert!(rtt < Duration::from_secs(1));

        let elapsed = upload_once(&mut client_send, &mut client_recv, 256 * 1024)
            .await
            .expect("upload");
        assert!(throughput_mbps(256 * 1024, elapsed).unwrap() > 0.0);

        // Closing the client's write half ends the responder loop cleanly.
        client_send.shutdown().await.unwrap();
        server.await.unwrap().expect("responder finished cleanly");
    }

    #[tokio::test]
    async fn responder_refuses_oversized_upload() {
        let (mut client_send, server_recv) = duplex(64 * 1024);
        let (server_send, mut client_recv) = duplex(64 * 1024);
        let server = tokio::spawn(serve_stream(server_send, server_recv));

        // Announce more than the responder will accept: it closes without
        // acking, so the client's wait for UploadDone fails.
        write_frame(
            &mut client_send,
            &Frame::UploadStart {
                bytes: MAX_UPLOAD_BYTES + 1,
            },
        )
        .await
        .unwrap();
        assert!(read_frame(&mut client_recv, Duration::from_secs(1))
            .await
            .is_err());
        server.await.unwrap().expect("responder finished cleanly");
    }
}
