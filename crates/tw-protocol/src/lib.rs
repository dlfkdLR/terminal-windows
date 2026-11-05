//! terminal-windows guest protocol, version 1. See docs/protocol.md.
//!
//! Wire frame (all integers big-endian):
//!
//! ```text
//! u32 length of the rest | u64 seq | [u8; 32] HMAC-SHA256 | JSON payload
//! ```
//!
//! The MAC covers `direction || seq || payload` with the session key, so
//! frames cannot be replayed, reordered, reflected back to the sender or moved
//! between sessions. Handshake frames use `seq = 0` and an all-zero MAC; their
//! authenticity comes from the proofs inside the handshake messages.

pub mod messages;
pub mod session;
pub mod wire;

pub use messages::*;
pub use session::{Connection, Role};

pub const PROTOCOL_NAME: &str = "terminal-windows";
pub const PROTOCOL_VERSION: u32 = 1;
/// Largest frame the host accepts (screenshots).
pub const MAX_FRAME_TO_HOST: usize = 16 * 1024 * 1024;
/// Largest frame the guest accepts (requests carry at most 32 KiB of text).
pub const MAX_FRAME_TO_GUEST: usize = 128 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    Io(String),
    FrameTooLarge {
        size: usize,
        limit: usize,
    },
    Malformed(String),
    BadMac,
    /// Sequence number was not exactly the expected next value.
    Replay {
        expected: u64,
        got: u64,
    },
    VersionMismatch {
        offered: Vec<u32>,
    },
    AuthenticationFailed,
    Timeout,
    Closed,
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::Io(e) => write!(f, "transport error: {e}"),
            ProtocolError::FrameTooLarge { size, limit } => write!(f, "frame of {size} bytes exceeds the {limit}-byte limit"),
            ProtocolError::Malformed(e) => write!(f, "malformed message: {e}"),
            ProtocolError::BadMac => write!(f, "message authentication failed"),
            ProtocolError::Replay { expected, got } => write!(f, "unexpected sequence number {got} (expected {expected})"),
            ProtocolError::VersionMismatch { offered } => {
                write!(f, "no common protocol version (peer offered {offered:?}, we speak {PROTOCOL_VERSION})")
            }
            ProtocolError::AuthenticationFailed => write!(f, "peer failed to prove knowledge of this VM's agent key"),
            ProtocolError::Timeout => write!(f, "timed out"),
            ProtocolError::Closed => write!(f, "connection closed"),
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<std::io::Error> for ProtocolError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            ProtocolError::Closed
        } else {
            ProtocolError::Io(e.to_string())
        }
    }
}
