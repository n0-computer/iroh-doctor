//! Per-relay latency probe.
//!
//! The probe itself (TLS connect + relay-protocol ping, sorted by ping)
//! lives in [`iroh_doctor_core::relay_probe`] so the CLI and the app report
//! the same numbers. This module is a thin adapter for the peer task, which
//! holds an owned `RelayMap`.

use iroh::RelayMap;

pub use iroh_doctor_core::relay_probe::RelayProbeResult;

/// Probes every relay in `relay_map` once, sorted ascending by ping with
/// failures last. See [`iroh_doctor_core::relay_probe::probe_relays`].
pub async fn probe(relay_map: RelayMap) -> Vec<RelayProbeResult> {
    iroh_doctor_core::relay_probe::probe_relays(&relay_map).await
}
