use assert_cmd::prelude::*;
use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};
use tempfile::tempdir;
use tokenzero_core::operation_abi::all_operations;

mod common;
use common::*;

/// Authority surfaces that exist in core but are not dispatched on CLI/MCP.
/// Ads must not name them as available tools. Partial never rounds up.
const UNDISPATCHED_AUTHORITY_ADS: &[&str] = &[
    "decision view",
    "decisionview",
    "reasoning-state",
    "opaque reasoning",
    "output novelty",
    "outputnovelty",
    "continuation class",
    "continuationkind",
    "decisionviewheadroom",
    "dv headroom",
];

fn assert_no_undispatched_authority_ads(haystack: &str, surface: &str) {
    let lower = haystack.to_lowercase();
    for needle in UNDISPATCHED_AUTHORITY_ADS {
        assert!(
            !lower.contains(needle),
            "{surface} advertises undispatched authority {needle:?}"
        );
    }
    for name_needle in [
        "decision_view",
        "decision-view",
        "reasoning_state",
        "output_novelty",
        "continuation_class",
        "headroom",
    ] {
        assert!(
            !lower.contains(name_needle),
            "{surface} advertises undispatched name {name_needle:?}"
        );
    }
}

/// F-TZ-011 is Missing. Ads must not invent a present strict-mode product flag.
const MISSING_STRICT_MODE_ADS: &[&str] =
    &["strict mode", "strict-mode", "strict_mode", "strictmode"];

fn assert_no_missing_strict_mode_ads(haystack: &str, surface: &str) {
    let lower = haystack.to_lowercase();
    for needle in MISSING_STRICT_MODE_ADS {
        assert!(
            !lower.contains(needle),
            "{surface} advertises missing strict-mode as present ({needle:?})"
        );
    }
}

#[test]
fn cli_help_does_not_advertise_undispatched_decision_views() {
    let help = Command::cargo_bin("tokenzero")
        .unwrap()
        .arg("--help")
        .output()
        .unwrap();
    assert!(
        help.status.success(),
        "{}",
        String::from_utf8_lossy(&help.stderr)
    );
    assert_no_undispatched_authority_ads(
        &String::from_utf8_lossy(&help.stdout),
        "tokenzero --help",
    );

    let mcp_help = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["mcp-server", "--help"])
        .output()
        .unwrap();
    assert!(
        mcp_help.status.success(),
        "{}",
        String::from_utf8_lossy(&mcp_help.stderr)
    );
    assert_no_undispatched_authority_ads(
        &String::from_utf8_lossy(&mcp_help.stdout),
        "tokenzero mcp-server --help",
    );

    let capabilities = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["capabilities", "--json"])
        .output()
        .unwrap();
    assert!(
        capabilities.status.success(),
        "{}",
        String::from_utf8_lossy(&capabilities.stderr)
    );
    assert_no_undispatched_authority_ads(
        &String::from_utf8_lossy(&capabilities.stdout),
        "tokenzero capabilities --json",
    );
}

#[test]
fn cli_help_does_not_advertise_missing_strict_mode() {
    let help = Command::cargo_bin("tokenzero")
        .unwrap()
        .arg("--help")
        .output()
        .unwrap();
    assert!(
        help.status.success(),
        "{}",
        String::from_utf8_lossy(&help.stderr)
    );
    assert_no_missing_strict_mode_ads(&String::from_utf8_lossy(&help.stdout), "tokenzero --help");

    let mcp_help = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["mcp-server", "--help"])
        .output()
        .unwrap();
    assert!(
        mcp_help.status.success(),
        "{}",
        String::from_utf8_lossy(&mcp_help.stderr)
    );
    assert_no_missing_strict_mode_ads(
        &String::from_utf8_lossy(&mcp_help.stdout),
        "tokenzero mcp-server --help",
    );

    let capabilities = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["capabilities", "--json"])
        .output()
        .unwrap();
    assert!(
        capabilities.status.success(),
        "{}",
        String::from_utf8_lossy(&capabilities.stderr)
    );
    let caps_text = String::from_utf8_lossy(&capabilities.stdout);
    assert_no_missing_strict_mode_ads(&caps_text, "tokenzero capabilities --json");
    let json: Value = serde_json::from_slice(&capabilities.stdout).unwrap();
    let flags = json["feature_flags"]
        .as_object()
        .expect("capabilities feature_flags");
    assert!(
        flags
            .keys()
            .all(|key| !key.to_lowercase().contains("strict")),
        "capabilities.feature_flags invented a strict-mode product flag: {flags:?}"
    );
    let features = json["features"].as_array().expect("capabilities features");
    assert!(
        features.iter().all(|feature| {
            feature
                .as_str()
                .is_none_or(|name| !name.to_lowercase().contains("strict"))
        }),
        "capabilities.features invented a strict-mode product flag: {features:?}"
    );
}

