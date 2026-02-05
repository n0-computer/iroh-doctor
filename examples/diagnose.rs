//! Network diagnostics example for iroh-powered applications.
//!
//! Demonstrates how to run a full network diagnostics report from an existing
//! iroh Endpoint — covering NAT type, UDP connectivity, relay latency, and
//! port mapping protocol availability.
//!
//! Run with: cargo run --example diagnose
//!
//! This is designed to be copy-pasteable into your own project.

use std::{
    fmt,
    net::{SocketAddr, SocketAddrV4, SocketAddrV6},
    time::{Duration, Instant},
};

use anyhow::Result;
use futures_lite::StreamExt;
use futures_util::SinkExt;
#[allow(unused_imports)]
use iroh::{dns::DnsResolver, Endpoint, RelayConfig, RelayMap, RelayMode, RelayUrl, SecretKey, Watcher};
#[allow(unused_imports)]
use iroh_relay::{protos::relay::{ClientToRelayMsg, RelayToClientMsg}, RelayQuicConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum NatType {
    Easy,
    Medium,
    Hard,
    Unknown,
}

impl NatType {
    fn description(&self) -> &'static str {
        match self {
            Self::Easy => "NAT allows easy P2P connectivity",
            Self::Medium => "NAT may require additional techniques for P2P",
            Self::Hard => "NAT is difficult for P2P connectivity",
            Self::Unknown => "NAT type could not be determined",
        }
    }

    fn difficulty(&self) -> u8 {
        match self {
            Self::Easy => 1,
            Self::Medium => 3,
            Self::Hard => 5,
            Self::Unknown => 4,
        }
    }
}

impl fmt::Display for NatType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Easy => write!(f, "Easy"),
            Self::Medium => write!(f, "Medium"),
            Self::Hard => write!(f, "Hard"),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

#[derive(Debug)]
struct RelayLatency {
    url: RelayUrl,
    connect_time: Option<Duration>,
    ping_latency: Option<Duration>,
    error: Option<String>,
}

#[derive(Debug)]
struct DiagnosticsReport {
    node_id: iroh::EndpointId,
    udp_v4: bool,
    udp_v6: bool,
    global_v4: Option<SocketAddrV4>,
    global_v6: Option<SocketAddrV6>,
    mapping_varies_by_dest_v4: Option<bool>,
    mapping_varies_by_dest_v6: Option<bool>,
    mapping_varies_by_dest_port_v4: Option<bool>,
    mapping_varies_by_dest_port_v6: Option<bool>,
    nat_type: NatType,
    relay_urls: Vec<RelayUrl>,
    direct_addrs: Vec<SocketAddr>,
    relay_latencies: Vec<RelayLatency>,
    portmap_probe: Option<String>,
}

impl fmt::Display for DiagnosticsReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "=== iroh Network Diagnostics ===")?;
        writeln!(f, "Node ID: {}", self.node_id)?;

        writeln!(
            f,
            "UDP IPv4: {} | Global: {}",
            yn(self.udp_v4),
            opt_addr_v4(self.global_v4)
        )?;
        writeln!(
            f,
            "UDP IPv6: {} | Global: {}",
            yn(self.udp_v6),
            opt_addr_v6(self.global_v6)
        )?;

        writeln!(
            f,
            "NAT Type: {} (difficulty {}/5) — {}",
            self.nat_type,
            self.nat_type.difficulty(),
            self.nat_type.description()
        )?;
        writeln!(
            f,
            "NAT mapping varies by dest addr (v4): {}  (v6): {}",
            opt_bool(self.mapping_varies_by_dest_v4),
            opt_bool(self.mapping_varies_by_dest_v6),
        )?;
        writeln!(
            f,
            "NAT mapping varies by dest port (v4): {}  (v6): {}",
            opt_bool(self.mapping_varies_by_dest_port_v4),
            opt_bool(self.mapping_varies_by_dest_port_v6),
        )?;

        if self.relay_urls.is_empty() {
            writeln!(f, "Relay: none")?;
        } else {
            let urls: Vec<_> = self.relay_urls.iter().map(|u| u.to_string()).collect();
            writeln!(f, "Relay URLs: {}", urls.join(", "))?;
        }

        if !self.relay_latencies.is_empty() {
            writeln!(f, "Relay Latency:")?;
            for rl in &self.relay_latencies {
                if let Some(ref err) = rl.error {
                    writeln!(f, "  {} — error: {}", rl.url, err)?;
                } else {
                    writeln!(
                        f,
                        "  {} — connect: {}, ping: {}",
                        rl.url,
                        fmt_dur(rl.connect_time),
                        fmt_dur(rl.ping_latency),
                    )?;
                }
            }
        }

        if self.direct_addrs.is_empty() {
            writeln!(f, "Direct addrs: none")?;
        } else {
            let addrs: Vec<_> = self.direct_addrs.iter().map(|a| a.to_string()).collect();
            writeln!(f, "Direct addrs: {}", addrs.join(", "))?;
        }

        if let Some(ref probe) = self.portmap_probe {
            writeln!(f, "Port mapping: {probe}")?;
        } else {
            writeln!(f, "Port mapping: probe failed or timed out")?;
        }

        Ok(())
    }
}

