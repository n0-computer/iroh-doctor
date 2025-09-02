//! NAT type classification based on network behavior analysis.
//!
//! This module implements proper NAT type detection following RFC 5780 principles
//! and the traditional STUN NAT classification scheme.

use iroh::net_report::Report;
use serde::{Deserialize, Serialize};

/// NAT type classification based on observed network behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NatType {
    /// No NAT - direct public IP access. Best possible P2P connectivity.
    Open,
    /// Full Cone NAT - most permissive NAT type.
    /// Once an internal address is mapped to an external address, any external host
    /// can send packets to the internal host by sending packets to the mapped external address.
    FullCone,
    /// Restricted Cone NAT - filters by source IP.
    /// External hosts can send packets to the internal host only if the internal host
    /// has previously sent a packet to that external host's IP address.
    RestrictedCone,
    /// Port Restricted Cone NAT - filters by source IP and port.
    /// External hosts can send packets to the internal host only if the internal host
    /// has previously sent a packet to that external host's IP address and port.
    PortRestrictedCone,
    /// Symmetric NAT - most restrictive NAT type.
    /// Uses different mappings for different destinations. Poor P2P connectivity.
    Symmetric,
    /// NAT type could not be determined due to insufficient data or test failures.
    Unknown,
}

impl NatType {
    /// Returns a human-readable description of this NAT type.
    pub fn description(&self) -> &'static str {
        match self {
            NatType::Open => "No NAT - direct public IP access. Best possible P2P connectivity.",
            NatType::FullCone => "One-to-one NAT mapping. Any external host can send packets once a mapping is created. Excellent for P2P connectivity.",
            NatType::RestrictedCone => "External hosts can only send packets if the internal host sent to them first (IP restricted). Good for P2P connectivity.",
            NatType::PortRestrictedCone => "Like Restricted Cone NAT, but also restricts by port number. Moderate P2P connectivity.",
            NatType::Symmetric => "Most restrictive NAT. Different mappings for different destinations. Poor P2P connectivity, often requires relay.",
            NatType::Unknown => "NAT type could not be determined. Network report may be unavailable.",
        }
    }

    /// Returns the relative difficulty of establishing P2P connections with this NAT type.
    /// Lower numbers indicate better P2P connectivity.
    pub fn p2p_difficulty(&self) -> u8 {
        match self {
            NatType::Open => 0,
            NatType::FullCone => 1,
            NatType::RestrictedCone => 2,
            NatType::PortRestrictedCone => 3,
            NatType::Symmetric => 4,
            NatType::Unknown => 5,
        }
    }
}

impl std::fmt::Display for NatType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            NatType::Open => "Open",
            NatType::FullCone => "Full Cone",
            NatType::RestrictedCone => "Restricted Cone",
            NatType::PortRestrictedCone => "Port Restricted Cone",
            NatType::Symmetric => "Symmetric",
            NatType::Unknown => "Unknown",
        };
        write!(f, "{name}")
    }
}

/// Classify NAT type based on network report data.
pub fn classify_nat_type(report: &Report) -> NatType {
    // Check if we have global addresses (indicates no NAT or open NAT)
    if report.global_v4.is_some() || report.global_v6.is_some() {
        // If we have UDP connectivity and global addresses, check for NAT behavior
        if report.udp_v4 || report.udp_v6 {
            match (
                report.mapping_varies_by_dest(),
                report.mapping_varies_by_dest_port(),
            ) {
                // Symmetric NAT: mapping varies by destination IP
                (Some(true), _) => NatType::Symmetric,

                // Cone NATs: mapping doesn't vary by destination IP
                (Some(false), Some(true)) => NatType::PortRestrictedCone, // Port varies
                (Some(false), Some(false)) => NatType::FullCone, // Port doesn't vary, but still behind NAT
                (Some(false), None) => NatType::PortRestrictedCone, // Assume more restrictive when port data missing

                // When destination mapping data is missing but port data exists
                (None, Some(true)) => NatType::PortRestrictedCone, // Port varies, likely NAT
                (None, Some(false)) => NatType::Open, // No variation detected, could be direct IP

                // No mapping variation data available
                (None, None) => NatType::Open, // With global IP but no variation data, assume direct connection
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
    use std::net::{Ipv4Addr, SocketAddrV4};

    use super::*;

    fn create_test_report() -> Report {
        let mut report = Report::default();
        report.udp_v4 = true;
        report.global_v4 = Some(SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 1), 12345));
        report
    }

    #[test]
    fn test_full_cone_classification() {
        let mut report = create_test_report();
        report.mapping_varies_by_dest_ipv4 = Some(false);
        report.mapping_varies_by_dest_port_ipv4 = Some(false);

        let nat_type = classify_nat_type(&report);
        assert_eq!(nat_type, NatType::FullCone);
    }

    #[test]
    fn test_port_restricted_cone_with_missing_port_data() {
        let mut report = create_test_report();
        report.mapping_varies_by_dest_ipv4 = Some(false);
        // mapping_varies_by_dest_port_ipv4 = None (missing port variation data)

        let nat_type = classify_nat_type(&report);
        assert_eq!(nat_type, NatType::PortRestrictedCone); // Assume more restrictive
    }

    #[test]
    fn test_port_restricted_cone_classification() {
        let mut report = create_test_report();
        report.mapping_varies_by_dest_ipv4 = Some(false);
        report.mapping_varies_by_dest_port_ipv4 = Some(true);

        let nat_type = classify_nat_type(&report);
        assert_eq!(nat_type, NatType::PortRestrictedCone);
    }

    #[test]
    fn test_symmetric_classification() {
        let mut report = create_test_report();
        report.mapping_varies_by_dest_ipv4 = Some(true);

        let nat_type = classify_nat_type(&report);
        assert_eq!(nat_type, NatType::Symmetric);
    }

    #[test]
    fn test_open_nat_classification() {
        let report = create_test_report();
        // No mapping variation data, suggesting direct IP access
        // mapping_varies_by_dest_ipv4 = None
        // mapping_varies_by_dest_port_ipv4 = None

        let nat_type = classify_nat_type(&report);
        assert_eq!(nat_type, NatType::Open);
    }

    #[test]
    fn test_unknown_classification() {
        let report = Report::default();
        let nat_type = classify_nat_type(&report);
        assert_eq!(nat_type, NatType::Unknown);
    }

    #[test]
    fn test_p2p_difficulty_ordering() {
        assert!(NatType::Open.p2p_difficulty() < NatType::FullCone.p2p_difficulty());
        assert!(NatType::FullCone.p2p_difficulty() < NatType::RestrictedCone.p2p_difficulty());
        assert!(
            NatType::RestrictedCone.p2p_difficulty() < NatType::PortRestrictedCone.p2p_difficulty()
        );
        assert!(NatType::PortRestrictedCone.p2p_difficulty() < NatType::Symmetric.p2p_difficulty());
        assert!(NatType::Symmetric.p2p_difficulty() < NatType::Unknown.p2p_difficulty());
    }
}
