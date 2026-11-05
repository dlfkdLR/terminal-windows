//! Computer-use control of a guest, independent of how it is delivered.
//!
//! Two planes implement this trait (see docs/PLAN.md §2.3):
//! * Plane H — the hypervisor console (QMP screendump + virtual tablet/keyboard).
//!   Works during setup, at the lock screen and on the UAC secure desktop.
//! * Plane G — the in-guest agent. Adds Unicode text, UI Automation and
//!   session state, but only inside a logged-in interactive session.

use crate::error::TwResult;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    #[default]
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScreenSource {
    /// Read from the virtual display by the hypervisor: ground truth.
    Hypervisor,
    /// Reported by software inside the guest; may be spoofed by guest code.
    GuestReported,
}

#[derive(Debug, Clone)]
pub struct Screenshot {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub source: ScreenSource,
}

/// Validated limits shared by every control implementation.
pub mod limits {
    pub const MAX_TEXT_BYTES: usize = 32 * 1024;
    pub const MAX_SCROLL: i32 = 10_000;
    pub const MAX_CLICKS: u8 = 3;
}

/// Implementations may hold an exclusive connection (QEMU's QMP socket accepts
/// one client at a time): drop the control before opening another connection
/// to the same VM.
#[async_trait]
pub trait ComputerControl: Send + Sync {
    /// Short identifier of the plane, for status output ("qmp", "agent").
    fn plane(&self) -> &'static str;
    async fn screenshot(&self) -> TwResult<Screenshot>;
    /// Current display size in pixels, if known without a capture.
    async fn display_size(&self) -> TwResult<(u32, u32)>;
    async fn move_pointer(&self, x: u32, y: u32) -> TwResult<()>;
    async fn click(&self, x: u32, y: u32, button: MouseButton, count: u8) -> TwResult<()>;
    async fn drag(&self, from: (u32, u32), to: (u32, u32)) -> TwResult<()>;
    /// Positive `dy` scrolls content up (wheel away from the user), negative down.
    async fn scroll(&self, x: u32, y: u32, dx: i32, dy: i32) -> TwResult<()>;
    async fn type_text(&self, text: &str) -> TwResult<()>;
    /// A chord such as `ctrl+shift+esc`, `win+r` or `return`.
    async fn key(&self, chord: &str) -> TwResult<()>;
}

/// A raw RGB24 frame of the guest display.
#[derive(Debug, Clone)]
pub struct RawFrame {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// Low-level console access for the interactive viewer: raw frames and
/// separate press/release events (needed for dragging and held keys).
#[async_trait]
pub trait InteractiveConsole: ComputerControl {
    async fn frame(&self) -> TwResult<RawFrame>;
    /// Moves the pointer to a pixel of a frame of size `frame_size`.
    async fn pointer(&self, x: u32, y: u32, frame_size: (u32, u32)) -> TwResult<()>;
    async fn button(&self, button: MouseButton, down: bool) -> TwResult<()>;
    /// One wheel notch; `up` scrolls content up.
    async fn wheel(&self, up: bool) -> TwResult<()>;
}

/// Shared argument validation so every plane rejects the same inputs.
pub fn validate_point(x: u32, y: u32, size: (u32, u32)) -> TwResult<()> {
    if x >= size.0 || y >= size.1 {
        return Err(crate::TwError::invalid(format!(
            "point ({x}, {y}) is outside the {}x{} display; coordinates are screenshot pixels from the top-left",
            size.0, size.1
        )));
    }
    Ok(())
}

pub fn validate_text(text: &str) -> TwResult<()> {
    if text.is_empty() {
        return Err(crate::TwError::invalid("text must not be empty"));
    }
    if text.len() > limits::MAX_TEXT_BYTES {
        return Err(crate::TwError::invalid(format!("text exceeds {} bytes", limits::MAX_TEXT_BYTES)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_outside_display_are_rejected() {
        assert!(validate_point(1919, 1079, (1920, 1080)).is_ok());
        assert!(validate_point(1920, 0, (1920, 1080)).is_err());
        assert!(validate_point(0, 1080, (1920, 1080)).is_err());
    }

    #[test]
    fn text_limits() {
        assert!(validate_text("").is_err());
        assert!(validate_text(&"a".repeat(limits::MAX_TEXT_BYTES + 1)).is_err());
        assert!(validate_text("hello").is_ok());
    }
}