/// Run full network diagnostics on an existing endpoint. 10s timeout.
async fn diagnose(endpoint: &Endpoint) -> Result<DiagnosticsReport> {
    diagnose_with_timeout(endpoint, Duration::from_secs(10)).await
}

/// Run full network diagnostics with a custom timeout for net report init.
async fn diagnose_with_timeout(
    endpoint: &Endpoint,
    timeout: Duration,
) -> Result<DiagnosticsReport> {
    let node_id = endpoint.id();

    // 1. Wait for relay connection
    if let Err(_) = tokio::time::timeout(timeout, endpoint.online()).await {
        eprintln!("waiting for relay connection timed out after {timeout:?}");
    }

    // 2. Net report
    let mut watcher = endpoint.net_report();
    let net_report = match tokio::time::timeout(timeout, watcher.initialized()).await {
        Ok(report) => Some(report),
        Err(_) => {
            eprintln!("net report timed out after {timeout:?}, using partial data");
            watcher.get()
        }
    };

    // 3. Extract fields + classify NAT
    let (udp_v4, udp_v6, global_v4, global_v6, varies_dest_v4, varies_dest_v6) =
        match net_report {
            Some(ref r) => (
                r.udp_v4,
                r.udp_v6,
                r.global_v4,
                r.global_v6,
                r.mapping_varies_by_dest_ipv4,
                r.mapping_varies_by_dest_ipv6,
            ),
            None => (false, false, None, None, None, None),
        };

    let nat_type = classify_nat(&net_report);

    // 4. Endpoint address info
    let addr = endpoint.addr();
    let relay_urls: Vec<RelayUrl> = addr.relay_urls().cloned().collect();
    let direct_addrs: Vec<SocketAddr> = addr.ip_addrs().copied().collect();

    // 5. Relay latency + port mapping probe in parallel
    let relay_urls_clone = relay_urls.clone();
    let relay_fut = async {
        let mut results = Vec::new();
        for url in &relay_urls_clone {
            results.push(probe_relay_latency(url).await);
        }
        results
    };

    let portmap_fut = async {
        match tokio::time::timeout(Duration::from_secs(5), probe_port_mapping()).await {
            Ok(Ok(s)) => Some(s),
            Ok(Err(e)) => {
                eprintln!("portmap probe failed: {e}");
                None
            }
            Err(_) => {
                eprintln!("portmap probe timed out");
                None
            }
        }
    };

    let (relay_latencies, portmap_probe) = tokio::join!(relay_fut, portmap_fut);

    Ok(DiagnosticsReport {
        node_id,
        udp_v4,
        udp_v6,
        global_v4,
        global_v6,
        mapping_varies_by_dest_v4: varies_dest_v4,
        mapping_varies_by_dest_v6: varies_dest_v6,
        // Not yet implemented in iroh's NetReport
        mapping_varies_by_dest_port_v4: None,
        mapping_varies_by_dest_port_v6: None,
        nat_type,
        relay_urls,
        direct_addrs,
        relay_latencies,
        portmap_probe,
    })
}

/// Classifies NAT type based on address mapping behavior observed across
/// multiple relay servers. iroh's NetReport probes several relays and
/// compares the external address seen by each — if it differs, the NAT
/// is endpoint-dependent (Hard).
///
/// Note: port-dependent mapping detection is not yet implemented in iroh,
/// so we conservatively classify stable-address NATs as Medium rather than
/// Easy (we can't confirm the port is also stable).
fn classify_nat(report: &Option<iroh::NetReport>) -> NatType {
    let Some(r) = report else {
        return NatType::Unknown;
    };

    if r.global_v4.is_none() && r.global_v6.is_none() {
        return NatType::Unknown;
    }
    if !r.udp_v4 && !r.udp_v6 {
        return NatType::Unknown;
    }

    let mapping_varies_by_dest = r
        .mapping_varies_by_dest_ipv4
        .or(r.mapping_varies_by_dest_ipv6);

    match mapping_varies_by_dest {
        Some(true) => NatType::Hard,
        Some(false) => NatType::Medium,
        None => NatType::Unknown,
    }
}