#[test]
fn cli_hook_requires_json_but_preserves_valid_fail_open_events() {
    let run_with_stdin = |input: &str| {
        let mut child = Command::cargo_bin("tokenzero")
            .unwrap()
            .args(["hook", "claude-code"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };

    // m80o (R-023): missing hook input is usage failure, never silent success.
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["hook", "claude-code"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("hook claude-code requires JSON on stdin (stdin was empty)"),
        "{stderr}"
    );
    assert!(
        stderr.contains(
            r#"usage: printf '%s\n' '{"tool_name":"Bash","tool_input":{"command":"git status"}}' | tokenzero hook claude-code"#
        ),
        "{stderr}"
    );

    // Malformed JSON names the parse failure and repeats the copyable repair.
    let output = run_with_stdin("{");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("stdin was not valid JSON"), "{stderr}");
    assert!(stderr.contains("| tokenzero hook claude-code"), "{stderr}");

    // A syntactically valid unsupported event remains intentionally fail-open.
    let output = run_with_stdin(r#"{"tool_name":"Other","tool_input":{}}"#);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());

    // A valid actionable event keeps the established decision JSON schema.
    let output =
        run_with_stdin(r#"{"tool_name":"Bash","tool_input":{"command":"printf hook-ok"}}"#);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let decision: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        decision["hookSpecificOutput"]["hookEventName"],
        "PreToolUse"
    );
    assert_eq!(
        decision["hookSpecificOutput"]["permissionDecision"],
        "allow"
    );
    assert!(
        decision["hookSpecificOutput"]["updatedInput"]["command"]
            .as_str()
            .unwrap()
            .contains(" run -- sh -c ")
    );
}

#[test]
fn cli_capabilities_json_exposes_agent_contract() {
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["capabilities", "--json"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema_version"], "tokenzero.capabilities.v1");
    assert_eq!(json["tool"], "tokenzero");
    assert_eq!(json["contract_version"], 1);
    assert_eq!(json["stdout_contract"]["json_flag"], "--json");
    let exit_codes = json["exit_codes"].as_array().expect("exit-code contract");
    let blocked = exit_codes
        .iter()
        .find(|row| row["code"] == 1)
        .expect("blocked exit code");
    let usage = exit_codes
        .iter()
        .find(|row| row["code"] == 2)
        .expect("usage exit code");
    assert_eq!(blocked["label"], "blocked");
    assert_eq!(usage["label"], "usage");
    assert!(
        blocked["meaning"]
            .as_str()
            .unwrap()
            .contains("refused or could not complete")
    );
    assert!(usage["meaning"].as_str().unwrap().contains("malformed"));
    let features = json["features"].as_array().unwrap();
    assert!(features.iter().any(|feature| feature == "json_output"));
    assert!(
        features
            .iter()
            .any(|feature| feature == "non_tty_output_discipline")
    );
    assert_eq!(json["feature_flags"]["capabilities_json"], true);
    assert!(json["feature_flags"].get("codemode_surface").is_none());
    assert!(
        !json["features"]
            .as_array()
            .unwrap()
            .iter()
            .any(|feature| feature == "codemode_surface")
    );
    assert_eq!(json["feature_flags"]["robot_docs_guide"], true);
    assert_eq!(json["feature_flags"]["intent_inference_aliases"], true);
    let dangerous = json["dangerous_operations"]
        .as_array()
        .expect("dangerous operation registry");
    for (command, safe_default, mutation_gate) in [
        (
            "edit",
            "tokenzero edit <path> --edits-json '<json>' --dry-run --json",
            "omit --dry-run only after reviewing the diff",
        ),
        ("install", "tokenzero install --plan --json", "--apply"),
        (
            "install rollback",
            "tokenzero doctor --json",
            "--rollback <id>",
        ),
        (
            "cache migrate-refs",
            "tokenzero cache migrate-refs --json",
            "--apply",
        ),
        (
            "cache migrate-rollback",
            "tokenzero cache migrate-rollback --json",
            "--apply",
        ),
        (
            "cache migrate-cleanup",
            "tokenzero cache migrate-verify --json",
            "--apply --confirm-cleanup",
        ),
        (
            "clients rollback",
            "tokenzero clients doctor --json",
            "clients rollback <id>",
        ),
    ] {
        let row = dangerous
            .iter()
            .find(|row| row["command"] == command)
            .unwrap_or_else(|| panic!("missing dangerous operation {command}"));
        assert_eq!(row["safe_default"], safe_default);
        assert_eq!(row["mutation_gate"], mutation_gate);
    }
    assert_eq!(
        json["commands_by_name"]["run"]["primary_invocation"],
        "tokenzero run --json -- <command>"
    );
    assert_eq!(
        json["output_schemas"]["capabilities"]["schema_version"],
        "tokenzero.capabilities.v1"
    );
    let required_keys = json["output_schemas"]["capabilities"]["required_keys"]
        .as_array()
        .expect("capabilities required keys");
    assert!(required_keys.iter().any(|key| key == "mcp_tools"));
    assert!(required_keys.iter().any(|key| key == "surface_parity"));
    assert!(
        required_keys.iter().any(|key| key == "kernel_orifices"),
        "capabilities required_keys must include kernel_orifices"
    );
    assert!(
        required_keys.iter().any(|key| key == "packaging_orifices"),
        "capabilities required_keys must include packaging_orifices"
    );

    let mcp_tools = json["mcp_tools"].as_array().expect("MCP tool map");
    let expected_names = all_operations()
        .iter()
        .filter(|operation| operation.exposure.fastmcp_tool || operation.exposure.codemode_mcp_tool)
        .map(|operation| operation.name)
        .collect::<Vec<_>>();
    let observed_names = mcp_tools
        .iter()
        .map(|row| row["mcp_tool"].as_str().expect("MCP tool name"))
        .collect::<Vec<_>>();
    assert_eq!(observed_names, expected_names);
    for row in mcp_tools {
        assert_eq!(row["behavioral_parity"], "not_claimed");
        assert_eq!(
            row["schema_relationship"],
            "operation_abi_args_surface_specific_envelopes"
        );
        let surfaces = row["mcp_surfaces"].as_array().expect("MCP surfaces");
        let expected_available =
            cfg!(feature = "surface-mcp") && surfaces.iter().any(|surface| surface == "classic");
        assert_eq!(
            row["available_in_this_build"], expected_available,
            "{} availability",
            row["mcp_tool"]
        );
    }
    let read = mcp_tools
        .iter()
        .find(|row| row["mcp_tool"] == "tz_read")
        .expect("tz_read row");
    assert_eq!(read["cli_verb"], "read");
    assert_eq!(read["codemode_binding"], "zero.read");
    let execute_code = mcp_tools
        .iter()
        .find(|row| row["mcp_tool"] == "tz_execute_code")
        .expect("tz_execute_code row");
    assert!(execute_code["cli_verb"].is_null());
    assert_eq!(execute_code["route_relationship"], "aggregate_control_only");
    let report_issue = mcp_tools
        .iter()
        .find(|row| row["mcp_tool"] == "tz_report_tool_issue")
        .expect("tz_report_tool_issue row");
    assert!(report_issue["cli_verb"].is_null());
    assert_eq!(report_issue["route_relationship"], "aggregate_control_only");
    assert_eq!(json["surface_parity"]["behavioral_parity"], "not_claimed");
    assert!(
        json["output_schemas"]["run"]["status_fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field == "telemetry.command_success")
    );
    assert!(json["commands"].as_array().unwrap().iter().any(|row| {
        row["name"] == "run"
            && row["aliases"]
                .as_array()
                .unwrap()
                .iter()
                .any(|alias| alias == "shell")
            && row["aliases"]
                .as_array()
                .unwrap()
                .iter()
                .any(|alias| alias == "rn")
            && row["aliases"]
                .as_array()
                .unwrap()
                .iter()
                .any(|alias| alias == "--jason")
            && row["primary_invocation"] == "tokenzero run --json -- <command>"
    }));
    assert!(json["commands"].as_array().unwrap().iter().any(|row| {
        row["name"] == "find"
            && row["aliases"]
                .as_array()
                .unwrap()
                .iter()
                .any(|alias| alias == "search")
    }));
    assert!(json["commands"].as_array().unwrap().iter().any(|row| {
        row["name"] == "capabilities"
            && row["json"] == true
            && row["aliases"]
                .as_array()
                .unwrap()
                .iter()
                .any(|alias| alias == "--jason")
    }));
    assert!(json["commands"].as_array().unwrap().iter().any(|row| {
        row["name"] == "robot-docs guide"
            && row["mutates"] == false
            && row["aliases"]
                .as_array()
                .unwrap()
                .iter()
                .any(|alias| alias == "robot-docs commands")
    }));
    assert!(
        !json["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["name"] == "codemode")
    );
    assert_eq!(json["aggregate_codemode"]["owner"], "zerostack");
    assert_eq!(json["aggregate_codemode"]["local_execution"], false);
    assert_eq!(
        json["aggregate_codemode"]["worker_transport"],
        "raw-worker-v2"
    );
    assert!(json["commands"].as_array().unwrap().iter().any(|row| {
        row["name"] == "doctor"
            && row["aliases"]
                .as_array()
                .unwrap()
                .iter()
                .any(|alias| alias == "doctor statuz")
    }));
    assert!(json["commands"].as_array().unwrap().iter().any(|row| {
        row["name"] == "pulse"
            && row["aliases"]
                .as_array()
                .unwrap()
                .iter()
                .any(|alias| alias == "pulse stats")
    }));
    assert!(
        json["exit_codes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["code"] == 2 && row["label"] == "usage")
    );
    assert!(
        json["canonical_invocations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item == "tokenzero --robot-help")
    );
    assert!(
        json["canonical_invocations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item == "tokenzero robot-help")
    );
    assert!(
        json["canonical_invocations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item == "tokenzero robot-docs guide")
    );
    assert!(
        json["canonical_invocations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item == "tokenzero search <query> <path> --json")
    );
    assert!(
        json["canonical_invocations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item == "tokenzero install status --json")
    );
    assert!(
        json["commands"].as_array().unwrap().len() >= 10,
        "should list many commands"
    );
}

#[test]
fn capabilities_declares_token_engine_kernel_orifices() {
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["capabilities", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    let kernel = &json["kernel_orifices"];
    assert_eq!(kernel["owner"], "tokenzero");
    assert_eq!(kernel["api"], "zero_abi::TokenEngine");
    assert_eq!(kernel["codemode_binding_status"], "noncanonical_v6_compat");
    let methods = kernel["methods"]
        .as_array()
        .expect("kernel methods")
        .iter()
        .map(|v| v.as_str().expect("method name"))
        .collect::<Vec<_>>();
    assert_eq!(
        methods,
        ["measure", "certify", "project", "compress", "expand"]
    );
    let facing = kernel["model_facing"]
        .as_array()
        .expect("model_facing")
        .iter()
        .map(|v| v.as_str().expect("z.*"))
        .collect::<Vec<_>>();
    for name in ["z.measure", "z.project", "z.compress", "z.expand"] {
        assert!(facing.contains(&name), "missing {name}");
    }
    assert!(
        !facing
            .iter()
            .any(|name| *name == "z.read" || *name == "z.find" || *name == "z.run"),
        "z.read/z.find/z.run are ZeroStack host ops, not TokenEngine"
    );
    let not_engine = kernel["not_token_engine"]
        .as_array()
        .expect("not_token_engine")
        .iter()
        .map(|v| v.as_str().expect("name"))
        .collect::<Vec<_>>();
    for name in [
        "z.read",
        "z.find",
        "z.run",
        "zero.read",
        "zero.token.expand",
    ] {
        assert!(not_engine.contains(&name), "must disclaim {name}");
    }
    assert_eq!(
        json["surface_parity"]["name_contract"]["kernel"],
        "TokenEngine measure/certify/project/compress/expand via z.measure/z.project/z.compress/z.expand"
    );
    assert_eq!(
        json["aggregate_codemode"]["status"],
        "noncanonical_v6_compat"
    );
}

#[test]
fn capabilities_does_not_advertise_dead_tokenzero_mcp_bin() {
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["capabilities", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    let mcp_bin = &json["packaging_orifices"]["tokenzero-mcp"];
    assert_eq!(mcp_bin["bin"], false);
    assert_eq!(mcp_bin["status"], "not_a_workspace_bin");
    assert_eq!(
        mcp_bin["source_present"],
        "crates/tokenzero-cli/src/bin/tokenzero_mcp.rs"
    );
    assert_eq!(
        mcp_bin["available_in_this_build"],
        cfg!(feature = "surface-mcp")
    );
    let mcp_server = json["commands"]
        .as_array()
        .expect("commands")
        .iter()
        .find(|row| row["name"] == "mcp-server")
        .expect("mcp-server row");
    assert_eq!(
        mcp_server["available_in_this_build"],
        cfg!(feature = "surface-mcp")
    );
    let tools = json["mcp_tools"].as_array().expect("mcp_tools");
    assert!(
        tools
            .iter()
            .all(|row| row["available_in_this_build"] == false || cfg!(feature = "surface-mcp")),
        "classic MCP tools must not claim available_in_this_build without surface-mcp"
    );
}

#[test]
fn doctor_json_does_not_claim_uncompiled_mcp_orifice() {
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["mcp"]["ready"], cfg!(feature = "surface-mcp"));
    assert_eq!(json["mcp"]["live"], cfg!(feature = "surface-mcp"));
    assert_eq!(json["mcp_orifice"]["status"], "not_a_workspace_bin");
    assert!(
        json["mcp"]["server"].is_null(),
        "doctor must not name tokenzero mcp-server as live: {}",
        json["mcp"]
    );
    let check = json["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|row| row["id"] == "mcp_server_entrypoint_declared")
        .expect("mcp_server_entrypoint_declared");
    assert_eq!(check["ok"], false);
    assert_eq!(check["severity"], "info");

    let caps = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["doctor", "capabilities", "--json"])
        .output()
        .unwrap();
    assert!(
        caps.status.success(),
        "{}",
        String::from_utf8_lossy(&caps.stderr)
    );
    let caps: Value = serde_json::from_slice(&caps.stdout).unwrap();
    assert_eq!(caps["mcp_orifice"]["live"], false);
    assert_eq!(caps["mcp_orifice"]["status"], "not_a_workspace_bin");
    let detector = caps["detectors"]
        .as_array()
        .expect("detectors")
        .iter()
        .find(|row| row["id"] == "tz-mcp-server-entrypoint-declared")
        .expect("tz-mcp-server-entrypoint-declared");
    assert_eq!(detector["severity"], "info");

    let planned = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["install", "--plan", "--mcp", "--json"])
        .output()
        .unwrap();
    assert!(
        planned.status.success(),
        "{}",
        String::from_utf8_lossy(&planned.stderr)
    );
    let planned: Value = serde_json::from_slice(&planned.stdout).unwrap();
    assert_eq!(planned["mcp_orifice"]["live"], false);
    assert_eq!(planned["mcp_orifice"]["status"], "not_a_workspace_bin");
}

#[test]
fn clap_visible_subcommands_are_in_capabilities() {
    let help = Command::cargo_bin("tokenzero")
        .unwrap()
        .arg("--help")
        .output()
        .unwrap();
    assert!(
        help.status.success(),
        "{}",
        String::from_utf8_lossy(&help.stderr)
    );
    let stdout = String::from_utf8_lossy(&help.stdout);
    let mut names = Vec::new();
    let mut in_commands = false;
    for line in stdout.lines() {
        if line.starts_with("Commands:") {
            in_commands = true;
            continue;
        }
        if in_commands {
            if line.is_empty() {
                break;
            }
            let trimmed = line.trim();
            if let Some(name) = trimmed.split_whitespace().next() {
                if name.starts_with('-') || name == "help" {
                    continue;
                }
                names.push(name.trim_end_matches(',').to_string());
            }
        }
    }
    assert!(
        names.contains(&"read".to_string()) && names.contains(&"expand".to_string()),
        "clap --help must list live verbs; got {names:?}"
    );

    let caps = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["capabilities", "--json"])
        .output()
        .unwrap();
    assert!(caps.status.success());
    let json: Value = serde_json::from_slice(&caps.stdout).unwrap();
    let commands = json["commands"].as_array().expect("commands");
    let experimental = json["experimental_commands"]
        .as_array()
        .expect("experimental_commands");
    for name in &names {
        let in_commands = commands.iter().any(|row| {
            let listed = row["name"].as_str().unwrap_or("");
            listed == name
                || listed.starts_with(&format!("{name} "))
                || row["aliases"]
                    .as_array()
                    .is_some_and(|aliases| aliases.iter().any(|alias| alias.as_str() == Some(name)))
        });
        let in_experimental = experimental.iter().any(|row| row.as_str() == Some(name));
        assert!(
            in_commands || in_experimental,
            "clap visible {name:?} is missing from capabilities.commands and experimental_commands"
        );
    }
}

#[test]
fn cli_install_smoke_defaults_to_plan_and_gates_apply() {
    let work = tempdir().unwrap();
    let help = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["install-smoke", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("--apply"), "{help}");
    assert!(help.contains("disposable temporary root"), "{help}");

    let planned = Command::cargo_bin("tokenzero")
        .unwrap()
        .current_dir(work.path())
        .args(["install-smoke", "--json"])
        .output()
        .unwrap();
    assert!(
        planned.status.success(),
        "{}",
        String::from_utf8_lossy(&planned.stderr)
    );
    let planned: Value = serde_json::from_slice(&planned.stdout).unwrap();
    assert_eq!(planned["mode"], "plan");
    assert_eq!(planned["apply_requested"], false);
    assert_eq!(planned["scope"], "disposable_temporary_root");
    assert!(planned["applied"].is_null());
    assert!(planned["rollback"].is_null());
    assert_eq!(planned["artifact_write_requested"], false);
    assert_eq!(planned["global_writes"], false);
    assert_eq!(planned["status"], "ok");
    assert_eq!(planned["ok"], true);
    assert_eq!(planned["checks"]["plan_observed"], true);
    assert_eq!(planned["checks"]["planned_writes_local"], true);
    assert_eq!(planned["checks"]["planned_root_unchanged"], true);
    assert_eq!(planned["checks"]["transition_observed"], true);
    assert!(planned["checks"]["apply_observed"].is_null());
    assert!(planned["checks"]["applied_targets_observed"].is_null());
    assert!(
        !work.path().join("results").exists(),
        "default install-smoke must not write an artifact tree"
    );

    let applied = Command::cargo_bin("tokenzero")
        .unwrap()
        .current_dir(work.path())
        .args(["install-smoke", "--apply", "--json"])
        .output()
        .unwrap();
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    let applied: Value = serde_json::from_slice(&applied.stdout).unwrap();
    assert_eq!(applied["mode"], "apply_and_rollback");
    assert_eq!(applied["apply_requested"], true);
    assert!(!applied["applied"].is_null());
    assert!(!applied["rollback"].is_null());
    assert_eq!(applied["artifact_write_requested"], false);
    assert_eq!(applied["global_writes"], false);
    assert_eq!(applied["status"], "ok");
    assert_eq!(applied["ok"], true);
    assert_eq!(applied["checks"]["plan_observed"], true);
    assert_eq!(applied["checks"]["apply_observed"], true);
    assert_eq!(applied["checks"]["applied_targets_observed"], true);
    assert_eq!(applied["checks"]["rollback_observed"], true);
    assert_eq!(
        applied["checks"]["restoration_scope"],
        "planned_target_bytes_and_presence"
    );
    assert_eq!(applied["checks"]["exact_restoration_observed"], true);
    assert_eq!(applied["checks"]["transition_observed"], true);
    assert!(!work.path().join("results").exists());
}

#[test]
fn cli_mutator_errors_name_safe_alternatives() {
    let work = tempdir().unwrap();

    let edit = tokenzero_cmd()
        .current_dir(work.path())
        .args([
            "edit",
            "missing.txt",
            "--edits-json",
            r#"[{"find":"x","replace":"y"}]"#,
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(edit.status.code(), Some(1));
    let edit: Value = serde_json::from_slice(&edit.stdout).unwrap();
    assert!(
        edit["error"]["repair"]
            .as_str()
            .unwrap_or_default()
            .contains("tokenzero edit <path> --edits-json '<json>' --dry-run --json"),
        "{edit}"
    );

    let missing_root = work.path().join("missing-client-root");
    let clients = tokenzero_cmd()
        .args([
            "clients",
            "rollback",
            "missing-id",
            "--root",
            missing_root.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(clients.status.code(), Some(1));
    let clients_stderr = String::from_utf8_lossy(&clients.stderr);
    assert!(
        clients_stderr.contains("tokenzero clients doctor --json"),
        "{clients_stderr}"
    );

    let cache_root = work.path().join("cache-root");
    std::fs::create_dir_all(&cache_root).unwrap();
    let migrate = tokenzero_cmd()
        .args([
            "cache",
            "migrate-refs",
            "--root",
            cache_root.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        migrate.status.success(),
        "{}",
        String::from_utf8_lossy(&migrate.stderr)
    );
    assert!(migrate.stderr.is_empty());
    let migrate_json: Value = serde_json::from_slice(&migrate.stdout).unwrap();
    assert!(
        migrate_json.get("safe_alternative").is_none(),
        "{migrate_json}"
    );

    let cleanup = tokenzero_cmd()
        .args([
            "cache",
            "migrate-cleanup",
            "--root",
            cache_root.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(cleanup.status.code(), Some(1));
    let cleanup_json: Value = serde_json::from_slice(&cleanup.stdout).unwrap();
    assert!(
        cleanup_json.get("safe_alternative").is_none(),
        "{cleanup_json}"
    );
    let cleanup_stderr = String::from_utf8_lossy(&cleanup.stderr);
    assert!(
        cleanup_stderr.contains("tokenzero cache migrate-verify --json"),
        "{cleanup_stderr}"
    );

    let blocked_root = work.path().join("not-a-directory");
    std::fs::write(&blocked_root, "block").unwrap();
    let install = tokenzero_cmd()
        .args([
            "install",
            "--root",
            blocked_root.to_str().unwrap(),
            "--apply",
            "--mcp",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(install.status.code(), Some(1));
    let install_stderr = String::from_utf8_lossy(&install.stderr);
    assert!(
        install_stderr.contains("tokenzero install --plan --json"),
        "{install_stderr}"
    );

    let rollback = tokenzero_cmd()
        .args([
            "install",
            "--root",
            missing_root.to_str().unwrap(),
            "--rollback",
            "missing-id",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(rollback.status.code(), Some(1));
    let rollback_stderr = String::from_utf8_lossy(&rollback.stderr);
    assert!(
        rollback_stderr.contains("tokenzero doctor --json"),
        "{rollback_stderr}"
    );
}

#[test]
fn cli_agent_contract_outputs_are_deterministic_and_env_clean() {
    let first = tokenzero_with_agent_env(&["capabilities", "--json"]);
    let second = tokenzero_with_agent_env(&["capabilities", "--json"]);

    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(first.stderr.is_empty());
    assert!(second.stderr.is_empty());
    assert_eq!(first.stdout, second.stdout);
    assert_no_ansi(&first.stdout);
    let json: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(json["schema_version"], "tokenzero.capabilities.v1");
    let features: Vec<&str> = json["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|feature| feature.as_str().unwrap())
        .collect();
    let expected = vec![
        "capabilities_json",
        "exact_recovery_refs",
        "intent_inference_aliases",
        "json_output",
        "non_tty_output_discipline",
        "pipeline_rerun_guidance",
        "robot_docs_guide",
        "status_truth_shell",
    ];
    assert_eq!(features, expected);
}

#[test]
fn cli_robot_docs_read_search_and_run_are_env_clean() {
    let dir = tempdir().unwrap();
    let sample = dir.path().join("sample.txt");
    std::fs::write(&sample, "TokenZero\n").unwrap();
    let allowed_root = dir.path().to_str().unwrap();
    let sample = sample.to_str().unwrap();

    for args in [
        &["robot-docs", "guide"][..],
        &["robot-docs", "commands"][..],
        &["robot-docs", "examples"][..],
        &["read", sample, "--allowed-root", allowed_root, "--json"][..],
        &[
            "search",
            "TokenZero",
            sample,
            "--allowed-root",
            allowed_root,
            "--json",
        ][..],
        &["run", "--json", "rustc", "--version"][..],
    ] {
        let output = tokenzero_with_agent_env(args);
        assert!(
            output.status.success(),
            "{args:?}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_no_ansi(&output.stdout);
        assert_no_ansi(&output.stderr);
        if args.contains(&"--json") {
            serde_json::from_slice::<Value>(&output.stdout).unwrap_or_else(|err| {
                panic!(
                    "{args:?}: {err}\n{}",
                    String::from_utf8_lossy(&output.stdout)
                )
            });
        }
    }
}

#[test]
fn cli_agent_contract_aliases_recover_common_wrong_invocations() {
    let capabilities = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["capabilites", "--json"])
        .output()
        .unwrap();

    assert!(
        capabilities.status.success(),
        "{}",
        String::from_utf8_lossy(&capabilities.stderr)
    );
    let json: Value = serde_json::from_slice(&capabilities.stdout).unwrap();
    assert_eq!(json["schema_version"], "tokenzero.capabilities.v1");
    assert!(json["commands"].as_array().unwrap().iter().any(|row| {
        row["name"] == "capabilities"
            && row["aliases"]
                .as_array()
                .unwrap()
                .iter()
                .any(|alias| alias == "capabilites")
    }));

    for args in [
        &["robot-doc", "manual"][..],
        &["--robot-help"][..],
        &["robot-help"][..],
        &["robot-docs", "commands"][..],
        &["robot-docs", "examples"][..],
    ] {
        let output = Command::cargo_bin("tokenzero")
            .unwrap()
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.stdout.is_empty(), "{args:?} produced empty stdout");
    }
}

#[test]
fn cli_safe_subcommand_recoveries_choose_read_or_plan_surfaces() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_str().unwrap();
    let cache = dir.path().join("cache.json");
    let cache = cache.to_str().unwrap();

    let cache_status = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["cache", "statuz", "--root", root, "--json"])
        .output()
        .unwrap();
    assert!(
        cache_status.status.success(),
        "{}",
        String::from_utf8_lossy(&cache_status.stderr)
    );
    let json: Value = serde_json::from_slice(&cache_status.stdout).unwrap();
    assert_eq!(json["tool"], "mem");
    assert_eq!(json["status"], "ok");

    let pulse_stats = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["pulse", "--root", root, "--json", "stats"])
        .output()
        .unwrap();
    assert!(
        pulse_stats.status.success(),
        "{}",
        String::from_utf8_lossy(&pulse_stats.stderr)
    );
    let json: Value = serde_json::from_slice(&pulse_stats.stdout).unwrap();
    assert!(json["event_count"].is_number());

    for subcommand in ["status", "statuz"] {
        let doctor = Command::cargo_bin("tokenzero")
            .unwrap()
            .args([
                "doctor",
                subcommand,
                "--root",
                root,
                "--cache-path",
                cache,
                "--json",
            ])
            .output()
            .unwrap();
        assert!(
            doctor.status.success(),
            "{subcommand}: {}",
            String::from_utf8_lossy(&doctor.stderr)
        );
        let json: Value = serde_json::from_slice(&doctor.stdout).unwrap();
        assert_eq!(json["schema_version"], "tokenzero.doctor.health.v1");
        assert_eq!(json["status"], "ok");
    }

    let install_plan = Command::cargo_bin("tokenzero")
        .unwrap()
        .args([
            "install", "plan", "--root", root, "--mcp", "--agent", "codex", "--json",
        ])
        .output()
        .unwrap();
    assert!(
        install_plan.status.success(),
        "{}",
        String::from_utf8_lossy(&install_plan.stderr)
    );
    let json: Value = serde_json::from_slice(&install_plan.stdout).unwrap();
    assert_eq!(json["status"], "planned");
    assert_eq!(json["dry_run"], true);
    assert!(!json["writes"].as_array().unwrap().is_empty());

    let install_status = Command::cargo_bin("tokenzero")
        .unwrap()
        .args([
            "install", "status", "--global", "--mcp", "--root", root, "--agent", "codex", "--json",
        ])
        .output()
        .unwrap();
    assert!(
        install_status.status.success(),
        "{}",
        String::from_utf8_lossy(&install_status.stderr)
    );
    let json: Value = serde_json::from_slice(&install_status.stdout).unwrap();
    assert_eq!(json["schema_version"], "tokenzero.clients.v1");
    assert_eq!(json["command"], "clients detect");
    assert_eq!(json["agents"].as_array().unwrap()[0], "codex");
}

#[test]
fn cli_run_recovers_common_wrong_json_and_timeout_invocations() {
    // Parent JSON / timeout typo recovery applies only to options parsed before
    // the child executable. Trailing --json stays in child argv (CE-P02-01).
    let cases: &[&[&str]] = &[
        &["run", "--jsno", "rustc", "--version"],
        &["run", "--jason", "rustc", "--version"],
        &["run", "--json", "rustc", "--version"],
        &["run", "--timout", "30", "--json", "rustc", "--version"],
        &["shell", "--jason", "rustc", "--version"],
        &["rn", "--json", "rustc", "--version"],
    ];

    for args in cases {
        let output = Command::cargo_bin("tokenzero")
            .unwrap()
            .env("TOKENZERO_SLIM_ENVELOPE", "0")
            .args(*args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
            panic!(
                "{args:?}: {err}\n{}",
                String::from_utf8_lossy(&output.stdout)
            )
        });
        assert_eq!(json["status"], "ok", "{args:?}");
        assert_eq!(json["telemetry"]["command_success"], true, "{args:?}");
        assert!(
            json["telemetry"]["argv"]
                .as_array()
                .unwrap()
                .iter()
                .any(|arg| arg == "rustc"),
            "{args:?}"
        );
    }
}

#[test]
fn cli_common_verb_typos_recover_to_canonical_verbs() {
    // R-011: rn/reed/instal are table-driven top-level verb recoveries that
    // reach the canonical surface and its typed envelope. Run-family recovery
    // never rewrites child argv after `--` (CE-P02-01).
    let dir = tempdir().unwrap();
    let sample = dir.path().join("sample.txt");
    std::fs::write(&sample, "TokenZero\n").unwrap();
    let sample = sample.to_str().unwrap();
    let allowed_root = dir.path().to_str().unwrap();

    // reed -> read: typed read envelope, status ok, canonical verb recorded.
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .env("TOKENZERO_SLIM_ENVELOPE", "0")
        .args(["reed", sample, "--allowed-root", allowed_root, "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "reed -> read\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "reed -> read: {err}\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    assert_eq!(json["status"], "ok", "reed -> read");
    assert!(
        json["visible"]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("TokenZero"),
        "typed read envelope must serve the file content: {json}"
    );

    // instal plan -> install --plan: dry-run plan JSON, no apply.
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .current_dir(dir.path())
        .args([
            "instal",
            "plan",
            "--root",
            allowed_root,
            "--mcp",
            "--agent",
            "codex",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "instal plan -> install plan\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "instal plan: {err}\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    assert_eq!(json["status"], "planned", "instal plan");
    assert_eq!(json["dry_run"], true, "instal plan");

    // rn with `--`: child argv after the delimiter passes through unchanged.
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["rn", "--", "printf", "%s\n", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "rn -- child\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("stdout:\n--json\n"),
        "child argv after -- must be untouched; got {stdout}"
    );
    assert!(
        serde_json::from_slice::<Value>(&output.stdout).is_err(),
        "parent must not promote child --json after the delimiter"
    );
}

#[test]
fn cli_run_preserves_trailing_child_json_without_delimiter() {
    // CE-P02-01: after the first child executable token, --json belongs to the
    // child argv and must not promote the parent envelope.
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["run", "printf", "%s\n", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        serde_json::from_slice::<Value>(&output.stdout).is_err(),
        "parent must not steal trailing --json into the JSON envelope; got {stdout}"
    );
    assert!(
        stdout.contains("stdout:\n--json\n"),
        "child must receive and print trailing --json; got {stdout}"
    );
    assert!(
        stdout.contains("exit_code: 0"),
        "text-mode run envelope expected; got {stdout}"
    );
    assert!(
        stdout.contains("combined_ref: tz://blob/"),
        "exact combined recovery ref expected; got {stdout}"
    );

    let full_child = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["run", "printf", "%s\n", "--json=full"])
        .output()
        .unwrap();
    assert!(full_child.status.success());
    let full_child_stdout = String::from_utf8_lossy(&full_child.stdout);
    assert!(
        full_child_stdout.contains("stdout:\n--json=full\n"),
        "child must receive the exact trailing argument: {full_child_stdout}"
    );
    assert!(
        serde_json::from_slice::<Value>(&full_child.stdout).is_err(),
        "trailing --json=full must not select the parent forensic envelope"
    );
}

#[test]
fn cli_run_inline_shell_envelope_handles_empty_stdout() {
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["run", "printf", ""])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("stdout:\n"), "{stdout}");
    assert!(stdout.contains("combined_ref: tz://blob/"), "{stdout}");
    assert!(stdout.contains("exit_code: 0"), "{stdout}");
}

#[test]
fn cli_run_nonzero_exit_keeps_existing_failure_envelope() {
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["run", "sh", "-c", "printf boom; exit 7"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("exit_code: 7"), "{stdout}");
    assert!(stdout.contains("combined_ref: tz://blob/"), "{stdout}");
}

#[test]
fn cli_mcp_tool_name_suggests_cli_verb_not_nearest_string() {
    // bara (R-016): tz_read must suggest 'read', never clap's generic 'tree'.
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["tz_read", "some/file.rs"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("the CLI verb is 'read'"), "{stderr}");
    assert!(
        stderr.contains("corrected command: tokenzero read some/file.rs"),
        "{stderr}"
    );
    assert!(!stderr.contains("'tree'"), "{stderr}");

    for (tool, verb) in [("tz_mem", "mem"), ("tz_discover", "discover")] {
        let output = Command::cargo_bin("tokenzero")
            .unwrap()
            .arg(tool)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains(&format!("the CLI verb is '{verb}'")),
            "{tool}: {stderr}"
        );
    }

    // Aggregate control schemas have no engine-local CLI route.
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .arg("tz_execute_code")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("the CLI verb is 'codemode'"), "{stderr}");
    assert!(!stderr.contains("tokenzero codemode"), "{stderr}");

    // Non-MCP typos keep clap's generic suggestion path.
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["tre"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("similar subcommand"), "{stderr}");
    assert!(!stderr.contains("MCP tool name"), "{stderr}");
}

#[test]
fn cli_usage_errors_name_exact_corrected_invocation() {
    // dzb2 (R-003): every usage error names the exact corrected command.
    let cases: &[(&[&str], &str)] = &[
        (&["read"], "corrected command: tokenzero read <path> --json"),
        (
            &["find"],
            "corrected command: tokenzero find --json <QUERY>",
        ),
        (&["edit"], "corrected command: tokenzero edit --json <PATH>"),
        (
            &["run"],
            "corrected command: tokenzero run --json -- <command>",
        ),
        (
            &["expand"],
            "corrected command: tokenzero expand <tz-ref> --raw",
        ),
    ];
    for (args, needle) in cases {
        let output = Command::cargo_bin("tokenzero")
            .unwrap()
            .args(*args)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?} must be a usage error: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            combined.contains(needle),
            "{args:?} missing {needle:?}: {combined}"
        );
    }
}

#[test]
fn cli_robot_triage_root_alias_matches_doctor_envelope() {
    // pec5 (R-001): root mega-command aliases reach doctor --robot-triage.
    for args in [
        vec!["--robot-triage"],
        vec!["robot-triage"],
        vec!["doctor", "--robot-triage"],
    ] {
        let output = Command::cargo_bin("tokenzero")
            .unwrap()
            .args(&args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            json["schema_version"], "tokenzero.doctor.robot_triage.v1",
            "{args:?}"
        );
        for key in [
            "health",
            "quick_ref",
            "recommendations",
            "commands",
            "findings",
            "recommended_command",
        ] {
            assert!(json.get(key).is_some(), "{args:?} missing {key}");
        }
    }

    // capabilities pins the triage schema.
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["capabilities", "--json"])
        .output()
        .unwrap();
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        json["output_schemas"]["doctor_robot_triage"]["schema_version"],
        "tokenzero.doctor.robot_triage.v1"
    );
}

#[test]
fn cli_flag_typo_distance_one_offers_corrected_command() {
    // bdki (R-002): distance-1 flag typo -> did-you-mean + corrected command.
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["read", "--jsonn", "some/file.rs"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("did you mean: '--json'"), "{stderr}");
    assert!(
        stderr.contains("corrected command: tokenzero read --json some/file.rs"),
        "{stderr}"
    );

    // A real flag placed before the verb is reordered, not renamed.
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["--jsno", "read", "some/file.rs"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("belongs after the subcommand"), "{stderr}");
    assert!(
        stderr.contains("corrected command: tokenzero read --jsno some/file.rs"),
        "{stderr}"
    );

    // Far-off typos get no misleading suggestion (rejects --exlpain->--help).
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["grep", "--exlpain", "needle"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("unexpected argument '--exlpain'"),
        "{stderr}"
    );
    assert!(!stderr.contains("did you mean"), "{stderr}");
    assert!(!stderr.contains("similar argument"), "{stderr}");
    assert!(stderr.contains("valid flags for 'grep'"), "{stderr}");
    assert!(stderr.contains("--json"), "{stderr}");
    assert!(stderr.contains("--max-files"), "{stderr}");
    assert!(
        stderr.contains("Usage: tokenzero grep [OPTIONS] <QUERY> [PATH]..."),
        "{stderr}"
    );
    assert!(!stderr.contains("Usage: tokenzero grep --help"), "{stderr}");
    assert!(!stderr.contains("try '--help'"), "{stderr}");

    // A global flag typo must not match a subcommand-only flag family.
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["--versio", "read", "some/file.rs"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("valid flags for 'read'"), "{stderr}");
    assert!(!stderr.contains("did you mean: '--version'"), "{stderr}");

    // The same distance-1 spelling is corrected in the global family.
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .arg("--versio")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("did you mean: '--version'"), "{stderr}");
    assert!(
        stderr.contains("corrected command: tokenzero --version"),
        "{stderr}"
    );
}

#[test]
fn cli_run_json_child_exit_default_mirrors_child_failure() {
    // nt0i (1cwf flip): JSON run mirrors the child exit code by default. This
    // test requests the full forensic envelope because it inspects telemetry.
    let default = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["run", "--json=full", "sh", "-c", "printf boom; exit 7"])
        .output()
        .unwrap();
    assert_eq!(default.status.code(), Some(7), "default mirrors child exit");
    let json: Value = serde_json::from_slice(&default.stdout).unwrap();
    assert_eq!(json["status"], "ok", "envelope content unchanged");
    assert_eq!(json["telemetry"]["command_success"], false);
    assert_eq!(json["telemetry"]["exit_code"], 7);

    let legacy = Command::cargo_bin("tokenzero")
        .unwrap()
        .env("TOKENZERO_RUN_CHILD_EXIT", "0")
        .args(["run", "--json=full", "sh", "-c", "printf boom; exit 7"])
        .output()
        .unwrap();
    assert!(
        legacy.status.success(),
        "explicit opt-out keeps the legacy exit-0 envelope contract"
    );
    let json: Value = serde_json::from_slice(&legacy.stdout).unwrap();
    assert_eq!(json["status"], "ok");
    assert_eq!(json["telemetry"]["exit_code"], 7);
}

#[test]
fn cli_run_parent_json_keeps_inline_payload_unwrapped() {
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["run", "--json=full", "printf", "%s\n", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["visible"]["text"], "--json");
    assert_eq!(json["telemetry"]["output_strategy"], "inline_shell");
    assert_eq!(json["telemetry"]["exit_code"], 0);
    assert!(
        json["refs"]
            .as_array()
            .is_some_and(|refs| refs.iter().any(|record| record["kind"] == "combined"))
    );
}

#[test]
fn cli_search_and_capabilities_json_typo_aliases_recover() {
    let capabilities_cases: &[&[&str]] =
        &[&["capabilities", "--jsno"], &["capabilities", "--jason"]];

    for args in capabilities_cases {
        let output = Command::cargo_bin("tokenzero")
            .unwrap()
            .args(*args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["schema_version"], "tokenzero.capabilities.v1");
    }

    let search = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["search", "TokenZero", "AGENTS.md", "--json"])
        .output()
        .unwrap();

    assert!(
        search.status.success(),
        "{}",
        String::from_utf8_lossy(&search.stderr)
    );
    let json: Value = serde_json::from_slice(&search.stdout).unwrap();
    assert_eq!(json["status"], "ok");
    assert_eq!(json["tool"], "find");
}

#[test]
fn cli_json_everywhere_read_side_matrix() {
    // n3fx (R-012): every read-side command advertised by capabilities must
    // accept --json and emit parseable JSON (exit 0 success or a structured
    // JSON error), and its schema must be listed in capabilities.output_schemas.
    let capabilities = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["capabilities", "--json"])
        .output()
        .unwrap();
    assert!(
        capabilities.status.success(),
        "{}",
        String::from_utf8_lossy(&capabilities.stderr)
    );
    let caps: Value = serde_json::from_slice(&capabilities.stdout).unwrap();
    let output_schemas = caps["output_schemas"]
        .as_object()
        .expect("capabilities output_schemas");
    // Read-side JSON verbs = mutates=false and json=true. hook claude-code is
    // the only such row without a --json output flag (it is a stdin JSON
    // adapter whose output is a rewritten command), so the matrix excludes it;
    // every other advertised row is exercised below.
    let advertised = caps["commands"]
        .as_array()
        .expect("capabilities commands")
        .iter()
        .filter(|row| {
            row["mutates"] == false && row["json"] == true && row["name"] != "hook claude-code"
        })
        .map(|row| row["name"].as_str().unwrap())
        .collect::<Vec<_>>();

    let dir = tempdir().unwrap();
    let sample = dir.path().join("sample.txt");
    std::fs::write(&sample, "TokenZero\n").unwrap();
    let root = dir.path().to_str().unwrap();
    let sample = sample.to_str().unwrap();

    // (name, output_schema_key, args). schema_key is the capabilities
    // output_schemas entry; it equals the command name except --robot-triage,
    // whose schema is documented under doctor_robot_triage.
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "read",
            "read",
            &["read", sample, "--allowed-root", root, "--json"],
        ),
        (
            "find",
            "find",
            &["find", "TokenZero", root, "--allowed-root", root, "--json"],
        ),
        (
            "grep",
            "grep",
            &["grep", "TokenZero", root, "--allowed-root", root, "--json"],
        ),
        (
            "glob",
            "glob",
            &["glob", "*.txt", root, "--allowed-root", root, "--json"],
        ),
        (
            "tree",
            "tree",
            &["tree", root, "--allowed-root", root, "--json"],
        ),
        ("recall", "recall", &["recall", "zzz-no-match", "--json"]),
        (
            "fetch",
            "fetch",
            &["fetch", "http://127.0.0.1:1/zzz", "--json"],
        ),
        ("expand", "expand", &["expand", "tz://o/0/0", "--json"]),
        ("mem", "mem", &["mem", "--root", root, "--json"]),
        ("pulse", "pulse", &["pulse", "--root", root, "--json"]),
        ("doctor", "doctor", &["doctor", "--json"]),
        ("capabilities", "capabilities", &["capabilities", "--json"]),
        ("discover", "discover", &["discover", "--json"]),
        ("stats", "stats", &["stats", "--root", root, "--json"]),
        (
            "session-ledger",
            "session-ledger",
            &["session-ledger", "--root", root, "--json"],
        ),
        (
            "session-open",
            "session-open",
            &["session-open", "--root", root, "--json"],
        ),
        (
            "cache-pack",
            "cache-pack",
            &["cache-pack", "--root", root, "--json"],
        ),
        (
            "quote",
            "quote",
            &["quote", "--platform", "sh", "--json", "--", "echo", "hi"],
        ),
        (
            "rewrite",
            "rewrite",
            &["rewrite", "--json", "--", "echo", "hi"],
        ),
        ("ingest", "ingest", &["ingest", sample, "--json"]),
        ("run", "run", &["run", "--json", "--", "echo", "hi"]),
        ("clients", "clients", &["clients", "detect", "--json"]),
        ("--robot-triage", "doctor_robot_triage", &["--robot-triage"]),
    ];

    // The advertised read-side set and the exercised set must not drift apart.
    let mut exercised = cases.iter().map(|(name, _, _)| *name).collect::<Vec<_>>();
    exercised.sort_unstable();
    let mut advertised = advertised.clone();
    advertised.sort_unstable();
    assert_eq!(
        exercised, advertised,
        "matrix rows must mirror advertised read-side commands"
    );

    for (name, schema_key, args) in cases {
        assert!(
            output_schemas.contains_key(*schema_key),
            "read-side command {name} is missing from capabilities.output_schemas (key {schema_key})"
        );
        let mut child = Command::cargo_bin("tokenzero").unwrap();
        child
            .args(*args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = child.spawn().unwrap();
        if let Some(stdin) = child.stdin.take() {
            let mut stdin = stdin;
            stdin.write_all(b"").unwrap();
            stdin.flush().unwrap();
        }
        let output = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed: Value = serde_json::from_str(&stdout).unwrap_or_else(|err| {
            panic!(
                "{name} --json produced non-JSON stdout: {err}\n{stdout}\nstderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        });
        if !output.status.success() {
            assert!(
                parsed.get("error").is_some()
                    || parsed.get("status") == Some(&Value::String("error".into())),
                "{name} --json exited {} without a structured JSON error: {stdout}",
                output.status.code().unwrap_or(-1)
            );
        }
        // n3fx (R-012): when the declared output schema names a version, the
        // emitted JSON must carry it (schema_version or schema), so metadata
        // cannot drift from actual output. quote stays shape-only.
        if let Some(expected) = output_schemas[*schema_key]
            .get("schema_version")
            .and_then(Value::as_str)
        {
            let actual = parsed
                .get("schema_version")
                .or_else(|| parsed.get("schema"))
                .and_then(Value::as_str);
            assert_eq!(
                actual,
                Some(expected),
                "{name} stdout schema {actual:?} does not match declared output_schemas[{schema_key}].schema_version {expected:?}: {stdout}"
            );
        }
    }
}
