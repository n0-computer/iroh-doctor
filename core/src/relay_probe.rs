//! Per-relay latency probe shared by the cli and the app.
//!
//! For each relay this measures the TLS connect time and a single
//! relay-protocol ping round-trip. Relays are probed concurrently and each
//! step has a [`PER_STEP_TIMEOUT`] budget, so unreachable relays bound the
//! whole sweep by at most twice that value rather than stalling it per relay.

use std::time::{Duration, Instant};

use iroh::dns::DnsResolver;
use iroh::{RelayMap, RelayUrl, SecretKey};
use iroh_relay::client::{Client, ClientBuilder};
use iroh_relay::protos::relay::{ClientToRelayMsg, RelayToClientMsg};
use iroh_relay::tls::{default_provider, CaRootsConfig};
use n0_future::{SinkExt, StreamExt};
use serde::Serialize;

/// One relay's connect and ping timings, or the error that prevented them.
#[derive(Debug, Clone, Serialize)]
pub struct RelayProbeResult {
    /// Display form of the relay URL.
    pub url: String,
    /// Time to set up the TLS connection, when the connect step succeeded.
    pub connect_ms: Option<f64>,
    /// Round-trip time of a single relay ping, when both connect and ping
    /// completed.
    pub ping_ms: Option<f64>,
    /// Short error description when the probe did not complete.
    pub error: Option<String>,
}

/// Per-step budget applied to the TLS connect and to the relay ping
/// separately. Worst case per relay is twice this value.
pub const PER_STEP_TIMEOUT: Duration = Duration::from_secs(3);

/// Orders rows ascending by `ping_ms`, with failures (no ping) last.
///
/// Extracted as a free function so callers and tests cannot drift from the
/// production sort.
#[must_use]
pub fn cmp_by_ping(a: &RelayProbeResult, b: &RelayProbeResult) -> std::cmp::Ordering {
    match (a.ping_ms, b.ping_ms) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

/// Probes every relay in `relay_map` once and returns a row per relay, sorted
/// ascending by `ping_ms` with failed relays last.
pub async fn probe_relays(relay_map: &RelayMap) -> Vec<RelayProbeResult> {
    let prober = match RelayProber::new() {
        Ok(p) => p,
        Err(e) => {
            return relay_map
                .relays::<Vec<_>>()
                .into_iter()
                .map(|c| RelayProbeResult {
                    url: c.url.to_string(),
                    connect_ms: None,
                    ping_ms: None,
                    error: Some(e.clone()),
                })
                .collect();
        }
    };

    let relays = relay_map.relays::<Vec<_>>();
    let mut out: Vec<RelayProbeResult> =
        n0_future::join_all(relays.iter().map(|config| prober.probe(&config.url))).await;
    out.sort_by(cmp_by_ping);
    out
}

/// Shared identity, DNS resolver, and TLS config for probing relays.
///
/// Building the TLS config is the expensive, fallible step; construct one
/// prober and reuse it across relays and rounds.
pub struct RelayProber {
    key: SecretKey,
    dns: DnsResolver,
    tls: rustls::ClientConfig,
}

impl RelayProber {
    /// Builds the shared TLS config and a fresh probe identity.
    ///
    /// # Errors
    ///
    /// Returns a short description when the TLS config cannot be built.
    pub fn new() -> Result<Self, String> {
        // iroh-relay 1.0.0-rc.1 dropped the implicit TLS config; every
        // `ClientBuilder` needs an explicit one or `connect` errors with
        // `MissingCryptoProvider`. Build one from the ring provider plus the
        // embedded Mozilla trust roots and share it across the sweep.
        // `embedded()` avoids platform-specific verifier facilities, keeping
        // the build matrix minimal.
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
    pub async fn probe(&self, url: &RelayUrl) -> RelayProbeResult {
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
pub async fn ping_relay(client: Client) -> Result<Duration, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(ping: Option<f64>, error: Option<&str>) -> RelayProbeResult {
        RelayProbeResult {
            url: "https://example/".into(),
            connect_ms: None,
            ping_ms: ping,
            error: error.map(str::to_string),
        }
    }

    #[test]
    fn cmp_by_ping_orders_ping_then_failure() {
        let mut v = [
            row(None, Some("fail")),
            row(Some(120.0), None),
            row(Some(40.0), None),
        ];
        v.sort_by(cmp_by_ping);
        assert_eq!(v[0].ping_ms, Some(40.0));
        assert_eq!(v[1].ping_ms, Some(120.0));
        assert!(v[2].error.is_some());
    }

    #[test]
    fn cmp_by_ping_treats_equal_pings_as_equal() {
        let a = row(Some(50.0), None);
        let b = row(Some(50.0), None);
        assert_eq!(cmp_by_ping(&a, &b), std::cmp::Ordering::Equal);
    }
}
