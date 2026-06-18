//! App entry point: owns the top-level Dioxus state and the bridge that
//! folds the headless node's event stream into it. Everything else lives
//! in focused modules: the UI in [`components`], the export bundle in
//! [`diagnostics_export`], platform glue in [`clipboard`] and [`logging`],
//! and the node itself in `iroh_doctor_core::node` (re-exported as
//! [`node`]).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use dioxus::prelude::*;
use tokio::sync::mpsc;

#[cfg(target_os = "android")]
mod android;
mod clipboard;
mod components;
mod diagnostics_export;
mod endpoints;
mod first_run;
mod identity;
mod logging;
mod node;
mod telemetry_pref;

use components::{
    status_kind, status_line, AppError, ConnectPage, DiagState, DiagnosticsPage, ErrorDialog,
    EventEntry, GossipView, Nav, Tab,
};
use node::{
    ConnectionState, DiagnosticsReport, NetReportSummary, NodeCommand, NodeEvent, PathSnapshot,
    TelemetryState,
};

/// Maximum samples kept in the RTT sparkline. An outgoing dial samples at
/// the probe ping interval (1 s, so 60 s of history); an incoming probe
/// samples at the 500 ms paths cadence (30 s of history).
const RTT_HISTORY_LEN: usize = 60;
/// Maximum number of connection events retained for the Diagnostics tab.
const EVENT_LOG_LEN: usize = 50;

const MAIN_CSS: Asset = asset!("/assets/styling/main.css");
const FAVICON: Asset = asset!("/assets/favicon.ico");

fn main() {
    let log_dir = logging::log_dir();
    let _log_guard = logging::init(log_dir.as_ref());

    // The desktop window title and taskbar icon are set at runtime: dx's
    // `[bundle] icon` only generates the packaged .app/.exe icon set, not the
    // live window chrome. Without this the window is titled "Dioxus app" with
    // the default Dioxus icon. Mobile/web don't have a desktop window, so they
    // keep the plain launch.
    #[cfg(feature = "desktop")]
    {
        use dioxus::desktop::{icon_from_memory, tao::window::Icon, Config, WindowBuilder};

        let mut cfg = Config::new().with_window(WindowBuilder::new().with_title("iroh doctor"));
        if let Ok(icon) = icon_from_memory::<Icon>(include_bytes!("../assets/icon/icon-master.png"))
        {
            cfg = cfg.with_icon(icon);
        }
        dioxus::LaunchBuilder::desktop().with_cfg(cfg).launch(App);
    }
    #[cfg(not(feature = "desktop"))]
    dioxus::launch(App);
}

/// Sender half of the node's command channel, shared with every component
/// that fires a [`NodeCommand`].
pub type NodeHandle = mpsc::Sender<NodeCommand>;

