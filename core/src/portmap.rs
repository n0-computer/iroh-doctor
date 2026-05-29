//! One-shot UPnP/PCP/NAT-PMP port-mapping protocol probe shared by the cli
//! and the app.
//!
//! Enables every supported protocol and asks the [`portmapper`] client to
//! probe the local gateway, reporting each protocol's availability as a
//! tri-state boolean (yes / no / not probed). Bounded by a 5 s timeout
//! because the underlying gateway calls can hang on hostile networks.

use std::time::Duration;

use portmapper::{Client, Config, Protocol};
use serde::Serialize;

/// Result of a one-shot port-mapping protocol probe. A `None` field means
/// the probe did not return a result for that protocol; `error` is set when
/// the probe failed before any protocol answered.
#[derive(Debug, Clone, Serialize)]
pub struct PortMapResult {
    /// Whether a UPnP gateway responded to the probe.
    pub upnp: Option<bool>,
    /// Whether a PCP gateway responded to the probe.
    pub pcp: Option<bool>,
    /// Whether a NAT-PMP gateway responded to the probe.
    pub nat_pmp: Option<bool>,
    /// Short error description when the probe failed.
    pub error: Option<String>,
}

/// Wall-clock ceiling for the whole probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Probes the local gateway with all three port-mapping protocols.
pub async fn probe() -> PortMapResult {
    let config = Config {
        enable_upnp: true,
        enable_pcp: true,
        enable_nat_pmp: true,
        protocol: Protocol::Udp,
    };
    // The client is kept alive across the .await: dropping it cancels the
    // in-flight probe. It is dropped at the end of the function, after the
    // probe future has resolved.
    let client = Client::new(config);
    let probe_rx = client.probe();
    match tokio::time::timeout(PROBE_TIMEOUT, probe_rx).await {
        Ok(Ok(Ok(probe))) => PortMapResult {
            upnp: Some(probe.upnp),
            pcp: Some(probe.pcp),
            nat_pmp: Some(probe.nat_pmp),
            error: None,
        },
        Ok(Ok(Err(e))) => PortMapResult {
            upnp: None,
            pcp: None,
            nat_pmp: None,
            error: Some(e.to_string()),
        },
        Ok(Err(_)) => PortMapResult {
            upnp: None,
            pcp: None,
            nat_pmp: None,
            error: Some("probe service dropped".into()),
        },
        Err(_) => PortMapResult {
            upnp: None,
            pcp: None,
            nat_pmp: None,
            error: Some("probe timed out".into()),
        },
    }
}
