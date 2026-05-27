use std::collections::VecDeque;
use std::sync::Arc;

use dioxus::prelude::*;
use tokio::sync::{mpsc, watch};

mod components;
mod diagnostics_export;
mod doctor;
mod endpoints;
mod game;
mod identity;
mod peer;
mod portmap_probe;
mod relay_probe;
mod wire;

use std::time::{Duration, Instant};

use components::{
    AppError, BlobsView, DiagState, DiagnosticsView, DocsView, EndpointsView, ErrorDialog,
    EventEntry, GossipView, PongScene,
};
use game::PongGame;
use peer::{
    BlobSummary, ConnectionState, DiagnosticsReport, NetReportSummary, PathInfo, PeerCallbacks,
    PeerCommand, TelemetryState,
};

/// Maximum samples kept in the RTT sparkline. At 500 ms intervals this is
/// 30 s of history.
const RTT_HISTORY_LEN: usize = 60;
/// Maximum number of connection events retained for the Diagnostics tab.
const EVENT_LOG_LEN: usize = 50;

const MAIN_CSS: Asset = asset!("/assets/styling/main.css");
const FAVICON: Asset = asset!("/assets/favicon.ico");

fn main() {
    let log_dir = log_dir();
    let _log_guard = init_logging(log_dir.as_ref());

    // rustls 0.23 needs a process-global crypto provider when callers
    // build TLS configs without an explicit provider. iroh's presets set
    // one per endpoint, but the per-relay probe in `relay_probe.rs`
    // builds `iroh_relay::client::ClientBuilder` directly and would
    // otherwise fail with "No rustls crypto provider configured". Ignore
    // the install error: a duplicate install just means another part of
    // the process beat us to it.
    let _ = rustls::crypto::ring::default_provider().install_default();
    dioxus::launch(App);
}

