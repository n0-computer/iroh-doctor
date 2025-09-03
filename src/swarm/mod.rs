//! Swarm module for doctor network testing

mod client;
mod config;
mod execution;
mod rpc;
mod runner;
mod tests;
mod types;

// Public API exports
// Main entry point for swarm functionality
pub use runner::run_swarm_client;

// Client types
pub use client::{SwarmClient, DOCTOR_ALPN};

// Configuration types
pub use config::{SwarmConfig, TransportConfig};

// Test types and results
pub use types::{
    TestType, TestConfig, TestCapability, TestStats, StreamStats, SwarmStats,
    AdvancedTestConfig, ThroughputAdvancedConfig, LatencyAdvancedConfig, NetworkAdvancedConfig,
    DoctorCaps, ErrorResult, TestAssignmentResult,
};

// RPC protocol and service types
pub use rpc::{
    DoctorError, TestAssignment, TestPair, TestPairResult,
    DoctorProtocol, DoctorMessage, DoctorService,
    // Request/Response types
    Auth, AuthResponse,
    DoctorRegister, DoctorRegisterResponse,
    DoctorHeartbeat, DoctorHeartbeatResponse,
    GetNodeInfo, GetNodeInfoResponse,
    GetTestAssignments, GetTestAssignmentsResponse,
    PutMetrics, PutMetricsResponse,
    TestResultReport, TestResultReportResponse,
    CreateTestRun, CreateTestRunResponse,
    GetTestRunStatus, GetTestRunStatusResponse,
    MarkTestStarted, MarkTestStartedResponse,
    // Client
    DoctorClient,
};
