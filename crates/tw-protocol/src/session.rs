//! Handshake, session keys, sequence enforcement and request correlation.

use crate::messages::{AgentInfo, Message, Request, Response};
use crate::wire::{self, Direction, RawFrame, MAC_LEN};
use crate::{ProtocolError, MAX_FRAME_TO_GUEST, MAX_FRAME_TO_HOST, PROTOCOL_NAME, PROTOCOL_VERSION};
use rand::RngCore;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Host,
    Guest,
}

impl Role {
    fn send_dir(self) -> Direction {
        match self {
            Role::Host => Direction::HostToGuest,
            Role::Guest => Direction::GuestToHost,
        }
    }
    fn recv_dir(self) -> Direction {
        match self {
            Role::Host => Direction::GuestToHost,
            Role::Guest => Direction::HostToGuest,
        }
    }
    fn recv_limit(self) -> usize {
        match self {
            Role::Host => MAX_FRAME_TO_HOST,
            Role::Guest => MAX_FRAME_TO_GUEST,
        }
    }
}

/// An authenticated connection over any ordered byte stream (virtio-serial,
/// hvsocket, a Unix socket or an in-memory duplex in tests).
pub struct Connection<S> {
    stream: S,
    role: Role,
    session_key: [u8; 32],
    send_seq: u64,
    recv_seq: u64,
    next_request_id: u64,
    peer: Option<AgentInfo>,
}

fn nonce() -> [u8; 32] {
    let mut n = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut n);
    n
}

fn decode_nonce(hex_str: &str) -> Result<[u8; 32], ProtocolError> {
    let bytes = hex::decode(hex_str).map_err(|_| ProtocolError::Malformed("nonce is not hex".into()))?;
    bytes.try_into().map_err(|_| ProtocolError::Malformed("nonce must be 32 bytes".into()))
}

fn session_key(key: &[u8], host_nonce: &[u8; 32], guest_nonce: &[u8; 32]) -> [u8; 32] {
    wire::mac(key, &[b"tw-v1-session", host_nonce, guest_nonce])
}

async fn send_plain<S: AsyncWrite + Unpin>(stream: &mut S, msg: &Message) -> Result<(), ProtocolError> {
    let payload = serde_json::to_vec(msg).map_err(|e| ProtocolError::Malformed(e.to_string()))?;
    wire::write_frame(stream, &RawFrame { seq: 0, mac: [0; MAC_LEN], payload }).await
}

async fn recv_plain<S: AsyncRead + Unpin>(stream: &mut S, limit: usize) -> Result<Message, ProtocolError> {
    // Handshake messages are small regardless of direction.
    let frame = wire::read_frame(stream, limit.min(16 * 1024)).await?;
    if frame.seq != 0 {
        return Err(ProtocolError::Replay { expected: 0, got: frame.seq });
    }
    serde_json::from_slice(&frame.payload).map_err(|e| ProtocolError::Malformed(e.to_string()))
}

async fn within<T>(timeout: Duration, f: impl std::future::Future<Output = Result<T, ProtocolError>>) -> Result<T, ProtocolError> {
    tokio::time::timeout(timeout, f).await.map_err(|_| ProtocolError::Timeout)?
}

