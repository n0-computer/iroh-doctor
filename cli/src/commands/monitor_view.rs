//! Shared live-monitor dashboard used by `connect` and `accept`.
//!
//! Both sides render the same `indicatif` rows: connection state, the path
//! table, time-to-first-direct-byte, latency over time with a sparkline,
//! and throughput. `connect` drives latency by pinging the peer; `accept`
//! reads it off the connection's path RTT. Throughput on `connect` comes
//! from `ProbeClient::upload`, on `accept` from
//! `iroh_doctor_core::probe::ProbeEvent::UploadCompleted`.

use std::{collections::VecDeque, time::Duration};

use console::style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use iroh::EndpointId;
use iroh_doctor_core::{
    monitor::{PathKind, PathSnapshot, StateKind},
    probe::throughput_mbps,
};

/// Number of recent RTT samples kept for the sparkline and running stats.
pub(crate) const HISTORY_LEN: usize = 60;

/// A small dashboard of in-place progress lines. Each row is an
/// `indicatif` progress bar whose message we rewrite as new data arrives,
/// so nothing scrolls past.
#[derive(Clone)]
pub(crate) struct MonitorView {
    state_pb: ProgressBar,
    paths_pb: ProgressBar,
    latency_pb: ProgressBar,
    spark_pb: ProgressBar,
    throughput_pb: ProgressBar,
    ttfdb_pb: ProgressBar,
}

impl MonitorView {
    /// Build a dashboard pinned to `mp`. `header` is rendered above the
    /// rows (e.g. `monitoring <peer>` for `connect` or `accepted <peer>`
    /// for `accept`).
    pub(crate) fn new(mp: &MultiProgress, header: String) -> Self {
        let template = ProgressStyle::default_bar().template("{msg}").unwrap();
        let make = || {
            let pb = mp.add(ProgressBar::hidden());
            pb.set_style(template.clone());
            pb.enable_steady_tick(Duration::from_millis(250));
            pb.set_message("");
            pb
        };
        let header_pb = make();
        let state_pb = make();
        let paths_pb = make();
        let ttfdb_pb = make();
        let latency_pb = make();
        let spark_pb = make();
        let throughput_pb = make();

        header_pb.set_message(header);
        state_pb.set_message(format!(
            "{}        {}",
            style("state:").dim(),
            style("waiting").yellow()
        ));
        paths_pb.set_message(format!("{}        -", style("paths:").dim()));
        ttfdb_pb.set_message(format!("{}        -", style("ttfdb:").dim()));
        latency_pb.set_message(format!("{}      -", style("latency:").dim()));
        spark_pb.set_message(format!("{}        -", style("graph:").dim()));
        throughput_pb.set_message(format!("{}   -", style("throughput:").dim()));

        Self {
            state_pb,
            paths_pb,
            latency_pb,
            spark_pb,
            throughput_pb,
            ttfdb_pb,
        }
    }

    /// Convenience: build the "monitoring <peer>" header used by `connect`.
    pub(crate) fn monitoring_header(peer: EndpointId) -> String {
        let peer_short: String = format!("{peer:#}").chars().take(16).collect();
        format!(
            "{} {}",
            style("monitoring").bold().cyan(),
            style(peer_short).dim()
        )
    }

    /// Convenience: build the "accepted <peer>" header used by `accept`.
    pub(crate) fn accepted_header(peer: EndpointId) -> String {
        let peer_short: String = format!("{peer:#}").chars().take(16).collect();
        format!(
            "{} {}",
            style("accepted").bold().cyan(),
            style(peer_short).dim()
        )
    }

    pub(crate) fn set_state(&self, kind: StateKind) {
        let label = match kind {
            StateKind::Direct => "direct",
            StateKind::Relay => "relay",
            StateKind::Custom => "custom",
            StateKind::NoPath => "no path",
        };
        let painted = match kind {
            StateKind::Direct => style(label).bold().green(),
            StateKind::Relay => style(label).bold().yellow(),
            StateKind::Custom => style(label).bold().magenta(),
            StateKind::NoPath => style(label).dim(),
        };
        self.state_pb
            .set_message(format!("{}        {painted}", style("state:").dim()));
    }

    pub(crate) fn set_paths(&self, lines: Vec<String>) {
        let body = if lines.is_empty() {
            style("(no paths)").dim().to_string()
        } else {
            lines.join("\n  ")
        };
        self.paths_pb
            .set_message(format!("{}\n  {body}", style("paths:").dim()));
    }

