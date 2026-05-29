use std::collections::VecDeque;
use std::time::Duration;

use dioxus::prelude::*;
use tokio::sync::oneshot;

use crate::identity;
use crate::peer::{
    ConnectionState, DiagnosticsReport, NetReportSummary, PathInfo, PathKind, PeerCommand,
    TelemetryState, ThroughputSnapshot,
};
use crate::portmap_probe::PortMapProbeResult;
use crate::relay_probe::RelayProbeResult;
use crate::PeerHandle;

#[derive(Clone)]
pub enum DiagState<T: Clone + 'static> {
    Idle,
    Running,
    Ok(T),
    Err(String),
}

/// One row in the connection-event log shown on the Diagnostics tab. `elapsed` is
/// measured from session start; the UI formats it as `+MM:SS`.
#[derive(Clone)]
pub struct EventEntry {
    pub elapsed: Duration,
    pub label: String,
    /// Mirrors the status-indicator's data-state attribute (`idle`,
    /// `pending`, `ready`, `connected`, `error`).
    pub kind: String,
}

pub fn trigger_pings(
    cmd_handle: Signal<Option<PeerHandle>>,
    mut services_state: Signal<DiagState<Duration>>,
) {
    services_state.set(DiagState::Running);
    let handle = cmd_handle.read().clone();
    spawn(async move {
        let result = run_ping_services(handle).await;
        services_state.set(into_state(result));
    });
}

pub fn trigger_net_diagnostics(
    cmd_handle: Signal<Option<PeerHandle>>,
    mut net_state: Signal<DiagState<DiagnosticsReport>>,
) {
    net_state.set(DiagState::Running);
    let handle = cmd_handle.read().clone();
    spawn(async move {
        let result = run_net(handle).await;
        net_state.set(into_state(result));
    });
}

pub fn trigger_probe_net_report(
    cmd_handle: Signal<Option<PeerHandle>>,
    mut net_report_state: Signal<DiagState<NetReportSummary>>,
) {
    net_report_state.set(DiagState::Running);
    let handle = cmd_handle.read().clone();
    spawn(async move {
        let result = run_probe_net_report(handle).await;
        net_report_state.set(into_state(result));
    });
}

pub fn trigger_probe_relays(
    cmd_handle: Signal<Option<PeerHandle>>,
    mut relays_state: Signal<DiagState<Vec<RelayProbeResult>>>,
) {
    relays_state.set(DiagState::Running);
    let handle = cmd_handle.read().clone();
    spawn(async move {
        let result = run_probe_relays(handle).await;
        relays_state.set(into_state(result));
    });
}

pub fn trigger_probe_portmap(
    cmd_handle: Signal<Option<PeerHandle>>,
    mut portmap_state: Signal<DiagState<PortMapProbeResult>>,
) {
    portmap_state.set(DiagState::Running);
    let handle = cmd_handle.read().clone();
    spawn(async move {
        let result = run_probe_portmap(handle).await;
        portmap_state.set(into_state(result));
    });
}

