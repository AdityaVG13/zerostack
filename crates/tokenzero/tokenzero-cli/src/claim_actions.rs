use crate::artifact_contracts::load_json_artifact;
use serde_json::json;
use std::path::Path;

fn values(value: &serde_json::Value) -> Vec<serde_json::Value> {
    value.as_array().cloned().unwrap_or_default()
}

pub(crate) fn completion_claim_gate_snapshot(path: &Path) -> serde_json::Value {
    let artifact = match load_json_artifact(path) {
        Ok(artifact) => artifact,
        Err(error) => {
            return json!({
                "present": false, "artifact_path": path.display().to_string(),
                "public_claims_approved": false, "gate_passes": {},
                "blocked_reasons": ["claim audit artifact missing or unreadable"],
                "release_candidate_ids": [], "release_candidate_artifacts": [],
                "error": error.to_string()
            });
        }
    };
    let mut gate_passes = artifact["gate_passes"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut gate_reasons = artifact["gate_reasons"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let ids_present = !artifact["release_candidate_ids"].is_null();
    let artifacts_present = !artifact["release_candidate_artifacts"].is_null();
    let mut ids = values(&artifact["release_candidate_ids"]);
    let mut artifacts = values(&artifact["release_candidate_artifacts"]);
    for gate in values(&artifact["evidence_gates"]) {
        let Some(id) = gate["id"].as_str() else {
            continue;
        };
        gate_passes
            .entry(id.to_string())
            .or_insert_with(|| gate["pass"].clone());
        gate_reasons
            .entry(id.to_string())
            .or_insert_with(|| values(&gate["reasons"]).into());
        if id == "release_candidate" {
            if !ids_present {
                ids = values(&gate["details"]["release_candidate_ids"]);
            }
            if !artifacts_present {
                artifacts = values(&gate["details"]["artifacts"]);
            }
        }
    }
    json!({
        "present": true, "artifact_path": path.display().to_string(),
        "schema_version": artifact["schema_version"],
        "release_candidate_id": artifact["release_candidate_id"],
        "public_claims_approved": artifact["public_claims_approved"],
        "gate_passes": gate_passes, "gate_reasons": gate_reasons,
        "blocked_reasons": values(&artifact["blocked_reasons"]),
        "release_candidate_ids": ids, "release_candidate_artifacts": artifacts
    })
}

pub(crate) fn completion_residual_gate_matrix(
    claim_gate_snapshot: &serde_json::Value,
) -> serde_json::Value {
    let Some(gate_passes) = claim_gate_snapshot["gate_passes"].as_object() else {
        return json!([]);
    };
    let mut rows = Vec::new();
    for (gate_id, pass) in gate_passes {
        if pass == &serde_json::Value::Bool(true) {
            continue;
        }
        let reasons = claim_gate_snapshot["gate_reasons"][gate_id]
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                claim_gate_snapshot["blocked_reasons"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            });
        let (next_action_id, owner, stop_before) = completion_gate_next_action(gate_id, &reasons);
        rows.push(json!({
            "gate_id": gate_id,
            "status": "blocked",
            "blocked_reasons": reasons,
            "next_action_id": next_action_id,
            "next_action": artifact_loop_next_action(next_action_id),
            "owner": owner,
            "stop_before": stop_before
        }));
    }
    json!(rows)
}

struct GateAction {
    gate_id: &'static str,
    action_id: &'static str,
    owner: &'static str,
    stop_before: &'static [&'static str],
}

const GATE_ACTIONS: &[GateAction] = &[
    GateAction {
        gate_id: "source_currency",
        action_id: "source_currency_refresh",
        owner: "product/release",
        stop_before: &["publication", "public benchmark claim"],
    },
    GateAction {
        gate_id: "benchmark_artifact",
        action_id: "benchmark_publication_approval",
        owner: "product/release",
        stop_before: &["publication", "public benchmark claim"],
    },
    GateAction {
        gate_id: "adapter_approval",
        action_id: "runnable_adapter_approval",
        owner: "bench/release",
        stop_before: &["competitor execution", "public benchmark claim"],
    },
    GateAction {
        gate_id: "os_artifact",
        action_id: "os_matrix_expansion",
        owner: "release/verification",
        stop_before: &["OS-agnostic public claim", "publication"],
    },
    GateAction {
        gate_id: "release_approval",
        action_id: "final_false_closure_audit",
        owner: "implementer",
        stop_before: &["release", "publication", "global install apply"],
    },
];

