//! Test implementations for swarm client

pub(crate) mod connectivity;
pub(crate) mod latency;
pub(crate) mod protocol;
pub(crate) mod throughput;

pub use connectivity::run_connectivity_test;
pub use latency::run_latency_test_with_config;
pub use throughput::run_bidirectional_throughput_test_with_config;
