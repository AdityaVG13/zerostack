use anyhow::Result;
use serde_json::Value;
use std::path::{Path, PathBuf};

macro_rules! object { ($($tt:tt)*) => { serde_json::json!($($tt)*) }; }
macro_rules! list { ($($tt:tt)*) => { vec![$($tt)*] }; }
macro_rules! array { ($($tt:tt)*) => { [$($tt)*] }; }

macro_rules! failures {
    ($($failed:expr => $reason:expr;)*) => { [$(($failed, $reason)),*].into_iter().filter_map(|(failed, reason)| failed.then(|| reason.into())).collect() };
}
macro_rules! artifact_gate {
    ($name:ident, $id:literal, $missing:literal, |$artifact:ident| $body:block) => {
        fn $name(path: Option<&PathBuf>) -> Result<Value> {
            loaded_gate(path, $id, $missing, |$artifact| $body)
        }
    };
}
macro_rules! requirements {
    ($reasons:expr; $($failed:expr => $reason:expr),* $(,)?) => { $(
        if $failed { push_unique_reason($reasons, $reason); }
    )* };
}

use crate::artifact_contracts::{json_artifact_path, load_json_artifact, release_candidate_id};
use crate::competitor_adapters::REQUIRED_COMPETITOR_ADAPTERS;
use crate::source_currency;
use crate::write_artifacts;

pub(crate) struct ClaimEvidenceInputs {
    pub(crate) source_artifact: Option<PathBuf>,
    pub(crate) benchmark_artifact: Option<PathBuf>,
    pub(crate) adapter_approval_artifact: Option<PathBuf>,
    pub(crate) recovery_artifact: Option<PathBuf>,
    pub(crate) task_success_artifact: Option<PathBuf>,
    pub(crate) os_artifact: Option<PathBuf>,
}

impl ClaimEvidenceInputs {
    fn with_current_defaults(mut self) -> Self {
        macro_rules! defaults {
            ($($field:ident => $name:literal),* $(,)?) => { $(if self.$field.is_none() {
                self.$field = current_claim_artifact_path($name);
            })* };
        }
        defaults! {
            source_artifact => "tokenzero_source_currency.json",
            benchmark_artifact => "tokenzero_bench_competitors_shell_heavy.json",
            adapter_approval_artifact => "tokenzero_adapter_approval_audit.json",
            recovery_artifact => "tokenzero_exact_recovery_audit.json",
            task_success_artifact => "tokenzero_one_shot_eval.json",
            os_artifact => "tokenzero_os_reach_audit.json",
        }
        self
    }
}

fn current_claim_artifact_path(filename: &str) -> Option<PathBuf> {
    let path = PathBuf::from("results").join("current").join(filename);
    path.is_file().then_some(path)
}

