//! Lean GraphZero MCP tool catalog (RACC: refs first, expand on demand).
//!
//! Catalog entries are derived from `graphzero_engine::operation_abi` so FastMCP
//! and CodeMode cannot drift from the canonical operation registry
//! (graphzero-o2uq.1). Full structural I/O schema parity is enforced in tests.

use serde_json::Value;

use graphzero_engine::operation_abi::{lean_fastmcp_tools_from_registry, orient_surface_names};

/// Maximum per-operation tools advertised on tools/list in --mode=mcp.
/// Referenced by the mcp.rs catalog-budget tests only.
#[cfg(test)]
pub const MCP_TOOL_BUDGET: usize = 10;

/// Surfaces routed through the single `orient` tool (not separate MCP entries).
/// Derived from the canonical registry / SURFACE_NAMES.
pub fn orient_surfaces() -> Vec<&'static str> {
    orient_surface_names()
}

/// Stable slice for call sites that need `&[&str]` equality with historical code.
pub const ORIENT_SURFACES: &[&str] = graphzero_engine::SURFACE_NAMES;

/// Lean FastMCP catalog -- single source of truth is operation_abi registry.
pub fn lean_tool_catalog() -> Vec<Value> {
    lean_fastmcp_tools_from_registry()
}
