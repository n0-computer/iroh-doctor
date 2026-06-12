//! The app's namespace for the headless doctor node, which lives in
//! [`iroh_doctor_core::node`], plus the report projections the UI renders.
//! Everything here is UI-framework-free; main.rs folds the node's event
//! stream into Dioxus signals.

pub use iroh_doctor_core::monitor::{PathKind, PathSnapshot};
pub use iroh_doctor_core::node::*;
pub use iroh_doctor_core::report::NetReportSummary;
pub use iroh_doctor_core::services::DiagnosticsReport;
