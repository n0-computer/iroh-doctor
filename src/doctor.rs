//! Tool to get information about the current network environment of a node,
//! and to test connectivity to specific other nodes.

use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    num::NonZeroU16,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context;
use clap::Subcommand;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures_lite::StreamExt;
use futures_util::SinkExt;
use indicatif::{HumanBytes, MultiProgress, ProgressBar};
use iroh::{
    discovery::{dns::DnsDiscovery, pkarr::PkarrPublisher, ConcurrentDiscovery},
    dns::DnsResolver,
    endpoint::{self, Connection, ConnectionType, RecvStream, RemoteInfo, SendStream},
    metrics::MagicsockMetrics,
    Endpoint, NodeAddr, NodeId, RelayMap, RelayMode, RelayNode, RelayUrl, SecretKey,
};
use iroh_metrics::static_core::Core;
use iroh_relay::{client::SendMessage, RelayQuicConfig};
use n0_watcher::Watcher;
use portable_atomic::AtomicU64;
use postcard::experimental::max_size::MaxSize;
use rand::Rng;
use ratatui::{prelude::*, widgets::*};
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncWriteExt, sync};
use tokio_util::task::AbortOnDropHandle;
use tracing::warn;

use crate::{
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
    },
    /// Connect to an iroh doctor accept node.
    Connect {
        /// Hexadecimal node id of the node to connect to.
        dial: NodeId,

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
}

