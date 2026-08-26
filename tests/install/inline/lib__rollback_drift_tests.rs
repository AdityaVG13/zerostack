use super::*;
use serde_json::{Value, json};

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
    let edited = serde_json::to_vec_pretty(&value).expect("encode");
    fs::write(&config, &edited).expect("edit");
    let edited_bytes = fs::read(&config).expect("reread edited");
    let err = rollback(root.path(), &applied.rollback.id).expect_err("must conflict");
    assert_eq!(
        err.kind(),
        ErrorKind::InvalidData,
        "rollback conflict must be InvalidData, got {err:?}"
    );
    // Byte-for-byte preservation: the post-install edit must remain untouched.
    assert_eq!(
        fs::read(&config).expect("reread after refused rollback"),
        edited_bytes,
        "refused rollback must not mutate drifted file"
    );
    let final_value: Value =
        serde_json::from_slice(&fs::read(&config).expect("reread")).expect("final json");
    assert_eq!(
        final_value.get("user_after"),
        Some(&json!({"must_survive_rollback": true})),
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
    // Public state oracle: every original byte must survive as a prefix.
    assert!(
        installed.starts_with(original),
        "existing bytes must remain a prefix"
    );
    // No-duplication oracle without counting private delimiters: a second merge
    // over the already-installed text must be byte-identical (idempotent).
    let installed_text = String::from_utf8(installed.clone()).expect("UTF-8 instructions");
    let remerged =
        merge_instructions(&installed_text, McpToolSurface::Classic).expect("re-merge installed");
    assert_eq!(
        remerged.as_bytes(),
        installed.as_slice(),
        "instruction merge must be idempotent over installed content"
    );

    let result = rollback(root.path(), &applied.rollback.id).expect("rollback");
    assert_eq!(result["status"], "ok");
    // Exact byte-for-byte rollback, not just semantic equality.
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

#[test]
fn apply_verification_hashes_match_bytes_on_disk() {
    let root = tempfile::tempdir().expect("tempdir");
    let applied = apply(root.path(), false, &["mcp".to_string()]).expect("apply");
    assert!(
        !applied.verification.is_empty(),
        "apply must record verification rows"
    );
    for row in &applied.verification {
        assert!(
            row.verified,
            "successful apply must only keep verified writes: {row:?}"
        );
        let observed = fs::read(&row.path).expect("read installed path");
        assert_eq!(row.byte_count, observed.len());
        // Independent oracle: compute SHA-256 without calling the production
        // helper that produced the verification row.
        let mut hasher = sha2::Sha256::new();
        hasher.update(&observed);
        let expected = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        assert_eq!(row.observed_sha256, expected);
    }
}

#[test]
fn verify_install_write_fails_loud_on_byte_mismatch() {
    let err = verify_install_write("probe.txt", b"expected", b"observed".to_vec())
        .expect_err("mismatch must fail closed");
    assert_eq!(err.kind(), ErrorKind::InvalidData);
    assert!(
        err.to_string().contains("install verification failed"),
        "unexpected error: {err}"
    );
}
