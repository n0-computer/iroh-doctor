//! Extended network report

use serde::{Deserialize, Serialize};

/// Extended network report that includes standard iroh report
/// plus space for future custom fields
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtendedNetworkReport {
    /// Base network report from iroh
    pub base_report: Option<iroh::net_report::Report>,

    /// Whether the NAT mapping varies by destination PORT for IPv4 (not implemented)
    pub mapping_varies_by_dest_port_ipv4: Option<bool>,

    /// Whether the NAT mapping varies by destination PORT for IPv6 (not implemented)
    pub mapping_varies_by_dest_port_ipv6: Option<bool>,
}

impl ExtendedNetworkReport {
    /// Create an extended report from a standard iroh Report
    pub fn from_base_report(base: Option<iroh::net_report::Report>) -> Self {
        Self {
            base_report: base,
            mapping_varies_by_dest_port_ipv4: None,
            mapping_varies_by_dest_port_ipv6: None,
        }
    }
}
