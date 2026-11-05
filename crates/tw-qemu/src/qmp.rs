//! Asynchronous QMP (QEMU Machine Protocol) client.
//!
//! QMP is line-delimited JSON. After the greeting the client must send
//! `qmp_capabilities`. Asynchronous events can arrive between a command and
//! its reply; they are kept in a bounded buffer.

use serde_json::{json, Value};
use std::collections::VecDeque;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tw_core::{ErrorCode, TwError, TwResult};

const MAX_LINE: usize = 1024 * 1024;
const MAX_EVENTS: usize = 64;

pub struct Qmp<S> {
    reader: BufReader<ReadHalf<S>>,
    writer: WriteHalf<S>,
    next_id: u64,
    events: VecDeque<Value>,
    timeout: Duration,
}

fn proto(msg: impl Into<String>) -> TwError {
    TwError::new(ErrorCode::Protocol, msg)
}

impl<S: AsyncRead + AsyncWrite> Qmp<S> {
    /// Reads the greeting and negotiates capabilities.
    pub async fn handshake(stream: S, timeout: Duration) -> TwResult<Self> {
        let (r, w) = tokio::io::split(stream);
        let mut qmp = Self { reader: BufReader::new(r), writer: w, next_id: 1, events: VecDeque::new(), timeout };
        let greeting = qmp.read_message().await?;
        if greeting.get("QMP").is_none() {
            return Err(proto(format!("unexpected QMP greeting: {greeting}")));
        }
        qmp.execute("qmp_capabilities", None).await?;
        Ok(qmp)
    }

    async fn read_message(&mut self) -> TwResult<Value> {
        let mut line = String::new();
        let fut = async {
            loop {
                line.clear();
                let n = (&mut self.reader).take(MAX_LINE as u64).read_line(&mut line).await.map_err(|e| TwError::io("QMP read", e))?;
                if n == 0 {
                    return Err(proto("QMP connection closed"));
                }
                if !line.ends_with('\n') && n >= MAX_LINE {
                    return Err(proto("QMP message too large"));
                }
                if !line.trim().is_empty() {
                    return serde_json::from_str::<Value>(&line).map_err(|e| proto(format!("invalid QMP JSON: {e}")));
                }
            }
        };
        tokio::time::timeout(self.timeout, fut).await.map_err(|_| TwError::new(ErrorCode::Timeout, "QMP timed out"))?
    }

    /// Executes a command and returns its `return` value.
    pub async fn execute(&mut self, command: &str, arguments: Option<Value>) -> TwResult<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = json!({ "execute": command, "id": id });
        if let Some(args) = arguments {
            msg["arguments"] = args;
        }
        let mut bytes = serde_json::to_vec(&msg).map_err(|e| proto(e.to_string()))?;
        bytes.push(b'\n');
        self.writer.write_all(&bytes).await.map_err(|e| TwError::io("QMP write", e))?;
        self.writer.flush().await.map_err(|e| TwError::io("QMP write", e))?;
        loop {
            let reply = self.read_message().await?;
            if reply.get("event").is_some() {
                if self.events.len() == MAX_EVENTS {
                    self.events.pop_front();
                }
                self.events.push_back(reply);
                continue;
            }
            if reply.get("id").and_then(Value::as_u64) != Some(id) {
                return Err(proto(format!("QMP reply for another request: {reply}")));
            }
            if let Some(err) = reply.get("error") {
                let class = err.get("class").and_then(Value::as_str).unwrap_or("GenericError");
                let desc = err.get("desc").and_then(Value::as_str).unwrap_or("unknown error");
                return Err(TwError::backend(format!("QEMU {command} failed ({class}): {desc}")));
            }
            return Ok(reply.get("return").cloned().unwrap_or(Value::Null));
        }
    }

    pub fn drain_events(&mut self) -> Vec<Value> {
        self.events.drain(..).collect()
    }

    /// `query-status` → (status string, running flag).
    pub async fn query_status(&mut self) -> TwResult<(String, bool)> {
        let r = self.execute("query-status", None).await?;
        let status = r.get("status").and_then(Value::as_str).unwrap_or("unknown").to_string();
        let running = r.get("running").and_then(Value::as_bool).unwrap_or(false);
        Ok((status, running))
    }

    pub async fn send_input(&mut self, events: Vec<Value>) -> TwResult<()> {
        self.execute("input-send-event", Some(json!({ "events": events }))).await.map(|_| ())
    }
}