pub(crate) fn run_claim_audit(
    output_json: PathBuf,
    output_md: Option<PathBuf>,
    release_approval: bool,
    inputs: ClaimEvidenceInputs,
) -> Result<Value> {
    let inputs = inputs.with_current_defaults();
    let source = inputs.source_artifact.as_ref().map_or_else(
        || {
            Ok(source_currency::source_currency_report(
                &release_candidate_id(),
            ))
        },
        |path| load_json_artifact(path),
    )?;
    let source_gate = evaluate_source_claim_gate(&source, inputs.source_artifact.as_ref());
    let benchmark_gate = evaluate_benchmark_claim_gate(inputs.benchmark_artifact.as_ref())?;
    let adapter_gate =
        evaluate_adapter_approval_claim_gate(inputs.adapter_approval_artifact.as_ref())?;
    let recovery_gate = evaluate_recovery_claim_gate(inputs.recovery_artifact.as_ref())?;
    let task_gate = evaluate_task_success_claim_gate(inputs.task_success_artifact.as_ref())?;
    let os_gate = evaluate_os_claim_gate(inputs.os_artifact.as_ref())?;
    let candidate_gate = evaluate_release_candidate_claim_gate(&inputs)?;
    let release_gate = claim_gate(
        "release_approval",
        None,
        (!release_approval)
            .then(|| "release approval not granted".to_string())
            .into_iter()
            .collect(),
        object!({"release_approval": release_approval}),
    );
    let safe = array! {&source_gate,&benchmark_gate,&adapter_gate,&recovery_gate,&task_gate,&os_gate,&candidate_gate,&release_gate,}
    .iter()
    .all(|gate| gate["pass"] == true);
    let os_safe = safe && os_gate["pass"] == true;
    let claims = list! {object!({"claim_id": "tokenzero_safe_savings", "claim": "TokenZero Safe Savings is release-ready", "source_current": source_gate["pass"], "benchmark_artifact_current": benchmark_gate["pass"], "adapter_execution_approved": adapter_gate["pass"], "byte_perfect_recovery": recovery_gate["pass"], "task_success": task_gate["pass"], "release_approval": release_approval, "approved": safe, "public_safe_to_publish": safe, "reason": "release-facing savings claims remain gated until fresh sources, benchmark artifacts, recovery evidence, task success, and explicit approval all agree"}),object!({"claim_id": "os_agnostic", "claim": "TokenZero is proven across Windows, macOS, and Linux", "source_current": source_gate["pass"], "benchmark_artifact_current": benchmark_gate["pass"], "byte_perfect_recovery": recovery_gate["pass"], "task_success": os_gate["pass"], "release_approval": release_approval, "approved": os_safe, "public_safe_to_publish": os_safe, "reason": "all three OS artifact rows must be present before the public OS claim is approved"}),};
    let evidence_gates = list! {source_gate,benchmark_gate,adapter_gate,recovery_gate,task_gate,os_gate,candidate_gate,release_gate,};
    let (gate_passes, gate_reasons, gate_paths, candidate_ids, candidate_artifacts) =
        claim_gate_summary(&evidence_gates);
    let mut blocked_reasons = Vec::new();
    for reason in evidence_gates
        .iter()
        .flat_map(|gate| gate["reasons"].as_array().into_iter().flatten())
    {
        if let Some(reason) = reason.as_str().filter(|reason| !reason.is_empty()) {
            push_unique_reason(&mut blocked_reasons, reason);
        }
    }
    let report = object!({"schema_version": "tokenzero.claim_audit.v1", "release_candidate_id": release_candidate_id(), "status": if safe {"ok"} else {"blocked"}, "transport_status": "ok", "claim_status": if safe {"approved"} else {"blocked"}, "ok": safe, "public_claims_approved": safe, "release_publication_allowed": safe, "blocked_reasons": blocked_reasons, "evidence_gates": evidence_gates, "gate_passes": gate_passes, "gate_reasons": gate_reasons, "gate_artifact_paths": gate_paths, "release_candidate_ids": candidate_ids, "release_candidate_artifacts": candidate_artifacts, "claims": claims, "source_currency": source, "source_ledger": source["rows"], "gated_actions": ["release", "publication", "remote mutation", "paid services", "global install apply"], "non_claims_doc": "docs/racc.md"});
    write_artifacts(&output_json, output_md.as_deref(), &report, "Claim audit")?;
    Ok(report)
}

type GateSummary = (Value, Value, Value, Vec<Value>, Vec<Value>);

