//! iroh endpoint binding, accept loop, per-session send/recv tasks, services telemetry.
//!
//! Mirrors the Swift `IrohPeer` + `PeerSession`: connector becomes ball authority,
//! both peers stream tagged paddle frames at ~60Hz, authority additionally streams
//! ball frames.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use iroh::endpoint::{self, presets};
use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey};
use iroh_blobs::store::mem::MemStore;
use iroh_blobs::BlobsProtocol;
use iroh_docs::protocol::Docs;
use iroh_gossip::net::Gossip;
use iroh_services::Client as ServicesClient;
use rand::thread_rng;
use tokio::sync::{mpsc, oneshot, Mutex, Semaphore};
use tokio::task::JoinHandle;
use tracing::{debug, info, instrument, warn};

use crate::game::PongGame;
use crate::wire;

pub const DEFAULT_API_SECRET: &str =
    "servicesaaqg6nnf7kr3uiacviqgbxeqconvhuz4ldr5dem4gqhsp3cyat6qxexoctwjsi7m6dh2t2qvfu2yhdoaav6eibaj4aaavhonlixbohceu4aa";

#[derive(Debug, Clone)]
pub enum ConnectionState {
    Idle,
    Binding,
    Ready,
    Connecting,
    Connected {
        peer_id: String,
        peer_short_id: String,
    },
    /// The remote side closed the session (clean QUIC close, app shut
    /// down, or the device went away). We hold the id around so the UI
    /// can show "<peer> disconnected" until the next state change.
    PeerDisconnected {
        peer_short_id: String,
    },
    Error(String),
}

#[derive(Debug, Clone)]
pub enum TelemetryState {
    Off,
    Starting,
    Active { name: String },
    Error(String),
}

