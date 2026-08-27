use crate::render::domain::{
    is_expected_false_exit, is_repo_inventory_command, is_search_no_match, requested_output_lines,
};
use crate::shell_family::shell_family_with_combined;
use crate::shell_parse::{
    failed_segment, looks_diagnostic, masking_warning, pipeline_rerun_command, repeated_line_count,
    shell_syntax_summary_for_status,
};
use crate::{CommandStatus, Mode, PolicyDecision};

pub(crate) fn command_succeeded(
    exit_code: Option<i32>,
    search_no_match: bool,
    expected_false_exit: bool,
    timed_out: bool,
    _failed_segment: Option<&str>,
) -> bool {
    // Status agrees with the process exit code (tokenzero-3ry6). A
    // failed_segment on exit 0 is advisory masking, not command_failed.
    !timed_out && (exit_code == Some(0) || search_no_match || expected_false_exit)
}

fn command_status_label(
    exit_code: Option<i32>,
    timed_out: bool,
    success: bool,
    pipeline_masked: bool,
) -> &'static str {
    match (timed_out, pipeline_masked, success, exit_code) {
        (true, _, _, _) => "command_timeout",
        (_, true, _, _) => "pipeline_masked",
        (_, _, true, _) => "command_success",
        (_, _, _, None) => "command_unknown",
        _ => "command_failed",
    }
}

fn is_diagnostic_shell_policy(
    family: &str,
    combined: &str,
    exit_code: Option<i32>,
    status_hazard: bool,
) -> bool {
    matches!(
        family,
        "test" | "build" | "lint" | "python-test" | "go-test"
    ) || status_hazard
        || exit_code.is_some_and(|code| code != 0)
        || looks_diagnostic(combined)
}

pub(crate) fn auto_shell_policy(
    command: &str,
    family: &str,
    combined: &str,
    exit_code: Option<i32>,
    search_no_match: bool,
    expected_false_exit: bool,
    status_hazard: bool,
) -> (&'static str, &'static str) {
    if is_repo_inventory_command(command) {
        ("structured", "repo inventory command")
    } else if family == "diff" {
        ("diff-aware", "diff-like output")
    } else if family == "search" && !status_hazard && (exit_code == Some(0) || search_no_match) {
        ("structured", "search output")
    } else if expected_false_exit {
        ("structured", "expected false predicate exit")
    } else if requested_output_lines(command).is_some() {
        ("structured", "explicit head/tail line request")
    } else if is_diagnostic_shell_policy(family, combined, exit_code, status_hazard) {
        ("diagnostic", "failure or diagnostic family")
    } else if matches!(family, "search" | "structured" | "status") {
        ("structured", "structured/status output")
    } else if repeated_line_count(combined) >= 3 || combined.lines().count() > 120 {
        ("dedupe", "repeated or long log")
    } else {
        ("passthrough", "small low-risk output")
    }
}

fn policy_decision(family: String, policy: (&str, &str)) -> PolicyDecision {
    PolicyDecision {
        policy: policy.0.to_string(),
        reason: policy.1.to_string(),
        family,
    }
}

pub fn decide_shell_policy(
    command: &str,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
    mode: Mode,
) -> PolicyDecision {
    let combined = format!("{stdout}\n{stderr}");
    decide_shell_policy_with_combined(command, stdout, stderr, exit_code, mode, &combined)
}

pub(crate) fn decide_shell_policy_with_combined(
    command: &str,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
    mode: Mode,
    combined: &str,
) -> PolicyDecision {
    let family = shell_family_with_combined(command, stdout, combined);
    let requested = mode.effective_policy();
    if requested != Mode::Auto {
        return policy_decision(family, (requested.as_str(), "explicit user mode"));
    }
    let search_no_match = is_search_no_match(command, stdout, stderr, exit_code);
    let expected_false_exit = is_expected_false_exit(command, stdout, stderr, exit_code);
    let status_hazard = failed_segment(command, stdout, stderr, exit_code).is_some()
        || masking_warning(command, stdout, stderr, exit_code).is_some();
    let policy = auto_shell_policy(
        command,
        &family,
        combined,
        exit_code,
        search_no_match,
        expected_false_exit,
        status_hazard,
    );
    policy_decision(family, policy)
}

pub fn classify_command_status(
    command: &str,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
    timed_out: bool,
) -> CommandStatus {
    let search_no_match = is_search_no_match(command, stdout, stderr, exit_code);
    let expected_false_exit = is_expected_false_exit(command, stdout, stderr, exit_code);
    let failed_segment = failed_segment(command, stdout, stderr, exit_code);
    let pipeline_masking_warning = masking_warning(command, stdout, stderr, exit_code);
    let command_success = command_succeeded(
        exit_code,
        search_no_match,
        expected_false_exit,
        timed_out,
        failed_segment.as_deref(),
    );
    let pipeline_masked = !timed_out && exit_code == Some(0) && failed_segment.is_some();
    CommandStatus {
        transport_status: "ok".to_string(),
        command_success,
        exit_code,
        pipeline_rerun_command: pipeline_rerun_command(command, pipeline_masking_warning.as_ref()),
        shell_syntax_summary: shell_syntax_summary_for_status(command, stdout, stderr, exit_code),
        status_label: command_status_label(exit_code, timed_out, command_success, pipeline_masked)
            .to_string(),
        failed_segment,
        pipeline_masking_warning,
    }
}

pub(crate) fn shell_stream_output(exit_code: Option<i32>, stdout: &str, stderr: &str) -> String {
    let mut combined = stdout.to_string();
    if !stderr.is_empty() {
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str("stderr:\n");
        combined.push_str(stderr);
    }
    if combined.is_empty() && exit_code != Some(0) {
        combined = format!(
            "exit_code: {}",
            exit_code.map_or_else(|| "null".to_string(), |value| value.to_string())
        );
    }
    combined
}

pub fn shell_combined_output(
    _command: &str,
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
) -> String {
    shell_stream_output(exit_code, stdout, stderr)
}

pub fn shell_raw_accounting_output(
    command: &str,
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
) -> String {
    let payload = shell_stream_output(exit_code, stdout, stderr);
    shell_raw_accounting_output_with_payload(command, &payload)
}

pub(crate) fn shell_raw_accounting_output_with_payload(command: &str, payload: &str) -> String {
    let mut raw = String::with_capacity(command.len() + payload.len() + 10);
    raw.push_str("command: ");
    raw.push_str(command);
    if !payload.is_empty() {
        raw.push('\n');
        raw.push_str(payload);
    }
    raw
}
