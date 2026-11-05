use crate::schema;
use async_trait::async_trait;
use base64::Engine;
use serde_json::{json, Map, Value};
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tw_core::backend::{StartOptions, StopMode};
use tw_core::control::{ComputerControl, MouseButton};
use tw_core::service::VmService;
use tw_core::{TwError, TwResult, VmName};

/// Newest first. The server answers with the client's version if supported.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];
const MAX_LINE: usize = 4 * 1024 * 1024;
const SETTLE: Duration = Duration::from_millis(400);

pub const INSTRUCTIONS: &str = "Controls a separate Windows virtual machine, never the user's own desktop. \
Call `screenshot` first; every x/y is a pixel in that screenshot (top-left origin). After acting, take another \
screenshot (or pass screenshot=true) and verify the result before continuing. If the guest shows a sign-in screen, \
a UAC prompt or a credential dialog, stop and ask the user to handle it.";

/// What the MCP tools act on. Implemented for `VmService`; tests use a fake.
#[async_trait]
pub trait Target: Send + Sync {
    async fn list_vms(&self) -> TwResult<Value>;
    async fn status(&self, vm: &VmName) -> TwResult<Value>;
    async fn start_vm(&self, vm: &VmName) -> TwResult<Value>;
    async fn stop_vm(&self, vm: &VmName, force: bool) -> TwResult<Value>;
    async fn console(&self, vm: &VmName) -> TwResult<Box<dyn ComputerControl>>;
}

#[async_trait]
impl Target for VmService {
    async fn list_vms(&self) -> TwResult<Value> {
        let vms: Vec<Value> = self
            .list()
            .await?
            .into_iter()
            .map(|(c, s)| json!({ "name": c.name.as_str(), "state": s.state, "arch": c.arch, "cpus": c.cpus, "memory_mib": c.memory_mib }))
            .collect();
        Ok(json!({ "vms": vms }))
    }
    async fn status(&self, vm: &VmName) -> TwResult<Value> {
        Ok(serde_json::to_value(VmService::status(self, vm).await?).unwrap_or(Value::Null))
    }
    async fn start_vm(&self, vm: &VmName) -> TwResult<Value> {
        Ok(serde_json::to_value(self.start(vm, &StartOptions::default()).await?).unwrap_or(Value::Null))
    }
    async fn stop_vm(&self, vm: &VmName, force: bool) -> TwResult<Value> {
        let mode = if force { StopMode::Force } else { StopMode::Graceful };
        Ok(serde_json::to_value(self.stop(vm, mode).await?).unwrap_or(Value::Null))
    }
    async fn console(&self, vm: &VmName) -> TwResult<Box<dyn ComputerControl>> {
        VmService::console(self, vm).await
    }
}

pub struct Server<T> {
    target: T,
    default_vm: VmName,
    server_version: String,
}

/// Typed, strict argument access. Unknown keys were already rejected.
struct Args<'a>(&'a Map<String, Value>);

impl Args<'_> {
    fn u32(&self, key: &str) -> TwResult<u32> {
        self.0
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| TwError::invalid(format!("'{key}' must be a non-negative integer")))
    }
    fn i32_or(&self, key: &str, default: i32) -> TwResult<i32> {
        match self.0.get(key) {
            None => Ok(default),
            Some(v) => {
                v.as_i64().and_then(|v| i32::try_from(v).ok()).ok_or_else(|| TwError::invalid(format!("'{key}' must be an integer")))
            }
        }
    }
    fn str(&self, key: &str) -> TwResult<&str> {
        self.0
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| TwError::invalid(format!("'{key}' must be a non-empty string")))
    }
    fn bool(&self, key: &str) -> TwResult<bool> {
        match self.0.get(key) {
            None => Ok(false),
            Some(v) => v.as_bool().ok_or_else(|| TwError::invalid(format!("'{key}' must be a boolean"))),
        }
    }
}

fn text(s: impl Into<String>) -> Value {
    json!({ "type": "text", "text": s.into() })
}

fn validate_against_schema(tool: &Value, args: &Map<String, Value>) -> TwResult<()> {
    let schema = &tool["inputSchema"];
    let props = schema["properties"].as_object().cloned().unwrap_or_default();
    for key in args.keys() {
        if !props.contains_key(key) {
            return Err(TwError::invalid(format!("unknown argument '{key}'")));
        }
    }
    for req in schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if !args.contains_key(req) {
            return Err(TwError::invalid(format!("missing required argument '{req}'")));
        }
    }
    for (key, value) in args {
        let p = &props[key];
        let ok = match p["type"].as_str() {
            Some("integer") => {
                value.as_i64().is_some_and(|v| p["minimum"].as_i64().is_none_or(|m| v >= m) && p["maximum"].as_i64().is_none_or(|m| v <= m))
            }
            Some("string") => value.as_str().is_some_and(|s| {
                let n = s.chars().count() as u64;
                p["minLength"].as_u64().is_none_or(|m| n >= m)
                    && p["maxLength"].as_u64().is_none_or(|m| n <= m)
                    && p["enum"].as_array().is_none_or(|e| e.iter().any(|x| x == value))
            }),
            Some("boolean") => value.is_boolean(),
            _ => true,
        };
        if !ok {
            return Err(TwError::invalid(format!("argument '{key}' does not match its schema: {p}")));
        }
    }
    Ok(())
}