#[component]
pub fn DiagnosticsView(
    cmd_handle: Signal<Option<PeerHandle>>,
    conn_state: Signal<ConnectionState>,
    telemetry: Signal<TelemetryState>,
    services_state: Signal<DiagState<Duration>>,
    net_state: Signal<DiagState<DiagnosticsReport>>,
    net_report_state: Signal<DiagState<NetReportSummary>>,
    relays_state: Signal<DiagState<Vec<RelayProbeResult>>>,
    portmap_state: Signal<DiagState<PortMapProbeResult>>,
    paths: Signal<Vec<PathInfo>>,
    rtt_history: Signal<VecDeque<f64>>,
    event_log: Signal<VecDeque<EventEntry>>,
    ttfdb: Signal<Option<Duration>>,
    throughput: Signal<Option<ThroughputSnapshot>>,
) -> Element {
    let busy = matches!(services_state(), DiagState::Running)
        || matches!(net_state(), DiagState::Running)
        || matches!(net_report_state(), DiagState::Running)
        || matches!(relays_state(), DiagState::Running)
        || matches!(portmap_state(), DiagState::Running);

    let conn_state_value = conn_state();
    let connection_state = derive_connection_state(&conn_state_value, &paths());
    let connected = matches!(
        connection_state,
        ConnectionStateLabel::Relay | ConnectionStateLabel::Direct | ConnectionStateLabel::Custom
    );
    let ttfdb_value = ttfdb();
    let throughput_value = throughput();

    rsx! {
        div { class: "diagnostics",
            ConnectionStateHeader {
                state: connection_state,
                ttfdb: ttfdb_value,
                throughput: throughput_value,
            }

            // Live connection detail: only meaningful while a peer is
            // connected, since it reflects the active QUIC paths.
            if connected {
                LiveLatency { paths, rtt_history }

                PathsTable { paths }

                EventLog { event_log }
            }

            // Peer-independent diagnostics. These probe the local
            // endpoint, the relays, and iroh-services rather than the
            // remote peer, so they stay available even when disconnected
            // (mirrors `iroh-doctor report`).
            section { class: "settings-section",
                div { class: "section-head",
                    label { class: "label", "Local network report" }
                    button {
                        class: "btn btn-primary",
                        disabled: busy,
                        onclick: move |_| {
                            trigger_pings(cmd_handle, services_state);
                            trigger_net_diagnostics(cmd_handle, net_state);
                            trigger_probe_net_report(cmd_handle, net_report_state);
                            trigger_probe_relays(cmd_handle, relays_state);
                            trigger_probe_portmap(cmd_handle, portmap_state);
                        },
                        "Refresh"
                    }
                }
                dl { class: "diag-table",
                    {render_net_report_rows(&net_report_state())}
                }
            }

            section { class: "settings-section",
                label { class: "label", "Direct port-map probe" }
                dl { class: "diag-table",
                    {render_portmap_rows(&portmap_state())}
                }
            }

            RelayLatencyPanel { relays_state }

            section { class: "settings-section",
                label { class: "label", "Services diagnostics" }
                dl { class: "diag-table",
                    dt { "ping" }
                    dd { {render_rtt(&services_state())} }
                    {render_net_rows(&net_state())}
                }
            }

            IrohServicesSection { cmd_handle, telemetry }
        }
    }
}

/// High-level connection state shown at the top of the Diagnostics tab.
/// Distinguishes the in-progress states (`Connecting`, `Error`) from
/// idle and from the connected branches so a user clicking Connect from
/// any tab sees immediate feedback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStateLabel {
    Disconnected,
    Connecting,
    Relay,
    Direct,
    /// Custom transport selected (e.g., BLE / Tor when those wire in).
    Custom,
    Error,
}

/// Picks a [`ConnectionStateLabel`] from the underlying state machine
/// plus the live path snapshot.
///
/// The path snapshot wins when a path is actually selected, because the
/// kind (relay vs direct) is more informative than just "connected".
/// Otherwise the underlying `ConnectionState` decides, so a freshly-
/// fired Connect command surfaces as `Connecting` instead of looking
/// like nothing happened.
fn derive_connection_state(
    conn_state: &ConnectionState,
    paths: &[PathInfo],
) -> ConnectionStateLabel {
    if let Some(p) = paths.iter().find(|p| p.selected) {
        return match p.kind {
            PathKind::Ip => ConnectionStateLabel::Direct,
            PathKind::Relay => ConnectionStateLabel::Relay,
            PathKind::Custom => ConnectionStateLabel::Custom,
        };
    }
    match conn_state {
        ConnectionState::Connecting => ConnectionStateLabel::Connecting,
        ConnectionState::Error(_) => ConnectionStateLabel::Error,
        ConnectionState::Connected { .. } => ConnectionStateLabel::Connecting,
        _ => ConnectionStateLabel::Disconnected,
    }
}

#[component]
fn ConnectionStateHeader(
    state: ConnectionStateLabel,
    ttfdb: Option<Duration>,
    throughput: Option<ThroughputSnapshot>,
) -> Element {
    let (label, kind_class) = match state {
        ConnectionStateLabel::Disconnected => ("disconnected", "disconnected"),
        ConnectionStateLabel::Connecting => ("connecting...", "connecting"),
        ConnectionStateLabel::Relay => ("relay", "relay"),
        ConnectionStateLabel::Direct => ("direct", "direct"),
        ConnectionStateLabel::Custom => ("custom", "custom"),
        ConnectionStateLabel::Error => ("error", "error"),
    };
    // Throughput only makes sense against a live peer, so it rides with the
    // rest of the live connection detail (latency, paths) and is removed in
    // every disconnected state.
    let connected = matches!(
        state,
        ConnectionStateLabel::Relay | ConnectionStateLabel::Direct | ConnectionStateLabel::Custom
    );
    rsx! {
        section { class: "connection-state-header",
            div { class: "connection-state-label connection-state-{kind_class}", "{label}" }
            {render_ttfdb(ttfdb)}
            if connected {
                {render_throughput(throughput.as_ref())}
            }
        }
    }
}