fn completion_gate_next_action(
    gate_id: &str,
    reasons: &[serde_json::Value],
) -> (&'static str, &'static str, Vec<&'static str>) {
    if gate_id == "benchmark_artifact"
        && reason_values_contain(
            reasons,
            "benchmark competitor rows must be runnable for public claims",
        )
    {
        return (
            "runnable_adapter_approval",
            "bench/release",
            vec!["competitor execution", "public benchmark claim"],
        );
    }
    GATE_ACTIONS
        .iter()
        .find(|row| row.gate_id == gate_id)
        .map_or(
            (
                "final_false_closure_audit",
                "implementer",
                vec!["release", "publication"],
            ),
            |row| (row.action_id, row.owner, row.stop_before.to_vec()),
        )
}

fn reason_values_contain(reasons: &[serde_json::Value], needle: &str) -> bool {
    reasons.iter().any(|reason| reason.as_str() == Some(needle))
}

pub(crate) fn artifact_loop_next_actions(
    residual_gate_matrix: &serde_json::Value,
) -> Vec<serde_json::Value> {
    let mut ids = Vec::new();
    for id in residual_gate_matrix
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row["next_action_id"].as_str())
    {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if !ids.contains(&"final_false_closure_audit") {
        ids.push("final_false_closure_audit");
    }
    ids.into_iter()
        .map(artifact_loop_next_action)
        .filter(|action| !action.is_null())
        .collect()
}

struct NextAction {
    id: &'static str,
    owner: &'static str,
    action: &'static str,
    validation: &'static str,
    stop_condition: &'static str,
}

const NEXT_ACTIONS: &[NextAction] = &[
    NextAction {
        id: "source_currency_refresh",
        owner: "product/release",
        action: "refresh primary source pages and pin release-candidate IDs across source, benchmark, recovery, task-success, OS, and adapter approval artifacts before public claims",
        validation: "tokenzero source-currency-audit --json and tokenzero claim-audit --source-artifact <source.json> --benchmark-artifact <bench.json> --adapter-approval-artifact <adapter.json> --recovery-artifact <recovery.json> --task-success-artifact <task.json> --os-artifact <os.json> --json",
        stop_condition: "do not publish savings/superiority claims while fresh_for_public_claim is false or release_candidate gate fails",
    },
    NextAction {
        id: "runnable_adapter_approval",
        owner: "bench/release",
        action: "approve reviewed competitor commands, link them into the benchmark as approved_not_executed evidence, and only then decide whether an explicitly approved execution phase is warranted",
        validation: "tokenzero adapter-approval-audit --approval-file <reviewed.json> --execution-approval --json, then tokenzero bench competitors --adapter-approval-artifact <adapter-approval.json> --json and inspect approved_not_executed rows before any runnable execution",
        stop_condition: "no blind install, no unreviewed competitor binary execution, and no public benchmark claim from approved_not_executed rows",
    },
    NextAction {
        id: "benchmark_publication_approval",
        owner: "product/release",
        action: "approve benchmark publication only after source, adapter, recovery, task-success, and OS evidence gates agree",
        validation: "tokenzero claim-audit --benchmark-artifact <bench.json> --adapter-approval-artifact <adapter.json> --source-artifact <source.json> --recovery-artifact <recovery.json> --task-success-artifact <task.json> --os-artifact <os.json> --json",
        stop_condition: "do not publish benchmark superiority claims until claim-audit reports public_claims_approved=true and release approval is explicit",
    },
    NextAction {
        id: "final_false_closure_audit",
        owner: "implementer",
        action: "rerun completion audit and reconcile every residual gate before claiming completion",
        validation: "tokenzero completion-audit --json",
        stop_condition: "completion_achieved must remain false until every required evidence row is direct and current",
    },
];

fn artifact_loop_next_action(action_id: &str) -> serde_json::Value {
    if action_id == "os_matrix_expansion" {
        return os_matrix_expansion_next_action();
    }
    NEXT_ACTIONS
        .iter()
        .find(|action| action.id == action_id)
        .map_or(serde_json::Value::Null, |action| {
            json!({
                "id": action.id,
                "owner": action.owner,
                "action": action.action,
                "validation": action.validation,
                "stop_condition": action.stop_condition
            })
        })
}

