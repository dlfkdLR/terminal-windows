//! tw-agent entry point. Runs only inside a Windows guest with the
//! terminal-windows virtio-serial port; it refuses to run anywhere else.

#[cfg(not(windows))]
fn main() {
    eprintln!("tw-agent runs only inside a Windows virtual machine created by terminal-windows. It never controls a host desktop.");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
    if let Err(e) = runtime.block_on(agent::run()) {
        eprintln!("tw-agent: {e}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod agent {
    use std::path::PathBuf;
    use std::time::Duration;
    use tw_agent::{bridge::bridge, dispatch, win32::Win32Desktop, PORT_PATH};
    use tw_protocol::{AgentInfo, Connection, Message, Outcome, Response};

    fn key_path() -> PathBuf {
        if let Some(p) = std::env::args().skip_while(|a| a != "--key-file").nth(1) {
            return PathBuf::from(p);
        }
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        base.join("terminal-windows-agent").join("agent.key")
    }

    fn load_key() -> Result<Vec<u8>, String> {
        let path = key_path();
        let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let text = text.trim();
        if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("{} must contain a 64-character hex key", path.display()));
        }
        Ok((0..32).map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap()).collect())
    }

    pub async fn run() -> Result<(), String> {
        let key = load_key()?;
        let mut backoff = Duration::from_millis(500);
        loop {
            match session(&key).await {
                Ok(()) => backoff = Duration::from_millis(500),
                Err(e) => eprintln!("tw-agent: session ended: {e}"),
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(10));
        }
    }

    async fn session(key: &[u8]) -> Result<(), String> {
        let device = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(PORT_PATH)
            .map_err(|e| format!("cannot open {PORT_PATH} (is this a terminal-windows VM with virtio-serial drivers?): {e}"))?;
        let writer = device.try_clone().map_err(|e| e.to_string())?;
        let stream = bridge(device, writer);
        let info = AgentInfo {
            version: env!("CARGO_PKG_VERSION").into(),
            os: "windows".into(),
            arch: std::env::consts::ARCH.into(),
            capabilities: vec!["status".into(), "move".into(), "click".into(), "drag".into(), "scroll".into(), "type".into(), "key".into()],
        };
        // The host connects whenever it needs the agent; wait for it indefinitely.
        let mut conn = Connection::accept_guest(stream, key, info, Duration::from_secs(24 * 3600)).await.map_err(|e| e.to_string())?;
        let mut desktop = Win32Desktop::new();
        loop {
            match conn.recv().await.map_err(|e| e.to_string())? {
                Message::Request(req) => {
                    let outcome: Outcome = dispatch::handle(&mut desktop, &req.action);
                    conn.send(&Message::Response(Response { id: req.id, outcome })).await.map_err(|e| e.to_string())?;
                }
                Message::Goodbye { .. } => return Ok(()),
                other => return Err(format!("unexpected message: {other:?}")),
            }
        }
    }
}
