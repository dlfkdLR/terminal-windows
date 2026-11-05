//! Raw terminal byte stream → input events.
//!
//! Adapted from upstream terminal-mac `Sources/TerminalMacCore/TerminalInput.swift`
//! (Copyright (c) 2026 Govijr, MIT). Changes from upstream:
//! * SS3 sequences (`ESC O P`…) for F1–F4 and application-mode arrows, which
//!   Windows Terminal and many emulators send.
//! * CSI modifier parameters (`ESC [1;5A` = ctrl+up) and `ESC [n~` function keys.
//! * Bounded buffering for unterminated sequences (same 128-byte bound as upstream).

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    Text(String),
    /// A chord in [`crate::keys::Chord`] syntax, for example `ctrl+up`.
    Key(String),
    /// SGR (1006) mouse report. `column`/`row` are 1-based cells.
    Mouse {
        button: u16,
        column: u16,
        row: u16,
        released: bool,
    },
    /// Ctrl+] — leave the viewer, keep the VM running.
    Quit,
}

#[derive(Debug, Default)]
pub struct TerminalInput {
    bytes: Vec<u8>,
}

const MAX_PENDING: usize = 128;

fn modifier_prefix(param: Option<u32>) -> String {
    // xterm: 1 + (shift 1 | alt 2 | ctrl 4)
    let Some(p) = param.filter(|p| *p >= 2) else { return String::new() };
    let bits = p - 1;
    let mut out = String::new();
    if bits & 4 != 0 {
        out.push_str("ctrl+");
    }
    if bits & 2 != 0 {
        out.push_str("alt+");
    }
    if bits & 1 != 0 {
        out.push_str("shift+");
    }
    out
}

fn tilde_key(n: u32) -> Option<&'static str> {
    Some(match n {
        1 | 7 => "home",
        2 => "insert",
        3 => "delete",
        4 | 8 => "end",
        5 => "pageup",
        6 => "pagedown",
        11 => "f1",
        12 => "f2",
        13 => "f3",
        14 => "f4",
        15 => "f5",
        17 => "f6",
        18 => "f7",
        19 => "f8",
        20 => "f9",
        21 => "f10",
        23 => "f11",
        24 => "f12",
        _ => return None,
    })
}

fn final_key(byte: u8) -> Option<&'static str> {
    Some(match byte {
        b'A' => "up",
        b'B' => "down",
        b'C' => "right",
        b'D' => "left",
        b'H' => "home",
        b'F' => "end",
        b'P' => "f1",
        b'Q' => "f2",
        b'R' => "f3",
        b'S' => "f4",
        _ => return None,
    })
}

impl TerminalInput {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds bytes and returns complete events. With `flush_escape`, a lone
    /// pending ESC (no follow-up within the caller's timeout) becomes `escape`.
    pub fn feed(&mut self, input: &[u8], flush_escape: bool) -> Vec<TerminalEvent> {
        self.bytes.extend_from_slice(input);
        let mut events = Vec::new();
        while !self.bytes.is_empty() {
            let first = self.bytes[0];
            if first == 29 {
                self.bytes.remove(0);
                events.push(TerminalEvent::Quit);
                continue;
            }
            if first == 27 {
                if self.bytes.len() == 1 {
                    if flush_escape {
                        self.bytes.clear();
                        events.push(TerminalEvent::Key("escape".into()));
                    }
                    break;
                }
                match self.bytes[1] {
                    b'[' => {
                        let Some(end) = (2..self.bytes.len()).find(|&i| (64..=126).contains(&self.bytes[i])) else {
                            if self.bytes.len() > MAX_PENDING {
                                self.bytes.clear();
                            }
                            break;
                        };
                        let seq: Vec<u8> = self.bytes.drain(..=end).collect();
                        if let Some(event) = Self::csi(&seq) {
                            events.push(event);
                        }
                        continue;
                    }
                    b'O' => {
                        if self.bytes.len() < 3 {
                            break;
                        }
                        let code = self.bytes[2];
                        self.bytes.drain(..3);
                        if let Some(key) = final_key(code) {
                            events.push(TerminalEvent::Key(key.into()));
                        }
                        continue;
                    }
                    _ => {
                        self.bytes.remove(0);
                        events.push(TerminalEvent::Key("escape".into()));
                        continue;
                    }
                }
            }
            if first < 32 || first == 127 {
                self.bytes.remove(0);
                match first {
                    13 | 10 => events.push(TerminalEvent::Key("return".into())),
                    9 => events.push(TerminalEvent::Key("tab".into())),
                    127 | 8 => events.push(TerminalEvent::Key("backspace".into())),
                    1..=26 => events.push(TerminalEvent::Key(format!("ctrl+{}", char::from(first + 96)))),
                    _ => {}
                }
                continue;
            }
            let len = match first {
                0x00..=0x7f => 1,
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => 0,
            };
            if len == 0 {
                self.bytes.remove(0);
                continue;
            }
            if self.bytes.len() < len {
                break;
            }
            match std::str::from_utf8(&self.bytes[..len]) {
                Ok(text) => {
                    events.push(TerminalEvent::Text(text.to_string()));
                    self.bytes.drain(..len);
                }
                Err(_) => {
                    self.bytes.remove(0);
                }
            }
        }
        events
    }

