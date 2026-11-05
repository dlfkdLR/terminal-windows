//! Plane H: computer control through the hypervisor console.
//!
//! Screen: QMP `screendump`. Input: QMP `input-send-event` to the USB tablet
//! (absolute pointer) and USB keyboard (QEMU qcodes). This works before
//! Windows is installed, at the lock screen and on the UAC secure desktop,
//! because it acts like the VM's physical monitor and keyboard.

use crate::image;
use crate::qmp::{input, Qmp};
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex;
use tw_core::control::{self, ComputerControl, InteractiveConsole, MouseButton, RawFrame, ScreenSource, Screenshot};
use tw_core::coords::to_qemu_abs;
use tw_core::{ErrorCode, TwError, TwResult};
use tw_input::keys::{us_ascii_key, Chord};

/// Windows `WHEEL_DELTA`: scroll units per wheel notch.
pub const WHEEL_DELTA: i32 = 120;

pub struct QmpConsole<S> {
    qmp: Mutex<Qmp<S>>,
    /// Private directory (0700) where QEMU writes screendumps.
    scratch: PathBuf,
    size: Mutex<Option<(u32, u32)>>,
    /// Gap between individual keyboard events. QEMU's USB HID queue holds 16
    /// events and drops the rest; firmware (UEFI) polls the keyboard slowly.
    /// Measured 2026-11-05 (QEMU 11.1.3, HVF, EDK II shell): batched events at
    /// 8 ms/char lost input; one event per 15 ms garbled it; 30 ms and 50 ms
    /// typed a 47-character mixed-case/symbol string exactly. Default: 30 ms.
    event_delay: Duration,
}

/// Overrides the keyboard event gap in milliseconds (for tuning on slow guests).
pub const EVENT_DELAY_ENV: &str = "TW_CONSOLE_EVENT_DELAY_MS";

fn button_name(b: MouseButton) -> &'static str {
    match b {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
    }
}

impl<S: AsyncRead + AsyncWrite + Send> QmpConsole<S> {
    pub fn new(qmp: Qmp<S>, scratch: PathBuf) -> Self {
        let ms = std::env::var(EVENT_DELAY_ENV).ok().and_then(|v| v.parse::<u64>().ok()).filter(|v| *v <= 1000).unwrap_or(30);
        Self { qmp: Mutex::new(qmp), scratch, size: Mutex::new(None), event_delay: Duration::from_millis(ms) }
    }

    /// Sends keyboard events one at a time, paced so the HID queue never overflows.
    async fn send_keys_paced(&self, events: Vec<serde_json::Value>) -> TwResult<()> {
        for event in events {
            self.send(vec![event]).await?;
            tokio::time::sleep(self.event_delay).await;
        }
        Ok(())
    }

    async fn capture(&self) -> TwResult<Screenshot> {
        let file = self.scratch.join(format!("screendump-{}.png", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let path = file.to_string_lossy().into_owned();
        let mut qmp = self.qmp.lock().await;
        // PNG output needs QEMU 7.1+; fall back to PPM for older builds.
        let png = match qmp.execute("screendump", Some(json!({ "filename": path, "format": "png" }))).await {
            Ok(_) => read_and_remove(&file)?,
            Err(e) if e.message.contains("format") => {
                qmp.execute("screendump", Some(json!({ "filename": path }))).await?;
                let ppm = read_and_remove(&file)?;
                let (w, h, rgb) = image::decode_ppm(&ppm)?;
                image::encode_png_rgb(w, h, &rgb)?
            }
            Err(e) => return Err(e),
        };
        drop(qmp);
        let (width, height) = image::png_dimensions(&png)?;
        *self.size.lock().await = Some((width, height));
        Ok(Screenshot { png, width, height, source: ScreenSource::Hypervisor })
    }

    async fn size(&self) -> TwResult<(u32, u32)> {
        if let Some(size) = *self.size.lock().await {
            return Ok(size);
        }
        self.capture().await.map(|s| (s.width, s.height))
    }

    async fn point(&self, x: u32, y: u32) -> TwResult<Vec<serde_json::Value>> {
        let size = self.size().await?;
        control::validate_point(x, y, size)?;
        Ok(vec![input::abs("x", to_qemu_abs(x, size.0)), input::abs("y", to_qemu_abs(y, size.1))])
    }

    async fn send(&self, events: Vec<serde_json::Value>) -> TwResult<()> {
        self.qmp.lock().await.send_input(events).await
    }
}

fn read_and_remove(file: &std::path::Path) -> TwResult<Vec<u8>> {
    let data = std::fs::read(file).map_err(|e| TwError::io(file.display(), e));
    let _ = std::fs::remove_file(file);
    data
}

#[async_trait]
impl<S: AsyncRead + AsyncWrite + Send + 'static> ComputerControl for QmpConsole<S> {
    fn plane(&self) -> &'static str {
        "qmp"
    }

