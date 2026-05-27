//! NAT type classification from an `iroh::NetReport`.
//!
//! Mirrors the simplified `Easy / Medium / Hard / Unknown` taxonomy
//! used by `iroh-doctor`, but takes a base `NetReport` directly
//! because we do not yet collect the per-destination-port variation
//! that the doctor's `ExtendedNetworkReport` carries. Adding the
//! port-variation extension here is a strict superset and can be
//! done later without breaking callers.
//!
//! The classification is a function of three bits of information in
//! the report:
//!
//! - Whether we observed a globally routable address (`global_v4`
//!   or `global_v6`).
//! - Whether UDP is reachable on either family (`udp_v4` or `udp_v6`).
//! - Whether the public address the network reports varies by
//!   destination (`mapping_varies_by_dest_ipv4` or `..._ipv6`).

use iroh::NetReport;

/// Classification of a NAT's expected behavior for QUIC holepunching.
///
/// Maps to the four buckets a doctor user is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NatType {
    /// Address mapping does not vary by destination; holepunching is reliable.
    Easy,
    /// Address mapping is stable but other behavior may complicate holepunching.
    Medium,
    /// Address mapping varies by destination; holepunching is unreliable.
    Hard,
    /// Not enough information in the report to classify.
    Unknown,
}

impl NatType {
    /// Returns a short human-readable description. The string does not
    /// repeat the variant name; the renderer is expected to show the
    /// [`Display`] form separately.
    pub fn description(self) -> &'static str {
        match self {
            Self::Easy => "P2P connections should establish quickly.",
            Self::Medium => "P2P connections may need extra time or a relay fallback.",
            Self::Hard => "P2P connections are unlikely; expect to relay.",
            Self::Unknown => "Not enough information in the network report to classify.",
        }
    }
}

impl std::fmt::Display for NatType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Easy => "Easy",
            Self::Medium => "Medium",
            Self::Hard => "Hard",
            Self::Unknown => "Unknown",
        };
        write!(f, "{name}")
    }
}

/// Classifies the NAT behavior described by `report`.
///
/// Returns [`NatType::Unknown`] when the report has no globally routable
/// address, no UDP reachability on either family, or no mapping-variation
/// data at all.
///
/// The mapping-variation lookup is `ipv4.or(ipv6)`, matching
/// `iroh-doctor::nat_classifier::classify_nat_type`. The base
/// [`iroh::NetReport`] also exposes a `mapping_varies_by_dest()` helper
/// that ORs the two families, but that disagrees with the doctor on
/// reports where one family is stable and the other is variable. Until
/// the wrappers converge, this function picks the doctor's behavior so
/// the app's "Local network report" panel does not drift from the CLI.
pub fn classify(report: &NetReport) -> NatType {
    let has_global_addr = report.global_v4.is_some() || report.global_v6.is_some();
    if !has_global_addr {
        return NatType::Unknown;
    }
    if !report.has_udp() {
        return NatType::Unknown;
    }
    match report
        .mapping_varies_by_dest_ipv4
        .or(report.mapping_varies_by_dest_ipv6)
    {
        Some(true) => NatType::Hard,
        // No port-variation data yet, so a stable address maps to Medium until
        // we extend the report. If the address is stable we never claim Easy,
        // matching the doctor's existing behavior when the port-variation flag
        // is missing.
        Some(false) => NatType::Medium,
        None => NatType::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddrV4};

    use super::*;

    fn report_with_global_udp() -> NetReport {
        NetReport {
            udp_v4: true,
            global_v4: Some(SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 1), 12345)),
            ..Default::default()
        }
    }

    #[test]
    fn unknown_when_default() {
        assert_eq!(classify(&NetReport::default()), NatType::Unknown);
    }

    #[test]
    fn unknown_when_no_global_addr() {
        let r = NetReport {
            udp_v4: true,
            mapping_varies_by_dest_ipv4: Some(false),
            ..Default::default()
        };
        assert_eq!(classify(&r), NatType::Unknown);
    }

    #[test]
    fn unknown_when_no_udp() {
        let mut r = report_with_global_udp();
        r.udp_v4 = false;
        r.mapping_varies_by_dest_ipv4 = Some(false);
        assert_eq!(classify(&r), NatType::Unknown);
    }

    #[test]
    fn unknown_when_no_variation_data() {
        let r = report_with_global_udp();
        assert_eq!(classify(&r), NatType::Unknown);
    }

    #[test]
    fn medium_when_mapping_stable() {
        let mut r = report_with_global_udp();
        r.mapping_varies_by_dest_ipv4 = Some(false);
        assert_eq!(classify(&r), NatType::Medium);
    }

    #[test]
    fn hard_when_mapping_varies() {
        let mut r = report_with_global_udp();
        r.mapping_varies_by_dest_ipv4 = Some(true);
        assert_eq!(classify(&r), NatType::Hard);
    }

    #[test]
    fn ipv4_wins_over_ipv6_when_both_present() {
        // First-present-wins semantics, matching iroh-doctor's classifier.
        // If this app instead used `mapping_varies_by_dest()` (which ORs
        // the two families), the two would disagree on this input.
        let mut r = report_with_global_udp();
        r.mapping_varies_by_dest_ipv4 = Some(false);
        r.mapping_varies_by_dest_ipv6 = Some(true);
        assert_eq!(classify(&r), NatType::Medium);
    }

    #[test]
    fn ipv6_picked_up_when_ipv4_missing() {
        let mut r = report_with_global_udp();
        r.mapping_varies_by_dest_ipv4 = None;
        r.mapping_varies_by_dest_ipv6 = Some(true);
        assert_eq!(classify(&r), NatType::Hard);
    }

    #[test]
    fn description_is_nonempty_for_every_variant() {
        for v in [
            NatType::Easy,
            NatType::Medium,
            NatType::Hard,
            NatType::Unknown,
        ] {
            assert!(!v.description().is_empty(), "{v} has empty description");
        }
    }

    #[test]
    fn display_matches_name() {
        assert_eq!(format!("{}", NatType::Easy), "Easy");
        assert_eq!(format!("{}", NatType::Medium), "Medium");
        assert_eq!(format!("{}", NatType::Hard), "Hard");
        assert_eq!(format!("{}", NatType::Unknown), "Unknown");
    }
}
