//! The headless doctor node: binds the iroh endpoint, routes incoming
//! protocols through [`iroh::protocol::Router`], runs the active connection
//! monitor, and manages services telemetry.
//!
//! The node is an actor. A front end sends [`NodeCommand`]s down an mpsc
//! channel and receives [`NodeEvent`]s back; nothing in here knows about
//! any UI framework. Connecting to a peer dials the iroh-doctor probe
//! protocol and runs the shared monitor loop from [`crate::monitor`], the
//! same one `iroh-doctor connect` uses, so every front end reports latency,
//! paths, time-to-first-direct-byte, and throughput identically to the cli.

use std::future::Future;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use iroh::endpoint::{self, presets};
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey};
use iroh_gossip::net::Gossip;
use iroh_services::Client as ServicesClient;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tracing::info;

mod accept;
mod gossip;
mod monitor;

pub use gossip::GossipEvent;

use accept::ProbeProtocol;
use gossip::{join_gossip, GossipSession};
use monitor::run_monitor;

use crate::monitor::{snapshot_paths, PathSnapshot};
use crate::report::{NetReportSummary, RelayLatencyRow};
use crate::services::DiagnosticsReport;
use crate::NET_REPORT_TIMEOUT;

/// The connection lifecycle as the front end sees it.
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

/// The services telemetry client's lifecycle.
#[derive(Debug, Clone)]
pub enum TelemetryState {
    Off,
    Starting,
    Active { name: String },
    Error(String),
}

