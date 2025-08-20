//! Protocol definitions for test communication

use std::fmt;

use anyhow::Result;
use postcard;
use serde::{Deserialize, Serialize};

/// ALPN protocol identifier for doctor swarm tests
pub const DOCTOR_SWARM_ALPN: &[u8] = b"doctor-swarm";

/// Test protocol types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestProtocolType {
    Throughput,
    Latency,
    Connectivity,
}

impl TestProtocolType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Throughput => "THROUGHPUT",
            Self::Latency => "LATENCY",
            Self::Connectivity => "CONNECTIVITY",
        }
    }
}

impl std::str::FromStr for TestProtocolType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "THROUGHPUT" => Ok(Self::Throughput),
            "LATENCY" => Ok(Self::Latency),
            "CONNECTIVITY" => Ok(Self::Connectivity),
            _ => Err(()),
        }
    }
}

impl fmt::Display for TestProtocolType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Protocol messages for latency testing
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LatencyMessage {
    Ping(u32),
    Pong(u32),
}

impl LatencyMessage {
    pub fn ping(number: u32) -> String {
        format!("PING {number}")
    }

    pub fn pong_from_ping(ping_str: &str) -> Option<String> {
        if ping_str.starts_with("PING") {
            Some(ping_str.replace("PING", "PONG"))
        } else {
            None
        }
    }

    pub fn is_pong_response(msg: &str) -> bool {
        msg.to_uppercase().contains("PONG")
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
        let header_bytes = postcard::to_allocvec(self).unwrap_or_else(|_| {
            let mut bytes = Vec::new();
            bytes.push(self.test_type as u8);
            bytes.extend_from_slice(&self.data_size.to_le_bytes());
            bytes.extend_from_slice(&self.parallel_streams.unwrap_or(0).to_le_bytes());
            bytes.extend_from_slice(&self.chunk_size.unwrap_or(0).to_le_bytes());
            bytes
        });
        let header_len = header_bytes.len() as u16;

        let mut result = Vec::new();
        result.extend_from_slice(&header_len.to_le_bytes());
        result.extend_from_slice(&header_bytes);
        result
    }

    pub async fn read_from(recv: &mut iroh::endpoint::RecvStream) -> Result<Self> {
        // Read header length (2 bytes)
        let mut len_buf = [0u8; 2];
        recv.read_exact(&mut len_buf)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read header length: {}", e))?;
        let header_len = u16::from_le_bytes(len_buf) as usize;

        // Read header
        let mut header_buf = vec![0u8; header_len];
        recv.read_exact(&mut header_buf)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read header: {}", e))?;

        // Parse binary header with postcard
        let header: Self = postcard::from_bytes(&header_buf)
            .map_err(|e| anyhow::anyhow!("Failed to parse header with postcard: {}", e))?;

        Ok(header)
    }
}