fn render_ttfdb(ttfdb: Option<Duration>) -> Element {
    let Some(d) = ttfdb else {
        return rsx! {};
    };
    let formatted = format_ttfdb(d);
    rsx! {
        div { class: "ttfdb-value",
            span { class: "ttfdb-label", "time to first direct byte: " }
            span { class: "ttfdb-number mono", "{formatted}" }
        }
    }
}

/// Renders the most recent peer-probe upload as a throughput readout next
/// to the TTFDB line. We show "-" when no upload has been observed yet so
/// the row has a stable layout the moment a probe peer connects.
fn render_throughput(throughput: Option<&ThroughputSnapshot>) -> Element {
    let Some(t) = throughput else {
        return rsx! {
            div { class: "ttfdb-value",
                span { class: "ttfdb-label", "throughput: " }
                span { class: "ttfdb-number mono", "-" }
            }
        };
    };
    let formatted = format_throughput(t);
    rsx! {
        div { class: "ttfdb-value",
            span { class: "ttfdb-label", "throughput: " }
            span { class: "ttfdb-number mono", "{formatted}" }
        }
    }
}

/// Compact "{mbps} Mbps ({MiB} MiB in {ms} ms)" rendering for the
/// Diagnostics throughput row. Falls back to "- Mbps" when elapsed was
/// zero (the responder doesn't compute Mbps in that case).
fn format_throughput(t: &ThroughputSnapshot) -> String {
    let mib = t.bytes as f64 / 1024.0 / 1024.0;
    let ms = t.elapsed.as_secs_f64() * 1000.0;
    match t.mbps {
        Some(m) => format!("{m:.1} Mbps ({mib:.2} MiB in {ms:.0} ms)"),
        None => format!("- Mbps ({mib:.2} MiB in {ms:.0} ms)"),
    }
}

/// Renders a TTFDB duration as a compact human string: milliseconds
/// below one second, seconds with one decimal up to a minute, then
/// `M:SS.S` past that. Matches the rest of the Diagnostics tab's
/// preference for unit-suffixed numeric output.
fn format_ttfdb(d: Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else if d.as_secs() < 60 {
        format!("{:.1} s", d.as_secs_f64())
    } else {
        let total = d.as_secs_f64();
        let minutes = (total / 60.0).floor() as u64;
        let seconds = total - (minutes as f64) * 60.0;
        format!("{minutes}:{seconds:04.1}")
    }
}

#[component]
fn RelayLatencyPanel(relays_state: Signal<DiagState<Vec<RelayProbeResult>>>) -> Element {
    let state = relays_state();
    rsx! {
        section { class: "settings-section",
            label { class: "label", "Relay latency" }
            {render_relay_rows(&state)}
        }
    }
}

#[component]
fn IrohServicesSection(
    cmd_handle: Signal<Option<PeerHandle>>,
    telemetry: Signal<TelemetryState>,
) -> Element {
    let initial = identity::load_api_secret_override();
    let api_secret_input = use_signal(|| initial.clone());
    let saved_override = use_signal(|| initial);

    let telemetry_text = telemetry_line(&telemetry());
    let using_default = saved_override().is_empty();

    let footer = if using_default {
        "Using the bundled default key. Paste a secret from services.iroh.computer to override it; stored locally on this device only."
    } else {
        "Using a custom key. Tap Clear to revert to the bundled default."
    };

    let input_value = api_secret_input();
    let trimmed_input = input_value.trim().to_string();
    let save_disabled = trimmed_input.is_empty() || trimmed_input == saved_override();
    let clear_disabled = saved_override().is_empty() && input_value.is_empty();

    rsx! {
        section { class: "settings-section",
            label { class: "label", "iroh services API key" }
            input {
                class: "api-input",
                r#type: "password",
                placeholder: "services1...",
                value: "{input_value}",
                autocapitalize: "off",
                autocorrect: "off",
                spellcheck: "false",
                oninput: move |evt| { api_secret_input.clone().set(evt.value()); },
            }
            div { class: "settings-actions",
                button {
                    class: "btn btn-primary",
                    disabled: save_disabled,
                    onclick: move |_| {
                        let value = api_secret_input();
                        let trimmed = value.trim().to_string();
                        if identity::save_api_secret_override(&trimmed).is_ok() {
                            saved_override.clone().set(trimmed.clone());
                        }
                        if let Some(handle) = cmd_handle.read().clone() {
                            let _ = handle.tx.try_send(PeerCommand::SaveApiSecret {
                                secret: trimmed,
                            });
                        }
                    },
                    "Save"
                }
                button {
                    class: "btn",
                    disabled: clear_disabled,
                    onclick: move |_| {
                        api_secret_input.clone().set(String::new());
                        let _ = identity::save_api_secret_override("");
                        saved_override.clone().set(String::new());
                        if let Some(handle) = cmd_handle.read().clone() {
                            let _ = handle.tx.try_send(PeerCommand::SaveApiSecret {
                                secret: String::new(),
                            });
                        }
                    },
                    "Clear"
                }
            }
            div { class: "footer-note", "{footer}" }
        }

        section { class: "settings-section",
            label { class: "label", "Telemetry status" }
            div { class: "telemetry-status", "{telemetry_text}" }
        }
    }
}