    async fn screenshot(&self) -> TwResult<Screenshot> {
        self.capture().await
    }

    async fn display_size(&self) -> TwResult<(u32, u32)> {
        self.size().await
    }

    async fn move_pointer(&self, x: u32, y: u32) -> TwResult<()> {
        let events = self.point(x, y).await?;
        self.send(events).await
    }

    async fn click(&self, x: u32, y: u32, button: MouseButton, count: u8) -> TwResult<()> {
        if !(1..=control::limits::MAX_CLICKS).contains(&count) {
            return Err(TwError::invalid("click count must be 1, 2 or 3"));
        }
        let pos = self.point(x, y).await?;
        self.send(pos).await?;
        let name = button_name(button);
        for i in 0..count {
            self.send(vec![input::btn(name, true)]).await?;
            self.send(vec![input::btn(name, false)]).await?;
            if i + 1 < count {
                // Well inside the default Windows double-click time (500 ms).
                tokio::time::sleep(Duration::from_millis(60)).await;
            }
        }
        Ok(())
    }

    async fn drag(&self, from: (u32, u32), to: (u32, u32)) -> TwResult<()> {
        let start = self.point(from.0, from.1).await?;
        self.point(to.0, to.1).await?; // validate before pressing anything
        self.send(start).await?;
        self.send(vec![input::btn("left", true)]).await?;
        let dist = (f64::from(to.0) - f64::from(from.0)).hypot(f64::from(to.1) - f64::from(from.1));
        let steps = ((dist / 12.0) as u32).clamp(2, 30);
        let mut result = Ok(());
        for step in 1..=steps {
            let t = f64::from(step) / f64::from(steps);
            let x = (f64::from(from.0) + (f64::from(to.0) - f64::from(from.0)) * t).round() as u32;
            let y = (f64::from(from.1) + (f64::from(to.1) - f64::from(from.1)) * t).round() as u32;
            let events = self.point(x, y).await?;
            if let Err(e) = self.send(events).await {
                result = Err(e);
                break;
            }
            tokio::time::sleep(Duration::from_millis(8)).await;
        }
        // Always release the button, even if a move failed.
        let release = self.send(vec![input::btn("left", false)]).await;
        result.and(release)
    }

    async fn scroll(&self, x: u32, y: u32, dx: i32, dy: i32) -> TwResult<()> {
        let max = control::limits::MAX_SCROLL;
        if !(-max..=max).contains(&dx) || !(-max..=max).contains(&dy) || (dx == 0 && dy == 0) {
            return Err(TwError::invalid(format!("scroll amounts must be within ±{max} and not both zero")));
        }
        let pos = self.point(x, y).await?;
        self.send(pos).await?;
        let notches = |v: i32| ((v.abs() + WHEEL_DELTA / 2) / WHEEL_DELTA).max(if v != 0 { 1 } else { 0 });
        for (amount, positive, negative) in [(dy, "wheel-up", "wheel-down"), (dx, "wheel-right", "wheel-left")] {
            let name = if amount > 0 { positive } else { negative };
            for _ in 0..notches(amount) {
                self.send(vec![input::btn(name, true), input::btn(name, false)]).await?;
            }
        }
        Ok(())
    }

