#![allow(dead_code)]

use assert_cmd::prelude::*;
use serde_json::Value;
use std::process::{Command, Output};
use tempfile::{TempDir, tempdir};

pub fn required_adapter_tools() -> &'static [&'static str] {
    &[
        "rtk",
        "ztk",
        "lean-ctx",
        "tokenpak",
        "tokenjuice",
        "context-mode",
        "caveman",
        "headroom",
        "claw",
        "compresh",
        "context-gateway",
    ]
}

pub fn reviewed_adapter_rows() -> Vec<Value> {
    required_adapter_tools()
        .iter()
        .map(|tool| {
            serde_json::json!({
                "tool": tool,
                "approval_status": "reviewed",
                "execution_allowed": true,
                "reviewed_command": format!("{tool} --version"),
                "blind_install_attempted": false
            })
        })
        .collect()
}

pub fn tokenzero_with_agent_env(args: &[&str]) -> Output {
    Command::cargo_bin("tokenzero")
        .unwrap()
        .args(args)
        .env("NO_COLOR", "1")
        .env("CI", "true")
        .env("TERM", "dumb")
        .env("SOURCE_DATE_EPOCH", "1234567890")
        .output()
        .unwrap()
}

pub fn assert_no_ansi(bytes: &[u8]) {
    assert!(
        !bytes.contains(&0x1b),
        "unexpected ANSI escape in output:\n{}",
        String::from_utf8_lossy(bytes)
    );
}

/// Remove ANSI CSI/OSC escape sequences from help text so command names parse
/// cleanly even when clap emits bold/underline styling.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        if chars.clone().next() == Some('[') {
            // CSI: ESC [ params final (0x40..=0x7e)
            chars.next();
            for n in chars.by_ref() {
                if ('\x40'..='\x7e').contains(&n) {
                    break;
                }
            }
        } else {
            // Short 2-byte escape (e.g. ESC ] or ESC (): consume one more byte.
            let _ = chars.next();
        }
    }
    out
}

pub fn setup_temp_with_cache() -> (TempDir, std::path::PathBuf) {
    let dir = tempdir().unwrap();
    let cache = dir.path().join("cache.json");
    (dir, cache)
}

fn run_tokenzero_json_configured(
    args: &[&str],
    cwd: Option<&std::path::Path>,
    envs: &[(&str, &str)],
) -> Value {
    let mut cmd = tokenzero_cmd();
    cmd.args(args);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    for (key, value) in envs {
        cmd.env(key, value);
    }
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "tokenzero {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

pub fn run_tokenzero_json(args: &[&str]) -> Value {
    run_tokenzero_json_configured(args, None, &[])
}

pub fn run_tokenzero_json_in(args: &[&str], cwd: &std::path::Path) -> Value {
    run_tokenzero_json_configured(args, Some(cwd), &[])
}

pub fn run_tokenzero_json_with_env(args: &[&str], envs: &[(&str, &str)]) -> Value {
    run_tokenzero_json_configured(args, None, envs)
}

pub fn write_json_fixture(path: &std::path::Path, value: &Value) {
    std::fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

pub fn write_results_fixture(root: &std::path::Path, file_name: &str, value: &Value) {
    let results_dir = root.join("results").join("current");
    std::fs::create_dir_all(&results_dir).unwrap();
    write_json_fixture(&results_dir.join(file_name), value);
}

pub fn write_minimal_handoff_completion_audit(
    results_dir: &std::path::Path,
    release_candidate_id: &str,
) {
    write_json_fixture(
        &results_dir.join("tokenzero_completion_audit.json"),
        &serde_json::json!({
            "schema_version": "tokenzero.completion_audit.v1",
            "release_candidate_id": release_candidate_id,
            "completion_achieved": false,
            "public_claims_approved": false,
            "release_publication_allowed": false,
            "residual_gate_matrix": []
        }),
    );
}

pub fn assert_reason(row: &Value, expected_reason: &str) {
    assert!(
        row["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason == expected_reason),
        "missing reason {expected_reason:?} in {}",
        row["reasons"]
    );
}

pub fn assert_reason_contains(reasons: &[Value], expected: &str) {
    assert!(
        reasons.iter().any(|reason| reason == expected),
        "missing reason {expected:?} in {reasons:?}"
    );
}

pub fn assert_blocked_reason(json: &Value, expected_reason: &str) {
    assert!(
        json["blocked_reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == expected_reason),
        "missing blocked reason {expected_reason:?} in {}",
        json["blocked_reasons"]
    );
}

pub fn find_gate<'a>(json: &'a Value, gate_id: &str) -> &'a Value {
    json["evidence_gates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|gate| gate["id"] == gate_id)
        .unwrap_or_else(|| panic!("evidence gate not found: {gate_id}"))
}

pub fn first_ref_with_kind(json: &Value, kind: &str) -> String {
    json["refs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["kind"] == kind)
        .unwrap_or_else(|| panic!("missing ref kind {kind}: {json}"))["ref"]
        .as_str()
        .unwrap()
        .to_string()
}

pub fn find_row_by<'a>(rows: &'a [Value], field: &str, value: &str) -> &'a Value {
    rows.iter()
        .find(|row| row[field] == value)
        .unwrap_or_else(|| panic!("row not found: {field}={value}"))
}

pub fn find_artifact<'a>(json: &'a Value, artifact_id: &str) -> &'a Value {
    find_row_by(json["artifacts"].as_array().unwrap(), "id", artifact_id)
}