impl<T: Target> Server<T> {
    pub fn new(target: T, default_vm: VmName, server_version: impl Into<String>) -> Self {
        Self { target, default_vm, server_version: server_version.into() }
    }

    /// Handles one JSON-RPC message; returns the response, or `None` for notifications.
    pub async fn handle(&self, message: Value) -> Option<Value> {
        let id = message.get("id").cloned();
        let method = message.get("method").and_then(Value::as_str).unwrap_or_default();
        let Some(id) = id else {
            return None; // notification (e.g. notifications/initialized)
        };
        let params = message.get("params").cloned().unwrap_or(json!({}));
        let result = match method {
            "initialize" => {
                let requested = params.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
                let version =
                    SUPPORTED_PROTOCOL_VERSIONS.iter().find(|v| **v == requested).copied().unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0]);
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "terminal-windows", "version": self.server_version },
                    "instructions": INSTRUCTIONS
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": schema::tools() })),
            "tools/call" => self.call(&params).await,
            _ => Err((-32601, format!("method not found: {method}"))),
        };
        Some(match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
        })
    }

    async fn call(&self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
        let tools = schema::tools();
        let Some(tool) = tools.as_array().and_then(|t| t.iter().find(|t| t["name"] == name)) else {
            return Err((-32602, format!("unknown tool: {name}")));
        };
        let empty = Map::new();
        let args = match params.get("arguments") {
            None | Some(Value::Null) => &empty,
            Some(Value::Object(m)) => m,
            Some(_) => return Err((-32602, "arguments must be an object".into())),
        };
        let outcome = match validate_against_schema(tool, args) {
            Ok(()) => self.run(name, Args(args)).await,
            Err(e) => Err(e),
        };
        Ok(match outcome {
            Ok(result) => result,
            Err(e) => json!({
                "content": [text(e.to_string())],
                "structuredContent": { "error": e },
                "isError": true
            }),
        })
    }

    async fn screenshot_content(control: &dyn ComputerControl) -> TwResult<Vec<Value>> {
        let shot = control.screenshot().await?;
        Ok(vec![
            json!({ "type": "image", "data": base64::engine::general_purpose::STANDARD.encode(&shot.png), "mimeType": "image/png" }),
            text(format!("Screen: {}x{} pixels (source: {:?}). Use these pixel coordinates.", shot.width, shot.height, shot.source)),
        ])
    }

    async fn run(&self, tool: &str, a: Args<'_>) -> TwResult<Value> {
        let vm = match a.0.get("vm").and_then(Value::as_str) {
            Some(v) => VmName::parse(v)?,
            None => self.default_vm.clone(),
        };
        match tool {
            "list_vms" => {
                let v = self.target.list_vms().await?;
                return Ok(json!({ "content": [text(v.to_string())], "structuredContent": v }));
            }
            "status" => {
                let v = self.target.status(&vm).await?;
                return Ok(json!({ "content": [text(v.to_string())], "structuredContent": v }));
            }
            "start_vm" => {
                let v = self.target.start_vm(&vm).await?;
                return Ok(json!({ "content": [text(v.to_string())], "structuredContent": v }));
            }
            "stop_vm" => {
                let v = self.target.stop_vm(&vm, a.bool("force")?).await?;
                return Ok(json!({ "content": [text(v.to_string())], "structuredContent": v }));
            }
            _ => {}
        }
        let control = self.target.console(&vm).await?;
        if tool == "screenshot" {
            return Ok(json!({ "content": Self::screenshot_content(control.as_ref()).await? }));
        }
        let summary = match tool {
            "click" => {
                let button = match a.0.get("button").and_then(Value::as_str) {
                    Some("right") => MouseButton::Right,
                    Some("middle") => MouseButton::Middle,
                    _ => MouseButton::Left,
                };
                let clicks = a.0.get("clicks").and_then(Value::as_u64).unwrap_or(1) as u8;
                let (x, y) = (a.u32("x")?, a.u32("y")?);
                control.click(x, y, button, clicks).await?;
                format!("Clicked {button:?} x{clicks} at ({x}, {y}).")
            }
            "move" => {
                let (x, y) = (a.u32("x")?, a.u32("y")?);
                control.move_pointer(x, y).await?;
                format!("Moved pointer to ({x}, {y}).")
            }
            "drag" => {
                let from = (a.u32("x")?, a.u32("y")?);
                let to = (a.u32("to_x")?, a.u32("to_y")?);
                control.drag(from, to).await?;
                format!("Dragged from {from:?} to {to:?}.")
            }
            "scroll" => {
                let (x, y) = (a.u32("x")?, a.u32("y")?);
                let (dx, dy) = (a.i32_or("dx", 0)?, a.i32_or("dy", 0)?);
                control.scroll(x, y, dx, dy).await?;
                format!("Scrolled dx={dx} dy={dy} at ({x}, {y}).")
            }
            "type" => {
                let t = a.str("text")?;
                control.type_text(t).await?;
                format!("Typed {} characters.", t.chars().count())
            }
            "key" => {
                let k = a.str("key")?;
                control.key(k).await?;
                format!("Pressed {k}.")
            }
            other => return Err(TwError::invalid(format!("unhandled tool {other}"))),
        };
        let mut content = vec![text(format!("{summary} (via {} console)", control.plane()))];
        if a.bool("screenshot")? {
            tokio::time::sleep(SETTLE).await;
            content.extend(Self::screenshot_content(control.as_ref()).await?);
        }
        Ok(json!({ "content": content }))
    }
}

