//! Peer probe protocol (`iroh-doctor/probe/1`): latency over time and
//! throughput against a peer.
//!
//! One bidi stream *per request*. A request is one of [`ping`], [`upload`], or
//! [`download`]; the client opens a fresh stream, writes a `u32` length prefix
//! and a postcard-encoded [`Request`] header, then for a transfer streams raw
//! bytes. Pings ride a high-priority stream so a liveness ping still completes
//! promptly while a bulk transfer saturates the path - which only works because
//! the responder serves every stream on a connection *concurrently* (see
//! [`handle_connection`]).
//!
//! There are no protocol-level timeouts: a vanished peer surfaces as a stream
//! error once iroh's connection idle timeout fires (iroh's default transport
//! config keeps paths alive with heartbeats and idles dead ones out), and a
//! quiet-but-alive peer is left alone. Transfer sizes are capped so a peer
//! cannot make the responder read or write an unbounded stream.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use iroh::endpoint;
use postcard::experimental::max_size::MaxSize;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinSet;
use tracing::warn;

/// ALPN for the peer probe. `/1` is the request-per-stream wire format that
/// replaced the original frame-based `/0`; the two are not interoperable, so a
/// wire-format change must bump this and the responder must accept both for one
/// release to avoid hard-failing a mixed-version deployment.
pub const ALPN: &[u8] = b"iroh-doctor/probe/1";

/// Application close code a responder sends when it rejects a probe
/// connection because it is already serving its maximum number of peers.
/// Clients see it in the connection error and can surface the reason.
pub const AT_CAPACITY_CLOSE_CODE: u32 = 1;

/// Close reason paired with [`AT_CAPACITY_CLOSE_CODE`].
pub const AT_CAPACITY_CLOSE_REASON: &[u8] = b"probe responder at capacity";

/// Upper bound on a single transfer the responder will read or write. Anything
/// larger fails immediately so a peer cannot make us stream forever.
pub const MAX_TRANSFER_BYTES: u64 = 256 * 1024 * 1024;

/// Streaming chunk size for transfer payloads.
const CHUNK: usize = 64 * 1024;

/// Stream priority for a ping, above the default (0) so a liveness ping is
/// scheduled ahead of bulk transfer data on the same connection.
const PING_PRIORITY: i32 = 1;

/// Maximum number of request streams served at once on one connection. The
/// probe ALPN is unauthenticated, so this bounds how much work and memory a
/// single peer can pin (each transfer stream can be up to
/// [`MAX_TRANSFER_BYTES`]).
const MAX_STREAMS_PER_CONN: usize = 16;

/// One probe request, sent as the postcard header of a fresh bidi stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, MaxSize)]
enum Request {
    /// Liveness check: the responder replies with a single byte.
    Ping,
    /// The client streams `bytes` to the responder, which drains and acks them
    /// (by closing its send side). Bounded by [`MAX_TRANSFER_BYTES`].
    Upload { bytes: u64 },
    /// The responder streams `bytes` to the client. Bounded by
    /// [`MAX_TRANSFER_BYTES`].
    Download { bytes: u64 },
}

/// Observable events emitted by the passive side as it serves a probe
/// connection. Use the `events` channel of [`handle_connection`] to receive
/// these; the app side surfaces them as throughput readouts.
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
    /// [`MAX_TRANSFER_BYTES`]; a larger value fails the upload immediately.
    pub upload_bytes: u64,
    /// Upload once every this many pings, counting the first (nonce 0). Zero
    /// disables throughput sampling and runs latency only.
    pub upload_every: u32,
}

