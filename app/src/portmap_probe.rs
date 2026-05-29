//! Direct port-mapping protocol probe.
//!
//! The probe itself (UPnP/PCP/NAT-PMP via the [`portmapper`] crate) lives in
//! [`iroh_doctor_core::portmap`] so the CLI and the app report the same
//! capability picture. This module re-exports it under the app's existing
//! names; it complements the iroh-services `net_diagnostics` call, which can
//! be unreachable when the services API is down.

pub use iroh_doctor_core::portmap::{probe, PortMapResult as PortMapProbeResult};
