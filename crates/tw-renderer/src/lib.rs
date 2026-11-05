//! Terminal graphics for the desktop viewer.
//!
//! * [`layout`] — fit a guest frame into terminal cells and map cells back to
//!   guest pixels (ported from upstream `TerminalMetrics.swift`, MIT).
//! * [`metrics`] — strip terminal replies (cell size, Kitty/DA1 answers) out of
//!   the input stream before keystrokes reach the guest (ported, extended with DA1).
//! * [`kitty`] — Kitty graphics protocol encoder with double buffering.
//! * [`detect`] — emulator-level capability detection. Environment variables
//!   only nominate a candidate; the answer to a live query decides.

pub mod detect;
pub mod kitty;
pub mod layout;
pub mod metrics;