pub enum PeerCommand {
    Connect {
        hex_id: String,
    },
    SaveApiSecret {
        secret: String,
    },
    UpdateMyPaddle {
        x: f32,
    },
    PingServices {
        reply: oneshot::Sender<Result<Duration, String>>,
    },
    RunNetDiagnostics {
        reply: oneshot::Sender<Result<DiagnosticsReport, String>>,
    },
    /// Probes the local network with iroh's built-in net reporter and
    /// returns a UI-facing summary that includes a NAT classification.
    /// Independent of iroh-services so the user can run it even with the
    /// services API offline or with no key configured.
    ProbeNetReport {
        reply: oneshot::Sender<Result<NetReportSummary, String>>,
    },
    /// Probes every relay in the active relay map once, measuring TLS
    /// connect time and a single relay-protocol ping. The reply is a row
    /// per relay, sorted by ping with failures last.
    ProbeRelayLatencies {
        reply: oneshot::Sender<Result<Vec<crate::relay_probe::RelayProbeResult>, String>>,
    },
    /// Probes the local gateway directly for UPnP, PCP, and NAT-PMP
    /// support. Reports the same tri-state booleans as the services-
    /// based net diagnostics, but without involving the services API.
    ProbePortMap {
        reply: oneshot::Sender<Result<crate::portmap_probe::PortMapProbeResult, String>>,
    },
    /// Generates a blob of the requested size and adds it to the local
    /// store. The reply carries a [`BlobSummary`] with the resulting hash
    /// and how long the add took. Sizes greater than [`MAX_BLOB_BYTES`]
    /// are rejected at the command layer.
    AddBlob {
        size_bytes: u64,
        reply: oneshot::Sender<Result<BlobSummary, String>>,
    },
    /// Pulls a blob by hash from the given peer into the local store.
    /// `progress_tx` receives cumulative byte offsets while the download
    /// runs; the reply fires once with the final summary (or an error).
    PullBlob {
        peer: String,
        hash: String,
        progress_tx: mpsc::Sender<u64>,
        reply: oneshot::Sender<Result<BlobSummary, String>>,
    },
    /// Joins an iroh-gossip topic. `topic_input` is parsed as 64-hex if it
    /// matches that shape, otherwise hashed with BLAKE3 so any string
    /// becomes a deterministic topic. Any previously joined topic is
    /// dropped.
    JoinGossip {
        topic_input: String,
        bootstrap: Vec<String>,
        events_tx: mpsc::Sender<GossipEventUi>,
        reply: oneshot::Sender<Result<String, String>>,
    },
    /// Broadcasts `msg` (UTF-8) on the currently-joined gossip topic.
    GossipBroadcast {
        msg: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Creates a new iroh-docs document, subscribes to its live events,
    /// and installs it as the active doc. The reply carries the new
    /// namespace id.
    CreateDoc {
        events_tx: mpsc::Sender<DocEventUi>,
        reply: oneshot::Sender<Result<String, String>>,
    },
    /// Imports a doc from a write ticket, subscribes to events, and
    /// installs it as the active doc. The reply carries the namespace id.
    ImportDoc {
        ticket: String,
        events_tx: mpsc::Sender<DocEventUi>,
        reply: oneshot::Sender<Result<String, String>>,
    },
    /// Produces a write ticket for the active doc. The reply carries the
    /// ticket string and errors if no doc is active.
    ShareDoc {
        reply: oneshot::Sender<Result<String, String>>,
    },
    /// Writes `value` at `key` on the active doc using the default author.
    /// The reply carries the resulting content hash hex.
    SetDocEntry {
        key: String,
        value: String,
        reply: oneshot::Sender<Result<String, String>>,
    },
    /// Returns a snapshot of every entry currently in the active doc.
    /// Values are read from the local blobs store; an entry whose content
    /// hasn't synced yet returns `value: None`.
    ListDocEntries {
        reply: oneshot::Sender<Result<Vec<DocEntrySummary>, String>>,
    },
}

/// Snapshot of one doc entry shaped for the UI.
#[derive(Debug, Clone)]
pub struct DocEntrySummary {
    pub key: String,
    /// Lossy UTF-8 view of the value, truncated to `MAX_VALUE_PREVIEW_BYTES`.
    /// `None` when the entry exists but the content hasn't synced locally.
    pub value: Option<String>,
    pub content_hash: String,
    pub content_len: u64,
}

/// Subset of [`iroh_docs::engine::LiveEvent`] reshaped for the Docs tab.
#[derive(Debug, Clone)]
pub enum DocEventUi {
    InsertLocal {
        key: String,
        value_hash: String,
    },
    InsertRemote {
        key: String,
        value_hash: String,
        from: String,
    },
    ContentReady {
        hash: String,
    },
    PendingContentReady,
    NeighborUp {
        peer: String,
    },
    NeighborDown {
        peer: String,
    },
    SyncFinished {
        peer: String,
    },
}

/// Subset of [`iroh_gossip::api::Event`] reshaped for the UI. Message
/// bodies are decoded as UTF-8 lossily so the Gossip tab can render them
/// without a separate decode step.
#[derive(Debug, Clone)]
pub enum GossipEventUi {
    NeighborUp { peer: String },
    NeighborDown { peer: String },
    Message { from: String, body: String },
    Lagged,
}

/// One blob's metadata as shown in the Blobs tab.
#[derive(Debug, Clone)]
pub struct BlobSummary {
    /// Hex-encoded BLAKE3 hash.
    pub hash: String,
    /// Size in bytes. `u64` so we can represent multi-GiB blobs on any
    /// target.
    pub size_bytes: u64,
    pub kind: BlobKind,
    /// Wall-clock duration in milliseconds for the operation that produced
    /// this row (add for `Generated`, download for `Pulled`).
    pub elapsed_ms: f64,
}

/// Upper bound on the size of a single generated blob. Two GiB is far past
/// any reasonable debug test and well below the point where the MemStore
/// would crowd out the rest of the process on a typical laptop.
pub const MAX_BLOB_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Validates a requested blob size against [`MAX_BLOB_BYTES`]. Returns the
/// size as a `usize` for use with `Vec::with_capacity`, or a UI-facing
/// error string. Extracted as a free function so the cap behaviour is
/// covered by unit tests without a real iroh-blobs `Store`.
fn check_blob_size(size_bytes: u64) -> Result<usize, String> {
    if size_bytes == 0 {
        return Err("size must be greater than zero".into());
    }
    if size_bytes > MAX_BLOB_BYTES {
        return Err(format!(
            "requested {} exceeds the {} cap",
            crate::components::format_bytes_iec(size_bytes),
            crate::components::format_bytes_iec(MAX_BLOB_BYTES),
        ));
    }
    usize::try_from(size_bytes)
        .map_err(|_| format!("size {size_bytes} does not fit in usize on this target"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobKind {
    Generated,
    Pulled,
}

#[derive(Debug, Clone)]
pub struct DiagnosticsReport {
    pub endpoint_id: String,
    pub direct_addrs: Vec<String>,
    pub iroh_version: String,
    pub iroh_services_version: String,
    pub has_net_report: bool,
    pub upnp: Option<bool>,
    pub pcp: Option<bool>,
    pub nat_pmp: Option<bool>,
}

/// Direct snapshot of `iroh::NetReport`, reshaped for the Diagnostics tab.
/// Captures the NAT classification, IPv4 and IPv6 visibility, and the
/// preferred relay so the tab can show a one-glance summary of the local
/// network environment without going through iroh-services.
#[derive(Debug, Clone)]
pub struct NetReportSummary {
    pub nat: iroh_doctor_core::nat::NatType,
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

impl From<&iroh::NetReport> for NetReportSummary {
    fn from(r: &iroh::NetReport) -> Self {
        Self {
            nat: iroh_doctor_core::nat::classify_base_report(r),
            global_v4: r.global_v4.map(|a| a.to_string()),
            global_v6: r.global_v6.map(|a| a.to_string()),
            udp_v4: r.udp_v4,
            udp_v6: r.udp_v6,
            mapping_varies_v4: r.mapping_varies_by_dest_ipv4,
            mapping_varies_v6: r.mapping_varies_by_dest_ipv6,
            captive_portal: r.captive_portal,
            preferred_relay: r.preferred_relay.as_ref().map(|u| u.to_string()),
            relays_seen: r.relay_latency.iter().count(),
        }
    }
}

impl From<iroh_services::net_diagnostics::DiagnosticsReport> for DiagnosticsReport {
    fn from(r: iroh_services::net_diagnostics::DiagnosticsReport) -> Self {
        let (upnp, pcp, nat_pmp) = match r.portmap_probe {
            Some(p) => (Some(p.upnp), Some(p.pcp), Some(p.nat_pmp)),
            None => (None, None, None),
        };
        Self {
            endpoint_id: r.endpoint_id.to_string(),
            direct_addrs: r.direct_addrs.into_iter().map(|s| s.to_string()).collect(),
            iroh_version: r.iroh_version,
            iroh_services_version: r.iroh_services_version,
            has_net_report: r.net_report.is_some(),
            upnp,
            pcp,
            nat_pmp,
        }
    }
}

type StateCb = Arc<dyn Fn(ConnectionState) + Send + Sync>;
type TelemetryCb = Arc<dyn Fn(TelemetryState) + Send + Sync>;
type GameCb = Arc<dyn Fn(PongGame) + Send + Sync>;
type IdCb = Arc<dyn Fn(String) + Send + Sync>;
type PathsCb = Arc<dyn Fn(Vec<PathInfo>) + Send + Sync>;
type TtfdbCb = Arc<dyn Fn(Option<Duration>) + Send + Sync>;
type ThroughputCb = Arc<dyn Fn(ThroughputSnapshot) + Send + Sync>;

/// Snapshot of one completed upload from the peer probe (`iroh-doctor connect`
/// monitor). `bytes` and `elapsed` come straight from the responder's
/// [`iroh_doctor_core::probe::ProbeEvent::UploadCompleted`]; `mbps` is the
/// pre-computed convenience value so the UI does not have to redo the
/// math for every render.
#[derive(Debug, Clone, PartialEq)]
pub struct ThroughputSnapshot {
    pub bytes: u64,
    pub elapsed: Duration,
    pub mbps: Option<f64>,
}

/// Callbacks the peer task invokes to push state into the UI. Bundling them
/// keeps `run_peer`'s signature compact and gives future commits an obvious
/// place to add new event channels.
pub struct PeerCallbacks {
    pub on_endpoint_id: Box<dyn Fn(String) + Send + Sync>,
    pub on_state: Box<dyn Fn(ConnectionState) + Send + Sync>,
    pub on_telemetry: Box<dyn Fn(TelemetryState) + Send + Sync>,
    pub on_game: Box<dyn Fn(PongGame) + Send + Sync>,
    pub on_paths: Box<dyn Fn(Vec<PathInfo>) + Send + Sync>,
    /// Fires once per dial with the elapsed time from `Connect` to the
    /// first selected direct (holepunched) path, or with `None` to
    /// clear the metric when a fresh dial begins.
    pub on_ttfdb: Box<dyn Fn(Option<Duration>) + Send + Sync>,
    /// Fires every time the responder side of an incoming peer-probe
    /// completes an upload. Carries the byte count, the drain time, and a
    /// pre-formatted Mbps value.
    pub on_throughput: Box<dyn Fn(ThroughputSnapshot) + Send + Sync>,
}

/// Shared state tracking the "time to first direct byte" measurement.
/// `dial_at` is set when the user clicks Connect; `published` flips to
/// true once the sampler observes the first selected direct path, so
/// the metric is reported exactly once per dial.
struct TtfdbState {
    dial_at: Option<std::time::Instant>,
    published: bool,
}

/// How often `run_peer` samples the pong connection's QUIC paths for the
/// Debug view. Short enough to feel live, long enough to keep overhead in
/// the noise floor.
const PATHS_SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

/// Wall-clock ceiling for `endpoint.net_report().initialized()`. Matches
/// `iroh-doctor::commands::probe::NET_REPORT_TIMEOUT` so the app and the
/// CLI give up at the same point on a flaky network.
const NET_REPORT_TIMEOUT: Duration = Duration::from_secs(15);

/// Classification of a QUIC path's remote address. Mirrors the variants on
/// [`iroh_base::TransportAddr`] but lives here so the UI does not need to
/// depend on iroh-base directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    Ip,
    Relay,
    Custom,
}

/// Snapshot of one QUIC path on the live pong connection.
#[derive(Debug, Clone, PartialEq)]
pub struct PathInfo {
    /// `TransportAddr` rendered via `Display` (`ip:1.2.3.4:5678`,
    /// `relay:https://...`, or `custom:...`).
    pub addr: String,
    pub kind: PathKind,
    pub selected: bool,
    pub rtt_ms: f64,
}

pub async fn run_peer(
    secret_key: SecretKey,
    initial_api_secret_override: String,
    mut commands: mpsc::Receiver<PeerCommand>,
    callbacks: PeerCallbacks,
) -> Result<()> {
    let on_endpoint_id: IdCb = Arc::from(callbacks.on_endpoint_id);
    let on_state: StateCb = Arc::from(callbacks.on_state);
    let on_telemetry: TelemetryCb = Arc::from(callbacks.on_telemetry);
    let on_game: GameCb = Arc::from(callbacks.on_game);
    let on_paths: PathsCb = Arc::from(callbacks.on_paths);
    let on_ttfdb: TtfdbCb = Arc::from(callbacks.on_ttfdb);
    let on_throughput: ThroughputCb = Arc::from(callbacks.on_throughput);

    let ttfdb: Arc<Mutex<TtfdbState>> = Arc::new(Mutex::new(TtfdbState {
        dial_at: None,
        published: false,
    }));

    on_state(ConnectionState::Binding);

    let endpoint = match bind_endpoint(secret_key).await {
        Ok(ep) => ep,
        Err(e) => {
            on_state(ConnectionState::Error(format!("bind failed: {e:#}")));
            return Err(e);
        }
    };
    on_endpoint_id(endpoint.id().to_string());
    on_state(ConnectionState::Ready);

    let game: Arc<Mutex<PongGame>> = Arc::new(Mutex::new(PongGame::default()));

    let mut api_secret_override = initial_api_secret_override;
    let mut services: Option<ServicesClient> =
        start_services_client(&endpoint, &api_secret_override, &on_telemetry).await;

    // Owners for the additional protocols. Each is cheaply cloneable; clones
    // share state and outlive any individual connection.
    let blobs_store = MemStore::new();
    let blobs = BlobsProtocol::new(&blobs_store, None);
    let gossip = Gossip::builder().spawn(endpoint.clone());
    let docs = Docs::memory()
        .spawn(endpoint.clone(), (*blobs_store).clone(), gossip.clone())
        .await
        .context("spawn iroh-docs")?;
    info!("multi-protocol endpoint ready (pong, blobs, gossip, docs)");

    let session: Arc<Mutex<Option<SessionHandles>>> = Arc::new(Mutex::new(None));
    let conn_slot: Arc<Mutex<Option<endpoint::Connection>>> = Arc::new(Mutex::new(None));

    // Long-running tasks (accept loop, paths sampler) live on a JoinSet
    // owned by run_peer so they shut down cleanly when the command pump
    // exits. Short-lived per-command spawns stay as bare `tokio::spawn`;
    // they complete and clean up on their own.
    let mut long_lived: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

    // Per-process cap on parallel peer-probe handlers. The probe ALPN
    // has no auth, so this is the only thing standing between the app
    // and a malicious peer opening N probe sessions in parallel.
    let probe_limit = Arc::new(Semaphore::new(MAX_CONCURRENT_PROBE_SERVERS));

    // Accept loop.
    {
        let ctx = AcceptCtx {
            endpoint: endpoint.clone(),
            game: game.clone(),
            session: session.clone(),
            conn_slot: conn_slot.clone(),
            on_state: on_state.clone(),
            on_game: on_game.clone(),
            on_throughput: on_throughput.clone(),
            ttfdb: ttfdb.clone(),
            blobs: blobs.clone(),
            gossip: gossip.clone(),
            docs: docs.clone(),
            probe_limit: probe_limit.clone(),
        };
        long_lived.spawn(async move { run_accept_loop(ctx).await });
    }

    // State for the gossip-join command: at most one topic is active at a
    // time. JoinGossip aborts the receiver task and drops the sender slot
    // before installing a new one.
    let gossip_recv_handle: Arc<Mutex<Option<JoinHandle<()>>>> = Arc::new(Mutex::new(None));
    let gossip_sender_slot: Arc<Mutex<Option<iroh_gossip::api::GossipSender>>> =
        Arc::new(Mutex::new(None));

    // State for the docs commands: at most one document is active at a time,
    // and the doc-events subscriber lives on `docs_events_handle` so we can
    // abort it when switching to a different doc.
    let docs_active: Arc<Mutex<Option<iroh_docs::api::Doc>>> = Arc::new(Mutex::new(None));
    let docs_events_handle: Arc<Mutex<Option<JoinHandle<()>>>> = Arc::new(Mutex::new(None));

    // Paths sampler: every PATHS_SAMPLE_INTERVAL, snapshot the active pong
    // connection's QUIC paths. We only notify the UI when the snapshot
    // actually changed (RTT, selected path, or set of addresses) so an
    // idle endpoint does not re-wake the Dioxus event pump twice a second.
    //
    // The sampler also detects the first time a selected direct path
    // appears and reports the elapsed time since `Connect` was issued.
    // QUIC migrates to a new path only after path validation succeeds,
    // so this is a useful proxy for "first byte over a holepunched
    // path" without reaching into iroh internals. Precise per-packet
    // timing would need an iroh-side hook.
    {
        let conn_slot = conn_slot.clone();
        let on_paths = on_paths.clone();
        let ttfdb = ttfdb.clone();
        let on_ttfdb = on_ttfdb.clone();
        long_lived.spawn(async move {
            let mut ticker = tokio::time::interval(PATHS_SAMPLE_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut last: Vec<PathInfo> = Vec::new();
            loop {
                ticker.tick().await;
                let snapshot: Vec<PathInfo> = {
                    let slot = conn_slot.lock().await;
                    slot.as_ref().map(snapshot_paths).unwrap_or_default()
                };
                if snapshot != last {
                    on_paths(snapshot.clone());
                    last = snapshot.clone();
                }
                let direct_selected = snapshot
                    .iter()
                    .any(|p| p.selected && p.kind == PathKind::Ip);
                if direct_selected {
                    let mut state = ttfdb.lock().await;
                    if let (Some(at), false) = (state.dial_at, state.published) {
                        let elapsed = at.elapsed();
                        state.published = true;
                        on_ttfdb(Some(elapsed));
                    }
                }
            }
        });
    }

    // Command pump. When `commands` closes, this loop falls through and
    // proceeds to shut the long-lived JoinSet down. Per-command spawn
    // tasks below are still bare `tokio::spawn`; they self-cleanup on
    // completion. A future expansion could route them through the same
    // JoinSet at the cost of periodic drain to bound memory.
    while let Some(cmd) = commands.recv().await {
        match cmd {
            PeerCommand::UpdateMyPaddle { x } => {
                let snap = {
                    let mut g = game.lock().await;
                    g.set_my_paddle(x);
                    *g
                };
                on_game(snap);
            }
            PeerCommand::Connect { hex_id } => {
                on_state(ConnectionState::Connecting);
                let trimmed = hex_id.trim().to_string();
                let parsed = match EndpointId::from_str(&trimmed) {
                    Ok(p) => p,
                    Err(_) => {
                        on_state(ConnectionState::Error("invalid endpoint id".into()));
                        continue;
                    }
                };
                // Reset the TTFDB measurement for this fresh dial so the
                // sampler reports the next selected direct path against this
                // start time, and the UI clears any value from a prior run.
                {
                    let mut state = ttfdb.lock().await;
                    state.dial_at = Some(std::time::Instant::now());
                    state.published = false;
                }
                on_ttfdb(None);
                let addr = EndpointAddr::from_parts(parsed, std::iter::empty());
                // Connect + open_bi can take seconds for hole punching. Spawn so
                // the command pump stays responsive to paddle, ping, and blob
                // commands while this is in flight.
                let endpoint = endpoint.clone();
                let game = game.clone();
                let session = session.clone();
                let conn_slot = conn_slot.clone();
                let on_state = on_state.clone();
                let on_game = on_game.clone();
                tokio::spawn(async move {
                    match endpoint.connect(addr, wire::ALPN).await {
                        Ok(conn) => match conn.open_bi().await {
                            Ok((send, recv)) => {
                                let remote = conn.remote_id().to_string();
                                adopt_session(AdoptArgs {
                                    conn,
                                    send,
                                    recv,
                                    remote_id: remote,
                                    as_authority: true,
                                    game,
                                    session,
                                    conn_slot,
                                    on_state,
                                    on_game,
                                })
                                .await;
                            }
                            Err(e) => {
                                on_state(ConnectionState::Error(format!("open_bi failed: {e:#}")));
                            }
                        },
                        Err(e) => {
                            on_state(ConnectionState::Error(format!("connect failed: {e:#}")));
                        }
                    }
                });
            }
            PeerCommand::SaveApiSecret { secret } => {
                api_secret_override = secret.trim().to_string();
                services.take(); // drop the old client so its background tasks stop
                services =
                    start_services_client(&endpoint, &api_secret_override, &on_telemetry).await;
            }
            PeerCommand::PingServices { reply } => {
                // services.ping() is a network round-trip to the iroh
                // services endpoint; spawn so it does not block paddle
                // updates or any other command for its duration.
                let client = services.clone();
                tokio::spawn(async move {
                    let result = match client {
                        Some(c) => {
                            let start = std::time::Instant::now();
                            match c.ping().await {
                                Ok(_) => Ok(start.elapsed()),
                                Err(e) => Err(format!("{e:#}")),
                            }
                        }
                        None => Err("services client not initialized".into()),
                    };
                    let _ = reply.send(result);
                });
            }
            PeerCommand::RunNetDiagnostics { reply } => {
                // net_diagnostics probes external services and can take
                // seconds; spawn so the command pump stays responsive.
                let client = services.clone();
                tokio::spawn(async move {
                    let result = match client {
                        Some(c) => match c.net_diagnostics(false).await {
                            Ok(report) => Ok(DiagnosticsReport::from(report)),
                            Err(e) => Err(format!("{e:#}")),
                        },
                        None => Err("services client not initialized".into()),
                    };
                    let _ = reply.send(result);
                });
            }
            PeerCommand::ProbeNetReport { reply } => {
                // Probe via iroh's own NetReport so the result is independent
                // of the iroh-services API state. The endpoint reporter
                // streams a fresh report as conditions change; we await the
                // first non-None value with a ceiling timeout so the UI can
                // surface a clear error instead of hanging.
                //
                // Note: `Watcher::initialized()` returns the cached value on
                // subsequent calls. The UI's refresh button therefore shows
                // whatever the iroh-internal reporter has most recently
                // observed; it does not force a fresh probe. iroh's reporter
                // updates on its own schedule.
                let endpoint = endpoint.clone();
                tokio::spawn(async move {
                    use iroh::Watcher as _;
                    let result = match tokio::time::timeout(
                        NET_REPORT_TIMEOUT,
                        endpoint.net_report().initialized(),
                    )
                    .await
                    {
                        Ok(report) => Ok(NetReportSummary::from(&report)),
                        Err(_) => Err(format!(
                            "net_report did not complete within {}s",
                            NET_REPORT_TIMEOUT.as_secs()
                        )),
                    };
                    let _ = reply.send(result);
                });
            }
            PeerCommand::ProbeRelayLatencies { reply } => {
                // Probe every relay in the same map iroh's default relay
                // mode resolves to. The bound endpoint is built with
                // `presets::N0` which uses `default_relay_mode()`, so
                // the two agree by construction. If `bind_endpoint`
                // ever switches to a custom relay map this code will
                // probe the wrong set silently; a future change should
                // plumb the bound RelayMap through `run_peer` instead.
                let relay_map = iroh::endpoint::default_relay_mode().relay_map();
                tokio::spawn(async move {
                    let rows = crate::relay_probe::probe(relay_map).await;
                    if rows.is_empty() {
                        let _ = reply.send(Err("no relays configured".into()));
                    } else {
                        let _ = reply.send(Ok(rows));
                    }
                });
            }
            PeerCommand::ProbePortMap { reply } => {
                // Direct port-mapping probe. The portmapper crate spawns
                // background gateway calls; we bound the whole thing with
                // a timeout inside `probe()`.
                tokio::spawn(async move {
                    let result = crate::portmap_probe::probe().await;
                    let _ = reply.send(Ok(result));
                });
            }
            PeerCommand::AddBlob { size_bytes, reply } => {
                let store = (*blobs_store).clone();
                tokio::spawn(async move {
                    let result = generate_and_store_blob(store, size_bytes).await;
                    let _ = reply.send(result);
                });
            }
            PeerCommand::PullBlob {
                peer,
                hash,
                progress_tx,
                reply,
            } => {
                let store = (*blobs_store).clone();
                let endpoint = endpoint.clone();
                tokio::spawn(async move {
                    let result = pull_blob(store, endpoint, peer, hash, progress_tx).await;
                    let _ = reply.send(result);
                });
            }
            PeerCommand::JoinGossip {
                topic_input,
                bootstrap,
                events_tx,
                reply,
            } => {
                let gossip = gossip.clone();
                let recv_handle = gossip_recv_handle.clone();
                let sender_slot = gossip_sender_slot.clone();
                tokio::spawn(async move {
                    let result = join_gossip(
                        gossip,
                        recv_handle,
                        sender_slot,
                        topic_input,
                        bootstrap,
                        events_tx,
                    )
                    .await;
                    let _ = reply.send(result);
                });
            }
            PeerCommand::GossipBroadcast { msg, reply } => {
                let sender_slot = gossip_sender_slot.clone();
                tokio::spawn(async move {
                    let sender = sender_slot.lock().await.clone();
                    let result = match sender {
                        Some(s) => s
                            .broadcast(bytes::Bytes::from(msg.into_bytes()))
                            .await
                            .map_err(|e| format!("{e:#}")),
                        None => Err("not joined to any topic".into()),
                    };
                    let _ = reply.send(result);
                });
            }
            PeerCommand::CreateDoc { events_tx, reply } => {
                let docs = docs.clone();
                let active = docs_active.clone();
                let events_handle = docs_events_handle.clone();
                tokio::spawn(async move {
                    let result = create_doc(docs, active, events_handle, events_tx).await;
                    let _ = reply.send(result);
                });
            }
            PeerCommand::ImportDoc {
                ticket,
                events_tx,
                reply,
            } => {
                let docs = docs.clone();
                let active = docs_active.clone();
                let events_handle = docs_events_handle.clone();
                tokio::spawn(async move {
                    let result = import_doc(docs, active, events_handle, ticket, events_tx).await;
                    let _ = reply.send(result);
                });
            }
            PeerCommand::ShareDoc { reply } => {
                let active = docs_active.clone();
                tokio::spawn(async move {
                    let doc = active.lock().await.clone();
                    let result = match doc {
                        Some(d) => share_active_doc(d).await,
                        None => Err("no active doc".into()),
                    };
                    let _ = reply.send(result);
                });
            }
            PeerCommand::SetDocEntry { key, value, reply } => {
                let docs = docs.clone();
                let active = docs_active.clone();
                tokio::spawn(async move {
                    let doc = active.lock().await.clone();
                    let result = match doc {
                        Some(d) => set_doc_entry(docs, d, key, value).await,
                        None => Err("no active doc".into()),
                    };
                    let _ = reply.send(result);
                });
            }
            PeerCommand::ListDocEntries { reply } => {
                let active = docs_active.clone();
                let store = (*blobs_store).clone();
                tokio::spawn(async move {
                    let doc = active.lock().await.clone();
                    let result = match doc {
                        Some(d) => list_doc_entries(d, store).await,
                        None => Err("no active doc".into()),
                    };
                    let _ = reply.send(result);
                });
            }
        }
    }

    // Command pump exited. Abort the accept loop and the paths sampler,
    // then close the endpoint so QUIC sends final shutdown notifications
    // before the runtime drops. `endpoint.close()` waits up to the
    // configured timeout for in-flight streams to drain.
    info!("command pump closed, shutting peer down");
    long_lived.shutdown().await;
    endpoint.close().await;
    Ok(())
}

#[instrument(skip(store), fields(size_bytes))]
async fn generate_and_store_blob(
    store: iroh_blobs::api::Store,
    size_bytes: u64,
) -> Result<BlobSummary, String> {
    use rand::RngCore;

    let size_usize = check_blob_size(size_bytes)?;

    // Eight random bytes at the front make each generation produce a unique
    // hash regardless of size. The rest is zero-filled so we can scale to
    // GiB-sized test blobs without paying the cost of randomizing every byte.
    let mut bytes = vec![0u8; size_usize];
    let salt_len = size_usize.min(8);
    rand::thread_rng().fill_bytes(&mut bytes[..salt_len]);

    let start = std::time::Instant::now();
    let tag_info = store
        .blobs()
        .add_bytes(bytes)
        .await
        .map_err(|e| format!("add_bytes: {e:#}"))?;
    let elapsed = start.elapsed();
    debug!(
        hash = %tag_info.hash,
        elapsed_ms = elapsed.as_secs_f64() * 1000.0,
        "stored generated blob"
    );

    Ok(BlobSummary {
        hash: tag_info.hash.to_string(),
        size_bytes,
        kind: BlobKind::Generated,
        elapsed_ms: elapsed.as_secs_f64() * 1000.0,
    })
}

#[instrument(skip(store, endpoint, progress_tx), fields(peer = %peer, hash = %hash))]
async fn pull_blob(
    store: iroh_blobs::api::Store,
    endpoint: Endpoint,
    peer: String,
    hash: String,
    progress_tx: mpsc::Sender<u64>,
) -> Result<BlobSummary, String> {
    use n0_future::StreamExt;

    let parsed_peer =
        EndpointId::from_str(peer.trim()).map_err(|e| format!("invalid peer endpoint id: {e}"))?;
    let parsed_hash =
        iroh_blobs::Hash::from_str(hash.trim()).map_err(|e| format!("invalid hash: {e}"))?;

    let downloader = store.downloader(&endpoint);
    let start = std::time::Instant::now();
    let mut stream = downloader
        .download(parsed_hash, Some(parsed_peer))
        .stream()
        .await
        .map_err(|e| format!("downloader.stream: {e:#}"))?;

    while let Some(item) = stream.next().await {
        use iroh_blobs::api::downloader::DownloadProgressItem;
        match item {
            DownloadProgressItem::Progress(offset) => {
                // The same cap that gates blob generation also gates pull
                // size, since a malicious or careless peer could otherwise
                // stream into MemStore until the process OOMs. Dropping
                // the stream aborts the underlying request.
                if offset > MAX_BLOB_BYTES {
                    return Err(format!(
                        "pull exceeded {} cap (at {})",
                        crate::components::format_bytes_iec(MAX_BLOB_BYTES),
                        crate::components::format_bytes_iec(offset),
                    ));
                }
                let _ = progress_tx.try_send(offset);
            }
            DownloadProgressItem::PartComplete { .. } => {}
            DownloadProgressItem::TryProvider { .. }
            | DownloadProgressItem::ProviderFailed { .. } => {}
            DownloadProgressItem::Error(e) => {
                return Err(format!("download error: {e}"));
            }
            DownloadProgressItem::DownloadError => {
                return Err("download error".into());
            }
        }
    }

    let elapsed = start.elapsed();
    // Query the store for the actual stored size. The Progress events only
    // fire periodically and may skip sub-chunk blobs entirely, which would
    // make a Progress-derived size unreliable. `await_completion` drains
    // the observe stream until `is_complete()` so the bitfield reflects the
    // post-download state, not the first-emitted snapshot.
    let size_bytes = match store.blobs().observe(parsed_hash).await_completion().await {
        Ok(bitfield) => bitfield.size(),
        Err(e) => {
            warn!(err = %e, "blob observe after download failed; reporting size 0");
            0
        }
    };

    Ok(BlobSummary {
        hash: parsed_hash.to_string(),
        size_bytes,
        kind: BlobKind::Pulled,
        elapsed_ms: elapsed.as_secs_f64() * 1000.0,
    })
}

#[instrument(
    skip(gossip, recv_handle, sender_slot, events_tx, bootstrap),
    fields(topic_input)
)]
async fn join_gossip(
    gossip: Gossip,
    recv_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    sender_slot: Arc<Mutex<Option<iroh_gossip::api::GossipSender>>>,
    topic_input: String,
    bootstrap: Vec<String>,
    events_tx: mpsc::Sender<GossipEventUi>,
) -> Result<String, String> {
    use n0_future::StreamExt;

    let topic_id = parse_topic_id(&topic_input);

    let mut bootstrap_ids = Vec::with_capacity(bootstrap.len());
    for raw in bootstrap {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let id = EndpointId::from_str(trimmed)
            .map_err(|e| format!("invalid bootstrap endpoint id `{trimmed}`: {e}"))?;
        bootstrap_ids.push(id);
    }

    {
        let mut h = recv_handle.lock().await;
        if let Some(prev) = h.take() {
            prev.abort();
        }
    }
    {
        let mut s = sender_slot.lock().await;
        s.take();
    }

    let topic = gossip
        .subscribe_and_join(topic_id, bootstrap_ids)
        .await
        .map_err(|e| format!("subscribe_and_join: {e:#}"))?;
    let (sender, mut receiver) = topic.split();
    {
        let mut s = sender_slot.lock().await;
        *s = Some(sender);
    }

    let handle = tokio::spawn(async move {
        while let Some(item) = receiver.next().await {
            use iroh_gossip::api::Event;
            let event = match item {
                Ok(e) => e,
                Err(err) => {
                    warn!(?err, "gossip receive error");
                    break;
                }
            };
            let ui = match event {
                Event::NeighborUp(peer) => GossipEventUi::NeighborUp {
                    peer: peer.to_string(),
                },
                Event::NeighborDown(peer) => GossipEventUi::NeighborDown {
                    peer: peer.to_string(),
                },
                Event::Received(msg) => GossipEventUi::Message {
                    from: msg.delivered_from.to_string(),
                    body: String::from_utf8_lossy(&msg.content).into_owned(),
                },
                Event::Lagged => GossipEventUi::Lagged,
            };
            if events_tx.try_send(ui).is_err() {
                warn!("gossip events_tx full, dropping event");
            }
        }
    });

    let mut h = recv_handle.lock().await;
    *h = Some(handle);

    Ok(topic_id.to_string())
}

async fn create_doc(
    docs: Docs,
    active: Arc<Mutex<Option<iroh_docs::api::Doc>>>,
    events_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    events_tx: mpsc::Sender<DocEventUi>,
) -> Result<String, String> {
    let doc = docs
        .create()
        .await
        .map_err(|e| format!("docs.create: {e:#}"))?;
    let stream = doc
        .subscribe()
        .await
        .map_err(|e| format!("doc.subscribe: {e:#}"))?;
    install_active_doc(active, events_handle, doc, Box::pin(stream), events_tx).await
}

async fn import_doc(
    docs: Docs,
    active: Arc<Mutex<Option<iroh_docs::api::Doc>>>,
    events_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    ticket_str: String,
    events_tx: mpsc::Sender<DocEventUi>,
) -> Result<String, String> {
    let ticket: iroh_docs::DocTicket = ticket_str
        .trim()
        .parse()
        .map_err(|e| format!("invalid doc ticket: {e}"))?;
    // `import_and_subscribe` ensures no sync events are missed between
    // import and subscribe; threading its stream through to
    // `install_active_doc` avoids opening (and racing) a second
    // subscription.
    let (doc, stream) = docs
        .import_and_subscribe(ticket)
        .await
        .map_err(|e| format!("import_and_subscribe: {e:#}"))?;
    install_active_doc(active, events_handle, doc, Box::pin(stream), events_tx).await
}

#[instrument(skip(active, events_handle, doc, stream, events_tx))]
async fn install_active_doc(
    active: Arc<Mutex<Option<iroh_docs::api::Doc>>>,
    events_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    doc: iroh_docs::api::Doc,
    mut stream: std::pin::Pin<
        Box<dyn n0_future::Stream<Item = anyhow::Result<iroh_docs::engine::LiveEvent>> + Send>,
    >,
    events_tx: mpsc::Sender<DocEventUi>,
) -> Result<String, String> {
    use n0_future::StreamExt;

    let doc_id = doc.id().to_string();

    // Spawn the events pump first. `tokio::spawn` returns the handle
    // immediately; the closure runs concurrently and never blocks the
    // install.
    let handle = tokio::spawn(async move {
        use iroh_docs::engine::LiveEvent;
        while let Some(item) = stream.next().await {
            let event = match item {
                Ok(e) => e,
                Err(err) => {
                    warn!(?err, "doc subscribe error");
                    break;
                }
            };
            let ui = match event {
                LiveEvent::InsertLocal { entry } => DocEventUi::InsertLocal {
                    key: String::from_utf8_lossy(entry.key()).into_owned(),
                    value_hash: entry.content_hash().to_string(),
                },
                LiveEvent::InsertRemote { from, entry, .. } => DocEventUi::InsertRemote {
                    key: String::from_utf8_lossy(entry.key()).into_owned(),
                    value_hash: entry.content_hash().to_string(),
                    from: from.to_string(),
                },
                LiveEvent::ContentReady { hash } => DocEventUi::ContentReady {
                    hash: hash.to_string(),
                },
                LiveEvent::PendingContentReady => DocEventUi::PendingContentReady,
                LiveEvent::NeighborUp(peer) => DocEventUi::NeighborUp {
                    peer: peer.to_string(),
                },
                LiveEvent::NeighborDown(peer) => DocEventUi::NeighborDown {
                    peer: peer.to_string(),
                },
                LiveEvent::SyncFinished(ev) => DocEventUi::SyncFinished {
                    peer: ev.peer.to_string(),
                },
            };
            if events_tx.try_send(ui).is_err() {
                warn!("docs events_tx full, dropping event");
            }
        }
    });

    // Atomic swap: take both locks (always in this order to avoid lock
    // inversion), abort any prior events task, install the new doc and
    // its handle together. A concurrent install racing this one
    // serializes on `events_handle.lock()` and either runs before or
    // after, never interleaved.
    let mut h = events_handle.lock().await;
    let mut a = active.lock().await;
    if let Some(prev) = h.take() {
        prev.abort();
    }
    *h = Some(handle);
    *a = Some(doc);

    Ok(doc_id)
}

async fn share_active_doc(doc: iroh_docs::api::Doc) -> Result<String, String> {
    use iroh_docs::api::protocol::{AddrInfoOptions, ShareMode};
    let ticket = doc
        .share(ShareMode::Write, AddrInfoOptions::Id)
        .await
        .map_err(|e| format!("doc.share: {e:#}"))?;
    Ok(ticket.to_string())
}

/// Hard cap on doc-entry value length. Doc entries land in the in-memory
/// store too, so a long paste behaves the same way as a giant blob
/// generation: it consumes RAM until the process exits. One MiB is large
/// for any reasonable debug case.
const MAX_DOC_VALUE_BYTES: usize = 1024 * 1024;

/// Cap on the bytes we read + render per entry in the UI list. Values
/// can be up to [`MAX_DOC_VALUE_BYTES`]; we don't want a single 1 MiB
/// paste to drag the renderer or blow up the IPC payload.
const MAX_VALUE_PREVIEW_BYTES: u64 = 4 * 1024;

async fn list_doc_entries(
    doc: iroh_docs::api::Doc,
    store: iroh_blobs::api::Store,
) -> Result<Vec<DocEntrySummary>, String> {
    use iroh_docs::store::Query;
    use n0_future::StreamExt;

    let stream = doc
        .get_many(Query::all())
        .await
        .map_err(|e| format!("get_many: {e:#}"))?;
    let mut stream = Box::pin(stream);

    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        let entry = item.map_err(|e| format!("entry: {e:#}"))?;
        let key = String::from_utf8_lossy(entry.key()).into_owned();
        let content_hash = entry.content_hash();
        let content_len = entry.content_len();

        // Only try to read content that's small enough to render and small
        // enough to be cheap. For larger entries we still show key + hash.
        let value = if content_len > 0 && content_len <= MAX_VALUE_PREVIEW_BYTES {
            match store.get_bytes(content_hash).await {
                Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
                Err(_) => None,
            }
        } else {
            None
        };

        out.push(DocEntrySummary {
            key,
            value,
            content_hash: content_hash.to_string(),
            content_len,
        });
    }
    Ok(out)
}

async fn set_doc_entry(
    docs: Docs,
    doc: iroh_docs::api::Doc,
    key: String,
    value: String,
) -> Result<String, String> {
    if key.is_empty() {
        return Err("key must not be empty".into());
    }
    if value.len() > MAX_DOC_VALUE_BYTES {
        return Err(format!(
            "value length {} exceeds the {} cap",
            crate::components::format_bytes_iec(value.len() as u64),
            crate::components::format_bytes_iec(MAX_DOC_VALUE_BYTES as u64),
        ));
    }
    let author = docs
        .author_default()
        .await
        .map_err(|e| format!("author_default: {e:#}"))?;
    let hash = doc
        .set_bytes(author, key.into_bytes(), value.into_bytes())
        .await
        .map_err(|e| format!("set_bytes: {e:#}"))?;
    Ok(hash.to_string())
}

fn parse_topic_id(input: &str) -> iroh_gossip::proto::TopicId {
    let trimmed = input.trim();
    if trimmed.len() == 64 {
        if let Ok(id) = trimmed.parse::<iroh_gossip::proto::TopicId>() {
            return id;
        }
    }
    // Hash the user-supplied string with BLAKE3 so any friendly name maps
    // deterministically to a topic both peers can compute.
    let hash = iroh_blobs::Hash::new(trimmed.as_bytes());
    iroh_gossip::proto::TopicId::from_bytes(*hash.as_bytes())
}

async fn bind_endpoint(secret_key: SecretKey) -> Result<Endpoint> {
    use endpoint::presets::Preset as _;
    let mut builder = endpoint::Builder::empty();
    builder = presets::N0.apply(builder);
    // `presets::N0` only installs DNS-based address resolution on native
    // targets, which fails entirely behind a resolver that NXDOMAINs the
    // pkarr-backed `_iroh.*.dns.iroh.link` zone (some home routers do this).
    // `PkarrResolver` fetches the same signed record over HTTPS from the n0
    // DNS server, bypassing the system resolver. iroh races all address
    // lookup services, so this is a fallback when DNS can't resolve a peer.
    // builder = builder.address_lookup(iroh::address_lookup::PkarrResolver::n0_dns());
    builder = builder.secret_key(secret_key);
    builder = builder.alpns(vec![
        wire::ALPN.to_vec(),
        iroh_blobs::ALPN.to_vec(),
        iroh_gossip::ALPN.to_vec(),
        iroh_docs::ALPN.to_vec(),
        iroh_doctor_core::probe::ALPN.to_vec(),
        crate::doctor::ALPN.to_vec(),
    ]);
    builder.bind().await.context("bind endpoint")
}

async fn start_services_client(
    endpoint: &Endpoint,
    api_secret_override: &str,
    on_telemetry: &TelemetryCb,
) -> Option<ServicesClient> {
    let secret = if api_secret_override.is_empty() {
        DEFAULT_API_SECRET
    } else {
        api_secret_override
    };

    on_telemetry(TelemetryState::Starting);
    let name = device_name(&endpoint.id().to_string());

    let builder = match ServicesClient::builder(endpoint).api_secret_from_str(secret) {
        Ok(b) => b,
        Err(e) => {
            on_telemetry(TelemetryState::Error(format!("api secret: {e:?}")));
            return None;
        }
    };
    let builder = match builder.name(name.clone()) {
        Ok(b) => b,
        Err(e) => {
            on_telemetry(TelemetryState::Error(format!("name: {e:?}")));
            return None;
        }
    };

    match builder.build().await {
        Ok(client) => {
            on_telemetry(TelemetryState::Active { name });
            Some(client)
        }
        Err(e) => {
            on_telemetry(TelemetryState::Error(format!("{e:?}")));
            None
        }
    }
}

fn device_name(endpoint_id_hex: &str) -> String {
    let short: String = endpoint_id_hex.chars().take(8).collect();
    if cfg!(target_os = "macos") {
        format!("macos-dx-{short}")
    } else if cfg!(target_os = "linux") {
        format!("linux-dx-{short}")
    } else if cfg!(target_os = "windows") {
        format!("win-dx-{short}")
    } else {
        format!("dx-{short}")
    }
}

struct AcceptCtx {
    endpoint: Endpoint,
    game: Arc<Mutex<PongGame>>,
    session: Arc<Mutex<Option<SessionHandles>>>,
    conn_slot: Arc<Mutex<Option<endpoint::Connection>>>,
    on_state: StateCb,
    on_game: GameCb,
    /// Fires per completed upload on an incoming peer-probe stream so the
    /// Diagnostics tab can show throughput against an `iroh-doctor connect`
    /// monitor.
    on_throughput: ThroughputCb,
    /// Shared TTFDB state. The probe accept arm primes `dial_at` so the
    /// existing paths sampler reports time-to-first-direct-byte for an
    /// incoming peer-probe just like it does for outgoing pong dials.
    ttfdb: Arc<Mutex<TtfdbState>>,
    blobs: BlobsProtocol,
    gossip: Gossip,
    docs: Docs,
    /// Per-process cap on concurrent peer-probe server handlers. The
    /// peer-probe ALPN has no auth layer; without this cap a peer that
    /// learned our endpoint id could open arbitrarily many parallel
    /// probe sessions.
    probe_limit: Arc<Semaphore>,
}

/// Maximum number of peer-probe server handlers we will run in parallel.
const MAX_CONCURRENT_PROBE_SERVERS: usize = 4;

async fn run_accept_loop(ctx: AcceptCtx) {
    use iroh::protocol::ProtocolHandler;

    loop {
        let Some(incoming) = ctx.endpoint.accept().await else {
            break;
        };
        let mut accepting = match incoming.accept() {
            Ok(a) => a,
            Err(_) => continue,
        };
        let alpn = match accepting.alpn().await {
            Ok(a) => a,
            Err(_) => continue,
        };
        let conn = match accepting.await {
            Ok(c) => c,
            Err(_) => continue,
        };

        let alpn_bytes: &[u8] = alpn.as_ref();
        if alpn_bytes == wire::ALPN {
            // Spawn the pong adoption so a peer that completes the QUIC
            // handshake but never opens a bidi stream cannot wedge the
            // accept loop for the other three ALPNs. adopt_session still
            // replaces the single pong session via conn_slot.
            let game = ctx.game.clone();
            let session = ctx.session.clone();
            let conn_slot = ctx.conn_slot.clone();
            let on_state = ctx.on_state.clone();
            let on_game = ctx.on_game.clone();
            tokio::spawn(async move {
                let (send, recv) = match conn.accept_bi().await {
                    Ok(p) => p,
                    Err(e) => {
                        warn!(err = %e, "pong accept_bi failed");
                        return;
                    }
                };
                let remote_id = conn.remote_id().to_string();
                adopt_session(AdoptArgs {
                    conn,
                    send,
                    recv,
                    remote_id,
                    as_authority: false,
                    game,
                    session,
                    conn_slot,
                    on_state,
                    on_game,
                })
                .await;
            });
        } else if alpn_bytes == iroh_blobs::ALPN {
            let handler = ctx.blobs.clone();
            tokio::spawn(async move {
                if let Err(e) = handler.accept(conn).await {
                    warn!(err = %e, "iroh-blobs accept failed");
                }
            });
        } else if alpn_bytes == iroh_gossip::ALPN {
            let handler = ctx.gossip.clone();
            tokio::spawn(async move {
                if let Err(e) = handler.handle_connection(conn).await {
                    warn!(err = ?e, "iroh-gossip accept failed");
                }
            });
        } else if alpn_bytes == iroh_docs::ALPN {
            let handler = ctx.docs.clone();
            tokio::spawn(async move {
                if let Err(e) = handler.accept(conn).await {
                    warn!(err = %e, "iroh-docs accept failed");
                }
            });
        } else if alpn_bytes == iroh_doctor_core::probe::ALPN {
            // Try to grab a probe-server permit. If we are at capacity
            // we drop the connection on the floor; the active side will
            // see a clean QUIC error and can retry.
            let limit = ctx.probe_limit.clone();
            let permit = match limit.try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    warn!("peer-probe rejected: at capacity");
                    drop(conn);
                    continue;
                }
            };
            // Surface the probe connection through the same conn_slot the
            // paths sampler reads, so the diagnostics view shows the
            // connection state, paths, and live RTT for an incoming
            // `iroh-doctor connect` monitor. If a pong session already
            // owns the slot we respond silently in the background.
            let conn_slot = ctx.conn_slot.clone();
            let on_state = ctx.on_state.clone();
            let on_throughput = ctx.on_throughput.clone();
            let ttfdb = ctx.ttfdb.clone();
            tokio::spawn(async move {
                let peer_id = conn.remote_id().to_string();
                let peer_short_id: String = peer_id.chars().take(10).collect();
                let claimed = {
                    let mut slot = conn_slot.lock().await;
                    if slot.is_none() {
                        *slot = Some(conn.clone());
                        on_state(ConnectionState::Connected {
                            peer_id: peer_id.clone(),
                            peer_short_id: peer_short_id.clone(),
                        });
                        // Prime the TTFDB state so the existing paths
                        // sampler reports time-to-first-direct-byte for
                        // this incoming probe, just like an outgoing
                        // pong dial does. We only do this when the
                        // probe actually owns conn_slot; otherwise a
                        // pong session is in charge and its dial-time
                        // baseline must not be clobbered.
                        let mut state = ttfdb.lock().await;
                        state.dial_at = Some(std::time::Instant::now());
                        state.published = false;
                        true
                    } else {
                        false
                    }
                };

                // Bounded channel: the responder emits one event per
                // upload, which clients pace by waiting for `UploadDone`,
                // so 8 slots is more than enough headroom for the
                // drainer to keep up.
                let (tx, mut rx) = mpsc::channel::<iroh_doctor_core::probe::ProbeEvent>(8);
                let drain = tokio::spawn(async move {
                    while let Some(event) = rx.recv().await {
                        match event {
                            iroh_doctor_core::probe::ProbeEvent::UploadCompleted {
                                bytes,
                                elapsed,
                            } => {
                                on_throughput(ThroughputSnapshot {
                                    bytes,
                                    elapsed,
                                    mbps: iroh_doctor_core::probe::throughput_mbps(bytes, elapsed),
                                });
                            }
                        }
                    }
                });

                if let Err(e) = iroh_doctor_core::probe::handle_connection_with(conn, tx).await {
                    warn!(err = %e, "peer-probe accept failed");
                }
                // Sender drops here when handle_connection_with returns;
                // the drainer's `rx.recv()` then returns None and the
                // task ends. Await it so we don't leak a JoinHandle.
                let _ = drain.await;
                drop(permit);

                if claimed {
                    let mut slot = conn_slot.lock().await;
                    // Only clear the slot if it still holds our connection;
                    // a pong session could have replaced it while we ran.
                    if slot
                        .as_ref()
                        .is_some_and(|c| c.remote_id().to_string() == peer_id)
                    {
                        *slot = None;
                    }
                    on_state(ConnectionState::PeerDisconnected { peer_short_id });
                }
            });
        } else if alpn_bytes == crate::doctor::ALPN {
            // Passive side of `iroh-doctor connect`. Bounded internally by
            // a per-connection timeout; spawned like the other protocol
            // handlers so a slow test does not wedge the accept loop.
            tokio::spawn(async move {
                if let Err(e) = crate::doctor::handle_connection(conn).await {
                    warn!(err = %e, "doctor accept failed");
                }
            });
        }
    }
}

