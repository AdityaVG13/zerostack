use anyhow::{Context, Result};
use serde_json::json;
use std::fs;
use std::path::Path;

type Json = serde_json::Value;

macro_rules! object { ($($tt:tt)*) => { serde_json::json!($($tt)*) }; }
macro_rules! list { ($($tt:tt)*) => { vec![$($tt)*] }; }

use std::process::Command;

pub(crate) fn load_json_artifact(path: &Path) -> Result<Json> {
    serde_json::from_slice(&fs::read(path).with_context(|| format!("read {}", path.display()))?)
        .with_context(|| format!("parse {}", path.display()))
}

pub(crate) fn release_candidate_id() -> String {
    if let Ok(value) = std::env::var("TOKENZERO_RELEASE_CANDIDATE_ID") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    if let Ok(output) = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
    {
        if output.status.success() {
            let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !sha.is_empty() {
                let dirty = Command::new("git")
                    .args(["diff", "--quiet", "--ignore-submodules"])
                    .status()
                    .is_ok_and(|status| !status.success());
                return if dirty {
                    format!("git-{sha}-dirty")
                } else {
                    format!("git-{sha}")
                };
            }
        }
    }

    "local-unpinned".to_string()
}

pub(crate) fn json_artifact_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn blocked_reason_strings(value: &Json) -> Vec<String> {
    value
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|reason| reason.as_str().map(str::to_string))
        .filter(|reason| !reason.is_empty())
        .collect()
}

fn increment_status_count(counts: &mut serde_json::Map<String, Json>, status: &str) {
    let count = counts.get(status).and_then(Json::as_u64).unwrap_or(0) + 1;
    counts.insert(status.to_string(), json!(count));
}

pub(crate) fn completion_claim_public_residual(claim_gate_snapshot: &Json) -> String {
    if claim_gate_snapshot["public_claims_approved"] == true {
        return "public claim approval is true; final publication still follows release handoff gates"
            .to_string();
    }
    let reasons = blocked_reason_strings(&claim_gate_snapshot["blocked_reasons"]);
    if reasons.is_empty() {
        return "public claim approval intentionally false until claim audit evidence gates pass"
            .to_string();
    }
    format!(
        "public claim approval intentionally false: {}",
        reasons.join("; ")
    )
}

pub(crate) fn completion_source_public_residual(path: &Path) -> String {
    let expected_release_candidate_id = release_candidate_id();
    let Ok(artifact) = load_json_artifact(path) else {
        return "source refresh required for public claims; source currency artifact missing or unreadable"
            .to_string();
    };

    if artifact["schema_version"] != "tokenzero.source_currency.v1" {
        return "source refresh required for public claims; source currency artifact schema invalid"
            .to_string();
    }

    let artifact_release_candidate_id = artifact["release_candidate_id"].as_str();
    let fresh_for_public_claim = artifact["fresh_for_public_claim"] == true;
    if fresh_for_public_claim
        && artifact_release_candidate_id == Some(expected_release_candidate_id.as_str())
    {
        return "source evidence is current; public claims still require benchmark, recovery, task-success, OS, adapter, and release approval gates"
            .to_string();
    }
    if fresh_for_public_claim {
        return "source evidence is fresh but release_candidate_id does not match this release candidate"
            .to_string();
    }

    let reasons = blocked_reason_strings(&artifact["blocked_reasons"]);
    if reasons.is_empty() {
        "source refresh required for public claims".to_string()
    } else {
        format!(
            "source refresh required for public claims: {}",
            reasons.join("; ")
        )
    }
}

fn status_summary<'a>(
    rows: impl Iterator<Item = &'a Json>,
    passed: impl Fn(&str) -> bool,
    blocked: impl Fn(&str) -> bool,
    id: impl Fn(&Json) -> &str,
) -> (Json, Json, bool) {
    let mut counts = serde_json::Map::new();
    let mut blocked_ids = Vec::new();
    let mut all_passed = true;
    for row in rows {
        let status = row["status"].as_str().unwrap_or("unknown");
        increment_status_count(&mut counts, status);
        all_passed &= passed(status);
        if blocked(status) {
            blocked_ids.push(id(row).to_string());
        }
    }
    (json!(counts), json!(blocked_ids), all_passed)
}

pub(crate) fn completion_requirement_status_summary(sections: &[&Vec<Json>]) -> (Json, Json, bool) {
    status_summary(
        sections.iter().flat_map(|section| section.iter()),
        |status| matches!(status, "passed" | "passed_private"),
        |status| !matches!(status, "passed" | "passed_private"),
        |row| row["id"].as_str().unwrap_or("unknown"),
    )
}