/// Returns the directory we write rolling log files to. `None` when no
/// config dir is available on the platform; the app then logs only to
/// stdout.
fn log_dir() -> Option<std::path::PathBuf> {
    let base = dirs::config_dir()?;
    let dir = base.join("iroh-doctor-app").join("logs");
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
pub struct PeerHandle {
    pub tx: mpsc::Sender<PeerCommand>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Diagnostics,
    Data,
    Gossip,
    Endpoints,
    Pong,
}

#[component]
fn App() -> Element {
    let endpoint_id = use_signal(String::new);
    let conn_state = use_signal(|| ConnectionState::Idle);
    let telemetry = use_signal(|| TelemetryState::Off);
    let game = use_signal(PongGame::default);
    let cmd_handle: Signal<Option<PeerHandle>> = use_signal(|| None);
    let peer_id_input = use_signal(String::new);
    let current_tab = use_signal(|| Tab::Diagnostics);

    let services_ping_state: Signal<DiagState<Duration>> = use_signal(|| DiagState::Idle);
    let net_state: Signal<DiagState<DiagnosticsReport>> = use_signal(|| DiagState::Idle);
    let net_report_state: Signal<DiagState<NetReportSummary>> = use_signal(|| DiagState::Idle);
    let relays_state: Signal<DiagState<Vec<relay_probe::RelayProbeResult>>> =
        use_signal(|| DiagState::Idle);
    let portmap_state: Signal<DiagState<portmap_probe::PortMapProbeResult>> =
        use_signal(|| DiagState::Idle);
    let paths: Signal<Vec<PathInfo>> = use_signal(Vec::new);
    let ttfdb: Signal<Option<Duration>> = use_signal(|| None);
    let rtt_history: Signal<VecDeque<f64>> =
        use_signal(|| VecDeque::with_capacity(RTT_HISTORY_LEN));
    let event_log: Signal<VecDeque<EventEntry>> =
        use_signal(|| VecDeque::with_capacity(EVENT_LOG_LEN));
    let start_instant = use_signal(Instant::now);
    let blobs_list: Signal<Vec<BlobSummary>> = use_signal(Vec::new);

    // Endpoints list lives in `App` so it survives tab switches and is
    // loaded from disk once on app start.
    let endpoints_list: Signal<Vec<endpoints::Endpoint>> = use_signal(endpoints::load);

    // Docs state lives in `App` (not inside `DocsView`) so a tab switch
    // does not unmount and discard the auto-created document.
    let active_doc: Signal<Option<String>> = use_signal(|| None);
    let doc_entries: Signal<Vec<peer::DocEntrySummary>> = use_signal(Vec::new);
    let doc_events: Signal<VecDeque<components::DocEventRow>> =
        use_signal(|| VecDeque::with_capacity(components::DOC_EVENT_LOG_CAPACITY));
    let doc_last_error: Signal<Option<String>> = use_signal(|| None);
    let doc_listing: Signal<bool> = use_signal(|| false);
    let docs_auto_created: Signal<bool> = use_signal(|| false);
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

        let (cmd_tx_inner, cmd_rx) = mpsc::channel::<PeerCommand>(64);
        let (id_tx, mut id_rx) = watch::channel::<String>(String::new());
        let (state_tx, mut state_rx) = watch::channel(ConnectionState::Idle);
        let (telemetry_tx, mut telemetry_rx) = watch::channel(TelemetryState::Off);
        let (game_tx, mut game_rx) = watch::channel(PongGame::default());
        let (paths_tx, mut paths_rx) = watch::channel(Vec::<PathInfo>::new());
        let (ttfdb_tx, mut ttfdb_rx) = watch::channel::<Option<Duration>>(None);

        cmd_handle
            .clone()
            .set(Some(PeerHandle { tx: cmd_tx_inner }));

        let id_tx = Arc::new(id_tx);
        let state_tx = Arc::new(state_tx);
        let telemetry_tx = Arc::new(telemetry_tx);
        let game_tx = Arc::new(game_tx);
        let paths_tx = Arc::new(paths_tx);
        let ttfdb_tx = Arc::new(ttfdb_tx);

        {
            let id_tx = id_tx.clone();
            let state_tx = state_tx.clone();
            let telemetry_tx = telemetry_tx.clone();
            let game_tx = game_tx.clone();
            let paths_tx = paths_tx.clone();
            let ttfdb_tx = ttfdb_tx.clone();
            tokio::spawn(async move {
                let _ = peer::run_peer(
                    secret_key,
                    api_override,
                    cmd_rx,
                    PeerCallbacks {
                        on_endpoint_id: Box::new(move |s| {
                            let _ = id_tx.send(s);
                        }),
                        on_state: Box::new(move |s| {
                            let _ = state_tx.send(s);
                        }),
                        on_telemetry: Box::new(move |t| {
                            let _ = telemetry_tx.send(t);
                        }),
                        on_game: Box::new(move |g| {
                            let _ = game_tx.send(g);
                        }),
                        on_paths: Box::new(move |p| {
                            let _ = paths_tx.send(p);
                        }),
                        on_ttfdb: Box::new(move |d| {
                            let _ = ttfdb_tx.send(d);
                        }),
                    },
                )
                .await;
            });
        }

        let _keep_senders = (id_tx, state_tx, telemetry_tx, game_tx, paths_tx, ttfdb_tx);

        loop {
            tokio::select! {
                Ok(()) = id_rx.changed() => endpoint_id.clone().set(id_rx.borrow().clone()),
                Ok(()) = state_rx.changed() => {
                    let new_state = state_rx.borrow().clone();
                    record_event(event_log, start_instant, &new_state);
                    if let ConnectionState::Connected { peer_id, .. } = &new_state {
                        record_endpoint(endpoints_list, peer_id);
                    }
                    conn_state.clone().set(new_state);
                }
                Ok(()) = telemetry_rx.changed() => telemetry.clone().set(telemetry_rx.borrow().clone()),
                Ok(()) = game_rx.changed() => game.clone().set(*game_rx.borrow()),
                Ok(()) = paths_rx.changed() => {
                    let snapshot = paths_rx.borrow().clone();
                    if let Some(selected) = snapshot.iter().find(|p| p.selected) {
                        push_rtt_sample(rtt_history, selected.rtt_ms);
                    }
                    paths.clone().set(snapshot);
                }
                Ok(()) = ttfdb_rx.changed() => {
                    ttfdb.clone().set(*ttfdb_rx.borrow());
                }
                else => break,
            }
        }
    });

    use_effect(move || {
        if matches!(conn_state(), ConnectionState::Connected { .. }) {
            components::trigger_pings(cmd_handle, services_ping_state);
            components::trigger_net_diagnostics(cmd_handle, net_state);
            components::trigger_probe_net_report(cmd_handle, net_report_state);
            components::trigger_probe_relays(cmd_handle, relays_state);
            components::trigger_probe_portmap(cmd_handle, portmap_state);
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
                components::trigger_pings(cmd_handle, services_ping_state);
                components::trigger_net_diagnostics(cmd_handle, net_state);
                components::trigger_probe_net_report(cmd_handle, net_report_state);
                components::trigger_probe_relays(cmd_handle, relays_state);
                components::trigger_probe_portmap(cmd_handle, portmap_state);
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
            error_sink.set(Some(AppError::new("connect", msg)));
        }
    });
    use_effect(move || {
        if let DiagState::Err(msg) = services_ping_state() {
            error_sink.set(Some(AppError::new("services ping", msg)));
        }
    });
    use_effect(move || {
        if let DiagState::Err(msg) = net_state() {
            error_sink.set(Some(AppError::new("services net_diagnostics", msg)));
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
    use_effect(move || {
        if let DiagState::Err(msg) = portmap_state() {
            error_sink.set(Some(AppError::new("portmap probe", msg)));
        }
    });
    use_effect(move || {
        if let Some(msg) = doc_last_error() {
            error_sink.set(Some(AppError::new("docs", msg)));
        }
    });

    // Auto-create the iroh-docs document once the peer task is ready so
    // the document survives the first tab switch and the user does not
    // have to click Create.
    {
        let mut auto_flag = docs_auto_created;
        use_effect(move || {
            if cmd_handle.read().is_some() && !auto_flag.peek().to_owned() {
                auto_flag.set(true);
                components::auto_create_doc(
                    cmd_handle,
                    active_doc,
                    doc_entries,
                    doc_events,
                    doc_listing,
                    doc_last_error,
                );
            }
        });
    }

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
                    Tab::Diagnostics => rsx! {
                        DiagnosticsPage {
                            endpoint_id, conn_state, cmd_handle, peer_id_input,
                            telemetry,
                            services_ping_state, net_state, net_report_state, relays_state,
                            portmap_state,
                            paths, rtt_history, event_log, ttfdb,
                        }
                    },
                    Tab::Data => rsx! {
                        div { class: "page",
                            h2 { class: "page-title", "Data" }
                            h3 { class: "page-subtitle", "Blobs" }
                            BlobsView { cmd_handle, conn_state, blobs_list }
                            h3 { class: "page-subtitle", "Docs" }
                            DocsView {
                                cmd_handle,
                                active_doc,
                                entries: doc_entries,
                                events: doc_events,
                                last_error: doc_last_error,
                                listing: doc_listing,
                            }
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
                                on_connect: move |_| current_tab.clone().set(Tab::Diagnostics),
                            }
                        }
                    },
                    Tab::Pong => rsx! { PongPage { game, endpoint_id, conn_state, cmd_handle } },
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
                        portmap_state,
                        relays_state,
                        ttfdb,
                        blobs_list,
                        endpoints_list,
                    );
                },
            }
        }
    }
}

