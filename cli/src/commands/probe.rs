//! `iroh-doctor probe` umbrella command.
//!
//! Runs the three diagnostics a human typically wants in one go:
//!
//! - the iroh `NetReport` with a NAT classification,
//! - a direct UPnP/PCP/NAT-PMP probe,
//! - one round of per-relay connect plus ping latency.
//!
//! Each block prints under its own header. The `--json` flag emits a
//! single combined structure suitable for piping into another tool.

use std::time::{Duration, Instant};

use anyhow::Context;
use iroh::{
    dns::DnsResolver, endpoint::presets, Endpoint, RelayMap, RelayMode, SecretKey, Watcher,
};
use iroh_relay::client::ClientBuilder;
use iroh_relay::protos::relay::{ClientToRelayMsg, RelayToClientMsg};
use iroh_relay::tls::{default_provider, CaRootsConfig};
use n0_future::{SinkExt, StreamExt};
use portmapper::{Client as PortMapClient, Config as PortMapConfig, Protocol as PortMapProtocol};
use serde::Serialize;

use crate::config::NodeConfig;
use crate::nat_classifier::{classify_base_report, NatType};

/// Combined output of the `probe` command. Serialized when `--json` is
/// set; no consumer deserializes this in-process today, so `Deserialize`
/// is omitted to keep the public type surface tight.
#[derive(Debug, Serialize)]
pub struct ProbeReport {
    pub net_report: Option<iroh::NetReport>,
    pub nat: NatType,
    pub port_map: Option<PortMapBlock>,
    pub relays: Vec<RelayBlock>,
}

#[derive(Debug, Serialize)]
pub struct PortMapBlock {
    pub upnp: Option<bool>,
    pub pcp: Option<bool>,
    pub nat_pmp: Option<bool>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RelayBlock {
    pub url: String,
    pub connect_ms: Option<f64>,
    pub ping_ms: Option<f64>,
    pub error: Option<String>,
}

/// Wall-clock ceiling for the `net_report().initialized()` wait. A
/// network with no DNS or no reachable STUN endpoints would otherwise
/// hang the command indefinitely.
const NET_REPORT_TIMEOUT: Duration = Duration::from_secs(15);

/// Runs every probe and prints the combined report.
pub async fn probe(
    config: &NodeConfig,
    no_port_map: bool,
    no_relays: bool,
    json: bool,
) -> anyhow::Result<()> {
    let relay_map = config.relay_map()?.unwrap_or_else(RelayMap::empty);

    let endpoint = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Custom(relay_map.clone()))
        .bind()
        .await?;

    // Run the actual work inside a helper so endpoint.close() always runs
    // even if one of the steps returns Err.
    let result = probe_inner(&endpoint, &relay_map, no_port_map, no_relays, json).await;
    endpoint.close().await;
    result
}

async fn probe_inner(
    endpoint: &Endpoint,
    relay_map: &RelayMap,
    no_port_map: bool,
    no_relays: bool,
    json: bool,
) -> anyhow::Result<()> {
    // Wait for the first non-empty report with a hard ceiling. The
    // reporter streams updates indefinitely; without a timeout the
    // command would hang on a network with no DNS or no reachable STUN.
    let net_report = tokio::time::timeout(NET_REPORT_TIMEOUT, endpoint.net_report().initialized())
        .await
        .context("net_report did not initialize within timeout")?;
    let nat = classify_base_report(&net_report);

    let port_map = if no_port_map {
        None
    } else {
        Some(port_map_probe().await)
    };

    let relays = if no_relays {
        Vec::new()
    } else {
        probe_relays(relay_map).await
    };

    let report = ProbeReport {
        net_report: Some(net_report),
        nat,
        port_map,
        relays,
    };

    if json {
        let buf = serde_json::to_string_pretty(&report).context("encoding json")?;
        println!("{buf}");
    } else {
        print_text(&report);
    }

    Ok(())
}