fn claim_gate_summary(gates: &[Value]) -> GateSummary {
    let (mut passes, mut reasons, mut paths): (
        serde_json::Map<String, Value>,
        serde_json::Map<String, Value>,
        serde_json::Map<String, Value>,
    ) = Default::default();
    let (mut candidate_ids, mut candidate_artifacts) = Default::default();
    for gate in gates {
        let Some(id) = gate["id"].as_str() else {
            continue;
        };
        passes.insert(id.into(), gate["pass"].clone());
        reasons.insert(
            id.into(),
            object!(gate["reasons"].as_array().cloned().unwrap_or_default()),
        );
        paths.insert(id.into(), gate["artifact_path"].clone());
        if id == "release_candidate" {
            candidate_ids = gate["details"]["release_candidate_ids"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            candidate_artifacts = gate["details"]["artifacts"]
                .as_array()
                .cloned()
                .unwrap_or_default();
        }
    }
    (
        object!(passes),
        object!(reasons),
        object!(paths),
        candidate_ids,
        candidate_artifacts,
    )
}

fn claim_gate(id: &str, path: Option<&Path>, reasons: Vec<String>, details: Value) -> Value {
    object!({"id": id, "pass": reasons.is_empty(), "artifact_path": path.map(json_artifact_path), "reasons": reasons, "details": details})
}

fn evaluate_release_candidate_claim_gate(inputs: &ClaimEvidenceInputs) -> Result<Value> {
    let paths = array! {("source_artifact",inputs.source_artifact.as_ref()),("benchmark_artifact",inputs.benchmark_artifact.as_ref()),("adapter_approval_artifact",inputs.adapter_approval_artifact.as_ref(),),("recovery_artifact",inputs.recovery_artifact.as_ref()),("task_success_artifact",inputs.task_success_artifact.as_ref(),),("os_artifact",inputs.os_artifact.as_ref()),};
    let (mut reasons, mut ids, mut rows, mut attached) =
        (Vec::new(), Vec::<String>::new(), Vec::new(), 0);
    for (artifact_id, path) in paths {
        let Some(path) = path else {
            push_unique_reason(&mut reasons, "same-release-candidate evidence incomplete");
            rows.push(object!({"artifact_id": artifact_id, "artifact_path": null, "schema_version": null, "release_candidate_id": null}));
            continue;
        };
        attached += 1;
        let artifact = load_json_artifact(path)?;
        let id = artifact["release_candidate_id"]
            .as_str()
            .unwrap_or_default()
            .trim();
        if id.is_empty() {
            push_unique_reason(
                &mut reasons,
                "evidence artifact missing release_candidate_id",
            );
        } else if !ids.iter().any(|existing| existing == id) {
            ids.push(id.to_string());
        }
        rows.push(
            object!({"artifact_id": artifact_id, "artifact_path": json_artifact_path(path), "schema_version": artifact["schema_version"], "release_candidate_id": (!id.is_empty()).then_some(id)}),
        );
    }
    if ids.len() > 1 {
        push_unique_reason(
            &mut reasons,
            "evidence artifacts are not from the same release candidate",
        );
    }
    Ok(claim_gate(
        "release_candidate",
        None,
        reasons,
        object!({"artifact_count": rows.len(), "attached_artifact_count": attached, "release_candidate_ids": ids, "artifacts": rows}),
    ))
}

fn evaluate_source_claim_gate(source: &Value, path: Option<&PathBuf>) -> Value {
    let rows = source["rows"].as_array();
    let mut reasons = failures! {
        source["schema_version"] != "tokenzero.source_currency.v1" => "source artifact schema mismatch";
        source["fresh_for_public_claim"] != true => "source ledger requires same-release-candidate refresh";
        source["fresh_for_public_claim"] != true => "source refresh not same-release-candidate";
        rows.is_none_or(|rows| rows.len() < REQUIRED_COMPETITOR_ADAPTERS.len()) => "source ledger missing required competitor rows";
    };
    let (mut pinned, mut missing, mut unpinned) = (0, 0, Vec::new());
    if let Some(rows) = rows {
        if REQUIRED_COMPETITOR_ADAPTERS
            .iter()
            .any(|tool| !rows.iter().any(|row| row["tool"] == *tool))
        {
            push_unique_reason(
                &mut reasons,
                "source ledger missing required competitor rows",
            );
        }
        for row in rows {
            if row["source_date"].as_str().is_none_or(str::is_empty) {
                push_unique_reason(&mut reasons, "source ledger row missing source date");
            }
            let commit = row["source_commit"].as_str().unwrap_or_default().trim();
            if commit.is_empty() {
                missing += 1;
                push_unique_reason(&mut reasons, "source ledger row missing source commit");
            } else if source_currency::source_commit_is_release_candidate_pin(commit) {
                pinned += 1;
            } else {
                push_unique_reason(
                    &mut reasons,
                    "source ledger row source commit is not a release-candidate pin",
                );
                unpinned.push(
                    object!({"tool": row["tool"], "url": row["url"], "source_commit": commit}),
                );
            }
            requirements! { &mut reasons;
                row["url"].as_str().is_none_or(|url| !url.starts_with("https://github.com/"))
                    => "source ledger row missing primary URL",
                row["claimed_scope"].as_str().is_none_or(str::is_empty)
                    => "source ledger row missing claimed scope",
                row["issue_pr_themes"].as_array().is_none_or(Vec::is_empty)
                    => "source ledger row missing issue/PR themes",
                row["strengths"].as_array().is_none_or(Vec::is_empty)
                    => "source ledger row missing strengths",
                row["gaps"].as_array().is_none_or(Vec::is_empty)
                    => "source ledger row missing gaps",
            }
        }
    }
    claim_gate(
        "source_currency",
        path.map(PathBuf::as_path),
        reasons,
        object!({"schema_version": source["schema_version"], "release_candidate_id": source["release_candidate_id"], "fresh_for_public_claim": source["fresh_for_public_claim"], "row_count": rows.map_or(0,Vec::len), "source_commit_pin_status": {"pinned": pinned, "missing": missing, "unpinned": unpinned.len()}, "unpinned_source_rows": unpinned}),
    )
}

fn push_unique_reason(reasons: &mut Vec<String>, reason: &str) {
    if !reasons.iter().any(|existing| existing == reason) {
        reasons.push(reason.into());
    }
}

fn loaded_gate<F>(path: Option<&PathBuf>, id: &str, missing: &str, evaluate: F) -> Result<Value>
where
    F: FnOnce(&Value) -> (Vec<String>, Value),
{
    let Some(path) = path else {
        return Ok(claim_gate(
            id,
            None,
            list! {missing.into()},
            object!({"supplied": false}),
        ));
    };
    let (reasons, details) = evaluate(&load_json_artifact(path)?);
    Ok(claim_gate(id, Some(path), reasons, details))
}

artifact_gate! { evaluate_benchmark_claim_gate, "benchmark_artifact", "benchmark artifact not approved for publication", |artifact| {
let matrix = &artifact["adapter_matrix"];
let rows = artifact["rows"].as_array();
let mut reasons = failures! {
        artifact["schema_version"] != "tokenzero.bench.v1" => "benchmark artifact schema mismatch";
        artifact["ok"] != true => "benchmark artifact did not pass";
        artifact["public_claims_approved"] != true => "benchmark artifact not approved for publication";
        matrix["all_required_adapters_accounted"] != true => "benchmark adapter matrix does not account for all required competitors";
        matrix["blind_install_attempted"] == true => "benchmark attempted blind install";
        rows.is_none_or(Vec::is_empty) => "benchmark rows missing";
    };
let mut status = benchmark_public_claim_status(rows);
for (field, reason) in [
    ("competitor_unavailable_rows", "benchmark competitor rows must be runnable for public claims"),
    ("competitor_non_runnable_rows", "benchmark competitor rows include non-runnable public claim evidence"),
] {
    if status[field].as_u64().unwrap_or(0) > 0 { push_unique_reason(&mut reasons, reason); }
}
if let Some(rows) = rows { for row in rows { validate_benchmark_row(row, &mut reasons); } }
status["gate_reasons"] = object!(reasons.clone());
(reasons, object!({"schema_version": artifact["schema_version"], "public_claims_approved": artifact["public_claims_approved"], "adapter_matrix": matrix, "public_claim_status": status}))
} }

fn benchmark_public_claim_status(rows: Option<&Vec<Value>>) -> Value {
    let (mut own, mut run, mut unavailable, mut other) = (0, 0, Vec::new(), Vec::new());
    if let Some(rows) = rows {
        for row in rows {
            let (tool, status) = (
                row["tool"].as_str().unwrap_or_default(),
                row["availability_status"].as_str().unwrap_or_default(),
            );
            match (tool == "tokenzero", status) {
                (true, "run") => own += 1,
                (true, _) => {}
                (false, "run") => run += 1,
                (false, status) => {
                    let summary = object!({"tool": row["tool"], "scenario_id": row["scenario_id"], "availability_status": row["availability_status"], "availability_reason": row["availability_reason"]});
                    if status == "unavailable" {
                        unavailable.push(summary);
                    } else {
                        other.push(summary);
                    }
                }
            }
        }
    }
    object!({"tokenzero_run_rows": own, "competitor_run_rows": run, "competitor_unavailable_rows": unavailable.len(), "competitor_non_runnable_rows": other.len(), "unavailable_competitors": unavailable, "non_runnable_competitors": other})
}

#[derive(Clone, Copy)]
enum JsonKind {
    Present,
    Number,
    Boolean,
}

fn validate_benchmark_row(row: &Value, reasons: &mut Vec<String>) {
    for &(field, kind) in &[
        ("tool", JsonKind::Present),
        ("suite", JsonKind::Present),
        ("availability_status", JsonKind::Present),
        ("fairness_notes", JsonKind::Present),
        ("raw_tokens", JsonKind::Number),
        ("visible_tokens", JsonKind::Number),
        ("recovery_tokens", JsonKind::Number),
        ("safe_savings", JsonKind::Number),
        ("harm_rate", JsonKind::Number),
        ("task_success", JsonKind::Boolean),
    ] {
        let value = &row[field];
        let missing = match kind {
            JsonKind::Present => value.is_null(),
            JsonKind::Number => !value.is_number(),
            JsonKind::Boolean => !value.is_boolean(),
        };
        if missing {
            push_unique_reason(
                reasons,
                &format!("benchmark row missing public-claim field: {field}"),
            );
        }
    }
    if row["availability_status"] != "run" {
        if row["availability_reason"]
            .as_str()
            .is_none_or(|value| value.trim().is_empty())
        {
            push_unique_reason(
                reasons,
                "benchmark unavailable row missing availability_reason",
            );
        }
        return;
    }
    if !row["byte_perfect_recovery"].is_boolean() {
        push_unique_reason(
            reasons,
            "benchmark row missing public-claim field: byte_perfect_recovery",
        );
    } else if row["byte_perfect_recovery"] != true {
        push_unique_reason(reasons, "benchmark row failed byte-perfect recovery");
    }
    match row["exact_expand_checks"].as_array() {
        Some(checks) if !checks.is_empty() => {
            if !checks.iter().all(|check| check["byte_perfect"] == true) {
                push_unique_reason(reasons, "benchmark row has non-byte-perfect expand checks");
            }
            if checks.iter().any(|check| {
                check["ref"]
                    .as_str()
                    .is_none_or(|value| !value.starts_with("tz://"))
            }) {
                push_unique_reason(reasons, "benchmark row exact expand check missing ref");
            }
        }
        Some(_) => push_unique_reason(reasons, "benchmark row has non-byte-perfect expand checks"),
        None => push_unique_reason(
            reasons,
            "benchmark row missing public-claim field: exact_expand_checks",
        ),
    }
}

artifact_gate! { evaluate_adapter_approval_claim_gate, "adapter_approval", "adapter approval artifact not attached to public claim", |artifact| {
let mut reasons = failures! {
        artifact["schema_version"] != "tokenzero.adapter_approval_audit.v1" => "adapter approval artifact schema invalid";
        artifact["blind_install_attempted"] == true => "adapter approval artifact attempted blind install";
        artifact["execution_allowed"] != true => "adapter approval artifact does not allow execution";
        artifact["public_claims_approved"] != true => "adapter approval artifact not approved for public claims";
        artifact["missing_reviewed_command_count"].as_u64().unwrap_or(1) > 0 => "adapter approval artifact has missing reviewed commands";
        artifact["unsafe_command_count"].as_u64().unwrap_or(1) > 0 => "adapter approval artifact has unsafe reviewed commands";
        artifact["duplicate_command_count"].as_u64().unwrap_or(0) > 0 => "adapter approval artifact has duplicate reviewed commands";
        artifact["required_adapter_count"].as_u64().unwrap_or(0) < REQUIRED_COMPETITOR_ADAPTERS.len() as u64 => "adapter approval artifact does not cover required adapters";
    };
let covered = artifact["adapters"].as_array().is_some_and(|rows| {
    REQUIRED_COMPETITOR_ADAPTERS.iter().all(|required| rows.iter().any(|row| {
        row["tool"].as_str() == Some(*required) && row["approval_status"] == "reviewed"
            && row["reviewed_command"].as_str().is_some_and(|command| !command.trim().is_empty() && command != "null")
    }))
});
if !covered { push_unique_reason(&mut reasons, "adapter approval artifact rows do not cover required adapters"); }
(reasons, object!({"schema_version": artifact["schema_version"], "execution_allowed": artifact["execution_allowed"], "public_claims_approved": artifact["public_claims_approved"], "blind_install_attempted": artifact["blind_install_attempted"], "required_adapter_count": artifact["required_adapter_count"], "reviewed_command_count": artifact["reviewed_command_count"], "missing_reviewed_command_count": artifact["missing_reviewed_command_count"], "duplicate_command_count": artifact["duplicate_command_count"], "unsafe_command_count": artifact["unsafe_command_count"]}))
} }

artifact_gate! { evaluate_recovery_claim_gate, "recovery_artifact", "byte-perfect recovery proof not attached to public claim", |artifact| {
let rows = artifact["normal_rows"].as_array();
let reasons = failures! {
        artifact["schema_version"] != "tokenzero.exact_recovery_audit.v1" => "recovery artifact schema mismatch";
        artifact["ok"] != true => "recovery artifact did not pass";
        !rows.is_some_and(|rows| !rows.is_empty() && rows.iter().all(|row| row["all_refs_recover"] == true)) => "byte-perfect recovery proof not attached to public claim";
    };
(reasons, object!({"schema_version": artifact["schema_version"], "normal_row_count": rows.map_or(0,Vec::len)}))
} }

artifact_gate! { evaluate_task_success_claim_gate, "task_success_artifact", "task-success proof not attached to public claim", |artifact| {
let rows = artifact["rows"].as_array();
let reasons = failures! {
        artifact["schema_version"] != "tokenzero.one_shot_eval.v1" => "task-success artifact schema mismatch";
        artifact["ok"] != true || artifact["critical_miss_rate"] != 0.0 || rows.is_none_or(|rows| rows.is_empty() || !rows.iter().all(|row| row["task_success"] == true)) => "task-success proof not attached to public claim";
    };
(reasons, object!({"schema_version": artifact["schema_version"], "critical_miss_rate": artifact["critical_miss_rate"], "row_count": rows.map_or(0,Vec::len)}))
} }

artifact_gate! { evaluate_os_claim_gate, "os_artifact", "OS artifact set not attached to public claim", |artifact| {
let reasons = failures! {
        artifact["schema_version"] != "tokenzero.os_reach_audit.v1" => "OS artifact schema mismatch";
        artifact["all_release_oses_run"] != true => "OS artifact set missing required release OS rows";
        artifact["public_os_claim_approved"] != true => "OS artifact set not approved for public claim";
    };
(reasons, object!({"schema_version": artifact["schema_version"], "all_release_oses_run": artifact["all_release_oses_run"], "public_os_claim_approved": artifact["public_os_claim_approved"]}))
} }