/// Everything a front end can ask the node to do.
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
    /// Turns telemetry on or off at runtime. Rebuilds the services client when
    /// enabling and drops it when disabling. The metrics task is `Arc`-backed
    /// and shared with any in-flight `PingServices`/`RunNetDiagnostics` probe,
    /// so disabling aborts those probes before dropping the client, releasing
    /// every clone so pushes stop promptly rather than at the next restart.
    SetTelemetryEnabled {
        enabled: bool,
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
    /// Reports the per-relay latencies iroh recorded in its net report,
    /// one row per relay, sorted ascending by latency.
    ProbeRelayLatencies {
        reply: oneshot::Sender<Result<Vec<RelayLatencyRow>, String>>,
    },
    /// Joins an iroh-gossip topic. `topic_input` is parsed as 64-hex if it
    /// matches that shape, otherwise hashed with BLAKE3 so any string
    /// becomes a deterministic topic. Any previously joined topic is
    /// dropped.
    JoinGossip {
        topic_input: String,
        bootstrap: Vec<String>,
        events_tx: mpsc::Sender<GossipEvent>,
        reply: oneshot::Sender<Result<String, String>>,
    },
    /// Broadcasts `msg` (UTF-8) on the currently-joined gossip topic.
    GossipBroadcast {
        msg: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
}

/// Everything the node pushes back to the front end. One channel carries
/// the whole stream; the front end folds each event into its own state.
#[derive(Debug, Clone)]
pub enum NodeEvent {
    /// The endpoint bound and this is its id.
    EndpointId(String),
    /// The connection lifecycle advanced.
    ConnectionState(ConnectionState),
    /// The services telemetry client's state changed.
    Telemetry(TelemetryState),
    /// A fresh snapshot of the live connection's QUIC paths.
    Paths(Vec<PathSnapshot>),
    /// Time from dial (or accept) to the first selected direct path, or
    /// `None` to clear the metric when a fresh dial begins.
    Ttfdb(Option<Duration>),
    /// A completed upload on the peer probe, from either side.
    Throughput(ThroughputSnapshot),
    /// One sample for the live latency graph. On an outgoing dial these
    /// are probe ping round-trips, the same series `iroh-doctor connect`
    /// plots; for an incoming probe they are the selected path's smoothed
    /// QUIC RTT, matching `iroh-doctor accept` (the passive side has no
    /// ping loop of its own).
    Latency(Duration),
}

/// Snapshot of one completed upload from the peer probe (`iroh-doctor
/// connect` monitor). `bytes` and `elapsed` come straight from
/// [`crate::probe::ProbeEvent::UploadCompleted`]; `mbps` is the
/// pre-computed convenience value so a UI does not have to redo the math
/// for every render.
#[derive(Debug, Clone, PartialEq)]
pub struct ThroughputSnapshot {
    pub bytes: u64,
    pub elapsed: Duration,
    pub mbps: Option<f64>,
}

/// Startup configuration for [`run_node`].
#[derive(Debug)]
pub struct NodeOptions {
    /// The endpoint identity.
    pub secret_key: SecretKey,
    /// User-supplied services API key overriding the bundled default;
    /// empty for none.
    pub api_secret_override: String,
    /// The user's saved telemetry opt-out.
    pub telemetry_disabled: bool,
}

/// Cloneable sender for [`NodeEvent`]s. A vanished receiver (front end
/// shutting down) just drops events.
#[derive(Debug, Clone)]
pub(crate) struct Events(mpsc::UnboundedSender<NodeEvent>);

impl Events {
    fn send(&self, event: NodeEvent) {
        let _ = self.0.send(event);
    }

    pub(crate) fn state(&self, state: ConnectionState) {
        self.send(NodeEvent::ConnectionState(state));
    }

    fn telemetry(&self, state: TelemetryState) {
        self.send(NodeEvent::Telemetry(state));
    }

    fn paths(&self, paths: Vec<PathSnapshot>) {
        self.send(NodeEvent::Paths(paths));
    }

    pub(crate) fn ttfdb(&self, elapsed: Option<Duration>) {
        self.send(NodeEvent::Ttfdb(elapsed));
    }

    pub(crate) fn throughput(&self, snapshot: ThroughputSnapshot) {
        self.send(NodeEvent::Throughput(snapshot));
    }

    pub(crate) fn latency(&self, rtt: Duration) {
        self.send(NodeEvent::Latency(rtt));
    }
}

/// The connection slot shared between the accept handler, the dial
/// monitor, and the paths sampler: whichever connection currently owns it
/// drives the paths view.
pub(crate) type SharedConn = Arc<Mutex<Option<endpoint::Connection>>>;

/// How often the node samples the live connection's QUIC paths for the
/// paths view. Short enough to feel live, long enough to keep overhead in
/// the noise floor.
const PATHS_SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

/// Reply when a services probe runs with no client configured. The UI
/// matches on this exact text to keep the expected "telemetry off" case out
/// of the global error modal, so producer and matcher share the constant.
pub const SERVICES_OFF_ERROR: &str = "services client not initialized";

/// Prefix of the [`ConnectionState::Error`] reported when the endpoint fails
/// to bind. The UI matches on it to append recovery guidance.
pub const BIND_FAILED_PREFIX: &str = "bind failed";

/// Runs the node until `commands` closes: binds the endpoint, spawns the
/// protocol router and the paths sampler, then pumps commands. Events go
/// out on `events` for the front end to fold into its state.
pub async fn run_node(
    options: NodeOptions,
    mut commands: mpsc::Receiver<NodeCommand>,
    events: mpsc::UnboundedSender<NodeEvent>,
) -> Result<()> {
    let NodeOptions {
        secret_key,
        api_secret_override,
        telemetry_disabled,
    } = options;
    let events = Events(events);

    events.state(ConnectionState::Binding);
    let endpoint = match bind_endpoint(secret_key).await {
        Ok(ep) => ep,
        Err(e) => {
            events.state(ConnectionState::Error(format!(
                "{BIND_FAILED_PREFIX}: {e:#}"
            )));
            return Err(e);
        }
    };
    events.send(NodeEvent::EndpointId(endpoint.id().to_string()));
    events.state(ConnectionState::Ready);

    let mut node = Node::new(endpoint, events, api_secret_override, telemetry_disabled);
    node.rebuild_services_client().await;

    // Route incoming connections by ALPN: gossip to the gossip handler,
    // the probe ALPN to the capacity-capped probe responder. The router
    // owns the accept loop and installs the ALPN list on the endpoint.
    let router = Router::builder(node.endpoint.clone())
        .accept(iroh_gossip::ALPN, node.gossip.clone())
        .accept(
            crate::probe::ALPN,
            ProbeProtocol::new(node.conn_slot.clone(), node.events.clone()),
        )
        .spawn();
    info!("multi-protocol endpoint ready (probe, gossip)");

    let sampler = node.spawn_paths_sampler();

    while let Some(cmd) = commands.recv().await {
        node.handle(cmd).await;
    }

    // Command pump exited: stop the sampler, then shut the router down,
    // which also drains the protocol handlers and closes the endpoint.
    info!("command pump closed, shutting node down");
    sampler.abort();
    let _ = router.shutdown().await;
    Ok(())
}

/// The node's mutable state, owned by the command pump. Long-lived shared
/// pieces (`conn_slot`, `dial_active`) are `Arc`ed because the sampler, the
/// accept handler, and the dial monitor read them from their own tasks.
struct Node {
    endpoint: Endpoint,
    events: Events,
    gossip: Gossip,
    /// The connection currently driving the paths view.
    conn_slot: SharedConn,
    /// The active dial monitor task. A fresh Connect aborts the previous
    /// one before installing its own, so at most one runs at a time.
    monitor: Option<JoinHandle<()>>,
    /// True while a dial monitor owns the latency series: it plots probe
    /// ping round-trips, so the paths sampler must hold its path-RTT
    /// samples back. The monitor sets this for its whole lifetime, so the
    /// sampler never mixes the two sources.
    dial_active: Arc<AtomicBool>,
    services: Option<ServicesClient>,
    /// In-flight PingServices/RunNetDiagnostics probes. Each holds a clone
    /// of the services client, whose metrics-push task is Arc-backed and
    /// stops only when the last clone drops; tracking the probes lets a
    /// telemetry toggle abort them so pushes stop promptly.
    services_probes: tokio::task::JoinSet<()>,
    api_secret_override: String,
    telemetry_disabled: bool,
    /// The joined gossip topic (receiver task plus sender), at most one at
    /// a time.
    gossip_session: Arc<Mutex<GossipSession>>,
}

impl Node {
    fn new(
        endpoint: Endpoint,
        events: Events,
        api_secret_override: String,
        telemetry_disabled: bool,
    ) -> Self {
        // Owner for the gossip protocol. Cheaply cloneable; clones share
        // state and outlive any individual connection.
        let gossip = Gossip::builder().spawn(endpoint.clone());
        Self {
            endpoint,
            events,
            gossip,
            conn_slot: Arc::new(Mutex::new(None)),
            monitor: None,
            dial_active: Arc::new(AtomicBool::new(false)),
            services: None,
            services_probes: tokio::task::JoinSet::new(),
            api_secret_override,
            telemetry_disabled,
            gossip_session: Arc::new(Mutex::new(GossipSession::default())),
        }
    }

    /// (Re)builds the services client for the current telemetry setting,
    /// aborting in-flight probes first so every clone of the previous
    /// client is released and its metrics pushes stop.
    async fn rebuild_services_client(&mut self) {
        self.services_probes.abort_all();
        self.services.take();
        self.services = start_services_client(
            &self.endpoint,
            self.telemetry_disabled,
            &self.api_secret_override,
            &self.events,
        )
        .await;
    }

    /// Runs one services query on its own task, replying with
    /// [`SERVICES_OFF_ERROR`] when no client is configured. The task joins
    /// `services_probes` (finished entries drained first) so a telemetry
    /// toggle can abort it and release its client clone.
    fn spawn_services_probe<T, Fut>(
        &mut self,
        reply: oneshot::Sender<Result<T, String>>,
        query: impl FnOnce(ServicesClient) -> Fut + Send + 'static,
    ) where
        T: Send + 'static,
        Fut: Future<Output = Result<T>> + Send,
    {
        let client = self.services.clone();
        while self.services_probes.try_join_next().is_some() {}
        self.services_probes.spawn(async move {
            let result = match client {
                Some(c) => query(c).await.map_err(|e| format!("{e:#}")),
                None => Err(SERVICES_OFF_ERROR.into()),
            };
            let _ = reply.send(result);
        });
    }

    /// Awaits the endpoint's net report on its own task and replies with a
    /// projection of it, so the command pump never blocks on the network.
    fn spawn_net_report_probe<T: Send + 'static>(
        &self,
        reply: oneshot::Sender<Result<T, String>>,
        project: impl FnOnce(&iroh::NetReport) -> Result<T, String> + Send + 'static,
    ) {
        let endpoint = self.endpoint.clone();
        tokio::spawn(async move {
            let result = net_report(&endpoint).await.and_then(|r| project(&r));
            let _ = reply.send(result);
        });
    }

    /// Paths sampler: every [`PATHS_SAMPLE_INTERVAL`], snapshot the active
    /// connection's QUIC paths. The front end is only notified when the
    /// snapshot actually changed (RTT, selected path, or set of addresses)
    /// so an idle endpoint does not re-wake the UI twice a second.
    ///
    /// The sampler also feeds the latency graph from the selected path's
    /// smoothed QUIC RTT, but only while no dial monitor is running: an
    /// outgoing dial plots probe ping round-trips instead (see
    /// [`run_monitor`]), and mixing the two sources would corrupt the
    /// series.
    fn spawn_paths_sampler(&self) -> JoinHandle<()> {
        let conn_slot = self.conn_slot.clone();
        let events = self.events.clone();
        let dial_active = self.dial_active.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(PATHS_SAMPLE_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut last: Vec<PathSnapshot> = Vec::new();
            loop {
                ticker.tick().await;
                let snapshot: Vec<PathSnapshot> = {
                    let slot = conn_slot.lock().await;
                    slot.as_ref().map(snapshot_paths).unwrap_or_default()
                };
                if !dial_active.load(Ordering::Acquire) {
                    if let Some(selected) = snapshot.iter().find(|p| p.selected) {
                        events.latency(selected.rtt);
                    }
                }
                if snapshot != last {
                    events.paths(snapshot.clone());
                    last = snapshot;
                }
            }
        })
    }

    async fn handle(&mut self, cmd: NodeCommand) {
        match cmd {
            NodeCommand::Connect { hex_id } => self.connect(hex_id).await,
            NodeCommand::Disconnect => self.disconnect().await,
            NodeCommand::SaveApiSecret { secret } => {
                self.api_secret_override = secret.trim().to_string();
                self.rebuild_services_client().await;
            }
            NodeCommand::SetTelemetryEnabled { enabled } => {
                info!(enabled, "telemetry toggled");
                self.telemetry_disabled = !enabled;
                self.rebuild_services_client().await;
            }
            NodeCommand::PingServices { reply } => {
                self.spawn_services_probe(reply, |c| async move { crate::services::ping(&c).await })
            }
            NodeCommand::RunNetDiagnostics { reply } => self
                .spawn_services_probe(reply, |c| async move {
                    crate::services::net_diagnostics(&c).await
                }),
            NodeCommand::ProbeNetReport { reply } => {
                self.spawn_net_report_probe(reply, |report| Ok(NetReportSummary::from(report)))
            }
            NodeCommand::ProbeRelayLatencies { reply } => {
                self.spawn_net_report_probe(reply, |report| {
                    let rows = crate::report::relay_latencies(report);
                    if rows.is_empty() {
                        Err("no relay latencies recorded yet".into())
                    } else {
                        Ok(rows)
                    }
                })
            }
            NodeCommand::JoinGossip {
                topic_input,
                bootstrap,
                events_tx,
                reply,
            } => {
                // Joining dials bootstrap peers and can take a while; spawn
                // so the command pump stays responsive.
                let gossip = self.gossip.clone();
                let session = self.gossip_session.clone();
                tokio::spawn(async move {
                    let result =
                        join_gossip(gossip, session, topic_input, bootstrap, events_tx).await;
                    let _ = reply.send(result);
                });
            }
            NodeCommand::GossipBroadcast { msg, reply } => {
                let session = self.gossip_session.clone();
                tokio::spawn(async move {
                    let result = match GossipSession::sender(&session).await {
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

    async fn connect(&mut self, hex_id: String) {
        self.events.state(ConnectionState::Connecting);
        let trimmed = hex_id.trim().to_string();
        let parsed = match EndpointId::from_str(&trimmed) {
            Ok(p) => p,
            Err(_) => {
                self.events
                    .state(ConnectionState::Error("invalid endpoint id".into()));
                return;
            }
        };
        // Clear any TTFDB value from a prior run; the per-connection
        // ttfdb watcher in run_monitor publishes the new one.
        self.events.ttfdb(None);
        let addr = EndpointAddr::from_parts(parsed, std::iter::empty());
        // Dialing plus hole punching can take seconds, so the monitor runs
        // on its own task and the command pump stays responsive. Abort any
        // monitor from a previous dial first, then own the new one.
        if let Some(prev) = self.monitor.take() {
            prev.abort();
        }
        // Mark the dial active before spawning, after aborting the previous
        // monitor, so the paths sampler suppresses path-RTT latency for this
        // dial's whole lifetime. run_monitor clears it at its natural end;
        // Disconnect clears it on abort.
        self.dial_active.store(true, Ordering::Release);
        let endpoint = self.endpoint.clone();
        let conn_slot = self.conn_slot.clone();
        let events = self.events.clone();
        let dial_active = self.dial_active.clone();
        self.monitor = Some(tokio::spawn(async move {
            run_monitor(endpoint, addr, conn_slot, events).await;
            // Natural end of the dial. If a re-dial's store(true) (after its
            // abort of this task) interleaves with this store, the flag can
            // be transiently stale by at most one sampler tick; an aborted
            // task never reaches this line, so a re-dial that aborts us
            // cannot be clobbered.
            dial_active.store(false, Ordering::Release);
        }));
    }

    async fn disconnect(&mut self) {
        // Abort the monitor task. Its own cleanup (including clearing
        // dial_active) does not run on abort, so we do it here: clear the
        // dial flag, conn_slot, and close the connection.
        if let Some(task) = self.monitor.take() {
            task.abort();
        }
        self.dial_active.store(false, Ordering::Release);
        if let Some(conn) = self.conn_slot.lock().await.take() {
            conn.close(0u32.into(), b"disconnect");
        }
        // Clear the UI metric so a later reconnect starts clean.
        self.events.ttfdb(None);
        self.events.state(ConnectionState::Ready);
    }
}

/// Awaits the endpoint's net report with a hard ceiling so a UI can
/// surface a clear error instead of hanging on a broken network.
///
/// Note: `Watcher::initialized()` returns the cached value on subsequent
/// calls. A UI's refresh button therefore shows whatever the iroh-internal
/// reporter has most recently observed; it does not force a fresh probe.
/// iroh's reporter updates on its own schedule.
async fn net_report(endpoint: &Endpoint) -> Result<iroh::NetReport, String> {
    use iroh::Watcher as _;
    tokio::time::timeout(NET_REPORT_TIMEOUT, endpoint.net_report().initialized())
        .await
        .map_err(|_| {
            format!(
                "net_report did not complete within {}s",
                NET_REPORT_TIMEOUT.as_secs()
            )
        })
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
    // The ALPN list is installed by the protocol router at spawn time.
    builder.bind().await.context("bind endpoint")
}

async fn start_services_client(
    endpoint: &Endpoint,
    telemetry_disabled: bool,
    api_secret_override: &str,
    events: &Events,
) -> Option<ServicesClient> {
    // `IROH_SERVICES_API_SECRET` wins when set (empty value opts out).
    // Otherwise telemetry is on by default with the bundled key: it resolves to
    // `None` only when the user turned it off or an empty env override opts out.
    let Some(secret) =
        crate::services::resolve_api_secret(crate::services::SecretSource::AppDefault {
            disabled: telemetry_disabled,
            custom: api_secret_override,
        })
    else {
        events.telemetry(TelemetryState::Off);
        return None;
    };

    events.telemetry(TelemetryState::Starting);
    let name = crate::services::device_name(&endpoint.id().to_string());
    match crate::services::build_client(endpoint, &secret, &name).await {
        Ok(client) => {
            events.telemetry(TelemetryState::Active { name });
            Some(client)
        }
        Err(e) => {
            events.telemetry(TelemetryState::Error(format!("{e:#}")));
            None
        }
    }
}

/// Returns whether `s` looks like a peer endpoint id (64 hex characters),
/// the shape a front end can validate before sending [`NodeCommand::Connect`].
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