impl<S: AsyncRead + AsyncWrite + Unpin> Connection<S> {
    /// Host side of the handshake. `key` is the per-VM agent key.
    pub async fn connect_host(mut stream: S, key: &[u8], timeout: Duration) -> Result<Self, ProtocolError> {
        within(timeout, async move {
            let host_nonce = nonce();
            send_plain(
                &mut stream,
                &Message::Hello { protocol: PROTOCOL_NAME.into(), versions: vec![PROTOCOL_VERSION], nonce: hex::encode(host_nonce) },
            )
            .await?;
            let (guest_nonce, agent) = match recv_plain(&mut stream, MAX_FRAME_TO_HOST).await? {
                Message::HelloAck { version, nonce, proof, agent } => {
                    if version != PROTOCOL_VERSION {
                        return Err(ProtocolError::VersionMismatch { offered: vec![version] });
                    }
                    let guest_nonce = decode_nonce(&nonce)?;
                    let proof = hex::decode(proof).map_err(|_| ProtocolError::AuthenticationFailed)?;
                    if !wire::verify(key, &[b"tw-v1-ack", &host_nonce, &guest_nonce], &proof) {
                        return Err(ProtocolError::AuthenticationFailed);
                    }
                    (guest_nonce, agent)
                }
                Message::Goodbye { reason } => return Err(ProtocolError::Malformed(format!("guest refused: {reason}"))),
                other => return Err(ProtocolError::Malformed(format!("expected hello_ack, got {other:?}"))),
            };
            let ready = wire::mac(key, &[b"tw-v1-ready", &guest_nonce, &host_nonce]);
            send_plain(&mut stream, &Message::Ready { proof: hex::encode(ready) }).await?;
            Ok(Self {
                stream,
                role: Role::Host,
                session_key: session_key(key, &host_nonce, &guest_nonce),
                send_seq: 0,
                recv_seq: 0,
                next_request_id: 1,
                peer: Some(agent),
            })
        })
        .await
    }

    /// Guest side of the handshake.
    pub async fn accept_guest(mut stream: S, key: &[u8], agent: AgentInfo, timeout: Duration) -> Result<Self, ProtocolError> {
        within(timeout, async move {
            let host_nonce = match recv_plain(&mut stream, MAX_FRAME_TO_GUEST).await? {
                Message::Hello { protocol, versions, nonce } => {
                    if protocol != PROTOCOL_NAME {
                        return Err(ProtocolError::Malformed(format!("unknown protocol {protocol:?}")));
                    }
                    if !versions.contains(&PROTOCOL_VERSION) {
                        let _ = send_plain(&mut stream, &Message::Goodbye { reason: "no common protocol version".into() }).await;
                        return Err(ProtocolError::VersionMismatch { offered: versions });
                    }
                    decode_nonce(&nonce)?
                }
                other => return Err(ProtocolError::Malformed(format!("expected hello, got {other:?}"))),
            };
            let guest_nonce = nonce();
            let proof = wire::mac(key, &[b"tw-v1-ack", &host_nonce, &guest_nonce]);
            send_plain(
                &mut stream,
                &Message::HelloAck { version: PROTOCOL_VERSION, nonce: hex::encode(guest_nonce), proof: hex::encode(proof), agent },
            )
            .await?;
            match recv_plain(&mut stream, MAX_FRAME_TO_GUEST).await? {
                Message::Ready { proof } => {
                    let proof = hex::decode(proof).map_err(|_| ProtocolError::AuthenticationFailed)?;
                    if !wire::verify(key, &[b"tw-v1-ready", &guest_nonce, &host_nonce], &proof) {
                        return Err(ProtocolError::AuthenticationFailed);
                    }
                }
                other => return Err(ProtocolError::Malformed(format!("expected ready, got {other:?}"))),
            }
            Ok(Self {
                stream,
                role: Role::Guest,
                session_key: session_key(key, &host_nonce, &guest_nonce),
                send_seq: 0,
                recv_seq: 0,
                next_request_id: 1,
                peer: None,
            })
        })
        .await
    }

    pub fn peer(&self) -> Option<&AgentInfo> {
        self.peer.as_ref()
    }

    pub async fn send(&mut self, msg: &Message) -> Result<(), ProtocolError> {
        let payload = serde_json::to_vec(msg).map_err(|e| ProtocolError::Malformed(e.to_string()))?;
        self.send_seq += 1;
        let mac = wire::frame_mac(&self.session_key, self.role.send_dir(), self.send_seq, &payload);
        wire::write_frame(&mut self.stream, &RawFrame { seq: self.send_seq, mac, payload }).await
    }

