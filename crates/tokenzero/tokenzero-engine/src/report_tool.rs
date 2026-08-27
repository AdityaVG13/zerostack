//! Field `report_tool_issue` allowlist (wqw.6).
//!
//! Agents report expand/root/shell failures against the primary CodeMode surface
//! name `zero_execute` (and aliases). Rejecting that name forced agents out of
//! the harness. This module owns TokenZero's reportable-name policy and the
//! MCP/CLI entry that records a structured field report.

use serde_json::{Value, json};
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Exact surface / tool names that are not covered by the prefix rules below.
const REPORTABLE_EXACT: &[&str] = &[
    "zero_execute",
    "zero-execute",
    "zerostack",
    "zero_search",
    "zero_describe",
    "execute_code",
    "codemode_search",
    "codemode_describe",
    "read",
    "expand",
    "shell",
    "edit",
    "find",
    "grep",
    "tree",
    "glob",
    "z.measure",
    "z.project",
    "z.compress",
    "z.expand",
];

/// TokenZero-owned prefixes. Sibling engines (FSZero/GraphZero) and V6
/// `zero.fs.*` / `zero.graph.*` are not this crate's report surface.
const REPORTABLE_PREFIXES: &[&str] = &["zero.token.", "tz_", "tokenzero"];

/// Returns true if `name` is a reportable tool/surface for field issue reports.
pub fn is_reportable_tool_name(name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if REPORTABLE_EXACT.iter().any(|n| *n == lower) {
        return true;
    }
    // zero_execute / zero_search style (underscore, not dotted namespaces).
    if lower.starts_with("zero_") {
        return true;
    }
    REPORTABLE_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

/// Normalize a reported tool name for storage (trim; keep original casing of body).
pub fn normalize_report_tool_name(name: &str) -> String {
    name.trim().to_string()
}

/// Build the structured field-report payload (no I/O).
pub fn build_tool_issue_report(
    tool: &str,
    summary: &str,
    detail: Option<&str>,
    session_id: Option<&str>,
) -> Result<Value, String> {
    let tool = normalize_report_tool_name(tool);
    if !is_reportable_tool_name(&tool) {
        return Err(format!(
            "tool name not reportable: {tool}. Accepted: zero_execute, zerostack, tz_* / \
             tokenzero*, zero.token.*, and TokenEngine z.measure|z.project|z.compress|z.expand. \
             FSZero/GraphZero names are not TokenZero's report surface."
        ));
    }
    let summary = summary.trim();
    if summary.is_empty() {
        return Err("summary must be non-empty".into());
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(json!({
        "schema_version": "tokenzero.tool_issue.v1",
        "status": "accepted",
        "tool": tool,
        "summary": summary,
        "detail": detail.unwrap_or("").trim(),
        "session_id": session_id.unwrap_or(""),
        "recorded_at_unix": ts,
        "note": "Field report recorded by TokenZero. Expand/root/shell failures may cite zero_execute.",
    }))
}

fn tool_issue_stem(ts: u64, tool: &str) -> String {
    let safe_tool: String = tool
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("issue-{ts}-{safe_tool}")
}

fn write_unique_report(dir: &Path, stem: &str, text: &str) -> Result<std::path::PathBuf, String> {
    for suffix in 0_u64.. {
        let file_name = if suffix == 0 {
            format!("{stem}.json")
        } else {
            format!("{stem}-{suffix}.json")
        };
        let path = dir.join(file_name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(text.as_bytes())
                    .map_err(|e| format!("write report: {e}"))?;
                return Ok(path);
            }
            Err(err) if err.kind() == ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(format!("create report: {err}")),
        }
    }
    unreachable!("u64 report suffix space exhausted")
}

/// Persist a field report under the recovery-cache parent `.tokenzero/tool-issues/`.
pub fn record_tool_issue(
    cache_path: &Path,
    tool: &str,
    summary: &str,
    detail: Option<&str>,
    session_id: Option<&str>,
) -> Result<Value, String> {
    let mut report = build_tool_issue_report(tool, summary, detail, session_id)?;
    let dir = cache_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("tool-issues");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create tool-issues dir: {e}"))?;
    let ts = report
        .get("recorded_at_unix")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    let path = write_unique_report(&dir, &tool_issue_stem(ts, tool), &text)?;
    report["report_path"] = json!(path.display().to_string());
    Ok(report)
}
