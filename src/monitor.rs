use std::sync::Arc;
use std::time::Duration;

use indicatif::ProgressBar;
use iroh::endpoint::{AfterHandshakeOutcome, BeforeConnectOutcome, ConnectionInfo, EndpointHooks};
use iroh::{TransportAddr, Watcher};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinSet;
use tokio_util::task::AbortOnDropHandle;
use tracing::{info, info_span, Instrument};

/// Our connection monitor impl.
///
/// This here only logs connection open and close events via tracing.
/// It could also maintain a datastructure of all connections, or send the stats to some metrics service.
#[derive(Clone, Debug)]
struct Monitor {
    tx: UnboundedSender<ConnectionInfo>,
    _task: Arc<AbortOnDropHandle<()>>,
}

impl EndpointHooks for Monitor {
    async fn before_connect<'a>(
        &'a self,
        _remote_addr: &'a iroh::EndpointAddr,
        _alpn: &'a [u8],
    ) -> BeforeConnectOutcome {
        BeforeConnectOutcome::Accept
    }

    async fn after_handshake(&self, conn: &ConnectionInfo) -> AfterHandshakeOutcome {
        self.tx.send(conn.clone()).ok();
        AfterHandshakeOutcome::Accept
    }
}

impl Monitor {
    fn new(target: ProgressBar) -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(Self::run(target, rx).instrument(info_span!("watcher")));
        Self {
            tx,
            _task: Arc::new(AbortOnDropHandle::new(task)),
        }
    }

    async fn run(target: ProgressBar, mut rx: UnboundedReceiver<ConnectionInfo>) {
        let mut tasks = JoinSet::new();
        loop {
            tokio::select! {
                Some(conn) = rx.recv() => {
                    let alpn = String::from_utf8_lossy(conn.alpn()).to_string();
                    let remote = conn.remote_id().fmt_short();
                    Self::update_remote_info(&target, &conn);

                    tasks.spawn(async move {
                        match conn.closed().await {
                            Some((close_reason, stats)) => {
                                // We have access to the final stats of the connection!
                                info!(%remote, %alpn, ?close_reason, udp_rx=stats.udp_rx.bytes, udp_tx=stats.udp_tx.bytes, "connection closed");
                            }
                            None => {
                                // The connection was closed before we could register our stats-on-close listener.
                                info!(%remote, %alpn, "connection closed before tracking started");
                            }
                        }
                    }.instrument(tracing::Span::current()));
                }
                Some(res) = tasks.join_next(), if !tasks.is_empty() => res.expect("conn close task panicked"),
                else => break,
            }
            while let Some(res) = tasks.join_next().await {
                res.expect("conn close task panicked");
            }
        }
    }

    /// Updates the information of the target progress bar.
    async fn update_remote_info(target: &ProgressBar, conn: &ConnectionInfo) {
        let format_latency = |x: Option<Duration>| {
            x.map(|x| format!("{:.6}s", x.as_secs_f64()))
                .unwrap_or_else(|| "unknown".to_string())
        };
        let rtt = conn.paths().peek().iter().map(|p| p.stats().rtt).min();
        let latency = format_latency(rtt);

        let mut relay_urls = Vec::new();
        let mut ips = Vec::new();
        let mut conn_type = None;

        let infos = conn.paths().get();
        for info in infos.iter() {
            match info.remote_addr() {
                TransportAddr::Relay(addr) => {
                    relay_urls.push(addr);
                }
                TransportAddr::Ip(addr) => {
                    ips.push(addr);
                }
                _ => {}
            }

            if info.is_selected() {
                conn_type.replace(info.remote_addr());
            }
        }
        let relay_urls = relay_urls
            .into_iter()
            .map(|relay_url| relay_url.to_string())
            .collect::<Vec<_>>()
            .join("; ");

        let addrs = ips
            .into_iter()
            .map(|addr| addr.to_string())
            .collect::<Vec<_>>()
            .join("; ");

        let conn_type = match conn_type {
            None => "unknown",
            Some(TransportAddr::Relay(_)) => "relay",
            Some(TransportAddr::Ip(_)) => "direct",
            Some(_) => "unknown",
        };

        let msg = format!(
            "relay urls: [{relay_urls}], latency: {latency}, connection type: {conn_type}, addrs: [{addrs}]"
        );

        target.set_message(msg);
    }
}
