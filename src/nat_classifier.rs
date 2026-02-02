//! NAT type classification based on network behavior analysis.
//!
//! This module classifies NAT types based on iroh's network reports and our extended
//! port variation detection. It simplifies NAT types into Easy/Medium/Hard categories
//! based on expected P2P connectivity difficulty, considering both address mapping
//! behavior and port mapping behavior.

use serde::{Deserialize, Serialize};

use crate::swarm::ExtendedNetworkReport;

/// NAT type classification based on expected holepunching difficulty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NatType {
    Easy,
    Medium,
    Hard,
    Unknown,
}

impl NatType {
    /// Returns a human-readable description of this NAT type.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Easy => "NAT type allows easy P2P connectivity",
            Self::Medium => "NAT type may require additional techniques for P2P connectivity",
            Self::Hard => "NAT type is difficult for P2P connectivity",
            Self::Unknown => "NAT type could not be determined. Network report may be unavailable.",
        }
    }

    /// Returns the relative difficulty of establishing P2P connections with this NAT type.
    /// Lower numbers indicate better P2P connectivity.
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

/// Classify NAT type based on network report data.
pub fn classify_nat_type(report: &ExtendedNetworkReport) -> NatType {
    // Check if we have a base report
    let Some(ref base) = report.base_report else {
        return NatType::Unknown;
    };

    // Check if we have global addresses (indicates no NAT or open NAT)
    if base.global_v4.is_some() || base.global_v6.is_some() {
        // If we have UDP connectivity and global addresses, check for NAT behavior
        if base.udp_v4 || base.udp_v6 {
            // Check both IPv4 and IPv6 mapping variations
            let mapping_varies_by_dest = base
                .mapping_varies_by_dest_ipv4
                .or(base.mapping_varies_by_dest_ipv6);
            let mapping_varies_by_dest_port = report
                .mapping_varies_by_dest_port_ipv4
                .or(report.mapping_varies_by_dest_port_ipv6);

            match (mapping_varies_by_dest, mapping_varies_by_dest_port) {
                (Some(true), Some(true)) => NatType::Hard,
                (Some(true), Some(false)) => NatType::Hard,
                (Some(true), None) => NatType::Hard,
                (Some(false), Some(true)) => NatType::Medium,
                (Some(false), None) => NatType::Medium,
                (Some(false), Some(false)) => NatType::Easy,
                (None, _) => NatType::Unknown,
            }
        } else {
            NatType::Unknown
        }
    } else {
        NatType::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_report() -> ExtendedNetworkReport {
        let base = iroh::NetReport {
            udp_v4: true,
            global_v4: Some(std::net::SocketAddrV4::new(
                std::net::Ipv4Addr::new(203, 0, 113, 1),
                12345,
            )),
            ..Default::default()
        };
        ExtendedNetworkReport::from_base_report(Some(base))
    }

    #[test]
    fn test_easy_nat_classification() {
        // Easy NAT: No address or port mapping variation
        // Best case for P2P connectivity
        let mut report = create_test_report();
        if let Some(ref mut base) = report.base_report {
            base.mapping_varies_by_dest_ipv4 = Some(false);
        }
        report.mapping_varies_by_dest_port_ipv4 = Some(false);

        let nat_type = classify_nat_type(&report);
        assert_eq!(nat_type, NatType::Easy);
    }

    #[test]
    fn test_medium_nat_classification() {
        // Medium NAT: No address variation but port mapping varies
        // Moderate difficulty for P2P connectivity
        let mut report = create_test_report();
        if let Some(ref mut base) = report.base_report {
            base.mapping_varies_by_dest_ipv4 = Some(false);
        }
        report.mapping_varies_by_dest_port_ipv4 = Some(true);

        let nat_type = classify_nat_type(&report);
        assert_eq!(nat_type, NatType::Medium);
    }

    #[test]
    fn test_hard_nat_classification() {
        // Hard NAT: Address mapping varies by destination
        // Most difficult for P2P connectivity
        let mut report = create_test_report();
        if let Some(ref mut base) = report.base_report {
            base.mapping_varies_by_dest_ipv4 = Some(true);
        }

        let nat_type = classify_nat_type(&report);
        assert_eq!(nat_type, NatType::Hard);
    }

    #[test]
    fn test_missing_variation_data() {
        let report = create_test_report();
        // When mapping variation data is missing (None), we can't determine NAT type
        // even though we have global addresses and UDP connectivity

        let nat_type = classify_nat_type(&report);
        assert_eq!(nat_type, NatType::Unknown);
    }

    #[test]
    fn test_unknown_classification() {
        let report = ExtendedNetworkReport::default();
        let nat_type = classify_nat_type(&report);
        assert_eq!(nat_type, NatType::Unknown);
    }
}
