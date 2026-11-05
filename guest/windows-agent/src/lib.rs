//! Guest agent library. Platform-independent request handling lives here and
//! is unit-tested on every host; the Win32 implementation of [`desktop::Desktop`]
//! is in [`win32`] and compiles only for Windows targets.

pub mod bridge;
pub mod desktop;
pub mod dispatch;
pub mod text;
#[cfg(windows)]
pub mod win32;

/// virtio-serial port name; must match `tw_qemu::cmdline::AGENT_PORT_NAME`.
pub const PORT_PATH: &str = r"\\.\Global\com.terminal-windows.agent.0";
