//! Wire types for the iroh-doctor connection test (`n0/doctor/1`).
//!
//! The CLI's `connect`/`accept` commands and the app's responder both speak
//! this protocol. Only the wire types live here; each binary keeps its own
//! handler, because the CLI's is woven into its progress UI and the app's is
//! a timeout-bounded responder.

use postcard::experimental::max_size::MaxSize;
use serde::{Deserialize, Serialize};

/// ALPN for the iroh-doctor connection test. Must stay byte-stable for
/// interop with deployed `iroh-doctor` binaries.
pub const ALPN: &[u8] = b"n0/doctor/1";

/// A single test the active side requests on a freshly opened stream.
#[derive(Debug, Serialize, Deserialize, MaxSize)]
pub enum TestStreamRequest {
    /// Echo the bytes the active side sends back to it.
    Echo { bytes: u64 },
    /// Discard the bytes the active side sends.
    Drain { bytes: u64 },
    /// Send `bytes` bytes to the active side in `block_size` chunks.
    Send { bytes: u64, block_size: u32 },
}