fn telemetry_line(state: &TelemetryState) -> String {
    match state {
        TelemetryState::Off => "off - paste an API secret to enable".into(),
        TelemetryState::Starting => "connecting...".into(),
        TelemetryState::Active { name } => format!("active - pushing as {name}"),
        TelemetryState::Error(msg) => format!("error: {msg}"),
    }
}

#[component]
fn LiveLatency(paths: Signal<Vec<PathInfo>>, rtt_history: Signal<VecDeque<f64>>) -> Element {
    let snapshot = paths();
    let history = rtt_history();
    let selected_rtt = snapshot.iter().find(|p| p.selected).map(|p| p.rtt_ms);

    let header = match selected_rtt {
        Some(rtt) => rsx! { span { class: "diag-ok", "{rtt:.1} ms" } },
        None => rsx! { span { class: "diag-idle", "no active path" } },
    };

    let (min, max, avg) = summarize_history(&history);
    let sparkline = render_sparkline(&history);

    rsx! {
        section { class: "settings-section",
            label { class: "label", "Live latency" }
            div { class: "latency-current",
                span { class: "mono latency-now", {header} }
                {sparkline}
            }
            div { class: "latency-summary",
                span { class: "label", "min" } span { class: "mono", {format_opt_ms(min)} }
                span { class: "label", "avg" } span { class: "mono", {format_opt_ms(avg)} }
                span { class: "label", "max" } span { class: "mono", {format_opt_ms(max)} }
                span { class: "label", "samples" } span { class: "mono", "{history.len()}" }
            }
        }
    }
}