    async fn type_text(&self, text: &str) -> TwResult<()> {
        control::validate_text(text)?;
        // Validate everything first so a partially typed string never happens.
        let keys: Vec<_> = text
            .chars()
            .map(|c| {
                us_ascii_key(c).ok_or_else(|| {
                    TwError::new(ErrorCode::Unsupported, format!("character {c:?} cannot be typed through the hypervisor console"))
                        .with_hint("The console presses physical keys on a US layout. Unicode text needs the in-guest agent (Plane G).")
                })
            })
            .collect::<TwResult<_>>()?;
        for (key, shift) in keys {
            let mut events = Vec::new();
            if shift {
                events.push(input::key("shift", true));
            }
            events.push(input::key(key.qcode, true));
            events.push(input::key(key.qcode, false));
            if shift {
                events.push(input::key("shift", false));
            }
            self.send_keys_paced(events).await?;
        }
        Ok(())
    }

    async fn key(&self, chord: &str) -> TwResult<()> {
        let chord = Chord::parse(chord).map_err(|e| TwError::invalid(e.0))?;
        let codes = chord.qcodes();
        let mut events: Vec<_> = codes.iter().map(|c| input::key(c, true)).collect();
        events.extend(codes.iter().rev().map(|c| input::key(c, false)));
        self.send_keys_paced(events).await
    }
}

