use std::{path::Path, time::Duration};

use anyhow::{Context, Result};
use iroh::{Endpoint, NodeAddr, NodeId};
use irpc::{channel::oneshot, rpc_requests};
use irpc_iroh::IrohRemoteConnection;
use rcan::Rcan;
use serde::{Deserialize, Serialize};
use ssh_key::PrivateKey;
use tracing::{info, warn};
use uuid::Uuid;

use crate::swarm::{
    client::DOCTOR_ALPN,
    net_report_ext::ExtendedNetworkReport,
    types::{DoctorCaps, TestAssignmentResult, TestConfig, TestType},
};

/// RPC client for communicating with the doctor coordinator
pub struct DoctorClient {
    client: DoctorServiceClient,
    endpoint: iroh::Endpoint,
    coordinator_addr: NodeAddr,
    auth: Auth,
    is_connected: bool,
}

impl std::fmt::Debug for DoctorClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DoctorClient")
            .field("endpoint", &self.endpoint)
            .field("coordinator_addr", &self.coordinator_addr)
            .finish()
    }
}

impl DoctorClient {
    /// Create a new client with SSH key authentication
    pub async fn with_ssh_key<P: AsRef<Path>, N: Into<NodeAddr>>(
        endpoint: &Endpoint,
        node_addr: N,
        ssh_key_path: P,
    ) -> Result<Self> {
        // Load SSH key from file
        let key_content = tokio::fs::read_to_string(ssh_key_path).await?;
        let private_key = PrivateKey::from_openssh(&key_content)?;

        // Create RCAN token with the client's own node ID as audience
        let issuer = private_key
            .key_data()
            .ed25519()
            .context("only Ed25519 keys supported")?;

        let signing_key = ed25519_dalek::SigningKey::from_bytes(&issuer.private.to_bytes());

        // Always create as test node (worker)
        let capability = DoctorCaps::test_node();

        let our_node_id = endpoint.node_id();
        let cap_expiry = Duration::from_secs(60 * 60 * 24 * 30); // 30 days
        let rcan = rcan::Rcan::issuing_builder(&signing_key, our_node_id.public(), capability)
            .sign(rcan::Expires::valid_for(cap_expiry));

        let auth = Auth { caps: rcan };

        Self::new(endpoint.clone(), node_addr.into(), auth).await
    }