#[component]
fn App() -> Element {
    let endpoint_id = use_signal(String::new);
    let conn_state = use_signal(|| ConnectionState::Idle);
    let telemetry = use_signal(|| TelemetryState::Off);
    let cmd_handle: Signal<Option<NodeHandle>> = use_signal(|| None);
    let peer_id_input = use_signal(String::new);
    let current_tab = use_signal(|| Tab::Connect);

    let services_ping_state: Signal<DiagState<Duration>> = use_signal(|| DiagState::Idle);
    let net_state: Signal<DiagState<DiagnosticsReport>> = use_signal(|| DiagState::Idle);
    let net_report_state: Signal<DiagState<NetReportSummary>> = use_signal(|| DiagState::Idle);
    let relays_state: Signal<DiagState<Vec<iroh_doctor_core::report::RelayLatencyRow>>> =
        use_signal(|| DiagState::Idle);
    let paths: Signal<Vec<PathSnapshot>> = use_signal(Vec::new);
    let ttfdb: Signal<Option<Duration>> = use_signal(|| None);
    let throughput: Signal<Option<node::ThroughputSnapshot>> = use_signal(|| None);
    let rtt_history: Signal<VecDeque<f64>> =
        use_signal(|| VecDeque::with_capacity(RTT_HISTORY_LEN));
    let event_log: Signal<VecDeque<EventEntry>> =
        use_signal(|| VecDeque::with_capacity(EVENT_LOG_LEN));
    let start_instant = use_signal(Instant::now);

    // Endpoints list lives in `App` so it survives tab switches and is
    // loaded from disk once on app start.
    let endpoints_list: Signal<Vec<endpoints::Endpoint>> = use_signal(endpoints::load);

    let diag_auto_run: Signal<bool> = use_signal(|| false);

    // Global error dialog state. Setters live in App so the modal can
    // be triggered from any subsystem without threading a signal
    // through every component.
    let app_error: Signal<Option<AppError>> = use_signal(|| None);
    use_context_provider(|| app_error);

    // The node bridge: spawn the headless node, then fold its event
    // stream into the signals above for as long as the app lives.
    use_future(move || async move {
        let secret_key = match identity::load_or_create_secret_key() {
            Ok(k) => k,
            Err(e) => {
                conn_state
                    .clone()
                    .set(ConnectionState::Error(format!("identity: {e:#}")));
                return;
            }
        };
        let options = node::NodeOptions {
            secret_key,
            api_secret_override: identity::load_api_secret_override(),
            telemetry_disabled: telemetry_pref::telemetry_disabled(),
        };

        let (cmd_tx, cmd_rx) = mpsc::channel::<NodeCommand>(64);
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<NodeEvent>();
        cmd_handle.clone().set(Some(cmd_tx));
        tokio::spawn(async move {
            let _ = node::run_node(options, cmd_rx, event_tx).await;
        });

        while let Some(event) = event_rx.recv().await {
            match event {
                NodeEvent::EndpointId(id) => endpoint_id.clone().set(id),
                NodeEvent::ConnectionState(new_state) => {
                    record_event(event_log, start_instant, &new_state);
                    match &new_state {
                        // A fresh dial or an incoming probe takes over the
                        // latency graph. Clear the previous peer's samples so
                        // the sparkline does not splice one peer's series (or
                        // one latency source) onto the next.
                        ConnectionState::Connecting => clear_rtt_history(rtt_history),
                        ConnectionState::Connected { peer_id, .. } => {
                            clear_rtt_history(rtt_history);
                            record_endpoint(endpoints_list, peer_id);
                        }
                        _ => {}
                    }
                    conn_state.clone().set(new_state);
                }
                NodeEvent::Telemetry(state) => telemetry.clone().set(state),
                NodeEvent::Paths(snapshot) => paths.clone().set(snapshot),
                NodeEvent::Latency(sample) => {
                    push_rtt_sample(rtt_history, sample.as_secs_f64() * 1000.0);
                }
                NodeEvent::Ttfdb(elapsed) => ttfdb.clone().set(elapsed),
                NodeEvent::Throughput(snapshot) => throughput.clone().set(Some(snapshot)),
            }
        }
    });

    use_effect(move || {
        if matches!(conn_state(), ConnectionState::Connected { .. }) {
            if services_configured() {
                components::trigger_pings(cmd_handle, services_ping_state);
                components::trigger_net_diagnostics(cmd_handle, net_state);
            }
            components::trigger_probe_net_report(cmd_handle, net_report_state);
            components::trigger_probe_relays(cmd_handle, relays_state);
        }
    });

    // Run the peer-independent diagnostics once as soon as the peer task
    // is ready, so the network report is populated on first load without
    // the user having to hit Refresh. These probe the local endpoint,
    // relays, and iroh-services, so they work even while disconnected.
    {
        let mut auto_flag = diag_auto_run;
        use_effect(move || {
            if cmd_handle.read().is_some() && !auto_flag.peek().to_owned() {
                auto_flag.set(true);
                // The services probes need an API key; without one they can
                // only fail with "not initialized", which would greet a
                // first-run user (telemetry is off by default) with an
                // error. Skip them and leave their panels at "not run yet".
                if services_configured() {
                    components::trigger_pings(cmd_handle, services_ping_state);
                    components::trigger_net_diagnostics(cmd_handle, net_state);
                }
                components::trigger_probe_net_report(cmd_handle, net_report_state);
                components::trigger_probe_relays(cmd_handle, relays_state);
            }
        });
    }

    // Funnel every error source into the global modal. Each effect
    // tracks one signal; when it transitions into the error state we
    // surface it, prefixing the source so the user knows which probe
    // or action triggered the modal.
    let mut error_sink = app_error;
    use_effect(move || {
        if let ConnectionState::Error(msg) = conn_state() {
            error_sink.set(Some(AppError::new("connect", explain_connect_error(&msg))));
        }
    });
    use_effect(move || {
        if let DiagState::Err(msg) = services_ping_state() {
            if !is_services_off_error(&msg) {
                error_sink.set(Some(AppError::new("services ping", msg)));
            }
        }
    });
    use_effect(move || {
        if let DiagState::Err(msg) = net_state() {
            if !is_services_off_error(&msg) {
                error_sink.set(Some(AppError::new("services net_diagnostics", msg)));
            }
        }
    });
    use_effect(move || {
        if let DiagState::Err(msg) = net_report_state() {
            error_sink.set(Some(AppError::new("net_report probe", msg)));
        }
    });
    use_effect(move || {
        if let DiagState::Err(msg) = relays_state() {
            error_sink.set(Some(AppError::new("relay latency probe", msg)));
        }
    });

    let tab = current_tab();
    let status = status_line(&conn_state());
    let status_kind = status_kind(&conn_state());

    rsx! {
        document::Meta {
            name: "viewport",
            content: "width=device-width, initial-scale=1, viewport-fit=cover",
        }
        document::Link { rel: "icon", href: FAVICON }
        document::Stylesheet { href: MAIN_CSS }

        div { class: "app-layout",
            Nav { current_tab }
            main { class: "main-content",
                match tab {
                    Tab::Connect => rsx! {
                        ConnectPage {
                            endpoint_id, conn_state, cmd_handle, peer_id_input,
                            paths, rtt_history, event_log, ttfdb, throughput,
                        }
                    },
                    Tab::Diagnostics => rsx! {
                        DiagnosticsPage {
                            cmd_handle,
                            telemetry,
                            services_ping_state, net_state, net_report_state, relays_state,
                        }
                    },
                    Tab::Gossip => rsx! {
                        div { class: "page",
                            h2 { class: "page-title", "Gossip" }
                            GossipView { cmd_handle, conn_state }
                        }
                    },
                    Tab::Endpoints => rsx! {
                        div { class: "page",
                            h2 { class: "page-title", "Endpoints" }
                            components::EndpointsView {
                                cmd_handle,
                                endpoints: endpoints_list,
                                on_change: move |next: Vec<endpoints::Endpoint>| save_endpoints(endpoints_list, next),
                                on_connect: move |_| current_tab.clone().set(Tab::Connect),
                            }
                        }
                    },
                }
            }
            div { class: "status-indicator", "data-state": "{status_kind}",
                span { class: "status-dot" }
                span { class: "status-text", "{status}" }
            }
            ErrorDialog {
                error: app_error,
                on_send_diagnostics: move |err: AppError| {
                    diagnostics_export::send(
                        err,
                        endpoint_id,
                        conn_state,
                        paths,
                        rtt_history,
                        event_log,
                        net_report_state,
                        relays_state,
                        ttfdb,
                        throughput,
                        endpoints_list,
                        app_error,
                    );
                },
            }
        }
    }
}

