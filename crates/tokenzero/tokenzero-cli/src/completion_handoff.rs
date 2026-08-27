use crate::artifact_contracts::{
    completion_claim_public_residual, completion_evidence_integrity_matrix, completion_req_row,
    completion_requirement_status_summary, completion_source_public_residual, handoff_artifact,
    handoff_artifact_integrity_matrix, handoff_completion_audit_snapshot,
    handoff_verification_evidence_integrity_matrix, handoff_verification_plan_status_summary,
    release_candidate_id, residual_gate_status_summary,
};
use crate::claim_actions::{
    artifact_loop_next_actions, completion_claim_gate_snapshot, completion_residual_gate_matrix,
    handoff_resolve_residual_next_actions, missing_release_os_rows, os_matrix_residual_message,
    os_reach_artifact_purpose, os_release_artifact_purpose, release_os_list_display,
};
use serde_json::json;
use std::path::Path;

type Json = serde_json::Value;

macro_rules! object { ($($tt:tt)*) => { serde_json::json!($($tt)*) }; }
macro_rules! list { ($($tt:tt)*) => { vec![$($tt)*] }; }

fn goal_row(id: &str, status: &str, claim: &str, direct_evidence: &[&str], residual: Json) -> Json {
    object!({"id": id, "status": status, "claim": claim, "direct_evidence": direct_evidence, "residual": residual})
}

macro_rules! goal_rows {
    ($($id:expr, $status:expr, $claim:expr, $evidence:expr, $residual:expr;)*) => {
        list! {$(goal_row($id,$status,$claim,$evidence,$residual)),*}
    };
}

macro_rules! requirement_rows {
    ($($id:expr, $status:expr, $evidence:expr, $residual:expr;)*) => {
        list! {$(completion_req_row($id,$status,$evidence,$residual)),*}
    };
}

macro_rules! handoff_artifacts {
    ($($id:expr, $path:expr, $purpose:expr;)*) => {
        const HANDOFF_ARTIFACTS: &[(&str, &str, &str)] = &[$(($id, $path, $purpose)),*];
    };
}

handoff_artifacts! {
    "completion_audit", "results/current/tokenzero_completion_audit.json", "false-closure audit and requirement map";
    "security_privacy_audit", "results/current/tokenzero_security_privacy_audit.json", "G-009/NFR-003/NFR-004 local security and privacy proof";
    "bench_competitors", "results/current/tokenzero_bench_competitors_shell_heavy.json", "Safe Savings benchmark and unavailable-row adapter matrix";
    "adapter_approval_audit", "results/current/tokenzero_adapter_approval_audit.json", "Non-executing reviewed-command gate for runnable competitor adapters";
    "adapter_approval_file", "results/current/tokenzero_adapter_approval_file.json", "reviewed command-shape approval file; execution and public claims remain gated";
    "source_currency", "results/current/tokenzero_source_currency.json", "private source ledger and public freshness gate";
    "claim_audit", "results/current/tokenzero_claim_audit.json", "public claim gate, same-release-candidate check, and gated action list";
    "os_reach", "results/current/tokenzero_os_reach_audit.json", "";
    "os_release_artifact", "results/current/tokenzero_os_release_artifact.json", "";
    "one_shot", "results/current/tokenzero_one_shot_eval.json", "golden critical trace one-shot adequacy evidence";
    "task_success", "results/current/tokenzero_one_shot_eval.json", "claim-gate task-success proof from one-shot adequacy rows";
    "exact_recovery", "results/current/tokenzero_exact_recovery_audit.json", "normal and degraded exact recovery audit";
    "exact_recovery_shell", "results/current/tokenzero_exact_recovery_shell.json", "VP-006 byte-perfect shell expand checks for emitted local refs";
    "false_success_shell", "results/current/tokenzero_false_success_shell.json", "FR-006 shell status truth audit for nonzero, failed cd, masked pipeline, timeout, and success";
    "reach", "results/current/tokenzero_reach.json", "FR-008/G-007 host reach and installed wrapper trust evidence";
    "mcp_smoke", "results/current/rust_mcp_smoke.json", "VP-003 MCP smoke proof with ok true and no unexpected exits";
    "shell_matrix", "results/current/tokenzero_shell_matrix.json", "VP-004 shell matrix proof for current-host runtime behavior";
    "advanced_adr", "docs/advanced-adr-execution-record.md", "phase decisions and evidence record";
    "competitive_reconciliation", "results/current/tokenzero_competitive_superiority_reconciliation.md", "residual gate reconciliation snapshot and no-gated-action proof";
}