pub(crate) fn residual_gate_status_summary(residual_gate_matrix: &Json) -> (Json, Json, bool) {
    let Some(rows) = residual_gate_matrix.as_array() else {
        return (object!({}), json!([]), true);
    };
    let (counts, blocked, all_resolved) = status_summary(
        rows.iter(),
        |status| status == "resolved",
        |status| status == "blocked",
        |row| {
            row["gate_id"]
                .as_str()
                .or_else(|| row["id"].as_str())
                .unwrap_or("unknown")
        },
    );
    let mut blocked = blocked.as_array().cloned().unwrap_or_default();
    blocked.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
    (counts, json!(blocked), all_resolved)
}

pub(crate) fn completion_req_row(
    id: &str,
    status: &str,
    direct_evidence: &[&str],
    residual: &str,
) -> Json {
    object!({"id": id, "status": status, "direct_evidence": direct_evidence, "residual": if residual.is_empty() {Json::Null} else {json!(residual)}})
}

pub(crate) fn completion_evidence_integrity_matrix(
    g_goals: &[Json],
    must_fr: &[Json],
    critical_nfr: &[Json],
) -> Vec<Json> {
    let mut rows = Vec::new();
    for (section, section_rows) in [
        ("g_goals", g_goals),
        ("must_fr", must_fr),
        ("critical_nfr", critical_nfr),
    ] {
        for row in section_rows {
            let requirement_id = row["id"].as_str().unwrap_or("unknown");
            let requirement_status = row["status"].clone();
            let requirement_residual = row["residual"].clone();
            if let Some(evidence_items) = row["direct_evidence"].as_array() {
                for evidence in evidence_items.iter().filter_map(|item| item.as_str()) {
                    let evidence_kind = completion_evidence_kind(evidence);
                    let _integrity =
                        completion_evidence_artifact_integrity(evidence, evidence_kind);
                    let mut integrity =
                        completion_evidence_artifact_integrity(evidence, evidence_kind);
                    let fields = integrity
                        .as_object_mut()
                        .expect("integrity row is an object");
                    for (key, value) in [
                        ("section", object!(section)),
                        ("requirement_id", object!(requirement_id)),
                        ("evidence", object!(evidence)),
                        ("evidence_kind", object!(evidence_kind)),
                        ("requirement_status", requirement_status.clone()),
                        ("requirement_residual", requirement_residual.clone()),
                    ] {
                        fields.insert(key.to_string(), value);
                    }
                    rows.push(integrity);
                }
            }
        }
    }
    rows
}

struct ArtifactContract {
    evidence_path: Option<&'static str>,
    handoff_ids: &'static [&'static str],
    schema: &'static str,
    release_candidate: bool,
}

macro_rules! artifact_contracts {
    (files { $($file:ident, [$($id:ident),*], $schema:ident, $release:literal;)* }
     virtual { $([$($virtual_id:ident),*], $virtual_schema:ident, $virtual_release:literal;)* }) => {
        const ARTIFACT_CONTRACTS: &[ArtifactContract] = &[
            $(ArtifactContract {
                evidence_path: Some(concat!("results/current/", stringify!($file), ".json")),
                handoff_ids: &[$(stringify!($id)),*],
                schema: concat!("tokenzero.", stringify!($schema), ".v1"),
                release_candidate: $release,
            },)*
            $(ArtifactContract {
                evidence_path: None,
                handoff_ids: &[$(stringify!($virtual_id)),*],
                schema: concat!("tokenzero.", stringify!($virtual_schema), ".v1"),
                release_candidate: $virtual_release,
            },)*
        ];
    };
}

artifact_contracts! {
    files {
        rust_mcp_smoke, [mcp_smoke], rust_mcp_churn, false;
        tokenzero_adapter_approval_audit, [adapter_approval_audit], adapter_approval_audit, true;
        tokenzero_artifact_handoff, [], artifact_handoff, true;
        tokenzero_bench_competitors_shell_heavy, [bench_competitors], bench, true;
        tokenzero_claim_audit, [claim_audit], claim_audit, true;
        tokenzero_exact_recovery_audit, [exact_recovery], exact_recovery_audit, true;
        tokenzero_exact_recovery_shell, [exact_recovery_shell], exact_recovery_shell, false;
        tokenzero_false_success_shell, [false_success_shell], false_success_shell, false;
        tokenzero_one_shot_eval, [one_shot, task_success], one_shot_eval, true;
        tokenzero_os_reach_audit, [os_reach], os_reach_audit, true;
        tokenzero_os_release_artifact, [os_release_artifact], os_release_artifact, true;
        tokenzero_protected_anchor_audit, [], protected_anchor_audit, false;
        tokenzero_reach, [reach], reach, false;
        tokenzero_security_privacy_audit, [security_privacy_audit], security_privacy_audit, false;
        tokenzero_shell_matrix, [shell_matrix], shell_matrix, false;
        tokenzero_source_currency, [source_currency], source_currency, true;
    }
    virtual {
        [completion_audit], completion_audit, true;
        [adapter_approval_file], adapter_approval_file, false;
    }
}

