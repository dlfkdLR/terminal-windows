//! `terminal-windows view NAME`: the guest desktop inside the terminal.
//!
//! Follows terminal-mac's viewer design (Kitty graphics, latest-frame-only
//! rendering, SGR mouse, Ctrl+] to leave, a shortcut palette), with one
//! difference: frames and input go through the hypervisor console, so the
//! viewer works during Windows Setup, at sign-in and on UAC prompts without
//! anything installed in the guest.

use crossterm::terminal;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tw_core::control::{InteractiveConsole, MouseButton, RawFrame};
use tw_core::service::VmService;
use tw_core::{ErrorCode, TwError, TwResult, VmName};
use tw_input::terminal::{TerminalEvent, TerminalInput};
use tw_renderer::kitty::{self, KittyEncoder};
use tw_renderer::layout::ImageLayout;
use tw_renderer::metrics::MetricFilter;

const FRAME_INTERVAL: Duration = Duration::from_millis(100);
const INPUT_QUEUE: usize = 64;

/// Windows shortcuts the terminal would otherwise swallow (Ctrl+G opens the palette).
pub const PALETTE: &[(&str, &str)] = &[
    ("win", "Start"),
    ("ctrl+alt+delete", "Ctrl+Alt+Del"),
    ("alt+tab", "Alt+Tab"),
    ("win+r", "Run"),
    ("win+e", "Explorer"),
    ("alt+f4", "Close"),
    ("ctrl+shift+esc", "Task Manager"),
    ("hangul", "Han/Eng"),
    ("win+d", "Desktop"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    /// Raw RGB through a temporary file (Kitty, Ghostty).
    TempFile,
    /// PNG chunks through the pty (other Kitty-protocol terminals).
    Direct,
}

enum InputJob {
    Pointer { x: u32, y: u32, size: (u32, u32), button: Option<(MouseButton, bool)> },
    Wheel { x: u32, y: u32, size: (u32, u32), up: bool },
    Text(String),
    Chord(String),
}

/// Maps an SGR mouse report to an input job. Pure, tested.
fn mouse_job(layout: &ImageLayout, button: u16, column: u16, row: u16, released: bool) -> Option<InputJob> {
    let size = (layout.width, layout.height);
    let motion = button & 32 != 0;
    if button & 64 != 0 {
        let (x, y) = layout.point(u32::from(column), u32::from(row), false)?;
        return Some(InputJob::Wheel { x, y, size, up: button & 1 == 0 });
    }
    let which = match button & 3 {
        0 => Some(MouseButton::Left),
        1 => Some(MouseButton::Middle),
        2 => Some(MouseButton::Right),
        _ => None,
    };
    // Presses must land on the desktop; drags and releases may leave it (clamped).
    let (x, y) = layout.point(u32::from(column), u32::from(row), motion || released)?;
    if motion {
        return Some(InputJob::Pointer { x, y, size, button: None });
    }
    Some(InputJob::Pointer { x, y, size, button: which.map(|b| (b, !released)) })
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> TwResult<Self> {
        terminal::enable_raw_mode().map_err(|e| TwError::io("raw mode", e))?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        remove_leftover_frames();
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x1b_Ga=d,d=I,i=2,q=2;\x1b\\\x1b_Ga=d,d=I,i=3,q=2;\x1b\\");
        let _ = out.write_all(b"\x1b[0m\x1b[2J\x1b[?1002l\x1b[?1006l\x1b[?25h\x1b[?1049l");
        let _ = out.flush();
        let _ = terminal::disable_raw_mode();
    }
}

/// Frame files the terminal never consumed (it deletes them itself when it does).
fn remove_leftover_frames() {
    let prefix = format!("tty-graphics-protocol-tw-{}-", std::process::id());
    for entry in std::fs::read_dir(std::env::temp_dir()).into_iter().flatten().flatten() {
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn write_out(s: &str) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(s.as_bytes());
    let _ = out.flush();
}

fn temp_path(tag: &str, seq: u64) -> PathBuf {
    std::env::temp_dir().join(format!("tty-graphics-protocol-tw-{}-{tag}-{seq}.rgb", std::process::id()))
}

fn layout_for(frame: (u32, u32)) -> Option<(ImageLayout, u16, u16)> {
    let ws = terminal::window_size().ok()?;
    let (cols, rows) = (u32::from(ws.columns.max(1)), u32::from(ws.rows.max(3)));
    let cw = if ws.width > 0 { f64::from(ws.width) / f64::from(cols) } else { 8.0 };
    let ch = if ws.height > 0 { f64::from(ws.height) / f64::from(rows) } else { 16.0 };
    Some((ImageLayout::fullscreen(frame.0, frame.1, cols, rows, cw, ch), ws.columns, ws.rows))
}

fn spawn_stdin_reader() -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel(256);
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

/// Asks the terminal what it supports. Environment variables are not trusted.
async fn probe(stdin: &mut mpsc::Receiver<Vec<u8>>, filter: &mut MetricFilter) -> TwResult<Transport> {
    let probe_file = temp_path("probe", 0);
    tw_core::secure::write_private_atomic(&probe_file, &[0u8, 0, 0])?;
    write_out(&format!("{}{}\x1b[c", kitty::temp_file_query(&probe_file), kitty::support_query()));
    let deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
    while filter.device_attributes.is_none() {
        match tokio::time::timeout_at(deadline, stdin.recv()).await {
            Ok(Some(bytes)) => {
                filter.feed(&bytes, false);
            }
            _ => break,
        }
    }
    let _ = std::fs::remove_file(&probe_file);
    if filter.graphics_replies.get(&32).is_some_and(|r| r == "OK") {
        Ok(Transport::TempFile)
    } else if filter.graphics_replies.get(&31).is_some_and(|r| r == "OK") {
        Ok(Transport::Direct)
    } else {
        Err(TwError::unsupported("this terminal did not confirm Kitty graphics support").with_hint(
            "Use Ghostty, Kitty or WezTerm, or keep using `terminal-windows action NAME screenshot`. tmux/screen block graphics.",
        ))
    }
}

fn spawn_input_worker(console: Arc<dyn InteractiveConsole>, status: mpsc::Sender<String>) -> mpsc::Sender<InputJob> {
    let (tx, mut rx) = mpsc::channel::<InputJob>(INPUT_QUEUE);
    tokio::spawn(async move {
        while let Some(job) = rx.recv().await {
            let result = match job {
                InputJob::Pointer { x, y, size, button } => {
                    let r = console.pointer(x, y, size).await;
                    match (r, button) {
                        (Ok(()), Some((b, down))) => console.button(b, down).await,
                        (r, _) => r,
                    }
                }
                InputJob::Wheel { x, y, size, up } => match console.pointer(x, y, size).await {
                    Ok(()) => console.wheel(up).await,
                    e => e,
                },
                InputJob::Text(t) => console.type_text(&t).await,
                InputJob::Chord(c) => console.key(&c).await.map(|_| ()),
            };
            if let Err(e) = result {
                let _ = status.send(e.message).await;
            }
        }
    });
    tx
}

pub async fn run(service: &VmService, name: &VmName) -> TwResult<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(TwError::invalid("view needs an interactive terminal"));
    }
    let console: Arc<dyn InteractiveConsole> = Arc::from(service.interactive_console(name).await?);
    let first = console.frame().await?;

    let _guard = TerminalGuard::enter()?;
    let mut stdin = spawn_stdin_reader();
    let mut filter = MetricFilter::new();
    let transport = probe(&mut stdin, &mut filter).await?;
    write_out("\x1b[?1049h\x1b[?25l\x1b[2J\x1b[?1002h\x1b[?1006h");

    let (status_tx, mut status_rx) = mpsc::channel::<String>(16);
    let input = spawn_input_worker(console.clone(), status_tx.clone());
    let mut parser = TerminalInput::new();
    let mut encoder = KittyEncoder::new();
    let mut ticker = tokio::time::interval(FRAME_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut frame: RawFrame = first;
    let mut last_hash = 0u64;
    let mut last_size = (0u16, 0u16);
    let mut layout = layout_for((frame.width, frame.height)).map(|l| l.0);
    let mut palette = false;
    let mut message = String::new();
    let mut seq = 0u64;
    let mut failures = 0u32;
    let mut force = true;

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                match console.frame().await {
                    Ok(f) => { frame = f; failures = 0; }
                    Err(e) => {
                        failures += 1;
                        message = e.message;
                        if failures >= 20 {
                            return Err(TwError::new(ErrorCode::GuestUnavailable, "lost the VM console (was the VM stopped?)"));
                        }
                    }
                }
                let Some((l, cols, rows)) = layout_for((frame.width, frame.height)) else { continue };
                if (cols, rows) != last_size {
                    last_size = (cols, rows);
                    write_out("\x1b[2J");
                    force = true;
                }
                let mut h = DefaultHasher::new();
                frame.rgb.hash(&mut h);
                (frame.width, frame.height).hash(&mut h);
                let hash = h.finish();
                if hash != last_hash || force {
                    last_hash = hash;
                    seq += 1;
                    let escape = match transport {
                        Transport::TempFile => {
                            let path = temp_path("frame", seq);
                            if tw_core::secure::write_private_atomic(&path, &frame.rgb).is_err() { continue; }
                            encoder.frame_temp_file(&path, frame.width, frame.height, l.columns, l.rows)
                        }
                        Transport::Direct => {
                            let Ok(png) = tw_qemu::image::encode_png_rgb(frame.width, frame.height, &frame.rgb) else { continue };
                            encoder.frame(&png, l.columns, l.rows)
                        }
                    };
                    write_out(&format!("\x1b[{};{}H{}", l.row, l.column, escape));
                }
                if force || hash != last_hash || !message.is_empty() || palette {
                    draw_status(name, &frame, transport, palette, &message, rows);
                }
                force = false;
                layout = Some(l);
            }
            Some(msg) = status_rx.recv() => { message = msg; force = true; }
            chunk = stdin.recv() => {
                let Some(bytes) = chunk else { break };
                let clean = filter.feed(&bytes, false);
                let mut events = parser.feed(&clean, false);
                if clean.last() == Some(&27) { events.extend(parser.feed(&[], true)); }
                for event in events {
                    match event {
                        TerminalEvent::Quit => return Ok(()),
                        TerminalEvent::Key(k) if k == "ctrl+g" => { palette = !palette; force = true; }
                        TerminalEvent::Key(k) if palette && k == "escape" => { palette = false; force = true; }
                        TerminalEvent::Text(t) if palette => {
                            if let Some((chord, label)) = t.parse::<usize>().ok().and_then(|n| PALETTE.get(n.wrapping_sub(1))) {
                                let _ = input.try_send(InputJob::Chord(chord.to_string()));
                                message = format!("sent {label}");
                            }
                            palette = false;
                            force = true;
                        }
                        TerminalEvent::Key(k) => {
                            if input.try_send(InputJob::Chord(k)).is_err() { message = "input is busy; wait a moment".into(); }
                        }
                        TerminalEvent::Text(t) => {
                            if !t.is_ascii() {
                                message = "non-ASCII text needs the guest agent; switch the guest IME with Ctrl+G, then 8 (Han/Eng key) and type on an English layout".into();
                                force = true;
                            } else if input.try_send(InputJob::Text(t)).is_err() {
                                message = "input is busy; wait a moment".into();
                            }
                        }
                        TerminalEvent::Mouse { button, column, row, released } => {
                            if let Some(job) = layout.as_ref().and_then(|l| mouse_job(l, button, column, row, released)) {
                                let _ = input.try_send(job);
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn draw_status(name: &VmName, frame: &RawFrame, transport: Transport, palette: bool, message: &str, rows: u16) {
    let line = if palette {
        PALETTE.iter().enumerate().map(|(i, (_, label))| format!("{} {label}", i + 1)).collect::<Vec<_>>().join("  ") + "   (Esc: close)"
    } else {
        let t = match transport {
            Transport::TempFile => "kitty/file",
            Transport::Direct => "kitty/png",
        };
        let mut s = format!("{name} · {}×{} · {t} · Ctrl+G shortcuts · Ctrl+] exit (VM keeps running)", frame.width, frame.height);
        if !message.is_empty() {
            s.push_str(" · ");
            s.push_str(message);
        }
        s
    };
    let style = if palette { "\x1b[7m" } else { "\x1b[2m" };
    write_out(&format!("\x1b[{rows};1H\x1b[2K{style}{line}\x1b[0m"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> ImageLayout {
        // 800x600 frame in a 100x40 terminal of 10x20 px cells → height-limited, 38 rows.
        ImageLayout::fullscreen(800, 600, 100, 40, 10.0, 20.0)
    }

    #[test]
    fn clicks_map_to_guest_pixels() {
        let l = layout();
        match mouse_job(&l, 0, l.column as u16, l.row as u16, false) {
            Some(InputJob::Pointer { x, y, size, button: Some((MouseButton::Left, true)) }) => {
                assert!(x < 15 && y < 15, "top-left cell maps near the origin: {x},{y}");
                assert_eq!(size, (800, 600));
            }
            _ => panic!("expected a left press"),
        }
        assert!(matches!(
            mouse_job(&l, 2, l.column as u16, l.row as u16, true),
            Some(InputJob::Pointer { button: Some((MouseButton::Right, false)), .. })
        ));
    }

    #[test]
    fn presses_outside_the_desktop_are_ignored_but_drags_clamp() {
        let l = layout();
        assert!(mouse_job(&l, 0, 1, 40, false).is_none(), "status row");
        assert!(matches!(mouse_job(&l, 32, 1, 40, false), Some(InputJob::Pointer { button: None, .. })));
    }

    #[test]
    fn wheel_reports() {
        let l = layout();
        let (c, r) = (l.column as u16 + 5, l.row as u16 + 5);
        assert!(matches!(mouse_job(&l, 64, c, r, false), Some(InputJob::Wheel { up: true, .. })));
        assert!(matches!(mouse_job(&l, 65, c, r, false), Some(InputJob::Wheel { up: false, .. })));
    }

    #[test]
    fn palette_chords_are_valid() {
        for (chord, _) in PALETTE {
            tw_input::keys::Chord::parse(chord).unwrap_or_else(|e| panic!("{chord}: {e}"));
        }
    }
}