/// Returns true when iroh-services telemetry is active: on by default with the
/// bundled key, unless the user turned it off or an empty env override opts
/// out. Without an active client the node never starts the services probes, so
/// the services ping and net_diagnostics could only fail; callers skip
/// triggering them in that case.
fn services_configured() -> bool {
    use iroh_doctor_core::services::{resolve_api_secret, SecretSource};
    resolve_api_secret(SecretSource::AppDefault {
        disabled: telemetry_pref::telemetry_disabled(),
        custom: &identity::load_api_secret_override(),
    })
    .is_some()
}

/// Matches the node's services-off reply. With no API key configured this
/// is the expected state, not a failure: the Diagnostics tab already
/// explains telemetry is off next to the key input, so the global error
/// modal stays quiet for it.
fn is_services_off_error(msg: &str) -> bool {
    msg.contains(node::SERVICES_OFF_ERROR)
}

/// Expands the node's connect-stage errors with recovery guidance where
/// the raw message alone leaves a first-time user stuck. A failed bind
/// is fatal for the process (the node task has exited), so the only
/// recovery is fixing the network and relaunching.
fn explain_connect_error(msg: &str) -> String {
    if msg.starts_with(node::BIND_FAILED_PREFIX) {
        format!(
            "{msg}\n\nThe app could not open a network socket, so it cannot \
             connect to peers or accept probes. Check the device's network \
             connection, then close and reopen the app."
        )
    } else {
        msg.to_string()
    }
}

