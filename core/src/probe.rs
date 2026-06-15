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
//! occasional throughput measurement. There are no protocol-level timeouts:
//! a vanished peer surfaces as a stream error once iroh's connection idle
//! timeout fires (iroh's default transport config keeps paths alive with
//! heartbeats and idles dead ones out), and a quiet-but-alive peer is left
//! alone. The upload size is capped so a peer cannot make the responder
//! drain an unbounded stream.

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

/// Application close code a responder sends when it rejects a probe
/// connection because it is already serving its maximum number of peers.
/// Clients see it in the connection error and can surface the reason.
pub const AT_CAPACITY_CLOSE_CODE: u32 = 1;

/// Close reason paired with [`AT_CAPACITY_CLOSE_CODE`].
pub const AT_CAPACITY_CLOSE_REASON: &[u8] = b"probe responder at capacity";

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

/// Observable events emitted by the passive side as it serves a probe
/// stream. Use [`handle_connection_with`] to receive these from the
/// responder; the app side surfaces them as throughput readouts.
#[derive(Debug, Clone)]
pub enum ProbeEvent {
    /// The responder drained an upload of `bytes` bytes in `elapsed` and
    /// acknowledged it.
    UploadCompleted { bytes: u64, elapsed: Duration },
}

/// Megabits per second for `bytes` transferred in `elapsed`, or `None` when
/// `elapsed` is zero.
#[must_use]
pub fn throughput_mbps(bytes: u64, elapsed: Duration) -> Option<f64> {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return None;
    }
    Some((bytes as f64 * 8.0) / secs / 1_000_000.0)
}

/// Pacing for the active probe client loop driven by [`run_client`].
#[derive(Debug, Clone, Copy)]
pub struct ClientConfig {
    /// Delay between successive pings.
    pub ping_interval: Duration,
    /// Bytes to upload on each throughput sample. Bounded by
    /// [`MAX_UPLOAD_BYTES`]; a larger value fails the upload immediately.
    pub upload_bytes: u64,
    /// Upload once every this many pings, counting the first (nonce 0). Zero
    /// disables throughput sampling and runs latency only.
    pub upload_every: u32,
}

impl Default for ClientConfig {
    /// One ping per second, a 1 MiB upload every tenth ping. These are the
    /// values the cli and app monitors used before [`run_client`] existed.
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(1),
            upload_bytes: 1024 * 1024,
            upload_every: 10,
        }
    }
}

/// A single measurement emitted by [`run_client`].
#[derive(Debug, Clone)]
pub enum ClientSample {
    /// Round-trip time observed for ping `nonce`.
    Latency { nonce: u32, rtt: Duration },
    /// A completed upload of `bytes` bytes that the peer acknowledged in
    /// `elapsed`. Pair with [`throughput_mbps`].
    Throughput { bytes: u64, elapsed: Duration },
}

/// Why the active probe client loop stopped.
#[derive(Debug, Clone)]
pub struct ClientEnd {
    /// Which phase ended the loop: `"setup"`, `"latency"`, `"throughput"`,
    /// or `"closed"` when the sample consumer went away.
    pub phase: &'static str,
    /// Human-readable cause, suitable for surfacing in a status line.
    pub cause: String,
}

/// Runs the active side of the probe against `conn`, emitting one
/// [`ClientSample`] on `samples` per ping and per upload until a ping or
/// upload fails, the peer goes away, or the sample consumer is dropped.
///
/// This is the shared monitor loop behind both `iroh-doctor connect` and the
/// app's Connect action, so both report latency and throughput the same way.
/// Latency and path state are also observable independently via
/// [`crate::monitor`]; callers that render a graph from QUIC's smoothed RTT
/// can ignore [`ClientSample::Latency`] and use these samples only to drive
/// probe traffic and surface throughput.
///
/// Returns a [`ClientEnd`] describing why the loop stopped. It does not error:
/// a dead peer is the normal end of a monitor session, not a failure.
pub async fn run_client(
    conn: &endpoint::Connection,
    config: ClientConfig,
    samples: tokio::sync::mpsc::Sender<ClientSample>,
) -> ClientEnd {
    let (mut send, mut recv) = match conn.open_bi().await {
        Ok(streams) => streams,
        Err(cause) => {
            return ClientEnd {
                phase: "setup",
                cause: format!("open probe bidi stream: {cause:#}"),
            }
        }
    };
    drive_client(&mut send, &mut recv, config, &samples).await
}