fn os_matrix_expansion_next_action() -> serde_json::Value {
    let missing =
        missing_release_os_rows(Path::new("results/current/tokenzero_os_reach_audit.json"));
    let missing_display = if missing.is_empty() {
        "Windows, Linux, and macOS".to_string()
    } else {
        join_human_list(
            &missing
                .iter()
                .map(|os| release_os_display_name(os).to_string())
                .collect::<Vec<_>>(),
        )
    };
    let artifact_args = if missing.is_empty() {
        "--os-artifact <windows.json> --os-artifact <linux.json> --os-artifact <macos.json>"
            .to_string()
    } else {
        missing
            .iter()
            .map(|os| format!("--os-artifact <{os}.json>"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    json!({
        "id": "os_matrix_expansion",
        "owner": "release/verification",
        "action": format!("run os-release-artifact on {missing_display}, then rerun OS reach audit with those artifacts"),
        "validation": format!("tokenzero os-release-artifact --json on {missing_display}, then tokenzero os-reach-audit {artifact_args} --json with Windows/Linux/macOS release-candidate rows"),
        "missing_release_oses": missing,
        "stop_condition": "do not claim OS-agnostic until all release OS rows pass"
    })
}

pub(crate) fn missing_release_os_rows(path: &Path) -> Vec<String> {
    let Ok(artifact) = load_json_artifact(path) else {
        return ["windows", "linux", "macos"]
            .iter()
            .map(|os| os.to_string())
            .collect();
    };
    let rows = artifact["os_rows"].as_array().cloned().unwrap_or_default();
    ["windows", "linux", "macos"]
        .iter()
        .filter(|os| {
            !rows
                .iter()
                .any(|row| row["os"].as_str() == Some(**os) && row["claim_ready"] == true)
        })
        .map(|os| os.to_string())
        .collect()
}

fn release_os_display_name(os: &str) -> &str {
    match os {
        "windows" => "Windows",
        "linux" => "Linux",
        "macos" => "macOS",
        _ => os,
    }
}

pub(crate) fn os_matrix_residual_message(missing: &[String]) -> String {
    if missing.is_empty() {
        return "all release OS artifacts are present; public OS claim still requires release approval"
            .to_string();
    }
    let missing_display = join_human_list(
        &missing
            .iter()
            .map(|os| release_os_display_name(os).to_string())
            .collect::<Vec<_>>(),
    );
    format!("{missing_display} shell and install artifacts missing")
}

fn os_purpose(missing: &[String], complete: &str, incomplete_prefix: &str) -> String {
    if missing.is_empty() {
        complete.to_string()
    } else {
        let oses = release_os_list_display(missing);
        if incomplete_prefix.contains("{}") {
            incomplete_prefix.replacen("{}", &oses, 1)
        } else {
            format!("{incomplete_prefix} {oses}")
        }
    }
}

pub(crate) fn os_reach_artifact_purpose(missing: &[String]) -> String {
    os_purpose(
        missing,
        "Windows, Linux, and macOS OS reach proof with no missing release OS rows",
        "OS reach evidence with {} release claim still blocked",
    )
}

pub(crate) fn os_release_artifact_purpose(missing: &[String]) -> String {
    os_purpose(
        missing,
        "Release artifact schema for completed Windows, Linux, and macOS OS matrix runs",
        "Current release artifact schema; next OS release artifact needed for",
    )
}

pub(crate) fn release_os_list_display(oses: &[String]) -> String {
    join_human_list(
        &oses
            .iter()
            .map(|os| release_os_display_name(os).to_string())
            .collect::<Vec<_>>(),
    )
}

fn join_human_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [first, second] => format!("{first} and {second}"),
        _ => {
            let mut joined = items[..items.len() - 1].join(", ");
            joined.push_str(", and ");
            joined.push_str(&items[items.len() - 1]);
            joined
        }
    }
}

pub(crate) fn handoff_resolve_residual_next_actions(
    residual_gate_matrix: &serde_json::Value,
    next_actions: &[serde_json::Value],
) -> serde_json::Value {
    let Some(rows) = residual_gate_matrix.as_array() else {
        return json!([]);
    };

    let enriched_rows = rows
        .iter()
        .map(|row| {
            let mut object = row.as_object().cloned().unwrap_or_default();
            let next_action = object
                .get("next_action_id")
                .and_then(serde_json::Value::as_str)
                .and_then(|id| {
                    next_actions
                        .iter()
                        .find(|action| action["id"].as_str() == Some(id))
                })
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            object.insert("next_action".to_string(), next_action);
            serde_json::Value::Object(object)
        })
        .collect::<Vec<_>>();

    json!(enriched_rows)
}
