//! Canonical JSON Schema compare for operation ABI parity.
//!
//! The structural machinery (normalization, canonical encoding, fingerprints,
//! diff) lives in the shared zero-abi foundation crate so TokenZero, FSZero,
//! and GraphZero can never drift on contract encoding. This module re-exports
//! it and keeps the TokenZero-specific parity assertion.

use serde_json::Value;

pub use zero_abi::schema::{
    canonical_json, canonical_schema_json, normalize_schema, schema_diff, schema_fingerprint_hex,
    schema_property_keys, schema_required_keys, schemas_structurally_equal,
};

/// Assert full I/O schema parity between a surface tool and a registry op.
/// Compares inputSchema / outputSchema when present on the tool object.
pub fn assert_tool_schema_parity(tool: &Value, op: &super::types::Operation) {
    if let Some(input) = tool.get("inputSchema").or_else(|| tool.get("input_schema")) {
        assert!(
            schemas_structurally_equal(input, &op.args.schema),
            "input schema drift for {}: {:?}",
            op.name,
            schema_diff(input, &op.args.schema)
        );
    }
    if let Some(output) = tool
        .get("outputSchema")
        .or_else(|| tool.get("output_schema"))
    {
        assert!(
            schemas_structurally_equal(output, &op.results.schema),
            "output schema drift for {}: {:?}",
            op.name,
            schema_diff(output, &op.results.schema)
        );
    }
}
