//! Protocol definitions for test communication

use std::fmt;

use anyhow::Result;
use postcard;
use serde::{Deserialize, Serialize};

/// ALPN protocol identifier for doctor swarm tests
pub const DOCTOR_SWARM_ALPN: &[u8] = b"n0/doctor-swarm/1";

/// Test protocol types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestProtocolType {
    Throughput,
    Latency,
}

impl fmt::Display for TestProtocolType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Throughput => write!(f, "THROUGHPUT"),
            Self::Latency => write!(f, "LATENCY"),
        }
    }
}

/// Protocol messages for latency testing
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LatencyMessage {
    Ping(u32),
    Pong(u32),
}

impl LatencyMessage {
    pub fn ping(number: u32) -> Self {
        LatencyMessage::Ping(number)
    }

    pub fn pong(number: u32) -> Self {
        LatencyMessage::Pong(number)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        postcard::to_allocvec(self).expect("failed to serialize latency message")
    }

    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        postcard::from_bytes(data).ok()
    }
}

/// Protocol header for test messages
#[derive(Debug, Serialize, Deserialize)]
pub struct TestProtocolHeader {
    pub test_type: TestProtocolType,
    pub data_size: u64,
    pub parallel_streams: Option<usize>,
    pub chunk_size: Option<usize>,
}

impl TestProtocolHeader {
    pub fn new(test_type: TestProtocolType, data_size: u64) -> Self {
        Self {
            test_type,
            data_size,
            parallel_streams: None,
            chunk_size: None,
        }
    }

    pub fn with_config(
        test_type: TestProtocolType,
        data_size: u64,
        parallel_streams: usize,
        chunk_size: usize,
    ) -> Self {
        Self {
            test_type,
            data_size,
            parallel_streams: Some(parallel_streams),
            chunk_size: Some(chunk_size),
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let header_bytes =
            postcard::to_allocvec(self).expect("failed to serialize protocol header");
        let header_len = header_bytes.len() as u16;

        let mut result = Vec::with_capacity(2 + header_bytes.len());
        result.extend_from_slice(&header_len.to_le_bytes());
        result.extend_from_slice(&header_bytes);
        result
    }

    pub async fn read_from(recv: &mut iroh::endpoint::RecvStream) -> Result<Self> {
        let mut len_buf = [0u8; 2];
        recv.read_exact(&mut len_buf)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read header length: {}", e))?;
        let header_len = u16::from_le_bytes(len_buf) as usize;

        let mut header_buf = vec![0u8; header_len];
        recv.read_exact(&mut header_buf)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read header: {}", e))?;

        let header: Self = postcard::from_bytes(&header_buf)
            .map_err(|e| anyhow::anyhow!("Failed to parse header with postcard: {}", e))?;

        Ok(header)
    }
}
