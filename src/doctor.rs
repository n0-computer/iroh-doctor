//! Tool to get information about the current network environment of a node,
//! and to test connectivity to specific other nodes.

use std::{
    net::SocketAddr,
    num::NonZeroU16,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::Context;
use clap::Subcommand;
use indicatif::{HumanBytes, MultiProgress, ProgressBar};
use iroh::{
    discovery::{dns::DnsDiscovery, pkarr::PkarrPublisher, ConcurrentDiscovery},
    endpoint::{self, Connection, ConnectionType, RecvStream, SendStream},
    metrics::MagicsockMetrics,
    Endpoint, EndpointId, RelayConfig, RelayMap, RelayMode, RelayUrl, SecretKey, Watcher,
};
use iroh_metrics::static_core::Core;
use iroh_relay::RelayQuicConfig;
use postcard::experimental::max_size::MaxSize;
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncWriteExt, sync};
use tokio_util::task::AbortOnDropHandle;

use crate::{
    commands,
    config::{iroh_data_root, NodeConfig},
    metrics::{IrohMetricsRegistry, MetricsRegistry},
    progress::ProgressWriter,
};

/// Options for the secret key usage.
#[derive(Debug, Clone, derive_more::Display)]
pub enum SecretKeyOption {
    /// Generate random secret key
    Random,
    /// Use local secret key
    Local,
    /// Explicitly specify a secret key
    Hex(String),
}

impl std::str::FromStr for SecretKeyOption {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s_lower = s.to_ascii_lowercase();
        Ok(if s_lower == "random" {
            SecretKeyOption::Random
        } else if s_lower == "local" {
            SecretKeyOption::Local
        } else {
            SecretKeyOption::Hex(s.to_string())
        })
    }
}

