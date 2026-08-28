//! Canonical TokenZero dispatcher identity after V6 CodeMode liquidation (tokenzero-irx9.6).

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
fn v6_zero_star_bindings_are_absent_from_registry() {
    let bindings: BTreeSet<&str> = all_operations()
        .iter()
        .filter_map(|op| op.exposure.codemode_binding)
        .collect();
    let aliases: BTreeSet<&str> = all_operations()
        .iter()
        .flat_map(|op| op.aliases.iter().copied())
        .collect();
    for retired in [
        "zero.read",
        "zero.find",
        "zero.glob",
        "zero.tree",
        "zero.edit",
        "zero.shell",
        "zero.mem",
        "zero.token.expand",
        "zero.token.compact",
        "zero.expand",
        "zero.compact",
    ] {
        assert_eq!(
            bindings.contains(retired),
            false,
            "retired V6 binding {retired} still advertised in {bindings:?}"
        );
        assert_eq!(
            aliases.contains(retired),
            false,
            "retired V6 alias {retired} still advertised in {aliases:?}"
        );
    }
    // Canonical tz_* names remain the public surface; they are not V6 zero.* bindings.
    let canonical: BTreeSet<&str> = all_operations()
        .iter()
        .filter(|op| {
            matches!(
                op.migration,
                MigrationStatus::Canonical | MigrationStatus::LegacyAlias
            ) && op.exposure.fastmcp_tool
                && op.exposure.resource_uri.is_none()
        })
        .map(|op| op.name)
        .collect();
    for required in ["tz_read", "tz_find", "tz_glob", "tz_tree", "tz_edit", "tz_shell"] {
        assert!(
            canonical.contains(required),
            "missing {required} in {canonical:?}"
        );
    }
}

/// Classic MCP and CodeMode both dispatch the canonical tz_* name.
#[test]
fn canonical_tz_names_dispatch_on_mcp_and_codemode() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("note.txt"), b"codemode-parity").unwrap();
    let root = dir.path();
    let note = root.join("note.txt").display().to_string();

    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("tz_read", json!({"path": note})),
        (
            "tz_glob",
            json!({"pattern": "*.txt", "path": root.display().to_string()}),
        ),
        (
            "tz_tree",
            json!({"path": root.display().to_string(), "depth": 1}),
        ),
        ("tz_mem", json!({})),
        (
            "tz_shell",
            json!({"command": "true", "cwd": root.display().to_string()}),
        ),
        (
            "tz_edit",
            json!({
                "path": note,
                "edits": [{"find": "codemode-parity", "replace": "codemode-parity"}],
                "dry_run": true
            }),
        ),
    ];

    for (name, args) in cases {
        let mcp = dispatch_mcp_tool(&engine_for(root), name, &args).expect("mcp");
        let cm = dispatch_codemode_method(&engine_for(root), name, &args).expect("cm");
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
            "CodeMode {name} must normalize to classic MCP {name}"
        );
    }
}

/// Canonical one-operation dispatch enters the domain dispatcher directly.
#[test]
fn canonical_tz_read_path_is_direct_dispatcher() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("note.txt"), b"x").unwrap();
    let eng = engine_for(dir.path());
    let out = dispatch_codemode_method(
        &eng,
        "tz_read",
        &json!({"path": dir.path().join("note.txt").display().to_string()}),
    )
    .expect("canonical tz_read path");
    assert!(out.is_ok());
}
