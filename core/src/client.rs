//! The active side of the probe: a [`Client`] owns a connection and drives the
//! measurement loop.
//!
//! [`Client::connect`] dials a peer; [`Client::run`] pings once per interval
//! and uploads on a cadence, emitting a [`ClientSample`] per measurement. The
//! one-off [`Client::ping`]/[`Client::upload`]/[`Client::download`] primitives
//! are exposed for an explicit single test. This is the shared driver behind
//! both `iroh-doctor connect` and the app's Connect action, so both report a
//! connection the same way.

use std::time::Duration;

use anyhow::{Context, Result};
use iroh::endpoint::Connection;
use iroh::{Endpoint, EndpointAddr};
use tokio::sync::mpsc;

use crate::probe;

/// Pacing for the active probe client loop driven by [`Client::run`].
#[derive(Debug, Clone, Copy)]
pub struct ClientConfig {
    /// Delay between successive pings.
    pub ping_interval: Duration,
    /// Bytes to upload on each throughput sample. Bounded by
    /// [`probe::MAX_TRANSFER_BYTES`]; a larger value fails the upload
    /// immediately.
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

/// A single measurement emitted by [`Client::run`].
#[derive(Debug, Clone)]
pub enum ClientSample {
    /// Round-trip time observed for ping `nonce`.
    Latency { nonce: u32, rtt: Duration },
    /// A completed upload of `bytes` bytes that the peer acknowledged in
    /// `elapsed`. Pair with [`probe::throughput_mbps`].
    Throughput { bytes: u64, elapsed: Duration },
}

/// Why the active probe client loop stopped.
#[derive(Debug, Clone)]
pub struct ClientEnd {
    /// Which phase ended the loop: `"latency"`, `"throughput"`, or `"closed"`
    /// when the sample consumer went away.
    pub phase: &'static str,
    /// Human-readable cause, suitable for surfacing in a status line.
    pub cause: String,
}

/// An active probe client against one peer. Cheap to hold; it owns only the
/// QUIC connection, which is reference-counted by iroh.
#[derive(Debug, Clone)]
pub struct Client {
    conn: Connection,
}

impl Client {
    /// Dials `addr` on the probe ALPN.
    ///
    /// # Errors
    ///
    /// Returns the dial error if the connection cannot be established.
    pub async fn connect(endpoint: &Endpoint, addr: EndpointAddr) -> Result<Self> {
        let conn = endpoint
            .connect(addr, probe::ALPN)
            .await
            .context("connect probe")?;
        Ok(Self { conn })
    }

    /// Wraps an already-established probe connection.
    #[must_use]
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }

    /// The underlying QUIC connection, for callers that watch its paths or read
    /// its close reason directly.
    #[must_use]
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Sends one liveness ping and returns the round-trip time.
    ///
    /// # Errors
    ///
    /// Propagates the stream errors from [`crate::probe`].
    pub async fn ping(&self) -> Result<Duration> {
        probe::ping(&self.conn).await
    }

    /// Uploads `bytes` to the peer and returns the elapsed time.
    ///
    /// # Errors
    ///
    /// Propagates the transfer errors from [`crate::probe`], including the
    /// peer's rejection of an oversized transfer.
    pub async fn upload(&self, bytes: u64) -> Result<Duration> {
        probe::upload(&self.conn, bytes).await
    }

    /// Downloads `bytes` from the peer and returns the elapsed time.
    ///
    /// # Errors
    ///
    /// Propagates the transfer errors from [`crate::probe`], including the
    /// peer's rejection of an oversized transfer.
    pub async fn download(&self, bytes: u64) -> Result<Duration> {
        probe::download(&self.conn, bytes).await
    }

    /// Runs the measurement loop, emitting one [`ClientSample`] on `samples`
    /// per ping and per upload until a ping or upload fails, the peer goes
    /// away, or the sample consumer is dropped.
    ///
    /// Each ping and each upload opens its own bidi stream; pings use a
    /// high-priority stream. Returns a [`ClientEnd`] describing why the loop
    /// stopped. It does not error: a dead peer is the normal end of a monitor
    /// session, not a failure.
    pub async fn run(
        &self,
        config: ClientConfig,
        samples: mpsc::Sender<ClientSample>,
    ) -> ClientEnd {
        let mut nonce: u32 = 0;
        loop {
            match self.ping().await {
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
                match self.upload(config.upload_bytes).await {
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
}

/// The loop's end when the [`ClientSample`] consumer is dropped, which the app
/// does by aborting the monitor task on a fresh dial.
fn consumer_gone() -> ClientEnd {
    ClientEnd {
        phase: "closed",
        cause: "monitor consumer dropped".to_string(),
    }
}