/// Subcommands for the iroh doctor.
#[derive(Subcommand, Debug, Clone)]
pub enum Commands {
    /// Report on the current network environment, using either an explicitly provided stun host
    /// or the settings from the config file.
    ///
    /// When no protocol flags are explicitly set, will run a report with all available probe
    /// protocols
    Report {
        /// Run a report including a QUIC Address Discovery probe over Ipv6
        ///
        /// When all protocol flags are false, will
        /// run a report with all available protocols
        #[clap(long, default_value_t = false)]
        quic_ipv4: bool,
        /// Run a report including a QUIC Address Discovery probe over Ipv6
        ///
        /// When all protocol flags are false, will
        /// run a report with all available protocols
        #[clap(long, default_value_t = false)]
        quic_ipv6: bool,
        /// Run a report including an HTTPS probe
        ///
        /// When all protocol flags are false, will
        /// run a report with all available protocols
        #[clap(long, default_value_t = false)]
        https: bool,
    },
    /// Wait for incoming requests from iroh doctor connect.
    Accept {
        /// Our own secret key, in hex. If not specified, the locally configured key will be used.
        #[clap(long, default_value_t = SecretKeyOption::Local)]
        secret_key: SecretKeyOption,

        /// Number of bytes to send to the remote for each test.
        #[clap(long, default_value_t = 1024 * 1024 * 16)]
        size: u64,

        /// Number of iterations to run the test for. If not specified, the test will run forever.
        #[clap(long)]
        iterations: Option<u64>,

        /// Use a local relay.
        #[clap(long)]
        local_relay_server: bool,

        /// Do not allow the node to dial and be dialed by id only.
        ///
        /// This disables DNS discovery, which would allow the node to dial other nodes by id only.
        /// And it disables Pkarr Publishing, which would allow the node to announce its address for dns discovery.
        ///
        /// Default is `false`
        #[clap(long, default_value_t = false)]
        disable_discovery: bool,

        /// Bind to this specific socket address.
        ///
        /// Default is `None`, which means the endpoint will bind to a random port.
        #[clap(long)]
        socket_addr: Option<SocketAddr>,
    },
    /// Connect to an iroh doctor accept node.
    Connect {
        /// Hexadecimal node id of the node to connect to.
        dial: EndpointId,

        /// One or more remote endpoints to use when dialing.
        #[clap(long)]
        remote_endpoint: Vec<SocketAddr>,

        /// Our own secret key, in hex. If not specified, a random key will be generated.
        #[clap(long, default_value_t = SecretKeyOption::Random)]
        secret_key: SecretKeyOption,

        /// Use a local relay:
        ///
        /// Overrides the `relay_url` field.
        #[clap(long)]
        local_relay_server: bool,

        /// The relay url the peer you are dialing can be found on.
        ///
        /// If `local_relay_server` is true, this field is ignored.
        ///
        /// When `None`, or if attempting to dial an unknown url, no hole punching can occur.
        ///
        /// Default is `None`.
        #[clap(long)]
        relay_url: Option<RelayUrl>,

        /// Do not allow the node to dial and be dialed by id only.
        ///
        /// This disables DNS discovery, which would allow the node to dial other nodes by id only.
        /// It also disables Pkarr Publishing, which would allow the node to announce its address for DNS discovery.
        ///
        /// Default is `false`
        #[clap(long, default_value_t = false)]
        disable_discovery: bool,

        /// Bind to this specific socket address.
        ///
        /// Default is `None`, which means the endpoint will bind to a random port.
        #[clap(long)]
        socket_addr: Option<SocketAddr>,
    },
    /// Probe the port mapping protocols.
    PortMapProbe {
        /// Whether to enable UPnP.
        #[clap(long)]
        enable_upnp: bool,
        /// Whether to enable PCP.
        #[clap(long)]
        enable_pcp: bool,
        /// Whether to enable NAT-PMP.
        #[clap(long)]
        enable_nat_pmp: bool,
    },
    /// Attempt to get a port mapping to the given local port.
    PortMap {
        /// Protocol to use for port mapping. One of ["upnp", "nat_pmp", "pcp"].
        protocol: String,
        /// Local port to get a mapping.
        local_port: NonZeroU16,
        /// How long to wait for an external port to be ready in seconds.
        #[clap(long, default_value_t = 10)]
        timeout_secs: u64,
    },
    /// Get the latencies of the different relay url
    ///
    /// Tests the latencies of the default relay url and nodes. To test custom urls or nodes,
    /// adjust the `Config`.
    RelayUrls {
        /// How often to execute.
        #[clap(long, default_value_t = 5)]
        count: usize,
    },
    /// Plot metric counters
    Plot {
        /// How often to collect samples in milliseconds.
        #[clap(long, default_value_t = 500)]
        interval: u64,
        /// Which metrics to plot. Commas separated list of metric names.
        metrics: String,
        /// What the plotted time frame should be in seconds.
        #[clap(long, default_value_t = 60)]
        timeframe: usize,
        /// Endpoint to scrape for prometheus metrics
        #[clap(long, default_value = "http://localhost:9090")]
        scrape_url: String,
        /// File to read the metrics from. Takes precedence over scrape_url.
        #[clap(long)]
        file: Option<PathBuf>,
    },
    /// Join a doctor swarm as a test node
    SwarmClient {
        /// SSH private key path for authentication
        #[clap(long, required = true)]
        ssh_key: PathBuf,
        /// Backend coordinator node ID
        #[clap(long, required = true)]
        coordinator: EndpointId,
        /// Assignment polling interval in seconds
        #[clap(long, default_value_t = 2)]
        assignment_interval: u64,
        /// Optional human-readable name for this node
        #[clap(long)]
        named: Option<String>,
        /// The secret key to use for the node.
        /// Can be "random", "local" (loads from ~/.config/iroh/keypair), or a hex-encoded secret key.
        #[clap(long, default_value = "random")]
        secret_key: SecretKeyOption,
    },
}

/// Possible streams that can be requested.
#[derive(Debug, Serialize, Deserialize, MaxSize)]
pub enum TestStreamRequest {
    Echo { bytes: u64 },
    Drain { bytes: u64 },
    Send { bytes: u64, block_size: u32 },
}

/// Configuration for testing.
#[derive(Debug, Clone, Copy)]
pub struct TestConfig {
    pub size: u64,
    pub iterations: Option<u64>,
}