#[component]
fn PathsTable(paths: Signal<Vec<PathInfo>>) -> Element {
    let snapshot = paths();
    if snapshot.is_empty() {
        return rsx! {
            section { class: "settings-section",
                label { class: "label", "Paths" }
                div { class: "diag-idle", "none - connect a peer to see paths populate" }
            }
        };
    }
    rsx! {
        section { class: "settings-section",
            label { class: "label", "Paths ({snapshot.len()})" }
            table { class: "transports-table",
                thead {
                    tr {
                        th { "Kind" }
                        th { "Address" }
                        th { "Sel" }
                        th { class: "ping-rtt-col", "RTT" }
                    }
                }
                tbody {
                    for p in snapshot.iter() {
                        tr {
                            td {
                                span { class: "transport-kind transport-kind-{kind_class(p.kind)}", {kind_label(p.kind)} }
                            }
                            td { class: "mono transports-addr", title: "{p.addr}", "{p.addr}" }
                            td { class: "paths-sel", if p.selected { "*" } else { "" } }
                            td { class: "ping-rtt-col mono", "{p.rtt_ms:.1} ms" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn EventLog(event_log: Signal<VecDeque<EventEntry>>) -> Element {
    let log = event_log();
    rsx! {
        section { class: "settings-section",
            label { class: "label", "Connection events" }
            if log.is_empty() {
                div { class: "diag-idle", "no events yet" }
            } else {
                ul { class: "event-log",
                    for entry in log.iter().rev() {
                        li { class: "event-row", "data-kind": "{entry.kind}",
                            span { class: "mono event-time", {format_elapsed(entry.elapsed)} }
                            span { class: "event-label", "{entry.label}" }
                        }
                    }
                }
            }
        }
    }
}

fn summarize_history(history: &VecDeque<f64>) -> (Option<f64>, Option<f64>, Option<f64>) {
    if history.is_empty() {
        return (None, None, None);
    }
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut sum = 0.0;
    for &v in history.iter() {
        if v < min {
            min = v;
        }
        if v > max {
            max = v;
        }
        sum += v;
    }
    let avg = sum / history.len() as f64;
    (Some(min), Some(max), Some(avg))
}

fn format_opt_ms(value: Option<f64>) -> String {
    match value {
        Some(v) => format!("{v:.1} ms"),
        None => "-".to_string(),
    }
}

fn format_elapsed(elapsed: Duration) -> String {
    let total_secs = elapsed.as_secs();
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    if h > 0 {
        format!("+{h}:{m:02}:{s:02}")
    } else {
        format!("+{m:02}:{s:02}")
    }
}

fn kind_label(kind: PathKind) -> &'static str {
    match kind {
        PathKind::Ip => "QUIC / IP",
        PathKind::Relay => "Relay",
        PathKind::Custom => "Custom",
    }
}

fn kind_class(kind: PathKind) -> &'static str {
    match kind {
        PathKind::Ip => "ip",
        PathKind::Relay => "relay",
        PathKind::Custom => "custom",
    }
}

/// Renders the rolling RTT history as an inline SVG sparkline. Returns an
/// empty placeholder until we have at least two samples to interpolate.
fn render_sparkline(history: &VecDeque<f64>) -> Element {
    if history.len() < 2 {
        return rsx! { span { class: "diag-idle latency-spark-empty", "collecting..." } };
    }
    const WIDTH: f64 = 180.0;
    const HEIGHT: f64 = 36.0;
    let max = history.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let min = history.iter().copied().fold(f64::INFINITY, f64::min);
    let range = (max - min).max(1.0);
    let step = if history.len() > 1 {
        WIDTH / (history.len() - 1) as f64
    } else {
        0.0
    };
    let mut path = String::with_capacity(history.len() * 16);
    for (i, &v) in history.iter().enumerate() {
        let x = i as f64 * step;
        let y = HEIGHT - ((v - min) / range) * HEIGHT;
        if i == 0 {
            path.push_str(&format!("M{x:.1},{y:.1}"));
        } else {
            path.push_str(&format!(" L{x:.1},{y:.1}"));
        }
    }
    let viewbox = format!("0 0 {WIDTH:.0} {HEIGHT:.0}");
    rsx! {
        svg {
            class: "latency-spark",
            view_box: "{viewbox}",
            width: "{WIDTH}",
            height: "{HEIGHT}",
            preserve_aspect_ratio: "none",
            path { d: "{path}", fill: "none", stroke: "currentColor", stroke_width: "1.5" }
        }
    }
}

fn into_state<T: Clone + 'static>(r: Result<T, String>) -> DiagState<T> {
    match r {
        Ok(v) => DiagState::Ok(v),
        Err(e) => DiagState::Err(e),
    }
}

async fn run_ping_services(handle: Option<PeerHandle>) -> Result<Duration, String> {
    let Some(h) = handle else {
        return Err("not ready".into());
    };
    let (tx, rx) = oneshot::channel();
    if h.tx
        .try_send(PeerCommand::PingServices { reply: tx })
        .is_err()
    {
        return Err("queue full".into());
    }
    rx.await.unwrap_or_else(|_| Err("reply dropped".into()))
}

async fn run_net(handle: Option<PeerHandle>) -> Result<DiagnosticsReport, String> {
    let Some(h) = handle else {
        return Err("not ready".into());
    };
    let (tx, rx) = oneshot::channel();
    if h.tx
        .try_send(PeerCommand::RunNetDiagnostics { reply: tx })
        .is_err()
    {
        return Err("queue full".into());
    }
    rx.await.unwrap_or_else(|_| Err("reply dropped".into()))
}

async fn run_probe_net_report(handle: Option<PeerHandle>) -> Result<NetReportSummary, String> {
    let Some(h) = handle else {
        return Err("not ready".into());
    };
    let (tx, rx) = oneshot::channel();
    if h.tx
        .try_send(PeerCommand::ProbeNetReport { reply: tx })
        .is_err()
    {
        return Err("queue full".into());
    }
    rx.await.unwrap_or_else(|_| Err("reply dropped".into()))
}

async fn run_probe_relays(handle: Option<PeerHandle>) -> Result<Vec<RelayProbeResult>, String> {
    let Some(h) = handle else {
        return Err("not ready".into());
    };
    let (tx, rx) = oneshot::channel();
    if h.tx
        .try_send(PeerCommand::ProbeRelayLatencies { reply: tx })
        .is_err()
    {
        return Err("queue full".into());
    }
    rx.await.unwrap_or_else(|_| Err("reply dropped".into()))
}

async fn run_probe_portmap(handle: Option<PeerHandle>) -> Result<PortMapProbeResult, String> {
    let Some(h) = handle else {
        return Err("not ready".into());
    };
    let (tx, rx) = oneshot::channel();
    if h.tx
        .try_send(PeerCommand::ProbePortMap { reply: tx })
        .is_err()
    {
        return Err("queue full".into());
    }
    rx.await.unwrap_or_else(|_| Err("reply dropped".into()))
}

fn render_rtt(state: &DiagState<Duration>) -> Element {
    match state {
        DiagState::Idle => rsx! { span { class: "diag-idle", "-" } },
        DiagState::Running => rsx! { span { class: "diag-running", "..." } },
        DiagState::Ok(d) => {
            let ms = d.as_secs_f64() * 1000.0;
            rsx! { span { class: "diag-ok", "{ms:.1} ms" } }
        }
        DiagState::Err(e) => rsx! { span { class: "diag-err", title: "{e}", "-" } },
    }
}

fn render_net_rows(state: &DiagState<DiagnosticsReport>) -> Element {
    match state {
        DiagState::Idle => rsx! {
            dt { "status" }
            dd { class: "diag-idle", "not run yet" }
        },
        DiagState::Running => rsx! {
            dt { "status" }
            dd { class: "diag-running", "running diagnostics..." }
        },
        DiagState::Err(e) => rsx! {
            dt { "status" }
            dd { class: "diag-err", "error: {e}" }
        },
        DiagState::Ok(r) => rsx! {
            // Endpoint id is already shown in the page header, so it is
            // not repeated here.
            dt { "direct addrs" }
            dd {
                if r.direct_addrs.is_empty() {
                    "(none)"
                } else {
                    ul { class: "mono addr-list",
                        for addr in r.direct_addrs.iter() {
                            li { "{addr}" }
                        }
                    }
                }
            }

            dt { "iroh version" }
            dd { "{r.iroh_version}" }

            dt { "iroh-services version" }
            dd { "{r.iroh_services_version}" }

            dt { "net report" }
            dd { if r.has_net_report { "available" } else { "not available" } }

            dt { "UPnP" }
            dd { "{tribool(r.upnp)}" }

            dt { "PCP" }
            dd { "{tribool(r.pcp)}" }

            dt { "NAT-PMP" }
            dd { "{tribool(r.nat_pmp)}" }
        },
    }
}

fn tribool(b: Option<bool>) -> &'static str {
    match b {
        Some(true) => "yes",
        Some(false) => "no",
        None => "(not probed)",
    }
}

fn render_net_report_rows(state: &DiagState<NetReportSummary>) -> Element {
    match state {
        DiagState::Idle => rsx! {
            dt { "status" }
            dd { class: "diag-idle", "not run yet" }
        },
        DiagState::Running => rsx! {
            dt { "status" }
            dd { class: "diag-running", "probing..." }
        },
        DiagState::Err(e) => rsx! {
            dt { "status" }
            dd { class: "diag-err", "error: {e}" }
        },
        DiagState::Ok(r) => {
            let nat_class = format!("nat-pill nat-{}", nat_kind_class(r.nat));
            let nat_label = r.nat.to_string();
            let nat_desc = r.nat.description();
            let global_v4 = r
                .global_v4
                .clone()
                .unwrap_or_else(|| "(not observed)".into());
            let global_v6 = r
                .global_v6
                .clone()
                .unwrap_or_else(|| "(not observed)".into());
            let preferred = r.preferred_relay.clone().unwrap_or_else(|| "(none)".into());
            rsx! {
                dt { "NAT type" }
                dd {
                    span { class: "{nat_class}", "{nat_label}" }
                    span { class: "nat-desc", " - {nat_desc}" }
                }
                dt { "UDP IPv4" }
                dd { if r.udp_v4 { "reachable" } else { "not reachable" } }
                dt { "UDP IPv6" }
                dd { if r.udp_v6 { "reachable" } else { "not reachable" } }
                dt { "global IPv4" }
                dd { class: "mono", "{global_v4}" }
                dt { "global IPv6" }
                dd { class: "mono", "{global_v6}" }
                dt { "mapping varies (IPv4)" }
                dd { "{tribool(r.mapping_varies_v4)}" }
                dt { "mapping varies (IPv6)" }
                dd { "{tribool(r.mapping_varies_v6)}" }
                dt { "captive portal" }
                dd { "{tribool(r.captive_portal)}" }
                dt { "preferred relay" }
                dd { class: "mono", "{preferred}" }
                dt { "relays seen" }
                dd { "{r.relays_seen}" }
            }
        }
    }
}

fn render_relay_rows(state: &DiagState<Vec<RelayProbeResult>>) -> Element {
    match state {
        DiagState::Idle => rsx! {
            div { class: "diag-idle", "not probed yet - hit Refresh" }
        },
        DiagState::Running => rsx! {
            div { class: "diag-running", "probing relays..." }
        },
        DiagState::Err(e) => rsx! {
            div { class: "diag-err", "error: {e}" }
        },
        DiagState::Ok(rows) => {
            if rows.is_empty() {
                return rsx! {
                    div { class: "diag-idle", "no relays returned" }
                };
            }
            rsx! {
                table { class: "transports-table",
                    thead {
                        tr {
                            th { "Relay" }
                            th { class: "ping-rtt-col", "Connect" }
                            th { class: "ping-rtt-col", "Ping" }
                            th { "Status" }
                        }
                    }
                    tbody {
                        for r in rows.iter() {
                            tr {
                                td { class: "mono transports-addr", title: "{r.url}", "{r.url}" }
                                td { class: "ping-rtt-col mono", {render_opt_ms(r.connect_ms)} }
                                td { class: "ping-rtt-col mono", {render_opt_ms(r.ping_ms)} }
                                td {
                                    {render_relay_status(r)}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn render_opt_ms(ms: Option<f64>) -> Element {
    match ms {
        Some(v) => rsx! { span { class: "diag-ok", "{v:.1} ms" } },
        None => rsx! { span { class: "diag-idle", "-" } },
    }
}

fn render_relay_status(row: &RelayProbeResult) -> Element {
    match &row.error {
        None => rsx! { span { class: "diag-ok", "ok" } },
        Some(msg) => rsx! { span { class: "diag-err", title: "{msg}", "{msg}" } },
    }
}

fn render_portmap_rows(state: &DiagState<PortMapProbeResult>) -> Element {
    match state {
        DiagState::Idle => rsx! {
            dt { "status" }
            dd { class: "diag-idle", "not probed yet" }
        },
        DiagState::Running => rsx! {
            dt { "status" }
            dd { class: "diag-running", "probing..." }
        },
        DiagState::Err(e) => rsx! {
            dt { "status" }
            dd { class: "diag-err", "error: {e}" }
        },
        DiagState::Ok(r) => {
            let mut rows = rsx! {
                dt { "UPnP" }
                dd { "{tribool(r.upnp)}" }
                dt { "PCP" }
                dd { "{tribool(r.pcp)}" }
                dt { "NAT-PMP" }
                dd { "{tribool(r.nat_pmp)}" }
            };
            if let Some(err) = &r.error {
                rows = rsx! {
                    {rows}
                    dt { "warning" }
                    dd { class: "diag-err", "{err}" }
                };
            }
            rows
        }
    }
}

fn nat_kind_class(nat: iroh_doctor_core::nat::NatType) -> &'static str {
    use iroh_doctor_core::nat::NatType;
    match nat {
        NatType::Easy => "easy",
        NatType::Medium => "medium",
        NatType::Hard => "hard",
        NatType::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_empty_history() {
        let h: VecDeque<f64> = VecDeque::new();
        assert_eq!(summarize_history(&h), (None, None, None));
    }

    #[test]
    fn summarize_single_sample() {
        let mut h = VecDeque::new();
        h.push_back(12.5);
        let (min, max, avg) = summarize_history(&h);
        assert_eq!(min, Some(12.5));
        assert_eq!(max, Some(12.5));
        assert_eq!(avg, Some(12.5));
    }

    #[test]
    fn summarize_multi_sample() {
        let h: VecDeque<f64> = [10.0, 20.0, 30.0].into_iter().collect();
        let (min, max, avg) = summarize_history(&h);
        assert_eq!(min, Some(10.0));
        assert_eq!(max, Some(30.0));
        assert_eq!(avg, Some(20.0));
    }

    #[test]
    fn format_elapsed_below_one_hour() {
        assert_eq!(format_elapsed(Duration::from_secs(0)), "+00:00");
        assert_eq!(format_elapsed(Duration::from_secs(5)), "+00:05");
        assert_eq!(format_elapsed(Duration::from_secs(65)), "+01:05");
        assert_eq!(format_elapsed(Duration::from_secs(3599)), "+59:59");
    }

    #[test]
    fn format_elapsed_above_one_hour() {
        assert_eq!(format_elapsed(Duration::from_secs(3600)), "+1:00:00");
        assert_eq!(
            format_elapsed(Duration::from_secs(2 * 3600 + 30 * 60 + 15)),
            "+2:30:15"
        );
    }

    #[test]
    fn kind_label_maps_each_variant() {
        assert_eq!(kind_label(PathKind::Ip), "QUIC / IP");
        assert_eq!(kind_label(PathKind::Relay), "Relay");
        assert_eq!(kind_label(PathKind::Custom), "Custom");
    }

    #[test]
    fn kind_class_matches_css_selector_suffix() {
        assert_eq!(kind_class(PathKind::Ip), "ip");
        assert_eq!(kind_class(PathKind::Relay), "relay");
        assert_eq!(kind_class(PathKind::Custom), "custom");
    }

    #[test]
    fn tribool_handles_each_case() {
        assert_eq!(tribool(Some(true)), "yes");
        assert_eq!(tribool(Some(false)), "no");
        assert_eq!(tribool(None), "(not probed)");
    }

    fn path(addr: &str, kind: PathKind, selected: bool) -> PathInfo {
        PathInfo {
            addr: addr.into(),
            kind,
            selected,
            rtt_ms: 0.0,
        }
    }

    #[test]
    fn derive_connection_state_disconnected_when_idle_with_no_paths() {
        assert_eq!(
            derive_connection_state(&ConnectionState::Idle, &[]),
            ConnectionStateLabel::Disconnected
        );
    }

    #[test]
    fn derive_connection_state_connecting_during_dial_before_paths_appear() {
        // The dial is in flight but no path is selected yet. The header
        // should surface progress rather than implying nothing happened.
        assert_eq!(
            derive_connection_state(&ConnectionState::Connecting, &[]),
            ConnectionStateLabel::Connecting
        );
    }

    #[test]
    fn derive_connection_state_error_when_dial_failed() {
        assert_eq!(
            derive_connection_state(&ConnectionState::Error("nope".into()), &[]),
            ConnectionStateLabel::Error
        );
    }

    #[test]
    fn derive_connection_state_relay() {
        let paths = [path("relay:https://a/", PathKind::Relay, true)];
        assert_eq!(
            derive_connection_state(
                &ConnectionState::Connected {
                    peer_id: "id".into(),
                    peer_short_id: "id".into()
                },
                &paths
            ),
            ConnectionStateLabel::Relay
        );
    }

    #[test]
    fn derive_connection_state_direct() {
        let paths = [path("ip:1.2.3.4:5", PathKind::Ip, true)];
        assert_eq!(
            derive_connection_state(
                &ConnectionState::Connected {
                    peer_id: "id".into(),
                    peer_short_id: "id".into()
                },
                &paths
            ),
            ConnectionStateLabel::Direct
        );
    }

    #[test]
    fn derive_connection_state_custom() {
        let paths = [path("custom:ble", PathKind::Custom, true)];
        assert_eq!(
            derive_connection_state(
                &ConnectionState::Connected {
                    peer_id: "id".into(),
                    peer_short_id: "id".into()
                },
                &paths
            ),
            ConnectionStateLabel::Custom
        );
    }

    #[test]
    fn derive_connection_state_only_uses_selected_path() {
        // Both an IP and a relay path exist, but the relay is selected.
        // The header must reflect the selected path, not the first one.
        let paths = [
            path("ip:1.2.3.4:5", PathKind::Ip, false),
            path("relay:https://a/", PathKind::Relay, true),
        ];
        assert_eq!(
            derive_connection_state(
                &ConnectionState::Connected {
                    peer_id: "id".into(),
                    peer_short_id: "id".into()
                },
                &paths
            ),
            ConnectionStateLabel::Relay
        );
    }

    #[test]
    fn derive_connection_state_connecting_when_connected_but_no_selected_path() {
        // QUIC has signalled Connected on the state machine but the
        // paths sampler hasn't pushed a selected path yet (e.g. mid-
        // migration). Show "connecting..." rather than "disconnected".
        assert_eq!(
            derive_connection_state(
                &ConnectionState::Connected {
                    peer_id: "id".into(),
                    peer_short_id: "id".into()
                },
                &[]
            ),
            ConnectionStateLabel::Connecting
        );
    }

    #[test]
    fn format_ttfdb_uses_millis_below_one_second() {
        assert_eq!(format_ttfdb(Duration::from_millis(0)), "0 ms");
        assert_eq!(format_ttfdb(Duration::from_millis(120)), "120 ms");
        assert_eq!(format_ttfdb(Duration::from_millis(999)), "999 ms");
    }

    #[test]
    fn format_ttfdb_uses_seconds_below_one_minute() {
        assert_eq!(format_ttfdb(Duration::from_millis(1000)), "1.0 s");
        assert_eq!(format_ttfdb(Duration::from_millis(2500)), "2.5 s");
        assert_eq!(format_ttfdb(Duration::from_secs(59)), "59.0 s");
    }

    #[test]
    fn format_ttfdb_uses_minutes_past_one_minute() {
        assert_eq!(format_ttfdb(Duration::from_secs(60)), "1:00.0");
        assert_eq!(format_ttfdb(Duration::from_secs(125)), "2:05.0");
    }
}
