//! NAT type classification from an `iroh::NetReport`.
//!
//! Simplifies NAT behavior into an `Easy / Medium / Hard / Unknown`
//! taxonomy based on expected P2P holepunching difficulty, considering
//! both address-mapping and (where available) port-mapping variation.
//!
//! Callers that only have a base [`iroh::NetReport`] use
//! [`classify_base_report`]; callers that also collect per-destination-port
//! variation wrap it in an [`ExtendedNetworkReport`] and use
//! [`classify_nat_type`].

use serde::{Deserialize, Serialize};

/// NAT type classification based on expected holepunching difficulty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NatType {
    /// Address mapping does not vary by destination; holepunching is reliable.
    Easy,
    /// Mapping is stable but other behavior may complicate holepunching.
    Medium,
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
            Self::Medium => "NAT type may require additional techniques for P2P connectivity",
            Self::Hard => "NAT type is difficult for P2P connectivity",
            Self::Unknown => "NAT type could not be determined. Network report may be unavailable.",
        }
    }

    /// Returns the relative difficulty of establishing P2P connections with
    /// this NAT type. Lower numbers indicate better P2P connectivity.
    #[must_use]
    pub fn p2p_difficulty(&self) -> u8 {
        match self {
            Self::Easy => 1,
            Self::Medium => 3,
            Self::Hard => 5,
            Self::Unknown => 4,
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

/// An `iroh::NetReport` plus space for per-destination-port variation that
/// iroh does not yet collect. Lets the classifier account for port mapping
/// behavior once that data is available.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtendedNetworkReport {
    /// Base network report from iroh.
    pub base_report: Option<iroh::NetReport>,
    /// Whether the NAT mapping varies by destination port for IPv4 (not yet collected).
    pub mapping_varies_by_dest_port_ipv4: Option<bool>,
    /// Whether the NAT mapping varies by destination port for IPv6 (not yet collected).
    pub mapping_varies_by_dest_port_ipv6: Option<bool>,
}

impl ExtendedNetworkReport {
    /// Wraps a base [`iroh::NetReport`] with no port-variation data.
    pub fn from_base_report(base: Option<iroh::NetReport>) -> Self {
        Self {
            base_report: base,
            mapping_varies_by_dest_port_ipv4: None,
            mapping_varies_by_dest_port_ipv6: None,
        }
    }
}

/// Classifies NAT type from an [`ExtendedNetworkReport`].
///
/// Returns [`NatType::Unknown`] when there is no base report, no globally
/// routable address, no UDP reachability, or no address-mapping-variation
/// data. The mapping-variation lookup is `ipv4.or(ipv6)`.
#[must_use]
pub fn classify_nat_type(report: &ExtendedNetworkReport) -> NatType {
    let Some(ref base) = report.base_report else {
        return NatType::Unknown;
    };

    if base.global_v4.is_none() && base.global_v6.is_none() {
        return NatType::Unknown;
    }
    if !base.udp_v4 && !base.udp_v6 {
        return NatType::Unknown;
    }

    let mapping_varies_by_dest = base
        .mapping_varies_by_dest_ipv4
        .or(base.mapping_varies_by_dest_ipv6);
    let mapping_varies_by_dest_port = report
        .mapping_varies_by_dest_port_ipv4
        .or(report.mapping_varies_by_dest_port_ipv6);

    match (mapping_varies_by_dest, mapping_varies_by_dest_port) {
        (Some(true), _) => NatType::Hard,
        (Some(false), Some(true)) => NatType::Medium,
        (Some(false), None) => NatType::Medium,
        (Some(false), Some(false)) => NatType::Easy,
        (None, _) => NatType::Unknown,
    }
}

/// Classifies NAT type from a base [`iroh::NetReport`] alone.
///
/// Convenience wrapper for callers that have a `NetReport` and do not
/// collect the port-variation extension yet. Without port-variation data a
/// stable address maps to [`NatType::Medium`], never [`NatType::Easy`].
#[must_use]
pub fn classify_base_report(report: &iroh::NetReport) -> NatType {
    classify_nat_type(&ExtendedNetworkReport::from_base_report(Some(
        report.clone(),
    )))
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddrV4};

    use super::*;

    fn create_test_report() -> ExtendedNetworkReport {
        let base = iroh::NetReport {
            udp_v4: true,
            global_v4: Some(SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 1), 12345)),
            ..Default::default()
        };
        ExtendedNetworkReport::from_base_report(Some(base))
    }

    #[test]
    fn test_easy_nat_classification() {
        let mut report = create_test_report();
        if let Some(ref mut base) = report.base_report {
            base.mapping_varies_by_dest_ipv4 = Some(false);
        }
        report.mapping_varies_by_dest_port_ipv4 = Some(false);
        assert_eq!(classify_nat_type(&report), NatType::Easy);
    }

    #[test]
    fn test_medium_nat_classification() {
        let mut report = create_test_report();
        if let Some(ref mut base) = report.base_report {
            base.mapping_varies_by_dest_ipv4 = Some(false);
        }
        report.mapping_varies_by_dest_port_ipv4 = Some(true);
        assert_eq!(classify_nat_type(&report), NatType::Medium);
    }

    #[test]
    fn test_hard_nat_classification() {
        let mut report = create_test_report();
        if let Some(ref mut base) = report.base_report {
            base.mapping_varies_by_dest_ipv4 = Some(true);
        }
        assert_eq!(classify_nat_type(&report), NatType::Hard);
    }

    #[test]
    fn test_missing_variation_data() {
        assert_eq!(classify_nat_type(&create_test_report()), NatType::Unknown);
    }

    #[test]
    fn test_unknown_classification() {
        assert_eq!(
            classify_nat_type(&ExtendedNetworkReport::default()),
            NatType::Unknown
        );
    }

    #[test]
    fn classify_base_report_matches_wrapper_with_no_port_variation() {
        let mut base = iroh::NetReport {
            udp_v4: true,
            global_v4: Some(SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 1), 12345)),
            ..Default::default()
        };
        base.mapping_varies_by_dest_ipv4 = Some(false);

        let direct = classify_base_report(&base);
        let via_wrapper =
            classify_nat_type(&ExtendedNetworkReport::from_base_report(Some(base.clone())));
        assert_eq!(direct, via_wrapper);
        // Without port-variation data, a stable mapping is Medium.
        assert_eq!(direct, NatType::Medium);
    }

    #[test]
    fn classify_base_report_unknown_on_empty() {
        assert_eq!(
            classify_base_report(&iroh::NetReport::default()),
            NatType::Unknown
        );
    }

    #[test]
    fn unknown_when_no_global_addr() {
        let base = iroh::NetReport {
            udp_v4: true,
            mapping_varies_by_dest_ipv4: Some(false),
            ..Default::default()
        };
        assert_eq!(
            classify_nat_type(&ExtendedNetworkReport::from_base_report(Some(base))),
            NatType::Unknown
        );
    }

    #[test]
    fn unknown_when_no_udp() {
        let mut report = create_test_report();
        if let Some(ref mut base) = report.base_report {
            base.udp_v4 = false;
            base.mapping_varies_by_dest_ipv4 = Some(false);
        }
        assert_eq!(classify_nat_type(&report), NatType::Unknown);
    }

    #[test]
    fn ipv4_wins_over_ipv6_when_both_present() {
        // First-present-wins semantics: ipv4 stable, ipv6 variable -> Medium.
        let mut base = iroh::NetReport {
            udp_v4: true,
            global_v4: Some(SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 1), 12345)),
            ..Default::default()
        };
        base.mapping_varies_by_dest_ipv4 = Some(false);
        base.mapping_varies_by_dest_ipv6 = Some(true);
        assert_eq!(classify_base_report(&base), NatType::Medium);
    }

    #[test]
    fn ipv6_picked_up_when_ipv4_missing() {
        let mut base = iroh::NetReport {
            udp_v4: true,
            global_v4: Some(SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 1), 12345)),
            ..Default::default()
        };
        base.mapping_varies_by_dest_ipv6 = Some(true);
        assert_eq!(classify_base_report(&base), NatType::Hard);
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
