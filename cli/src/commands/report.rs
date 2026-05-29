//! `iroh-doctor report` command.
//!
//! One command that paints the whole picture of the current network: a NAT
//! classification on top of iroh's `NetReport`, which port-mapping protocols
//! (UPnP/PCP/NAT-PMP) the local gateway offers, and one round of per-relay
//! connect plus ping latency. The default output is a set of tables; `--json`
//! emits the same data as a single structure for piping into another tool.

use std::time::{Duration, Instant};

use anyhow::Context;
use iroh::{
    dns::DnsResolver, endpoint::presets, Endpoint, NetReport, RelayMap, RelayMode, SecretKey,
    Watcher,
};
use iroh_relay::client::ClientBuilder;
use iroh_relay::protos::relay::{ClientToRelayMsg, RelayToClientMsg};
use iroh_relay::tls::{default_provider, CaRootsConfig};
use n0_future::{SinkExt, StreamExt};
use portmapper::{Client as PortMapClient, Config as PortMapConfig, Protocol as PortMapProtocol};
use serde::Serialize;

use iroh_doctor_core::nat::{classify_base_report, NatType};

use crate::config::NodeConfig;

/// Combined output of the `report` command. Serialized when `--json` is set;
/// no consumer deserializes this in-process today, so `Deserialize` is
/// omitted to keep the public type surface tight.
#[derive(Debug, Serialize)]
pub struct Report {
    pub net_report: Option<NetReport>,
    pub nat: NatType,
    pub port_map: Option<PortMapBlock>,
    pub relays: Vec<RelayBlock>,
}

/// Which port-mapping protocols the local gateway answered for. `None` on a
/// field means the protocol was not probed; the whole block carries an
/// `error` when the probe itself failed.
#[derive(Debug, Serialize)]
pub struct PortMapBlock {
    pub upnp: Option<bool>,
    pub pcp: Option<bool>,
    pub nat_pmp: Option<bool>,
    pub error: Option<String>,
}

/// One relay's connect and ping timings, or the `error` that prevented them.
#[derive(Debug, Serialize)]
pub struct RelayBlock {
    pub url: String,
    pub connect_ms: Option<f64>,
    pub ping_ms: Option<f64>,
    pub error: Option<String>,
}

/// Wall-clock ceiling for the `net_report().initialized()` wait. A network
/// with no DNS or no reachable STUN endpoints would otherwise hang the
/// command indefinitely.
const NET_REPORT_TIMEOUT: Duration = Duration::from_secs(15);

/// Runs every probe and prints the combined report.
pub async fn report(
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
    let result = report_inner(&endpoint, &relay_map, no_port_map, no_relays, json).await;
    endpoint.close().await;
    result
}

