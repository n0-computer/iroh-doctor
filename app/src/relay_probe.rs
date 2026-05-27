//! Per-relay latency probe.
//!
//! Mirrors `iroh-doctor`'s `relay-urls` command in shape: for each
//! relay in the active `RelayMap`, measures both the TLS connect time
//! and a single ping round-trip on the relay protocol. The UI calls
//! [`probe`] from the peer task; each step (connect, ping) has a
//! 3 s budget, so a single unreachable relay can stall the sweep by
//! at most 6 s before moving on.
//!
//! Source of inspiration:
//! `iroh-doctor/src/commands/relay_urls.rs` at `1.0.0-rc.0`. Keep in
//! sync with `iroh-doctor/src/commands/probe.rs` where the same
//! algorithm lives in the CLI's `probe` subcommand.

use std::time::{Duration, Instant};

use iroh::dns::DnsResolver;
use iroh::{RelayMap, RelayUrl, SecretKey};
use iroh_relay::client::ClientBuilder;
use iroh_relay::protos::relay::{ClientToRelayMsg, RelayToClientMsg};
use iroh_relay::tls::{default_provider, CaRootsConfig};
use n0_future::{SinkExt, StreamExt};

/// One row of probe output for the Diagnostics tab.
#[derive(Debug, Clone)]
pub struct RelayProbeResult {
    /// Display form of the relay URL.
    pub url: String,
    /// Time to set up the TLS connection to the relay, when the connect
    /// step succeeded.
    pub connect_ms: Option<f64>,
    /// Round-trip time of a single relay ping, when both the connect and
    /// the ping completed.
    pub ping_ms: Option<f64>,
    /// Short error description when the probe did not complete.
    pub error: Option<String>,
}

/// Per-step budget applied to the TLS connect and to the relay-protocol
/// ping separately. Worst-case per relay is twice this value; the
/// reviewer flagged that the prior 3 s comment understated the real
/// budget, so this constant is now named for what it actually bounds.
const PER_STEP_TIMEOUT: Duration = Duration::from_secs(3);

/// Comparator that orders rows ascending by `ping_ms` with failures
/// (no ping) at the bottom. Extracted as a free function so the test
/// and the production sort cannot drift.
pub fn cmp_by_ping(a: &RelayProbeResult, b: &RelayProbeResult) -> std::cmp::Ordering {
    match (a.ping_ms, b.ping_ms) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

/// Probes every relay in `relay_map` once and returns a row per relay,
/// sorted ascending by `ping_ms` with failed relays last. The doctor
/// keeps separate success and failure lists; the app merges them so the
/// table renders in one place.
pub async fn probe(relay_map: RelayMap) -> Vec<RelayProbeResult> {
    let dns = DnsResolver::new();
    let key = SecretKey::generate();
    // iroh-relay 1.0.0-rc.1 dropped the implicit TLS config; every
    // `ClientBuilder::new` must be paired with an explicit
    // `tls_client_config` or `connect` errors with
    // `MissingCryptoProvider`. Build one ClientConfig from the ring
    // provider plus the embedded Mozilla trust roots and share it
    // across every probed relay. `embedded()` does not depend on
    // platform-specific verifier facilities, which keeps the build
    // matrix minimal.
    let tls = match CaRootsConfig::embedded().client_config(default_provider()) {
        Ok(cfg) => cfg,
        Err(e) => {
            return relay_map
                .relays::<Vec<_>>()
                .into_iter()
                .map(|c| RelayProbeResult {
                    url: c.url.to_string(),
                    connect_ms: None,
                    ping_ms: None,
                    error: Some(format!("tls: {e}")),
                })
                .collect();
        }
    };
    let mut out: Vec<RelayProbeResult> = Vec::new();

    for config in relay_map.relays::<Vec<_>>() {
        let url = config.url.clone();
        let row = probe_one(&url, &key, &dns, &tls).await;
        out.push(row);
    }

    out.sort_by(cmp_by_ping);
    out
}

async fn probe_one(
    url: &RelayUrl,
    key: &SecretKey,
    dns: &DnsResolver,
    tls: &rustls::ClientConfig,
) -> RelayProbeResult {
    let builder =
        ClientBuilder::new(url.clone(), key.clone(), dns.clone()).tls_client_config(tls.clone());
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
    let connect_elapsed = started.elapsed();

    match ping_relay(client).await {
        Ok(ping) => RelayProbeResult {
            url: url.to_string(),
            connect_ms: Some(connect_elapsed.as_secs_f64() * 1000.0),
            ping_ms: Some(ping.as_secs_f64() * 1000.0),
            error: None,
        },
        Err(msg) => RelayProbeResult {
            url: url.to_string(),
            connect_ms: Some(connect_elapsed.as_secs_f64() * 1000.0),
            ping_ms: None,
            error: Some(msg),
        },
    }
}

async fn ping_relay(client: iroh_relay::client::Client) -> Result<Duration, String> {
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
    fn sort_orders_ping_then_failure() {
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
