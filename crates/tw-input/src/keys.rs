//! Key chords and their device encodings.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Modifier {
    Ctrl,
    Shift,
    Alt,
    /// The Windows key. `cmd`, `meta` and `super` are accepted as aliases so
    /// agents trained on macOS shortcuts get a predictable result.
    Win,
}

impl Modifier {
    pub fn qcode(self) -> &'static str {
        match self {
            Modifier::Ctrl => "ctrl",
            Modifier::Shift => "shift",
            Modifier::Alt => "alt",
            Modifier::Win => "meta_l",
        }
    }
    pub fn vk(self) -> u16 {
        match self {
            Modifier::Ctrl => 0x11,
            Modifier::Shift => 0x10,
            Modifier::Alt => 0x12,
            Modifier::Win => 0x5B,
        }
    }
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "ctrl" | "control" => Modifier::Ctrl,
            "shift" => Modifier::Shift,
            "alt" | "option" | "opt" => Modifier::Alt,
            "win" | "windows" | "meta" | "super" | "cmd" | "command" => Modifier::Win,
            _ => return None,
        })
    }
}

/// A non-modifier key: (canonical name, QEMU qcode, Windows VK code).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub name: &'static str,
    pub qcode: &'static str,
    pub vk: u16,
}

macro_rules! keys {
    ($( $name:literal => ($qcode:literal, $vk:literal) $(, alias $($alias:literal)|+ )? );* $(;)?) => {
        const KEYS: &[(&[&str], Key)] = &[
            $( (&[$name $($(, $alias)+)?], Key { name: $name, qcode: $qcode, vk: $vk }) ),*
        ];
    };
}

keys! {
    "a" => ("a", 0x41); "b" => ("b", 0x42); "c" => ("c", 0x43); "d" => ("d", 0x44);
    "e" => ("e", 0x45); "f" => ("f", 0x46); "g" => ("g", 0x47); "h" => ("h", 0x48);
    "i" => ("i", 0x49); "j" => ("j", 0x4A); "k" => ("k", 0x4B); "l" => ("l", 0x4C);
    "m" => ("m", 0x4D); "n" => ("n", 0x4E); "o" => ("o", 0x4F); "p" => ("p", 0x50);
    "q" => ("q", 0x51); "r" => ("r", 0x52); "s" => ("s", 0x53); "t" => ("t", 0x54);
    "u" => ("u", 0x55); "v" => ("v", 0x56); "w" => ("w", 0x57); "x" => ("x", 0x58);
    "y" => ("y", 0x59); "z" => ("z", 0x5A);
    "0" => ("0", 0x30); "1" => ("1", 0x31); "2" => ("2", 0x32); "3" => ("3", 0x33);
    "4" => ("4", 0x34); "5" => ("5", 0x35); "6" => ("6", 0x36); "7" => ("7", 0x37);
    "8" => ("8", 0x38); "9" => ("9", 0x39);
    "return" => ("ret", 0x0D), alias "enter" | "ret";
    "space" => ("spc", 0x20), alias "spc";
    "tab" => ("tab", 0x09);
    "escape" => ("esc", 0x1B), alias "esc";
    "backspace" => ("backspace", 0x08);
    "delete" => ("delete", 0x2E), alias "del";
    "insert" => ("insert", 0x2D), alias "ins";
    "up" => ("up", 0x26); "down" => ("down", 0x28); "left" => ("left", 0x25); "right" => ("right", 0x27);
    "home" => ("home", 0x24); "end" => ("end", 0x23);
    "pageup" => ("pgup", 0x21), alias "pgup";
    "pagedown" => ("pgdn", 0x22), alias "pgdn";
    "f1" => ("f1", 0x70); "f2" => ("f2", 0x71); "f3" => ("f3", 0x72); "f4" => ("f4", 0x73);
    "f5" => ("f5", 0x74); "f6" => ("f6", 0x75); "f7" => ("f7", 0x76); "f8" => ("f8", 0x77);
    "f9" => ("f9", 0x78); "f10" => ("f10", 0x79); "f11" => ("f11", 0x7A); "f12" => ("f12", 0x7B);
    "printscreen" => ("print", 0x2C), alias "print" | "prtsc";
    "menu" => ("menu", 0x5D), alias "apps";
    "capslock" => ("caps_lock", 0x14);
    "minus" => ("minus", 0xBD), alias "-";
    "equal" => ("equal", 0xBB), alias "=" | "plus";
    "bracketleft" => ("bracket_left", 0xDB), alias "[";
    "bracketright" => ("bracket_right", 0xDD), alias "]";
    "backslash" => ("backslash", 0xDC), alias "\\";
    "semicolon" => ("semicolon", 0xBA), alias ";";
    "apostrophe" => ("apostrophe", 0xDE), alias "'";
    "grave" => ("grave_accent", 0xC0), alias "`";
    "comma" => ("comma", 0xBC), alias ",";
    "period" => ("dot", 0xBE), alias "." | "dot";
    "slash" => ("slash", 0xBF), alias "/";
    "hangul" => ("lang1", 0x15), alias "hanyeong" | "han_yeong";
    "hanja" => ("lang2", 0x19);
}