/// Updates the progress bar.
fn update_pb(
    task: &'static str,
    pb: Option<ProgressBar>,
    total_bytes: u64,
    mut updates: sync::mpsc::Receiver<u64>,
) -> tokio::task::JoinHandle<()> {
    if let Some(pb) = pb {
        pb.set_message(task);
        pb.set_position(0);
        pb.set_length(total_bytes);
        tokio::spawn(async move {
            while let Some(position) = updates.recv().await {
                pb.set_position(position);
            }
        })
    } else {
        tokio::spawn(std::future::ready(()))
    }
}

/// Handles a test stream request.
async fn handle_test_request(
    mut send: SendStream,
    mut recv: RecvStream,
    gui: &Gui,
) -> anyhow::Result<()> {
    let mut buf = [0u8; TestStreamRequest::POSTCARD_MAX_SIZE];
    recv.read_exact(&mut buf).await?;
    let request: TestStreamRequest = postcard::from_bytes(&buf)?;
    let pb = Some(gui.pb.clone());
    match request {
        TestStreamRequest::Echo { bytes } => {
            // copy the stream back
            let (mut send, updates) = ProgressWriter::new(&mut send);
            let t0 = Instant::now();
            let progress = update_pb("echo", pb, bytes, updates);
            tokio::io::copy(&mut recv, &mut send).await?;
            let elapsed = t0.elapsed();
            drop(send);
            progress.await?;
            gui.set_echo(bytes, elapsed);
        }
        TestStreamRequest::Drain { bytes } => {
            // drain the stream
            let (mut send, updates) = ProgressWriter::new(tokio::io::sink());
            let progress = update_pb("recv", pb, bytes, updates);
            let t0 = Instant::now();
            tokio::io::copy(&mut recv, &mut send).await?;
            let elapsed = t0.elapsed();
            drop(send);
            progress.await?;
            gui.set_recv(bytes, elapsed);
        }
        TestStreamRequest::Send { bytes, block_size } => {
            // send the requested number of bytes, in blocks of the requested size
            let (mut send, updates) = ProgressWriter::new(&mut send);
            let progress = update_pb("send", pb, bytes, updates);
            let t0 = Instant::now();
            send_blocks(&mut send, bytes, block_size).await?;
            drop(send);
            let elapsed = t0.elapsed();
            progress.await?;
            gui.set_send(bytes, elapsed);
        }
    }
    send.finish()?;
    Ok(())
}

/// Sends the requested number of bytes, in blocks of the requested size.
async fn send_blocks(
    mut send: impl tokio::io::AsyncWrite + Unpin,
    total_bytes: u64,
    block_size: u32,
) -> anyhow::Result<()> {
    let buf = vec![0u8; block_size as usize];
    let mut remaining = total_bytes;
    while remaining > 0 {
        let n = remaining.min(block_size as u64);
        send.write_all(&buf[..n as usize]).await?;
        remaining -= n;
    }
    Ok(())
}

/// Contains all the GUI state.
pub struct Gui {
    #[allow(dead_code)]
    pub mp: MultiProgress,
    pub pb: ProgressBar,
    #[allow(dead_code)]
    pub counters: ProgressBar,
    pub send_pb: ProgressBar,
    pub recv_pb: ProgressBar,
    pub echo_pb: ProgressBar,
    #[allow(dead_code)]
    pub counter_task: Option<AbortOnDropHandle<()>>,
}

