//! NAT type classification from an `iroh::NetReport`.
//!
//! Simplifies NAT behavior into an `Easy / Hard / Unknown` taxonomy based
//! on expected P2P holepunching difficulty. The only behavioral input is
//! the one iroh's net report measures: whether the NAT's address mapping
//! varies by destination (per address family).

use serde::{Deserialize, Serialize};

/// NAT type classification based on expected holepunching difficulty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NatType {
    /// Address mapping does not vary by destination; holepunching is reliable.
    Easy,
    /// Address mapping varies by destination; holepunching is unreliable.
    Hard,
    /// Not enough information in the report to classify.
    Unknown,
}

impl NatType {
    /// Returns a human-readable description of this NAT type.
    #[must_use]
    pub fn description(&self) -> &'static str {
        match self {
            Self::Easy => "NAT type allows easy P2P connectivity",
            Self::Hard => "NAT type is difficult for P2P connectivity",
            Self::Unknown => "NAT type could not be determined. Network report may be unavailable.",
        }
    }
}

impl std::fmt::Display for NatType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Easy => "Easy",
            Self::Hard => "Hard",
            Self::Unknown => "Unknown",
        };
        write!(f, "{name}")
    }
}

/// Classifies NAT type from an [`iroh::NetReport`].
///
/// Returns [`NatType::Unknown`] when there is no globally routable address,
/// no UDP reachability, or no mapping-variation data.
///
/// Each address family is classified on its own from its
/// `mapping_varies_by_dest` axis, then the two combine to the easier
/// outcome, because P2P succeeds over whichever family can holepunch; a
/// family that was not measured is ignored rather than dragging the result
/// down.
#[must_use]
pub fn classify_net_report(report: &iroh::NetReport) -> NatType {
    if report.global_v4.is_none() && report.global_v6.is_none() {
        return NatType::Unknown;
    }
    if !report.udp_v4 && !report.udp_v6 {
        return NatType::Unknown;
    }

    let v4 = classify_family(report.mapping_varies_by_dest_ipv4);
    let v6 = classify_family(report.mapping_varies_by_dest_ipv6);
    combine_families(v4, v6)
}

/// Classifies one address family from its destination-address variation.
/// `None` means the family was not measured.
fn classify_family(varies_by_dest: Option<bool>) -> NatType {
    match varies_by_dest {
        Some(true) => NatType::Hard,
        Some(false) => NatType::Easy,
        None => NatType::Unknown,
    }
}

/// Combines the per-family classifications to the easier of the two, since
/// P2P only needs one family to holepunch. A family that was not measured
/// ([`NatType::Unknown`]) is ignored so it cannot mask a good path on the
/// other family.
fn combine_families(v4: NatType, v6: NatType) -> NatType {
    match (v4, v6) {
        (NatType::Unknown, other) | (other, NatType::Unknown) => other,
        (NatType::Easy, _) | (_, NatType::Easy) => NatType::Easy,
        (NatType::Hard, NatType::Hard) => NatType::Hard,
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddrV4};

    use super::*;

    fn base_report() -> iroh::NetReport {
        iroh::NetReport {
            udp_v4: true,
            global_v4: Some(SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 1), 12345)),
            ..Default::default()
        }
    }

    #[test]
    fn stable_mapping_is_easy() {
        let mut report = base_report();
        report.mapping_varies_by_dest_ipv4 = Some(false);
        assert_eq!(classify_net_report(&report), NatType::Easy);
    }

    #[test]
    fn varying_mapping_is_hard() {
        let mut report = base_report();
        report.mapping_varies_by_dest_ipv4 = Some(true);
        assert_eq!(classify_net_report(&report), NatType::Hard);
    }

    #[test]
    fn unknown_without_variation_data() {
        assert_eq!(classify_net_report(&base_report()), NatType::Unknown);
    }

    #[test]
    fn unknown_on_empty_report() {
        assert_eq!(
            classify_net_report(&iroh::NetReport::default()),
            NatType::Unknown
        );
    }

    #[test]
    fn unknown_when_no_global_addr() {
        let report = iroh::NetReport {
            udp_v4: true,
            mapping_varies_by_dest_ipv4: Some(false),
            ..Default::default()
        };
        assert_eq!(classify_net_report(&report), NatType::Unknown);
    }

    #[test]
    fn unknown_when_no_udp() {
        let mut report = base_report();
        report.udp_v4 = false;
        report.mapping_varies_by_dest_ipv4 = Some(false);
        assert_eq!(classify_net_report(&report), NatType::Unknown);
    }

    #[test]
    fn easier_family_wins_when_both_present() {
        // v4 stable (Easy), v6 address-dependent (Hard). P2P succeeds over
        // v4, so the overall verdict is Easy.
        let mut report = base_report();
        report.mapping_varies_by_dest_ipv4 = Some(false);
        report.mapping_varies_by_dest_ipv6 = Some(true);
        assert_eq!(classify_net_report(&report), NatType::Easy);
    }

    #[test]
    fn ipv6_picked_up_when_ipv4_missing() {
        let mut report = base_report();
        report.mapping_varies_by_dest_ipv6 = Some(true);
        assert_eq!(classify_net_report(&report), NatType::Hard);
    }

    #[test]
    fn description_is_nonempty_for_every_variant() {
        for v in [NatType::Easy, NatType::Hard, NatType::Unknown] {
            assert!(!v.description().is_empty(), "{v} has empty description");
        }
    }

    #[test]
    fn display_matches_name() {
        assert_eq!(format!("{}", NatType::Easy), "Easy");
        assert_eq!(format!("{}", NatType::Hard), "Hard");
        assert_eq!(format!("{}", NatType::Unknown), "Unknown");
    }
}
