//! Catalog views derived from the canonical operation registry.

use serde_json::{Value, json};

use super::registry::all_operations;
use super::types::{MigrationStatus, Operation};

/// Resolve a caller-facing name or alias to the canonical operation.
pub fn resolve_operation(name: &str) -> Option<&'static Operation> {
    let ops = all_operations();
    if let Some(op) = ops.iter().find(|op| op.name == name) {
        return Some(op);
    }
    // Alias match (including FastMCP legacy spellings).
    if let Some(op) = ops.iter().find(|op| op.aliases.contains(&name)) {
        return Some(op);
    }
    // CodeMode binding path exact match.
    if let Some(op) = ops
        .iter()
        .find(|op| op.exposure.codemode_binding == Some(name))
    {
        return Some(op);
    }
    // Orient sub-surface bare name (e.g. "locate" → orient.locate) when not a top-level op.
    let orient_name = format!("orient.{name}");
    ops.iter().find(|op| op.name == orient_name)
}

/// Lean FastMCP tool names in registry order of appearance among exposed tools.
pub fn lean_fastmcp_tool_names() -> Vec<&'static str> {
    // Product order matches historical mcp_catalog (not alphabetical).
    const ORDER: &[&str] = &[
        "orient", "search", "snap", "remember", "recall", "expand", "index", "blast", "reserve",
        "verify",
    ];
    let set: std::collections::BTreeSet<_> = all_operations()
        .iter()
        .filter(|op| op.exposure.fastmcp_tool)
        .map(|op| op.name)
        .collect();
    ORDER.iter().copied().filter(|n| set.contains(n)).collect()
}

/// Build MCP tools/list entries from the registry for FastMCP lean mode.
pub fn lean_fastmcp_tools_from_registry() -> Vec<Value> {
    lean_fastmcp_tool_names()
        .into_iter()
        .filter_map(|name| {
            let op = all_operations().iter().find(|o| o.name == name)?;
            Some(tool_obj(op))
        })
        .collect()
}

/// CodeMode meta tool catalog (gz_execute_code, search, describe).
pub fn codemode_meta_tools_from_registry() -> Vec<Value> {
    const ORDER: &[&str] = &["execute_code", "codemode_search", "codemode_describe"];
    ORDER
        .iter()
        .filter_map(|name| {
            let op = all_operations()
                .iter()
                .find(|o| o.name == *name && o.exposure.codemode_meta)?;
            let mcp_name = op.aliases.first().copied().unwrap_or(op.name);
            Some(tool_obj_named(mcp_name, op))
        })
        .collect()
}

/// CodeMode binding names (`graph.*` / `ctx.*`).
pub fn codemode_binding_names() -> Vec<&'static str> {
    let mut names: Vec<_> = all_operations()
        .iter()
        .filter_map(|op| op.exposure.codemode_binding)
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// Orient sub-surface names (matches `SURFACE_NAMES`).
pub fn orient_surface_names() -> Vec<&'static str> {
    let mut names: Vec<_> = all_operations()
        .iter()
        .filter(|op| op.migration == MigrationStatus::OrientSubSurface)
        .filter_map(|op| op.name.strip_prefix("orient."))
        .collect();
    names.sort_unstable();
    names
}

/// Discovery hits for CodeMode `search` (stable subset of bindings + recipes).
pub fn codemode_discovery_hits() -> Vec<Value> {
    let mut hits = Vec::new();
    for op in all_operations() {
        if let Some(binding) = op.exposure.codemode_binding {
            hits.push(json!({
                "name": binding,
                "kind": if binding.starts_with("ctx.") { "context" } else { "method" },
                "mutable": matches!(op.mutability, super::types::Mutability::StoreOnly),
                "canonical": op.name,
                "description": op.description,
                "cost_class": format!("{:?}", op.cost_class).to_ascii_lowercase(),
                "ref_ownership": format!("{:?}", op.ref_ownership).to_ascii_lowercase(),
            }));
        }
    }
    hits.push(json!({
        "name": "recipes",
        "kind": "recipe",
        "examples": ["defs:alpha", "callers:beta", "blast:function_foo", "tests:alpha", "expand:gz://query/<id>"]
    }));
    hits
}

/// Schema property key set for set-equality tests (ignores description text drift).
pub fn schema_property_keys(schema: &Value) -> std::collections::BTreeSet<String> {
    schema
        .get("properties")
        .and_then(|p| p.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// Required-field set for set-equality tests.
pub fn schema_required_keys(schema: &Value) -> std::collections::BTreeSet<String> {
    schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn tool_obj(op: &Operation) -> Value {
    tool_obj_named(op.name, op)
}

fn tool_obj_named(name: &str, op: &Operation) -> Value {
    json!({
        "name": name,
        "description": op.description,
        "inputSchema": op.args.schema,
        "outputSchema": op.results.schema,
    })
}
