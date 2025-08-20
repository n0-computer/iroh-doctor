//! Test implementations for swarm client

pub mod connectivity;
pub mod latency;
pub mod protocol;
pub mod throughput;

pub use connectivity::run_connectivity_test;
pub use latency::{run_latency_test, run_latency_test_with_config};
pub use protocol::{TestProtocolHeader, DOCTOR_SWARM_ALPN};
pub use throughput::{
    run_bidirectional_throughput_test, run_bidirectional_throughput_test_with_config,
    BidirectionalThroughputResult,
};