    /// Create a new client and authenticate
    pub async fn new(
        endpoint: iroh::Endpoint,
        coordinator_addr: NodeAddr,
        auth: Auth,
    ) -> Result<Self> {
        // Create IRPC connection
        let conn = IrohRemoteConnection::new(
            endpoint.clone(),
            coordinator_addr.clone(),
            DOCTOR_ALPN.to_vec(),
        );
        let client = DoctorServiceClient::boxed(conn);

        info!(
            "Connecting to doctor server at {}",
            coordinator_addr.node_id
        );

        let auth_resp = client
            .rpc(auth.clone())
            .await
            .context("Failed to send auth request")?
            .map_err(|e| anyhow::anyhow!("Authentication failed: {:?}", e))?;

        info!("Authenticated with project: {}", auth_resp.project_id);

        Ok(Self {
            client,
            endpoint,
            coordinator_addr,
            auth,
            is_connected: true,
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
            .map_err(|e| anyhow::anyhow!("Register failed: {:?}", e))?;

        Ok(response)
    }

    /// Get test assignments with network report
    pub async fn get_assignments(
        &mut self,
        network_report: ExtendedNetworkReport,
    ) -> Result<GetTestAssignmentsResponse> {
        let test_msg = GetTestAssignments { network_report };

        self.client
            .rpc(test_msg)
            .await
            .inspect_err(|_| {
                self.mark_disconnected();
            })
            .context("Failed to send get assignments request")?
            .map_err(|e| anyhow::anyhow!("GetAssignments failed: {:?}", e))
    }

    /// Create a test run
    pub async fn create_test_run(
        &self,
        test_type: TestType,
        pair_count: usize,
        config: Option<crate::swarm::TestConfig>,
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
            .map_err(|e| anyhow::anyhow!("CreateTestRun failed: {:?}", e))?;

        Ok(response)
    }

    /// Report test result
    pub async fn report_result(&mut self, report: TestResultReport) -> Result<()> {
        self.client
            .rpc(report)
            .await
            .inspect_err(|_| {
                // Mark connection as dead on RPC failure
                self.mark_disconnected();
            })
            .context("Failed to send report result request")?
            .map_err(|e| anyhow::anyhow!("ReportResult failed: {:?}", e))?;

        Ok(())
    }

    /// Get node info
    pub async fn get_node_info(&self) -> Result<GetNodeInfoResponse> {
        let response = self
            .client
            .rpc(GetNodeInfo {})
            .await
            .context("Failed to send get node info request")?
            .map_err(|e| anyhow::anyhow!("GetNodeInfo failed: {:?}", e))?;

        Ok(response)
    }

    /// Mark a test as started
    pub async fn mark_test_started(
        &mut self,
        test_run_id: Uuid,
        node_a_id: NodeId,
        node_b_id: NodeId,
    ) -> Result<MarkTestStartedResponse> {
        let response = self
            .client
            .rpc(MarkTestStarted {
                test_run_id,
                node_a_id,
                node_b_id,
            })
            .await
            .inspect_err(|_| {
                // Mark connection as dead on RPC failure
                self.mark_disconnected();
            })
            .context("Failed to send mark test started request")?
            .map_err(|e| anyhow::anyhow!("MarkTestStarted failed: {:?}", e))?;

        Ok(response)
    }

    /// Get test run status
    pub async fn get_test_run_status(&self, test_run_id: Uuid) -> Result<GetTestRunStatusResponse> {
        let response = self
            .client
            .rpc(GetTestRunStatus { test_run_id })
            .await
            .context("Failed to send get test run status request")?
            .map_err(|e| anyhow::anyhow!("GetTestRunStatus failed: {:?}", e))?;

        Ok(response)
    }

    /// Check if connection is still alive
    pub fn is_alive(&self) -> bool {
        self.is_connected
    }

    /// Mark connection as dead
    pub fn mark_disconnected(&mut self) {
        self.is_connected = false;
    }

    /// Get the auth token
    pub fn auth(&self) -> &Auth {
        &self.auth
    }

    /// Reconnect if needed
    pub async fn ensure_connected(&mut self) -> Result<()> {
        // If we detect the connection is broken (through failed requests),
        // we can recreate the entire client
        if !self.is_alive() {
            warn!("Connection lost, reconnecting...");
            let auth = self.auth.clone();
            *self = Self::new(self.endpoint.clone(), self.coordinator_addr.clone(), auth).await?;
        }
        Ok(())
    }
}

/// IRPC client type for DoctorService
pub type DoctorServiceClient = irpc::Client<DoctorProtocol>;

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
    pub node_id: NodeId,
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
    /// Confirmation status
    pub status: String,
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
    /// Target node to test with
    pub node_id: NodeId,
    /// Type of test to run
    pub test_type: TestType,
    /// Test configuration
    pub test_config: TestConfig,
    /// Retry count for this assignment (0 for original, 1+ for retries)
    #[serde(default)]
    pub retry_count: i32,
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
    pub node_a: NodeId,
    /// Second node in pair
    pub node_b: NodeId,
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
    pub node_a_id: NodeId,
    /// Second node in the test
    pub node_b_id: NodeId,
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
    pub node_a_id: NodeId,
    /// Second node in the test
    pub node_b_id: NodeId,
    /// Whether the test succeeded
    pub success: bool,
    /// Error message if test failed
    pub error: Option<String>,
    /// Request ID for idempotency (optional for backward compatibility)
    pub request_id: Option<Uuid>,
    /// Full test result data
    pub result_data: TestAssignmentResult,
}

/// Get test run status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetTestRunStatus {
    pub test_run_id: Uuid,
}

/// Test pair result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestPairResult {
    pub node_a_id: NodeId,
    pub node_b_id: NodeId,
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
pub struct GetNodeInfo {
    // Empty - node ID comes from connection
}

/// Node info response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetNodeInfoResponse {
    /// Node ID for which info is requested
    pub node_id: NodeId,
    /// Project ID the node belongs to
    pub project_id: Uuid,
}

/// Submit metrics update
#[derive(Debug, Serialize, Deserialize)]
pub struct PutMetrics {
    pub update: iroh_metrics::encoding::Update,
}

/// Metrics submission response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PutMetricsResponse {
    pub stored_count: u64,
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
    #[rpc(tx = oneshot::Sender<Result<PutMetricsResponse, DoctorError>>)]
    PutMetrics(PutMetrics),
}