#[derive(Default)]
struct ArtifactInspection {
    present: bool,
    readable: bool,
    schema_version: Json,
    schema_matches: Json,
    release_candidate_id: Json,
    release_candidate_matches: Json,
    content_markers_present: Json,
    valid: bool,
    reasons: Vec<String>,
}

fn inspect_artifact(
    path: &Path,
    expected_schema: Option<&str>,
    expected_release_candidate: Option<&str>,
    markers: &[&str],
    capture_release_candidate: bool,
    parse_json_extension: bool,
    contextual_errors: bool,
) -> ArtifactInspection {
    let mut result = ArtifactInspection {
        present: path.exists(),
        schema_matches: expected_schema.map_or(Json::Null, |_| json!(false)),
        content_markers_present: markers
            .is_empty()
            .then_some(Json::Null)
            .unwrap_or(json!(false)),
        ..ArtifactInspection::default()
    };
    if !result.present {
        result.reasons.push("artifact missing".to_string());
        return result;
    }
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => {
            result.reasons.push(if contextual_errors {
                format!("read {}", path.display())
            } else {
                "artifact unreadable".to_string()
            });
            return result;
        }
    };
    result.readable = true;
    if !markers.is_empty() {
        let text = String::from_utf8_lossy(&bytes);
        for marker in markers.iter().filter(|marker| !text.contains(**marker)) {
            result
                .reasons
                .push(format!("content marker missing: {marker}"));
        }
        result.content_markers_present = json!(result.reasons.is_empty());
    }
    let parse_json = expected_schema.is_some()
        || expected_release_candidate.is_some()
        || parse_json_extension
            && path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("json"));
    if !parse_json {
        result.valid = result.reasons.is_empty();
        return result;
    }
    let artifact = match serde_json::from_slice::<Json>(&bytes) {
        Ok(artifact) => artifact,
        Err(_) => {
            result.reasons.push(if contextual_errors {
                format!("parse {}", path.display())
            } else {
                "artifact JSON unreadable".to_string()
            });
            if expected_schema.is_some() {
                result.schema_matches = json!(false);
            }
            if expected_release_candidate.is_some() {
                result.release_candidate_matches = json!(false);
            }
            result.valid = result.reasons.is_empty();
            return result;
        }
    };
    result.schema_version = artifact["schema_version"].clone();
    if result.schema_version.is_null() {
        result.reasons.push("schema_version missing".to_string());
    } else if let Some(expected) = expected_schema {
        let matches = result.schema_version.as_str() == Some(expected);
        result.schema_matches = json!(matches);
        if !matches {
            result.reasons.push("schema_version mismatch".to_string());
        }
    } else {
        result.schema_matches = Json::Null;
    }
    if capture_release_candidate {
        result.release_candidate_id = artifact["release_candidate_id"].clone();
    }
    if let Some(expected) = expected_release_candidate {
        if result.release_candidate_id.is_null() {
            result.release_candidate_matches = json!(false);
            result
                .reasons
                .push("release_candidate_id missing".to_string());
        } else {
            let matches = result.release_candidate_id.as_str() == Some(expected);
            result.release_candidate_matches = json!(matches);
            if !matches {
                result
                    .reasons
                    .push("release_candidate_id mismatch".to_string());
            }
        }
    }
    result.valid = result.reasons.is_empty();
    result
}