async fn probe_relay_latency(url: &RelayUrl) -> RelayLatency {
    let key = SecretKey::generate(&mut rand::rng());
    let dns = DnsResolver::new();
    let builder = iroh_relay::client::ClientBuilder::new(url.clone(), key, dns);

    let start = Instant::now();
    let client = match tokio::time::timeout(Duration::from_secs(3), builder.connect()).await {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            return RelayLatency {
                url: url.clone(),
                connect_time: None,
                ping_latency: None,
                error: Some(format!("connect error: {e}")),
            };
        }
        Err(_) => {
            return RelayLatency {
                url: url.clone(),
                connect_time: None,
                ping_latency: None,
                error: Some("connect timeout".into()),
            };
        }
    };
    let connect_time = start.elapsed();

    let (mut stream, mut sink) = client.split();
    let data: [u8; 8] = rand::random();
    let ping_start = Instant::now();

    if let Err(e) = sink.send(ClientToRelayMsg::Ping(data)).await {
        return RelayLatency {
            url: url.clone(),
            connect_time: Some(connect_time),
            ping_latency: None,
            error: Some(format!("ping send error: {e}")),
        };
    }

    let ping_result = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(res) = stream.next().await {
            match res {
                Ok(RelayToClientMsg::Pong(d)) if d == data => {
                    return Ok(ping_start.elapsed());
                }
                Ok(_) => continue,
                Err(e) => return Err(anyhow::anyhow!("stream error: {e}")),
            }
        }
        Err(anyhow::anyhow!("stream ended without pong"))
    })
    .await;

    match ping_result {
        Ok(Ok(latency)) => RelayLatency {
            url: url.clone(),
            connect_time: Some(connect_time),
            ping_latency: Some(latency),
            error: None,
        },
        Ok(Err(e)) => RelayLatency {
            url: url.clone(),
            connect_time: Some(connect_time),
            ping_latency: None,
            error: Some(e.to_string()),
        },
        Err(_) => RelayLatency {
            url: url.clone(),
            connect_time: Some(connect_time),
            ping_latency: None,
            error: Some("ping timeout".into()),
        },
    }
}

async fn probe_port_mapping() -> Result<String> {
    let config = portmapper::Config {
        enable_upnp: true,
        enable_pcp: true,
        enable_nat_pmp: true,
        protocol: portmapper::Protocol::Udp,
    };
    let client = portmapper::Client::new(config);
    let probe_rx = client.probe();
    let probe = probe_rx.await?.map_err(|e| anyhow::anyhow!(e))?;
    Ok(format!("{probe}"))
}

fn yn(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

fn opt_bool(b: Option<bool>) -> &'static str {
    match b {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    }
}

fn opt_addr_v4(a: Option<SocketAddrV4>) -> String {
    a.map(|a| a.to_string()).unwrap_or_else(|| "none".into())
}

fn opt_addr_v6(a: Option<SocketAddrV6>) -> String {
    a.map(|a| a.to_string()).unwrap_or_else(|| "none".into())
}

fn fmt_dur(d: Option<Duration>) -> String {
    match d {
        Some(d) => format!("{:.1?}", d),
        None => "n/a".into(),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // In your app you already have an Endpoint. Here we create one to demonstrate.
    //
    // To use custom relays, build a RelayMap and pass it via RelayMode::Custom:
    //
    //   let relay_map = RelayMap::from_iter([
    //       RelayConfig {
    //           url: "https://my-relay.example.com".parse().unwrap(),
    //           quic: Some(RelayQuicConfig::default()),
    //       },
    //       RelayConfig {
    //           url: "https://my-relay-2.example.com".parse().unwrap(),
    //           quic: Some(RelayQuicConfig { port: 7843 }),
    //       },
    //   ]);
    //   let endpoint = Endpoint::builder()
    //       .relay_mode(RelayMode::Custom(relay_map))
    //       .bind()
    //       .await?;
    //
    let endpoint = Endpoint::builder()
        .relay_mode(RelayMode::Default)
        .bind()
        .await?;

    println!("Running network diagnostics...\n");

    let report = diagnose(&endpoint).await?;
    println!("{report}");

    // You can also inspect fields programmatically
    if report.nat_type == NatType::Hard {
        eprintln!("WARNING: Hard NAT — P2P connections will likely need relay fallback");
    }
    if !report.udp_v4 && !report.udp_v6 {
        eprintln!("WARNING: No UDP connectivity detected");
    }

    endpoint.close().await;
    Ok(())
}