pub fn lookup(name: &str) -> Option<Key> {
    KEYS.iter().find(|(names, _)| names.contains(&name)).map(|(_, key)| *key)
}

/// Modifiers (deduplicated, in press order) plus at most one key. A chord with
/// only modifiers (for example `win`) presses and releases those modifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    pub modifiers: Vec<Modifier>,
    pub key: Option<Key>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChordError(pub String);

impl fmt::Display for ChordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ChordError {}

impl Chord {
    pub fn parse(input: &str) -> Result<Self, ChordError> {
        let lower = input.trim().to_ascii_lowercase();
        if lower.is_empty() || lower.len() > 64 {
            return Err(ChordError("key chord must be 1-64 characters, for example ctrl+shift+esc".into()));
        }
        // A trailing "+" is the plus key itself: "ctrl++".
        let (body, trailing_plus) = match lower.strip_suffix("++") {
            Some(rest) => (rest.to_string(), true),
            None if lower == "+" => (String::new(), true),
            None => (lower.clone(), false),
        };
        let mut parts: Vec<&str> = if body.is_empty() { Vec::new() } else { body.split('+').collect() };
        if parts.iter().any(|p| p.is_empty()) {
            return Err(ChordError(format!("malformed key chord {input:?}")));
        }
        let mut key_name = if trailing_plus { Some("plus") } else { None };
        if key_name.is_none() {
            if let Some(last) = parts.last() {
                if Modifier::parse(last).is_none() {
                    key_name = parts.pop();
                }
            }
        }
        let mut modifiers = Vec::new();
        for part in parts {
            let m = Modifier::parse(part)
                .ok_or_else(|| ChordError(format!("unknown modifier {part:?} in {input:?}; use ctrl, shift, alt or win")))?;
            if !modifiers.contains(&m) {
                modifiers.push(m);
            }
        }
        let key = match key_name {
            Some(name) => {
                let mut key = lookup(name).ok_or_else(|| ChordError(format!("unknown key {name:?} in {input:?}")))?;
                if name == "plus" {
                    // US layout: '+' is shift+'='.
                    key = lookup("equal").expect("equal key");
                    if !modifiers.contains(&Modifier::Shift) {
                        modifiers.push(Modifier::Shift);
                    }
                }
                Some(key)
            }
            None => None,
        };
        if key.is_none() && modifiers.is_empty() {
            return Err(ChordError("empty key chord".into()));
        }
        Ok(Self { modifiers, key })
    }

    /// QEMU qcodes in press order; release in reverse.
    pub fn qcodes(&self) -> Vec<&'static str> {
        self.modifiers.iter().map(|m| m.qcode()).chain(self.key.map(|k| k.qcode)).collect()
    }

    /// Windows virtual-key codes in press order; release in reverse.
    pub fn vks(&self) -> Vec<u16> {
        self.modifiers.iter().map(|m| m.vk()).chain(self.key.map(|k| k.vk)).collect()
    }
}

