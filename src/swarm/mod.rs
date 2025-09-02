//! Swarm module for doctor network testing

pub mod client;
pub mod config;
pub mod execution;
pub mod rpc;
pub mod runner;
pub mod tests;
pub mod types;

// Re-export main types for backward compatibility
pub use client::{SwarmClient, DOCTOR_ALPN};
pub use config::{SwarmConfig, TransportConfig};
pub use execution::execute_test;
// Re-export RPC and domain types
pub use rpc::{
    Auth, AuthResponse, CreateTestRun, CreateTestRunResponse, DoctorClient, DoctorError,
    DoctorHeartbeat, DoctorHeartbeatResponse, DoctorProtocol, DoctorRegister,
    DoctorRegisterResponse, DoctorService, GetNodeInfo, GetNodeInfoResponse, GetTestAssignments,
    GetTestAssignmentsResponse, GetTestRunStatus, GetTestRunStatusResponse, MarkTestStarted,
    MarkTestStartedResponse, PutMetrics, PutMetricsResponse, TestAssignment, TestPair,
    TestPairResult, TestResultReport, TestResultReportResponse,
};
pub use runner::run_swarm_client;
pub use tests::throughput::BidirectionalThroughputResult;
pub use types::{
    AdvancedTestConfig, DoctorCaps, LatencyAdvancedConfig, NetworkAdvancedConfig, StreamStats,
    SwarmStats, TestCapability, TestConfig, TestStats, TestType, ThroughputAdvancedConfig,
};
