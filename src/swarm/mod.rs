//! Swarm module for doctor network testing

mod client;
mod config;
mod execution;
pub mod net_report_ext;
mod rpc;
mod runner;
mod tests;
mod transfer_utils;
mod types;

// Public API exports
// Main entry point for swarm functionality
// Client types
pub use client::{SwarmClient, DOCTOR_ALPN};
// Configuration types
pub use config::{SwarmConfig, TransportConfig};
// Extended network report
pub use net_report_ext::ExtendedNetworkReport;
// RPC protocol and service types
pub use rpc::{
    // Request/Response types
    Auth,
    AuthResponse,
    CreateTestRun,
    CreateTestRunResponse,
    // Client
    DoctorClient,
    DoctorError,
    DoctorMessage,
    DoctorProtocol,
    DoctorRegister,
    DoctorRegisterResponse,
    DoctorService,
    GetNodeInfo,
    GetNodeInfoResponse,
    GetTestAssignments,
    GetTestAssignmentsResponse,
    GetTestRunStatus,
    GetTestRunStatusResponse,
    MarkTestStarted,
    MarkTestStartedResponse,
    PutMetrics,
    PutMetricsResponse,
    TestAssignment,
    TestPair,
    TestPairResult,
    TestResultReport,
};
pub use runner::run_swarm_client;
// Test types and results
pub use types::{
    AdvancedTestConfig, DoctorCaps, ErrorResult, FingerprintResult, LatencyAdvancedConfig,
    LatencyResult, NetworkAdvancedConfig, StreamStats, SwarmStats, TestAssignmentResult,
    TestConfig, TestStats, TestType, ThroughputAdvancedConfig, ThroughputResult,
};
