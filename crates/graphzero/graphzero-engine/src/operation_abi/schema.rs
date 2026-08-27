//! Canonical JSON Schema compare for operation ABI parity.
//!
//! The structural machinery (normalization, canonical encoding, fingerprints,
//! diff) lives in the shared zero-abi foundation crate so TokenZero, FSZero,
//! and GraphZero can never drift on contract encoding. This module re-exports
//! it and keeps the GraphZero-specific parity assertion.

use serde_json::Value;

pub use zero_abi::schema::{
    canonical_schema_json, normalize_schema, schema_diff, schema_fingerprint_hex,
    schemas_structurally_equal,
};

/// Assert full I/O schema parity between a surface tool and a registry op.
/// Compares `inputSchema` / `outputSchema` when present on the tool object.
pub fn assert_tool_schema_parity(tool: &Value, op: &super::types::Operation) -> Result<(), String> {
    let name = tool
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("<unnamed>");
    let input = tool
        .get("inputSchema")
        .ok_or_else(|| format!("{name}: missing inputSchema"))?;
    if !schemas_structurally_equal(input, &op.args.schema) {
        return Err(format!(
            "{name}: inputSchema drift: {}",
            schema_diff(input, &op.args.schema).unwrap_or_default()
        ));
    }
    if let Some(out) = tool.get("outputSchema") {
        if !schemas_structurally_equal(out, &op.results.schema) {
            return Err(format!(
                "{name}: outputSchema drift: {}",
                schema_diff(out, &op.results.schema).unwrap_or_default()
            ));
        }
    } else {
        return Err(format!(
            "{name}: missing outputSchema (registry owns complete I/O schemas)"
        ));
    }
    Ok(())
}
