use std::time::Duration;

use anyhow::{Context, Result};
use iroh::{Endpoint, NodeAddr, NodeId};
use irpc::{channel::oneshot, rpc_requests};
use irpc_iroh::IrohRemoteConnection;
use postcard;
use rcan::Rcan;
use serde::{Deserialize, Serialize};
use ssh_key::PrivateKey;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::swarm::{
    client::DOCTOR_ALPN,
    types::{DoctorCaps, NetworkReport, TestCapability, TestConfig, TestType},
};

/// RPC client for communicating with the doctor coordinator
pub struct DoctorClient {
    client: DoctorServiceClient,
    endpoint: iroh::Endpoint,
    node_addr: NodeAddr,
    auth: Auth,
    is_connected: bool,
}

impl std::fmt::Debug for DoctorClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DoctorClient")
            .field("endpoint", &self.endpoint)
            .field("node_addr", &self.node_addr)
            .finish()
    }
}

impl DoctorClient {
    /// Create a new client with SSH key authentication
    pub async fn with_ssh_key<P: AsRef<std::path::Path>>(
        endpoint: &Endpoint,
        node_addr: NodeAddr,
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

        Self::new(endpoint.clone(), node_addr, auth).await
    }

    /// Create a new client and authenticate
    pub async fn new(endpoint: iroh::Endpoint, node_addr: NodeAddr, auth: Auth) -> Result<Self> {
        // Create IRPC connection
        let conn =
            IrohRemoteConnection::new(endpoint.clone(), node_addr.clone(), DOCTOR_ALPN.to_vec());
        let client = DoctorServiceClient::boxed(conn);

        info!("Connected to doctor server at {}", node_addr.node_id);

        debug!("Sending auth message with RCAN");
        let auth_resp = client
            .rpc(auth.clone())
            .await
            .context("Failed to send auth request")?
            .map_err(|e| anyhow::anyhow!("Authentication failed: {:?}", e))?;

        info!("Authenticated with project: {}", auth_resp.project_id);

        Ok(Self {
            client,
            endpoint,
            node_addr,
            auth,
            is_connected: true,
        })
    }

    /// Register with the doctor coordinator
    pub async fn register(
        &self,
        capabilities: Vec<TestCapability>,
        name: Option<String>,
        network_report: Option<NetworkReport>,
    ) -> Result<DoctorRegisterResponse> {
        let response = self
            .client
            .rpc(DoctorRegister {
                capabilities,
                name,
                network_report,
            })
            .await
            .context("Failed to send register request")?
            .map_err(|e| anyhow::anyhow!("Register failed: {:?}", e))?;

        Ok(response)
    }

    /// Send heartbeat
    pub async fn heartbeat(&mut self) -> Result<DoctorHeartbeatResponse> {
        self.heartbeat_with_network_report(None).await
    }

    /// Send heartbeat with network report
    pub async fn heartbeat_with_network_report(
        &mut self,
        network_report: Option<NetworkReport>,
    ) -> Result<DoctorHeartbeatResponse> {
        let response = self
            .client
            .rpc(DoctorHeartbeat { network_report })
            .await
            .map_err(|e| {
                // Mark connection as dead on RPC failure
                self.mark_disconnected();
                anyhow::anyhow!("Failed to send heartbeat request: {}", e)
            })?
            .map_err(|e| anyhow::anyhow!("Heartbeat failed: {:?}", e))?;

        Ok(response)
    }

