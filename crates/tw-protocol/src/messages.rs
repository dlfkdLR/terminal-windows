//! Typed protocol messages.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    /// Host → guest, first frame.
    Hello {
        protocol: String,
        versions: Vec<u32>,
        nonce: String,
    },
    /// Guest → host. `proof = HMAC(key, "tw-v1-ack" || host_nonce || guest_nonce)`.
    HelloAck {
        version: u32,
        nonce: String,
        proof: String,
        agent: AgentInfo,
    },
    /// Host → guest. `proof = HMAC(key, "tw-v1-ready" || guest_nonce || host_nonce)`.
    Ready {
        proof: String,
    },
    Request(Request),
    Response(Response),
    Event(Event),
    /// Either side, before closing.
    Goodbye {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentInfo {
    pub version: String,
    pub os: String,
    pub arch: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    /// Relative timeout. Absolute deadlines are not used because guest and
    /// host clocks diverge after boot, pause and snapshot restore.
    pub timeout_ms: u32,
    #[serde(flatten)]
    pub action: Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Button {
    #[default]
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Status,
    Screenshot {
        max_width: u32,
        max_height: u32,
    },
    Move {
        x: u32,
        y: u32,
    },
    Click {
        x: u32,
        y: u32,
        #[serde(default)]
        button: Button,
        #[serde(default = "one")]
        count: u8,
    },
    Drag {
        x: u32,
        y: u32,
        to_x: u32,
        to_y: u32,
    },
    Scroll {
        x: u32,
        y: u32,
        #[serde(default)]
        dx: i32,
        dy: i32,
    },
    Type {
        text: String,
    },
    Key {
        chord: String,
    },
    UiTree {
        max_depth: u8,
        max_nodes: u32,
    },
    /// Ask Windows to shut down gracefully (the user may be prompted).
    Shutdown,
}

fn one() -> u8 {
    1
}

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Action::Status => "status",
            Action::Screenshot { .. } => "screenshot",
            Action::Move { .. } => "move",
            Action::Click { .. } => "click",
            Action::Drag { .. } => "drag",
            Action::Scroll { .. } => "scroll",
            Action::Type { .. } => "type",
            Action::Key { .. } => "key",
            Action::UiTree { .. } => "ui_tree",
            Action::Shutdown => "shutdown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(flatten)]
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Ok { result: ActionResult },
    Error { code: GuestErrorCode, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestErrorCode {
    InvalidArgument,
    Unsupported,
    /// The interactive desktop is the secure desktop (UAC, Ctrl+Alt+Del) or the
    /// lock screen. Only the hypervisor console can reach it.
    SecureDesktop,
    /// UIPI blocked input to an elevated window.
    ElevatedTarget,
    NoInteractiveSession,
    Timeout,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Active,
    Locked,
    SecureDesktop,
    NoSession,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub width: u32,
    pub height: u32,
    /// Windows scale factor in percent (100, 125, 150, ...).
    pub scale_percent: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiNode {
    pub role: String,
    pub name: String,
    pub bounds: [i32; 4],
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<UiNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActionResult {
    Done,
    Status { session: SessionState, display: DisplayInfo, foreground: Option<String> },
    Screenshot { width: u32, height: u32, png_base64: String },
    UiTree { root: UiNode, truncated: bool },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    SessionChanged { session: SessionState },
    DisplayChanged { display: DisplayInfo },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_json_is_flat_and_readable() {
        let req = Request { id: 7, timeout_ms: 5000, action: Action::Click { x: 10, y: 20, button: Button::Right, count: 2 } };
        let json = serde_json::to_value(Message::Request(req.clone())).unwrap();
        assert_eq!(json["type"], "request");
        assert_eq!(json["action"], "click");
        assert_eq!(json["button"], "right");
        let back: Message = serde_json::from_value(json).unwrap();
        assert_eq!(back, Message::Request(req));
    }

    #[test]
    fn defaults_and_unknown_fields() {
        let m: Message = serde_json::from_str(r#"{"type":"request","id":1,"timeout_ms":10,"action":"click","x":1,"y":2}"#).unwrap();
        let Message::Request(r) = m else { panic!() };
        assert_eq!(r.action, Action::Click { x: 1, y: 2, button: Button::Left, count: 1 });
        assert!(serde_json::from_str::<Message>(r#"{"type":"request","id":1,"timeout_ms":10,"action":"exec","cmd":"calc"}"#).is_err());
    }

    #[test]
    fn error_outcome_round_trip() {
        let resp = Response { id: 3, outcome: Outcome::Error { code: GuestErrorCode::SecureDesktop, message: "UAC prompt".into() } };
        let text = serde_json::to_string(&Message::Response(resp.clone())).unwrap();
        assert!(text.contains("\"code\":\"secure_desktop\""));
        assert_eq!(serde_json::from_str::<Message>(&text).unwrap(), Message::Response(resp));
    }
}