/// Glue: snapshot every relevant App-level signal, build the zip, and
/// hand it off to the platform save dialog.
#[allow(clippy::too_many_arguments)]
fn handle_send_diagnostics(
    err: AppError,
    endpoint_id: Signal<String>,
    conn_state: Signal<ConnectionState>,
    paths: Signal<Vec<peer::PathInfo>>,
    rtt_history: Signal<VecDeque<f64>>,
    event_log: Signal<VecDeque<EventEntry>>,
    net_report_state: Signal<DiagState<NetReportSummary>>,
    portmap_state: Signal<DiagState<portmap_probe::PortMapProbeResult>>,
    relays_state: Signal<DiagState<Vec<relay_probe::RelayProbeResult>>>,
    ttfdb: Signal<Option<Duration>>,
    blobs_list: Signal<Vec<peer::BlobSummary>>,
    endpoints_list: Signal<Vec<endpoints::Endpoint>>,
) {
    let (net_report, _) = diagnostics_export::Snapshot::extract_diag_state(&net_report_state());
    let (portmap, portmap_err) = diagnostics_export::Snapshot::extract_diag_state(&portmap_state());
    let (relays, relays_err) = diagnostics_export::Snapshot::extract_diag_state(&relays_state());

    let snapshot = diagnostics_export::Snapshot {
        error_message: format!("[{}] {}", err.source, err.message),
        endpoint_id: endpoint_id(),
        conn_state_label: short_event_label(&conn_state()),
        paths: paths(),
        rtt_history: rtt_history(),
        events: event_log(),
        net_report,
        portmap,
        portmap_err,
        relays: relays.unwrap_or_default(),
        relays_err,
        ttfdb: ttfdb(),
        blobs: blobs_list(),
        endpoints: endpoints_list(),
        log_dir: log_dir(),
    };
    spawn(async move {
        let bytes = match diagnostics_export::build_zip(&snapshot) {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(err = %e, "building diagnostics zip");
                return;
            }
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let filename = format!("iroh-doctor-app-diagnostics-{now}.zip");
        save_diagnostics_zip(&filename, &bytes).await;
    });
}