fn record_event(
    mut event_log: Signal<VecDeque<EventEntry>>,
    start_instant: Signal<Instant>,
    state: &ConnectionState,
) {
    let elapsed = start_instant.read().elapsed();
    let entry = EventEntry {
        elapsed,
        label: components::short_event_label(state),
        kind: status_kind(state).to_string(),
    };
    let mut log = event_log.write();
    if log.len() >= EVENT_LOG_LEN {
        log.pop_front();
    }
    log.push_back(entry);
}

/// Records a successful connection in the persistent endpoints list and
/// writes the result to disk. New ids are appended; existing ids have
/// their `last_seen` bumped.
fn record_endpoint(mut endpoints_list: Signal<Vec<endpoints::Endpoint>>, peer_id: &str) {
    let next = endpoints::record_connection(endpoints_list.peek().clone(), peer_id);
    if let Err(e) = endpoints::save(&next) {
        tracing::warn!(err = %e, "writing endpoints.json");
    }
    endpoints_list.set(next);
}

/// Applies a UI-driven mutation (rename, delete) and persists the
/// result. Logs and surfaces the write error rather than rolling back so
/// the user sees both the immediate UI change and a tracing warning if
/// disk is unhappy.
fn save_endpoints(
    mut endpoints_list: Signal<Vec<endpoints::Endpoint>>,
    next: Vec<endpoints::Endpoint>,
) {
    if let Err(e) = endpoints::save(&next) {
        tracing::warn!(err = %e, "writing endpoints.json");
    }
    endpoints_list.set(next);
}

fn push_rtt_sample(mut rtt_history: Signal<VecDeque<f64>>, sample_ms: f64) {
    let mut hist = rtt_history.write();
    if hist.len() >= RTT_HISTORY_LEN {
        hist.pop_front();
    }
    hist.push_back(sample_ms);
}

/// Drops the accumulated latency samples. Called when a new connection takes
/// over so the sparkline starts clean for each peer.
fn clear_rtt_history(mut rtt_history: Signal<VecDeque<f64>>) {
    rtt_history.write().clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn services_off_error_is_recognized_from_the_node_constant() {
        // Pin the matcher to the exact text the node produces, so rewording
        // the node-side reply cannot silently resurrect the first-run modal.
        assert!(is_services_off_error(node::SERVICES_OFF_ERROR));
        assert!(is_services_off_error(&format!(
            "ping: {}",
            node::SERVICES_OFF_ERROR
        )));
        assert!(!is_services_off_error("some other failure"));
    }

    #[test]
    fn connect_error_explains_only_bind_failures() {
        let bind = explain_connect_error(&format!("{}: address in use", node::BIND_FAILED_PREFIX));
        assert!(bind.contains("close and reopen"));
        // A non-bind error is passed through unchanged.
        let other = explain_connect_error("invalid endpoint id");
        assert_eq!(other, "invalid endpoint id");
    }
}
