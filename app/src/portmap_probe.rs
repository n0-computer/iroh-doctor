//! Direct port-mapping protocol probe.
//!
//! Mirrors `iroh-doctor`'s `port-map-probe` command: enables every
//! supported protocol (UPnP, PCP, NAT-PMP) and asks the [`portmapper`]
//! crate's client to probe the gateway for support. Reports each
//! protocol's availability as a tri-state boolean so the UI can show
//! "yes / no / not probed".
//!
//! This complements the iroh-services `net_diagnostics` call: when the
//! services API is unreachable the user can still see the local
//! capability picture.

use std::time::Duration;

use portmapper::{Client, Config, Protocol};

/// Result of a one-shot port-mapping protocol probe.
#[derive(Debug, Clone)]
pub struct PortMapProbeResult {
    /// Whether a UPnP gateway responded to the probe.
    pub upnp: Option<bool>,
    /// Whether a PCP gateway responded to the probe.
    pub pcp: Option<bool>,
    /// Whether a NAT-PMP gateway responded to the probe.
    pub nat_pmp: Option<bool>,
    /// Short error description when the probe failed before any
    /// protocol returned a result.
    pub error: Option<String>,
}

/// Probes the local gateway with all three port-mapping protocols.
///
/// Bounds the whole probe by a 5 s timeout because the underlying
/// gateway calls can hang on hostile networks.
pub async fn probe() -> PortMapProbeResult {
    let config = Config {
        enable_upnp: true,
        enable_pcp: true,
        enable_nat_pmp: true,
        protocol: Protocol::Udp,
    };
    // Drop happens at end of scope after the probe future has been
    // awaited; the client is kept alive across the .await for that
    // reason. An earlier version added an explicit drop after the match
    // block, which was a no-op because drop already happens at end of
    // function.
    let client = Client::new(config);
    let probe_rx = client.probe();
    let timeout = Duration::from_secs(5);
    match tokio::time::timeout(timeout, probe_rx).await {
        Ok(Ok(Ok(probe))) => PortMapProbeResult {
            upnp: Some(probe.upnp),
            pcp: Some(probe.pcp),
            nat_pmp: Some(probe.nat_pmp),
            error: None,
        },
        Ok(Ok(Err(e))) => PortMapProbeResult {
            upnp: None,
            pcp: None,
            nat_pmp: None,
            error: Some(e.to_string()),
        },
        Ok(Err(_)) => PortMapProbeResult {
            upnp: None,
            pcp: None,
            nat_pmp: None,
            error: Some("probe service dropped".into()),
        },
        Err(_) => PortMapProbeResult {
            upnp: None,
            pcp: None,
            nat_pmp: None,
            error: Some("probe timed out".into()),
        },
    }
}