async fn port_map_probe() -> PortMapBlock {
    let cfg = PortMapConfig {
        enable_upnp: true,
        enable_pcp: true,
        enable_nat_pmp: true,
        protocol: PortMapProtocol::Udp,
    };
    // Drop happens at end of scope after the probe future has been
    // awaited; the client is kept alive across the .await for that
    // reason. The earlier version added an explicit drop after the
    // match, which was a no-op because drop already happens at end of
    // function.
    let client = PortMapClient::new(cfg);
    let probe_rx = client.probe();
    match tokio::time::timeout(Duration::from_secs(5), probe_rx).await {
        Ok(Ok(Ok(p))) => PortMapBlock {
            upnp: Some(p.upnp),
            pcp: Some(p.pcp),
            nat_pmp: Some(p.nat_pmp),
            error: None,
        },
        Ok(Ok(Err(e))) => PortMapBlock {
            upnp: None,
            pcp: None,
            nat_pmp: None,
            error: Some(e.to_string()),
        },
        Ok(Err(_)) => PortMapBlock {
            upnp: None,
            pcp: None,
            nat_pmp: None,
            error: Some("probe service dropped".into()),
        },
        Err(_) => PortMapBlock {
            upnp: None,
            pcp: None,
            nat_pmp: None,
            error: Some("probe timed out".into()),
        },
    }
}

/// Comparator that orders rows ascending by `ping_ms` with failures
/// (no ping) at the bottom. Extracted as a free function so the test
/// and the production sort cannot drift. Mirrors
/// `iroh-pong::relay_probe::cmp_by_ping`.
fn cmp_by_ping(a: &RelayBlock, b: &RelayBlock) -> std::cmp::Ordering {
    match (a.ping_ms, b.ping_ms) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

async fn probe_relays(relay_map: &RelayMap) -> Vec<RelayBlock> {
    let dns = DnsResolver::new();
    let key = SecretKey::generate();
    // iroh-relay 1.0.0-rc.1 dropped the implicit TLS config; every
    // `ClientBuilder` needs an explicit one or `connect` errors with
    // `MissingCryptoProvider`. Build one and share it across the sweep.
    let tls = match CaRootsConfig::embedded().client_config(default_provider()) {
        Ok(cfg) => cfg,
        Err(e) => {
            return relay_map
                .relays::<Vec<_>>()
                .into_iter()
                .map(|c| RelayBlock {
                    url: c.url.to_string(),
                    connect_ms: None,
                    ping_ms: None,
                    error: Some(format!("tls: {e}")),
                })
                .collect();
        }
    };
    let mut rows: Vec<RelayBlock> = Vec::new();
    for config in relay_map.relays::<Vec<_>>() {
        rows.push(probe_one_relay(&config.url, &key, &dns, &tls).await);
    }
    rows.sort_by(cmp_by_ping);
    rows
}

async fn probe_one_relay(
    url: &iroh::RelayUrl,
    key: &SecretKey,
    dns: &DnsResolver,
    tls: &rustls::ClientConfig,
) -> RelayBlock {
    let builder =
        ClientBuilder::new(url.clone(), key.clone(), dns.clone()).tls_client_config(tls.clone());
    let started = Instant::now();
    let connect = tokio::time::timeout(Duration::from_secs(3), builder.connect()).await;
    let client = match connect {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            return RelayBlock {
                url: url.to_string(),
                connect_ms: None,
                ping_ms: None,
                error: Some(format!("connect: {e}")),
            };
        }
        Err(_) => {
            return RelayBlock {
                url: url.to_string(),
                connect_ms: None,
                ping_ms: None,
                error: Some("connect timed out".into()),
            };
        }
    };
    let connect_elapsed = started.elapsed();
    let (mut stream, mut sink) = client.split();
    let nonce: [u8; 8] = rand::random();
    let started = Instant::now();
    if let Err(e) = sink.send(ClientToRelayMsg::Ping(nonce)).await {
        return RelayBlock {
            url: url.to_string(),
            connect_ms: Some(connect_elapsed.as_secs_f64() * 1000.0),
            ping_ms: None,
            error: Some(format!("send ping: {e}")),
        };
    }
    let ping = tokio::time::timeout(Duration::from_secs(3), async move {
        while let Some(res) = stream.next().await {
            match res {
                Ok(RelayToClientMsg::Pong(d)) if d == nonce => return Ok(started.elapsed()),
                Ok(_) => continue,
                Err(e) => return Err(format!("recv: {e}")),
            }
        }
        Err("stream ended before pong".to_string())
    })
    .await;
    match ping {
        Ok(Ok(rtt)) => RelayBlock {
            url: url.to_string(),
            connect_ms: Some(connect_elapsed.as_secs_f64() * 1000.0),
            ping_ms: Some(rtt.as_secs_f64() * 1000.0),
            error: None,
        },
        Ok(Err(msg)) => RelayBlock {
            url: url.to_string(),
            connect_ms: Some(connect_elapsed.as_secs_f64() * 1000.0),
            ping_ms: None,
            error: Some(msg),
        },
        Err(_) => RelayBlock {
            url: url.to_string(),
            connect_ms: Some(connect_elapsed.as_secs_f64() * 1000.0),
            ping_ms: None,
            error: Some("ping timed out".into()),
        },
    }
}

