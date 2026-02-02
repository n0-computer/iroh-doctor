use std::{path::Path, time::Duration};

use anyhow::{anyhow, Context, Result};
use iroh::{Endpoint, EndpointAddr, EndpointId};
use irpc::{channel::oneshot, rpc_requests};
use irpc_iroh::IrohRemoteConnection;
use rcan::Rcan;
use serde::{Deserialize, Serialize};
use ssh_key::PrivateKey;
use uuid::Uuid;

use crate::swarm::{
    client::N0DES_DOCTOR_ALPN,
    net_report_ext::ExtendedNetworkReport,
    types::{DoctorCaps, TestAssignmentResult, TestConfig, TestType},
};

/// RPC client for communicating with the doctor coordinator
pub struct DoctorClient {
    client: DoctorServiceClient,
    coordinator_addr: EndpointAddr,
    auth: Auth,
}

impl std::fmt::Debug for DoctorClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DoctorClient")
            .field("coordinator_addr", &self.coordinator_addr)
            .finish_non_exhaustive()
    }
}

impl DoctorClient {
    /// Create a new client with SSH key authentication
    pub async fn with_ssh_key<P: AsRef<Path>, N: Into<EndpointAddr>>(
        endpoint: &Endpoint,
        node_addr: N,
        ssh_key_path: P,
    ) -> Result<Self> {
        let key_content = tokio::fs::read_to_string(ssh_key_path).await?;
        let private_key = PrivateKey::from_openssh(&key_content)?;

        let issuer = private_key
            .key_data()
            .ed25519()
            .context("only Ed25519 keys supported")?;

        let signing_key = ed25519_dalek::SigningKey::from_bytes(&issuer.private.to_bytes());

        let capability = DoctorCaps::default();

        let our_node_id = endpoint.id();
        let cap_expiry = Duration::from_secs(60 * 60 * 24 * 30); // 30 days
        let rcan =
            rcan::Rcan::issuing_builder(&signing_key, our_node_id.as_verifying_key(), capability)
                .sign(rcan::Expires::valid_for(cap_expiry));

        let auth = Auth { caps: rcan };

        Self::new(endpoint.clone(), node_addr.into(), auth).await
    }

    /// Create a new client and authenticate
    pub async fn new(
        endpoint: iroh::Endpoint,
        coordinator_addr: EndpointAddr,
        auth: Auth,
    ) -> Result<Self> {
        let connection = endpoint
            .connect(coordinator_addr.clone(), N0DES_DOCTOR_ALPN)
            .await
            .context("Failed to connect to coordinator")?;
        let conn = IrohRemoteConnection::new(connection);
        let client = DoctorServiceClient::boxed(conn);

        client
            .rpc(auth.clone())
            .await
            .context("Failed to send auth request")?
            .map_err(|e| anyhow!("Authentication failed: {e:?}"))?;

        Ok(Self {
            client,
            coordinator_addr,
            auth,
        })
    }

    /// Register with the doctor coordinator
    pub async fn register(
        &self,
        name: Option<String>,
        network_report: Option<ExtendedNetworkReport>,
    ) -> Result<DoctorRegisterResponse> {
        let response = self
            .client
            .rpc(DoctorRegister {
                name,
                network_report,
            })
            .await
            .context("Failed to send register request")?
            .map_err(|e| anyhow!("Register failed: {e:?}"))?;

        Ok(response)
    }

    /// Get test assignments with network report
    pub async fn get_assignments(
        &self,
        network_report: ExtendedNetworkReport,
    ) -> Result<GetTestAssignmentsResponse> {
        let test_msg = GetTestAssignments { network_report };

        self.client
            .rpc(test_msg)
            .await
            .context("Failed to send get assignments request")?
            .map_err(|e| anyhow!("GetAssignments failed: {e:?}"))
    }

    /// Create a test run
    pub async fn create_test_run(
        &self,
        test_type: TestType,
        pair_count: usize,
        config: Option<TestConfig>,
    ) -> Result<CreateTestRunResponse> {
        let response = self
            .client
            .rpc(CreateTestRun {
                test_type,
                pair_count,
                config,
            })
            .await
            .context("Failed to send create test run request")?
            .map_err(|e| anyhow!("CreateTestRun failed: {e:?}"))?;

        Ok(response)
    }

    /// Report test result
    pub async fn report_result(&self, report: TestResultReport) -> Result<()> {
        self.client
            .rpc(report)
            .await
            .context("Failed to send report result request")?
            .map_err(|e| anyhow!("ReportResult failed: {e:?}"))?;

        Ok(())
    }

