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
use n0_future::{SinkExt, StreamExt};
use portmapper::{Client as PortMapClient, Config as PortMapConfig, Protocol as PortMapProtocol};
use serde::{Deserialize, Serialize};

use crate::config::NodeConfig;
use crate::nat_classifier::{classify_base_report, NatType};

/// Combined output of the `probe` command. Serialized when `--json` is
/// set.
#[derive(Debug, Serialize, Deserialize)]
pub struct ProbeReport {
    pub net_report: Option<iroh::NetReport>,
    pub nat: NatType,
    pub port_map: Option<PortMapBlock>,
    pub relays: Vec<RelayBlock>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PortMapBlock {
    pub upnp: Option<bool>,
    pub pcp: Option<bool>,
    pub nat_pmp: Option<bool>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RelayBlock {
    pub url: String,
    pub connect_ms: Option<f64>,
    pub ping_ms: Option<f64>,
    pub error: Option<String>,
}

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

    // Wait for the first non-empty report. The reporter streams updates,
    // so a single .initialized() is enough for a one-shot summary.
    let net_report = endpoint.net_report().initialized().await;
    let nat = classify_base_report(&net_report);

    let port_map = if no_port_map {
        None
    } else {
        Some(port_map_probe().await)
    };

    let relays = if no_relays {
        Vec::new()
    } else {
        probe_relays(&relay_map).await
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

    endpoint.close().await;
    Ok(())
}

async fn port_map_probe() -> PortMapBlock {
    let cfg = PortMapConfig {
        enable_upnp: true,
        enable_pcp: true,
        enable_nat_pmp: true,
        protocol: PortMapProtocol::Udp,
    };
    let client = PortMapClient::new(cfg);
    let probe_rx = client.probe();
    let out = match tokio::time::timeout(Duration::from_secs(5), probe_rx).await {
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
    };
    drop(client);
    out
}

async fn probe_relays(relay_map: &RelayMap) -> Vec<RelayBlock> {
    let dns = DnsResolver::new();
    let key = SecretKey::generate();
    let mut rows: Vec<RelayBlock> = Vec::new();
    for config in relay_map.relays::<Vec<_>>() {
        rows.push(probe_one_relay(&config.url, &key, &dns).await);
    }
    rows.sort_by(|a, b| match (a.ping_ms, b.ping_ms) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    rows
}

async fn probe_one_relay(url: &iroh::RelayUrl, key: &SecretKey, dns: &DnsResolver) -> RelayBlock {
    let builder = ClientBuilder::new(url.clone(), key.clone(), dns.clone());
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
