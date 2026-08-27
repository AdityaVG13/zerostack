//! Contract tests for the canonical operation ABI (graphzero-o2uq.1).

use graphzero_engine::operation_abi::{
    DomainError, DomainErrorKind, DomainResult, SEMANTIC_CONTRACT_VERSION, all_operations,
    assert_tool_schema_parity, codemode_binding_names, contract_digest_hex, contract_manifest,
    golden_vectors, lean_fastmcp_tool_names, lean_fastmcp_tools_from_registry,
    orient_surface_names, resolve_operation, schema_diff, schema_fingerprint_hex,
    schema_property_keys, schema_required_keys, schemas_structurally_equal,
};
use graphzero_engine::{SURFACE_NAMES, codemode};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[test]
pub fn registry_is_non_empty_and_unique() {
    let ops = all_operations();
    assert!(
        ops.len() >= 20,
        "expected full inventory, got {}",
        ops.len()
    );
    let mut names = BTreeSet::new();
    for op in ops {
        assert!(names.insert(op.name), "duplicate {}", op.name);
    }
}

#[test]
fn contract_digest_stable_within_process() {
    let a = contract_digest_hex();
    let b = contract_digest_hex();
    assert_eq!(a, b);
    assert_eq!(a.len(), 64);
}

#[test]
fn semantic_version_published() {
    assert_eq!(SEMANTIC_CONTRACT_VERSION, "1.0.0");
}

#[test]
fn lean_fastmcp_is_exactly_ten_tools() {
    assert_eq!(lean_fastmcp_tool_names().len(), 10);
    assert_eq!(lean_fastmcp_tools_from_registry().len(), 10);
}

#[test]
fn codemode_discovery_set_equals_registry_bindings() {
    let raw = codemode::search("").expect("search");
    let v: Value = serde_json::from_str(&raw).unwrap();
    let hits = v["hits"].as_array().unwrap();
    let hit_names: BTreeSet<_> = hits
        .iter()
        .filter_map(|h| h.get("name").and_then(|n| n.as_str()))
        .filter(|n| *n != "recipes")
        .map(str::to_string)
        .collect();
    let expected: BTreeSet<_> = codemode_binding_names()
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(
        hit_names, expected,
        "CodeMode search hits must equal registry bindings"
    );
}

#[test]
fn orient_subsurfaces_match_surface_names() {
    let a: BTreeSet<_> = orient_surface_names().into_iter().collect();
    let b: BTreeSet<_> = SURFACE_NAMES.iter().copied().collect();
    assert_eq!(a, b);
}

#[test]
fn golden_vectors_serialize_domain_shapes() {
    for v in golden_vectors() {
        if let Some(ok) = &v.expected_ok {
            let wire = serde_json::to_value(ok).unwrap();
            assert_eq!(wire["op"], v.op);
        }
        if let Some(err) = &v.expected_err {
            let wire = serde_json::to_value(err).unwrap();
            assert_eq!(wire["kind"], err.kind.as_str());
            assert_eq!(wire["retryable"], err.retryable);
        }
        assert!(
            resolve_operation(v.op).is_some(),
            "vector {} op missing",
            v.id
        );
    }
}

#[test]
fn domain_error_retryability_defaults() {
    assert!(!DomainError::new(DomainErrorKind::Validation, "x").retryable);
    assert!(!DomainError::new(DomainErrorKind::Policy, "x").retryable);
    assert!(DomainError::new(DomainErrorKind::Busy, "x").retryable);
    assert!(DomainError::new(DomainErrorKind::Cancelled, "x").retryable);
    assert!(DomainError::new(DomainErrorKind::DeadlineExceeded, "x").retryable);
}

#[test]
fn domain_result_refs_roundtrip() {
    let r = DomainResult::new("expand", serde_json::json!({"ok": true}))
        .with_refs(vec!["gz://blob/aa".into()])
        .expose_primary_ref();
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["refs"][0], "gz://blob/aa");
    assert_eq!(v["value"]["ref"], "gz://blob/aa");

    let miss = DomainResult::new("blast", serde_json::json!({"found": false})).expose_primary_ref();
    assert!(miss.value.get("ref").is_none());

    let compact = DomainResult::new("snap", serde_json::json!("gz://query/compact"))
        .with_refs(vec!["gz://query/compact".into()])
        .expose_primary_ref();
    assert_eq!(compact.value["ref"], "gz://query/compact");
    assert_eq!(compact.value["value"], "gz://query/compact");
}

#[test]
fn fastmcp_catalog_full_structural_io_parity() {
    for tool in lean_fastmcp_tools_from_registry() {
        let name = tool.get("name").and_then(|n| n.as_str()).unwrap();
        let op = resolve_operation(name).unwrap();
        assert_tool_schema_parity(&tool, op).unwrap_or_else(|e| panic!("{e}"));
    }
}

#[test]
fn codemode_describe_io_schemas_match_registry() {
    for binding in codemode_binding_names() {
        let raw = codemode::describe(binding).unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        let desc = v.get("description").unwrap();
        if desc.get("error").is_some() {
            panic!("describe failed for {binding}: {desc}");
        }
        let op = resolve_operation(binding).unwrap();
        let input = desc.get("input_schema").expect("input_schema");
        let output = desc.get("output_schema").expect("output_schema");
        assert!(
            schemas_structurally_equal(input, &op.args.schema),
            "CodeMode input drift for {binding}: {:?}",
            schema_diff(input, &op.args.schema)
        );
        assert!(
            schemas_structurally_equal(output, &op.results.schema),
            "CodeMode output drift for {binding}: {:?}",
            schema_diff(output, &op.results.schema)
        );
    }
}