    /// Get node info
    pub async fn get_node_info(&self) -> Result<GetNodeInfoResponse> {
        let response = self
            .client
            .rpc(GetNodeInfo {})
            .await
            .context("Failed to send get node info request")?
            .map_err(|e| anyhow!("GetNodeInfo failed: {e:?}"))?;

        Ok(response)
    }

    /// Mark a test as started
    pub async fn mark_test_started(
        &self,
        test_run_id: Uuid,
        node_a: EndpointId,
        node_b: EndpointId,
    ) -> Result<MarkTestStartedResponse> {
        let response = self
            .client
            .rpc(MarkTestStarted {
                test_run_id,
                node_a,
                node_b,
            })
            .await
            .context("Failed to send mark test started request")?
            .map_err(|e| anyhow!("MarkTestStarted failed: {e:?}"))?;

        Ok(response)
    }

    /// Get test run status
    pub async fn get_test_run_status(&self, test_run_id: Uuid) -> Result<GetTestRunStatusResponse> {
        let response = self
            .client
            .rpc(GetTestRunStatus { test_run_id })
            .await
            .context("Failed to send get test run status request")?
            .map_err(|e| anyhow!("GetTestRunStatus failed: {e:?}"))?;

        Ok(response)
    }

    /// Get the auth token
    pub fn auth(&self) -> &Auth {
        &self.auth
    }
}

/// IRPC client type for DoctorService
pub(super) type DoctorServiceClient = irpc::Client<DoctorProtocol>;

/// Authentication message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Auth {
    /// RCAN with doctor capabilities
    pub caps: Rcan<DoctorCaps>,
}

/// Authentication response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub project_id: Uuid,
    pub node_id: EndpointId,
}

/// Register a doctor node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorRegister {
    /// Optional human-readable name for this node
    pub name: Option<String>,
    /// Extended network report for NAT detection including port variation
    pub network_report: Option<ExtendedNetworkReport>,
}

/// Response to doctor registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorRegisterResponse {
    /// Assigned project ID
    pub project_id: Uuid,
}

/// Request test assignments
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetTestAssignments {
    /// Network report to send with request (acts as heartbeat)
    pub network_report: ExtendedNetworkReport,
}

/// Test assignment for a node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestAssignment {
    /// Test run ID
    pub test_run_id: Uuid,
    /// Target node to test against
    pub node_id: EndpointId,
    /// Type of test to run
    pub test_type: TestType,
    /// Test configuration
    pub test_config: TestConfig,
}

impl TestAssignment {
    /// Extract throughput configuration with defaults
    ///
    /// Returns (parallel_streams, chunk_size_bytes)
    pub fn throughput_config(&self) -> (usize, usize) {
        use crate::swarm::types::{DEFAULT_CHUNK_SIZE, DEFAULT_PARALLEL_STREAMS};

        let throughput_config = self
            .test_config
            .advanced
            .as_ref()
            .and_then(|c| c.throughput.as_ref());

        let parallel_streams = throughput_config
            .as_ref()
            .and_then(|c| c.parallel_streams)
            .unwrap_or(DEFAULT_PARALLEL_STREAMS)
            .clamp(1, 16) as usize; // enforce range: 1-16

        let chunk_size_bytes = throughput_config
            .as_ref()
            .and_then(|c| c.chunk_size_kb.map(|kb| (kb as usize) * 1024))
            .unwrap_or(DEFAULT_CHUNK_SIZE)
            .clamp(1024, 16 * 1024 * 1024); // enforce range: 1KB-16MB

        (parallel_streams, chunk_size_bytes)
    }

    /// Extract latency test configuration with defaults
    ///
    /// Returns (ping_interval, ping_timeout)
    pub fn latency_config(&self) -> (Duration, Duration) {
        use crate::swarm::types::{DEFAULT_PING_INTERVAL, DEFAULT_PING_TIMEOUT};

        let advanced_config = self
            .test_config
            .advanced
            .as_ref()
            .and_then(|a| a.latency.as_ref());

        let ping_interval_ms = advanced_config
            .as_ref()
            .and_then(|c| c.ping_interval_ms)
            .unwrap_or(DEFAULT_PING_INTERVAL.as_millis() as u32)
            .clamp(1, 1000); // enforce range: 1-1000ms

        let ping_timeout_ms = advanced_config
            .as_ref()
            .and_then(|c| c.ping_timeout_ms)
            .unwrap_or(DEFAULT_PING_TIMEOUT.as_millis() as u32)
            .clamp(100, 10000); // enforce range: 100ms-10s

        (
            Duration::from_millis(ping_interval_ms as u64),
            Duration::from_millis(ping_timeout_ms as u64),
        )
    }
}