    pub(crate) fn set_ttfdb(&self, elapsed: Duration) {
        let ms = elapsed.as_secs_f64() * 1000.0;
        self.ttfdb_pb.set_message(format!(
            "{}        {}",
            style("ttfdb:").dim(),
            style(format!("{ms:.0} ms")).bold()
        ));
    }

    pub(crate) fn set_latency(&self, latest: Duration, history: &VecDeque<Duration>) {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let latest_ms = ms(latest);
        let latest_str = format!("{latest_ms:>6.1} ms");
        let latest_painted = if latest_ms < 50.0 {
            style(latest_str).bold().green()
        } else if latest_ms < 150.0 {
            style(latest_str).bold().yellow()
        } else {
            style(latest_str).bold().red()
        };
        let (min, avg, max, count) = stats(history);
        self.latency_pb.set_message(format!(
            "{}      {latest_painted}  {}",
            style("latency:").dim(),
            style(format!(
                "(min {:.1} / avg {:.1} / max {:.1}, n={count})",
                ms(min),
                ms(avg),
                ms(max),
            ))
            .dim(),
        ));
        self.spark_pb.set_message(format!(
            "{}        {}",
            style("graph:").dim(),
            style(sparkline(history)).cyan()
        ));
    }

    pub(crate) fn set_throughput(&self, bytes: u64, elapsed: Duration) {
        let mbps_str = throughput_mbps(bytes, elapsed)
            .map(|m| format!("{m:>6.1} Mbps"))
            .unwrap_or_else(|| "       -    ".to_string());
        let mib = bytes as f64 / (1024.0 * 1024.0);
        let ms = elapsed.as_secs_f64() * 1000.0;
        self.throughput_pb.set_message(format!(
            "{}   {} {}",
            style("throughput:").dim(),
            style(mbps_str).bold(),
            style(format!("({mib:.1} MiB in {ms:.0} ms)")).dim(),
        ));
    }

    pub(crate) fn set_probe_ended(&self, kind: &str, cause: impl std::fmt::Display) {
        self.latency_pb.set_message(format!(
            "{}      {}",
            style("latency:").dim(),
            style(format!("{kind} ended: {cause}")).red()
        ));
    }
}

/// Renders one aligned line per path: a selection marker, the transport
/// kind, the remote address, and the path RTT.
#[must_use]
pub(crate) fn format_path_lines(paths: &[PathSnapshot]) -> Vec<String> {
    paths
        .iter()
        .map(|p| {
            let sel = if p.selected { '*' } else { ' ' };
            let kind_str = match p.kind {
                PathKind::Direct => "direct",
                PathKind::Relay => "relay ",
                PathKind::Custom => "custom",
            };
            let rtt_ms = p.rtt.as_secs_f64() * 1000.0;
            format!("{sel} {kind_str}  {:<44}  rtt {rtt_ms:>6.1} ms", p.addr)
        })
        .collect()
}

/// Returns the min, average, max, and sample count of the RTT history.
/// All-zero with a count of zero when the history is empty.
#[must_use]
pub(crate) fn stats(history: &VecDeque<Duration>) -> (Duration, Duration, Duration, u64) {
    if history.is_empty() {
        return (Duration::ZERO, Duration::ZERO, Duration::ZERO, 0);
    }
    let min = history.iter().min().copied().unwrap_or(Duration::ZERO);
    let max = history.iter().max().copied().unwrap_or(Duration::ZERO);
    let total: Duration = history.iter().copied().sum();
    let count = history.len() as u64;
    let avg = total / (count as u32);
    (min, avg, max, count)
}

/// Renders the RTT history as a unicode block sparkline, scaled to the
/// window's own min and max so relative variation is visible.
#[must_use]
pub(crate) fn sparkline(samples: &VecDeque<Duration>) -> String {
    const CHARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if samples.is_empty() {
        return String::new();
    }
    let min = samples.iter().min().copied().unwrap();
    let max = samples.iter().max().copied().unwrap();
    let range = max.saturating_sub(min);
    samples
        .iter()
        .map(|d| {
            if range.is_zero() {
                CHARS[0]
            } else {
                let span = (*d - min).as_nanos() as f64;
                let total = range.as_nanos() as f64;
                let idx = ((span / total) * (CHARS.len() as f64 - 1.0)).round() as usize;
                CHARS[idx.min(CHARS.len() - 1)]
            }
        })
        .collect()
}