fn print_text(r: &ProbeReport) {
    println!("== Net report ==");
    match &r.net_report {
        Some(rep) => println!("{rep:#?}"),
        None => println!("(no report)"),
    }
    println!();
    println!("NAT classification: {} - {}", r.nat, r.nat.description());
    println!();
    if let Some(pm) = &r.port_map {
        println!("== Port-map probe ==");
        println!("  UPnP:     {}", tribool_text(pm.upnp));
        println!("  PCP:      {}", tribool_text(pm.pcp));
        println!("  NAT-PMP:  {}", tribool_text(pm.nat_pmp));
        if let Some(err) = &pm.error {
            println!("  warning: {err}");
        }
        println!();
    }
    if !r.relays.is_empty() {
        println!("== Relay latency ==");
        for row in &r.relays {
            let connect = fmt_opt_ms(row.connect_ms);
            let ping = fmt_opt_ms(row.ping_ms);
            match &row.error {
                Some(err) => println!(
                    "  {url:60} connect={connect} ping={ping} err: {err}",
                    url = row.url
                ),
                None => println!("  {url:60} connect={connect} ping={ping}", url = row.url),
            }
        }
    }
}

fn tribool_text(b: Option<bool>) -> &'static str {
    match b {
        Some(true) => "yes",
        Some(false) => "no",
        None => "(not probed)",
    }
}

fn fmt_opt_ms(ms: Option<f64>) -> String {
    match ms {
        Some(v) => format!("{v:.1}ms"),
        None => "-".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tribool_text_covers_all_states() {
        assert_eq!(tribool_text(Some(true)), "yes");
        assert_eq!(tribool_text(Some(false)), "no");
        assert_eq!(tribool_text(None), "(not probed)");
    }

    #[test]
    fn fmt_opt_ms_formats_to_one_decimal_or_dash() {
        assert_eq!(fmt_opt_ms(Some(42.0)), "42.0ms");
        assert_eq!(fmt_opt_ms(Some(0.12)), "0.1ms");
        assert_eq!(fmt_opt_ms(None), "-");
    }

    #[test]
    fn relay_sort_orders_ping_then_failure() {
        let mut rows = [
            RelayBlock {
                url: "https://c/".into(),
                connect_ms: None,
                ping_ms: None,
                error: Some("fail".into()),
            },
            RelayBlock {
                url: "https://b/".into(),
                connect_ms: None,
                ping_ms: Some(120.0),
                error: None,
            },
            RelayBlock {
                url: "https://a/".into(),
                connect_ms: None,
                ping_ms: Some(40.0),
                error: None,
            },
        ];
        rows.sort_by(cmp_by_ping);
        assert_eq!(rows[0].url, "https://a/");
        assert_eq!(rows[1].url, "https://b/");
        assert!(rows[2].error.is_some());
    }
}
