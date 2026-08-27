//! Aggregate bindings over the typed TokenZero dispatcher (tokenzero-irx9.6).

use serde_json::json;
use std::collections::BTreeSet;
use std::fs;
use tempfile::tempdir;
use tokenzero_core::operation_abi::{MigrationStatus, all_operations};
use tokenzero_engine::{
    EngineConfig, TokenZeroEngine, dispatch_codemode_method, dispatch_mcp_tool,
};

fn engine_for(root: &std::path::Path) -> TokenZeroEngine {
    let mut config = EngineConfig::for_root(root);
    config.session_dedup = false;
    config.diff_reads = false;
    config.fetch_enabled = false;
    TokenZeroEngine::new(config)
}

#[test]
fn every_aggregate_domain_binding_is_registry_backed() {
    let bindings: BTreeSet<&str> = all_operations()
        .iter()
        .filter(|op| {
            op.exposure.codemode_binding.is_some()
                && matches!(
                    op.migration,
                    MigrationStatus::Canonical | MigrationStatus::LegacyAlias
                )
                && op.exposure.resource_uri.is_none()
        })
        .filter_map(|op| op.exposure.codemode_binding)
        .collect();
    for required in [
        "zero.read",
        "zero.find",
        "zero.glob",
        "zero.tree",
        "zero.edit",
        "zero.shell",
        "zero.token.expand",
    ] {
        assert!(
            bindings.contains(required),
            "missing {required} in {bindings:?}"
        );
    }
}

/// One aggregate binding normalizes to classic MCP for bound domain operations.
#[test]
fn one_aggregate_binding_normalizes_to_classic_mcp() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("note.txt"), b"codemode-parity").unwrap();
    let root = dir.path();
    let note = root.join("note.txt").display().to_string();

    let cases: Vec<(&str, &str, serde_json::Value)> = vec![
        ("tz_read", "zero.read", json!({"path": note})),
        (
            "tz_glob",
            "zero.glob",
            json!({"pattern": "*.txt", "path": root.display().to_string()}),
        ),
        (
            "tz_tree",
            "zero.tree",
            json!({"path": root.display().to_string(), "depth": 1}),
        ),
        ("tz_mem", "zero.mem", json!({})),
        (
            "tz_shell",
            "zero.shell",
            json!({"command": "true", "cwd": root.display().to_string()}),
        ),
        (
            "tz_edit",
            "zero.edit",
            json!({
                "path": note,
                "edits": [{"find": "codemode-parity", "replace": "codemode-parity"}],
                "dry_run": true
            }),
        ),
    ];

    for (mcp_name, cm_name, args) in cases {
        let mcp = dispatch_mcp_tool(&engine_for(root), mcp_name, &args).expect("mcp");
        let cm = dispatch_codemode_method(&engine_for(root), cm_name, &args).expect("cm");
        let n = |o: &tokenzero_engine::DispatchOutcome| {
            let r = o.tool_response.as_ref().expect("resp");
            (
                r.status.clone(),
                r.error.as_ref().map(|e| e.code.clone()),
                r.visible.as_ref().map(|v| v.text.clone()),
                r.refs.iter().map(|x| x.ref_id.clone()).collect::<Vec<_>>(),
            )
        };
        assert_eq!(
            n(&mcp),
            n(&cm),
            "aggregate binding {cm_name} must normalize to classic MCP {mcp_name}"
        );
    }
}

/// Aggregate one-operation dispatch enters the domain dispatcher directly.
#[test]
fn aggregate_binding_path_is_direct_dispatcher() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("note.txt"), b"x").unwrap();
    let eng = engine_for(dir.path());
    let out = dispatch_codemode_method(
        &eng,
        "zero.read",
        &json!({"path": dir.path().join("note.txt").display().to_string()}),
    )
    .expect("aggregate binding path");
    assert!(out.is_ok());
}
