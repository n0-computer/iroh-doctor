//! Extended network report with port variation detection

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use anyhow::{Context, Result};
use iroh::{Endpoint, NodeId};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::{
    doctor::{create_secret_key, SecretKeyOption},
    swarm::config::PortVariationConfig,
};

/// ALPN protocol identifier for port variation probes
const PORT_PROBE_ALPN: &[u8] = b"n0/port-probe/1";

/// Extended network report that includes standard iroh report
/// plus our custom port variation detection results
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtendedNetworkReport {
    /// Base network report from iroh
    pub base_report: Option<iroh::net_report::Report>,

    // Our custom port variation fields
    /// Whether the NAT mapping varies by destination PORT for IPv4
    pub mapping_varies_by_dest_port_ipv4: Option<bool>,

    /// Whether the NAT mapping varies by destination PORT for IPv6
    pub mapping_varies_by_dest_port_ipv6: Option<bool>,
}

impl ExtendedNetworkReport {
    /// Create an extended report from a standard iroh Report
    pub fn from_base_report(base: Option<iroh::net_report::Report>) -> Self {
        Self {
            base_report: base,
            // Port variation fields default to None
            mapping_varies_by_dest_port_ipv4: None,
            mapping_varies_by_dest_port_ipv6: None,
        }
    }

    /// Update the report with port variation detection results
    pub fn with_port_variation(
        mut self,
        ipv4_varies: Option<bool>,
        ipv6_varies: Option<bool>,
    ) -> Self {
        self.mapping_varies_by_dest_port_ipv4 = ipv4_varies;
        self.mapping_varies_by_dest_port_ipv6 = ipv6_varies;
        self
    }
}

/// Result from port variation probing
#[derive(Debug, Clone)]
pub struct PortVariationResult {
    pub ipv4_varies: Option<bool>,
    pub ipv6_varies: Option<bool>,
    pub ipv4_mappings: HashMap<u16, u16>,
    pub ipv6_mappings: HashMap<u16, u16>,
}

/// Probe for port variation detection by connecting to a probe service
///
/// The probe service (running on the coordinator) listens on multiple ports.
/// We connect to it via n0 discovery and send probe requests specifying which port to test.
/// The service responds with the observed source port, allowing us to detect NAT behavior.
pub async fn probe_port_variation(
    config: &PortVariationConfig,
    probe_service_node_id: NodeId,
) -> Result<PortVariationResult> {
    if !config.enabled {
        return Ok(PortVariationResult {
            ipv4_varies: None,
            ipv6_varies: None,
            ipv4_mappings: HashMap::new(),
            ipv6_mappings: HashMap::new(),
        });
    }

    info!("Starting port variation detection probe");

    let mut ipv4_mappings = HashMap::new();
    let mut ipv6_mappings = HashMap::new();

    // Create probe tasks for all configured ports
    let mut probe_tasks = tokio::task::JoinSet::new();

    // Add primary port probe
    let timeout = config.probe_timeout;
    let primary_port = config.primary_port;
    let node_id = probe_service_node_id;
    probe_tasks.spawn(async move {
        let result = probe_service_port(primary_port, timeout, node_id).await;
        (primary_port, result)
    });

    // Add alternate port probes
    for alt_port in config.alternate_ports.clone() {
        let timeout = config.probe_timeout;
        let node_id = probe_service_node_id;
        probe_tasks.spawn(async move {
            let result = probe_service_port(alt_port, timeout, node_id).await;
            (alt_port, result)
        });
    }

    let task_count = probe_tasks.len();
    info!("Running {} port variation probes in parallel", task_count);

    // Execute all probes concurrently and collect results
    while let Some(result) = probe_tasks.join_next().await {
        let (port, probe_result) = result.context("Probe task failed")?;
        match probe_result {
            Ok((local_addr, observed_port)) => {
                debug!(
                    "Port {} probe: local {:?}, observed port {}",
                    port, local_addr, observed_port
                );
                match local_addr.ip() {
                    IpAddr::V4(_) => {
                        ipv4_mappings.insert(port, observed_port);
                    }
                    IpAddr::V6(_) => {
                        ipv6_mappings.insert(port, observed_port);
                    }
                }
            }
            Err(e) => {
                warn!("Failed to probe port {}: {}", port, e);
            }
        }
    }

    // Determine if mapping varies
    let ipv4_varies = if ipv4_mappings.len() > 1 {
        let ports: Vec<_> = ipv4_mappings.values().collect();
        let first = ports[0];
        let varies = ports.iter().any(|&p| p != first);
        info!(
            "IPv4 port mapping analysis: {} different destination ports tested, mapping varies: {}",
            ipv4_mappings.len(),
            varies
        );
        Some(varies)
    } else if ipv4_mappings.len() == 1 {
        info!("Only one IPv4 port tested, cannot determine variation");
        None
    } else {
        info!("No successful IPv4 probes");
        None
    };

    let ipv6_varies = if ipv6_mappings.len() > 1 {
        let ports: Vec<_> = ipv6_mappings.values().collect();
        let first = ports[0];
        let varies = ports.iter().any(|&p| p != first);
        info!(
            "IPv6 port mapping analysis: {} different destination ports tested, mapping varies: {}",
            ipv6_mappings.len(),
            varies
        );
        Some(varies)
    } else if ipv6_mappings.len() == 1 {
        info!("Only one IPv6 port tested, cannot determine variation");
        None
    } else {
        info!("No successful IPv6 probes");
        None
    };

    info!(
        "Port variation detection complete: IPv4 varies: {:?}, IPv6 varies: {:?}",
        ipv4_varies, ipv6_varies
    );

    if let Some(true) = ipv4_varies {
        info!("IPv4 NAT port mapping varies by destination port!");
        info!(
            "IPv4 mappings: dest_port -> observed_src_port: {:?}",
            ipv4_mappings
        );
    }

    if let Some(true) = ipv6_varies {
        info!("IPv6 NAT port mapping varies by destination port!");
        info!(
            "IPv6 mappings: dest_port -> observed_src_port: {:?}",
            ipv6_mappings
        );
    }

    Ok(PortVariationResult {
        ipv4_varies,
        ipv6_varies,
        ipv4_mappings,
        ipv6_mappings,
    })
}