pub(crate) fn completion_audit_report() -> Json {
    let claim_gate_snapshot =
        completion_claim_gate_snapshot(Path::new("results/current/tokenzero_claim_audit.json"));
    let residual_gate_matrix = completion_residual_gate_matrix(&claim_gate_snapshot);
    let (residual_gate_status_counts, blocked_residual_gate_ids, all_residual_gates_resolved) =
        residual_gate_status_summary(&residual_gate_matrix);
    let missing_release_oses =
        missing_release_os_rows(Path::new("results/current/tokenzero_os_reach_audit.json"));
    let os_matrix_residual = os_matrix_residual_message(&missing_release_oses);
    let claim_public_residual = completion_claim_public_residual(&claim_gate_snapshot);
    let source_public_residual = completion_source_public_residual(Path::new(
        "results/current/tokenzero_source_currency.json",
    ));
    let g_goals = goal_rows! {
        "G-001", "passed_private", "Competitive evidence ledger covers named and adjacent repositories", &["results/current/tokenzero_source_currency.json"], json!(&source_public_residual);
        "G-002", "passed_private", "Benchmark harness measures TokenZero and accounts for competitor adapters", &["results/current/tokenzero_bench_competitors_shell_heavy.json", "results/current/tokenzero_adapter_approval_audit.json",], json!("runnable competitor execution remains approval-gated");
        "G-003", "passed", "Exact Recovery Always has refs or degraded diagnostics", &["results/current/tokenzero_exact_recovery_audit.json", "results/current/tokenzero_exact_recovery_shell.json",], Json::Null;
        "G-004", "passed_private", "Adaptive One-Shot Planner avoids hidden second-call dependence on golden critical traces", &["results/current/tokenzero_one_shot_eval.json"], json!("public one-shot claim remains gated");
        "G-005", "blocked_public", "No-Daemon OS Runtime preserves Windows, macOS, and Linux behavior", &["results/current/tokenzero_os_reach_audit.json", "results/current/tokenzero_os_release_artifact.json",], json!(&os_matrix_residual);
        "G-006", "passed", "Stable CLI/MCP diagnostics separate transport from child command success", &["cargo test --workspace", "results/current/tokenzero_false_success_shell.json", "results/current/tokenzero_bench_competitors_shell_heavy.json",], Json::Null;
        "G-007", "passed_private", "Reach and install coverage identifies intercepted and bypassed host surfaces", &["results/current/tokenzero_reach.json", "results/current/tokenzero_os_reach_audit.json", "results/current/tokenzero_os_release_artifact.json",], json!("non-current OS release artifacts remain gated");
        "G-008", "passed_blocked", "Public Claim Gate blocks release-facing savings claims", &["results/current/tokenzero_claim_audit.json"], json!(&claim_public_residual);
        "G-009", "passed", "Security and privacy keep raw payloads local and avoid unapproved external writes", &["results/current/tokenzero_security_privacy_audit.json"], Json::Null;
        "G-010", "passed_private", "Agent Execution Pack supports future implementation without this chat", &["results/current/tokenzero_artifact_handoff.json", "docs/advanced-adr-execution-record.md", "results/current/tokenzero_competitive_superiority_reconciliation.md", "validate_prd_goal.py --min-score 930",], json!("completion remains blocked by explicit residual gates");
    };
    let must_fr = requirement_rows! {
        "FR-001", "passed_private", &["results/current/tokenzero_source_currency.json"], &source_public_residual;
        "FR-002", "passed_private", &["results/current/tokenzero_bench_competitors_shell_heavy.json", "results/current/tokenzero_adapter_approval_audit.json",], "runnable competitor adapters require approval";
        "FR-003", "passed", &["results/current/tokenzero_exact_recovery_audit.json"], "";
        "FR-004", "passed", &["results/current/tokenzero_protected_anchor_audit.json"], "";
        "FR-005", "passed_private", &["results/current/tokenzero_one_shot_eval.json"], "public one-shot claim still gated";
        "FR-006", "passed", &["cargo test --workspace", "results/current/tokenzero_false_success_shell.json",], "";
        "FR-007", "blocked_public", &["results/current/tokenzero_os_reach_audit.json", "results/current/tokenzero_os_release_artifact.json",], &os_matrix_residual;
        "FR-010", "passed_blocked", &["results/current/tokenzero_claim_audit.json"], &claim_public_residual;
    };
    let critical_nfr = requirement_rows! {
        "NFR-001", "passed", &["results/current/tokenzero_exact_recovery_audit.json"], "";
        "NFR-002", "blocked_public", &["results/current/tokenzero_os_reach_audit.json", "results/current/tokenzero_os_release_artifact.json",], &os_matrix_residual;
        "NFR-003", "passed", &["results/current/tokenzero_security_privacy_audit.json"], "";
        "NFR-004", "passed", &["results/current/tokenzero_security_privacy_audit.json"], "";
    };
    let (requirement_status_counts, blocked_requirement_ids, all_requirement_rows_passed) =
        completion_requirement_status_summary(&[&g_goals, &must_fr, &critical_nfr]);
    let evidence_integrity_matrix =
        completion_evidence_integrity_matrix(&g_goals, &must_fr, &critical_nfr);
    let all_direct_file_evidence_present = evidence_integrity_matrix
        .iter()
        .all(|row| row["status"] != "missing");
    let all_direct_artifact_evidence_valid = evidence_integrity_matrix.iter().all(|row| {
        row["evidence_kind"] != "artifact" || row["artifact_valid"].as_bool().unwrap_or(false)
    });
    let os_matrix_residual_gap =
        format!("{os_matrix_residual}; do not claim OS-agnostic release readiness");
    let claim_public_residual_gap =
        format!("{claim_public_residual}; do not publish release-facing savings claims");
    let residual_gaps = list! {os_matrix_residual_gap.as_str(),claim_public_residual_gap.as_str(), "runnable competitor adapter execution requires reviewed commands and explicit approval", "release/publication/global install apply remain gated actions",};
    object!({"schema_version": "tokenzero.completion_audit.v1", "release_candidate_id": release_candidate_id(), "status": "blocked", "completion_status": "blocked", "ok": false, "exit_code": 0, "completion_achieved": false, "final_summary_is_evidence": false, "public_claims_approved": false, "release_publication_allowed": false, "g_goals": g_goals, "must_fr": must_fr, "critical_nfr": critical_nfr, "requirement_status_counts": requirement_status_counts, "blocked_requirement_ids": blocked_requirement_ids, "all_requirement_rows_passed": all_requirement_rows_passed, "residual_gate_status_counts": residual_gate_status_counts, "blocked_residual_gate_ids": blocked_residual_gate_ids, "all_residual_gates_resolved": all_residual_gates_resolved, "evidence_integrity_matrix": evidence_integrity_matrix, "all_direct_file_evidence_present": all_direct_file_evidence_present, "all_direct_artifact_evidence_valid": all_direct_artifact_evidence_valid, "residual_gaps": residual_gaps, "claim_gate_snapshot": claim_gate_snapshot, "residual_gate_matrix": residual_gate_matrix, "artifact_loop_handoff": {"next": "OS matrix expansion, runnable adapter approval if desired, and final release-gate review", "stop_before": ["release", "publication", "remote mutation", "paid services", "global install apply"]}})
}