#[async_trait]
impl<S: AsyncRead + AsyncWrite + Send + 'static> InteractiveConsole for QmpConsole<S> {
    /// PPM screendump: raw pixels, no PNG encode/decode on the hot path.
    async fn frame(&self) -> TwResult<RawFrame> {
        let file = self.scratch.join(format!("frame-{}.ppm", std::process::id()));
        let path = file.to_string_lossy().into_owned();
        self.qmp.lock().await.execute("screendump", Some(json!({ "filename": path }))).await?;
        let ppm = read_and_remove(&file)?;
        let (width, height, rgb) = image::decode_ppm(&ppm)?;
        *self.size.lock().await = Some((width, height));
        Ok(RawFrame { width, height, rgb })
    }

    async fn pointer(&self, x: u32, y: u32, frame_size: (u32, u32)) -> TwResult<()> {
        let (w, h) = frame_size;
        self.send(vec![
            input::abs("x", to_qemu_abs(x.min(w.saturating_sub(1)), w)),
            input::abs("y", to_qemu_abs(y.min(h.saturating_sub(1)), h)),
        ])
        .await
    }

    async fn button(&self, button: MouseButton, down: bool) -> TwResult<()> {
        self.send(vec![input::btn(button_name(button), down)]).await
    }

    async fn wheel(&self, up: bool) -> TwResult<()> {
        let name = if up { "wheel-up" } else { "wheel-down" };
        self.send(vec![input::btn(name, true), input::btn(name, false)]).await
    }

    async fn key_event(&self, qcode: &str, down: bool) -> TwResult<()> {
        if qcode.is_empty() || qcode.len() > 20 || !qcode.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
            return Err(TwError::invalid(format!("invalid key code {qcode:?}")));
        }
        self.send(vec![input::key(qcode, down)]).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qmp::mock;
    use serde_json::Value;

    async fn console(dir: &std::path::Path, w: u32, h: u32) -> (QmpConsole<tokio::io::DuplexStream>, mock::Log) {
        let (client, server) = tokio::io::duplex(1 << 20);
        let log = mock::serve(server, move |cmd, args| {
            if cmd == "screendump" {
                let rgb = vec![0u8; (w * h * 3) as usize];
                let png = image::encode_png_rgb(w, h, &rgb).unwrap();
                std::fs::write(args["filename"].as_str().unwrap(), png).unwrap();
            }
            Ok(serde_json::json!({}))
        });
        let qmp = Qmp::handshake(client, Duration::from_secs(2)).await.unwrap();
        let mut console = QmpConsole::new(qmp, dir.to_path_buf());
        console.event_delay = Duration::ZERO;
        (console, log)
    }

    fn inputs(log: &mock::Log) -> Vec<Value> {
        log.lock()
            .unwrap()
            .iter()
            .filter(|m| m["execute"] == "input-send-event")
            .flat_map(|m| m["arguments"]["events"].as_array().unwrap().clone())
            .collect()
    }

    #[tokio::test]
    async fn screenshot_reports_size_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let (c, _) = console(dir.path(), 64, 48).await;
        let shot = c.screenshot().await.unwrap();
        assert_eq!((shot.width, shot.height, shot.source), (64, 48, ScreenSource::Hypervisor));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "screendump file removed");
    }

    #[tokio::test]
    async fn click_maps_to_tablet_coordinates() {
        let dir = tempfile::tempdir().unwrap();
        let (c, log) = console(dir.path(), 100, 50).await;
        c.click(99, 0, MouseButton::Right, 2).await.unwrap();
        let ev = inputs(&log);
        assert_eq!(ev[0]["data"]["value"], 0x7fff);
        assert_eq!(ev[1]["data"]["value"], 0);
        let downs = ev.iter().filter(|e| e["type"] == "btn" && e["data"]["down"] == true && e["data"]["button"] == "right").count();
        assert_eq!(downs, 2);
        assert!(c.click(100, 0, MouseButton::Left, 1).await.is_err(), "outside display");
    }

    #[tokio::test]
    async fn chords_press_and_release_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let (c, log) = console(dir.path(), 10, 10).await;
        c.key("ctrl+alt+delete").await.unwrap();
        let seq: Vec<(String, bool)> = inputs(&log)
            .iter()
            .map(|e| (e["data"]["key"]["data"].as_str().unwrap().to_string(), e["data"]["down"].as_bool().unwrap()))
            .collect();
        let expect = [("ctrl", true), ("alt", true), ("delete", true), ("delete", false), ("alt", false), ("ctrl", false)];
        assert_eq!(seq, expect.iter().map(|(k, d)| (k.to_string(), *d)).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn typing_is_all_or_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (c, log) = console(dir.path(), 10, 10).await;
        let err = c.type_text("Hi 한글").await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unsupported);
        assert!(inputs(&log).is_empty(), "nothing typed");
        c.type_text("A!").await.unwrap();
        let keys: Vec<String> = inputs(&log).iter().map(|e| e["data"]["key"]["data"].as_str().unwrap().to_string()).collect();
        assert_eq!(keys, ["shift", "a", "a", "shift", "shift", "1", "1", "shift"]);
    }

    #[tokio::test]
    async fn scroll_converts_to_wheel_notches() {
        let dir = tempfile::tempdir().unwrap();
        let (c, log) = console(dir.path(), 10, 10).await;
        c.scroll(5, 5, 0, -300).await.unwrap();
        let downs = inputs(&log).iter().filter(|e| e["data"]["button"] == "wheel-down" && e["data"]["down"] == true).count();
        assert_eq!(downs, 3);
        assert!(c.scroll(5, 5, 0, 0).await.is_err());
    }

    #[tokio::test]
    async fn single_key_events_and_validation() {
        let dir = tempfile::tempdir().unwrap();
        let (c, log) = console(dir.path(), 10, 10).await;
        c.key_event("shift_r", true).await.unwrap();
        c.key_event("shift_r", false).await.unwrap();
        let seq: Vec<(String, bool)> = inputs(&log)
            .iter()
            .map(|e| (e["data"]["key"]["data"].as_str().unwrap().to_string(), e["data"]["down"].as_bool().unwrap()))
            .collect();
        assert_eq!(seq, vec![("shift_r".to_string(), true), ("shift_r".to_string(), false)]);
        for bad in ["", "Shift", "a b", "x\"y", &"a".repeat(21)] {
            assert!(c.key_event(bad, true).await.is_err(), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn drag_always_releases() {
        let dir = tempfile::tempdir().unwrap();
        let (c, log) = console(dir.path(), 200, 200).await;
        c.drag((10, 10), (150, 150)).await.unwrap();
        let ev = inputs(&log);
        assert_eq!(ev.last().unwrap()["data"]["down"], false);
        assert!(c.drag((10, 10), (500, 10)).await.is_err());
    }
}
