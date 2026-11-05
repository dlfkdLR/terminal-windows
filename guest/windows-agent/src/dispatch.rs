//! Validates protocol actions and executes them through a [`Desktop`].

use crate::desktop::{Desktop, DesktopError};
use crate::text;
use tw_core::control::limits;
use tw_core::coords::{normalize, SENDINPUT_ABS_MAX};
use tw_input::keys::Chord;
use tw_protocol::{Action, ActionResult, Button, GuestErrorCode, Outcome, SessionState};

fn err(code: GuestErrorCode, message: impl Into<String>) -> Outcome {
    Outcome::Error { code, message: message.into() }
}

impl From<DesktopError> for Outcome {
    fn from(e: DesktopError) -> Self {
        match e {
            DesktopError::SecureDesktop => {
                err(GuestErrorCode::SecureDesktop, "the secure desktop (UAC / sign-in) is active; only the hypervisor console can reach it")
            }
            DesktopError::ElevatedTarget => err(GuestErrorCode::ElevatedTarget, "Windows blocked input to an elevated window (UIPI)"),
            DesktopError::NoSession => err(GuestErrorCode::NoInteractiveSession, "no interactive user session"),
            DesktopError::Failed(m) => err(GuestErrorCode::Internal, m),
        }
    }
}

macro_rules! tri {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return Outcome::from(e),
        }
    };
}

fn point<D: Desktop>(d: &D, x: u32, y: u32) -> Result<(u32, u32), Outcome> {
    let info = d.display().map_err(Outcome::from)?;
    if x >= info.width || y >= info.height {
        return Err(err(GuestErrorCode::InvalidArgument, format!("({x}, {y}) is outside the {}x{} display", info.width, info.height)));
    }
    Ok((normalize(x, info.width, SENDINPUT_ABS_MAX), normalize(y, info.height, SENDINPUT_ABS_MAX)))
}