/// Response with test assignments
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetTestAssignmentsResponse {
    /// List of test assignments
    pub assignments: Vec<TestAssignment>,
}

/// Create a new test run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTestRun {
    /// Type of test to run
    pub test_type: TestType,
    /// Number of node pairs to test
    pub pair_count: usize,
    /// Optional test configuration
    pub config: Option<TestConfig>,
}

/// Test pair assignment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestPair {
    /// First node in pair
    pub node_a: EndpointId,
    /// Second node in pair
    pub node_b: EndpointId,
}

/// Response to test run creation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTestRunResponse {
    /// Created test run ID
    pub test_run_id: Uuid,
    /// Assigned node pairs
    pub pairs: Vec<TestPair>,
}

/// Mark a test as started
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkTestStarted {
    /// Test run ID
    pub test_run_id: Uuid,
    /// First node in the test
    pub node_a: EndpointId,
    /// Second node in the test
    pub node_b: EndpointId,
}

/// Response to marking test as started
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkTestStartedResponse {
    /// Whether the test was successfully marked as started
    pub success: bool,
}

/// Report test results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResultReport {
    /// Test run ID
    pub test_run_id: Uuid,
    /// First node in the test
    pub node_a: EndpointId,
    /// Second node in the test
    pub node_b: EndpointId,
    /// Request ID for idempotency (optional for backward compatibility)
    pub request_id: Option<Uuid>,
    /// Full test result data
    pub result_data: TestAssignmentResult,
}

impl TestResultReport {
    /// Check if the test was successful
    pub fn success(&self) -> bool {
        !matches!(&self.result_data, TestAssignmentResult::Error(_))
    }

    /// Get the error message if test failed
    pub fn error(&self) -> Option<&str> {
        match &self.result_data {
            TestAssignmentResult::Error(e) => Some(&e.error),
            _ => None,
        }
    }
}

/// Get test run status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetTestRunStatus {
    pub test_run_id: Uuid,
}

/// Test pair result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestPairResult {
    pub node_a: EndpointId,
    pub node_b: EndpointId,
    pub status: String,
    pub success: bool,
    pub error: Option<String>,
}

/// Test run status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetTestRunStatusResponse {
    pub test_run_id: Uuid,
    pub status: String,
    pub total_pairs: usize,
    pub completed_pairs: usize,
    pub successful_pairs: usize,
    pub failed_pairs: usize,
    pub pair_results: Vec<TestPairResult>,
}

/// Get node info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetNodeInfo {}

/// Node info response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetNodeInfoResponse {
    /// Node ID for which info is requested
    pub node_id: EndpointId,
    /// Project ID the node belongs to
    pub project_id: Uuid,
}

/// Doctor protocol errors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DoctorError {
    Unauthorized,
    InvalidRequest,
    InternalError,
    NotFound,
}

/// Doctor service marker
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DoctorService;

/// Doctor protocol messages
#[rpc_requests(message = DoctorMessage)]
#[derive(Debug, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum DoctorProtocol {
    #[rpc(tx = oneshot::Sender<Result<AuthResponse, DoctorError>>)]
    Auth(Auth),
    #[rpc(tx = oneshot::Sender<Result<DoctorRegisterResponse, DoctorError>>)]
    Register(DoctorRegister),
    #[rpc(tx = oneshot::Sender<Result<GetTestAssignmentsResponse, DoctorError>>)]
    GetAssignments(GetTestAssignments),
    #[rpc(tx = oneshot::Sender<Result<CreateTestRunResponse, DoctorError>>)]
    CreateTestRun(CreateTestRun),
    #[rpc(tx = oneshot::Sender<Result<(), DoctorError>>)]
    ReportResult(TestResultReport),
    #[rpc(tx = oneshot::Sender<Result<GetNodeInfoResponse, DoctorError>>)]
    GetNodeInfo(GetNodeInfo),
    #[rpc(tx = oneshot::Sender<Result<MarkTestStartedResponse, DoctorError>>)]
    MarkTestStarted(MarkTestStarted),
    #[rpc(tx = oneshot::Sender<Result<GetTestRunStatusResponse, DoctorError>>)]
    GetTestRunStatus(GetTestRunStatus),
}
