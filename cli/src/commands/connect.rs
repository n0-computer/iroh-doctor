//! Connect command implementation

use std::{
    collections::VecDeque,
    net::SocketAddr,
    time::{Duration, Instant},
};

use console::style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayUrl};
use iroh_doctor_core::probe::{throughput_mbps, ProbeClient};
use n0_future::StreamExt;

use crate::doctor::{log_connection_changes, passive_side, Gui};

/// Connects to a [`EndpointId`].
///
/// By default this runs a live connection monitor against the peer's probe
/// protocol (state, paths, latency over time, periodic throughput, ttfdb).
/// With `test` set it runs the legacy doctor throughput test as the passive
/// side, pairing with `iroh-doctor accept`.
pub async fn connect(
    endpoint_id: EndpointId,
    direct_addresses: Vec<SocketAddr>,
    relay_url: Option<RelayUrl>,
    endpoint: Endpoint,
    test: bool,
) -> anyhow::Result<()> {
    let res = run(
        endpoint_id,
        direct_addresses,
        relay_url,
        endpoint.clone(),
        test,
    )
    .await;
    // Close the endpoint gracefully on every exit path; otherwise iroh logs
    // "Endpoint dropped without calling `Endpoint::close`. Aborting
    // ungracefully." as the process tears down.
    endpoint.close().await;
    res
}

async fn run(
    endpoint_id: EndpointId,
    direct_addresses: Vec<SocketAddr>,
    relay_url: Option<RelayUrl>,
    endpoint: Endpoint,
    test: bool,
) -> anyhow::Result<()> {
    tracing::info!("dialing {:?}", endpoint_id);
    let mut endpoint_addr = EndpointAddr::new(endpoint_id);
    if let Some(relay_url) = relay_url {
        endpoint_addr = endpoint_addr.with_relay_url(relay_url);
    }
    for ip_addr in direct_addresses {
        endpoint_addr = endpoint_addr.with_ip_addr(ip_addr);
    }

    let (alpn, alpn_label) = if test {
        (iroh_doctor_core::doctor::ALPN, "doctor test")
    } else {
        (iroh_doctor_core::probe::ALPN, "monitor")
    };

    eprintln!("dialing {endpoint_id} ({alpn_label})...");
    let dial = tokio::time::timeout(
        Duration::from_secs(30),
        endpoint.connect(endpoint_addr, alpn),
    )
    .await;
    let connection = match dial {
        Ok(Ok(c)) => c,
        Ok(Err(cause)) => {
            eprintln!("unable to connect to {endpoint_id}: {cause:#}");
            return Ok(());
        }
        Err(_) => {
            eprintln!(
                "timed out dialing {endpoint_id} after 30s.\n\
                 Try passing --relay-url and/or --remote-endpoint so the peer is reachable."
            );
            return Ok(());
        }
    };
    eprintln!("connected; starting {alpn_label}...");

    let gui = Gui::new(endpoint, endpoint_id);
    let close_reason = connection
        .close_reason()
        .map(|e| format!(" (reason: {e})"))
        .unwrap_or_default();

    if test {
        log_connection_changes(gui.mp.clone(), endpoint_id, connection.clone());
        if let Err(cause) = passive_side(gui, &connection).await {
            eprintln!("error handling connection: {cause:#}{close_reason}");
        } else {
            eprintln!("Connection closed{close_reason}");
        }
    } else if let Err(cause) = monitor(&gui, endpoint_id, &connection).await {
        eprintln!("error monitoring connection: {cause:#}{close_reason}");
    } else {
        eprintln!("Connection closed{close_reason}");
    }

    Ok(())
}

/// Number of recent RTT samples kept for the sparkline and running stats.
const HISTORY_LEN: usize = 60;

#[derive(Copy, Clone)]
enum StateKind {
    Direct,
    Relay,
    Custom,
    Unknown,
}

/// A small dashboard of in-place progress lines for the live monitor. Each
/// line is an `indicatif` progress bar whose message we rewrite as new data
/// arrives, so nothing scrolls past.
#[derive(Clone)]
struct MonitorView {
    state_pb: ProgressBar,
    paths_pb: ProgressBar,
    latency_pb: ProgressBar,
    spark_pb: ProgressBar,
    throughput_pb: ProgressBar,
    ttfdb_pb: ProgressBar,
}

