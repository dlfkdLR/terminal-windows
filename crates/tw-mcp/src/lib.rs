//! MCP server over stdio (JSON-RPC 2.0, one message per line).
//!
//! Every tool goes through [`Target`], implemented for [`tw_core::service::VmService`],
//! so the MCP server shares lifecycle logic with the CLI. Tool results report
//! completion only after the hypervisor/agent acknowledged the action.

mod schema;
mod server;

pub use server::{serve, Server, Target, SUPPORTED_PROTOCOL_VERSIONS};
