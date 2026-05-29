//! iroh endpoint binding, accept loop, the active connection monitor, and
//! services telemetry.
//!
//! Connecting to a peer dials the iroh-doctor probe protocol and runs the
//! shared monitor loop from [`iroh_doctor_core::probe`], the same one
//! `iroh-doctor connect` uses, so the app reports latency, paths,
//! time-to-first-direct-byte, and throughput identically to the cli.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use iroh::endpoint::{self, presets};
use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey};
use iroh_gossip::net::Gossip;
use iroh_services::Client as ServicesClient;
use tokio::sync::{mpsc, oneshot, Mutex, Semaphore};
use tokio::task::JoinHandle;
use tracing::info;

mod accept;
mod gossip;
mod monitor;

use accept::run_accept_loop;
use gossip::join_gossip;
use monitor::{run_monitor, snapshot_paths};

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

pub enum NodeCommand {
    Connect {
        hex_id: String,
    },
    /// Tears down the active monitor: aborts the probe loop, closes the
    /// connection, and returns to the ready state. Also cancels a dial that
    /// is still in `Connecting`.
    Disconnect,
    SaveApiSecret {
        secret: String,
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

/// The services-side `net_diagnostics` summary. Lives in core so the cli and
/// the app render the same shape; re-exported so `node::DiagnosticsReport`
/// call sites keep working.
pub use iroh_doctor_core::services::DiagnosticsReport;

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

type StateCb = Arc<dyn Fn(ConnectionState) + Send + Sync>;
type TelemetryCb = Arc<dyn Fn(TelemetryState) + Send + Sync>;
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
/// keeps `run_node`'s signature compact and gives future commits an obvious
/// place to add new event channels.
pub struct NodeCallbacks {
    pub on_endpoint_id: Box<dyn Fn(String) + Send + Sync>,
    pub on_state: Box<dyn Fn(ConnectionState) + Send + Sync>,
    pub on_telemetry: Box<dyn Fn(TelemetryState) + Send + Sync>,
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

/// How often `run_node` samples the live connection's QUIC paths for the
/// Diagnostics view. Short enough to feel live, long enough to keep overhead
/// in the noise floor.
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

/// Snapshot of one QUIC path on the live connection.
#[derive(Debug, Clone, PartialEq)]
pub struct PathInfo {
    /// `TransportAddr` rendered via `Display` (`ip:1.2.3.4:5678`,
    /// `relay:https://...`, or `custom:...`).
    pub addr: String,
    pub kind: PathKind,
    pub selected: bool,
    pub rtt_ms: f64,
}

pub async fn run_node(
    secret_key: SecretKey,
    initial_api_secret_override: String,
    mut commands: mpsc::Receiver<NodeCommand>,
    callbacks: NodeCallbacks,
) -> Result<()> {
    let on_endpoint_id: IdCb = Arc::from(callbacks.on_endpoint_id);
    let on_state: StateCb = Arc::from(callbacks.on_state);
    let on_telemetry: TelemetryCb = Arc::from(callbacks.on_telemetry);
    let on_paths: PathsCb = Arc::from(callbacks.on_paths);
    let on_ttfdb: TtfdbCb = Arc::from(callbacks.on_ttfdb);
    let on_throughput: ThroughputCb = Arc::from(callbacks.on_throughput);

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

    let mut api_secret_override = initial_api_secret_override;
    let mut services: Option<ServicesClient> =
        start_services_client(&endpoint, &api_secret_override, &on_telemetry).await;

    // Owner for the gossip protocol. Cheaply cloneable; clones share state
    // and outlive any individual connection.
    let gossip = Gossip::builder().spawn(endpoint.clone());
    info!("multi-protocol endpoint ready (probe, gossip)");

    // The active monitor for the current dial. A fresh Connect aborts the
    // previous one before installing its own, so at most one runs at a time.
    let monitor: Arc<Mutex<Option<JoinHandle<()>>>> = Arc::new(Mutex::new(None));
    let conn_slot: Arc<Mutex<Option<endpoint::Connection>>> = Arc::new(Mutex::new(None));

    // Long-running tasks (accept loop, paths sampler) live on a JoinSet
    // owned by run_node so they shut down cleanly when the command pump
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
            conn_slot: conn_slot.clone(),
            on_state: on_state.clone(),
            on_throughput: on_throughput.clone(),
            on_ttfdb: on_ttfdb.clone(),
            gossip: gossip.clone(),
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

    // Paths sampler: every PATHS_SAMPLE_INTERVAL, snapshot the active
    // connection's QUIC paths for the Diagnostics view. We only notify the UI
    // when the snapshot actually changed (RTT, selected path, or set of
    // addresses) so an idle endpoint does not re-wake the Dioxus event pump
    // twice a second. Time-to-first-direct-byte is handled separately by a
    // per-connection `ttfdb_watch` task (see `run_monitor` and the probe
    // accept arm).
    {
        let conn_slot = conn_slot.clone();
        let on_paths = on_paths.clone();
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
            NodeCommand::Connect { hex_id } => {
                on_state(ConnectionState::Connecting);
                let trimmed = hex_id.trim().to_string();
                let parsed = match EndpointId::from_str(&trimmed) {
                    Ok(p) => p,
                    Err(_) => {
                        on_state(ConnectionState::Error("invalid endpoint id".into()));
                        continue;
                    }
                };
                // Clear any TTFDB value from a prior run; the per-connection
                // ttfdb_watch task in run_monitor publishes the new one.
                on_ttfdb(None);
                let addr = EndpointAddr::from_parts(parsed, std::iter::empty());
                // Dialing plus hole punching can take seconds, so the monitor
                // runs on its own task and the command pump stays responsive.
                // Abort any monitor from a previous dial first, then own the
                // new one in the slot.
                let endpoint = endpoint.clone();
                let conn_slot = conn_slot.clone();
                let on_state = on_state.clone();
                let on_throughput = on_throughput.clone();
                let on_ttfdb = on_ttfdb.clone();
                let mut slot = monitor.lock().await;
                if let Some(prev) = slot.take() {
                    prev.abort();
                }
                *slot = Some(tokio::spawn(async move {
                    run_monitor(endpoint, addr, conn_slot, on_state, on_throughput, on_ttfdb).await;
                }));
            }
            NodeCommand::Disconnect => {
                // Abort the monitor task. Its own cleanup does not run on
                // abort, so we clear conn_slot and close the connection here.
                {
                    let mut slot = monitor.lock().await;
                    if let Some(task) = slot.take() {
                        task.abort();
                    }
                }
                if let Some(conn) = conn_slot.lock().await.take() {
                    conn.close(0u32.into(), b"disconnect");
                }
                // Clear the UI metric so a later reconnect starts clean.
                on_ttfdb(None);
                on_state(ConnectionState::Ready);
            }
            NodeCommand::SaveApiSecret { secret } => {
                api_secret_override = secret.trim().to_string();
                services.take(); // drop the old client so its background tasks stop
                services =
                    start_services_client(&endpoint, &api_secret_override, &on_telemetry).await;
            }
            NodeCommand::PingServices { reply } => {
                // A network round-trip to the services endpoint; spawn so it
                // does not block the command pump for its duration.
                let client = services.clone();
                tokio::spawn(async move {
                    let result = match client {
                        Some(c) => iroh_doctor_core::services::ping(&c)
                            .await
                            .map_err(|e| format!("{e:#}")),
                        None => Err("services client not initialized".into()),
                    };
                    let _ = reply.send(result);
                });
            }
            NodeCommand::RunNetDiagnostics { reply } => {
                // net_diagnostics probes external services and can take
                // seconds; spawn so the command pump stays responsive.
                let client = services.clone();
                tokio::spawn(async move {
                    let result = match client {
                        Some(c) => iroh_doctor_core::services::net_diagnostics(&c)
                            .await
                            .map_err(|e| format!("{e:#}")),
                        None => Err("services client not initialized".into()),
                    };
                    let _ = reply.send(result);
                });
            }
            NodeCommand::ProbeNetReport { reply } => {
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
            NodeCommand::ProbeRelayLatencies { reply } => {
                // Probe every relay in the same map iroh's default relay
                // mode resolves to. The bound endpoint is built with
                // `presets::N0` which uses `default_relay_mode()`, so
                // the two agree by construction. If `bind_endpoint`
                // ever switches to a custom relay map this code will
                // probe the wrong set silently; a future change should
                // plumb the bound RelayMap through `run_node` instead.
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
            NodeCommand::ProbePortMap { reply } => {
                // Direct port-mapping probe. The portmapper crate spawns
                // background gateway calls; we bound the whole thing with
                // a timeout inside `probe()`.
                tokio::spawn(async move {
                    let result = crate::portmap_probe::probe().await;
                    let _ = reply.send(Ok(result));
                });
            }
            NodeCommand::JoinGossip {
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
            NodeCommand::GossipBroadcast { msg, reply } => {
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
        iroh_gossip::ALPN.to_vec(),
        iroh_doctor_core::probe::ALPN.to_vec(),
    ]);
    builder.bind().await.context("bind endpoint")
}

async fn start_services_client(
    endpoint: &Endpoint,
    api_secret_override: &str,
    on_telemetry: &TelemetryCb,
) -> Option<ServicesClient> {
    // `IROH_SERVICES_API_SECRET=""` opts out; otherwise core resolves the env
    // var, then the saved override, then the bundled default.
    let Some(secret) = iroh_doctor_core::services::resolve_api_secret(Some(api_secret_override))
    else {
        on_telemetry(TelemetryState::Off);
        return None;
    };

    on_telemetry(TelemetryState::Starting);
    let name = iroh_doctor_core::services::device_name(&endpoint.id().to_string());
    match iroh_doctor_core::services::build_client(endpoint, &secret, &name).await {
        Ok(client) => {
            on_telemetry(TelemetryState::Active { name });
            Some(client)
        }
        Err(e) => {
            on_telemetry(TelemetryState::Error(format!("{e:#}")));
            None
        }
    }
}

struct AcceptCtx {
    endpoint: Endpoint,
    conn_slot: Arc<Mutex<Option<endpoint::Connection>>>,
    on_state: StateCb,
    /// Fires per completed upload on an incoming peer-probe stream so the
    /// Diagnostics tab can show throughput against an `iroh-doctor connect`
    /// monitor.
    on_throughput: ThroughputCb,
    /// Reports time-to-first-direct-byte for an incoming peer-probe, the same
    /// metric an outgoing dial publishes. The accept arm runs a `ttfdb_watch`
    /// task when it owns `conn_slot`.
    on_ttfdb: TtfdbCb,
    gossip: Gossip,
    /// Per-process cap on concurrent peer-probe server handlers. The
    /// peer-probe ALPN has no auth layer; without this cap a peer that
    /// learned our endpoint id could open arbitrarily many parallel
    /// probe sessions.
    probe_limit: Arc<Semaphore>,
}

/// Maximum number of peer-probe server handlers we will run in parallel.
const MAX_CONCURRENT_PROBE_SERVERS: usize = 4;

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
}