impl MonitorView {
    fn new(mp: &MultiProgress, peer: EndpointId) -> Self {
        let template = ProgressStyle::default_bar().template("{msg}").unwrap();
        let make = || {
            let pb = mp.add(ProgressBar::hidden());
            pb.set_style(template.clone());
            pb.enable_steady_tick(Duration::from_millis(250));
            pb.set_message("");
            pb
        };
        let header = make();
        let state_pb = make();
        let paths_pb = make();
        let ttfdb_pb = make();
        let latency_pb = make();
        let spark_pb = make();
        let throughput_pb = make();

        let peer_short: String = format!("{peer:#}").chars().take(16).collect();
        header.set_message(format!(
            "{} {}",
            style("monitoring").bold().cyan(),
            style(peer_short).dim()
        ));
        state_pb.set_message(format!(
            "{}        {}",
            style("state:").dim(),
            style("connecting").yellow()
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

    fn set_state(&self, label: &str, kind: StateKind) {
        let painted = match kind {
            StateKind::Direct => style(label).bold().green(),
            StateKind::Relay => style(label).bold().yellow(),
            StateKind::Custom => style(label).bold().magenta(),
            StateKind::Unknown => style(label).dim(),
        };
        self.state_pb
            .set_message(format!("{}        {painted}", style("state:").dim()));
    }

    fn set_paths(&self, lines: Vec<String>) {
        let body = if lines.is_empty() {
            style("(no paths)").dim().to_string()
        } else {
            lines.join("\n  ")
        };
        self.paths_pb
            .set_message(format!("{}\n  {body}", style("paths:").dim()));
    }

    fn set_ttfdb(&self, elapsed: Duration) {
        let ms = elapsed.as_secs_f64() * 1000.0;
        self.ttfdb_pb.set_message(format!(
            "{}        {}",
            style("ttfdb:").dim(),
            style(format!("{ms:.0} ms")).bold()
        ));
    }

    fn set_latency(&self, latest: Duration, history: &VecDeque<Duration>) {
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

    fn set_throughput(&self, bytes: u64, elapsed: Duration) {
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

    fn set_probe_ended(&self, kind: &str, cause: impl std::fmt::Display) {
        self.latency_pb.set_message(format!(
            "{}      {}",
            style("latency:").dim(),
            style(format!("{kind} ended: {cause}")).red()
        ));
    }
}

fn stats(history: &VecDeque<Duration>) -> (Duration, Duration, Duration, u64) {
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

fn sparkline(samples: &VecDeque<Duration>) -> String {
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

/// Watches the connection's path stream and updates the dashboard's
/// state and paths lines as paths come and go. TTFDB is handled by a
/// separate spawn that calls [`iroh_doctor_core::monitor::ttfdb_watch`].
fn spawn_paths_watcher(
    connection: iroh::endpoint::Connection,
    view: MonitorView,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut paths = connection.paths_stream();
        while let Some(path_list) = paths.next().await {
            let (label, kind) = match path_list.iter().find(|p| p.is_selected()) {
                Some(p) if p.remote_addr().is_ip() => ("direct", StateKind::Direct),
                Some(p) if p.remote_addr().is_relay() => ("relay", StateKind::Relay),
                Some(_) => ("custom", StateKind::Custom),
                None => ("no path", StateKind::Unknown),
            };
            view.set_state(label, kind);

            let lines: Vec<String> = path_list
                .iter()
                .map(|p| {
                    let sel = if p.is_selected() { '*' } else { ' ' };
                    let kind_str = if p.remote_addr().is_ip() {
                        "direct"
                    } else if p.remote_addr().is_relay() {
                        "relay "
                    } else {
                        "custom"
                    };
                    let rtt_ms = p.rtt().as_secs_f64() * 1000.0;
                    format!(
                        "{sel} {kind_str}  {:<44}  rtt {rtt_ms:>6.1} ms",
                        p.remote_addr().to_string(),
                    )
                })
                .collect();
            view.set_paths(lines);
        }
    })
}

/// Runs the live connection monitor: maintains a dashboard via the
/// existing `Gui`'s `MultiProgress`, repeatedly pings the peer's probe
/// responder for latency-over-time, and periodically uploads to measure
/// throughput. Returns `Ok(())` cleanly once the peer goes away.
async fn monitor(
    gui: &Gui,
    endpoint_id: EndpointId,
    connection: &iroh::endpoint::Connection,
) -> anyhow::Result<()> {
    let view = MonitorView::new(&gui.mp, endpoint_id);
    let started = Instant::now();
    let _watcher = spawn_paths_watcher(connection.clone(), view.clone());
    // Time-to-first-direct-byte is computed by core so both the cli and the
    // app report the same number for the same physical holepunch.
    {
        let conn = connection.clone();
        let view = view.clone();
        tokio::spawn(async move {
            if let Some(elapsed) = iroh_doctor_core::monitor::ttfdb_watch(&conn, started).await {
                view.set_ttfdb(elapsed);
            }
        });
    }

    let mut client =
        match tokio::time::timeout(Duration::from_secs(10), ProbeClient::connect(connection)).await
        {
            Ok(Ok(c)) => c,
            Ok(Err(cause)) => {
                view.set_probe_ended("setup", cause);
                return Ok(());
            }
            Err(_) => {
                view.set_probe_ended("setup", "timed out opening probe stream after 10s");
                return Ok(());
            }
        };

    let mut nonce: u32 = 0;
    let mut history: VecDeque<Duration> = VecDeque::with_capacity(HISTORY_LEN);

    loop {
        match client.ping(nonce).await {
            Ok(rtt) => {
                if history.len() == HISTORY_LEN {
                    history.pop_front();
                }
                history.push_back(rtt);
                view.set_latency(rtt, &history);
            }
            Err(cause) => {
                view.set_probe_ended("latency", cause);
                break;
            }
        }

        // Every tenth tick, starting at the first, so the user gets an
        // immediate throughput sample and then one roughly every 10s.
        if nonce.is_multiple_of(10) {
            const UPLOAD_BYTES: u64 = 1024 * 1024;
            match client.upload(UPLOAD_BYTES).await {
                Ok(elapsed) => view.set_throughput(UPLOAD_BYTES, elapsed),
                Err(cause) => {
                    view.set_probe_ended("throughput", cause);
                    break;
                }
            }
        }

        nonce = nonce.wrapping_add(1);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    Ok(())
}