pub fn assert_json_fields(value: &Value, expected: &[(&str, Value)]) {
    for (field, expected) in expected {
        assert_eq!(&value[*field], expected, "{field}");
    }
}

pub fn tokenzero_cmd() -> Command {
    let mut cmd = Command::cargo_bin("tokenzero").unwrap();
    // Isolate from ambient agent env (e.g. TOKENZERO_ROOT=/ would disable path
    // allowlisting and make every absolute path appear in-root).
    cmd.env_remove("TOKENZERO_ROOT");
    cmd.env_remove("TOKENZERO_CACHE_PATH");
    cmd.env_remove("TOKENZERO_SHARED_STORE");
    cmd.env_remove("ZEROSTACK_SHARED_STORE");
    cmd.env_remove("ZEROSTACK_STORE_ROOT");
    // Most integration fixtures pin the legacy forensic JSON schema. Product
    // default is slim; those fixtures opt into full compatibility explicitly.
    // Focused default-envelope tests remove this override.
    cmd.env("TOKENZERO_SLIM_ENVELOPE", "0");
    cmd
}

pub fn assert_success(output: Output, label: &str) -> Output {
    assert!(
        output.status.success(),
        "{label}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

pub fn assert_success_ref(output: &Output, label: &str) {
    assert!(
        output.status.success(),
        "{label}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

pub fn parse_json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

pub fn run_tokenzero_output(args: &[&str]) -> Output {
    tokenzero_cmd().args(args).output().unwrap()
}

pub fn run_tool_json(
    tool: &str,
    extra_args: &[&str],
    root: &std::path::Path,
    cache: &std::path::Path,
) -> Value {
    let mut args = vec![tool];
    args.extend_from_slice(extra_args);
    args.extend([
        "--cache-path",
        cache.to_str().unwrap(),
        "--allowed-root",
        root.to_str().unwrap(),
        "--json",
    ]);
    run_tokenzero_json(&args)
}

pub fn run_tokenzero_json_in_with_env(
    args: &[&str],
    cwd: &std::path::Path,
    envs: &[(&str, &str)],
) -> Value {
    run_tokenzero_json_configured(args, Some(cwd), envs)
}

pub fn results_current_dir(root: &std::path::Path) -> std::path::PathBuf {
    let results_dir = root.join("results").join("current");
    std::fs::create_dir_all(&results_dir).unwrap();
    results_dir
}

pub fn write_json_fixture_pretty(path: &std::path::Path, value: &Value) {
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

pub fn source_currency_rows() -> Vec<Value> {
    required_adapter_tools()
        .iter()
        .enumerate()
        .map(|(idx, tool)| {
            serde_json::json!({
                "tool": tool,
                "url": format!("https://github.com/example/{tool}"),
                "source_date": "2026-06-04",
                "source_commit": format!("{:040x}", idx + 1),
                "claimed_scope": "claim gate fixture",
                "issue_pr_themes": ["fixture issue"],
                "strengths": ["fixture strength"],
                "gaps": ["fixture gap"],
                "fresh_for_private_planning": true,
                "fresh_for_public_claim": true
            })
        })
        .collect()
}

pub fn run_cli_run_output(
    dir: &std::path::Path,
    cache: &std::path::Path,
    trailing: &[&str],
) -> Output {
    let mut args: Vec<&str> = vec![
        "run",
        "--json",
        "--cache-path",
        cache.to_str().unwrap(),
        "--allowed-root",
        dir.to_str().unwrap(),
        "--cwd",
        dir.to_str().unwrap(),
        "--",
    ];
    args.extend_from_slice(trailing);
    tokenzero_cmd().args(&args).output().unwrap()
}

pub fn run_cli_run_json(
    dir: &std::path::Path,
    cache: &std::path::Path,
    trailing: &[&str],
) -> Value {
    let output = assert_success(run_cli_run_output(dir, cache, trailing), "cli run");
    parse_json_stdout(&output)
}

pub fn expand_raw_text(
    r: &str,
    cache: Option<&std::path::Path>,
    cwd: Option<&std::path::Path>,
    envs: &[(&str, &str)],
) -> String {
    let mut cmd = tokenzero_cmd();
    cmd.arg("expand").arg(r).arg("--raw");
    if let Some(c) = cache {
        cmd.args(["--cache-path", c.to_str().unwrap()]);
    }
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let output = assert_success(cmd.output().unwrap(), "expand");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn adapter_approval_audit_fixture(rc_id: &str, execution_allowed: bool) -> Value {
    serde_json::json!({
        "schema_version": "tokenzero.adapter_approval_audit.v1",
        "ok": true,
        "release_candidate_id": rc_id,
        "execution_allowed": execution_allowed,
        "public_claims_approved": true,
        "blind_install_attempted": false,
        "required_adapter_count": 11,
        "reviewed_command_count": 11,
        "missing_reviewed_command_count": 0,
        "duplicate_command_count": 0,
        "unsafe_command_count": 0,
        "adapters": reviewed_adapter_rows()
    })
}

pub fn evidence_artifact_paths(dir: &std::path::Path) -> [std::path::PathBuf; 6] {
    [
        dir.join("source-currency.json"),
        dir.join("benchmark.json"),
        dir.join("adapter-approval.json"),
        dir.join("recovery.json"),
        dir.join("task-success.json"),
        dir.join("os.json"),
    ]
}

pub fn run_claim_audit_with_all_artifacts(dir: &std::path::Path) -> Value {
    let paths = evidence_artifact_paths(dir);
    run_tokenzero_json(&[
        "claim-audit",
        "--release-approval",
        "--source-artifact",
        paths[0].to_str().unwrap(),
        "--benchmark-artifact",
        paths[1].to_str().unwrap(),
        "--adapter-approval-artifact",
        paths[2].to_str().unwrap(),
        "--recovery-artifact",
        paths[3].to_str().unwrap(),
        "--task-success-artifact",
        paths[4].to_str().unwrap(),
        "--os-artifact",
        paths[5].to_str().unwrap(),
        "--json",
    ])
}

#[allow(clippy::too_many_arguments)]
pub fn assert_integrity_row(
    row: &Value,
    present: bool,
    readable: bool,
    schema: Value,
    expected_schema: Value,
    schema_matches: Option<bool>,
    expected_rc: Value,
    rc: Value,
    rc_matches: bool,
    valid: bool,
    reason: &str,
) {
    assert_eq!(row["present"], present);
    assert_eq!(row["readable"], readable);
    assert_eq!(row["schema_version"], schema);
    if !expected_schema.is_null() {
        assert_eq!(row["expected_schema_version"], expected_schema);
    }
    if let Some(matches) = schema_matches {
        assert_eq!(row["schema_matches"], matches);
    }
    if !expected_rc.is_null() {
        assert_eq!(row["expected_release_candidate_id"], expected_rc);
    }
    assert_eq!(row["release_candidate_id"], rc);
    assert_eq!(row["release_candidate_matches"], rc_matches);
    assert_eq!(row["valid"], valid);
    assert_reason(row, reason);
}
