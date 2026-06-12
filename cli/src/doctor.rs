//! Tool to get information about the current network environment of a node,
//! and to test connectivity to specific other nodes.

use std::{
    net::SocketAddr,
    num::NonZeroU16,
    time::{Duration, Instant},
};

use anyhow::Context;
use clap::Subcommand;
use indicatif::{HumanBytes, MultiProgress, ProgressBar};
use iroh::{
    endpoint::{self, presets, Connection},
    metrics::SocketMetrics,
    Endpoint, EndpointId, RelayConfig, RelayMap, RelayMode, RelayUrl, SecretKey,
};
use iroh_metrics::static_core::Core;
use iroh_relay::RelayQuicConfig;
use n0_future::StreamExt;
use tokio_util::task::AbortOnDropHandle;

use crate::{
    commands,
    config::{iroh_data_root, NodeConfig},
    metrics::{IrohMetricsRegistry, MetricsRegistry},
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
            Self::Random
        } else if s_lower == "local" {
            Self::Local
        } else {
            Self::Hex(s.to_string())
        })
    }
}

/// Subcommands for the iroh doctor.
#[derive(Subcommand, Debug, Clone)]
pub enum Commands {
    /// Report on the current network environment.
    ///
    /// Paints the whole picture in one command: iroh's `NetReport` with a NAT
    /// classification, the per-relay latencies iroh recorded while building
    /// the report, and the iroh-services checks (whose net_diagnostics covers
    /// the UPnP/PCP/NAT-PMP gateway protocols). Prints a set of tables by
    /// default, or `--json` for tooling.
    Diagnostics {
        /// Emit the report as JSON to stdout instead of tables.
        #[clap(long, default_value_t = false)]
        json: bool,
    },
    /// Wait for incoming connections and monitor each one live (latency,
    /// paths, throughput), the accepting side of `iroh-doctor connect`.
    Accept {
        /// Our own secret key, in hex. If not specified, the locally configured key will be used.
        #[clap(long, default_value_t = SecretKeyOption::Local)]
        secret_key: SecretKeyOption,

        /// Use a local relay.
        #[clap(long)]
        local_relay_server: bool,

        /// Do not allow the node to dial and be dialed by id only.
        ///
        /// This disables DNS address lookup, which would allow the node to dial other nodes by id only.
        /// And it disables Pkarr Publishing, which would allow the node to announce its address for address lookup.
        ///
        /// Default is `false`
        #[clap(long, default_value_t = false)]
        disable_address_lookup: bool,

        /// Bind to this specific socket address.
        ///
        /// Default is `None`, which means the endpoint will bind to a random port.
        #[clap(long)]
        socket_addr: Option<SocketAddr>,
    },
    /// Connect to a peer and monitor the connection live: state, paths,
    /// latency over time, and throughput.
    Connect {
        /// Hexadecimal node id of the node to connect to.
        dial: EndpointId,

        /// One or more remote endpoints to use when dialing.
        #[clap(long)]
        remote_endpoint: Vec<SocketAddr>,

        /// Our own secret key. Defaults to the local persistent key
        /// (created on first use under the iroh data dir), so the
        /// command announces a stable endpoint id across runs.
        #[clap(long, default_value_t = SecretKeyOption::Local)]
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
        /// This disables DNS address lookup, which would allow the node to dial other nodes by id only.
        /// It also disables Pkarr Publishing, which would allow the node to announce its address for address lookup .
        ///
        /// Default is `false`
        #[clap(long, default_value_t = false)]
        disable_address_lookup: bool,

        /// Bind to this specific socket address.
        ///
        /// Default is `None`, which means the endpoint will bind to a random port.
        #[clap(long)]
        socket_addr: Option<SocketAddr>,
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
}

/// Holds the live header shown above the monitor dashboard: a network byte
/// counter and the peer's relay url and direct addresses, refreshed by a
/// background task for as long as the `Gui` is alive.
pub struct Gui {
    pub mp: MultiProgress,
    #[allow(dead_code)]
    pub counters: ProgressBar,
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
        let style = indicatif::ProgressStyle::default_bar()
            .template("{msg}")
            .unwrap();
        remote_info.set_style(style.clone());
        counters.set_style(style);
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
            counters,
            counter_task: Some(AbortOnDropHandle::new(counter_task)),
        }
    }

    /// Updates the information of the target progress bar.
    async fn update_remote_info(target: &ProgressBar, endpoint: &Endpoint, _node_id: &EndpointId) {
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

        let msg = format!("relay url: {relay_url}, addrs: [{addrs}]");
        target.set_message(msg);
    }

    /// Updates the counters for the target progress bar.
    fn update_counters(target: &ProgressBar) {
        if let Some(core) = Core::get() {
            let metrics = core.get_collector::<SocketMetrics>().unwrap();
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

    /// Clears the [`MultiProgress`] field.
    pub fn clear(&self) {
        self.mp.clear().ok();
    }
}

/// Configures a relay map with some default values.
fn configure_local_relay_map() -> RelayMap {
    let url = "http://localhost:3340".parse().unwrap();
    RelayMap::from(RelayConfig::new(url, Some(RelayQuicConfig::default())))
}

/// Creates an iroh [`Endpoint`] from a [`SecretKey`] and a [`RelayMap`].
async fn make_endpoint(
    secret_key: SecretKey,
    relay_map: Option<RelayMap>,
    disable_address_lookup: bool,
    metrics: IrohMetricsRegistry,
    socket_addr: Option<SocketAddr>,
) -> anyhow::Result<Endpoint> {
    tracing::info!(
        "public key: {}",
        hex::encode(secret_key.public().as_bytes())
    );
    tracing::info!("relay map {:#?}", relay_map);

    let transport_config = endpoint::QuicTransportConfig::builder()
        .keep_alive_interval(Duration::from_secs(5))
        .max_idle_timeout(Some(Duration::from_secs(10).try_into().unwrap()))
        .build();

    let mut endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret_key)
        .alpns(vec![iroh_doctor_core::probe::ALPN.to_vec()])
        .transport_config(transport_config);

    if disable_address_lookup {
        endpoint = endpoint.clear_address_lookup();
    }

    if let Some(address) = socket_addr {
        endpoint = endpoint.bind_addr(address)?;
    }

    let endpoint = match relay_map {
        Some(relay_map) => endpoint.relay_mode(RelayMode::Custom(relay_map)),
        None => endpoint,
    };
    let endpoint = endpoint.bind().await?;

    {
        let mut registry = metrics.write().expect("poisoned");
        registry.register_all(endpoint.metrics());
    }

    tokio::time::timeout(Duration::from_secs(10), endpoint.online())
        .await
        .context("wait for relay connection")?;

    Ok(endpoint)
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
pub fn log_connection_changes(pb: MultiProgress, node_id: EndpointId, connection: Connection) {
    tokio::spawn(async move {
        let start = Instant::now();
        let mut paths = connection.paths_stream();
        while let Some(path_list) = paths.next().await {
            let selected = path_list.iter().find(|p| p.is_selected());
            let path_desc = match selected {
                Some(p) => format!("{:?}", p.remote_addr()),
                None => "no path".to_string(),
            };
            pb.println(format!(
                "Connection with {node_id:#} changed: {path_desc} (after {:?})",
                start.elapsed()
            ))
            .ok();
        }
    });
}

/// Creates a [`SecretKey`] from a [`SecretKeyOption`].
fn create_secret_key(secret_key: SecretKeyOption) -> anyhow::Result<SecretKey> {
    Ok(match secret_key {
        SecretKeyOption::Random => SecretKey::generate(),
        SecretKeyOption::Hex(hex) => {
            let bytes = hex::decode(hex)?;
            SecretKey::try_from(&bytes[..])?
        }
        SecretKeyOption::Local => {
            let dir = iroh_data_root()?;
            // Backward compatibility: an existing OpenSSH `keypair` still
            // wins, so users who already set one up keep that identity.
            let openssh_path = dir.join("keypair");
            if openssh_path.exists() {
                let bytes = std::fs::read(&openssh_path)?;
                try_secret_key_from_openssh(bytes)?
            } else {
                // Otherwise persist a raw 32-byte key under the same
                // directory and reuse it across runs, so `iroh-doctor`
                // announces a stable endpoint id by default.
                iroh_doctor_core::identity::load_or_create_secret_key(&dir.join("secret_key.bin"))?
            }
        }
    })
}

/// Runs the doctor commands.
pub async fn run(command: Commands, config: &NodeConfig) -> anyhow::Result<()> {
    let data_dir = iroh_data_root()?;
    let _guard = crate::logging::init_terminal_and_file_logging(&config.file_logs, &data_dir)?;

    let metrics = MetricsRegistry::default();
    // doesn't start the server if the address is None
    let metrics_server = match config.metrics_addr {
        Some(metrics_addr) => {
            match iroh_metrics::service::MetricsServer::spawn(metrics_addr, metrics.clone()).await {
                Ok(server) => Some(server),
                Err(e) => {
                    eprintln!("Failed to start metrics server: {e}");
                    None
                }
            }
        }
        None => {
            tracing::info!("Metrics server not started, no address provided");
            None
        }
    };
    let cmd_res = match command {
        Commands::Diagnostics { json } => commands::diagnostics::diagnostics(config, json).await,
        Commands::Connect {
            dial,
            secret_key,
            local_relay_server,
            relay_url,
            remote_endpoint,
            disable_address_lookup,
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

            let endpoint = make_endpoint(
                secret_key.clone(),
                relay_map.clone(),
                disable_address_lookup,
                metrics.iroh.clone(),
                socket_addr,
            )
            .await?;

            n0_future::future::race(close_endpoint_on_ctrl_c(endpoint.clone()), async move {
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
            disable_address_lookup,
            socket_addr,
        } => {
            let relay_map = if local_relay_server {
                Some(configure_local_relay_map())
            } else {
                config.relay_map()?
            };
            let secret_key = create_secret_key(secret_key)?;

            let endpoint = make_endpoint(
                secret_key.clone(),
                relay_map.clone(),
                disable_address_lookup,
                metrics.iroh.clone(),
                socket_addr,
            )
            .await?;

            n0_future::future::race(close_endpoint_on_ctrl_c(endpoint.clone()), async move {
                if let Err(e) = commands::accept::accept(secret_key, endpoint).await {
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
        Commands::RelayUrls { count } => commands::relay_urls::relay_urls(count, config).await,
    };
    if let Some(server) = metrics_server {
        server.shutdown().await;
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