/// Input event builders for `input-send-event`.
pub mod input {
    use serde_json::{json, Value};

    pub fn abs(axis: &str, value: u32) -> Value {
        json!({ "type": "abs", "data": { "axis": axis, "value": value } })
    }
    pub fn btn(button: &str, down: bool) -> Value {
        json!({ "type": "btn", "data": { "button": button, "down": down } })
    }
    pub fn key(qcode: &str, down: bool) -> Value {
        json!({ "type": "key", "data": { "down": down, "key": { "type": "qcode", "data": qcode } } })
    }
}

#[cfg(unix)]
pub async fn connect_unix(path: &std::path::Path, timeout: Duration) -> TwResult<Qmp<tokio::net::UnixStream>> {
    let stream = tokio::time::timeout(timeout, tokio::net::UnixStream::connect(path))
        .await
        .map_err(|_| TwError::new(ErrorCode::Timeout, "QMP connect timed out"))?
        .map_err(|e| TwError::io(format!("connect {}", path.display()), e))?;
    Qmp::handshake(stream, timeout).await
}

#[cfg(windows)]
pub async fn connect_pipe(name: &str, timeout: Duration) -> TwResult<Qmp<tokio::net::windows::named_pipe::NamedPipeClient>> {
    let full = format!(r"\\.\pipe\{name}");
    let client = tokio::net::windows::named_pipe::ClientOptions::new().open(&full).map_err(|e| TwError::io(&full, e))?;
    Qmp::handshake(client, timeout).await
}

#[cfg(test)]
pub(crate) mod mock {
    //! A scripted fake QEMU monitor for tests.
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

    pub type Log = Arc<Mutex<Vec<Value>>>;

    /// Serves QMP on `stream`. `respond` maps a command to its `return` value or an error message.
    pub fn serve(stream: DuplexStream, respond: impl Fn(&str, &Value) -> Result<Value, String> + Send + 'static) -> Log {
        let log: Log = Arc::default();
        let log2 = log.clone();
        tokio::spawn(async move {
            let (r, mut w) = tokio::io::split(stream);
            let mut lines = BufReader::new(r).lines();
            w.write_all(b"{\"QMP\": {\"version\": {\"qemu\": {\"major\": 9}}, \"capabilities\": []}}\n").await.unwrap();
            while let Ok(Some(line)) = lines.next_line().await {
                let msg: Value = serde_json::from_str(&line).unwrap();
                let cmd = msg["execute"].as_str().unwrap_or("").to_string();
                log2.lock().unwrap().push(msg.clone());
                // Emit an unrelated event before every reply to exercise buffering.
                w.write_all(b"{\"event\": \"RTC_CHANGE\", \"data\": {}}\n").await.unwrap();
                let reply = match respond(&cmd, &msg["arguments"]) {
                    Ok(ret) => json!({ "return": ret, "id": msg["id"] }),
                    Err(desc) => json!({ "error": { "class": "GenericError", "desc": desc }, "id": msg["id"] }),
                };
                w.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
            }
        });
        log
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn handshake_execute_events_and_errors() {
        let (client, server) = tokio::io::duplex(1 << 16);
        let log = mock::serve(server, |cmd, _| match cmd {
            "query-status" => Ok(json!({ "status": "paused", "running": false })),
            "bogus" => Err("The command bogus has not been found".into()),
            _ => Ok(json!({})),
        });
        let mut qmp = Qmp::handshake(client, Duration::from_secs(2)).await.unwrap();
        assert_eq!(qmp.query_status().await.unwrap(), ("paused".into(), false));
        let err = qmp.execute("bogus", None).await.unwrap_err();
        assert!(err.message.contains("has not been found"));
        assert_eq!(qmp.drain_events().len(), 3);
        qmp.send_input(vec![input::key("ctrl", true)]).await.unwrap();
        let log = log.lock().unwrap();
        assert_eq!(log[0]["execute"], "qmp_capabilities");
        assert_eq!(log[3]["arguments"]["events"][0]["data"]["key"]["data"], "ctrl");
    }

    #[tokio::test]
    async fn silent_server_times_out() {
        let (client, _server) = tokio::io::duplex(64);
        let err = Qmp::handshake(client, Duration::from_millis(50)).await.err().unwrap();
        assert_eq!(err.code, ErrorCode::Timeout);
    }
}
