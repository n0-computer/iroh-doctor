//! Relay URLs command implementation.
//!
//! Actively probes each relay in the config: a TLS connect followed by a
//! relay-protocol ping, each bounded by [`PER_STEP_TIMEOUT`]. This is the
//! one place that still dials relays by hand: the command's whole purpose
//! is to measure user-configured relay URLs on demand, which the
//! endpoint's own net report (scoped to the bound relay map, on iroh's
//! schedule) does not cover.

use std::time::{Duration, Instant};

use iroh::dns::DnsResolver;
use iroh::{RelayUrl, SecretKey};
use iroh_relay::client::{Client, ClientBuilder};
use iroh_relay::protos::relay::{ClientToRelayMsg, RelayToClientMsg};
use iroh_relay::tls::{default_provider, CaRootsConfig};
use n0_future::{SinkExt, StreamExt};

use crate::config::NodeConfig;

/// Per-step budget applied to the TLS connect and to the relay ping
/// separately. Worst case per relay is twice this value.
const PER_STEP_TIMEOUT: Duration = Duration::from_secs(3);

/// One relay's connect and ping timings, or the error that prevented them.
struct RelayProbeResult {
    url: String,
    connect_ms: Option<f64>,
    ping_ms: Option<f64>,
    error: Option<String>,
}

/// Checks a certain amount (`count`) of the nodes given by the [`NodeConfig`].
pub async fn relay_urls(count: usize, config: &NodeConfig) -> anyhow::Result<()> {
    if config.relay_nodes.is_empty() {
        println!("No relay nodes specified in the config file.");
    }

    let prober = RelayProber::new().map_err(|e| anyhow::anyhow!("build relay prober: {e}"))?;

    let mut success = Vec::new();
    let mut fail = Vec::new();

    for i in 0..count {
        println!("Round {}/{count}", i + 1);
        for node in &config.relay_nodes {
            let result = prober.probe(&node.url).await;
            if result.error.is_none() {
                success.push(result);
            } else {
                fail.push(result);
            }
        }
    }

    if !success.is_empty() {
        println!("Relay Node Latencies:");
        println!();
    }
    for node in success {
        print_result(&node);
        println!();
    }
    if !fail.is_empty() {
        println!("Connection Failures:");
        println!();
    }
    for node in fail {
        print_result(&node);
        println!();
    }

    Ok(())
}

/// Shared identity, DNS resolver, and TLS config for probing relays.
///
/// Building the TLS config is the expensive, fallible step; construct one
/// prober and reuse it across relays and rounds.
struct RelayProber {
    key: SecretKey,
    dns: DnsResolver,
    tls: rustls::ClientConfig,
}

impl RelayProber {
    /// Builds the shared TLS config and a fresh probe identity.
    fn new() -> Result<Self, String> {
        // iroh-relay 1.0.0-rc.1 dropped the implicit TLS config; every
        // `ClientBuilder` needs an explicit one or `connect` errors with
        // `MissingCryptoProvider`. Build one from the ring provider plus the
        // embedded Mozilla trust roots and share it across the sweep.
        let tls = CaRootsConfig::embedded()
            .client_config(default_provider())
            .map_err(|e| format!("tls: {e}"))?;
        Ok(Self {
            key: SecretKey::generate(),
            dns: DnsResolver::new(),
            tls,
        })
    }

    /// Probes one relay: a TLS connect, then a single relay ping, each
    /// bounded by [`PER_STEP_TIMEOUT`].
    async fn probe(&self, url: &RelayUrl) -> RelayProbeResult {
        let builder = ClientBuilder::new(url.clone(), self.key.clone(), self.dns.clone())
            .tls_client_config(self.tls.clone());
        let started = Instant::now();
        let connect = tokio::time::timeout(PER_STEP_TIMEOUT, builder.connect()).await;
        let client = match connect {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => {
                return RelayProbeResult {
                    url: url.to_string(),
                    connect_ms: None,
                    ping_ms: None,
                    error: Some(format!("connect: {e}")),
                };
            }
            Err(_) => {
                return RelayProbeResult {
                    url: url.to_string(),
                    connect_ms: None,
                    ping_ms: None,
                    error: Some("connect timed out".into()),
                };
            }
        };
        let connect_ms = Some(started.elapsed().as_secs_f64() * 1000.0);

        match ping_relay(client).await {
            Ok(ping) => RelayProbeResult {
                url: url.to_string(),
                connect_ms,
                ping_ms: Some(ping.as_secs_f64() * 1000.0),
                error: None,
            },
            Err(msg) => RelayProbeResult {
                url: url.to_string(),
                connect_ms,
                ping_ms: None,
                error: Some(msg),
            },
        }
    }
}

/// Sends one ping on an established relay client and returns the round-trip
/// time, or a short error description. Bounded by [`PER_STEP_TIMEOUT`].
async fn ping_relay(client: Client) -> Result<Duration, String> {
    let (mut stream, mut sink) = client.split();
    let nonce: [u8; 8] = rand::random();
    let start = Instant::now();
    sink.send(ClientToRelayMsg::Ping(nonce))
        .await
        .map_err(|e| format!("send ping: {e}"))?;
    match tokio::time::timeout(PER_STEP_TIMEOUT, async move {
        while let Some(res) = stream.next().await {
            match res {
                Ok(RelayToClientMsg::Pong(d)) if d == nonce => return Ok(start.elapsed()),
                Ok(_) => continue,
                Err(e) => return Err(format!("recv: {e}")),
            }
        }
        Err("stream ended before pong".to_string())
    })
    .await
    {
        Ok(res) => res,
        Err(_) => Err("ping timed out".into()),
    }
}

fn print_result(r: &RelayProbeResult) {
    match &r.error {
        None => println!(
            "Node {}\nConnect: {}\nLatency: {}",
            r.url,
            fmt_ms(r.connect_ms),
            fmt_ms(r.ping_ms)
        ),
        Some(err) => println!("Node {}\nConnection Error: {err:?}", r.url),
    }
}

fn fmt_ms(ms: Option<f64>) -> String {
    ms.map_or_else(|| "-".to_string(), |v| format!("{v:.1}ms"))
}
