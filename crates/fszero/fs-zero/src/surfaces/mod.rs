//! Ship targets: MCP (per-op) vs CodeMode (plan execution).

pub mod codemode;
pub mod mcp;

pub use codemode::codemode_tools;
pub use mcp::{mcp_catalog_is_raw_endpoint, mcp_tool_opcode, mcp_tools, tool_names};
