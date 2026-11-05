//! Frame encoding and MAC computation.

use crate::ProtocolError;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub type HmacSha256 = Hmac<Sha256>;
pub const MAC_LEN: usize = 32;
const HEADER_LEN: usize = 8 + MAC_LEN;

/// Direction byte mixed into every MAC so a frame cannot be reflected back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    HostToGuest = 1,
    GuestToHost = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFrame {
    pub seq: u64,
    pub mac: [u8; MAC_LEN],
    pub payload: Vec<u8>,
}

pub fn mac(key: &[u8], parts: &[&[u8]]) -> [u8; MAC_LEN] {
    let mut m = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    for part in parts {
        m.update(part);
    }
    m.finalize().into_bytes().into()
}

/// Constant-time MAC verification.
pub fn verify(key: &[u8], parts: &[&[u8]], tag: &[u8]) -> bool {
    let mut m = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    for part in parts {
        m.update(part);
    }
    m.verify_slice(tag).is_ok()
}

pub fn frame_mac(key: &[u8], dir: Direction, seq: u64, payload: &[u8]) -> [u8; MAC_LEN] {
    mac(key, &[&[dir as u8], &seq.to_be_bytes(), payload])
}

pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, frame: &RawFrame) -> Result<(), ProtocolError> {
    let len = HEADER_LEN + frame.payload.len();
    let len32 = u32::try_from(len).map_err(|_| ProtocolError::FrameTooLarge { size: len, limit: u32::MAX as usize })?;
    let mut buf = Vec::with_capacity(4 + len);
    buf.extend_from_slice(&len32.to_be_bytes());
    buf.extend_from_slice(&frame.seq.to_be_bytes());
    buf.extend_from_slice(&frame.mac);
    buf.extend_from_slice(&frame.payload);
    w.write_all(&buf).await?;
    w.flush().await?;
    Ok(())
}

/// Reads one frame, rejecting anything larger than `limit` bytes of payload
/// *before* allocating for it.
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R, limit: usize) -> Result<RawFrame, ProtocolError> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len < HEADER_LEN {
        return Err(ProtocolError::Malformed(format!("frame length {len} is shorter than the header")));
    }
    let payload_len = len - HEADER_LEN;
    if payload_len > limit {
        return Err(ProtocolError::FrameTooLarge { size: payload_len, limit });
    }
    let mut header = [0u8; HEADER_LEN];
    r.read_exact(&mut header).await?;
    let mut payload = vec![0u8; payload_len];
    r.read_exact(&mut payload).await?;
    let seq = u64::from_be_bytes(header[..8].try_into().expect("8 bytes"));
    let mut mac = [0u8; MAC_LEN];
    mac.copy_from_slice(&header[8..]);
    Ok(RawFrame { seq, mac, payload })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn round_trip_and_size_limit() {
        let (mut a, mut b) = tokio::io::duplex(1 << 20);
        let frame = RawFrame { seq: 9, mac: [7; MAC_LEN], payload: b"{}".to_vec() };
        write_frame(&mut a, &frame).await.unwrap();
        assert_eq!(read_frame(&mut b, 16).await.unwrap(), frame);

        let big = RawFrame { seq: 1, mac: [0; MAC_LEN], payload: vec![b'x'; 100] };
        write_frame(&mut a, &big).await.unwrap();
        assert_eq!(read_frame(&mut b, 99).await.unwrap_err(), ProtocolError::FrameTooLarge { size: 100, limit: 99 });
    }

    #[tokio::test]
    async fn short_length_is_malformed_and_eof_is_closed() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&3u32.to_be_bytes()).await.unwrap();
        assert!(matches!(read_frame(&mut b, 16).await, Err(ProtocolError::Malformed(_))));
        drop(a);
        assert_eq!(read_frame(&mut b, 16).await.unwrap_err(), ProtocolError::Closed);
    }

    #[test]
    fn mac_binds_direction_and_sequence() {
        let key = [1u8; 32];
        let m = frame_mac(&key, Direction::HostToGuest, 1, b"x");
        assert_ne!(m, frame_mac(&key, Direction::GuestToHost, 1, b"x"));
        assert_ne!(m, frame_mac(&key, Direction::HostToGuest, 2, b"x"));
        assert!(verify(&key, &[&[1], &1u64.to_be_bytes(), b"x"], &m));
        assert!(!verify(&[2u8; 32], &[&[1], &1u64.to_be_bytes(), b"x"], &m));
    }
}
