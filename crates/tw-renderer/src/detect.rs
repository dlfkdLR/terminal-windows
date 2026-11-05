//! Terminal emulator capability detection.
//!
//! The shell (PowerShell, zsh) is not the emulator; graphics support is a
//! property of the emulator (Kitty, Ghostty, WezTerm, Windows Terminal …).
//! Environment variables are only hints. The viewer sends
//! [`probe_sequence`] and decides from the replies captured by
//! [`crate::metrics::MetricFilter`].

use crate::metrics::MetricFilter;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Graphics {
    /// Kitty graphics protocol, verified by an `OK` reply.
    Kitty,
    /// DEC Sixel, verified by DA1 attribute 4. Slow; low frame rate mode.
    Sixel,
    /// No verified inline graphics: use the native window fallback.
    None,
}

/// Emulator guessed from the environment, for messages only.
pub fn emulator_hint(env: &HashMap<String, String>) -> &'static str {
    let get = |k: &str| env.get(k).map(String::as_str);
    if get("KITTY_WINDOW_ID").is_some() || get("TERM").is_some_and(|t| t.contains("kitty")) {
        "kitty"
    } else if get("TERM_PROGRAM") == Some("ghostty") {
        "ghostty"
    } else if get("TERM_PROGRAM") == Some("WezTerm") {
        "wezterm"
    } else if get("WT_SESSION").is_some() {
        "windows-terminal"
    } else if get("TERM_PROGRAM") == Some("iTerm.app") {
        "iterm2"
    } else {
        "unknown"
    }
}

/// Inside tmux/screen the queries would be answered (or swallowed) by the
/// multiplexer, so graphics are not attempted.
pub fn inside_multiplexer(env: &HashMap<String, String>) -> bool {
    env.contains_key("TMUX") || env.get("TERM").is_some_and(|t| t.starts_with("screen"))
}

/// Kitty support query, cell-size and window-size reports, then DA1. DA1 is
/// answered by virtually every terminal, so its reply marks the end of probing.
pub fn probe_sequence() -> String {
    format!("{}\x1b[16t\x1b[14t\x1b[c", crate::kitty::support_query())
}

/// Decision after the DA1 reply arrived (or the probe timed out).
pub fn decide(filter: &MetricFilter) -> Graphics {
    if filter.graphics_replies.get(&31).is_some_and(|r| r == "OK") {
        Graphics::Kitty
    } else if filter.sixel() {
        Graphics::Sixel
    } else {
        Graphics::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn hints_are_not_decisions() {
        assert_eq!(emulator_hint(&env(&[("WT_SESSION", "x")])), "windows-terminal");
        assert_eq!(emulator_hint(&env(&[("TERM_PROGRAM", "ghostty")])), "ghostty");
        assert!(inside_multiplexer(&env(&[("TMUX", "/tmp/x")])));
        // No replies at all → no graphics, regardless of hints.
        assert_eq!(decide(&MetricFilter::new()), Graphics::None);
    }

    #[test]
    fn replies_decide() {
        let mut f = MetricFilter::new();
        f.feed(b"\x1b[?61;6;7;22;23;24;28;32;42c", false);
        assert_eq!(decide(&f), Graphics::None);
        f.feed(b"\x1b[?61;4;6c", false);
        assert_eq!(decide(&f), Graphics::Sixel);
        f.feed(b"\x1b_Gi=31;OK\x1b\\", false);
        assert_eq!(decide(&f), Graphics::Kitty);
    }
}
