//! Code shared between the `iroh-doctor` CLI and the `iroh-doctor-app` GUI.
//!
//! Both binaries diagnose iroh connectivity and used to carry parallel,
//! hand-synchronized copies of the same logic. This crate holds the pieces
//! that genuinely belong to both:
//!
//! - [`nat`]: the NAT classification taxonomy and the function that maps an
//!   `iroh::NetReport` onto it.
//! - [`doctor`]: the wire types for the iroh-doctor connection test
//!   (`n0/doctor/1`).
//! - [`probe`]: the peer probe protocol (`iroh-pong-probe/0`): a passive
//!   responder plus a client that measures latency over time and upload
//!   throughput against a peer.

pub mod doctor;
pub mod nat;
pub mod probe;
