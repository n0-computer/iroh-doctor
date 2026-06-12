//! Code shared between the `iroh-doctor` CLI and the `iroh-doctor-app` GUI.
//!
//! Both binaries diagnose iroh connectivity and used to carry parallel,
//! hand-synchronized copies of the same logic. This crate holds the pieces
//! that genuinely belong to both:
//!
//! - [`fmt`]: tiny text renderings (yes/no/unknown) shared by the cli tables
//!   and the app UI.
//! - [`identity`]: read-or-create persistence for a raw 32-byte secret key.
//! - [`nat`]: the NAT classification taxonomy and the function that maps an
//!   `iroh::NetReport` onto it.
//! - [`monitor`]: helpers for the live connection monitor (path snapshots,
//!   state derivation, time-to-first-direct-byte).
//! - [`probe`]: the peer probe protocol (`iroh-pong-probe/0`): a passive
//!   responder plus a client that measures latency over time and upload
//!   throughput against a peer.
//! - [`report`]: UI-facing projections of `iroh::NetReport` (per-relay
//!   latency rows).
//! - [`services`]: iroh-services client setup, API-secret resolution, and the
//!   ping + net_diagnostics queries.

pub mod fmt;
pub mod identity;

/// Wall-clock ceiling both binaries apply to the
/// `endpoint.net_report().initialized()` wait. The reporter streams updates
/// indefinitely; on a network with no DNS or no reachable STUN endpoints the
/// wait would otherwise hang forever.
pub const NET_REPORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

pub mod monitor;
pub mod nat;
pub mod probe;
pub mod report;
pub mod services;