impl Gui {
    /// Create a new GUI struct.
    pub fn new(endpoint: Endpoint, node_id: EndpointId) -> Self {
        let mp = MultiProgress::new();
        mp.set_draw_target(indicatif::ProgressDrawTarget::stderr());
        let counters = mp.add(ProgressBar::hidden());
        let remote_info = mp.add(ProgressBar::hidden());
        let send_pb = mp.add(ProgressBar::hidden());
        let recv_pb = mp.add(ProgressBar::hidden());
        let echo_pb = mp.add(ProgressBar::hidden());
        let style = indicatif::ProgressStyle::default_bar()
            .template("{msg}")
            .unwrap();
        send_pb.set_style(style.clone());
        recv_pb.set_style(style.clone());
        echo_pb.set_style(style.clone());
        remote_info.set_style(style.clone());
        counters.set_style(style);
        let pb = mp.add(indicatif::ProgressBar::hidden());
        pb.enable_steady_tick(Duration::from_millis(100));
        pb.set_style(indicatif::ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:80.cyan/blue}] {msg} {bytes}/{total_bytes} ({bytes_per_sec})").unwrap()
            .progress_chars("█▉▊▋▌▍▎▏ "));
        let counters2 = counters.clone();
        let counter_task = tokio::spawn(async move {
            loop {
                Self::update_counters(&counters2);
                Self::update_remote_info(&remote_info, &endpoint, &node_id).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
        Self {
            mp,
            pb,
            counters,
            send_pb,
            recv_pb,
            echo_pb,
            counter_task: Some(AbortOnDropHandle::new(counter_task)),
        }
    }

    /// Updates the information of the target progress bar.
    async fn update_remote_info(target: &ProgressBar, endpoint: &Endpoint, node_id: &EndpointId) {
        let format_latency = |x: Option<Duration>| {
            x.map(|x| format!("{:.6}s", x.as_secs_f64()))
                .unwrap_or_else(|| "unknown".to_string())
        };
        let conn_type = endpoint.conn_type(*node_id).map(|mut c| c.get());

        let msg = if let Some(conn_type) = conn_type {
            let latency = format_latency(endpoint.latency(*node_id).await);
            let node_addr = endpoint.addr();
            let relay_url = node_addr.relay_urls().next();
            let relay_url = relay_url
                .map(|relay_url| relay_url.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let addrs = node_addr
                .ip_addrs()
                .map(|addr| addr.to_string())
                .collect::<Vec<_>>()
                .join("; ");

            format!(
                "relay url: {relay_url}, latency: {latency}, connection type: {conn_type}, addrs: [{addrs}]"
            )
        } else {
            "connection info unavailable".to_string()
        };
        target.set_message(msg);
    }

    /// Updates the counters for the target progress bar.
    fn update_counters(target: &ProgressBar) {
        if let Some(core) = Core::get() {
            let metrics = core.get_collector::<MagicsockMetrics>().unwrap();
            let send_ipv4 = HumanBytes(metrics.send_ipv4.get());
            let send_ipv6 = HumanBytes(metrics.send_ipv6.get());
            let send_relay = HumanBytes(metrics.send_relay.get());
            let recv_data_relay = HumanBytes(metrics.recv_data_relay.get());
            let recv_data_ipv4 = HumanBytes(metrics.recv_data_ipv4.get());
            let recv_data_ipv6 = HumanBytes(metrics.recv_data_ipv6.get());

            let latency_histogram = Self::format_histogram(&metrics.connection_latency_ms);

            let text = format!(
                r#"Counters

Relay:
  send: {send_relay}
  recv: {recv_data_relay}
Ipv4:
  send: {send_ipv4}
  recv: {recv_data_ipv4}
Ipv6:
  send: {send_ipv6}
  recv: {recv_data_ipv6}

Connection Latency Distribution (ms):
{latency_histogram}
"#,
            );
            target.set_message(text);
        }
    }

    /// Formats histogram bucket distribution for display.
    fn format_histogram(histogram: &iroh_metrics::Histogram) -> String {
        let count = histogram.count();
        if count == 0 {
            return "  No data collected yet".to_string();
        }

        let buckets = histogram.buckets();
        let mut result = String::new();

        let mut prev_bound = 0.0;
        for (upper_bound, cumulative_count) in buckets {
            if upper_bound.is_infinite() {
                result.push_str(&format!(
                    "  {:.1}+ ms: {} samples\n",
                    prev_bound, cumulative_count
                ));
            } else {
                result.push_str(&format!(
                    "  {:.1}-{:.1} ms: {} samples\n",
                    prev_bound, upper_bound, cumulative_count
                ));
                prev_bound = upper_bound;
            }
        }

        let sum = histogram.sum();
        let avg = sum / count as f64;
        let p50 = histogram.percentile(0.5);
        let p95 = histogram.percentile(0.95);
        let p99 = histogram.percentile(0.99);

        result.push_str(&format!(
            "  Total: {} samples, Avg: {:.2}ms, p50: {:.2}ms, p95: {:.2}ms, p99: {:.2}ms",
            count, avg, p50, p95, p99
        ));

        result
    }

    /// Sets the "send" text and the speed for the progress bar.
    fn set_send(&self, bytes: u64, duration: Duration) {
        Self::set_bench_speed(&self.send_pb, "send", bytes, duration);
    }

    /// Sets the "recv" text and the speed for the progress bar.
    fn set_recv(&self, bytes: u64, duration: Duration) {
        Self::set_bench_speed(&self.recv_pb, "recv", bytes, duration);
    }

    /// Sets the "echo" text and the speed for the progress bar.
    fn set_echo(&self, bytes: u64, duration: Duration) {
        Self::set_bench_speed(&self.echo_pb, "echo", bytes, duration);
    }

    /// Sets a text and the speed for the progress bar.
    fn set_bench_speed(pb: &ProgressBar, text: &str, bytes: u64, duration: Duration) {
        pb.set_message(format!(
            "{}: {}/s",
            text,
            HumanBytes((bytes as f64 / duration.as_secs_f64()) as u64)
        ));
    }

    /// Clears the [`MultiProgress`] field.
    pub fn clear(&self) {
        self.mp.clear().ok();
    }
}

/// Sends, receives and echoes data in a connection.
pub async fn active_side(
    connection: &Connection,
    config: &TestConfig,
    gui: Option<&Gui>,
) -> anyhow::Result<()> {
    let n = config.iterations.unwrap_or(u64::MAX);
    if let Some(gui) = gui {
        let pb = Some(&gui.pb);
        for _ in 0..n {
            let d = send_test(connection, config, pb).await?;
            gui.set_send(config.size, d);
            let d = recv_test(connection, config, pb).await?;
            gui.set_recv(config.size, d);
            let d = echo_test(connection, config, pb).await?;
            gui.set_echo(config.size, d);
        }
    } else {
        let pb = None;
        for _ in 0..n {
            let _d = send_test(connection, config, pb).await?;
            let _d = recv_test(connection, config, pb).await?;
            let _d = echo_test(connection, config, pb).await?;
        }
    }

    // Close the connection gracefully.
    // We're always the ones last receiving data, because
    // `echo_test` waits for data on the connection as the last thing.
    connection.close(0u32.into(), b"done");
    connection.closed().await;

    Ok(())
}

/// Sends a test request in a connection.
async fn send_test_request(
    send: &mut SendStream,
    request: &TestStreamRequest,
) -> anyhow::Result<()> {
    let mut buf = [0u8; TestStreamRequest::POSTCARD_MAX_SIZE];
    postcard::to_slice(&request, &mut buf)?;
    send.write_all(&buf).await?;
    Ok(())
}

/// Echoes test a connection.
async fn echo_test(
    connection: &Connection,
    config: &TestConfig,
    pb: Option<&indicatif::ProgressBar>,
) -> anyhow::Result<Duration> {
    let size = config.size;
    let (mut send, mut recv) = connection.open_bi().await?;
    send_test_request(&mut send, &TestStreamRequest::Echo { bytes: size }).await?;
    let (mut sink, updates) = ProgressWriter::new(tokio::io::sink());
    let copying = tokio::spawn(async move { tokio::io::copy(&mut recv, &mut sink).await });
    let progress = update_pb("echo", pb.cloned(), size, updates);
    let t0 = Instant::now();
    send_blocks(&mut send, size, 1024 * 1024).await?;
    send.finish()?;
    let received = copying.await??;
    anyhow::ensure!(received == size);
    let duration = t0.elapsed();
    progress.await?;
    Ok(duration)
}

/// Sends test a connection.
async fn send_test(
    connection: &Connection,
    config: &TestConfig,
    pb: Option<&indicatif::ProgressBar>,
) -> anyhow::Result<Duration> {
    let size = config.size;
    let (mut send, mut recv) = connection.open_bi().await?;
    send_test_request(&mut send, &TestStreamRequest::Drain { bytes: size }).await?;
    let (mut send_with_progress, updates) = ProgressWriter::new(&mut send);
    let copying =
        tokio::spawn(async move { tokio::io::copy(&mut recv, &mut tokio::io::sink()).await });
    let progress = update_pb("send", pb.cloned(), size, updates);
    let t0 = Instant::now();
    send_blocks(&mut send_with_progress, size, 1024 * 1024).await?;
    drop(send_with_progress);
    send.finish()?;
    drop(send);
    let received = copying.await??;
    anyhow::ensure!(received == 0);
    let duration = t0.elapsed();
    progress.await?;
    Ok(duration)
}

/// Receives test a connection.
async fn recv_test(
    connection: &Connection,
    config: &TestConfig,
    pb: Option<&indicatif::ProgressBar>,
) -> anyhow::Result<Duration> {
    let size = config.size;
    let (mut send, mut recv) = connection.open_bi().await?;
    let t0 = Instant::now();
    let (mut sink, updates) = ProgressWriter::new(tokio::io::sink());
    send_test_request(
        &mut send,
        &TestStreamRequest::Send {
            bytes: size,
            block_size: 1024 * 1024,
        },
    )
    .await?;
    let copying = tokio::spawn(async move { tokio::io::copy(&mut recv, &mut sink).await });
    let progress = update_pb("recv", pb.cloned(), size, updates);
    send.finish()?;
    let received = copying.await??;
    anyhow::ensure!(received == size);
    let duration = t0.elapsed();
    progress.await?;
    Ok(duration)
}

/// Accepts connections and answers requests (echo, drain or send) as passive side.
pub async fn passive_side(gui: Gui, connection: &Connection) -> anyhow::Result<()> {
    let conn = connection.clone();
    let accept_loop = async move {
        let result = loop {
            match conn.accept_bi().await {
                Ok((send, recv)) => {
                    if let Err(cause) = handle_test_request(send, recv, &gui).await {
                        eprintln!("Error handling test request {cause}");
                    }
                }
                Err(cause) => {
                    eprintln!("error accepting bidi stream {cause}");
                    break Err(cause.into());
                }
            };
        };

        conn.close(0u32.into(), b"internal err");
        conn.closed().await;
        eprintln!("Connection closed.");

        result
    };
    let conn_closed = async move {
        connection.closed().await;
        eprintln!("Connection closed.");
        anyhow::Ok(())
    };
    futures_lite::future::race(conn_closed, accept_loop).await
}

/// Configures a relay map with some default values.
fn configure_local_relay_map() -> RelayMap {
    let url = "http://localhost:3340".parse().unwrap();
    RelayMap::from(RelayConfig {
        url,
        quic: Some(RelayQuicConfig::default()),
    })
}

/// ALPN protocol address.
pub const DR_RELAY_ALPN: [u8; 11] = *b"n0/doctor/1";

/// Creates an iroh net [`Endpoint`] from a [SecreetKey`], a [`RelayMap`] and a [`Discovery`].
async fn make_endpoint(
    secret_key: SecretKey,
    relay_map: Option<RelayMap>,
    discovery: Option<ConcurrentDiscovery>,
    service_node: Option<EndpointId>,
    ssh_key: Option<PathBuf>,
    metrics: IrohMetricsRegistry,
    socket_addr: Option<SocketAddr>,
) -> anyhow::Result<(Endpoint, Option<iroh_n0des::Client>)> {
    tracing::info!(
        "public key: {}",
        hex::encode(secret_key.public().as_bytes())
    );
    tracing::info!("relay map {:#?}", relay_map);

    let mut transport_config = endpoint::TransportConfig::default();
    transport_config.keep_alive_interval(Some(Duration::from_secs(5)));
    transport_config.max_idle_timeout(Some(Duration::from_secs(10).try_into().unwrap()));

    let mut endpoint = Endpoint::builder()
        .secret_key(secret_key)
        .alpns(vec![DR_RELAY_ALPN.to_vec()])
        .transport_config(transport_config);

    if let Some(address) = socket_addr {
        match address {
            SocketAddr::V6(addr) => {
                endpoint = endpoint.bind_addr_v6(addr);
            }
            SocketAddr::V4(addr) => {
                endpoint = endpoint.bind_addr_v4(addr);
            }
        }
    }

    let endpoint = match discovery {
        Some(discovery) => endpoint.discovery(discovery),
        None => endpoint,
    };

    let endpoint = match relay_map {
        Some(relay_map) => endpoint.relay_mode(RelayMode::Custom(relay_map)),
        None => endpoint,
    };
    let endpoint = endpoint.bind().await?;

    {
        let mut registry = metrics.write().expect("poisoned");
        registry.register_all(endpoint.metrics());
    }

    let rpc_client = if let Some(remote_node) = service_node {
        // Grab ssh key
        let ssh_key_path = ssh_key.expect("missing ssh key location");
        let client = iroh_n0des::Client::builder(&endpoint)
            .ssh_key_from_file(ssh_key_path)
            .await?
            .remote(remote_node)
            .build()
            .await?;
        Some(client)
    } else {
        None
    };

    tokio::time::timeout(Duration::from_secs(10), endpoint.online())
        .await
        .context("wait for relay connection")?;

    Ok((endpoint, rpc_client))
}

pub async fn close_endpoint_on_ctrl_c(endpoint: Endpoint) {
    tokio::signal::ctrl_c()
        .await
        .expect("failed listening to SIGINT");
    endpoint.close().await;
}

/// Formats a [`SocketAddr`] so that console doesn't escape it.
pub fn format_addr(addr: SocketAddr) -> String {
    if addr.is_ipv6() {
        format!("'{addr}'")
    } else {
        format!("{addr}")
    }
}

/// Logs the connection changes to the multiprogress.
pub fn log_connection_changes(
    pb: MultiProgress,
    node_id: EndpointId,
    mut conn_type: n0_watcher::Direct<ConnectionType>,
) {
    tokio::spawn(async move {
        let start = Instant::now();
        while let Ok(conn_type) = conn_type.updated().await {
            pb.println(format!(
                "Connection with {node_id:#} changed: {conn_type} (after {:?})",
                start.elapsed()
            ))
            .ok();
        }
    });
}

/// Creates a [`SecretKey`] from a [`SecretKeyOption`].
fn create_secret_key(secret_key: SecretKeyOption) -> anyhow::Result<SecretKey> {
    Ok(match secret_key {
        SecretKeyOption::Random => SecretKey::generate(&mut rand::rng()),
        SecretKeyOption::Hex(hex) => {
            let bytes = hex::decode(hex)?;
            SecretKey::try_from(&bytes[..])?
        }
        SecretKeyOption::Local => {
            let path = iroh_data_root()?.join("keypair");
            if path.exists() {
                let bytes = std::fs::read(&path)?;
                try_secret_key_from_openssh(bytes)?
            } else {
                println!(
                    "Local key not found in {}. Using random key.",
                    path.display()
                );
                SecretKey::generate(&mut rand::rng())
            }
        }
    })
}

/// Creates a [`Discovery`] service from a [`SecretKey`].
fn create_discovery(
    disable_discovery: bool,
    secret_key: &SecretKey,
) -> Option<ConcurrentDiscovery> {
    if disable_discovery {
        None
    } else {
        Some(ConcurrentDiscovery::from_services(vec![
            // Enable DNS discovery by default
            Box::new(DnsDiscovery::n0_dns().build()),
            // Enable pkarr publishing by default
            Box::new(PkarrPublisher::n0_dns().build(secret_key.clone())),
        ]))
    }
}

/// Runs the doctor commands.
pub async fn run(
    command: Commands,
    config: &NodeConfig,
    service_node: Option<EndpointId>,
    ssh_key: Option<PathBuf>,
) -> anyhow::Result<()> {
    let data_dir = iroh_data_root()?;
    let _guard = crate::logging::init_terminal_and_file_logging(&config.file_logs, &data_dir)?;

    let metrics = MetricsRegistry::default();
    // doesn't start the server if the address is None
    let metrics_clone = metrics.clone();
    let metrics_fut = config.metrics_addr.map(|metrics_addr| {
        tokio::task::spawn(async move {
            if let Err(e) =
                iroh_metrics::service::start_metrics_server(metrics_addr, metrics_clone.clone())
                    .await
            {
                eprintln!("Failed to start metrics server: {e}");
            }
        })
    });
    if metrics_fut.is_none() {
        tracing::info!("Metrics server not started, no address provided");
    }
    let cmd_res = match command {
        Commands::Report {
            quic_ipv4,
            quic_ipv6,
            https,
        } => commands::report::report(config, quic_ipv4, quic_ipv6, https).await,
        Commands::Connect {
            dial,
            secret_key,
            local_relay_server,
            relay_url,
            remote_endpoint,
            disable_discovery,
            socket_addr,
        } => {
            let (relay_map, relay_url) = if local_relay_server {
                let dm = configure_local_relay_map();
                let url = dm.urls::<Vec<_>>().pop().unwrap().clone();
                (Some(dm), Some(url))
            } else {
                (config.relay_map()?, relay_url)
            };
            let secret_key = create_secret_key(secret_key)?;
            let discovery = create_discovery(disable_discovery, &secret_key);

            let (endpoint, _client) = make_endpoint(
                secret_key.clone(),
                relay_map.clone(),
                discovery,
                service_node,
                ssh_key,
                metrics.iroh.clone(),
                socket_addr,
            )
            .await?;

            futures_lite::future::race(close_endpoint_on_ctrl_c(endpoint.clone()), async move {
                if let Err(e) =
                    commands::connect::connect(dial, remote_endpoint, relay_url, endpoint).await
                {
                    eprintln!("connect error: {e}");
                }
            })
            .await;

            Ok(())
        }
        Commands::Accept {
            secret_key,
            local_relay_server,
            size,
            iterations,
            disable_discovery,
            socket_addr,
        } => {
            let relay_map = if local_relay_server {
                Some(configure_local_relay_map())
            } else {
                config.relay_map()?
            };
            let secret_key = create_secret_key(secret_key)?;
            let config = TestConfig { size, iterations };
            let discovery = create_discovery(disable_discovery, &secret_key);

            let (endpoint, _client) = make_endpoint(
                secret_key.clone(),
                relay_map.clone(),
                discovery,
                service_node,
                ssh_key,
                metrics.iroh.clone(),
                socket_addr,
            )
            .await?;

            futures_lite::future::race(close_endpoint_on_ctrl_c(endpoint.clone()), async move {
                if let Err(e) = commands::accept::accept(secret_key, config, endpoint).await {
                    eprintln!("accept error: {e}");
                }
            })
            .await;

            Ok(())
        }
        Commands::PortMap {
            protocol,
            local_port,
            timeout_secs,
        } => {
            commands::port_map::port_map(&protocol, local_port, Duration::from_secs(timeout_secs))
                .await
        }
        Commands::PortMapProbe {
            enable_upnp,
            enable_pcp,
            enable_nat_pmp,
        } => {
            let config = portmapper::Config {
                enable_upnp,
                enable_pcp,
                enable_nat_pmp,
                protocol: portmapper::Protocol::Udp,
            };

            commands::port_map::port_map_probe(config).await
        }
        Commands::RelayUrls { count } => commands::relay_urls::relay_urls(count, config).await,
        Commands::Plot {
            interval,
            metrics,
            timeframe,
            scrape_url,
            file,
        } => commands::plot::plot(interval, metrics, timeframe, scrape_url, file).await,
        Commands::SwarmClient {
            ssh_key,
            coordinator,
            assignment_interval,
            named,
            secret_key,
        } => {
            let secret_key = create_secret_key(secret_key)?;
            commands::swarm_client::run_swarm_client(
                ssh_key,
                coordinator,
                assignment_interval,
                named,
                secret_key,
                metrics.iroh.clone(),
            )
            .await
        }
    };
    if let Some(metrics_fut) = metrics_fut {
        metrics_fut.abort();
    }
    cmd_res
}

/// Deserialise a SecretKey from OpenSSH format.
fn try_secret_key_from_openssh<T: AsRef<[u8]>>(data: T) -> anyhow::Result<SecretKey> {
    let ser_key = ssh_key::private::PrivateKey::from_openssh(data)?;
    match ser_key.key_data() {
        ssh_key::private::KeypairData::Ed25519(kp) => {
            Ok(SecretKey::from_bytes(&kp.private.to_bytes()))
        }
        _ => anyhow::bail!("invalid key format"),
    }
}