/// Possible streams that can be requested.
#[derive(Debug, Serialize, Deserialize, MaxSize)]
enum TestStreamRequest {
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

/// Prints a client report.
#[allow(clippy::too_many_arguments)]
async fn report(
    config: &NodeConfig,
    mut quic_ipv4: bool,
    mut quic_ipv6: bool,
    mut https: bool,
) -> anyhow::Result<()> {
    // if all protocol flags are false, set them all to true
    if !(quic_ipv4 || quic_ipv6 || https) {
        quic_ipv4 = true;
        quic_ipv6 = true;
        https = true;
    }
    println!("Probe protocols selected:");
    if quic_ipv4 {
        println!("quic ipv4")
    }
    if quic_ipv6 {
        println!("quic ipv6")
    }
    if https {
        println!("https")
    }
    let relay_map = config.relay_map()?.unwrap_or_else(RelayMap::empty);

    let endpoint = iroh::Endpoint::builder()
        .relay_mode(RelayMode::Custom(relay_map.clone()))
        .bind()
        .await?;

    println!("\nRelay Map:");
    for (url, node) in relay_map.urls().zip(relay_map.nodes()) {
        println!(
            r#"- {url}
  QUIC port: {:?}"#,
            node.quic.as_ref().map(|c| c.port),
        );
    }

    let mut stream = endpoint.net_report().stream();
    while let Some(report) = stream.next().await {
        println!("{report:#?}");
    }

    endpoint.close().await;
    Ok(())
}

/// Contains all the GUI state.
struct Gui {
    #[allow(dead_code)]
    mp: MultiProgress,
    pb: ProgressBar,
    #[allow(dead_code)]
    counters: ProgressBar,
    send_pb: ProgressBar,
    recv_pb: ProgressBar,
    echo_pb: ProgressBar,
    #[allow(dead_code)]
    counter_task: Option<AbortOnDropHandle<()>>,
}

impl Gui {
    /// Create a new GUI struct.
    fn new(endpoint: Endpoint, node_id: NodeId) -> Self {
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
                Self::update_remote_info(&remote_info, &endpoint, &node_id);
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
    fn update_remote_info(target: &ProgressBar, endpoint: &Endpoint, node_id: &NodeId) {
        let format_latency = |x: Option<Duration>| {
            x.map(|x| format!("{:.6}s", x.as_secs_f64()))
                .unwrap_or_else(|| "unknown".to_string())
        };
        let msg = match endpoint.remote_info(*node_id) {
            Some(RemoteInfo {
                relay_url,
                conn_type,
                latency,
                addrs,
                ..
            }) => {
                let relay_url = relay_url
                    .map(|x| x.relay_url.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                let latency = format_latency(latency);
                let addrs = addrs
                    .into_iter()
                    .map(|addr_info| {
                        format!("{} ({})", addr_info.addr, format_latency(addr_info.latency))
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                format!(
                    "relay url: {relay_url}, latency: {latency}, connection type: {conn_type}, addrs: [{addrs}]"
                )
            }
            None => "connection info unavailable".to_string(),
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
"#,
            );
            target.set_message(text);
        }
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
    fn clear(&self) {
        self.mp.clear().ok();
    }
}

/// Sends, receives and echoes data in a connection.
async fn active_side(
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
async fn passive_side(gui: Gui, connection: &Connection) -> anyhow::Result<()> {
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
    RelayMap::from(RelayNode {
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
    service_node: Option<NodeId>,
    ssh_key: Option<PathBuf>,
    metrics: IrohMetricsRegistry,
) -> anyhow::Result<(Endpoint, Option<iroh_n0des::Client>)> {
    tracing::info!(
        "public key: {}",
        hex::encode(secret_key.public().as_bytes())
    );
    tracing::info!("relay map {:#?}", relay_map);

    let mut transport_config = endpoint::TransportConfig::default();
    transport_config.keep_alive_interval(Some(Duration::from_secs(5)));
    transport_config.max_idle_timeout(Some(Duration::from_secs(10).try_into().unwrap()));

    let endpoint = Endpoint::builder()
        .secret_key(secret_key)
        .alpns(vec![DR_RELAY_ALPN.to_vec()])
        .transport_config(transport_config);

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
            .build(remote_node)
            .await?;
        Some(client)
    } else {
        None
    };

    tokio::time::timeout(
        Duration::from_secs(10),
        endpoint.direct_addresses().initialized(),
    )
    .await
    .context("wait for relay connection")??;

    Ok((endpoint, rpc_client))
}

/// Connects to a [`NodeId`].
async fn connect(
    node_id: NodeId,
    direct_addresses: Vec<SocketAddr>,
    relay_url: Option<RelayUrl>,
    endpoint: Endpoint,
) -> anyhow::Result<()> {
    tracing::info!("dialing {:?}", node_id);
    let node_addr = NodeAddr::from_parts(node_id, relay_url, direct_addresses);
    let conn = endpoint.connect(node_addr, &DR_RELAY_ALPN).await;
    match conn {
        Ok(connection) => {
            let maybe_conn_type = endpoint.conn_type(node_id);
            let gui = Gui::new(endpoint, node_id);
            if let Some(conn_type) = maybe_conn_type {
                log_connection_changes(gui.mp.clone(), node_id, conn_type);
            }

            let close_reason = connection
                .close_reason()
                .map(|e| format!(" (reason: {e})"))
                .unwrap_or_default();

            if let Err(cause) = passive_side(gui, &connection).await {
                eprintln!("error handling connection: {cause}{close_reason}");
            } else {
                eprintln!("Connection closed{close_reason}");
            }
        }
        Err(cause) => {
            eprintln!("unable to connect to {node_id}: {cause}");
        }
    }

    Ok(())
}

async fn close_endpoint_on_ctrl_c(endpoint: Endpoint) {
    tokio::signal::ctrl_c()
        .await
        .expect("failed listening to SIGINT");
    endpoint.close().await;
}

/// Formats a [`SocketAddr`] so that console doesn't escape it.
fn format_addr(addr: SocketAddr) -> String {
    if addr.is_ipv6() {
        format!("'{addr}'")
    } else {
        format!("{addr}")
    }
}

/// Accepts the connections.
pub async fn accept(
    secret_key: SecretKey,
    config: TestConfig,
    endpoint: Endpoint,
) -> anyhow::Result<()> {
    let endpoints = endpoint
        .direct_addresses()
        .initialized()
        .await
        .expect("endpoint alive");

    let remote_addrs = endpoints
        .iter()
        .map(|endpoint| format!("--remote-endpoint {}", format_addr(endpoint.addr)))
        .collect::<Vec<_>>()
        .join(" ");
    println!("Connect to this node using one of the following commands:\n");
    println!(
        "\tUsing the relay url and direct connections:\niroh-doctor connect {} {}\n",
        secret_key.public(),
        remote_addrs,
    );
    if let Some(relay_url) = endpoint.home_relay().get().expect("endpoint alive").pop() {
        println!(
            "\tUsing just the relay url:\niroh-doctor connect {} --relay-url {}\n",
            secret_key.public(),
            relay_url,
        );
    }
    if endpoint.discovery().is_some() {
        println!(
            "\tUsing just the node id:\niroh-doctor connect {}\n",
            secret_key.public(),
        );
    }
    let connections = Arc::new(AtomicU64::default());
    while let Some(incoming) = endpoint.accept().await {
        let connecting = match incoming.accept() {
            Ok(connecting) => connecting,
            Err(err) => {
                warn!("incoming connection failed: {err:#}");
                // we can carry on in these cases:
                // this can be caused by retransmitted datagrams
                continue;
            }
        };
        let connections = connections.clone();
        let endpoint = endpoint.clone();
        tokio::task::spawn(async move {
            let n = connections.fetch_add(1, portable_atomic::Ordering::SeqCst);
            match connecting.await {
                Ok(connection) => {
                    if n == 0 {
                        let Ok(remote_peer_id) = connection.remote_node_id() else {
                            return;
                        };
                        println!("Accepted connection from {remote_peer_id}");
                        let t0 = Instant::now();
                        let gui = Gui::new(endpoint.clone(), remote_peer_id);
                        if let Some(conn_type) = endpoint.conn_type(remote_peer_id) {
                            log_connection_changes(gui.mp.clone(), remote_peer_id, conn_type);
                        }
                        let res = active_side(&connection, &config, Some(&gui)).await;
                        gui.clear();
                        let dt = t0.elapsed().as_secs_f64();
                        if let Err(cause) = res {
                            let close_reason = connection
                                .close_reason()
                                .map(|e| format!(" (reason: {e})"))
                                .unwrap_or_default();
                            eprintln!("Test finished after {dt}s: {cause}{close_reason}",);
                        } else {
                            eprintln!("Test finished after {dt}s",);
                        }
                    } else {
                        // silent
                        active_side(&connection, &config, None).await.ok();
                    }
                }
                Err(cause) => {
                    eprintln!("error accepting connection {cause}");
                }
            };
            connections.sub(1, portable_atomic::Ordering::SeqCst);
        });
    }

    Ok(())
}

/// Logs the connection changes to the multiprogress.
fn log_connection_changes(
    pb: MultiProgress,
    node_id: NodeId,
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

/// Checks if there's a port mapping in the local port, and if it's ready.
async fn port_map(protocol: &str, local_port: NonZeroU16, timeout: Duration) -> anyhow::Result<()> {
    // Create the config that enables exclusively the required protocol
    let mut enable_upnp = false;
    let mut enable_pcp = false;
    let mut enable_nat_pmp = false;
    match protocol.to_ascii_lowercase().as_ref() {
        "upnp" => enable_upnp = true,
        "nat_pmp" => enable_nat_pmp = true,
        "pcp" => enable_pcp = true,
        other => anyhow::bail!("Unknown port mapping protocol {other}"),
    }
    let config = portmapper::Config {
        enable_upnp,
        enable_pcp,
        enable_nat_pmp,
    };
    let port_mapper = portmapper::Client::new(config);
    let mut watcher = port_mapper.watch_external_address();
    port_mapper.update_local_port(local_port);

    // Wait for the mapping to be ready, or timeout waiting for a change.
    match tokio::time::timeout(timeout, watcher.changed()).await {
        Ok(Ok(_)) => match *watcher.borrow() {
            Some(address) => {
                println!("Port mapping ready: {address}");
                // Ensure the port mapper remains alive until the end.
                drop(port_mapper);
                Ok(())
            }
            None => anyhow::bail!("No port mapping found"),
        },
        Ok(Err(_recv_err)) => anyhow::bail!("Service dropped. This is a bug"),
        Err(_) => anyhow::bail!("Timed out waiting for a port mapping"),
    }
}

/// Probes a port map.
async fn port_map_probe(config: portmapper::Config) -> anyhow::Result<()> {
    println!("probing port mapping protocols with {config:?}");
    let port_mapper = portmapper::Client::new(config);
    let probe_rx = port_mapper.probe();
    let probe = probe_rx.await?.map_err(|e| anyhow::anyhow!(e))?;
    println!("{probe}");
    Ok(())
}

/// Checks a certain amount (`count`) of the nodes given by the [`NodeConfig`].
async fn relay_urls(count: usize, config: &NodeConfig) -> anyhow::Result<()> {
    let key = SecretKey::generate(rand::rngs::OsRng);
    if config.relay_nodes.is_empty() {
        println!("No relay nodes specified in the config file.");
    }

    let dns_resolver = DnsResolver::new();
    let mut client_builders = HashMap::new();
    for node in &config.relay_nodes {
        let secret_key = key.clone();
        let client_builder = iroh_relay::client::ClientBuilder::new(
            node.url.clone(),
            secret_key,
            dns_resolver.clone(),
        );

        client_builders.insert(node.url.clone(), client_builder);
    }

    let mut success = Vec::new();
    let mut fail = Vec::new();

    for i in 0..count {
        println!("Round {}/{count}", i + 1);
        let relay_nodes = config.relay_nodes.clone();
        for node in relay_nodes.into_iter() {
            let mut node_details = NodeDetails {
                connect: None,
                latency: None,
                error: None,
                host: node.url.clone(),
            };

            let client_builder = client_builders.get(&node.url).cloned().unwrap();

            let start = std::time::Instant::now();
            match tokio::time::timeout(Duration::from_secs(2), client_builder.connect()).await {
                Err(e) => {
                    tracing::warn!("connect timeout");
                    node_details.error = Some(e.to_string());
                }
                Ok(Err(e)) => {
                    tracing::warn!("connect error");
                    node_details.error = Some(e.to_string());
                }
                Ok(Ok(client)) => {
                    node_details.connect = Some(start.elapsed());
                    match ping(client).await {
                        Ok(latency) => {
                            node_details.latency = Some(latency);
                        }
                        Err(e) => {
                            tracing::warn!("ping error: {:?}", e);
                            node_details.error = Some(e.to_string());
                        }
                    }
                }
            }

            if node_details.error.is_none() {
                success.push(node_details);
            } else {
                fail.push(node_details);
            }
        }
    }

    // success.sort_by_key(|d| d.latency);
    if !success.is_empty() {
        println!("Relay Node Latencies:");
        println!();
    }
    for node in success {
        println!("{node}");
        println!();
    }
    if !fail.is_empty() {
        println!("Connection Failures:");
        println!();
    }
    for node in fail {
        println!("{node}");
        println!();
    }

    Ok(())
}

async fn ping(client: iroh_relay::client::Client) -> anyhow::Result<Duration> {
    let (mut client_stream, mut client_sink) = client.split();
    let data: [u8; 8] = rand::random();
    let start = Instant::now();
    client_sink.send(SendMessage::Ping(data)).await?;
    match tokio::time::timeout(Duration::from_secs(2), async move {
        while let Some(res) = client_stream.next().await {
            let res = res?;
            if let iroh_relay::client::ReceivedMessage::Pong(d) = res {
                if d == data {
                    return Ok(start.elapsed());
                }
            }
        }
        anyhow::bail!("no pong received");
    })
    .await
    {
        Err(_) => {
            anyhow::bail!("ping timeout");
        }
        Ok(res) => res,
    }
}

/// Information about a node and its connection.
struct NodeDetails {
    connect: Option<Duration>,
    latency: Option<Duration>,
    host: RelayUrl,
    error: Option<String>,
}

impl std::fmt::Display for NodeDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.error {
            None => {
                write!(
                    f,
                    "Node {}\nConnect: {:?}\nLatency: {:?}",
                    self.host,
                    self.connect.unwrap_or_default(),
                    self.latency.unwrap_or_default(),
                )
            }
            Some(ref err) => {
                write!(f, "Node {}\nConnection Error: {:?}", self.host, err,)
            }
        }
    }
}

/// Creates a [`SecretKey`] from a [`SecretKeyOption`].
fn create_secret_key(secret_key: SecretKeyOption) -> anyhow::Result<SecretKey> {
    Ok(match secret_key {
        SecretKeyOption::Random => SecretKey::generate(rand::rngs::OsRng),
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
                SecretKey::generate(rand::rngs::OsRng)
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
    service_node: Option<NodeId>,
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
        } => report(config, quic_ipv4, quic_ipv6, https).await,
        Commands::Connect {
            dial,
            secret_key,
            local_relay_server,
            relay_url,
            remote_endpoint,
            disable_discovery,
        } => {
            let (relay_map, relay_url) = if local_relay_server {
                let dm = configure_local_relay_map();
                let url = dm.urls().next().unwrap().clone();
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
            )
            .await?;

            futures_lite::future::race(close_endpoint_on_ctrl_c(endpoint.clone()), async move {
                if let Err(e) = connect(dial, remote_endpoint, relay_url, endpoint).await {
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
            )
            .await?;

            futures_lite::future::race(close_endpoint_on_ctrl_c(endpoint.clone()), async move {
                if let Err(e) = accept(secret_key, config, endpoint).await {
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
        } => port_map(&protocol, local_port, Duration::from_secs(timeout_secs)).await,
        Commands::PortMapProbe {
            enable_upnp,
            enable_pcp,
            enable_nat_pmp,
        } => {
            let config = portmapper::Config {
                enable_upnp,
                enable_pcp,
                enable_nat_pmp,
            };

            port_map_probe(config).await
        }
        Commands::RelayUrls { count } => relay_urls(count, config).await,
        Commands::Plot {
            interval,
            metrics,
            timeframe,
            scrape_url,
            file,
        } => {
            let metrics: Vec<String> = metrics.split(',').map(|s| s.to_string()).collect();
            let interval = Duration::from_millis(interval);

            enable_raw_mode()?;
            let mut stdout = io::stdout();
            execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
            let backend = CrosstermBackend::new(stdout);
            let mut terminal = Terminal::new(backend)?;

            let app = PlotterApp::new(metrics, timeframe, scrape_url, file);
            let res = run_plotter(&mut terminal, app, interval).await;
            disable_raw_mode()?;
            execute!(
                terminal.backend_mut(),
                LeaveAlternateScreen,
                DisableMouseCapture
            )?;
            terminal.show_cursor()?;

            if let Err(err) = res {
                println!("{err:?}");
            }

            Ok(())
        }
    };
    if let Some(metrics_fut) = metrics_fut {
        metrics_fut.abort();
    }
    cmd_res
}

/// Runs the [`PlotterApp`].
async fn run_plotter<B: Backend>(
    terminal: &mut Terminal<B>,
    mut app: PlotterApp,
    tick_rate: Duration,
) -> anyhow::Result<()> {
    let mut last_tick = Instant::now();
    loop {
        terminal.draw(|f| plotter_draw(f, &mut app))?;

        if crossterm::event::poll(Duration::from_millis(10))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if let KeyCode::Char(c) = key.code {
                        app.on_key(c)
                    }
                }
            }
        }
        if last_tick.elapsed() >= tick_rate {
            app.on_tick().await;
            last_tick = Instant::now();
        }
        if app.should_quit {
            return Ok(());
        }
    }
}

/// Converts an area into `n` chunks.
fn area_into_chunks(area: Rect, n: usize, is_horizontal: bool) -> std::rc::Rc<[Rect]> {
    let mut constraints = vec![];
    for _ in 0..n {
        constraints.push(Constraint::Percentage(100 / n as u16));
    }
    let layout = match is_horizontal {
        true => Layout::horizontal(constraints),
        false => Layout::vertical(constraints),
    };
    layout.split(area)
}

/// Creates a collection of [`Rect`] by splitting an [`Rect`] area into `n` chunks.
fn generate_layout_chunks(area: Rect, n: usize) -> Vec<Rect> {
    if n < 4 {
        let chunks = area_into_chunks(area, n, false);
        return chunks.iter().copied().collect();
    }
    let main_chunks = area_into_chunks(area, 2, true);
    let left_chunks = area_into_chunks(main_chunks[0], n / 2 + n % 2, false);
    let right_chunks = area_into_chunks(main_chunks[1], n / 2, false);
    let mut chunks = vec![];
    chunks.extend(left_chunks.iter());
    chunks.extend(right_chunks.iter());
    chunks
}

/// Draws the [`Frame`] given a [`PlotterApp`].
fn plotter_draw(f: &mut Frame, app: &mut PlotterApp) {
    let area = f.area();

    let metrics_cnt = app.metrics.len();
    let areas = generate_layout_chunks(area, metrics_cnt);

    for (i, metric) in app.metrics.iter().enumerate() {
        plot_chart(f, areas[i], app, metric);
    }
}

/// Draws the chart defined in the [`Frame`].
fn plot_chart(frame: &mut Frame, area: Rect, app: &PlotterApp, metric: &str) {
    let elapsed = app.internal_ts.as_secs_f64();
    let data = app.data.get(metric).unwrap().clone();
    let data_y_range = app.data_y_range.get(metric).unwrap();

    let moved = (elapsed / 15.0).floor() * 15.0 - app.timeframe as f64;
    let moved = moved.max(0.0);
    let x_start = 0.0 + moved;
    let x_end = moved + app.timeframe as f64 + 25.0;

    let y_start = data_y_range.0;
    let y_end = data_y_range.1;

    let last_val = data.last();
    let name = match last_val {
        Some(val) => {
            let val_y = val.1;
            format!("{metric}: {val_y:.0}")
        }
        None => metric.to_string(),
    };
    let datasets = vec![Dataset::default()
        .name(name)
        .marker(symbols::Marker::Dot)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(Color::Cyan))
        .data(&data)];

    // TODO(arqu): labels are incorrectly spaced for > 3 labels https://github.com/ratatui-org/ratatui/issues/334
    let x_labels = vec![
        Span::styled(
            format!("{x_start:.1}s"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("{:.1}s", x_start + (x_end - x_start) / 2.0)),
        Span::styled(
            format!("{x_end:.1}s"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ];

    let mut y_labels = vec![Span::styled(
        format!("{y_start:.0}"),
        Style::default().add_modifier(Modifier::BOLD),
    )];

    for i in 1..=10 {
        y_labels.push(Span::raw(format!(
            "{:.0}",
            y_start + (y_end - y_start) / 10.0 * i as f64
        )));
    }

    y_labels.push(Span::styled(
        format!("{y_end:.0}"),
        Style::default().add_modifier(Modifier::BOLD),
    ));

    let chart = Chart::new(datasets)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Chart: {metric}")),
        )
        .x_axis(
            Axis::default()
                .title("X Axis")
                .style(Style::default().fg(Color::Gray))
                .labels(x_labels)
                .bounds([x_start, x_end]),
        )
        .y_axis(
            Axis::default()
                .title("Y Axis")
                .style(Style::default().fg(Color::Gray))
                .labels(y_labels)
                .bounds([y_start, y_end]),
        );

    frame.render_widget(chart, area);
}

/// All the information about the plotter app.
struct PlotterApp {
    should_quit: bool,
    metrics: Vec<String>,
    start_ts: Instant,
    data: HashMap<String, Vec<(f64, f64)>>,
    data_y_range: HashMap<String, (f64, f64)>,
    timeframe: usize,
    rng: rand::rngs::ThreadRng,
    freeze: bool,
    internal_ts: Duration,
    scrape_url: String,
    file_data: Vec<String>,
    file_header: Vec<String>,
}

impl PlotterApp {
    /// Creates a new [`PlotterApp`].
    fn new(
        metrics: Vec<String>,
        timeframe: usize,
        scrape_url: String,
        file: Option<PathBuf>,
    ) -> Self {
        let data = metrics.iter().map(|m| (m.clone(), vec![])).collect();
        let data_y_range = metrics.iter().map(|m| (m.clone(), (0.0, 0.0))).collect();
        let mut file_data: Vec<String> = file
            .map(|f| std::fs::read_to_string(f).unwrap())
            .unwrap_or_default()
            .split('\n')
            .map(|s| s.to_string())
            .collect();
        let mut file_header = vec![];
        let mut timeframe = timeframe;
        if !file_data.is_empty() {
            file_header = file_data[0].split(',').map(|s| s.to_string()).collect();
            file_data.remove(0);

            while file_data.last().unwrap().is_empty() {
                file_data.pop();
            }

            let first_line: Vec<String> = file_data[0].split(',').map(|s| s.to_string()).collect();
            let last_line: Vec<String> = file_data
                .last()
                .unwrap()
                .split(',')
                .map(|s| s.to_string())
                .collect();

            let start_time: usize = first_line.first().unwrap().parse().unwrap();
            let end_time: usize = last_line.first().unwrap().parse().unwrap();

            timeframe = (end_time - start_time) / 1000;
        }
        timeframe = timeframe.clamp(30, 90);

        file_data.reverse();
        Self {
            should_quit: false,
            metrics,
            start_ts: Instant::now(),
            data,
            data_y_range,
            timeframe,
            rng: rand::thread_rng(),
            freeze: false,
            internal_ts: Duration::default(),
            scrape_url,
            file_data,
            file_header,
        }
    }

    /// Chooses what to do when a key is pressed.
    fn on_key(&mut self, c: char) {
        match c {
            'q' => {
                self.should_quit = true;
            }
            'f' => {
                self.freeze = !self.freeze;
            }
            _ => {}
        }
    }

    /// Chooses what to do on a tick.
    async fn on_tick(&mut self) {
        if self.freeze {
            return;
        }

        let metrics_response = match self.file_data.is_empty() {
            true => {
                let req = reqwest::Client::new().get(&self.scrape_url).send().await;
                if req.is_err() {
                    return;
                }
                let data = req.unwrap().text().await.unwrap();
                iroh_metrics::parse_prometheus_metrics(&data)
            }
            false => {
                if self.file_data.len() == 1 {
                    self.freeze = true;
                    return;
                }
                let data = self.file_data.pop().unwrap();
                let r = parse_csv_metrics(&self.file_header, &data);
                if let Ok(mr) = r {
                    mr
                } else {
                    warn!("Failed to parse csv metrics: {:?}", r.err());
                    HashMap::new()
                }
            }
        };
        self.internal_ts = self.start_ts.elapsed();
        for metric in &self.metrics {
            let val = if metric.eq("random") {
                self.rng.gen_range(0..101) as f64
            } else if let Some(v) = metrics_response.get(metric) {
                *v
            } else {
                0.0
            };
            let e = self.data.entry(metric.clone()).or_default();
            let mut ts = self.internal_ts.as_secs_f64();
            if metrics_response.contains_key("time") {
                ts = *metrics_response.get("time").unwrap() / 1000.0;
            }
            self.internal_ts = Duration::from_secs_f64(ts);
            e.push((ts, val));
            let yr = self.data_y_range.get_mut(metric).unwrap();
            if val * 1.1 < yr.0 {
                yr.0 = val * 1.2;
            }
            if val * 1.1 > yr.1 {
                yr.1 = val * 1.2;
            }
        }
    }
}

/// Parses CSV metrics into a [`HashMap`] of `String` -> `f64`.
fn parse_csv_metrics(header: &[String], data: &str) -> anyhow::Result<HashMap<String, f64>> {
    let mut metrics = HashMap::new();
    let data = data.split(',').collect::<Vec<&str>>();
    for (i, h) in header.iter().enumerate() {
        let val = match h.as_str() {
            "time" => {
                let ts = data[i].parse::<u64>()?;
                ts as f64
            }
            _ => data[i].parse::<f64>()?,
        };
        metrics.insert(h.clone(), val);
    }
    Ok(metrics)
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
