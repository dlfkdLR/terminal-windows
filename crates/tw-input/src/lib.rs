//! Input handling shared by the viewer, the hypervisor console and the guest agent.
//!
//! * [`terminal`] parses raw terminal bytes (keys, SGR mouse) — ported from
//!   upstream terminal-mac `TerminalInput.swift` (MIT, see docs/PROVENANCE.md).
//! * [`keys`] parses chords like `ctrl+shift+esc` and maps them to QEMU qcodes
//!   and Windows virtual-key codes.

pub mod keys;
pub mod terminal;