struct AdoptArgs {
    conn: endpoint::Connection,
    send: endpoint::SendStream,
    recv: endpoint::RecvStream,
    remote_id: String,
    as_authority: bool,
    game: Arc<Mutex<PongGame>>,
    session: Arc<Mutex<Option<SessionHandles>>>,
    conn_slot: Arc<Mutex<Option<endpoint::Connection>>>,
    on_state: StateCb,
    on_game: GameCb,
}

fn snapshot_paths(conn: &endpoint::Connection) -> Vec<PathInfo> {
    conn.paths()
        .iter()
        .map(|p| {
            let addr = p.remote_addr();
            let kind = if addr.is_relay() {
                PathKind::Relay
            } else if addr.is_ip() {
                PathKind::Ip
            } else {
                PathKind::Custom
            };
            PathInfo {
                addr: addr.to_string(),
                kind,
                selected: p.is_selected(),
                rtt_ms: p.rtt().as_secs_f64() * 1000.0,
            }
        })
        .collect()
}

struct SessionHandles {
    send_task: JoinHandle<()>,
    recv_task: JoinHandle<()>,
}

impl SessionHandles {
    fn abort(&self) {
        self.send_task.abort();
        self.recv_task.abort();
    }
}

async fn adopt_session(args: AdoptArgs) {
    {
        let mut slot = args.session.lock().await;
        if let Some(prev) = slot.take() {
            prev.abort();
        }
    }
    {
        let mut slot = args.conn_slot.lock().await;
        *slot = Some(args.conn);
    }

    {
        let mut g = args.game.lock().await;
        let mut rng = thread_rng();
        g.reset_for_new_session(args.as_authority, &mut rng);
        let snap = *g;
        drop(g);
        (args.on_game)(snap);
    }

    let send_task = {
        let game = args.game.clone();
        let on_game = args.on_game.clone();
        let mut send = args.send;
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_micros(16_667));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let (paddle_frame, ball_frame, snap) = {
                    let mut g = game.lock().await;
                    let mut rng = thread_rng();
                    g.tick_from_clock(&mut rng);
                    let paddle = wire::encode_paddle(g.my_paddle_x);
                    let ball = g.ball_frame();
                    (paddle, ball, *g)
                };
                on_game(snap);
                if send.write_all(&paddle_frame).await.is_err() {
                    break;
                }
                if let Some(b) = ball_frame {
                    if send.write_all(&b).await.is_err() {
                        break;
                    }
                }
            }
        })
    };

    let peer_short_id: String = args.remote_id.chars().take(10).collect();
    let recv_task = {
        let game = args.game.clone();
        let on_game = args.on_game.clone();
        let on_state = args.on_state.clone();
        let session = args.session.clone();
        let conn_slot = args.conn_slot.clone();
        let peer_short_id = peer_short_id.clone();
        let mut recv = args.recv;
        tokio::spawn(async move {
            let mut tag_buf = [0u8; 1];
            loop {
                if recv.read_exact(&mut tag_buf).await.is_err() {
                    break;
                }
                match tag_buf[0] {
                    wire::TAG_PADDLE => {
                        let mut body = [0u8; 4];
                        if recv.read_exact(&mut body).await.is_err() {
                            break;
                        }
                        if let Some(x) = wire::decode_paddle_body(&body) {
                            let snap = {
                                let mut g = game.lock().await;
                                g.received_opponent_paddle(x);
                                *g
                            };
                            on_game(snap);
                        }
                    }
                    wire::TAG_BALL => {
                        let mut body = [0u8; 20];
                        if recv.read_exact(&mut body).await.is_err() {
                            break;
                        }
                        if let Some(payload) = wire::decode_ball_body(&body) {
                            let snap = {
                                let mut g = game.lock().await;
                                g.received_ball(payload);
                                *g
                            };
                            on_game(snap);
                        }
                    }
                    _ => break,
                }
            }

            // Natural exit means the remote stream EOF'd: the peer
            // closed their endpoint, their device disappeared, or they
            // shut the app down. tokio::abort jumps past this block
            // entirely, so reaching here is always a peer-initiated
            // disconnect (not a local connection swap). Clear our
            // session bookkeeping so the next Connect doesn't think
            // there's still a live session, and surface the change to
            // the UI.
            {
                let mut s = session.lock().await;
                *s = None;
            }
            {
                let mut c = conn_slot.lock().await;
                *c = None;
            }
            on_state(ConnectionState::PeerDisconnected { peer_short_id });
        })
    };

    {
        let mut slot = args.session.lock().await;
        *slot = Some(SessionHandles {
            send_task,
            recv_task,
        });
    }

    (args.on_state)(ConnectionState::Connected {
        peer_id: args.remote_id.clone(),
        peer_short_id,
    });
}

