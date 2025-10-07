//! Swarm module for doctor network testing

mod client;
mod config;
mod execution;
mod net_report_ext;
mod rpc;
mod runner;
mod tests;
mod transfer_utils;
mod types;

pub use client::{SwarmClient, N0DES_DOCTOR_ALPN};
pub use config::{SwarmConfig, TransportConfig};
pub use net_report_ext::ExtendedNetworkReport;
pub use rpc::{
    Auth, AuthResponse, CreateTestRun, CreateTestRunResponse, DoctorClient, DoctorError,
    DoctorMessage, DoctorProtocol, DoctorRegister, DoctorRegisterResponse, DoctorService,
    GetNodeInfo, GetNodeInfoResponse, GetTestAssignments, GetTestAssignmentsResponse,
    GetTestRunStatus, GetTestRunStatusResponse, MarkTestStarted, MarkTestStartedResponse,
    PutMetrics, PutMetricsResponse, TestAssignment, TestPair, TestPairResult, TestResultReport,
};
pub use runner::run_swarm_client;
pub use types::{
    AdvancedTestConfig, DoctorCaps, ErrorResult, FingerprintResult, LatencyConfig, LatencyResult,
    NetworkConfig, StreamStats, SwarmStats, TestAssignmentResult, TestConfig, TestStats, TestType,
    ThroughputConfig, ThroughputResult,
};
