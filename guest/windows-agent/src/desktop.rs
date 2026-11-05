//! The OS-facing operations the agent needs. Implemented by `win32::Win32Desktop`
//! on Windows and by a recording fake in tests.

use tw_protocol::{Button, DisplayInfo, SessionState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopError {
    SecureDesktop,
    ElevatedTarget,
    NoSession,
    Failed(String),
}

pub type DResult<T> = Result<T, DesktopError>;

pub trait Desktop: Send {
    fn session(&self) -> SessionState;
    fn display(&self) -> DResult<DisplayInfo>;
    fn foreground_title(&self) -> Option<String>;
    /// Absolute pointer position, already normalized to 0..=65535 on each axis.
    fn move_abs(&mut self, nx: u32, ny: u32) -> DResult<()>;
    fn button(&mut self, button: Button, down: bool) -> DResult<()>;
    /// Wheel delta in Windows units (120 per notch); `horizontal` for HWHEEL.
    fn wheel(&mut self, delta: i32, horizontal: bool) -> DResult<()>;
    fn vk(&mut self, vk: u16, down: bool) -> DResult<()>;
    fn unicode(&mut self, units: &[u16]) -> DResult<()>;
    fn sleep_ms(&mut self, ms: u64);
}