    fn csi(seq: &[u8]) -> Option<TerminalEvent> {
        let final_byte = *seq.last()?;
        let body = std::str::from_utf8(&seq[2..seq.len() - 1]).ok()?;
        if let Some(sgr) = body.strip_prefix('<') {
            if final_byte != b'M' && final_byte != b'm' {
                return None;
            }
            let parts: Vec<u16> = sgr.split(';').filter_map(|p| p.parse().ok()).collect();
            if parts.len() == 3 && parts[1] > 0 && parts[2] > 0 {
                return Some(TerminalEvent::Mouse { button: parts[0], column: parts[1], row: parts[2], released: final_byte == b'm' });
            }
            return None;
        }
        if final_byte == b'Z' && body.is_empty() {
            return Some(TerminalEvent::Key("shift+tab".into()));
        }
        let params: Vec<Option<u32>> = body.split(';').map(|p| p.parse().ok()).collect();
        if final_byte == b'~' {
            let key = tilde_key(params.first().copied().flatten()?)?;
            return Some(TerminalEvent::Key(format!("{}{key}", modifier_prefix(params.get(1).copied().flatten()))));
        }
        let key = final_key(final_byte)?;
        Some(TerminalEvent::Key(format!("{}{key}", modifier_prefix(params.get(1).copied().flatten()))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TerminalEvent::*;

    fn key(k: &str) -> TerminalEvent {
        Key(k.into())
    }

    // Ported from upstream testInputHandlesFragmentedUnicodeArrowsMouseAndExit.
    #[test]
    fn fragmented_unicode_arrows_mouse_and_exit() {
        let mut p = TerminalInput::new();
        let emoji = "🌍".as_bytes();
        assert_eq!(p.feed(&emoji[..2], false), vec![]);
        assert_eq!(p.feed(&emoji[2..], false), vec![Text("🌍".into())]);
        assert_eq!(p.feed(&[27, 91], false), vec![]);
        assert_eq!(p.feed(&[65], false), vec![key("up")]);
        assert_eq!(p.feed(b"\x1b[<0;12;8M", false), vec![Mouse { button: 0, column: 12, row: 8, released: false }]);
        assert_eq!(p.feed(b"\x1b[<0;12;8m", false), vec![Mouse { button: 0, column: 12, row: 8, released: true }]);
        assert_eq!(p.feed(&[3, 29], false), vec![key("ctrl+c"), Quit]);
    }

    // Ported from upstream testEscapeAndUnknownSequencesDoNotBecomeGuestText.
    #[test]
    fn escape_and_unknown_sequences_do_not_become_text() {
        let mut p = TerminalInput::new();
        assert_eq!(p.feed(&[27], false), vec![]);
        assert_eq!(p.feed(&[], true), vec![key("escape")]);
        assert_eq!(p.feed(b"\x1b[99~", false), vec![]);
        assert_eq!(p.feed(b"\x1b[<0;0;0M", false), vec![]);
        assert_eq!(p.feed(&[255, 65], false), vec![Text("A".into())]);
    }

    #[test]
    fn modifiers_function_keys_and_ss3() {
        let mut p = TerminalInput::new();
        assert_eq!(p.feed(b"\x1b[1;5A", false), vec![key("ctrl+up")]);
        assert_eq!(p.feed(b"\x1b[3;2~", false), vec![key("shift+delete")]);
        assert_eq!(p.feed(b"\x1bOP\x1b[15~\x1b[24~", false), vec![key("f1"), key("f5"), key("f12")]);
        assert_eq!(p.feed(b"\x1b[Z", false), vec![key("shift+tab")]);
        assert_eq!(p.feed("한글".as_bytes(), false), vec![Text("한".into()), Text("글".into())]);
    }

    #[test]
    fn unterminated_sequences_are_bounded() {
        let mut p = TerminalInput::new();
        let mut junk = b"\x1b[".to_vec();
        junk.extend(std::iter::repeat_n(b'1', 200));
        assert_eq!(p.feed(&junk, false), vec![]);
        assert_eq!(p.feed(b"x", false), vec![Text("x".into())]);
    }
}
