//! UI-facing projections of [`iroh::unstable_net_report::NetReport`] shared by the cli and the
//! app, so both render the same numbers iroh measured.

use serde::Serialize;

/// Direct snapshot of `iroh::unstable_net_report::NetReport`, reshaped for a diagnostics view.
/// Captures the NAT classification, IPv4 and IPv6 visibility, and the
/// preferred relay so a front end can show a one-glance summary of the
/// local network environment.
#[derive(Debug, Clone)]
pub struct NetReportSummary {
    pub nat: crate::nat::NatType,
    /// Globally routable IPv4 SocketAddr observed during the probe.
    pub global_v4: Option<String>,
    /// Globally routable IPv6 SocketAddr observed during the probe.
    pub global_v6: Option<String>,
    pub udp_v4: bool,
    pub udp_v6: bool,
    pub mapping_varies_v4: Option<bool>,
    pub mapping_varies_v6: Option<bool>,
    /// `Some(true)` if the network appears to be a captive portal.
    pub captive_portal: Option<bool>,
    /// URL of the relay with the lowest measured latency.
    pub preferred_relay: Option<String>,
    /// Total number of relays for which a latency sample was recorded.
    pub relays_seen: usize,
}

impl From<&iroh::unstable_net_report::NetReport> for NetReportSummary {
    fn from(r: &iroh::unstable_net_report::NetReport) -> Self {
        Self {
            nat: crate::nat::classify_net_report(r),
            global_v4: r.global_v4.map(|a| a.to_string()),
            global_v6: r.global_v6.map(|a| a.to_string()),
            udp_v4: r.udp_v4,
            udp_v6: r.udp_v6,
            mapping_varies_v4: r.mapping_varies_by_dest_ipv4,
            mapping_varies_v6: r.mapping_varies_by_dest_ipv6,
            captive_portal: r.captive_portal,
            preferred_relay: r.preferred_relay.as_ref().map(|u| u.to_string()),
            relays_seen: relay_latencies(r).len(),
        }
    }
}

/// One relay's latency as recorded by iroh's net report.
#[derive(Debug, Clone, Serialize)]
pub struct RelayLatencyRow {
    /// Display form of the relay URL.
    pub url: String,
    /// Lowest latency iroh recorded for this relay across its probes
    /// (QAD over IPv4/IPv6 and HTTPS).
    pub latency_ms: f64,
}

/// Projects the per-relay latencies out of a [`iroh::unstable_net_report::NetReport`], one row
/// per relay, sorted ascending by latency.
///
/// iroh records a latency per (relay, probe kind); this keeps the lowest
/// per relay, matching how iroh itself picks `preferred_relay`.
#[must_use]
pub fn relay_latencies(report: &iroh::unstable_net_report::NetReport) -> Vec<RelayLatencyRow> {
    let mut lowest: std::collections::BTreeMap<String, f64> = Default::default();
    for (_probe, url, latency) in report.relay_latency.iter() {
        let ms = latency.as_secs_f64() * 1000.0;
        lowest
            .entry(url.to_string())
            .and_modify(|v| *v = v.min(ms))
            .or_insert(ms);
    }
    let mut rows: Vec<RelayLatencyRow> = lowest
        .into_iter()
        .map(|(url, latency_ms)| RelayLatencyRow { url, latency_ms })
        .collect();
    rows.sort_by(|a, b| {
        a.latency_ms
            .partial_cmp(&b.latency_ms)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_report_yields_no_rows() {
        assert!(relay_latencies(&iroh::unstable_net_report::NetReport::default()).is_empty());
    }
}