    /// Get test assignments
    pub async fn get_assignments(&mut self) -> Result<GetTestAssignmentsResponse> {
        debug!("Sending GetTestAssignments request");

        // Debug: Try to serialize the request to see what we're sending
        let test_msg = GetTestAssignments {};
        match postcard::to_allocvec(&test_msg) {
            Ok(bytes) => {
                debug!("GetTestAssignments serialized to {} bytes", bytes.len());
                debug!("First 50 bytes: {:02x?}", &bytes[..bytes.len().min(50)]);
            }
            Err(e) => {
                warn!(
                    "Failed to serialize GetTestAssignments for debugging: {}",
                    e
                );
            }
        }

        let response_result = self.client.rpc(GetTestAssignments {}).await;

        match response_result {
            Ok(result) => {
                match result {
                    Ok(response) => {
                        info!(
                            "Successfully received GetTestAssignmentsResponse with {} assignments",
                            response.assignments.len()
                        );

                        // Log details of received assignments
                        for (i, assignment) in response.assignments.iter().enumerate() {
                            debug!(
                                "Assignment {}: test_run={}, peer={}, type={:?}, has_config={}",
                                i,
                                assignment.test_run_id,
                                assignment.peer_node_id,
                                assignment.test_type,
                                assignment.test_config.is_some()
                            );
                        }

                        Ok(response)
                    }
                    Err(e) => {
                        warn!("GetAssignments RPC returned error: {:?}", e);
                        Err(anyhow::anyhow!("GetAssignments failed: {:?}", e))
                    }
                }
            }
            Err(e) => {
                warn!("GetAssignments RPC transport error: {}", e);

                // Check if it's a deserialization error
                let error_str = format!("{e:?}");
                if error_str.contains("Hit the end of buffer")
                    || error_str.contains("end of buffer")
                {
                    warn!("DESERIALIZATION ERROR DETECTED: This suggests the backend is sending data that can't be deserialized");
                    warn!("Full error details: {:#?}", e);
                }

                // Mark connection as dead on RPC failure
                self.mark_disconnected();
                Err(anyhow::anyhow!(
                    "Failed to send get assignments request: {}",
                    e
                ))
            }
        }
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
    pub async fn report_result(
        &mut self,
        report: TestResultReport,
    ) -> Result<TestResultReportResponse> {
        let response = self
            .client
            .rpc(report)
            .await
            .map_err(|e| {
                // Mark connection as dead on RPC failure
                self.mark_disconnected();
                anyhow::anyhow!("Failed to send report result request: {}", e)
            })?
            .map_err(|e| anyhow::anyhow!("ReportResult failed: {:?}", e))?;

        Ok(response)
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
            .map_err(|e| {
                // Mark connection as dead on RPC failure
                self.mark_disconnected();
                anyhow::anyhow!("Failed to send mark test started request: {}", e)
            })?
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
            *self = Self::new(self.endpoint.clone(), self.node_addr.clone(), auth).await?;
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
    /// Test capabilities this node supports
    pub capabilities: Vec<TestCapability>,
    /// Optional human-readable name for this node
    pub name: Option<String>,
    /// Network report for NAT detection (structured data)
    pub network_report: Option<NetworkReport>,
}

/// Response to doctor registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorRegisterResponse {
    /// Confirmation status
    pub status: String,
    /// Assigned project ID
    pub project_id: Uuid,
}

/// Heartbeat from doctor node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorHeartbeat {
    /// Optional network report update (structured data)
    pub network_report: Option<NetworkReport>,
}

/// Response to heartbeat
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorHeartbeatResponse {
    /// Acknowledgment
    pub ok: bool,
}

/// Request test assignments
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetTestAssignments {
    // Empty - node ID comes from connection
}

/// Test assignment for a node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestAssignment {
    /// Test run ID
    pub test_run_id: Uuid,
    /// Peer node to test with
    pub peer_node_id: NodeId,
    /// Type of test to run
    pub test_type: TestType,
    /// Peer's relay URL for discovery
    pub peer_relay_url: Option<String>,
    /// Test configuration (as JSON string for PostCard compatibility)
    pub test_config: Option<String>,
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
    /// Test metrics/results (as JSON string for PostCard compatibility)
    pub test_result: String,
    /// Request ID for idempotency (optional for backward compatibility)
    pub request_id: Option<Uuid>,
}

/// Response to result report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResultReportResponse {
    /// Confirmation
    pub status: String,
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
    pub result_json: Option<String>,
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
    /// Node ID
    pub node_id: NodeId,
    /// Project ID the node belongs to
    pub project_id: Uuid,
    /// Doctor capabilities if registered
    pub capabilities: Option<Vec<TestCapability>>,
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
    #[rpc(tx = oneshot::Sender<Result<DoctorHeartbeatResponse, DoctorError>>)]
    Heartbeat(DoctorHeartbeat),
    #[rpc(tx = oneshot::Sender<Result<GetTestAssignmentsResponse, DoctorError>>)]
    GetAssignments(GetTestAssignments),
    #[rpc(tx = oneshot::Sender<Result<CreateTestRunResponse, DoctorError>>)]
    CreateTestRun(CreateTestRun),
    #[rpc(tx = oneshot::Sender<Result<TestResultReportResponse, DoctorError>>)]
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