/// The [`run_client`] loop, generic over the stream types so it can run over
/// an in-memory duplex pair in tests without a QUIC connection.
async fn drive_client<S, R>(
    send: &mut S,
    recv: &mut R,
    config: ClientConfig,
    samples: &tokio::sync::mpsc::Sender<ClientSample>,
) -> ClientEnd
where
    S: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    let mut nonce: u32 = 0;
    loop {
        match ping_once(send, recv, nonce).await {
            Ok(rtt) => {
                if samples
                    .send(ClientSample::Latency { nonce, rtt })
                    .await
                    .is_err()
                {
                    return consumer_gone();
                }
            }
            Err(cause) => {
                return ClientEnd {
                    phase: "latency",
                    cause: format!("{cause:#}"),
                }
            }
        }

        if config.upload_every != 0 && nonce.is_multiple_of(config.upload_every) {
            match upload_once(send, recv, config.upload_bytes).await {
                Ok(elapsed) => {
                    if samples
                        .send(ClientSample::Throughput {
                            bytes: config.upload_bytes,
                            elapsed,
                        })
                        .await
                        .is_err()
                    {
                        return consumer_gone();
                    }
                }
                Err(cause) => {
                    return ClientEnd {
                        phase: "throughput",
                        cause: format!("{cause:#}"),
                    }
                }
            }
        }

        nonce = nonce.wrapping_add(1);
        tokio::time::sleep(config.ping_interval).await;
    }
}

/// The loop's end when the [`ClientSample`] consumer is dropped, which the
/// app does by aborting the monitor task on a fresh dial.
fn consumer_gone() -> ClientEnd {
    ClientEnd {
        phase: "closed",
        cause: "monitor consumer dropped".to_string(),
    }
}

/// Serves the passive side of one probe stream until the client closes it
/// or the connection ends (iroh's idle timeout reaps a vanished peer).
/// Pass an `events` channel to observe [`ProbeEvent`]s as they happen (the
/// app surfaces them as throughput readouts); `None` serves silently.
pub async fn handle_connection(
    conn: endpoint::Connection,
    events: Option<tokio::sync::mpsc::Sender<ProbeEvent>>,
) -> Result<()> {
    let (send, recv) = conn.accept_bi().await.context("accept probe bidi stream")?;
    serve_stream(send, recv, events).await
}

