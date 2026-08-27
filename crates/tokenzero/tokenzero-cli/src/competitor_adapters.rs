use anyhow::{Context, Result};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

use crate::source_currency;

macro_rules! str_table {
    ($name:ident, $($value:literal),+ $(,)?) => {
        pub(crate) const $name: &[&str] = &[$($value),+];
    };
}
macro_rules! pair_table {
    ($name:ident, $($needle:literal => $reason:literal),+ $(,)?) => {
        pub(crate) const $name: &[(&str, &str)] = &[$(($needle, $reason)),+];
    };
}

str_table!(
    REQUIRED_COMPETITOR_ADAPTERS,
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
);
str_table!(
    PACKAGE_INSTALL_PATTERNS,
    "npm install",
    "npm i ",
    "pnpm add",
    "pnpm install",
    "pnpm dlx",
    "yarn add",
    "yarn install",
    "cargo install",
    "go install",
    "gem install",
    "pip install",
    "pip3 install",
    "pipx install",
    "uv tool install",
    "uv pip install",
    "brew install",
    "choco install",
    "winget install",
    "scoop install",
    "apt install",
    "apt-get install",
);
pair_table!(UNSAFE_PATTERNS,
    "git clone" => "external fetch or container execution command is not reviewed-safe",
    "docker run" => "external fetch or container execution command is not reviewed-safe",
    "docker pull" => "external fetch or container execution command is not reviewed-safe",
    "rm -rf" => "destructive filesystem command is not reviewed-safe",
    "remove-item" => "destructive filesystem command is not reviewed-safe",
);
pair_table!(PIPED_FETCH_PATTERNS,
    "curl" => "curl pipe execution is not reviewed-safe",
    "irm " => "PowerShell web pipe execution is not reviewed-safe",
    "iwr " => "PowerShell web pipe execution is not reviewed-safe",
);

fn adapter_row_json(
    suite: &str,
    source: &source_currency::CompetitorAdapterSource,
    approved_not_executed: bool,
    approval_row: Option<&serde_json::Value>,
) -> serde_json::Value {
    let adapter_command = if approved_not_executed {
        approval_row
            .and_then(|r| r["reviewed_command"].as_str())
            .map(|c| json!(c))
            .unwrap_or(serde_json::Value::Null)
    } else {
        serde_json::Value::Null
    };
    let (status, reason, depth) = if approved_not_executed {
        (
            "approved_not_executed",
            "adapter command reviewed and explicitly approved, but competitor execution is still not performed by this local proof command",
            "approved_adapter_not_executed",
        )
    } else {
        (
            "unavailable",
            "adapter command is approval-gated; no blind install or unreviewed competitor binary execution in this local proof",
            "adapter_not_run",
        )
    };
    let notes = if approved_not_executed {
        format!(
            "adapter row linked to reviewed approval artifact; {} command is not executed or blindly installed",
            source.tool
        )
    } else {
        format!(
            "adapter row accounted from source ledger; {} is unavailable rather than fabricated or blindly installed",
            source.tool
        )
    };
    json!({
        "schema_version": "tokenzero.bench.v1", "suite": suite,
        "scenario_id": format!("adapter_{}", source.tool),
        "tool": source.tool, "adapter_kind": "competitor",
        "adapter_allowlisted": true, "blind_install_attempted": false,
        "availability_status": status, "availability_reason": reason,
        "adapter_command": adapter_command,
        "adapter_command_reviewed": approved_not_executed,
        "adapter_source_url": source.url,
        "adapter_source_commit": source.source_commit,
        "raw_tokens": 0, "visible_tokens": 0, "recovery_tokens": 0,
        "recovery_adjusted_savings": 0.0, "byte_perfect_recovery": false,
        "task_success": false, "harm_gate": "not_evaluated_unavailable",
        "harm_rate": 0.0, "latency_overhead_ms": 0,
        "host_coverage": ["cli"], "interception_depth": depth,
        "safe_savings": 0.0, "fairness_notes": notes
    })
}

fn approved_not_executed<'a>(
    adapter_approval: Option<&'a serde_json::Value>,
    source: &source_currency::CompetitorAdapterSource,
) -> Option<&'a serde_json::Value> {
    let artifact = adapter_approval?;
    if artifact["schema_version"] != "tokenzero.adapter_approval_audit.v1"
        || artifact["execution_allowed"] != true
        || artifact["public_claims_approved"] != true
        || artifact["blind_install_attempted"] != false
    {
        return None;
    }
    let row = artifact["adapters"]
        .as_array()?
        .iter()
        .find(|r| r["tool"] == source.tool)?;
    if row["approval_status"] != "reviewed"
        || row["execution_allowed"] != true
        || row["blind_install_attempted"] != false
        || row["reviewed_command"]
            .as_str()
            .is_none_or(|c| c.is_empty() || adapter_command_unsafe_reason(c).is_some())
    {
        return None;
    }
    Some(row)
}