/// Desktop path: open a native save dialog via rfd and write the bytes
/// to whichever location the user picks.
#[cfg(not(target_os = "ios"))]
async fn save_diagnostics_zip(filename: &str, bytes: &[u8]) {
    let dialog = rfd::AsyncFileDialog::new()
        .set_file_name(filename)
        .set_title("Save iroh-doctor-app diagnostics");
    if let Some(handle) = dialog.save_file().await {
        if let Err(e) = handle.write(bytes).await {
            tracing::error!(err = %e, "writing diagnostics zip");
        }
    }
}

/// iOS path: rfd has no backend on iOS, so write to the app's sandbox
/// documents directory where the Files app exposes it. The user can
/// share the file from there. Logs the resulting path so a developer
/// inspecting the log file can find it without guessing.
#[cfg(target_os = "ios")]
async fn save_diagnostics_zip(filename: &str, bytes: &[u8]) {
    let Some(dir) = dirs::document_dir().or_else(dirs::data_local_dir) else {
        tracing::error!("no document directory on this iOS build");
        return;
    };
    let path = dir.join(filename);
    if let Err(e) = std::fs::write(&path, bytes) {
        tracing::error!(err = %e, path = %path.display(), "writing diagnostics zip");
        return;
    }
    tracing::info!(path = %path.display(), "wrote diagnostics zip");
}

#[component]
fn Nav(current_tab: Signal<Tab>) -> Element {
    let active = current_tab();
    rsx! {
        nav { class: "nav",
            NavItem {
                label: "Diagnostics",
                icon: "⌁",
                is_active: active == Tab::Diagnostics,
                on_select: move |_| current_tab.clone().set(Tab::Diagnostics),
            }
            NavItem {
                label: "Data",
                icon: "◇",
                is_active: active == Tab::Data,
                on_select: move |_| current_tab.clone().set(Tab::Data),
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
            NavItem {
                label: "Pong",
                icon: "⏵",
                is_active: active == Tab::Pong,
                on_select: move |_| current_tab.clone().set(Tab::Pong),
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
fn DiagnosticsPage(
    endpoint_id: Signal<String>,
    conn_state: Signal<ConnectionState>,
    cmd_handle: Signal<Option<PeerHandle>>,
    peer_id_input: Signal<String>,
    telemetry: Signal<TelemetryState>,
    services_ping_state: Signal<DiagState<Duration>>,
    net_state: Signal<DiagState<DiagnosticsReport>>,
    net_report_state: Signal<DiagState<NetReportSummary>>,
    relays_state: Signal<DiagState<Vec<relay_probe::RelayProbeResult>>>,
    portmap_state: Signal<DiagState<portmap_probe::PortMapProbeResult>>,
    paths: Signal<Vec<PathInfo>>,
    rtt_history: Signal<VecDeque<f64>>,
    event_log: Signal<VecDeque<EventEntry>>,
    ttfdb: Signal<Option<Duration>>,
) -> Element {
    rsx! {
        div { class: "page",
            h2 { class: "page-title", "Diagnostics" }
            Header { endpoint_id }
            ConnectBar { cmd_handle, peer_id_input }
            DiagnosticsView {
                cmd_handle,
                conn_state,
                telemetry,
                services_state: services_ping_state,
                net_state,
                net_report_state,
                relays_state,
                portmap_state,
                paths,
                rtt_history,
                event_log,
                ttfdb,
            }
        }
    }
}

#[component]
fn PongPage(
    game: Signal<PongGame>,
    endpoint_id: Signal<String>,
    conn_state: Signal<ConnectionState>,
    cmd_handle: Signal<Option<PeerHandle>>,
) -> Element {
    rsx! {
        div { class: "page",
            h2 { class: "page-title", "Pong" }
            PongScene { game, endpoint_id, conn_state, cmd_handle }
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
fn ConnectBar(cmd_handle: Signal<Option<PeerHandle>>, peer_id_input: Signal<String>) -> Element {
    let input_value = peer_id_input();
    let connect_disabled = !peer::looks_like_endpoint_id(&input_value);

    rsx! {
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
                        let _ = handle.tx.try_send(PeerCommand::Connect { hex_id: id });
                    }
                },
                "Connect"
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
    #[cfg(any(feature = "desktop", feature = "mobile"))]
    {
        let text = _text.to_string();
        dioxus::prelude::document::eval(&format!(
            "navigator.clipboard.writeText({});",
            serde_escape(&text)
        ));
    }
}

#[cfg(any(feature = "desktop", feature = "mobile"))]
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