/// Runs the server until stdin closes.
pub async fn serve<T: Target, R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin>(
    server: &Server<T>,
    input: R,
    mut output: W,
) -> std::io::Result<()> {
    let mut lines = input.lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = if line.len() > MAX_LINE {
            Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32600, "message": "message too large" } }))
        } else {
            match serde_json::from_str::<Value>(&line) {
                Ok(msg) if msg.is_object() => server.handle(msg).await,
                Ok(_) => Some(
                    json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32600, "message": "batch and non-object messages are not supported" } }),
                ),
                Err(_) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "parse error" } })),
            }
        };
        if let Some(r) = response {
            let mut bytes = serde_json::to_vec(&r).unwrap_or_default();
            bytes.push(b'\n');
            output.write_all(&bytes).await?;
            output.flush().await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tw_core::control::{ScreenSource, Screenshot};

    type Calls = Arc<Mutex<Vec<String>>>;

    struct FakeControl(Calls);
    #[async_trait]
    impl ComputerControl for FakeControl {
        fn plane(&self) -> &'static str {
            "fake"
        }
        async fn screenshot(&self) -> TwResult<Screenshot> {
            self.0.lock().unwrap().push("screenshot".into());
            Ok(Screenshot { png: vec![1, 2, 3], width: 800, height: 600, source: ScreenSource::Hypervisor })
        }
        async fn display_size(&self) -> TwResult<(u32, u32)> {
            Ok((800, 600))
        }
        async fn move_pointer(&self, x: u32, y: u32) -> TwResult<()> {
            self.0.lock().unwrap().push(format!("move {x} {y}"));
            Ok(())
        }
        async fn click(&self, x: u32, y: u32, b: MouseButton, n: u8) -> TwResult<()> {
            tw_core::control::validate_point(x, y, (800, 600))?;
            self.0.lock().unwrap().push(format!("click {x} {y} {b:?} {n}"));
            Ok(())
        }
        async fn drag(&self, f: (u32, u32), t: (u32, u32)) -> TwResult<()> {
            self.0.lock().unwrap().push(format!("drag {f:?} {t:?}"));
            Ok(())
        }
        async fn scroll(&self, x: u32, y: u32, dx: i32, dy: i32) -> TwResult<()> {
            self.0.lock().unwrap().push(format!("scroll {x} {y} {dx} {dy}"));
            Ok(())
        }
        async fn type_text(&self, t: &str) -> TwResult<()> {
            self.0.lock().unwrap().push(format!("type {t}"));
            Ok(())
        }
        async fn key(&self, k: &str) -> TwResult<()> {
            self.0.lock().unwrap().push(format!("key {k}"));
            Ok(())
        }
    }

    struct FakeTarget {
        calls: Calls,
        running: bool,
    }
    #[async_trait]
    impl Target for FakeTarget {
        async fn list_vms(&self) -> TwResult<Value> {
            Ok(json!({ "vms": [{ "name": "dev", "state": "running" }] }))
        }
        async fn status(&self, vm: &VmName) -> TwResult<Value> {
            Ok(json!({ "name": vm.as_str(), "state": "running" }))
        }
        async fn start_vm(&self, vm: &VmName) -> TwResult<Value> {
            self.calls.lock().unwrap().push(format!("start {vm}"));
            Ok(json!({ "state": "running" }))
        }
        async fn stop_vm(&self, vm: &VmName, force: bool) -> TwResult<Value> {
            self.calls.lock().unwrap().push(format!("stop {vm} {force}"));
            Ok(json!({ "state": "shutting-down" }))
        }
        async fn console(&self, vm: &VmName) -> TwResult<Box<dyn ComputerControl>> {
            if !self.running {
                return Err(TwError::state(format!("VM '{vm}' is stopped")).with_hint("terminal-windows start dev"));
            }
            Ok(Box::new(FakeControl(self.calls.clone())))
        }
    }

    fn server(running: bool) -> (Server<FakeTarget>, Calls) {
        let calls: Calls = Arc::default();
        (Server::new(FakeTarget { calls: calls.clone(), running }, VmName::parse("dev").unwrap(), "0.1.0"), calls)
    }

    async fn call(s: &Server<FakeTarget>, name: &str, args: Value) -> Value {
        s.handle(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": args } })).await.unwrap()
    }

    #[tokio::test]
    async fn initialize_negotiates_version() {
        let (s, _) = server(true);
        let r = s
            .handle(json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } }))
            .await
            .unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        let r = s
            .handle(json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": "1999-01-01" } }))
            .await
            .unwrap();
        assert_eq!(r["result"]["protocolVersion"], SUPPORTED_PROTOCOL_VERSIONS[0]);
        assert!(s.handle(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await.is_none());
    }

    #[tokio::test]
    async fn tools_list_has_strict_schemas() {
        let (s, _) = server(true);
        let r = s.handle(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })).await.unwrap();
        let tools = r["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        for expected in ["screenshot", "click", "move", "drag", "scroll", "type", "key", "status", "list_vms", "start_vm", "stop_vm"] {
            assert!(names.contains(&expected), "{expected}");
        }
        assert!(tools.iter().all(|t| t["inputSchema"]["additionalProperties"] == false));
    }

    #[tokio::test]
    async fn observe_act_verify_loop() {
        let (s, calls) = server(true);
        let shot = call(&s, "screenshot", json!({})).await;
        assert_eq!(shot["result"]["content"][0]["type"], "image");
        assert_eq!(shot["result"]["content"][0]["data"], "AQID");
        let click = call(&s, "click", json!({ "x": 10, "y": 20, "clicks": 2, "screenshot": true })).await;
        assert!(click["result"]["isError"].is_null());
        assert_eq!(click["result"]["content"][1]["type"], "image");
        assert_eq!(*calls.lock().unwrap(), vec!["screenshot", "click 10 20 Left 2", "screenshot"]);
    }

    #[tokio::test]
    async fn invalid_arguments_are_tool_errors_not_crashes() {
        let (s, calls) = server(true);
        for args in [
            json!({ "x": -1, "y": 0 }),
            json!({ "x": 1 }),
            json!({ "x": 1, "y": 2, "extra": 1 }),
            json!({ "x": 1, "y": 2, "button": "side" }),
        ] {
            let r = call(&s, "click", args.clone()).await;
            assert_eq!(r["result"]["isError"], true, "{args}");
            assert_eq!(r["result"]["structuredContent"]["error"]["code"], "invalid_input");
        }
        let outside = call(&s, "click", json!({ "x": 900, "y": 2 })).await;
        assert_eq!(outside["result"]["isError"], true);
        assert!(calls.lock().unwrap().is_empty());
        let unknown = call(&s, "format_disk", json!({})).await;
        assert_eq!(unknown["error"]["code"], -32602);
        let bad_vm = call(&s, "status", json!({ "vm": "../x" })).await;
        assert_eq!(bad_vm["result"]["isError"], true);
    }

    #[tokio::test]
    async fn stopped_vm_gives_actionable_error() {
        let (s, _) = server(false);
        let r = call(&s, "type", json!({ "text": "hi" })).await;
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("terminal-windows start dev"));
    }

    #[tokio::test]
    async fn lifecycle_tools_and_stdio_loop() {
        let (s, calls) = server(true);
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"stop_vm\",\"arguments\":{\"force\":true}}}\n",
            "not json\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"nope\"}\n"
        );
        let mut out = Vec::new();
        serve(&s, tokio::io::BufReader::new(input.as_bytes()), &mut out).await.unwrap();
        let lines: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["result"]["structuredContent"]["state"], "shutting-down");
        assert_eq!(lines[1]["error"]["code"], -32700);
        assert_eq!(lines[2]["error"]["code"], -32601);
        assert_eq!(*calls.lock().unwrap(), vec!["stop dev true"]);
    }
}