    /// Receives one authenticated message. Any MAC or sequence failure is
    /// fatal for the connection; the caller must reconnect.
    pub async fn recv(&mut self) -> Result<Message, ProtocolError> {
        let frame = wire::read_frame(&mut self.stream, self.role.recv_limit()).await?;
        let expected = self.recv_seq + 1;
        // Check the MAC first so an attacker cannot probe sequence state.
        if !wire::verify(&self.session_key, &[&[self.role.recv_dir() as u8], &frame.seq.to_be_bytes(), &frame.payload], &frame.mac) {
            return Err(ProtocolError::BadMac);
        }
        if frame.seq != expected {
            return Err(ProtocolError::Replay { expected, got: frame.seq });
        }
        self.recv_seq = expected;
        serde_json::from_slice(&frame.payload).map_err(|e| ProtocolError::Malformed(e.to_string()))
    }

    /// Host: sends a request and waits for its response. Events that arrive
    /// meanwhile are returned through `on_event`. One request in flight at a time.
    pub async fn call(
        &mut self,
        action: crate::messages::Action,
        timeout: Duration,
        mut on_event: impl FnMut(crate::messages::Event),
    ) -> Result<Response, ProtocolError> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        let timeout_ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        self.send(&Message::Request(Request { id, timeout_ms, action })).await?;
        within(timeout + Duration::from_millis(250), async {
            loop {
                match self.recv().await? {
                    Message::Response(r) if r.id == id => return Ok(r),
                    Message::Response(r) => return Err(ProtocolError::Malformed(format!("response for unknown request {}", r.id))),
                    Message::Event(e) => on_event(e),
                    Message::Goodbye { reason } => return Err(ProtocolError::Io(format!("guest closed the session: {reason}"))),
                    other => return Err(ProtocolError::Malformed(format!("unexpected message {other:?}"))),
                }
            }
        })
        .await
    }

    pub fn into_inner(self) -> S {
        self.stream
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::*;

    fn agent() -> AgentInfo {
        AgentInfo { version: "0.1.0".into(), os: "windows".into(), arch: "aarch64".into(), capabilities: vec!["type".into()] }
    }
    const T: Duration = Duration::from_secs(2);

    #[tokio::test]
    async fn handshake_and_request_response() {
        let (h, g) = tokio::io::duplex(1 << 16);
        let key = [42u8; 32];
        let guest = tokio::spawn(async move {
            let mut conn = Connection::accept_guest(g, &key, agent(), T).await.unwrap();
            let Message::Request(req) = conn.recv().await.unwrap() else { panic!() };
            assert_eq!(req.action, Action::Type { text: "안녕 Windows".into() });
            conn.send(&Message::Event(Event::SessionChanged { session: SessionState::Active })).await.unwrap();
            conn.send(&Message::Response(Response { id: req.id, outcome: Outcome::Ok { result: ActionResult::Done } })).await.unwrap();
        });
        let mut host = Connection::connect_host(h, &key, T).await.unwrap();
        assert_eq!(host.peer().unwrap().arch, "aarch64");
        let mut events = Vec::new();
        let resp = host.call(Action::Type { text: "안녕 Windows".into() }, T, |e| events.push(e)).await.unwrap();
        assert_eq!(resp.outcome, Outcome::Ok { result: ActionResult::Done });
        assert_eq!(events, vec![Event::SessionChanged { session: SessionState::Active }]);
        guest.await.unwrap();
    }

    #[tokio::test]
    async fn wrong_key_fails_on_both_sides() {
        let (h, g) = tokio::io::duplex(1 << 16);
        let guest = tokio::spawn(async move { Connection::accept_guest(g, &[1u8; 32], agent(), T).await.err() });
        let host = Connection::connect_host(h, &[2u8; 32], T).await;
        assert_eq!(host.err(), Some(ProtocolError::AuthenticationFailed));
        // The guest never receives a valid Ready.
        assert!(guest.await.unwrap().is_some());
    }

    #[tokio::test]
    async fn guest_that_never_answers_times_out() {
        let (h, _g) = tokio::io::duplex(1 << 16);
        let err = Connection::connect_host(h, &[0u8; 32], Duration::from_millis(50)).await.err();
        assert_eq!(err, Some(ProtocolError::Timeout));
    }

    #[tokio::test]
    async fn version_mismatch_is_reported() {
        let (mut h, g) = tokio::io::duplex(1 << 16);
        let guest = tokio::spawn(async move { Connection::accept_guest(g, &[0u8; 32], agent(), T).await.err() });
        send_plain(&mut h, &Message::Hello { protocol: PROTOCOL_NAME.into(), versions: vec![99], nonce: hex::encode([0u8; 32]) })
            .await
            .unwrap();
        assert_eq!(guest.await.unwrap(), Some(ProtocolError::VersionMismatch { offered: vec![99] }));
        assert!(matches!(recv_plain(&mut h, 1024).await.unwrap(), Message::Goodbye { .. }));
    }

    async fn pair(key: [u8; 32]) -> (Connection<tokio::io::DuplexStream>, Connection<tokio::io::DuplexStream>) {
        let (h, g) = tokio::io::duplex(1 << 16);
        let guest = tokio::spawn(async move { Connection::accept_guest(g, &key, agent(), T).await.unwrap() });
        let host = Connection::connect_host(h, &key, T).await.unwrap();
        (host, guest.await.unwrap())
    }

    #[tokio::test]
    async fn tampered_frames_are_rejected() {
        let (mut host, guest) = pair([5u8; 32]).await;
        let mut raw = guest.into_inner();
        let forged = RawFrame { seq: 1, mac: [0; MAC_LEN], payload: br#"{"type":"goodbye","reason":"x"}"#.to_vec() };
        wire::write_frame(&mut raw, &forged).await.unwrap();
        assert_eq!(host.recv().await.unwrap_err(), ProtocolError::BadMac);
    }

    #[tokio::test]
    async fn replayed_frames_are_rejected() {
        let (mut host, guest) = pair([9u8; 32]).await;
        let payload = br#"{"type":"goodbye","reason":"once"}"#.to_vec();
        let mac = wire::frame_mac(&guest.session_key, Direction::GuestToHost, 1, &payload);
        let frame = RawFrame { seq: 1, mac, payload };
        let mut raw = guest.into_inner();
        wire::write_frame(&mut raw, &frame).await.unwrap();
        wire::write_frame(&mut raw, &frame).await.unwrap();
        assert!(matches!(host.recv().await.unwrap(), Message::Goodbye { .. }));
        assert_eq!(host.recv().await.unwrap_err(), ProtocolError::Replay { expected: 2, got: 1 });
    }

    #[tokio::test]
    async fn reflected_frames_are_rejected() {
        // A host frame echoed back to the host has the wrong direction in its MAC.
        let key = [3u8; 32];
        let (h, g) = tokio::io::duplex(1 << 16);
        let guest = tokio::spawn(async move { Connection::accept_guest(g, &key, agent(), T).await.unwrap() });
        let mut host = Connection::connect_host(h, &key, T).await.unwrap();
        let guest = guest.await.unwrap();
        let mut gs = guest.into_inner();
        host.send(&Message::Goodbye { reason: "bye".into() }).await.unwrap();
        let echoed = wire::read_frame(&mut gs, 1 << 16).await.unwrap();
        wire::write_frame(&mut gs, &echoed).await.unwrap();
        assert_eq!(host.recv().await.unwrap_err(), ProtocolError::BadMac);
    }

    #[tokio::test]
    async fn oversized_request_to_guest_is_rejected_before_allocation() {
        let key = [4u8; 32];
        let (h, g) = tokio::io::duplex(1 << 20);
        let guest = tokio::spawn(async move {
            let mut conn = Connection::accept_guest(g, &key, agent(), T).await.unwrap();
            conn.recv().await
        });
        let mut host = Connection::connect_host(h, &key, T).await.unwrap();
        let text = "x".repeat(MAX_FRAME_TO_GUEST + 1);
        host.send(&Message::Request(Request { id: 1, timeout_ms: 1, action: Action::Type { text } })).await.unwrap();
        assert!(matches!(guest.await.unwrap(), Err(ProtocolError::FrameTooLarge { .. })));
    }
}