async fn report_inner(
    endpoint: &Endpoint,
    relay_map: &RelayMap,
    no_port_map: bool,
    no_relays: bool,
    json: bool,
) -> anyhow::Result<()> {
    // Wait for the first non-empty report with a hard ceiling. The reporter
    // streams updates indefinitely; without a timeout the command would hang
    // on a network with no DNS or no reachable STUN.
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

    let report = Report {
        net_report: Some(net_report),
        nat,
        port_map,
        relays,
    };

    if json {
        let buf = serde_json::to_string_pretty(&report).context("encoding json")?;
        println!("{buf}");
    } else {
        print_tables(&report);
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
    // The client is kept alive across the .await: dropping it cancels the
    // in-flight probe. It is dropped at the end of the function, after the
    // probe future has resolved.
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

/// Comparator that orders rows ascending by `ping_ms` with failures (no ping)
/// at the bottom. Extracted as a free function so the test and the production
/// sort cannot drift.
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

/// Prints the report as a stack of markdown tables: a network summary, the
/// port-mapping availability, and the relay latencies.
fn print_tables(r: &Report) {
    let nr = r.net_report.as_ref();
    let summary = vec![
        kv("NAT", r.nat.to_string()),
        kv("UDP IPv4", opt_bool(nr.map(|n| n.udp_v4))),
        kv("UDP IPv6", opt_bool(nr.map(|n| n.udp_v6))),
        kv("Public IPv4", opt_addr(nr.and_then(|n| n.global_v4))),
        kv("Public IPv6", opt_addr(nr.and_then(|n| n.global_v6))),
        kv(
            "Mapping varies",
            opt_bool(nr.and_then(|n| n.mapping_varies_by_dest())),
        ),
        kv(
            "Captive portal",
            opt_bool(nr.and_then(|n| n.captive_portal)),
        ),
        kv(
            "Preferred relay",
            opt_addr(nr.and_then(|n| n.preferred_relay.as_ref())),
        ),
    ];
    println!("Network report");
    print!("{}", markdown_table(&["Property", "Value"], &summary));
    println!("{} - {}", r.nat, r.nat.description());
    println!();

    if let Some(pm) = &r.port_map {
        let rows = vec![
            kv("UPnP", tribool_text(pm.upnp)),
            kv("PCP", tribool_text(pm.pcp)),
            kv("NAT-PMP", tribool_text(pm.nat_pmp)),
        ];
        println!("Port mapping");
        print!("{}", markdown_table(&["Protocol", "Available"], &rows));
        if let Some(err) = &pm.error {
            println!("warning: {err}");
        }
        println!();
    }

    if !r.relays.is_empty() {
        let rows: Vec<Vec<String>> = r
            .relays
            .iter()
            .map(|row| {
                vec![
                    row.url.clone(),
                    fmt_opt_ms(row.connect_ms),
                    fmt_opt_ms(row.ping_ms),
                    row.error.clone().unwrap_or_default(),
                ]
            })
            .collect();
        println!("Relay latency");
        print!(
            "{}",
            markdown_table(&["Relay", "Connect", "Ping", "Note"], &rows)
        );
    }
}

/// Builds a two-cell key/value row for the summary table.
fn kv(key: &str, value: impl Into<String>) -> Vec<String> {
    vec![key.to_string(), value.into()]
}

/// Renders a GitHub-flavored markdown table: a header row, a `---` separator,
/// then one row per entry. Every column is padded to its widest cell so the
/// table also lines up when read straight from a terminal. Missing trailing
/// cells in a row render as empty.
fn markdown_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let cols = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(cols) {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let pad = |s: &str, w: usize| format!("{s}{}", " ".repeat(w - s.chars().count()));
    let line = |cells: Vec<String>| format!("| {} |\n", cells.join(" | "));

    let mut out = String::new();
    out.push_str(&line(
        headers
            .iter()
            .enumerate()
            .map(|(i, h)| pad(h, widths[i]))
            .collect(),
    ));
    out.push_str(&line(widths.iter().map(|w| "-".repeat(*w)).collect()));
    for row in rows {
        out.push_str(&line(
            (0..cols)
                .map(|i| pad(row.get(i).map_or("", String::as_str), widths[i]))
                .collect(),
        ));
    }
    out
}

/// "yes"/"no" for a known boolean, "unknown" when the report did not
/// determine it (or there was no report).
fn opt_bool(b: Option<bool>) -> &'static str {
    match b {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    }
}

/// "yes"/"no" for a known boolean, "(not probed)" when the protocol was not
/// probed. Used for the port-mapping block, where `None` is a deliberate skip
/// rather than an unknown.
fn tribool_text(b: Option<bool>) -> &'static str {
    match b {
        Some(true) => "yes",
        Some(false) => "no",
        None => "(not probed)",
    }
}

/// Renders an optional address (or any `Display`) as itself, or "-" when absent.
fn opt_addr<A: std::fmt::Display>(addr: Option<A>) -> String {
    addr.map_or_else(|| "-".to_string(), |a| a.to_string())
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
    fn opt_bool_covers_all_states() {
        assert_eq!(opt_bool(Some(true)), "yes");
        assert_eq!(opt_bool(Some(false)), "no");
        assert_eq!(opt_bool(None), "unknown");
    }

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
    fn opt_addr_renders_value_or_dash() {
        assert_eq!(opt_addr(Some("1.2.3.4:5")), "1.2.3.4:5");
        assert_eq!(opt_addr(None::<&str>), "-");
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

    #[test]
    fn markdown_table_aligns_and_separates() {
        let table = markdown_table(
            &["Protocol", "Available"],
            &[
                vec!["UPnP".into(), "yes".into()],
                vec!["NAT-PMP".into(), "(not probed)".into()],
            ],
        );
        let lines: Vec<&str> = table.lines().collect();
        // Header, separator, two rows.
        assert_eq!(lines.len(), 4);
        // Every column lines up, so all rows render to the same width.
        let width = lines[0].chars().count();
        assert!(lines.iter().all(|l| l.chars().count() == width));
        // Each line is a bordered markdown row.
        assert!(lines
            .iter()
            .all(|l| l.starts_with("| ") && l.ends_with(" |")));
        // The separator is only dashes, spaces, and pipes.
        assert!(lines[1].chars().all(|c| matches!(c, '-' | ' ' | '|')));
        assert!(lines[1].contains("---"));
        // Content survives the layout.
        assert!(lines[0].contains("Protocol") && lines[0].contains("Available"));
        assert!(lines[2].contains("UPnP") && lines[2].contains("yes"));
        assert!(lines[3].contains("NAT-PMP") && lines[3].contains("(not probed)"));
    }

    #[test]
    fn markdown_table_fills_missing_trailing_cells() {
        let table = markdown_table(&["A", "B"], &[vec!["x".into()]]);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 3);
        // The provided cell renders and the row is padded to full width.
        assert!(lines[2].starts_with("| x "));
        assert!(lines[2].ends_with(" |"));
        assert_eq!(lines[2].chars().count(), lines[0].chars().count());
    }
}
