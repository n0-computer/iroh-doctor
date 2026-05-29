//! Code shared between the `iroh-doctor` CLI and the `iroh-doctor-app` GUI.
//!
//! Both binaries diagnose iroh connectivity and used to carry parallel,
//! hand-synchronized copies of the same logic. This crate holds the pieces
//! that genuinely belong to both:
//!
//! - [`identity`]: read-or-create persistence for a raw 32-byte secret key.
//! - [`nat`]: the NAT classification taxonomy and the function that maps an
//!   `iroh::NetReport` onto it.
//! - [`monitor`]: helpers for the live connection monitor (path snapshots,
//!   state derivation, time-to-first-direct-byte).
//! - [`probe`]: the peer probe protocol (`iroh-pong-probe/0`): a passive
//!   responder plus a client that measures latency over time and upload
//!   throughput against a peer.
//! - [`relay_probe`]: per-relay TLS connect plus relay-protocol ping latency.
//! - [`portmap`]: a one-shot UPnP/PCP/NAT-PMP gateway probe.

pub mod identity;
pub mod monitor;
pub mod nat;
pub mod portmap;
pub mod probe;
pub mod relay_probe;