pub fn looks_like_endpoint_id(s: &str) -> bool {
    let trimmed = s.trim();
    trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_like_endpoint_id_accepts_64_hex() {
        let s = "a".repeat(64);
        assert!(looks_like_endpoint_id(&s));
        assert!(looks_like_endpoint_id(&format!("  {s}  ")));
    }

    #[test]
    fn looks_like_endpoint_id_rejects_wrong_length() {
        assert!(!looks_like_endpoint_id("abc"));
        assert!(!looks_like_endpoint_id(&"a".repeat(63)));
        assert!(!looks_like_endpoint_id(&"a".repeat(65)));
    }

    #[test]
    fn looks_like_endpoint_id_rejects_non_hex() {
        let s = "z".repeat(64);
        assert!(!looks_like_endpoint_id(&s));
    }

    #[test]
    fn looks_like_endpoint_id_accepts_uppercase_hex() {
        // is_ascii_hexdigit accepts 'A'..='F'; the helper inherits that.
        let s: String = std::iter::repeat_n('F', 64).collect();
        assert!(looks_like_endpoint_id(&s));
    }

    #[test]
    fn looks_like_endpoint_id_rejects_all_whitespace() {
        // 64 ASCII spaces trim to empty, which then fails the length check.
        let s = " ".repeat(64);
        assert!(!looks_like_endpoint_id(&s));
    }

    #[test]
    fn parse_topic_id_64_hex_roundtrips() {
        let hex: String = "ab".repeat(32);
        let topic = parse_topic_id(&hex);
        assert_eq!(topic.to_string(), hex);
    }

    #[test]
    fn parse_topic_id_friendly_string_hashes() {
        let a = parse_topic_id("hello-iroh-pong");
        let b = parse_topic_id("hello-iroh-pong");
        let c = parse_topic_id("different-topic");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn parse_topic_id_trims_whitespace_before_hashing() {
        let a = parse_topic_id("  test  ");
        let b = parse_topic_id("test");
        assert_eq!(a, b);
    }

    #[test]
    fn parse_topic_id_64_non_hex_falls_back_to_hash() {
        // 64 chars but with 'z' (not a hex digit) - must not be parsed as
        // hex; falls through to the BLAKE3 fallback.
        let s = "z".repeat(64);
        let from_blake = parse_topic_id(&s);
        // Same input through the hash branch must be deterministic.
        let again = parse_topic_id(&s);
        assert_eq!(from_blake, again);
        // And it must differ from a different input.
        let other = parse_topic_id("something-else");
        assert_ne!(from_blake, other);
    }

    #[test]
    fn check_blob_size_rejects_zero() {
        let err = check_blob_size(0).unwrap_err();
        assert!(err.contains("greater than zero"), "got: {err}");
    }

    #[test]
    fn check_blob_size_accepts_below_cap() {
        assert_eq!(check_blob_size(1024).unwrap(), 1024);
        assert_eq!(
            check_blob_size(MAX_BLOB_BYTES).unwrap(),
            MAX_BLOB_BYTES as usize
        );
    }

    #[test]
    fn check_blob_size_rejects_above_cap() {
        let err = check_blob_size(MAX_BLOB_BYTES + 1).unwrap_err();
        assert!(err.contains("exceeds"), "got: {err}");
        // The error message uses IEC units, not raw bytes.
        assert!(err.contains("GiB"), "got: {err}");
    }
}