/// Probe a service on a specific port and get the observed source port
///
/// Creates a temporary endpoint with default relays and n0 discovery,
/// connects to the probe service, and exchanges PROBE/OBSERVED messages.
async fn probe_service_port(
    port: u16,
    timeout: Duration,
    probe_service_node_id: NodeId,
) -> Result<(SocketAddr, u16)> {
    // Create a temporary endpoint for this probe using default relays and n0 discovery
    // Always use a random key for probe endpoints since they're temporary
    let secret_key = create_secret_key(SecretKeyOption::Random)
        .context("Failed to create secret key for probe")?;
    let endpoint = Endpoint::builder()
        .secret_key(secret_key)
        .discovery_n0()
        .relay_mode(iroh::RelayMode::Default)
        .bind()
        .await
        .context("Failed to create probe endpoint")?;

    debug!(
        "Created probe endpoint with node_id: {}",
        endpoint.node_id()
    );

    // Wait 5 sec to get the home relay
    tokio::time::sleep(Duration::from_secs(5)).await;

    let sockets = endpoint.bound_sockets();
    let local_addr = sockets
        .iter()
        .find(|s| s.is_ipv4())
        .or_else(|| sockets.first())
        .copied()
        .context("No bound sockets available")?;

    // The port parameter tells us which port the probe service is listening on
    // We'll send this port number in our probe request
    debug!(
        "Connecting to probe service (node {}) to test port {}",
        probe_service_node_id, port
    );

    let conn = match tokio::time::timeout(
        timeout,
        endpoint.connect(probe_service_node_id, PORT_PROBE_ALPN),
    )
    .await
    {
        Ok(Ok(conn)) => conn,
        Ok(Err(e)) => {
            debug!("Failed to connect to probe service: {}", e);
            endpoint.close().await;
            return Err(e).context("Failed to connect to probe service");
        }
        Err(_) => {
            debug!("Connection to probe service timed out");
            endpoint.close().await;
            return Err(anyhow::anyhow!("Connection to probe service timed out"));
        }
    };

    debug!("Connected to probe service, opening stream");

    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .context("Failed to open bidirectional stream")?;

    let probe_request = format!("PROBE:{port}");
    send.write_all(probe_request.as_bytes())
        .await
        .context("Failed to send probe request")?;
    send.finish().context("Failed to finish send stream")?;

    let response = recv
        .read_to_end(1024)
        .await
        .context("Failed to read probe response")?;
    let response_str = String::from_utf8(response).context("Invalid UTF-8 in probe response")?;

    let observed_port = if let Some(port_str) = response_str.strip_prefix("OBSERVED:") {
        port_str.trim().parse::<u16>()
    } else {
        response_str.trim().parse::<u16>()
    }
    .context("Failed to parse observed port")?;

    debug!(
        "Probe complete - local: {:?}, observed port: {}",
        local_addr, observed_port
    );
    conn.close(0u32.into(), b"probe-done");
    endpoint.close().await;

    Ok((local_addr, observed_port))
}
