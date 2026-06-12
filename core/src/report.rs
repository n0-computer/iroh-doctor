//! UI-facing projections of [`iroh::NetReport`] shared by the cli and the
//! app, so both render the same numbers iroh measured.

use serde::Serialize;

/// One relay's latency as recorded by iroh's net report.
#[derive(Debug, Clone, Serialize)]
pub struct RelayLatencyRow {
    /// Display form of the relay URL.
    pub url: String,
    /// Lowest latency iroh recorded for this relay across its probes
    /// (QAD over IPv4/IPv6 and HTTPS).
    pub latency_ms: f64,
}

/// Projects the per-relay latencies out of a [`iroh::NetReport`], one row
/// per relay, sorted ascending by latency.
///
/// iroh records a latency per (relay, probe kind); this keeps the lowest
/// per relay, matching how iroh itself picks `preferred_relay`.
#[must_use]
pub fn relay_latencies(report: &iroh::NetReport) -> Vec<RelayLatencyRow> {
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
        assert!(relay_latencies(&iroh::NetReport::default()).is_empty());
    }
}