impl Default for ClientConfig {
    /// One ping per second, a 1 MiB upload every tenth ping.
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
/// Each ping and each upload opens its own bidi stream; pings use a
/// high-priority stream. This is the shared monitor loop behind both
/// `iroh-doctor connect` and the app's Connect action.
///
/// Returns a [`ClientEnd`] describing why the loop stopped. It does not error:
/// a dead peer is the normal end of a monitor session, not a failure.
pub async fn run_client(
    conn: &endpoint::Connection,
    config: ClientConfig,
    samples: mpsc::Sender<ClientSample>,
) -> ClientEnd {
    let mut nonce: u32 = 0;
    loop {
        match ping(conn).await {
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
            match upload(conn, config.upload_bytes).await {
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

// --- Client side: each request opens its own bidi stream. ---

/// Sends one ping over a fresh high-priority stream and returns the round-trip
/// time. The timer starts after the stream is open, so it reflects the round
/// trip rather than stream setup.
pub async fn ping(conn: &endpoint::Connection) -> Result<Duration> {
    let (mut send, mut recv) = conn.open_bi().await.context("open ping stream")?;
    let _ = send.set_priority(PING_PRIORITY);
    let started = Instant::now();
    write_request(&mut send, Request::Ping).await?;
    send.finish().context("finish ping stream")?;
    let mut pong = [0u8; 1];
    recv.read_exact(&mut pong).await.context("read pong")?;
    Ok(started.elapsed())
}

/// Uploads `bytes` to the peer and returns the elapsed time, measured until
/// the responder has drained the whole stream and closed its side.
pub async fn upload(conn: &endpoint::Connection, bytes: u64) -> Result<Duration> {
    let (mut send, mut recv) = conn.open_bi().await.context("open upload stream")?;
    let started = Instant::now();
    write_request(&mut send, Request::Upload { bytes }).await?;
    write_payload(&mut send, bytes).await?;
    send.finish().context("finish upload stream")?;
    // The responder closes its send side once it has drained the upload.
    recv.read_to_end(0).await.context("await upload ack")?;
    Ok(started.elapsed())
}

/// Downloads `bytes` from the peer and returns the elapsed time.
pub async fn download(conn: &endpoint::Connection, bytes: u64) -> Result<Duration> {
    let (mut send, mut recv) = conn.open_bi().await.context("open download stream")?;
    let started = Instant::now();
    write_request(&mut send, Request::Download { bytes }).await?;
    send.finish().context("finish download request")?;
    let got = drain(&mut recv, bytes).await?;
    if got != bytes {
        bail!("short download: got {got} of {bytes}");
    }
    Ok(started.elapsed())
}

// --- Server side: accept and dispatch request streams. ---

/// Serves the passive side of one probe connection until the client stops
/// opening streams or the connection ends (iroh's idle timeout reaps a
/// vanished peer). Each accepted stream carries one [`Request`] and is served
/// concurrently, so a liveness ping is answered while a bulk transfer is still
/// draining; concurrency is capped at [`MAX_STREAMS_PER_CONN`]. Pass an
/// `events` channel to observe [`ProbeEvent`]s as they happen (the app
/// surfaces them as throughput readouts); `None` serves silently.
pub async fn handle_connection(
    conn: endpoint::Connection,
    events: Option<mpsc::Sender<ProbeEvent>>,
) -> Result<()> {
    let limit = Arc::new(Semaphore::new(MAX_STREAMS_PER_CONN));
    // Serve tasks are owned here, so they are aborted the moment this returns
    // (the connection closed) rather than lingering on dropped streams.
    let mut streams = JoinSet::new();
    loop {
        // Reap finished serve tasks so the set does not grow without bound.
        while streams.try_join_next().is_some() {}
        let (mut send, mut recv) = match conn.accept_bi().await {
            Ok(streams) => streams,
            // The connection closed; no more requests will arrive.
            Err(_) => break,
        };
        // Block for a slot before serving the next stream, bounding how many an
        // unauthenticated peer can pin at once.
        let Ok(permit) = limit.clone().acquire_owned().await else {
            break;
        };
        let events = events.clone();
        streams.spawn(async move {
            let _permit = permit;
            if let Err(e) = serve(&mut send, &mut recv, events.as_ref()).await {
                warn!(err = %e, "probe: serving request failed");
            }
            // Closing our send side acks an upload and ends a ping/download.
            let _ = send.finish();
        });
    }
    Ok(())
}

/// Serves one accepted request stream: reads the [`Request`] header and
/// fulfills it. Generic over the stream types so it can run over an in-memory
/// duplex pair in tests. The caller finishes `send` afterwards.
async fn serve<S, R>(
    send: &mut S,
    recv: &mut R,
    events: Option<&mpsc::Sender<ProbeEvent>>,
) -> Result<()>
where
    S: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    match read_request(recv).await? {
        Request::Ping => send.write_all(&[0u8]).await.context("write pong")?,
        Request::Upload { bytes } => {
            check_size(bytes)?;
            let started = Instant::now();
            let got = drain(recv, bytes).await?;
            if got != bytes {
                bail!("short upload: got {got} of {bytes}");
            }
            let elapsed = started.elapsed();
            if let Some(events) = events {
                // Best-effort; drop the event if the consumer is gone.
                let _ = events.try_send(ProbeEvent::UploadCompleted { bytes, elapsed });
            }
        }
        Request::Download { bytes } => {
            check_size(bytes)?;
            write_payload(send, bytes).await?;
        }
    }
    Ok(())
}

fn check_size(bytes: u64) -> Result<()> {
    if bytes > MAX_TRANSFER_BYTES {
        bail!("transfer too large: {bytes} > {MAX_TRANSFER_BYTES}");
    }
    Ok(())
}

// --- Framing helpers, generic for testability. ---

async fn write_request<W: AsyncWrite + Unpin>(send: &mut W, request: Request) -> Result<()> {
    let mut buf = [0u8; Request::POSTCARD_MAX_SIZE];
    let encoded = postcard::to_slice(&request, &mut buf).context("encode request")?;
    send.write_u32(encoded.len() as u32)
        .await
        .context("write header len")?;
    send.write_all(encoded).await.context("write header")?;
    Ok(())
}

async fn read_request<R: AsyncRead + Unpin>(recv: &mut R) -> Result<Request> {
    let len = recv.read_u32().await.context("read header len")? as usize;
    if len > Request::POSTCARD_MAX_SIZE {
        bail!("request header too large: {len}");
    }
    let mut buf = vec![0u8; len];
    recv.read_exact(&mut buf).await.context("read header")?;
    postcard::from_bytes(&buf).context("decode request")
}

/// Writes `bytes` zero bytes in [`CHUNK`]-sized pieces.
async fn write_payload<W: AsyncWrite + Unpin>(send: &mut W, bytes: u64) -> Result<()> {
    let zeros = [0u8; CHUNK];
    let mut remaining = bytes;
    while remaining > 0 {
        let take = remaining.min(CHUNK as u64) as usize;
        send.write_all(&zeros[..take])
            .await
            .context("write payload")?;
        remaining -= take as u64;
    }
    Ok(())
}

/// Reads the stream to EOF and returns how many bytes it carried, bailing if it
/// runs past `limit` (the announced, [`check_size`]-bounded size) so a peer
/// cannot make us read an unbounded stream off a mis-announced transfer.
async fn drain<R: AsyncRead + Unpin>(recv: &mut R, limit: u64) -> Result<u64> {
    let mut buf = [0u8; CHUNK];
    let mut total = 0u64;
    loop {
        let n = recv.read(&mut buf).await.context("drain stream")?;
        if n == 0 {
            return Ok(total);
        }
        total += n as u64;
        if total > limit {
            bail!("transfer exceeded announced {limit} bytes");
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::duplex;

    use super::*;

    /// Round-trip every Request variant through postcard so a future
    /// serialization-shape change fails loudly here.
    #[test]
    fn request_postcard_roundtrip() {
        let cases = [
            Request::Ping,
            Request::Upload { bytes: 0 },
            Request::Upload {
                bytes: MAX_TRANSFER_BYTES,
            },
            Request::Download { bytes: 1 << 20 },
        ];
        let mut buf = [0u8; Request::POSTCARD_MAX_SIZE];
        for request in cases {
            let encoded = postcard::to_slice(&request, &mut buf).expect("encode");
            let decoded: Request = postcard::from_bytes(encoded).expect("decode");
            assert_eq!(
                request, decoded,
                "request did not survive postcard roundtrip"
            );
        }
    }

    #[test]
    fn max_transfer_bytes_is_bounded() {
        const _: () = assert!(MAX_TRANSFER_BYTES >= 1024 * 1024);
        const _: () = assert!(MAX_TRANSFER_BYTES <= 1024 * 1024 * 1024);
    }

    #[test]
    fn throughput_mbps_is_none_on_zero_time() {
        assert!(throughput_mbps(1024, Duration::ZERO).is_none());
        // 1 MiB in 100ms = ~83.9 Mbps.
        let mbps = throughput_mbps(1024 * 1024, Duration::from_millis(100)).unwrap();
        assert!((80.0..90.0).contains(&mbps), "got {mbps}");
    }

    /// A ping request makes the responder write a single pong byte.
    #[tokio::test]
    async fn serve_ping_writes_one_byte() {
        let (mut client_send, mut server_recv) = duplex(64 * 1024);
        let (mut server_send, mut client_recv) = duplex(64 * 1024);
        write_request(&mut client_send, Request::Ping)
            .await
            .unwrap();
        serve(&mut server_send, &mut server_recv, None)
            .await
            .unwrap();
        let mut pong = [0u8; 1];
        client_recv.read_exact(&mut pong).await.unwrap();
        assert_eq!(pong, [0u8]);
    }

    /// An upload request drains exactly the announced bytes and reports the
    /// completion event. Sized to fit the duplex buffer so the writer can
    /// finish before serve drains it, no second task needed.
    #[tokio::test]
    async fn serve_drains_upload_and_reports() {
        let (mut client_send, mut server_recv) = duplex(64 * 1024);
        let (mut server_send, _client_recv) = duplex(64 * 1024);
        let (tx, mut rx) = mpsc::channel(1);
        write_request(&mut client_send, Request::Upload { bytes: 4096 })
            .await
            .unwrap();
        write_payload(&mut client_send, 4096).await.unwrap();
        drop(client_send);
        serve(&mut server_send, &mut server_recv, Some(&tx))
            .await
            .unwrap();
        match rx.recv().await.expect("event") {
            ProbeEvent::UploadCompleted { bytes, .. } => assert_eq!(bytes, 4096),
        }
    }

    /// An upload that overruns its announced size is refused rather than
    /// drained without bound.
    #[tokio::test]
    async fn serve_rejects_upload_overrun() {
        let (mut client_send, mut server_recv) = duplex(64 * 1024);
        let (mut server_send, _client_recv) = duplex(64 * 1024);
        let server =
            tokio::spawn(async move { serve(&mut server_send, &mut server_recv, None).await });
        write_request(&mut client_send, Request::Upload { bytes: 1 })
            .await
            .unwrap();
        // Stream far more than announced; serve must bail before reading it all.
        let _ = write_payload(&mut client_send, 200_000).await;
        drop(client_send);
        let err = server.await.unwrap().unwrap_err();
        assert!(format!("{err:#}").contains("exceeded announced"));
    }

    /// An oversized announced transfer is refused immediately.
    #[tokio::test]
    async fn serve_rejects_oversized_transfer() {
        let (mut client_send, mut server_recv) = duplex(64 * 1024);
        let (mut server_send, _client_recv) = duplex(64 * 1024);
        write_request(
            &mut client_send,
            Request::Download {
                bytes: MAX_TRANSFER_BYTES + 1,
            },
        )
        .await
        .unwrap();
        drop(client_send);
        let err = serve(&mut server_send, &mut server_recv, None)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("too large"));
    }

    /// A download request makes the responder stream the requested bytes.
    #[tokio::test]
    async fn serve_download_streams_bytes() {
        let (mut client_send, mut server_recv) = duplex(64 * 1024);
        let (mut server_send, mut client_recv) = duplex(64 * 1024);
        let server = tokio::spawn(async move {
            serve(&mut server_send, &mut server_recv, None)
                .await
                .unwrap();
            drop(server_send);
        });
        write_request(&mut client_send, Request::Download { bytes: 150_000 })
            .await
            .unwrap();
        let got = drain(&mut client_recv, 150_000).await.unwrap();
        assert_eq!(got, 150_000);
        server.await.unwrap();
    }
}
