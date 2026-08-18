//! Public surface lowering contracts for direct file reads and shell timeouts.

use serde_json::{Value, json};
use zsx_core::lower;

#[test]
fn direct_fs_read_lowers_to_compound_read_operation() {
    let (_, operation, args) = lower("fs", "read", Value::String("AGENTS.md".into())).unwrap();

    assert_eq!(operation, "fs.read");
    assert_eq!(args, json!({"path": "AGENTS.md"}));
}

#[test]
fn shell_accepts_camel_case_timeout() {
    let (_, operation, args) = lower(
        "token",
        "shell",
        json!({"command": "pwd", "timeoutMs": 20_000}),
    )
    .unwrap();

    assert_eq!(operation, "shell");
    assert_eq!(args, json!({"command": "pwd", "timeout_ms": 20_000}));
}

#[test]
fn shell_rejects_conflicting_timeout_spellings() {
    let error = lower(
        "token",
        "shell",
        json!({"command": "pwd", "timeoutMs": 20_000, "timeout_ms": 30_000}),
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("must not include both 'timeoutMs' and 'timeout_ms'")
    );
}