pub(crate) fn load_benchmark_adapter_approval(
    path: Option<&PathBuf>,
) -> Result<Option<serde_json::Value>> {
    let Some(path) = path else { return Ok(None) };
    serde_json::from_slice::<serde_json::Value>(
        &fs::read(path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("parse {}", path.display()))
    .map(Some)
}

pub(crate) fn competitor_adapter_rows(
    suite: &str,
    adapter_approval: Option<&serde_json::Value>,
) -> Vec<serde_json::Value> {
    source_currency::competitor_adapter_sources()
        .map(|source| {
            let row = approved_not_executed(adapter_approval, &source);
            adapter_row_json(suite, &source, row.is_some(), row)
        })
        .collect()
}

pub(crate) fn competitor_adapter_matrix(adapter_rows: &[serde_json::Value]) -> serde_json::Value {
    let accounted: Vec<_> = adapter_rows
        .iter()
        .filter_map(|r| r["tool"].as_str())
        .collect();
    let all_accounted = REQUIRED_COMPETITOR_ADAPTERS
        .iter()
        .all(|t| accounted.iter().any(|rt| rt == t));
    let counts = |s: &str| {
        adapter_rows
            .iter()
            .filter(|r| r["availability_status"] == s)
            .count()
    };
    let blind = adapter_rows
        .iter()
        .any(|r| r["blind_install_attempted"] == true);
    let ok = all_accounted && !blind;
    json!({
        "schema_version": "tokenzero.adapter_matrix.v1",
        "status": if ok { "ok" } else { "blocked" }, "ok": ok,
        "required_adapter_count": REQUIRED_COMPETITOR_ADAPTERS.len(),
        "adapter_total_rows": adapter_rows.len(),
        "runnable_adapter_count": counts("run"),
        "approved_adapter_count": counts("approved_not_executed"),
        "unavailable_adapter_count": counts("unavailable"),
        "all_required_adapters_accounted": all_accounted,
        "blind_install_attempted": blind,
        "public_claims_approved": false,
        "release_publication_allowed": false,
        "blocked_reasons": [
            "competitor adapter execution requires reviewed commands and approval",
            "unavailable rows are evidence-accounting rows, not public superiority measurements"
        ]
    })
}

pub(crate) fn adapter_approval_audit_report(
    approval_file: Option<&Path>,
    execution_approval: bool,
    release_candidate_id: &str,
) -> Result<serde_json::Value> {
    let approval = approval_file
        .map(|p| {
            fs::read(p).map_err(anyhow::Error::from).and_then(|bytes| {
                serde_json::from_slice::<serde_json::Value>(&bytes).map_err(anyhow::Error::from)
            })
        })
        .transpose()?;
    let schema_valid = approval
        .as_ref()
        .is_some_and(|v| v["schema_version"] == "tokenzero.adapter_approval_file.v1");
    let approved_cmds = approval
        .as_ref()
        .filter(|_| schema_valid)
        .and_then(|v| v["commands"].as_array())
        .cloned()
        .unwrap_or_default();

    let mut reviewed_cnt = 0usize;
    let mut unsafe_cnt = 0usize;
    let adapters: Vec<_> = REQUIRED_COMPETITOR_ADAPTERS.iter().map(|tool| {
        let matching: Vec<_> = approved_cmds.iter().filter(|r| r["tool"].as_str() == Some(*tool)).collect();
        let dup = matching.len() > 1;
        let (status, cmd, reason) = if dup {
            ("duplicate_command", None, None)
        } else if let Some(row) = matching.first() {
            let r = row["reviewed"].as_bool().unwrap_or(false);
            let c = row["command"].as_str().unwrap_or("").trim();
            let reason = adapter_command_unsafe_reason(c);
            if r && !c.is_empty() && reason.is_none() { reviewed_cnt += 1; ("reviewed", Some(c.to_string()), None) }
            else if reason.is_some() { unsafe_cnt += 1; ("unsafe_command", Some(c.to_string()), reason) }
            else { ("missing_reviewed_command", None, None) }
        } else { ("missing_reviewed_command", None, None) };
        json!({
            "tool": tool, "url": source_currency::competitor_source_url(tool).unwrap_or(""),
            "approval_status": status,
            "execution_allowed": execution_approval && status == "reviewed",
            "reviewed_command": cmd, "unsafe_reason": reason,
            "blind_install_attempted": false, "sandbox_required": true, "approval_required": true
        })
    }).collect();

    let missing_cnt = adapters
        .iter()
        .filter(|r| r["approval_status"] == "missing_reviewed_command")
        .count();
    let dup_cnt = adapters
        .iter()
        .filter(|r| r["approval_status"] == "duplicate_command")
        .count();
    let mut blocked = Vec::new();
    if approval.is_some() && !schema_valid {
        blocked.push("adapter approval file schema invalid".to_string());
    }
    if dup_cnt > 0 {
        blocked.push("duplicate adapter approval commands rejected".to_string());
    }
    if missing_cnt > 0 {
        blocked.push("reviewed competitor commands missing".to_string());
    }
    if unsafe_cnt > 0 {
        blocked.push("unsafe reviewed competitor commands rejected".to_string());
    }
    if !execution_approval {
        blocked.push("explicit runnable adapter execution approval not granted".to_string());
    }
    let all_reviewed = schema_valid
        && reviewed_cnt == REQUIRED_COMPETITOR_ADAPTERS.len()
        && missing_cnt == 0
        && unsafe_cnt == 0
        && dup_cnt == 0;
    let exec_ok = execution_approval && all_reviewed;
    let status = if exec_ok { "ok" } else { "blocked" };

    Ok(json!({
        "schema_version": "tokenzero.adapter_approval_audit.v1",
        "status": status, "ok": exec_ok,
        "release_candidate_id": release_candidate_id,
        "execution_approval_granted": execution_approval,
        "execution_allowed": exec_ok, "public_claims_approved": exec_ok,
        "release_publication_allowed": false, "blind_install_attempted": false,
        "approval_file_schema_valid": schema_valid,
        "approval_file": approval_file.map(|p| p.display().to_string()),
        "required_adapter_count": REQUIRED_COMPETITOR_ADAPTERS.len(),
        "reviewed_command_count": reviewed_cnt, "unsafe_command_count": unsafe_cnt,
        "duplicate_command_count": dup_cnt, "missing_reviewed_command_count": missing_cnt,
        "command_safety_policy": adapter_command_safety_policy(),
        "adapters": adapters, "blocked_reasons": blocked
    }))
}

pub(crate) fn adapter_approval_template_report(release_candidate_id: &str) -> serde_json::Value {
    let cmds: Vec<_> = REQUIRED_COMPETITOR_ADAPTERS
        .iter()
        .map(|tool| {
            json!({
                "tool": tool, "url": source_currency::competitor_source_url(tool).unwrap_or(""),
                "reviewed": true, "command": format!("{tool} --version"),
                "review_scope": "command-shape safety review only; does not approve execution",
                "execution_approval_required": true
            })
        })
        .collect();
    json!({
        "schema_version": "tokenzero.adapter_approval_file.v1",
        "status": "template", "ok": false, "exit_code": 0,
        "release_candidate_id": release_candidate_id,
        "generated_by": "tokenzero adapter-approval-template",
        "review_scope": "command-shape safety review only; explicit execution approval remains required",
        "execution_approval_required": true, "public_claims_approved": false,
        "required_adapter_count": REQUIRED_COMPETITOR_ADAPTERS.len(),
        "command_count": cmds.len(),
        "command_safety_policy": adapter_command_safety_policy(),
        "commands": cmds
    })
}

fn adapter_command_safety_policy() -> serde_json::Value {
    json!({
        "schema_version": "tokenzero.adapter_command_safety.v1",
        "blocked_side_effects": [
            "package_manager_install", "external_fetch_or_container_execution",
            "curl_pipe_execution", "powershell_web_pipe_execution",
            "destructive_filesystem_command"
        ],
        "execution_policy": "reviewed commands may be linked as approved_not_executed evidence; commands with install/fetch/container/destructive side effects remain blocked before any runnable execution phase"
    })
}

fn adapter_command_unsafe_reason(command: &str) -> Option<&'static str> {
    let lower = command.to_ascii_lowercase();
    if PACKAGE_INSTALL_PATTERNS.iter().any(|p| lower.contains(p)) {
        return Some("package manager install command is not reviewed-safe");
    }
    for (needle, reason) in UNSAFE_PATTERNS {
        if lower.contains(needle) {
            return Some(reason);
        }
    }
    for (prefix, reason) in PIPED_FETCH_PATTERNS {
        if lower.contains(prefix) && lower.contains('|') {
            return Some(reason);
        }
    }
    None
}
