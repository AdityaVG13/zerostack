//! FSZero MCP tool catalog (per-op surface, fszero-ncib.5).
//!
//! Catalog schemas are owned by `contracts/operation-abi-schemas-v1.json` and
//! materialized through the operation ABI (fszero-ncib.1). Handlers validate
//! once, invoke the typed dispatcher once (`dispatch_mcp_tool`), and serialize
//! once — no plan execution, sandbox startup, or CodeMode tool exposure.

use crate::core::dispatcher::opcode_for_operation;
use crate::core::operation_abi::resolve_alias;
use crate::core::operation_schemas::materialize_mcp_tools;
use crate::core::parse_exec_opcode;
use serde_json::Value;

/// Live MCP tool catalog — exact materialization of the canonical schema doc.
pub fn mcp_tools() -> Vec<Value> {
    materialize_mcp_tools()
}

/// Map an MCP tool name (+ args for `fszero.exec`) to a CLI opcode when one exists.
///
/// Derived from the operation ABI registry aliases — not a parallel hand table.
/// `fszero.exec` uses [`parse_exec_opcode`] so word mistakes like `write` do not
/// silently map to W (world).
pub fn mcp_tool_opcode(name: &str, args: &serde_json::Value) -> Option<char> {
    if name == "fszero.exec" {
        return parse_exec_opcode(args.get("code")?.as_str()?).ok();
    }
    let op_id = resolve_alias("mcp", name)?;
    opcode_for_operation(op_id)
}

pub fn tool_names(tools: &[Value]) -> Vec<&str> {
    tools
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .collect()
}

/// True when the catalog is a pure FastMCP per-op surface (no CodeMode tools).
pub fn mcp_catalog_is_raw_endpoint(tools: &[Value]) -> bool {
    let names = tool_names(tools);
    !names.is_empty()
        && names.iter().all(|n| n.starts_with("fszero."))
        && !names
            .iter()
            .any(|n| n.starts_with("fz_") || n.contains("codemode") || *n == "fz_execute_code")
}