pub fn handle<D: Desktop>(d: &mut D, action: &Action) -> Outcome {
    // Input actions are refused up front when the input desktop is not the user's.
    let is_input = !matches!(action, Action::Status | Action::Screenshot { .. } | Action::UiTree { .. } | Action::Shutdown);
    if is_input {
        match d.session() {
            SessionState::Active => {}
            SessionState::SecureDesktop | SessionState::Locked => return DesktopError::SecureDesktop.into(),
            SessionState::NoSession => return DesktopError::NoSession.into(),
        }
    }
    let done = Outcome::Ok { result: ActionResult::Done };
    match action {
        Action::Status => {
            let display = tri!(d.display());
            Outcome::Ok { result: ActionResult::Status { session: d.session(), display, foreground: d.foreground_title() } }
        }
        Action::Move { x, y } => {
            let (nx, ny) = match point(d, *x, *y) {
                Ok(p) => p,
                Err(o) => return o,
            };
            tri!(d.move_abs(nx, ny));
            done
        }
        Action::Click { x, y, button, count } => {
            if !(1..=limits::MAX_CLICKS).contains(count) {
                return err(GuestErrorCode::InvalidArgument, "count must be 1-3");
            }
            let (nx, ny) = match point(d, *x, *y) {
                Ok(p) => p,
                Err(o) => return o,
            };
            tri!(d.move_abs(nx, ny));
            for i in 0..*count {
                tri!(d.button(*button, true));
                tri!(d.button(*button, false));
                if i + 1 < *count {
                    d.sleep_ms(50);
                }
            }
            done
        }
        Action::Drag { x, y, to_x, to_y } => {
            let (a, b) = match (point(d, *x, *y), point(d, *to_x, *to_y)) {
                (Ok(a), Ok(b)) => (a, b),
                (Err(o), _) | (_, Err(o)) => return o,
            };
            tri!(d.move_abs(a.0, a.1));
            tri!(d.button(Button::Left, true));
            let steps = 20u32;
            let mut failure = None;
            for s in 1..=steps {
                let lerp = |p: u32, q: u32| (i64::from(p) + (i64::from(q) - i64::from(p)) * i64::from(s) / i64::from(steps)) as u32;
                if let Err(e) = d.move_abs(lerp(a.0, b.0), lerp(a.1, b.1)) {
                    failure = Some(e);
                    break;
                }
                d.sleep_ms(8);
            }
            let release = d.button(Button::Left, false);
            if let Some(e) = failure {
                return e.into();
            }
            tri!(release);
            done
        }
        Action::Scroll { x, y, dx, dy } => {
            let max = limits::MAX_SCROLL;
            if dx.abs() > max || dy.abs() > max || (*dx == 0 && *dy == 0) {
                return err(GuestErrorCode::InvalidArgument, "scroll amounts out of range");
            }
            let (nx, ny) = match point(d, *x, *y) {
                Ok(p) => p,
                Err(o) => return o,
            };
            tri!(d.move_abs(nx, ny));
            if *dy != 0 {
                tri!(d.wheel(*dy, false));
            }
            if *dx != 0 {
                tri!(d.wheel(*dx, true));
            }
            done
        }
        Action::Type { text: t } => {
            if t.is_empty() || t.len() > limits::MAX_TEXT_BYTES {
                return err(GuestErrorCode::InvalidArgument, "text must be 1 byte to 32 KiB");
            }
            for chunk in text::chunks(t, 16) {
                match chunk {
                    Some(units) => tri!(d.unicode(&units)),
                    None => {
                        tri!(d.vk(0x0D, true));
                        tri!(d.vk(0x0D, false));
                    }
                }
            }
            done
        }
        Action::Key { chord } => {
            let chord = match Chord::parse(chord) {
                Ok(c) => c,
                Err(e) => return err(GuestErrorCode::InvalidArgument, e.0),
            };
            let vks = chord.vks();
            let mut pressed = Vec::new();
            let mut failure = None;
            for vk in &vks {
                match d.vk(*vk, true) {
                    Ok(()) => pressed.push(*vk),
                    Err(e) => {
                        failure = Some(e);
                        break;
                    }
                }
            }
            // Release whatever was pressed, in reverse, even after a failure.
            for vk in pressed.iter().rev() {
                let _ = d.vk(*vk, false);
            }
            match failure {
                Some(e) => e.into(),
                None => done,
            }
        }
        Action::Screenshot { .. } => {
            err(GuestErrorCode::Unsupported, "agent screenshots are not implemented; the host captures the virtual display")
        }
        Action::UiTree { .. } => err(GuestErrorCode::Unsupported, "UI Automation tree is not implemented yet"),
        Action::Shutdown => err(GuestErrorCode::Unsupported, "use the hypervisor's ACPI power button (terminal-windows stop)"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::desktop::DResult;
    use tw_protocol::DisplayInfo;

    #[derive(Default)]
    pub struct Fake {
        pub log: Vec<String>,
        pub session: Option<SessionState>,
        pub fail_vk: Option<u16>,
    }

    impl Desktop for Fake {
        fn session(&self) -> SessionState {
            self.session.unwrap_or(SessionState::Active)
        }
        fn display(&self) -> DResult<DisplayInfo> {
            Ok(DisplayInfo { width: 1920, height: 1080, scale_percent: 150 })
        }
        fn foreground_title(&self) -> Option<String> {
            Some("Notepad".into())
        }
        fn move_abs(&mut self, nx: u32, ny: u32) -> DResult<()> {
            self.log.push(format!("move {nx} {ny}"));
            Ok(())
        }
        fn button(&mut self, b: Button, down: bool) -> DResult<()> {
            self.log.push(format!("{b:?} {}", if down { "down" } else { "up" }));
            Ok(())
        }
        fn wheel(&mut self, delta: i32, h: bool) -> DResult<()> {
            self.log.push(format!("wheel {delta} {h}"));
            Ok(())
        }
        fn vk(&mut self, vk: u16, down: bool) -> DResult<()> {
            if self.fail_vk == Some(vk) && down {
                return Err(DesktopError::ElevatedTarget);
            }
            self.log.push(format!("vk {vk:#04x} {down}"));
            Ok(())
        }
        fn unicode(&mut self, units: &[u16]) -> DResult<()> {
            self.log.push(format!("text {}", String::from_utf16_lossy(units)));
            Ok(())
        }
        fn sleep_ms(&mut self, _: u64) {}
    }

    #[test]
    fn click_normalizes_to_sendinput_range() {
        let mut d = Fake::default();
        assert_eq!(
            handle(&mut d, &Action::Click { x: 1919, y: 0, button: Button::Left, count: 2 }),
            Outcome::Ok { result: ActionResult::Done }
        );
        assert_eq!(d.log, vec!["move 65535 0", "Left down", "Left up", "Left down", "Left up"]);
        assert!(matches!(handle(&mut d, &Action::Move { x: 1920, y: 0 }), Outcome::Error { code: GuestErrorCode::InvalidArgument, .. }));
    }

    #[test]
    fn unicode_text_and_newlines() {
        let mut d = Fake::default();
        handle(&mut d, &Action::Type { text: "안녕\nWindows".into() });
        assert_eq!(d.log, vec!["text 안녕", "vk 0x0d true", "vk 0x0d false", "text Windows"]);
    }

    #[test]
    fn secure_desktop_blocks_input_but_not_status() {
        let mut d = Fake { session: Some(SessionState::SecureDesktop), ..Default::default() };
        assert!(matches!(
            handle(&mut d, &Action::Key { chord: "return".into() }),
            Outcome::Error { code: GuestErrorCode::SecureDesktop, .. }
        ));
        assert!(d.log.is_empty());
        assert!(matches!(
            handle(&mut d, &Action::Status),
            Outcome::Ok { result: ActionResult::Status { session: SessionState::SecureDesktop, .. } }
        ));
    }

    #[test]
    fn chord_failure_still_releases_pressed_keys() {
        let mut d = Fake { fail_vk: Some(0x1B), ..Default::default() };
        let out = handle(&mut d, &Action::Key { chord: "ctrl+shift+esc".into() });
        assert!(matches!(out, Outcome::Error { code: GuestErrorCode::ElevatedTarget, .. }));
        assert_eq!(d.log, vec!["vk 0x11 true", "vk 0x10 true", "vk 0x10 false", "vk 0x11 false"]);
    }

    #[test]
    fn drag_and_scroll() {
        let mut d = Fake::default();
        handle(&mut d, &Action::Drag { x: 0, y: 0, to_x: 1919, to_y: 1079 });
        assert_eq!(d.log.first().unwrap(), "move 0 0");
        assert_eq!(d.log.last().unwrap(), "Left up");
        assert!(d.log.contains(&"move 65535 65535".to_string()));
        let mut d = Fake::default();
        handle(&mut d, &Action::Scroll { x: 10, y: 10, dx: 0, dy: -240 });
        assert_eq!(d.log.last().unwrap(), "wheel -240 false");
    }
}
