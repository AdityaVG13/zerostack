//! SPEC-TZ-RB-001: install apply/rollback fail-loud contract.
//!
//! Minimal public-API driver. The previous inline file
//! `tests/install/inline/lib__rollback_drift_tests.rs` was unwired after
//! `d8c0844` (and called private `verify_install_write` / `sha256_bytes`
//! that the hub crate no longer exports).

use std::fs;
use std::io::ErrorKind;

use serde_json::{Value, json};
use tokenzero_install::{apply, rollback};

#[test]
fn rollback_refuses_when_post_install_edit_drifts() {
    let root = tempfile::tempdir().expect("tempdir");
    let config = root.path().join(".tokenzero/mcp-server.json");
    fs::create_dir_all(config.parent().unwrap()).expect("mkdir");
    fs::write(
        &config,
        serde_json::to_vec(&json!({"user_before": true})).expect("seed"),
    )
    .expect("write seed");
    let applied = apply(root.path(), false, &["mcp".to_string()]).expect("apply");
    let mut value: Value = serde_json::from_slice(&fs::read(&config).expect("read")).expect("json");
    value["user_after"] = json!({"must_survive_rollback": true});
    fs::write(&config, serde_json::to_vec_pretty(&value).expect("encode")).expect("edit");
    let err = rollback(root.path(), &applied.rollback.id).expect_err("must conflict");
    assert!(
        err.to_string().contains("rollback conflict"),
        "unexpected error: {err}"
    );
    let final_value: Value =
        serde_json::from_slice(&fs::read(&config).expect("reread")).expect("final json");
    assert!(
        final_value.get("user_after").is_some(),
        "post-install edit must remain untouched: {final_value}"
    );
}

#[test]
fn rollback_restores_when_installed_bytes_unchanged() {
    let root = tempfile::tempdir().expect("tempdir");
    let config = root.path().join(".tokenzero/mcp-server.json");
    fs::create_dir_all(config.parent().unwrap()).expect("mkdir");
    fs::write(
        &config,
        serde_json::to_vec(&json!({"user_before": true})).expect("seed"),
    )
    .expect("write seed");
    let applied = apply(root.path(), false, &["mcp".to_string()]).expect("apply");
    let result = rollback(root.path(), &applied.rollback.id).expect("rollback");
    assert_eq!(result["status"], "ok");
    let final_value: Value =
        serde_json::from_slice(&fs::read(&config).expect("reread")).expect("final json");
    assert_eq!(final_value, json!({"user_before": true}));
    assert!(final_value.get("mcpServers").is_none());
}

#[test]
fn instructions_merge_preserves_and_rolls_back_existing_agents_bytes() {
    let root = tempfile::tempdir().expect("tempdir");
    let agents = root.path().join("AGENTS.md");
    let original = b"# Existing project law\r\nKeep `quotes`, \\slashes, and this final byte";
    fs::write(&agents, original).expect("seed AGENTS.md");

    let applied = apply(root.path(), false, &["instructions".to_string()]).expect("apply");
    let installed = fs::read(&agents).expect("read installed instructions");
    assert!(
        installed.starts_with(original),
        "existing bytes must remain a prefix"
    );
    let installed_text = String::from_utf8(installed).expect("UTF-8 instructions");
    assert_eq!(
        installed_text
            .matches("<!-- tokenzero:rust-core:start -->")
            .count(),
        1
    );
    assert_eq!(
        installed_text
            .matches("<!-- tokenzero:rust-core:end -->")
            .count(),
        1
    );

    let result = rollback(root.path(), &applied.rollback.id).expect("rollback");
    assert_eq!(result["status"], "ok");
    assert_eq!(
        fs::read(&agents).expect("read restored AGENTS.md"),
        original
    );
}

#[test]
fn instructions_refuse_non_utf8_agents_without_mutation() {
    let root = tempfile::tempdir().expect("tempdir");
    let agents = root.path().join("AGENTS.md");
    let original = [b'#', b' ', 0xff, b'\n'];
    fs::write(&agents, original).expect("seed non-UTF-8 AGENTS.md");

    let err = apply(root.path(), false, &["instructions".to_string()])
        .expect_err("non-UTF-8 project law must fail closed");
    assert_eq!(err.kind(), ErrorKind::InvalidData);
    assert!(err.to_string().contains("refusing to replace non-UTF-8"));
    assert_eq!(
        fs::read(&agents).expect("read untouched AGENTS.md"),
        original
    );
}
