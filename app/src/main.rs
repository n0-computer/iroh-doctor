use std::collections::VecDeque;
use std::sync::Arc;

use dioxus::prelude::*;
use tokio::sync::{mpsc, watch};

mod components;
mod diagnostics_export;
mod endpoints;
mod first_run;
mod identity;
mod node;
mod telemetry_pref;

use std::time::{Duration, Instant};

use components::{
    AppError, ConnectView, DiagState, DiagnosticsView, EndpointsView, ErrorDialog, EventEntry,
    FirstRunNote, GossipView,
};
use node::{
    ConnectionState, DiagnosticsReport, NetReportSummary, NodeCallbacks, NodeCommand, PathInfo,
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
    let log_dir = log_dir();
    let _log_guard = init_logging(log_dir.as_ref());

    dioxus::launch(App);
}

/// Returns the directory we write rolling log files to. `None` when no
/// config dir is available on the platform; the app then logs only to
/// stdout.
fn log_dir() -> Option<std::path::PathBuf> {
    let dir = identity::config_dir().ok()?.join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Sets up stdout + (optional) daily rolling file logging. Returns the
/// `WorkerGuard` for the file writer; dropping it would stop flushing
/// log lines, so `main()` keeps it bound for the program lifetime.
fn init_logging(
    log_dir: Option<&std::path::PathBuf>,
) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,iroh_doctor_app=debug".into());

    let stdout_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stdout);

    let (file_layer, guard) = match log_dir {
        Some(dir) => {
            let file_appender = tracing_appender::rolling::daily(dir, "iroh-doctor-app.log");
            let (writer, guard) = tracing_appender::non_blocking(file_appender);
            (
                Some(
                    tracing_subscriber::fmt::layer()
                        .with_writer(writer)
                        .with_ansi(false),
                ),
                Some(guard),
            )
        }
        None => (None, None),
    };

    // On iOS, stdout is dropped (GUI apps aren't attached to a terminal) and
    // the rolling file lives inside the app sandbox, so neither layer above is
    // visible to `log stream` / Console.app / Xcode. Forward tracing into
    // `os_log` so iroh's logs (discovery publish, relay, magicsock, ...) land
    // in the iOS unified log, filterable by `subsystem:com.number0.iroh-doctor-app`.
    // `Option<L>` implements `Layer`, so the non-iOS no-op (`None`) keeps the
    // registry type identical across targets.
    #[cfg(target_os = "ios")]
    let oslog_layer = Some(tracing_oslog::OsLogger::new(
        "com.number0.iroh-doctor-app",
        "default",
    ));
    #[cfg(not(target_os = "ios"))]
    let oslog_layer: Option<tracing_subscriber::layer::Identity> = None;

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stdout_layer)
        .with(file_layer)
        .with(oslog_layer)
        .init();
    guard
}

