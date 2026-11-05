//! Core of terminal-windows: typed VM configuration, persistent storage, error
//! model and the abstractions shared by the CLI, MCP server and viewer.
//!
//! Nothing in this crate talks to a hypervisor directly; backends live in their
//! own crates (`tw-qemu`, later `tw-hyperv`) and implement [`backend::VirtualizationBackend`].

pub mod backend;
pub mod control;
pub mod coords;
pub mod error;
pub mod host;
pub mod model;
pub mod name;
pub mod secure;
pub mod service;
pub mod store;

pub use error::{ErrorCode, TwError, TwResult};
pub use name::VmName;
