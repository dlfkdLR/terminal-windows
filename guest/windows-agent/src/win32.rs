//! Win32 implementation of [`Desktop`].
//!
//! STATUS: compiled in CI for Windows targets only; not yet validated inside a
//! Windows VM (docs/compatibility.md). Coordinates are physical pixels of the
//! primary monitor; the process opts into Per-Monitor-V2 DPI awareness first.

use crate::desktop::{DResult, Desktop, DesktopError};
use tw_protocol::{Button, DisplayInfo, SessionState};
use windows_sys::Win32::System::StationsAndDesktops::{CloseDesktop, GetUserObjectInformationW, OpenInputDesktop, UOI_NAME};
use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetSystemMetrics, GetWindowTextW, SM_CXSCREEN, SM_CYSCREEN};

pub struct Win32Desktop;

impl Win32Desktop {
    pub fn new() -> Self {
        // SAFETY: plain Win32 call with a predefined constant; failure only means
        // the manifest already set DPI awareness.
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        Self
    }

    fn send(inputs: &[INPUT]) -> DResult<()> {
        // SAFETY: `inputs` is a valid slice of initialised INPUT structures.
        let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
        if sent as usize != inputs.len() {
            return Err(DesktopError::Failed(format!(
                "SendInput injected {sent} of {} events: {}",
                inputs.len(),
                std::io::Error::last_os_error()
            )));
        }
        Ok(())
    }

    fn mouse(dx: i32, dy: i32, data: i32, flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data as _, dwFlags: flags as _, time: 0, dwExtraInfo: 0 } },
        }
    }

    fn key(vk: u16, scan: u16, flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk as _, wScan: scan, dwFlags: flags as _, time: 0, dwExtraInfo: 0 } },
        }
    }
}

impl Default for Win32Desktop {
    fn default() -> Self {
        Self::new()
    }
}

/// Keys that live in the extended block of the keyboard.
fn extended(vk: u16) -> bool {
    matches!(vk, 0x21..=0x28 | 0x2D | 0x2E | 0x5B | 0x5C | 0x5D | 0x2C)
}

impl Desktop for Win32Desktop {
    fn session(&self) -> SessionState {
        // The user's desktop is named "Default"; Winlogon/secure desktops are not
        // accessible (OpenInputDesktop fails) or have another name.
        // SAFETY: handles are checked and closed; the buffer outlives the call.
        unsafe {
            let desk = OpenInputDesktop(0, 0, 0x0001 /* DESKTOP_READOBJECTS */);
            if desk as usize == 0 {
                return SessionState::SecureDesktop;
            }
            let mut name = [0u16; 64];
            let mut needed = 0u32;
            let ok = GetUserObjectInformationW(desk as _, UOI_NAME, name.as_mut_ptr().cast(), (name.len() * 2) as u32, &mut needed);
            CloseDesktop(desk);
            if ok == 0 {
                return SessionState::SecureDesktop;
            }
            let len = name.iter().position(|c| *c == 0).unwrap_or(name.len());
            if String::from_utf16_lossy(&name[..len]).eq_ignore_ascii_case("Default") {
                SessionState::Active
            } else {
                SessionState::SecureDesktop
            }
        }
    }

    fn display(&self) -> DResult<DisplayInfo> {
        // SAFETY: simple metric queries.
        let (w, h, dpi) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN), GetDpiForSystem()) };
        if w <= 0 || h <= 0 {
            return Err(DesktopError::NoSession);
        }
        Ok(DisplayInfo { width: w as u32, height: h as u32, scale_percent: dpi * 100 / 96 })
    }

    fn foreground_title(&self) -> Option<String> {
        // SAFETY: the buffer is valid for its length.
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd as usize == 0 {
                return None;
            }
            let mut buf = [0u16; 256];
            let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
            (n > 0).then(|| String::from_utf16_lossy(&buf[..n as usize]))
        }
    }

    fn move_abs(&mut self, nx: u32, ny: u32) -> DResult<()> {
        Self::send(&[Self::mouse(nx as i32, ny as i32, 0, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE)])
    }

    fn button(&mut self, button: Button, down: bool) -> DResult<()> {
        let flag = match (button, down) {
            (Button::Left, true) => MOUSEEVENTF_LEFTDOWN,
            (Button::Left, false) => MOUSEEVENTF_LEFTUP,
            (Button::Right, true) => MOUSEEVENTF_RIGHTDOWN,
            (Button::Right, false) => MOUSEEVENTF_RIGHTUP,
            (Button::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
            (Button::Middle, false) => MOUSEEVENTF_MIDDLEUP,
        };
        Self::send(&[Self::mouse(0, 0, 0, flag)])
    }

    fn wheel(&mut self, delta: i32, horizontal: bool) -> DResult<()> {
        let flag = if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL };
        Self::send(&[Self::mouse(0, 0, delta, flag)])
    }

    fn vk(&mut self, vk: u16, down: bool) -> DResult<()> {
        let mut flags = if down { 0 } else { KEYEVENTF_KEYUP };
        if extended(vk) {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        Self::send(&[Self::key(vk, 0, flags)])
    }

    fn unicode(&mut self, units: &[u16]) -> DResult<()> {
        let mut inputs = Vec::with_capacity(units.len() * 2);
        for &u in units {
            inputs.push(Self::key(0, u, KEYEVENTF_UNICODE));
            inputs.push(Self::key(0, u, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
        }
        Self::send(&inputs)
    }

    fn sleep_ms(&mut self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}