async fn serve_stream<S, R>(
    mut send: S,
    mut recv: R,
    events: Option<tokio::sync::mpsc::Sender<ProbeEvent>>,
) -> Result<()>
where
    S: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    loop {
        let frame = match read_frame(&mut recv).await {
            Ok(f) => f,
            // Closed, connection gone, or malformed: the client is done
            // with us.
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
                let drain_started = Instant::now();
                while remaining > 0 {
                    let take = remaining.min(buf.len() as u64) as usize;
                    recv.read_exact(&mut buf[..take])
                        .await
                        .context("upload drain")?;
                    remaining -= take as u64;
                }
                let elapsed = drain_started.elapsed();
                write_frame(&mut send, &Frame::UploadDone).await?;
                if let Some(events) = events.as_ref() {
                    // Best-effort; drop the event if the consumer is gone.
                    let _ = events.try_send(ProbeEvent::UploadCompleted { bytes, elapsed });
                }
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

async fn ping_once<S, R>(send: &mut S, recv: &mut R, nonce: u32) -> Result<Duration>
where
    S: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    let started = Instant::now();
    write_frame(send, &Frame::Ping(nonce)).await?;
    match read_frame(recv).await? {
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
    match read_frame(recv).await? {
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

// Not cancel-safe: cancelling mid-read leaves the stream mid-frame. No
// caller resumes a cancelled read; the stream is dropped instead.
async fn read_frame<R: AsyncRead + Unpin>(recv: &mut R) -> Result<Frame> {
    let mut len_buf = [0u8; 2];
    recv.read_exact(&mut len_buf)
        .await
        .context("read frame length")?;
    let len = u16::from_le_bytes(len_buf) as usize;
    if len > Frame::POSTCARD_MAX_SIZE {
        anyhow::bail!("frame too large: {len}");
    }
    let mut buf = vec![0u8; len];
    recv.read_exact(&mut buf).await.context("read frame body")?;
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
        let server = tokio::spawn(serve_stream(server_send, server_recv, None));

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
        let server = tokio::spawn(serve_stream(server_send, server_recv, None));

        // Announce more than the responder will accept: it closes without
        // acking, so the client's wait for UploadDone fails. The timeout is
        // test-local insurance against a hang, not protocol behavior.
        write_frame(
            &mut client_send,
            &Frame::UploadStart {
                bytes: MAX_UPLOAD_BYTES + 1,
            },
        )
        .await
        .unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut client_recv))
            .await
            .expect("responder must close the stream rather than stall");
        assert!(reply.is_err());
        server.await.unwrap().expect("responder finished cleanly");
    }

    /// The responder must serve a long ping stream (the old one-shot
    /// version capped pings and closed after a single upload) and keep
    /// answering pings after an upload.
    #[tokio::test]
    async fn responder_serves_continuously() {
        let (mut client_send, server_recv) = duplex(128 * 1024);
        let (server_send, mut client_recv) = duplex(128 * 1024);
        let server = tokio::spawn(serve_stream(server_send, server_recv, None));

        // Well past the old per-stream ping cap (which was 10).
        for nonce in 0..15u32 {
            ping_once(&mut client_send, &mut client_recv, nonce)
                .await
                .expect("ping");
        }
        upload_once(&mut client_send, &mut client_recv, 64 * 1024)
            .await
            .expect("upload");
        // Still answering pings after the upload.
        ping_once(&mut client_send, &mut client_recv, 99)
            .await
            .expect("post-upload ping");

        client_send.shutdown().await.unwrap();
        server.await.unwrap().expect("responder finished cleanly");
    }

    /// The shared monitor loop must emit both a latency and a throughput
    /// sample against the real responder, and stop cleanly once the sample
    /// consumer is dropped (how the app cancels a monitor on a fresh dial).
    #[tokio::test]
    async fn run_client_loop_emits_latency_and_throughput() {
        let (mut client_send, server_recv) = duplex(256 * 1024);
        let (server_send, mut client_recv) = duplex(256 * 1024);
        let server = tokio::spawn(serve_stream(server_send, server_recv, None));

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        // Tiny interval and an upload on every other ping so the test sees
        // both sample kinds quickly without real time passing.
        let config = ClientConfig {
            ping_interval: Duration::from_millis(1),
            upload_bytes: 64 * 1024,
            upload_every: 2,
        };
        let driver = tokio::spawn(async move {
            drive_client(&mut client_send, &mut client_recv, config, &tx).await
        });

        let mut latencies = 0;
        let mut throughputs = 0;
        for _ in 0..6 {
            match rx.recv().await.expect("sample") {
                ClientSample::Latency { .. } => latencies += 1,
                ClientSample::Throughput { bytes, elapsed } => {
                    assert_eq!(bytes, 64 * 1024);
                    assert!(throughput_mbps(bytes, elapsed).unwrap() > 0.0);
                    throughputs += 1;
                }
            }
        }
        assert!(latencies >= 1, "expected at least one latency sample");
        assert!(throughputs >= 1, "expected at least one throughput sample");

        // Dropping the consumer ends the loop with the "closed" phase, and
        // dropping the client streams (owned by the task) ends the responder.
        drop(rx);
        let end = driver.await.unwrap();
        assert_eq!(end.phase, "closed");
        server.await.unwrap().expect("responder finished cleanly");
    }
}
