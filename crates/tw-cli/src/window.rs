//! Native window viewer: the fallback for terminals without Kitty graphics, like
//! terminal-mac's native VM window. It shows the guest display and forwards the mouse
//! and the keyboard through the hypervisor console, so it works during Windows Setup,
//! at sign-in and on UAC prompts. Closing the window leaves the VM running.
//!
//! macOS and Windows only; elsewhere [`run`] reports that it is unsupported.

use tw_core::service::VmService;
use tw_core::{TwResult, VmName};

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub async fn run(_service: &VmService, _name: &VmName) -> TwResult<()> {
    Err(tw_core::TwError::unsupported("the native window viewer is only available on macOS and Windows"))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub async fn run(service: &VmService, name: &VmName) -> TwResult<()> {
    use std::sync::Arc;
    use tw_core::control::InteractiveConsole;
    let console: Arc<dyn InteractiveConsole> = Arc::from(service.interactive_console(name).await?);
    // Fail before opening a window if the console does not answer.
    let first = console.frame().await?;
    let handle = tokio::runtime::Handle::current();
    imp::run_blocking(handle, console, first, name.to_string())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod imp {
    use crate::geometry::{blit, fit, to_guest, Rect};
    use std::collections::HashSet;
    use std::num::NonZeroU32;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tw_core::control::InteractiveConsole;
    use tw_core::control::{MouseButton, RawFrame};
    use tw_core::{TwError, TwResult};
    use winit::application::ApplicationHandler;
    use winit::dpi::LogicalSize;
    use winit::event::{ElementState, MouseButton as WinitButton, MouseScrollDelta, WindowEvent};
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
    use winit::keyboard::{KeyCode, PhysicalKey};
    use winit::window::{Window, WindowId};

    const FRAME_INTERVAL: Duration = Duration::from_millis(60);
    const BACKGROUND: u32 = 0x0018181b;

    /// Environment variable that opens the window without taking focus (used by tests).
    const NO_ACTIVATE_ENV: &str = "TW_WINDOW_NO_ACTIVATE";
    /// Environment variable that logs every forwarded input event to stderr (viewer.log).
    const DEBUG_ENV: &str = "TW_WINDOW_DEBUG";

    enum UserEvent {
        Frame(RawFrame),
        Failed(String),
    }

    #[derive(Debug, Clone, PartialEq)]
    pub enum Job {
        Move { x: u32, y: u32, size: (u32, u32) },
        Button { x: u32, y: u32, size: (u32, u32), button: MouseButton, down: bool },
        Wheel { up: bool },
        Key { qcode: &'static str, down: bool },
    }

    /// Drops pointer moves that a later job supersedes, so a flood of mouse events never queues up
    /// behind the (slow) console while presses, releases and keys keep their order.
    pub fn coalesce(jobs: Vec<Job>) -> Vec<Job> {
        let mut out: Vec<Job> = Vec::with_capacity(jobs.len());
        for job in jobs {
            if matches!(job, Job::Move { .. }) && matches!(out.last(), Some(Job::Move { .. })) {
                out.pop();
            }
            out.push(job);
        }
        out
    }

    /// Physical key to QEMU qcode. Physical keys are forwarded (like a real keyboard), so the
    /// guest's own layout and IME decide what is typed. Right Alt is the Korean Hangul key on
    /// 101-key layouts, and `Lang1`/`Lang2` cover keyboards that have dedicated keys.
    pub fn qcode(code: KeyCode) -> Option<&'static str> {
        use KeyCode::*;
        Some(match code {
            KeyA => "a",
            KeyB => "b",
            KeyC => "c",
            KeyD => "d",
            KeyE => "e",
            KeyF => "f",
            KeyG => "g",
            KeyH => "h",
            KeyI => "i",
            KeyJ => "j",
            KeyK => "k",
            KeyL => "l",
            KeyM => "m",
            KeyN => "n",
            KeyO => "o",
            KeyP => "p",
            KeyQ => "q",
            KeyR => "r",
            KeyS => "s",
            KeyT => "t",
            KeyU => "u",
            KeyV => "v",
            KeyW => "w",
            KeyX => "x",
            KeyY => "y",
            KeyZ => "z",
            Digit0 => "0",
            Digit1 => "1",
            Digit2 => "2",
            Digit3 => "3",
            Digit4 => "4",
            Digit5 => "5",
            Digit6 => "6",
            Digit7 => "7",
            Digit8 => "8",
            Digit9 => "9",
            Minus => "minus",
            Equal => "equal",
            BracketLeft => "bracket_left",
            BracketRight => "bracket_right",
            Backslash => "backslash",
            Semicolon => "semicolon",
            Quote => "apostrophe",
            Backquote => "grave_accent",
            Comma => "comma",
            Period => "dot",
            Slash => "slash",
            IntlBackslash => "less",
            IntlRo => "ro",
            IntlYen => "yen",
            Enter => "ret",
            Space => "spc",
            Tab => "tab",
            Escape => "esc",
            Backspace => "backspace",
            Delete => "delete",
            Insert => "insert",
            ArrowUp => "up",
            ArrowDown => "down",
            ArrowLeft => "left",
            ArrowRight => "right",
            Home => "home",
            End => "end",
            PageUp => "pgup",
            PageDown => "pgdn",
            F1 => "f1",
            F2 => "f2",
            F3 => "f3",
            F4 => "f4",
            F5 => "f5",
            F6 => "f6",
            F7 => "f7",
            F8 => "f8",
            F9 => "f9",
            F10 => "f10",
            F11 => "f11",
            F12 => "f12",
            PrintScreen => "print",
            ContextMenu => "menu",
            CapsLock => "caps_lock",
            NumLock => "num_lock",
            ScrollLock => "scroll_lock",
            Pause => "pause",
            ShiftLeft => "shift",
            ShiftRight => "shift_r",
            ControlLeft => "ctrl",
            ControlRight => "ctrl_r",
            AltLeft => "alt",
            AltRight => "alt_r",
            // The Command key is the Windows key.
            SuperLeft => "meta_l",
            SuperRight => "meta_r",
            Lang1 => "lang1",
            Lang2 => "lang2",
            Numpad0 => "kp_0",
            Numpad1 => "kp_1",
            Numpad2 => "kp_2",
            Numpad3 => "kp_3",
            Numpad4 => "kp_4",
            Numpad5 => "kp_5",
            Numpad6 => "kp_6",
            Numpad7 => "kp_7",
            Numpad8 => "kp_8",
            Numpad9 => "kp_9",
            NumpadEnter => "kp_enter",
            NumpadAdd => "kp_add",
            NumpadSubtract => "kp_subtract",
            NumpadMultiply => "kp_multiply",
            NumpadDivide => "kp_divide",
            NumpadDecimal => "kp_decimal",
            _ => return None,
        })
    }

    pub fn run_blocking(
        handle: tokio::runtime::Handle,
        console: Arc<dyn InteractiveConsole>,
        first: RawFrame,
        name: String,
    ) -> TwResult<()> {
        let event_loop = EventLoop::<UserEvent>::with_user_event()
            .build()
            .map_err(|e| TwError::backend(format!("cannot start the window system: {e}")))?;
        event_loop.set_control_flow(ControlFlow::Wait);
        let proxy = event_loop.create_proxy();

        // Frames: poll the console and hand the GUI thread only changed frames, at most one in flight.
        let pending = Arc::new(AtomicBool::new(false));
        {
            let (console, pending, proxy) = (console.clone(), pending.clone(), proxy.clone());
            handle.spawn(frame_loop(console, pending, proxy));
        }

        // Input: one worker applies jobs in order.
        let (tx, mut rx) = mpsc::channel::<Job>(512);
        {
            let console = console.clone();
            let proxy = proxy.clone();
            handle.spawn(async move {
                while let Some(first) = rx.recv().await {
                    let mut batch = vec![first];
                    while let Ok(job) = rx.try_recv() {
                        batch.push(job);
                    }
                    for job in coalesce(batch) {
                        let result = match job {
                            Job::Move { x, y, size } => console.pointer(x, y, size).await,
                            Job::Button { x, y, size, button, down } => match console.pointer(x, y, size).await {
                                Ok(()) => console.button(button, down).await,
                                e => e,
                            },
                            Job::Wheel { up } => console.wheel(up).await,
                            Job::Key { qcode, down } => console.key_event(qcode, down).await,
                        };
                        if let Err(e) = result {
                            let _ = proxy.send_event(UserEvent::Failed(e.message));
                        }
                    }
                }
            });
        }

        let mut app = App {
            name,
            frame: first,
            pending,
            tx,
            window: None,
            surface: None,
            cursor: (0.0, 0.0),
            pressed: HashSet::new(),
            dragging: false,
            error: None,
            debug: std::env::var_os(DEBUG_ENV).is_some(),
        };
        let _keep_runtime_alive = handle;
        event_loop.run_app(&mut app).map_err(|e| TwError::backend(format!("window loop failed: {e}")))?;
        match app.error {
            Some(message) => Err(TwError::new(tw_core::ErrorCode::GuestUnavailable, message)),
            None => Ok(()),
        }
    }

    async fn frame_loop(console: Arc<dyn InteractiveConsole>, pending: Arc<AtomicBool>, proxy: EventLoopProxy<UserEvent>) {
        let mut last = (0u64, 0u32, 0u32);
        let mut failures = 0u32;
        loop {
            tokio::time::sleep(FRAME_INTERVAL).await;
            if pending.load(Ordering::Acquire) {
                continue;
            }
            match console.frame().await {
                Ok(frame) => {
                    failures = 0;
                    let hash = fnv1a(&frame.rgb);
                    if (hash, frame.width, frame.height) != last {
                        last = (hash, frame.width, frame.height);
                        pending.store(true, Ordering::Release);
                        if proxy.send_event(UserEvent::Frame(frame)).is_err() {
                            return;
                        }
                    }
                }
                Err(e) => {
                    failures += 1;
                    if failures >= 20 {
                        let _ = proxy.send_event(UserEvent::Failed(format!("lost the VM console: {}", e.message)));
                        return;
                    }
                }
            }
        }
    }

    fn fnv1a(data: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in data {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^ data.len() as u64
    }

    struct App {
        name: String,
        frame: RawFrame,
        pending: Arc<AtomicBool>,
        tx: mpsc::Sender<Job>,
        window: Option<Arc<Window>>,
        surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
        cursor: (f64, f64),
        pressed: HashSet<&'static str>,
        dragging: bool,
        error: Option<String>,
        debug: bool,
    }

    impl App {
        fn frame_size(&self) -> (u32, u32) {
            (self.frame.width, self.frame.height)
        }

        fn rect(&self) -> Option<Rect> {
            let window = self.window.as_ref()?;
            let size = window.inner_size();
            Some(fit(self.frame_size(), (size.width, size.height)))
        }

        fn send_pointer(&self, button: Option<(MouseButton, bool)>, clamp: bool) {
            let Some(rect) = self.rect() else { return };
            let size = self.frame_size();
            let Some((x, y)) = to_guest(self.cursor.0, self.cursor.1, &rect, size, clamp) else { return };
            let job = match button {
                Some((button, down)) => Job::Button { x, y, size, button, down },
                None => Job::Move { x, y, size },
            };
            if self.debug {
                eprintln!("input: {job:?} (window position {:.0},{:.0})", self.cursor.0, self.cursor.1);
            }
            let _ = self.tx.try_send(job);
        }

        fn send_key(&self, qcode: &'static str, down: bool) {
            // Key events are never dropped: a lost release would leave a key stuck in the guest.
            if self.debug {
                eprintln!("input: key {qcode} {}", if down { "down" } else { "up" });
            }
            let _ = self.tx.blocking_send(Job::Key { qcode, down });
        }

        fn release_all_keys(&mut self) {
            let held: Vec<&'static str> = self.pressed.drain().collect();
            for code in held {
                self.send_key(code, false);
            }
        }

        fn draw(&mut self) {
            let (Some(window), Some(surface)) = (self.window.as_ref(), self.surface.as_mut()) else { return };
            let size = window.inner_size();
            let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
            if surface.resize(w, h).is_err() {
                return;
            }
            let Ok(mut buffer) = surface.buffer_mut() else { return };
            let rect = fit((self.frame.width, self.frame.height), (size.width, size.height));
            blit(&self.frame.rgb, (self.frame.width, self.frame.height), &mut buffer, size.width, size.height, &rect, BACKGROUND);
            let _ = buffer.present();
        }
    }

    impl ApplicationHandler<UserEvent> for App {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let attributes = Window::default_attributes()
                .with_title(format!("terminal-windows · {}", self.name))
                .with_inner_size(LogicalSize::new(f64::from(self.frame.width.min(1600)), f64::from(self.frame.height.min(1000))))
                .with_active(std::env::var_os(NO_ACTIVATE_ENV).is_none());
            let window = match event_loop.create_window(attributes) {
                Ok(w) => Arc::new(w),
                Err(e) => {
                    self.error = Some(format!("cannot create the window: {e}"));
                    event_loop.exit();
                    return;
                }
            };
            let surface = softbuffer::Context::new(window.clone()).and_then(|ctx| softbuffer::Surface::new(&ctx, window.clone()));
            match surface {
                Ok(s) => self.surface = Some(s),
                Err(e) => {
                    self.error = Some(format!("cannot create the drawing surface: {e}"));
                    event_loop.exit();
                    return;
                }
            }
            window.request_redraw();
            self.window = Some(window);
        }

        fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
            match event {
                UserEvent::Frame(frame) => {
                    self.frame = frame;
                    self.pending.store(false, Ordering::Release);
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                UserEvent::Failed(message) => {
                    self.error = Some(message);
                    event_loop.exit();
                }
            }
        }

        fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
            match event {
                WindowEvent::CloseRequested => {
                    self.release_all_keys();
                    event_loop.exit();
                }
                WindowEvent::RedrawRequested => self.draw(),
                WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                WindowEvent::Focused(false) => {
                    self.release_all_keys();
                    if self.dragging {
                        self.dragging = false;
                        self.send_pointer(Some((MouseButton::Left, false)), true);
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    self.cursor = (position.x, position.y);
                    self.send_pointer(None, self.dragging);
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    let button = match button {
                        WinitButton::Left => MouseButton::Left,
                        WinitButton::Right => MouseButton::Right,
                        WinitButton::Middle => MouseButton::Middle,
                        _ => return,
                    };
                    let down = state == ElementState::Pressed;
                    if button == MouseButton::Left {
                        self.dragging = down;
                    }
                    self.send_pointer(Some((button, down)), !down);
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let dy = match delta {
                        MouseScrollDelta::LineDelta(_, y) => f64::from(y),
                        MouseScrollDelta::PixelDelta(p) => p.y / 40.0,
                    };
                    let notches = dy.abs().round().max(if dy != 0.0 { 1.0 } else { 0.0 }) as u32;
                    for _ in 0..notches.min(10) {
                        let _ = self.tx.try_send(Job::Wheel { up: dy > 0.0 });
                    }
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    let PhysicalKey::Code(code) = event.physical_key else { return };
                    let Some(qcode) = qcode(code) else { return };
                    if event.repeat {
                        return; // the guest repeats keys itself
                    }
                    if qcode == "caps_lock" {
                        // macOS reports Caps Lock only when its state changes: always send a tap.
                        self.send_key(qcode, true);
                        self.send_key(qcode, false);
                        return;
                    }
                    let down = event.state == ElementState::Pressed;
                    if down {
                        self.pressed.insert(qcode);
                    } else {
                        self.pressed.remove(qcode);
                    }
                    self.send_key(qcode, down);
                }
                _ => {}
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn coalescing_keeps_order_and_only_the_last_move() {
            let size = (800, 600);
            let jobs = vec![
                Job::Move { x: 1, y: 1, size },
                Job::Move { x: 2, y: 2, size },
                Job::Button { x: 2, y: 2, size, button: MouseButton::Left, down: true },
                Job::Move { x: 3, y: 3, size },
                Job::Move { x: 4, y: 4, size },
                Job::Key { qcode: "a", down: true },
                Job::Move { x: 5, y: 5, size },
            ];
            assert_eq!(
                coalesce(jobs),
                vec![
                    Job::Move { x: 2, y: 2, size },
                    Job::Button { x: 2, y: 2, size, button: MouseButton::Left, down: true },
                    Job::Move { x: 4, y: 4, size },
                    Job::Key { qcode: "a", down: true },
                    Job::Move { x: 5, y: 5, size },
                ]
            );
        }

        #[test]
        fn key_map_covers_what_windows_needs() {
            assert_eq!(qcode(KeyCode::KeyA), Some("a"));
            assert_eq!(qcode(KeyCode::Enter), Some("ret"));
            assert_eq!(qcode(KeyCode::SuperLeft), Some("meta_l"), "Command is the Windows key");
            assert_eq!(qcode(KeyCode::AltRight), Some("alt_r"), "Hangul toggle on 101-key layouts");
            assert_eq!(qcode(KeyCode::Lang1), Some("lang1"));
            assert_eq!(qcode(KeyCode::F12), Some("f12"));
            assert_eq!(qcode(KeyCode::Numpad7), Some("kp_7"));
        }

        #[test]
        fn every_qcode_is_valid_for_the_console() {
            // The console accepts [a-z0-9_] up to 20 characters.
            for code in [
                KeyCode::KeyA,
                KeyCode::Digit0,
                KeyCode::Minus,
                KeyCode::BracketLeft,
                KeyCode::Backquote,
                KeyCode::NumpadSubtract,
                KeyCode::ControlRight,
                KeyCode::ScrollLock,
                KeyCode::IntlBackslash,
            ] {
                let q = qcode(code).unwrap();
                assert!(q.len() <= 20 && q.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'), "{q}");
            }
        }

        #[test]
        fn frame_hash_detects_changes() {
            let mut a = vec![0u8; 4096];
            let before = fnv1a(&a);
            a[0] = 1;
            assert_ne!(before, fnv1a(&a));
            assert_ne!(fnv1a(&a[..2048]), fnv1a(&a));
            a[4095] = 9; // a change in the very last byte is detected too
            assert_ne!(fnv1a(&a), before);
        }
    }
}
