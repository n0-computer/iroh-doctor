//! Accept side of the iroh-doctor connection test, so
//! `iroh-doctor connect <endpoint>` works against an iroh-doctor-app endpoint.
//!
//! iroh-doctor's active side (`connect`) opens a connection on [`ALPN`]
//! and, per test, opens a bidi stream carrying a [`TestStreamRequest`]:
//! echo the upload back, drain it, or send a requested number of bytes.
//! This mirrors iroh-doctor's `passive_side` / `handle_test_request`
//! without its progress GUI. We register the ALPN in `bind_endpoint` and
//! the accept loop in `peer.rs` dispatches here.

use std::time::Duration;

use anyhow::{Context, Result};
use iroh::endpoint;
// The wire types now live in `iroh-doctor-core`. `ALPN` is re-exported so
// existing `crate::doctor::ALPN` references (e.g. in `peer.rs`) keep working.
pub use iroh_doctor_core::doctor::{TestStreamRequest, ALPN};
use postcard::experimental::max_size::MaxSize;
use tracing::warn;

/// Overall ceiling for one doctor connection so a peer that opens the
/// ALPN and then stalls cannot pin a handler forever. Generous because a
/// real test can transfer many MiB over a slow path.
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(120);

/// Block-size cap for `Send` so a peer cannot make us allocate an absurd
/// per-write buffer. Real tests use modest block sizes.
const MAX_BLOCK_SIZE: u32 = 1024 * 1024;

/// Serves the passive side of a doctor connection. Bounded by
/// [`CONNECTION_TIMEOUT`] so a misbehaving peer cannot pin the task.
pub async fn handle_connection(conn: endpoint::Connection) -> Result<()> {
    tokio::time::timeout(CONNECTION_TIMEOUT, handle_connection_inner(conn))
        .await
        .context("doctor handler exceeded connection timeout")?
}

async fn handle_connection_inner(conn: endpoint::Connection) -> Result<()> {
    loop {
        let (send, recv) = match conn.accept_bi().await {
            Ok(pair) => pair,
            // The active side closes the connection once its tests are
            // done; that surfaces here as an accept error and ends the
            // loop cleanly.
            Err(_) => break,
        };
        if let Err(e) = handle_test_request(send, recv).await {
            warn!(err = %e, "doctor: test request failed");
            break;
        }
    }
    Ok(())
}

async fn handle_test_request(
    mut send: endpoint::SendStream,
    mut recv: endpoint::RecvStream,
) -> Result<()> {
    let mut buf = [0u8; TestStreamRequest::POSTCARD_MAX_SIZE];
    recv.read_exact(&mut buf)
        .await
        .context("read test request")?;
    let request: TestStreamRequest = postcard::from_bytes(&buf).context("decode test request")?;
    match request {
        // Echo everything the peer uploads back to it until it finishes
        // its send stream. `bytes` is only a progress hint on the active
        // side, so the copy runs to EOF rather than a fixed length.
        TestStreamRequest::Echo { .. } => {
            tokio::io::copy(&mut recv, &mut send)
                .await
                .context("echo copy")?;
        }
        // Drain whatever the peer uploads.
        TestStreamRequest::Drain { .. } => {
            tokio::io::copy(&mut recv, &mut tokio::io::sink())
                .await
                .context("drain copy")?;
        }
        // Send the requested number of bytes in fixed-size blocks.
        TestStreamRequest::Send { bytes, block_size } => {
            let block_size = block_size.clamp(1, MAX_BLOCK_SIZE);
            send_blocks(&mut send, bytes, block_size).await?;
        }
    }
    send.finish().context("finish test stream")?;
    Ok(())
}

async fn send_blocks(
    send: &mut endpoint::SendStream,
    total_bytes: u64,
    block_size: u32,
) -> Result<()> {
    let buf = vec![0u8; block_size as usize];
    let mut remaining = total_bytes;
    while remaining > 0 {
        let n = remaining.min(block_size as u64);
        send.write_all(&buf[..n as usize])
            .await
            .context("send block")?;
        remaining -= n;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire enum must round-trip through postcard with the same
    /// shape iroh-doctor encodes, or `connect` requests won't decode.
    #[test]
    fn test_stream_request_roundtrips() {
        let cases = [
            TestStreamRequest::Echo { bytes: 0 },
            TestStreamRequest::Drain { bytes: u64::MAX },
            TestStreamRequest::Send {
                bytes: 1024,
                block_size: 64 * 1024,
            },
        ];
        let mut buf = [0u8; TestStreamRequest::POSTCARD_MAX_SIZE];
        for req in cases {
            let encoded = postcard::to_slice(&req, &mut buf).expect("encode");
            let decoded: TestStreamRequest = postcard::from_bytes(encoded).expect("decode");
            // TestStreamRequest has no PartialEq (mirrors iroh-doctor),
            // so compare via Debug.
            assert_eq!(format!("{req:?}"), format!("{decoded:?}"));
        }
    }
}