/// Maps printable US-ASCII to the physical key and shift state on a US
/// keyboard layout. Used by the hypervisor console, which can only press
/// physical keys; the guest's active layout decides the resulting character.
pub fn us_ascii_key(c: char) -> Option<(Key, bool)> {
    let (name, shift): (&str, bool) = match c {
        'a'..='z' | '0'..='9' => return lookup(c.encode_utf8(&mut [0; 4])).map(|k| (k, false)),
        'A'..='Z' => return lookup(c.to_ascii_lowercase().encode_utf8(&mut [0; 4])).map(|k| (k, true)),
        ' ' => ("space", false),
        '\n' => ("return", false),
        '\t' => ("tab", false),
        '-' => ("minus", false),
        '_' => ("minus", true),
        '=' => ("equal", false),
        '+' => ("equal", true),
        '[' => ("bracketleft", false),
        '{' => ("bracketleft", true),
        ']' => ("bracketright", false),
        '}' => ("bracketright", true),
        '\\' => ("backslash", false),
        '|' => ("backslash", true),
        ';' => ("semicolon", false),
        ':' => ("semicolon", true),
        '\'' => ("apostrophe", false),
        '"' => ("apostrophe", true),
        '`' => ("grave", false),
        '~' => ("grave", true),
        ',' => ("comma", false),
        '<' => ("comma", true),
        '.' => ("period", false),
        '>' => ("period", true),
        '/' => ("slash", false),
        '?' => ("slash", true),
        '!' => ("1", true),
        '@' => ("2", true),
        '#' => ("3", true),
        '$' => ("4", true),
        '%' => ("5", true),
        '^' => ("6", true),
        '&' => ("7", true),
        '*' => ("8", true),
        '(' => ("9", true),
        ')' => ("0", true),
        _ => return None,
    };
    lookup(name).map(|k| (k, shift))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_windows_chords() {
        let c = Chord::parse("ctrl+shift+esc").unwrap();
        assert_eq!(c.qcodes(), vec!["ctrl", "shift", "esc"]);
        assert_eq!(c.vks(), vec![0x11, 0x10, 0x1B]);
        assert_eq!(Chord::parse("Win+R").unwrap().qcodes(), vec!["meta_l", "r"]);
        assert_eq!(Chord::parse("cmd+space").unwrap().qcodes(), vec!["meta_l", "spc"]);
        assert_eq!(Chord::parse("return").unwrap().vks(), vec![0x0D]);
        assert_eq!(Chord::parse("alt+f4").unwrap().qcodes(), vec!["alt", "f4"]);
        assert_eq!(Chord::parse("win").unwrap().qcodes(), vec!["meta_l"]);
        assert_eq!(Chord::parse("ctrl+alt+delete").unwrap().qcodes(), vec!["ctrl", "alt", "delete"]);
        assert_eq!(Chord::parse("hangul").unwrap().qcodes(), vec!["lang1"]);
    }

    #[test]
    fn plus_and_duplicates() {
        assert_eq!(Chord::parse("ctrl++").unwrap().qcodes(), vec!["ctrl", "shift", "equal"]);
        assert_eq!(Chord::parse("ctrl+ctrl+a").unwrap().qcodes(), vec!["ctrl", "a"]);
    }

    #[test]
    fn rejects_garbage() {
        for bad in ["", "ctrl+", "+a", "hyper+a", "ctrl+nosuchkey", "a+b", &"a".repeat(65)] {
            assert!(Chord::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn ascii_mapping_covers_printable_range() {
        for c in (0x20u8..0x7f).map(char::from) {
            assert!(us_ascii_key(c).is_some(), "{c:?} unmapped");
        }
        assert_eq!(us_ascii_key('A').unwrap(), (lookup("a").unwrap(), true));
        assert_eq!(us_ascii_key('?').unwrap(), (lookup("slash").unwrap(), true));
        assert!(us_ascii_key('é').is_none());
        assert!(us_ascii_key('한').is_none());
    }
}
