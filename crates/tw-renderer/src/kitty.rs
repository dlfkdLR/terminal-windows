//! Kitty graphics protocol encoder.
//!
//! Frames alternate between two image ids so the new image is placed before
//! the old one is deleted (no flicker) — the same double-buffering scheme as
//! upstream `DesktopRenderer.swift`. This module emits PNG payloads through the
//! pty (`f=100`), which works on every Kitty-protocol terminal including WezTerm
//! on Windows. The POSIX shared-memory path (`t=s`) is a later optimisation for
//! Kitty/Ghostty on macOS and is not available on Windows.

use base64::Engine;

const CHUNK: usize = 4096;
/// Below cell backgrounds, so overlays drawn with a background stay visible.
const Z_INDEX: i64 = -1_073_741_825;

#[derive(Debug, Default)]
pub struct KittyEncoder {
    current: u32,
}

impl KittyEncoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Escape sequence that places `png` over `columns`×`rows` cells at the cursor
    /// and removes the previous frame.
    pub fn frame(&mut self, png: &[u8], columns: u32, rows: u32) -> String {
        let next = if self.current == 2 { 3 } else { 2 };
        let b64 = base64::engine::general_purpose::STANDARD.encode(png);
        let bytes = b64.as_bytes();
        let mut out = String::with_capacity(b64.len() + b64.len() / CHUNK * 16 + 128);
        let mut offset = 0;
        loop {
            let end = (offset + CHUNK).min(bytes.len());
            let more = u8::from(end < bytes.len());
            out.push_str("\x1b_G");
            if offset == 0 {
                out.push_str(&format!("a=T,f=100,i={next},q=2,C=1,c={columns},r={rows},z={Z_INDEX},"));
            }
            out.push_str(&format!("m={more};"));
            out.push_str(std::str::from_utf8(&bytes[offset..end]).expect("base64 is ASCII"));
            out.push_str("\x1b\\");
            offset = end;
            if offset >= bytes.len() {
                break;
            }
        }
        if self.current != 0 {
            out.push_str(&delete(self.current));
        }
        self.current = next;
        out
    }

    /// Removes any image this encoder placed. Call before leaving the alternate screen.
    pub fn clear(&mut self) -> String {
        self.current = 0;
        format!("{}{}", delete(2), delete(3))
    }
}

fn delete(id: u32) -> String {
    format!("\x1b_Ga=d,d=I,i={id},q=2;\x1b\\")
}

/// Query whose `OK` reply proves the terminal implements Kitty graphics.
pub fn support_query() -> &'static str {
    "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_and_double_buffers() {
        let mut enc = KittyEncoder::new();
        let png = vec![0u8; 10_000];
        let first = enc.frame(&png, 80, 24);
        assert!(first.starts_with("\x1b_Ga=T,f=100,i=2,"));
        assert_eq!(first.matches("\x1b_G").count(), 4, "13336 base64 bytes → 4 chunks");
        assert!(first.contains("m=0;"));
        assert!(!first.contains("a=d"), "nothing to delete on the first frame");
        let second = enc.frame(&png, 80, 24);
        assert!(second.starts_with("\x1b_Ga=T,f=100,i=3,"));
        assert!(second.ends_with("\x1b_Ga=d,d=I,i=2,q=2;\x1b\\"));
        assert_eq!(enc.clear().matches("a=d").count(), 2);
    }
}
