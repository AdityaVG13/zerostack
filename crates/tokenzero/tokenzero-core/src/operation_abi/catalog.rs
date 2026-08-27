//! Catalog views derived from the operation registry.

use super::registry::{all_operations, operation_by_name};
use super::types::Operation;

/// Resolve a canonical name or alias to the registry operation.
pub fn resolve_operation(name: &str) -> Option<&'static Operation> {
    if let Some(op) = operation_by_name(name) {
        return Some(op);
    }
    // bare form without tz_ prefix
    let with_prefix = if name.starts_with("tz_") {
        None
    } else {
        Some(format!("tz_{name}"))
    };
    if let Some(prefixed) = with_prefix.as_deref()
        && let Some(op) = operation_by_name(prefixed)
    {
        return Some(op);
    }
    for op in all_operations() {
        if op.aliases.contains(&name) {
            return Some(op);
        }
        if let Some(binding) = op.exposure.codemode_binding
            && binding == name
        {
            return Some(op);
        }
        // zero.expand is alias of zero.token.expand
        if name == "zero.expand" && op.name == "tz_expand" {
            return Some(op);
        }
        if name == "zero.compact" && op.name == "zero.token.compact" {
            return Some(op);
        }
    }
    None
}

fn names_where(pred: impl Fn(&Operation) -> bool) -> Vec<&'static str> {
    all_operations()
        .iter()
        .filter(|op| pred(op))
        .map(|op| op.name)
        .collect()
}

/// Canonical FastMCP tool names (classic mcp surface).
pub fn fastmcp_tool_names() -> Vec<&'static str> {
    names_where(|op| op.exposure.fastmcp_tool)
}

/// Aggregate-host control schemas retained for ZeroStack registration metadata.
pub fn codemode_mcp_tool_names() -> Vec<&'static str> {
    names_where(|op| op.exposure.codemode_mcp_tool)
}

/// All CodeMode binding paths registered in the ABI.
pub fn codemode_binding_names() -> Vec<&'static str> {
    all_operations()
        .iter()
        .filter_map(|op| op.exposure.codemode_binding)
        .collect()
}

/// Resource URIs from the registry.
pub fn resource_uris() -> Vec<&'static str> {
    all_operations()
        .iter()
        .filter_map(|op| op.exposure.resource_uri)
        .collect()
}

/// Input schema for a canonical FastMCP tool (owned Value clone).
pub fn input_schema_for(name: &str) -> Option<serde_json::Value> {
    resolve_operation(name).map(|op| op.args.schema.clone())
}

/// Output schema for a canonical operation.
pub fn output_schema_for(name: &str) -> Option<serde_json::Value> {
    resolve_operation(name).map(|op| op.results.schema.clone())
}