#[derive(Clone)]
pub struct NodeHandle {
    pub tx: mpsc::Sender<NodeCommand>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Connect,
    Diagnostics,
    Gossip,
    Endpoints,
}

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
    let paths: Signal<Vec<PathInfo>> = use_signal(Vec::new);
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
        let api_override = identity::load_api_secret_override();
        let telemetry_disabled = telemetry_pref::telemetry_disabled();

        let (cmd_tx_inner, cmd_rx) = mpsc::channel::<NodeCommand>(64);
        let (id_tx, mut id_rx) = watch::channel::<String>(String::new());
        let (state_tx, mut state_rx) = watch::channel(ConnectionState::Idle);
        let (telemetry_tx, mut telemetry_rx) = watch::channel(TelemetryState::Off);
        let (paths_tx, mut paths_rx) = watch::channel(Vec::<PathInfo>::new());
        let (ttfdb_tx, mut ttfdb_rx) = watch::channel::<Option<Duration>>(None);
        let (throughput_tx, mut throughput_rx) =
            watch::channel::<Option<node::ThroughputSnapshot>>(None);
        let (latency_tx, mut latency_rx) = watch::channel::<Option<Duration>>(None);

        cmd_handle
            .clone()
            .set(Some(NodeHandle { tx: cmd_tx_inner }));

        let id_tx = Arc::new(id_tx);
        let state_tx = Arc::new(state_tx);
        let telemetry_tx = Arc::new(telemetry_tx);
        let paths_tx = Arc::new(paths_tx);
        let ttfdb_tx = Arc::new(ttfdb_tx);
        let throughput_tx = Arc::new(throughput_tx);
        let latency_tx = Arc::new(latency_tx);

        {
            let id_tx = id_tx.clone();
            let state_tx = state_tx.clone();
            let telemetry_tx = telemetry_tx.clone();
            let paths_tx = paths_tx.clone();
            let ttfdb_tx = ttfdb_tx.clone();
            let throughput_tx = throughput_tx.clone();
            let latency_tx = latency_tx.clone();
            tokio::spawn(async move {
                let _ = node::run_node(
                    secret_key,
                    api_override,
                    telemetry_disabled,
                    cmd_rx,
                    NodeCallbacks {
                        on_endpoint_id: Box::new(move |s| {
                            let _ = id_tx.send(s);
                        }),
                        on_state: Box::new(move |s| {
                            let _ = state_tx.send(s);
                        }),
                        on_telemetry: Box::new(move |t| {
                            let _ = telemetry_tx.send(t);
                        }),
                        on_paths: Box::new(move |p| {
                            let _ = paths_tx.send(p);
                        }),
                        on_ttfdb: Box::new(move |d| {
                            let _ = ttfdb_tx.send(d);
                        }),
                        on_throughput: Box::new(move |t| {
                            let _ = throughput_tx.send(Some(t));
                        }),
                        on_latency: Box::new(move |d| {
                            let _ = latency_tx.send(Some(d));
                        }),
                    },
                )
                .await;
            });
        }

        let _keep_senders = (
            id_tx,
            state_tx,
            telemetry_tx,
            paths_tx,
            ttfdb_tx,
            throughput_tx,
            latency_tx,
        );

        loop {
            tokio::select! {
                Ok(()) = id_rx.changed() => endpoint_id.clone().set(id_rx.borrow().clone()),
                Ok(()) = state_rx.changed() => {
                    let new_state = state_rx.borrow().clone();
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
                Ok(()) = telemetry_rx.changed() => telemetry.clone().set(telemetry_rx.borrow().clone()),
                Ok(()) = paths_rx.changed() => {
                    let snapshot = paths_rx.borrow().clone();
                    paths.clone().set(snapshot);
                }
                Ok(()) = latency_rx.changed() => {
                    if let Some(sample) = *latency_rx.borrow() {
                        push_rtt_sample(rtt_history, sample.as_secs_f64() * 1000.0);
                    }
                }
                Ok(()) = ttfdb_rx.changed() => {
                    ttfdb.clone().set(*ttfdb_rx.borrow());
                }
                Ok(()) = throughput_rx.changed() => {
                    throughput.clone().set(throughput_rx.borrow().clone());
                }
                else => break,
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
                            EndpointsView {
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
                    handle_send_diagnostics(
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

/// Glue: snapshot every relevant App-level signal, build the zip, and
/// hand it off to the platform save dialog.
#[allow(clippy::too_many_arguments)]
fn handle_send_diagnostics(
    err: AppError,
    endpoint_id: Signal<String>,
    conn_state: Signal<ConnectionState>,
    paths: Signal<Vec<node::PathInfo>>,
    rtt_history: Signal<VecDeque<f64>>,
    event_log: Signal<VecDeque<EventEntry>>,
    net_report_state: Signal<DiagState<NetReportSummary>>,
    relays_state: Signal<DiagState<Vec<iroh_doctor_core::report::RelayLatencyRow>>>,
    ttfdb: Signal<Option<Duration>>,
    throughput: Signal<Option<node::ThroughputSnapshot>>,
    endpoints_list: Signal<Vec<endpoints::Endpoint>>,
    mut error_sink: Signal<Option<AppError>>,
) {
    use anyhow::Context as _;
    let (net_report, _) = diagnostics_export::Snapshot::extract_diag_state(&net_report_state());
    let (relays, relays_err) = diagnostics_export::Snapshot::extract_diag_state(&relays_state());

    let snapshot = diagnostics_export::Snapshot {
        error_message: format!("[{}] {}", err.source, err.message),
        endpoint_id: endpoint_id(),
        conn_state_label: short_event_label(&conn_state()),
        paths: paths(),
        rtt_history: rtt_history(),
        events: event_log(),
        net_report,
        relays: relays.unwrap_or_default(),
        relays_err,
        ttfdb: ttfdb(),
        throughput: throughput(),
        endpoints: endpoints_list(),
        log_dir: log_dir(),
    };
    spawn(async move {
        let result = async {
            let bytes =
                diagnostics_export::build_zip(&snapshot).context("building diagnostics zip")?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let filename = format!("iroh-doctor-app-diagnostics-{now}.zip");
            save_diagnostics_zip(&filename, &bytes).await
        }
        .await;
        if let Err(e) = result {
            let msg = format!("{e:#}");
            tracing::error!(err = %msg, "diagnostics export failed");
            error_sink.set(Some(AppError::new("diagnostics export", msg)));
        }
    });
}

/// Desktop path: open a native save dialog via rfd and write the bytes
/// to whichever location the user picks. Cancelling the dialog is `Ok`;
/// only a failed write is an error worth surfacing.
#[cfg(not(any(target_os = "ios", target_os = "android")))]
async fn save_diagnostics_zip(filename: &str, bytes: &[u8]) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let dialog = rfd::AsyncFileDialog::new()
        .set_file_name(filename)
        .set_title("Save iroh-doctor-app diagnostics");
    let Some(handle) = dialog.save_file().await else {
        return Ok(());
    };
    handle
        .write(bytes)
        .await
        .context("writing diagnostics zip")?;
    Ok(())
}

/// Mobile path (iOS + Android): rfd has no usable backend, so write to the
/// app's sandbox documents directory where the platform's Files browser
/// exposes it. The user can share the file from there. Logs the resulting
/// path so a developer inspecting the log file can find it without guessing.
#[cfg(any(target_os = "ios", target_os = "android"))]
async fn save_diagnostics_zip(filename: &str, bytes: &[u8]) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let dir = dirs::document_dir()
        .or_else(dirs::data_local_dir)
        .context("no documents directory on this device")?;
    let path = dir.join(filename);
    std::fs::write(&path, bytes)
        .with_context(|| format!("writing diagnostics zip to {}", path.display()))?;
    tracing::info!(path = %path.display(), "wrote diagnostics zip");
    Ok(())
}

#[component]
fn Nav(current_tab: Signal<Tab>) -> Element {
    let active = current_tab();
    rsx! {
        nav { class: "nav",
            NavItem {
                label: "Connect",
                icon: "⇄",
                is_active: active == Tab::Connect,
                on_select: move |_| current_tab.clone().set(Tab::Connect),
            }
            NavItem {
                label: "Diagnostics",
                icon: "⌁",
                is_active: active == Tab::Diagnostics,
                on_select: move |_| current_tab.clone().set(Tab::Diagnostics),
            }
            NavItem {
                label: "Gossip",
                icon: "≈",
                is_active: active == Tab::Gossip,
                on_select: move |_| current_tab.clone().set(Tab::Gossip),
            }
            NavItem {
                label: "Endpoints",
                icon: "▣",
                is_active: active == Tab::Endpoints,
                on_select: move |_| current_tab.clone().set(Tab::Endpoints),
            }
        }
    }
}

#[component]
fn NavItem(label: String, icon: String, is_active: bool, on_select: EventHandler<()>) -> Element {
    let class = if is_active {
        "nav-item active"
    } else {
        "nav-item"
    };
    rsx! {
        button {
            class: "{class}",
            onclick: move |_| on_select.call(()),
            span { class: "nav-icon", "{icon}" }
            span { class: "nav-label", "{label}" }
        }
    }
}

#[component]
fn ConnectPage(
    endpoint_id: Signal<String>,
    conn_state: Signal<ConnectionState>,
    cmd_handle: Signal<Option<NodeHandle>>,
    peer_id_input: Signal<String>,
    paths: Signal<Vec<PathInfo>>,
    rtt_history: Signal<VecDeque<f64>>,
    event_log: Signal<VecDeque<EventEntry>>,
    ttfdb: Signal<Option<Duration>>,
    throughput: Signal<Option<node::ThroughputSnapshot>>,
) -> Element {
    rsx! {
        div { class: "page",
            h2 { class: "page-title", "Connect" }
            FirstRunNote {}
            Header { endpoint_id }
            ConnectBar { cmd_handle, peer_id_input, conn_state }
            ConnectView {
                conn_state,
                paths,
                rtt_history,
                event_log,
                ttfdb,
                throughput,
            }
        }
    }
}

#[component]
fn DiagnosticsPage(
    cmd_handle: Signal<Option<NodeHandle>>,
    telemetry: Signal<TelemetryState>,
    services_ping_state: Signal<DiagState<Duration>>,
    net_state: Signal<DiagState<DiagnosticsReport>>,
    net_report_state: Signal<DiagState<NetReportSummary>>,
    relays_state: Signal<DiagState<Vec<iroh_doctor_core::report::RelayLatencyRow>>>,
) -> Element {
    rsx! {
        div { class: "page",
            h2 { class: "page-title", "Diagnostics" }
            DiagnosticsView {
                cmd_handle,
                telemetry,
                services_state: services_ping_state,
                net_state,
                net_report_state,
                relays_state,
            }
        }
    }
}

#[component]
fn Header(endpoint_id: Signal<String>) -> Element {
    let id = endpoint_id();
    let display = if id.is_empty() {
        "...".to_string()
    } else {
        id.clone()
    };
    let copy_disabled = id.is_empty();

    rsx! {
        div { class: "header",
            span { class: "label", "My id:" }
            span { class: "endpoint-id", title: "{id}", "{display}" }
            button {
                class: "btn",
                disabled: copy_disabled,
                onclick: move |_| {
                    let id = endpoint_id();
                    if !id.is_empty() {
                        copy_to_clipboard(&id);
                    }
                },
                "Copy"
            }
        }
    }
}

#[component]
fn ConnectBar(
    cmd_handle: Signal<Option<NodeHandle>>,
    peer_id_input: Signal<String>,
    conn_state: Signal<ConnectionState>,
) -> Element {
    // Connect and disconnect are distinct steps. Once a session is dialing or
    // live, the input gives way to a single Disconnect button (Cancel while a
    // dial is still in flight); otherwise we show the input and Connect.
    let state = conn_state();
    let connecting = matches!(state, ConnectionState::Connecting);
    let active = connecting || matches!(state, ConnectionState::Connected { .. });

    if active {
        let label = if connecting { "Cancel" } else { "Disconnect" };
        return rsx! {
            div { class: "connect-bar",
                span { class: "connect-status", "{status_line(&state)}" }
                button {
                    class: "btn btn-danger",
                    onclick: move |_| {
                        if let Some(handle) = cmd_handle.read().clone() {
                            let _ = handle.tx.try_send(NodeCommand::Disconnect);
                        }
                    },
                    "{label}"
                }
            }
        };
    }

    let input_value = peer_id_input();
    let connect_disabled = !node::looks_like_endpoint_id(&input_value);
    // The Connect button stays disabled on malformed input; without a
    // hint a first-time user pasting a truncated id only sees a button
    // that will not press. Explain what a valid id looks like.
    let show_invalid_hint = connect_disabled && !input_value.trim().is_empty();

    rsx! {
        div { class: "connect-bar-wrap",
            div { class: "connect-bar",
                input {
                    class: "peer-id-input",
                    r#type: "text",
                    placeholder: "Peer endpoint id",
                    value: "{input_value}",
                    autocapitalize: "off",
                    autocorrect: "off",
                    spellcheck: "false",
                    oninput: move |evt| { peer_id_input.clone().set(evt.value()); },
                }
                button {
                    class: "btn btn-primary",
                    disabled: connect_disabled,
                    onclick: move |_| {
                        let id = peer_id_input();
                        if let Some(handle) = cmd_handle.read().clone() {
                            let _ = handle.tx.try_send(NodeCommand::Connect { hex_id: id });
                        }
                    },
                    "Connect"
                }
            }
            if show_invalid_hint {
                div { class: "input-hint",
                    "Not a valid endpoint id yet: ids are 64 hex characters. "
                    "Paste the full id from the other device's Copy button."
                }
            }
        }
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
        label: short_event_label(state),
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

fn short_event_label(state: &ConnectionState) -> String {
    match state {
        ConnectionState::Idle => "idle".into(),
        ConnectionState::Binding => "binding".into(),
        ConnectionState::Ready => "ready".into(),
        ConnectionState::Connecting => "connecting".into(),
        ConnectionState::Connected { peer_short_id, .. } => format!("connected: {peer_short_id}"),
        ConnectionState::PeerDisconnected { peer_short_id } => {
            format!("peer disconnected: {peer_short_id}")
        }
        ConnectionState::Error(msg) => format!("error: {msg}"),
    }
}

fn status_line(state: &ConnectionState) -> String {
    match state {
        ConnectionState::Idle => "idle".into(),
        ConnectionState::Binding => "binding...".into(),
        ConnectionState::Ready => "ready".into(),
        ConnectionState::Connecting => "connecting...".into(),
        ConnectionState::Connected { peer_short_id, .. } => format!("connected to {peer_short_id}"),
        ConnectionState::PeerDisconnected { peer_short_id } => {
            format!("{peer_short_id} disconnected")
        }
        ConnectionState::Error(msg) => format!("error: {msg}"),
    }
}

fn status_kind(state: &ConnectionState) -> &'static str {
    match state {
        ConnectionState::Idle => "idle",
        ConnectionState::Binding => "pending",
        ConnectionState::Ready => "ready",
        ConnectionState::Connecting => "pending",
        ConnectionState::Connected { .. } => "connected",
        ConnectionState::PeerDisconnected { .. } => "disconnected",
        ConnectionState::Error(_) => "error",
    }
}

pub fn copy_to_clipboard(_text: &str) {
    // On iOS, write through UIPasteboard instead of the JS Clipboard API.
    // When the iOS build runs on an Apple silicon Mac ("iOS app on Mac"),
    // navigator.clipboard.writeText resolves ok but only WebKit's private
    // com.apple.WebKit.custom-pasteboard-data type crosses the
    // UIPasteboard -> NSPasteboard bridge; the text/plain representation
    // is dropped, so pasting into any other app yields nothing.
    // UIPasteboard bridges correctly in both environments.
    #[cfg(target_os = "ios")]
    {
        use objc2_foundation::NSString;
        use objc2_ui_kit::UIPasteboard;

        let text = NSString::from_str(_text);
        // SAFETY: setString is unsafe only because UIPasteboard is not
        // documented as thread-safe. We are on the main thread here: this
        // is only called from Dioxus event handlers, which run on the UI
        // thread on mobile.
        unsafe { UIPasteboard::generalPasteboard().setString(Some(&text)) };
    }
    #[cfg(all(any(feature = "desktop", feature = "mobile"), not(target_os = "ios")))]
    {
        let text = _text.to_string();
        dioxus::prelude::document::eval(&format!(
            "navigator.clipboard.writeText({});",
            serde_escape(&text)
        ));
    }
}

#[cfg(all(any(feature = "desktop", feature = "mobile"), not(target_os = "ios")))]
fn serde_escape(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
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