fn completion_evidence_artifact_integrity(evidence: &str, evidence_kind: &str) -> Json {
    if evidence_kind != "artifact" {
        return object!({"present": null, "status": "command_evidence", "schema_version": null,
            "expected_schema_version": null, "schema_matches": null, "release_candidate_id": null,
            "expected_release_candidate_id": null, "release_candidate_matches": null,
            "expected_content_markers": null, "content_markers_present": null,
            "artifact_valid": null, "reasons": []});
    }
    let (expected_schema_version, expected_release_candidate_id) =
        contract_expectations(evidence_contract(evidence));
    let expected_content_markers = completion_expected_evidence_content_markers(evidence);
    let check = inspect_artifact(
        Path::new(evidence),
        expected_schema_version,
        expected_release_candidate_id.as_deref(),
        &expected_content_markers,
        true,
        false,
        true,
    );
    object!({"present": check.present,
        "status": match (check.present, check.valid) { (false, _) => "missing", (_, true) => "present", _ => "invalid" },
        "schema_version": check.schema_version, "expected_schema_version": expected_schema_version,
        "schema_matches": check.schema_matches, "release_candidate_id": check.release_candidate_id,
        "expected_release_candidate_id": expected_release_candidate_id,
        "release_candidate_matches": check.release_candidate_matches,
        "expected_content_markers": completion_content_marker_value(&expected_content_markers),
        "content_markers_present": check.content_markers_present, "artifact_valid": check.valid,
        "reasons": check.reasons})
}

fn completion_content_marker_value(markers: &[&'static str]) -> Json {
    if markers.is_empty() {
        Json::Null
    } else {
        json!(markers)
    }
}

fn evidence_contract(evidence: &str) -> Option<&'static ArtifactContract> {
    ARTIFACT_CONTRACTS
        .iter()
        .find(|contract| contract.evidence_path == Some(evidence))
}

fn contract_expectations(
    contract: Option<&ArtifactContract>,
) -> (Option<&'static str>, Option<String>) {
    (
        contract.map(|value| value.schema),
        contract
            .filter(|value| value.release_candidate)
            .map(|_| release_candidate_id()),
    )
}

fn completion_expected_evidence_content_markers(evidence: &str) -> Vec<&'static str> {
    match evidence {
        "docs/advanced-adr-execution-record.md" => {
            list! {"## ADR-", "Failure-first evidence:", "Residual gates:", "validate_prd_goal.py", "cargo test --workspace",}
        }
        "results/current/tokenzero_competitive_superiority_reconciliation.md" => {
            list! {"Snapshot", "no gated action was performed"}
        }
        _ => Vec::new(),
    }
}

fn completion_evidence_kind(evidence: &str) -> &'static str {
    if evidence.starts_with("results/")
        || evidence.starts_with("docs/")
        || evidence.starts_with("crates/")
    {
        "artifact"
    } else {
        "command"
    }
}

pub(crate) fn handoff_verification_plan_status_summary(
    verification_plan_matrix: &Json,
) -> (Json, Json, bool) {
    let Some(rows) = verification_plan_matrix.as_array() else {
        return (object!({}), json!([]), false);
    };
    status_summary(
        rows.iter(),
        |status| status.starts_with("passed"),
        |status| !status.starts_with("passed"),
        |row| row["id"].as_str().unwrap_or("unknown"),
    )
}

pub(crate) fn handoff_verification_evidence_integrity_matrix(
    verification_plan_matrix: &Json,
    artifact_integrity_matrix: &Json,
) -> (Json, bool, bool) {
    let mut rows = Vec::new();
    let mut all_present = true;
    let mut all_valid = true;
    let artifact_rows = artifact_integrity_matrix
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    let Some(vp_rows) = verification_plan_matrix.as_array() else {
        return (json!([]), false, false);
    };

    for vp_row in vp_rows {
        let verification_id = vp_row["id"].as_str().unwrap_or("unknown");
        let Some(evidence_ids) = vp_row["evidence_artifact_ids"].as_array() else {
            continue;
        };
        for artifact_id in evidence_ids.iter().filter_map(|item| item.as_str()) {
            let artifact_row = artifact_rows
                .iter()
                .find(|row| row["id"].as_str() == Some(artifact_id));
            let (artifact_path, present, valid, status, reasons) =
                if let Some(artifact_row) = artifact_row {
                    let present = artifact_row["present"].as_bool().unwrap_or(false);
                    let valid = artifact_row["valid"].as_bool().unwrap_or(false);
                    let status = if present && valid {
                        "linked_valid"
                    } else if !present {
                        "missing"
                    } else {
                        "invalid"
                    };
                    (
                        artifact_row["path"].clone(),
                        present,
                        valid,
                        status,
                        artifact_row["reasons"].clone(),
                    )
                } else {
                    (
                        Json::Null,
                        false,
                        false,
                        "unlinked",
                        json!(["artifact id not listed in handoff artifacts"]),
                    )
                };

            if !present {
                all_present = false;
            }
            if !valid {
                all_valid = false;
            }

            rows.push(object!({"verification_id": verification_id, "artifact_id": artifact_id, "artifact_path": artifact_path, "present": present, "valid": valid, "status": status, "reasons": reasons}));
        }
    }

    (json!(rows), all_present, all_valid)
}

