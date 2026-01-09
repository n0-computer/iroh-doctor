use std::{sync::Arc, time::Duration};

use indicatif::ProgressBar;
use iroh::{
    endpoint::{AfterHandshakeOutcome, BeforeConnectOutcome, ConnectionInfo, EndpointHooks},
    TransportAddr, Watcher,
};
use n0_future::stream::StreamExt;
use tokio::{
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
    task::JoinSet,
};
use tokio_util::task::AbortOnDropHandle;
use tracing::{info, info_span, Instrument};

/// Connection monitor that implements EndpointHooks.
/// Updates a progress bar with connection info and logs connection events.
#[derive(Clone, Debug)]
pub struct Monitor {
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
    pub fn new(target: ProgressBar) -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(Self::run(target, rx).instrument(info_span!("monitor")));
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

                    // Spawn task to watch this connection's paths and update remote_info
                    let target_clone = target.clone();
                    let paths = conn.paths();
                    tasks.spawn(Self::watch_paths(target_clone, paths).instrument(tracing::Span::current()));

                    // Spawn task to log when connection closes
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
                Some(res) = tasks.join_next(), if !tasks.is_empty() => {
                    if let Err(e) = res {
                        tracing::warn!("monitor task panicked: {e}");
                    }
                }
                else => break,
            }
        }
    }

    async fn watch_paths(
        target: ProgressBar,
        paths: impl Watcher<Value = iroh::endpoint::PathInfoList> + Send + Unpin,
    ) {
        let format_latency = |x: Option<Duration>| {
            x.map(|x| format!("{:.6}s", x.as_secs_f64()))
                .unwrap_or_else(|| "unknown".to_string())
        };

        let mut stream = paths.stream();
        while let Some(infos) = stream.next().await {
            let rtt = infos.iter().map(|p| p.rtt()).min();
            let latency = format_latency(rtt);

            let mut relay_urls = Vec::new();
            let mut ips = Vec::new();
            let mut conn_type = None;

            for info in infos.iter() {
                match info.remote_addr() {
                    TransportAddr::Relay(addr) => relay_urls.push(addr.to_string()),
                    TransportAddr::Ip(addr) => ips.push(addr.to_string()),
                    _ => {}
                }
                if info.is_selected() {
                    conn_type.replace(info.remote_addr());
                }
            }

            let relay_urls = relay_urls.join("; ");
            let addrs = ips.join("; ");
            let conn_type = match conn_type {
                None => "unknown",
                Some(TransportAddr::Relay(_)) => "relay",
                Some(TransportAddr::Ip(_)) => "direct",
                Some(_) => "unknown",
            };

            target.set_message(format!(
                "relay urls: [{relay_urls}], latency: {latency}, connection type: {conn_type}, addrs: [{addrs}]"
            ));
        }
    }
}
