//! Windows installation media, obtained only from Microsoft.
//!
//! terminal-mac downloads macOS restore images from Apple's public catalog.
//! Microsoft offers no public API for Windows ISOs; its download page calls a
//! connector API that is guarded by bot detection ("Sentinel"). This crate:
//!
//! 1. reads the official page (edition id, version, published SHA-256 per language),
//! 2. asks the same connector API the page uses for a download link,
//! 3. if Microsoft grants one, downloads it with resume support;
//!    if Microsoft rejects the request, it does **not** try to defeat the bot
//!    detection — the caller opens the official page in the user's browser and
//!    [`watch`] picks up the finished download,
//! 4. verifies every ISO against Microsoft's published hash before it is cached.

pub mod cache;
pub mod download;
pub mod microsoft;
pub mod verify;
pub mod watch;
