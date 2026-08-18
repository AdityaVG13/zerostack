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

#[test]
fn shell_auto_backgrounds_above_documented_threshold() {
    let (_, operation, args) = lower(
        "token",
        "shell",
        json!({"command": "cargo test", "timeout_ms": 120_000}),
    )
    .unwrap();

    assert_eq!(operation, "shell");
    assert_eq!(
        args,
        json!({"command": "cargo test", "timeout_ms": 120_000, "background": true})
    );
}

#[test]
fn shell_timeout_seconds_above_threshold_auto_backgrounds() {
    let (_, _, args) = lower(
        "token",
        "shell",
        json!({"command": "cargo test", "timeout_seconds": 120}),
    )
    .unwrap();

    assert_eq!(
        args,
        json!({"command": "cargo test", "timeout_seconds": 120, "background": true})
    );
}

#[test]
fn shell_camel_case_timeout_above_threshold_auto_backgrounds() {
    let (_, _, args) = lower(
        "token",
        "shell",
        json!({"command": "cargo test", "timeoutMs": 300_000}),
    )
    .unwrap();

    assert_eq!(
        args,
        json!({"command": "cargo test", "timeout_ms": 300_000, "background": true})
    );
}

#[test]
fn shell_short_calls_stay_direct() {
    for args in [
        json!({"command": "echo hi"}),
        json!({"command": "echo hi", "timeout_ms": 30_000}),
        json!({"command": "echo hi", "timeout_seconds": 60}),
    ] {
        let (_, _, lowered) = lower("token", "shell", args).unwrap();
        assert!(lowered.get("background").is_none(), "{lowered}");
    }
}

#[test]
fn shell_explicit_background_choice_wins() {
    let (_, _, foreground) = lower(
        "token",
        "shell",
        json!({"command": "cargo test", "timeout_ms": 120_000, "background": false}),
    )
    .unwrap();
    assert_eq!(foreground.get("background"), Some(&Value::Bool(false)));

    let (_, _, background) = lower(
        "token",
        "shell",
        json!({"command": "cargo test", "background": true}),
    )
    .unwrap();
    assert_eq!(background.get("background"), Some(&Value::Bool(true)));
}

#[test]
fn shell_argv_stays_foreground_even_above_threshold() {
    let (_, _, args) = lower(
        "token",
        "shell",
        json!({"command": ["echo", "hi"], "timeout_ms": 120_000}),
    )
    .unwrap();

    assert_eq!(args, json!({"argv": ["echo", "hi"], "timeout_ms": 120_000}));
    assert!(args.get("background").is_none());
}