// ── Kill tests: mutations that name-set equality cannot see ──────────────

#[test]
fn kill_type_change_passes_key_sets_fails_structural() {
    let op = resolve_operation("snap").unwrap();
    let mut forged = op.args.schema.clone();
    forged["properties"]["budget"]["type"] = json!("string");
    assert_eq!(
        schema_property_keys(&op.args.schema),
        schema_property_keys(&forged),
        "setup: keys still equal"
    );
    assert_eq!(
        schema_required_keys(&op.args.schema),
        schema_required_keys(&forged),
        "setup: required still equal"
    );
    assert!(
        !schemas_structurally_equal(&op.args.schema, &forged),
        "type change must fail structural parity"
    );
}

#[test]
fn kill_requiredness_change() {
    let op = resolve_operation("search").unwrap();
    let mut forged = op.args.schema.clone();
    forged["required"] = json!([]);
    assert!(!schemas_structurally_equal(&op.args.schema, &forged));
}

#[test]
fn kill_missing_property() {
    let op = resolve_operation("orient").unwrap();
    let mut forged = op.args.schema.clone();
    forged["properties"]
        .as_object_mut()
        .unwrap()
        .remove("budget");
    assert!(!schemas_structurally_equal(&op.args.schema, &forged));
}

#[test]
fn kill_extra_property() {
    let op = resolve_operation("blast").unwrap();
    let mut forged = op.args.schema.clone();
    forged["properties"]
        .as_object_mut()
        .unwrap()
        .insert("sneaky".into(), json!({"type": "string"}));
    assert!(!schemas_structurally_equal(&op.args.schema, &forged));
}

#[test]
fn kill_nested_max_length_constraint() {
    let op = resolve_operation("execute_code").unwrap();
    let mut forged = op.args.schema.clone();
    forged["properties"]["plan"]["maxLength"] = json!(1);
    assert_eq!(
        schema_property_keys(&op.args.schema),
        schema_property_keys(&forged)
    );
    assert!(!schemas_structurally_equal(&op.args.schema, &forged));
}

#[test]
fn kill_nested_enum_constraint() {
    let op = resolve_operation("snap").unwrap();
    let mut forged = op.args.schema.clone();
    forged["properties"]["format"]["enum"] = json!(["minimal"]);
    assert!(!schemas_structurally_equal(&op.args.schema, &forged));
}

#[test]
fn kill_output_schema_drift() {
    let op = resolve_operation("blast").unwrap();
    let mut forged = op.results.schema.clone();
    // Drop error arm required field to simulate output-shape drift.
    if let Some(arr) = forged.get_mut("oneOf").and_then(|v| v.as_array_mut()) {
        if let Some(err_arm) = arr.get_mut(1) {
            err_arm["required"] = json!(["kind"]);
        }
    }
    assert!(
        !schemas_structurally_equal(&op.results.schema, &forged),
        "output requiredness drift must be detected"
    );
    assert_ne!(
        schema_fingerprint_hex(&op.results.schema),
        schema_fingerprint_hex(&forged)
    );
}

#[test]
fn kill_missing_output_schema_on_tool() {
    let op = resolve_operation("expand").unwrap();
    let tool = json!({
        "name": "expand",
        "inputSchema": op.args.schema,
        // no outputSchema
    });
    let err = assert_tool_schema_parity(&tool, op).unwrap_err();
    assert!(err.contains("outputSchema"), "{err}");
}

#[test]
fn kill_forged_catalog_tool_output_type_drift() {
    let mut tools = lean_fastmcp_tools_from_registry();
    let tool = tools
        .iter_mut()
        .find(|t| t.get("name").and_then(|n| n.as_str()) == Some("index"))
        .unwrap();
    let op = resolve_operation("index").unwrap();
    tool.get_mut("outputSchema")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert("type".into(), json!("array"));
    assert!(assert_tool_schema_parity(tool, op).is_err());
}

#[test]
fn contract_manifest_owns_complete_io_schemas() {
    let m = contract_manifest();
    assert_eq!(m["schema_parity"], "structural_io");
    for op in m["operations"].as_array().unwrap() {
        assert!(op.get("input_schema").is_some());
        assert!(op.get("output_schema").is_some());
        // Fingerprints are stable 64-char digests of normalized schemas.
        assert_eq!(op["input_schema_fingerprint"].as_str().unwrap().len(), 64);
        assert_eq!(op["output_schema_fingerprint"].as_str().unwrap().len(), 64);
    }
}

#[test]
fn legacy_aliases_inventory() {
    for (alias, canonical) in [
        ("blast_intent", "blast"),
        ("verify_claim", "verify"),
        ("graph.query", "query"),
        ("graph.multiQuery", "multi_query"),
        ("gz_execute_code", "execute_code"),
    ] {
        let op = resolve_operation(alias).unwrap_or_else(|| panic!("missing alias {alias}"));
        assert_eq!(op.name, canonical, "alias {alias}");
    }
}

/// graphzero-ztd-mcp-alias-contract-javd: blast_intent absorbed as documented legacy alias.
#[test]
fn absorbs_mcp_alias_contract_blast_intent() {
    let op = resolve_operation("blast_intent").unwrap();
    assert_eq!(op.name, "blast");
    assert!(op.aliases.contains(&"blast_intent"));
    // Canonical FastMCP name remains `blast` only.
    assert!(lean_fastmcp_tool_names().contains(&"blast"));
    assert!(!lean_fastmcp_tool_names().contains(&"blast_intent"));
}