pub(crate) fn artifact_handoff_report(installed_wrapper_audit: Json) -> Json {
    let completion_audit_snapshot = handoff_completion_audit_snapshot(Path::new(
        "results/current/tokenzero_completion_audit.json",
    ));
    let completion_residual_gate_matrix = completion_audit_snapshot["residual_gate_matrix"].clone();
    let missing_release_oses =
        missing_release_os_rows(Path::new("results/current/tokenzero_os_reach_audit.json"));
    let os_reach_purpose = os_reach_artifact_purpose(&missing_release_oses);
    let os_release_artifact_purpose = os_release_artifact_purpose(&missing_release_oses);
    let artifacts = HANDOFF_ARTIFACTS
        .iter()
        .map(|&(id, path, purpose)| {
            let purpose = match id {
                "os_reach" => &os_reach_purpose,
                "os_release_artifact" => &os_release_artifact_purpose,
                _ => purpose,
            };
            handoff_artifact(id, path, purpose)
        })
        .collect::<Vec<_>>();
    let (artifact_integrity_matrix, all_required_artifacts_present, all_required_artifacts_valid) =
        handoff_artifact_integrity_matrix(&artifacts);
    let verification_plan_matrix = handoff_verification_plan_matrix(&missing_release_oses);
    let (
        verification_plan_status_counts,
        blocked_verification_plan_ids,
        all_verification_plan_rows_passed,
    ) = handoff_verification_plan_status_summary(&verification_plan_matrix);
    let (
        verification_evidence_integrity_matrix,
        all_verification_evidence_artifacts_present,
        all_verification_evidence_artifacts_valid,
    ) = handoff_verification_evidence_integrity_matrix(
        &verification_plan_matrix,
        &artifact_integrity_matrix,
    );
    let next_actions = artifact_loop_next_actions(&completion_residual_gate_matrix);
    let residual_gate_matrix =
        handoff_resolve_residual_next_actions(&completion_residual_gate_matrix, &next_actions);
    let (residual_gate_status_counts, blocked_residual_gate_ids, mut all_residual_gates_resolved) =
        residual_gate_status_summary(&residual_gate_matrix);
    if completion_audit_snapshot["present"] != true {
        all_residual_gates_resolved = false;
    }
    let global_tokenzero_release_verification_trusted =
        installed_wrapper_audit["resolved_is_current_exe"]
            .as_bool()
            .unwrap_or(false);
    let release_verification_binary = installed_wrapper_audit["current_exe"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let anti_drift_reminders = list! {object!({"risk": "Repeated source staleness", "action": "Add or rerun source currency command before public claims", "surface": "repo CLI or docs", "validation": "claim audit"}),object!({"risk": "Repeated agent drift", "action": "Use completion-audit and this handoff packet before final response", "surface": "PRD template or skill", "validation": "completion-audit"}),object!({"risk": "Global wrapper drift", "action": if global_tokenzero_release_verification_trusted {"global tokenzero resolves to the current release-verification executable"} else {"use release_verification_binary or explicitly approved install apply before relying on global tokenzero"}, "surface": "local shell", "validation": "installed_wrapper_audit"}),};
    let stop_before = list! {"release", "publication", "remote mutation", "paid services", "global install apply", "public benchmark claim",};
    object!({"schema_version": "tokenzero.artifact_handoff.v1", "release_candidate_id": release_candidate_id(), "status": "blocked", "ok": false, "exit_code": 0, "completion_achieved": false, "public_claims_approved": false, "release_publication_allowed": false, "installed_wrapper_audit": installed_wrapper_audit, "global_tokenzero_release_verification_trusted": global_tokenzero_release_verification_trusted, "approved_install_required_for_global_update": !global_tokenzero_release_verification_trusted, "release_verification_binary": release_verification_binary, "artifacts": artifacts, "artifact_integrity_matrix": artifact_integrity_matrix, "all_required_artifacts_present": all_required_artifacts_present, "all_required_artifacts_valid": all_required_artifacts_valid, "verification_plan_matrix": verification_plan_matrix, "verification_evidence_integrity_matrix": verification_evidence_integrity_matrix, "all_verification_evidence_artifacts_present": all_verification_evidence_artifacts_present, "all_verification_evidence_artifacts_valid": all_verification_evidence_artifacts_valid, "verification_plan_status_counts": verification_plan_status_counts, "blocked_verification_plan_ids": blocked_verification_plan_ids, "all_verification_plan_rows_passed": all_verification_plan_rows_passed, "requirement_status_counts": completion_audit_snapshot["requirement_status_counts"].clone(), "blocked_requirement_ids": completion_audit_snapshot["blocked_requirement_ids"].clone(), "all_requirement_rows_passed": completion_audit_snapshot["all_requirement_rows_passed"].clone(), "residual_gate_status_counts": residual_gate_status_counts, "blocked_residual_gate_ids": blocked_residual_gate_ids, "all_residual_gates_resolved": all_residual_gates_resolved, "completion_audit_snapshot": completion_audit_snapshot, "residual_gate_matrix": residual_gate_matrix, "next_actions": next_actions, "anti_drift_reminders": anti_drift_reminders, "stop_before": stop_before, "thread_goal": "Implement tokenzero_competitive_superiority_goal.md phase by phase with verification evidence", "handoff_note": "Use current worktree and artifacts as authoritative; do not infer completion from summary prose."})
}

macro_rules! vp_row {
    ($id:literal, $command:literal, $status:expr, $evidence:expr, $condition:literal, $reasons:expr, $stop:expr) => {
        object!({"id": $id, "command": $command, "status": $status, "evidence_artifact_ids": $evidence, "passing_condition": $condition, "blocked_reasons": $reasons, "stop_before": $stop})
    };
}

macro_rules! vp_rows {
    ($($id:expr, $command:expr, $status:expr, $artifacts:expr, $success:expr,
       $blocks:expr, $resolves:expr;)*) => {
        json!([$(vp_row!($id, $command, $status, $artifacts, $success, $blocks, $resolves)),*])
    };
}

fn handoff_verification_plan_matrix(missing_release_oses: &[String]) -> Json {
    let missing = !missing_release_oses.is_empty();
    vp_rows! {
        "VP-001", "python scripts/validate_prd_goal.py PRD_GOAL.md --min-score 930", "passed_local", ["advanced_adr"], "PASS and no check failures", Vec::<&str>::new(), Vec::<&str>::new();
        "VP-002", "cargo test --workspace", "passed_local", ["completion_audit"], "exit code 0", Vec::<&str>::new(), Vec::<&str>::new();
        "VP-003", "target\\windows-verify\\release\\tokenzero.exe mcp-smoke --json", "passed_local", ["mcp_smoke"], "ok true and no unexpected exits", Vec::<&str>::new(), Vec::<&str>::new();
        "VP-004", "target\\windows-verify\\release\\tokenzero.exe shell-matrix --json", if missing {"blocked_public"} else {"passed_local"}, ["shell_matrix", "os_release_artifact", "os_reach"], "each release OS passes before OS claim", os_matrix_verification_blocked_reasons(missing_release_oses), if missing {list! {"OS-agnostic public claim", "publication"}} else {Vec::new()};
        "VP-005", "target\\windows-verify\\release\\tokenzero.exe bench competitors --suite shell-heavy --json", "passed_private", ["bench_competitors", "adapter_approval_audit"], "Safe Savings artifact, honest unavailable rows, public_claims_approved false until evidence", ["public benchmark claim remains gated"], ["public benchmark claim", "publication"];
        "VP-006", "exact expand checks for every emitted local ref in benchmark suite", "passed_local", ["exact_recovery"], "byte-perfect recovery true", Vec::<&str>::new(), Vec::<&str>::new();
        "VP-007", "one-shot golden trace evaluator", "passed_private", ["one_shot", "task_success"], "0% critical miss and less than 2% overall miss", ["public one-shot claim remains gated"], ["publication"];
        "VP-008", "claim audit with source refresh", "blocked_public", ["claim_audit"], "approved only when source and benchmark evidence agree", ["public claims intentionally blocked until release gates pass"], ["release", "publication", "global install apply"];
    }
}

fn os_matrix_verification_blocked_reasons(missing: &[String]) -> Vec<String> {
    if missing.is_empty() {
        return Vec::new();
    }
    list! {format!("{} shell matrix artifacts not run on this host",release_os_list_display(missing))}
}