pub(crate) fn handoff_artifact_integrity_matrix(artifacts: &[Json]) -> (Json, bool, bool) {
    let mut rows = Vec::new();
    let mut all_required_present = true;
    let mut all_required_valid = true;
    for artifact in artifacts {
        let id = artifact["id"].as_str().unwrap_or_default();
        let path_text = artifact["path"].as_str().unwrap_or_default();
        let required = artifact["required_for_final_reconciliation"]
            .as_bool()
            .unwrap_or(false);
        let (expected_schema_version, expected_release_candidate_id) =
            contract_expectations(handoff_contract(id));
        let expected_content_markers = completion_expected_evidence_content_markers(path_text);
        let check = inspect_artifact(
            Path::new(path_text),
            expected_schema_version,
            expected_release_candidate_id.as_deref(),
            &expected_content_markers,
            expected_release_candidate_id.is_some(),
            true,
            false,
        );
        if required {
            all_required_present &= check.present;
            all_required_valid &= check.valid;
        }
        rows.push(object!({"id": id, "path": path_text, "required_for_final_reconciliation": required, "present": check.present, "readable": check.readable, "schema_version": check.schema_version, "expected_schema_version": expected_schema_version, "schema_matches": check.schema_matches, "release_candidate_id": check.release_candidate_id, "expected_release_candidate_id": expected_release_candidate_id, "release_candidate_matches": check.release_candidate_matches, "expected_content_markers": completion_content_marker_value(&expected_content_markers), "content_markers_present": check.content_markers_present, "valid": check.valid, "reasons": check.reasons}));
    }
    (json!(rows), all_required_present, all_required_valid)
}

fn handoff_contract(id: &str) -> Option<&'static ArtifactContract> {
    ARTIFACT_CONTRACTS
        .iter()
        .find(|contract| contract.handoff_ids.contains(&id))
}

pub(crate) fn handoff_completion_audit_snapshot(path: &Path) -> Json {
    let artifact = match load_json_artifact(path) {
        Ok(artifact) => artifact,
        Err(error) => {
            return object!({"present": false, "artifact_path": path.display().to_string(), "completion_achieved": false, "public_claims_approved": false, "release_publication_allowed": false, "all_direct_artifact_evidence_valid": false, "residual_gate_status_counts": {}, "blocked_residual_gate_ids": [], "all_residual_gates_resolved": false, "residual_gate_matrix": [], "error": error.to_string()});
        }
    };

    object!({"present": true, "artifact_path": path.display().to_string(), "schema_version": artifact["schema_version"], "release_candidate_id": artifact["release_candidate_id"], "completion_achieved": artifact["completion_achieved"], "completion_status": artifact["completion_status"], "public_claims_approved": artifact["public_claims_approved"], "release_publication_allowed": artifact["release_publication_allowed"], "all_direct_file_evidence_present": artifact["all_direct_file_evidence_present"], "all_direct_artifact_evidence_valid": artifact["all_direct_artifact_evidence_valid"], "all_requirement_rows_passed": artifact["all_requirement_rows_passed"], "blocked_requirement_ids": artifact["blocked_requirement_ids"].as_array().cloned().unwrap_or_default(), "requirement_status_counts": artifact["requirement_status_counts"].as_object().cloned().unwrap_or_default(), "residual_gate_status_counts": artifact["residual_gate_status_counts"].as_object().cloned().unwrap_or_default(), "blocked_residual_gate_ids": artifact["blocked_residual_gate_ids"].as_array().cloned().unwrap_or_default(), "all_residual_gates_resolved": artifact["all_residual_gates_resolved"], "evidence_integrity_matrix": artifact["evidence_integrity_matrix"].as_array().cloned().unwrap_or_default(), "claim_gate_snapshot": artifact["claim_gate_snapshot"].as_object().cloned().unwrap_or_default(), "residual_gate_matrix": artifact["residual_gate_matrix"].as_array().cloned().unwrap_or_default()})
}

pub(crate) fn handoff_artifact(id: &str, path: &str, purpose: &str) -> Json {
    object!({"id": id, "path": path, "purpose": purpose, "required_for_final_reconciliation": true})
}

