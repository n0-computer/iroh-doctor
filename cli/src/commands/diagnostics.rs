//! `iroh-doctor diagnostics` command.
//!
//! One command that paints the whole picture of the current network: a NAT
//! classification on top of iroh's `NetReport`, the per-relay latencies iroh
//! recorded while building the report, and the iroh-services checks (whose
//! net_diagnostics covers the UPnP/PCP/NAT-PMP gateway protocols). The
//! default output is a set of tables; `--json` emits the same data as a
//! single structure for piping into another tool.
//!
//! The report projections and the NAT classifier live in `iroh-doctor-core`
//! so the app reports the same numbers; this module only orchestrates them
//! and renders the tables.

use anyhow::Context;
use iroh::{endpoint::presets, Endpoint, NetReport, RelayMap, RelayMode, Watcher};
use serde::Serialize;

use iroh_doctor_core::fmt::{opt_bool, tribool_text};
use iroh_doctor_core::nat::{classify_net_report, NatType};
use iroh_doctor_core::report::{relay_latencies, RelayLatencyRow};
use iroh_doctor_core::services::DiagnosticsReport as ServicesDiagnostics;
use iroh_doctor_core::NET_REPORT_TIMEOUT;

use crate::config::NodeConfig;

/// Combined output of the `report` command. Serialized when `--json` is set;
/// no consumer deserializes this in-process today, so `Deserialize` is
/// omitted to keep the public type surface tight.
#[derive(Debug, Serialize)]
pub struct Report {
    pub net_report: Option<NetReport>,
    pub nat: NatType,
    /// Per-relay latencies recorded by iroh while building the net report.
    pub relays: Vec<RelayLatencyRow>,
    /// iroh-services checks (ping + server-side net_diagnostics). `None` when
    /// services are opted out via `IROH_SERVICES_API_SECRET=""`.
    pub services: Option<ServicesBlock>,
}

/// The iroh-services-backed checks, paired with the local probes above.
#[derive(Debug, Serialize)]
pub struct ServicesBlock {
    pub ping_ms: Option<f64>,
    pub net_diagnostics: Option<ServicesDiagnostics>,
    pub error: Option<String>,
}

/// Runs every probe and prints the combined report.
pub async fn diagnostics(config: &NodeConfig, json: bool) -> anyhow::Result<()> {
    let relay_map = config.relay_map()?.unwrap_or_else(RelayMap::empty);

    let endpoint = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Custom(relay_map))
        .bind()
        .await?;

    // Run the actual work inside a helper so endpoint.close() always runs
    // even if one of the steps returns Err.
    let result = report_inner(&endpoint, json).await;
    endpoint.close().await;
    result
}

async fn report_inner(endpoint: &Endpoint, json: bool) -> anyhow::Result<()> {
    // Wait for the first non-empty report with a hard ceiling. The reporter
    // streams updates indefinitely; without a timeout the command would hang
    // on a network with no DNS or no reachable STUN.
    let net_report = tokio::time::timeout(NET_REPORT_TIMEOUT, endpoint.net_report().initialized())
        .await
        .context("net_report did not initialize within timeout")?;

    let nat = classify_net_report(&net_report);
    // The per-relay latencies come out of the report iroh already built;
    // no separate sweep needed.
    let relays = relay_latencies(&net_report);

    let services = run_services(endpoint).await;

    let report = Report {
        net_report: Some(net_report),
        nat,
        relays,
        services,
    };

    if json {
        let buf = serde_json::to_string_pretty(&report).context("encoding json")?;
        println!("{buf}");
    } else {
        print_tables(&report);
    }

    Ok(())
}

/// Runs the iroh-services checks (a ping plus the server-side
/// net_diagnostics) using the resolved API secret. Returns `None` when
/// services are opted out via `IROH_SERVICES_API_SECRET=""`.
async fn run_services(endpoint: &Endpoint) -> Option<ServicesBlock> {
    let secret = iroh_doctor_core::services::resolve_api_secret(
        iroh_doctor_core::services::SecretSource::BundledDefault,
    )?;
    let name = iroh_doctor_core::services::device_name(&endpoint.id().to_string());
    let client = match iroh_doctor_core::services::build_client(endpoint, &secret, &name).await {
        Ok(c) => c,
        Err(e) => {
            return Some(ServicesBlock {
                ping_ms: None,
                net_diagnostics: None,
                error: Some(format!("{e:#}")),
            })
        }
    };
    let ping_ms = iroh_doctor_core::services::ping(&client)
        .await
        .ok()
        .map(|d| d.as_secs_f64() * 1000.0);
    let (net_diagnostics, error) = match iroh_doctor_core::services::net_diagnostics(&client).await
    {
        Ok(report) => (Some(report), None),
        Err(e) => (None, Some(format!("{e:#}"))),
    };
    Some(ServicesBlock {
        ping_ms,
        net_diagnostics,
        error,
    })
}

/// Prints the report as a stack of markdown tables: a network summary, the
/// port-mapping availability, the relay latencies, and the iroh-services
/// checks.
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

    if !r.relays.is_empty() {
        let rows: Vec<Vec<String>> = r
            .relays
            .iter()
            .map(|row| vec![row.url.clone(), fmt_opt_ms(Some(row.latency_ms))])
            .collect();
        println!("Relay latency");
        print!("{}", markdown_table(&["Relay", "Latency"], &rows));
    }

    if let Some(s) = &r.services {
        println!();
        println!("Services");
        let mut rows = vec![kv("ping", fmt_opt_ms(s.ping_ms))];
        if let Some(n) = &s.net_diagnostics {
            rows.push(kv("iroh version", n.iroh_version.clone()));
            rows.push(kv("services version", n.iroh_services_version.clone()));
            rows.push(kv("direct addrs", n.direct_addrs.len().to_string()));
            rows.push(kv("UPnP", tribool_text(n.upnp)));
            rows.push(kv("PCP", tribool_text(n.pcp)));
            rows.push(kv("NAT-PMP", tribool_text(n.nat_pmp)));
        }
        print!("{}", markdown_table(&["Property", "Value"], &rows));
        if let Some(err) = &s.error {
            println!("warning: {err}");
        }
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
