use anyhow::{Context, Result};
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

struct CompetitorSource {
    tool: &'static str,
    url: &'static str,
    source_commit: &'static str,
    claimed_scope: &'static str,
    issue_pr_themes: &'static [&'static str],
    strengths: &'static [&'static str],
    gaps: &'static [&'static str],
}

pub(crate) struct CompetitorAdapterSource {
    pub(crate) tool: &'static str,
    pub(crate) url: &'static str,
    pub(crate) source_commit: &'static str,
}

const COMPETITOR_SOURCE_DATE: &str = "2026-06-04";

macro_rules! competitor_sources {
    ($(($tool:literal, $url:literal, $scope:literal,
        [$($theme:literal),*], [$($strength:literal),*], [$($gap:literal),*]
    )),* $(,)?) => {
        const COMPETITOR_SOURCES: &[CompetitorSource] = &[$(CompetitorSource {
            tool: $tool,
            url: $url,
            source_commit: "snapshot-20260604",
            claimed_scope: $scope,
            issue_pr_themes: &[$($theme),*],
            strengths: &[$($strength),*],
            gaps: &[$($gap),*],
        }),*];
    };
}

competitor_sources! {
    ("rtk", "https://github.com/rtk-ai/rtk", "Rust command proxy and shell-hook command filters", ["parser coverage", "install/platform", "deps/CI", "reach"], ["command-filter mindshare", "large adoption signal"], ["exact recovery beyond command filters", "Safe Savings evidence"]),
    ("ztk", "https://github.com/codejunkie99/ztk", "Tiny Zig binary with hooks and manual raw mode", ["stderr preservation", "failure fidelity"], ["small binary", "manual raw mode"], ["diagnostic invariants", "host reach evidence"]),
    ("lean-ctx", "https://github.com/yvgude/lean-ctx", "Context OS with MCP tools, read modes, memory, routing, and dashboard", ["Windows Codex compression gap", "signature map"], ["MCP breadth", "context routing"], ["daemonless core proof", "exact runtime contract"]),
    ("tokenpak", "https://github.com/tokenpak/tokenpak", "Local HTTP proxy with compression recipes, cost tracking, and routing", ["coverage gate mismatch", "pricing bootstrap", "model leak", "stream aborts"], ["cost tracking", "provider routing"], ["proxy default", "provider path fragility"]),
    ("tokenjuice", "https://github.com/vincentkoc/tokenjuice", "Deterministic terminal output compaction and broad integrations", ["dependency maintenance", "adapter breadth"], ["deterministic output compaction", "host adapter breadth"], ["exact recovery proof", "recovery-adjusted benchmark metrics"]),
    ("context-mode", "https://github.com/mksglu/context-mode", "MCP sandbox, search/index, FTS knowledge base, and high reduction summaries", ["concurrency flood guard", "cwd bug", "install manifests", "platform beta coverage"], ["MCP search/index breadth", "sandbox model"], ["no pre-index default", "shell truth", "one-shot adequacy"]),
    ("caveman", "https://github.com/JuliusBrussee/caveman", "Claude skill and terse agent communication style", ["install/platform failures", "Node removal", "OpenCode", "PowerShell"], ["social spread", "prompt-style compression"], ["runtime proof system", "exact recovery", "protected-anchor recall"]),
    ("cavekit", "https://github.com/JuliusBrussee/cavekit", "Natural-language blueprint and planning validation", ["manual install", "Codex port", "security", "CLI support"], ["planning flow", "blueprint validation"], ["runtime compression", "exact refs"]),
    ("cavemem", "https://github.com/JuliusBrussee/cavemem", "Cross-agent compressed local memory", ["Windows paths", "Codex installer", "session leakage", "sqlite backend"], ["cross-agent memory", "local compression"], ["daemon default avoidance", "source/context boundaries"]),
    ("caveman-code", "https://github.com/JuliusBrussee/caveman-code", "Agent/code integration layer", ["install", "login", "provider failures"], ["code-agent integration"], ["host-integration failure evidence", "recovery proof"]),
    ("headroom", "https://github.com/chopratejas/headroom", "Library/proxy/MCP compression with retrieval store", ["provider-agnostic proxy", "timeout", "telemetry", "Homebrew", "RTK hook"], ["library/proxy/MCP surface", "retrieval store"], ["no-proxy default", "exact local runtime"]),
    ("engram", "https://github.com/pythondatascrape/engram", "Local daemon for conversation history compression", ["identity compression compatibility"], ["conversation history compression"], ["daemon default", "on-demand cache packs"]),
    ("claw", "https://github.com/open-compress/claw-compactor", "Reversible multi-stage compression and RewindStore", ["credential exposure reports", "RewindStore persistence"], ["reversible pipeline", "multi-stage compression"], ["CLI/MCP exact refs", "security gates"]),
    ("contextpilot", "https://github.com/msousa202/ContextPilot", "SDK wrapper, proxy, MCP, and quality fallback", ["report capsule", "session memory", "hybrid scoring", "skeletonization"], ["quality gate concept", "SDK integration"], ["service dependency", "local exact recovery"]),
    ("wilpel-caveman-compression", "https://github.com/wilpel/caveman-compression", "Semantic grammar stripping for prompt compression", ["negation preservation", "missing Skill.md", "endpoint support"], ["semantic grammar stripping"], ["lossy protected-anchor risk", "exact recovery"]),
    ("compresh", "https://github.com/compresh/compresh", "Depth-aware proxy context compression and fetch markers", ["no active backlog snapshot"], ["depth-aware compression", "fetch markers"], ["local runtime", "recovery-adjusted accounting"]),
    ("compresh-mcp", "https://github.com/compresh/compresh-mcp", "MCP wrapper around Compresh and remote/local fallback", ["no active backlog snapshot"], ["MCP wrapper", "remote/local fallback"], ["paid/remote default risk", "exact local refs"]),
    ("context-gateway", "https://github.com/Compresr-ai/Context-Gateway", "Agentic API gateway with background history compaction", ["thinking-block 400 errors", "Copilot integration", "SSE/header PRs"], ["agentic API gateway", "session history compaction"], ["background compaction default", "Core local recovery"]),
}

pub(crate) fn competitor_adapter_sources() -> impl Iterator<Item = CompetitorAdapterSource> {
    COMPETITOR_SOURCES
        .iter()
        .map(|source| CompetitorAdapterSource {
            tool: source.tool,
            url: source.url,
            source_commit: source.source_commit,
        })
}

pub(crate) fn competitor_source_url(tool: &str) -> Option<&'static str> {
    COMPETITOR_SOURCES
        .iter()
        .find_map(|source| (source.tool == tool).then_some(source.url))
}

pub(crate) fn source_currency_report(release_candidate_id: &str) -> serde_json::Value {
    let rows = competitor_source_rows();
    let pin_audit = source_commit_pin_audit(&rows);
    json!({
        "schema_version": "tokenzero.source_currency.v1",
        "status": "blocked",
        "ok": false,
        "exit_code": 0,
        "release_candidate_id": release_candidate_id,
        "source_date": COMPETITOR_SOURCE_DATE,
        "source_snapshot": "PRD matrix generated 2026-06-03 and spot-checked 2026-06-04",
        "fresh_for_private_planning": true,
        "fresh_for_public_claim": false,
        "public_claims_approved": false,
        "release_publication_allowed": false,
        "blocked_reasons": [
            "source ledger requires same-release-candidate refresh",
            "source ledger requires release-candidate commit pins from refreshed primary sources",
            "public claims require benchmark, recovery, task-success, and approval gates to agree"
        ],
        "source_commit_pin_status": pin_audit["source_commit_pin_status"].clone(),
        "unpinned_source_rows": pin_audit["unpinned_source_rows"].clone(),
        "rows": rows
    })
}

fn competitor_source_rows() -> Vec<serde_json::Value> {
    COMPETITOR_SOURCES
        .iter()
        .map(|source| {
            json!({
                "tool": source.tool,
                "url": source.url,
                "source_date": COMPETITOR_SOURCE_DATE,
                "source_commit": source.source_commit,
                "claimed_scope": source.claimed_scope,
                "issue_pr_themes": source.issue_pr_themes,
                "strengths": source.strengths,
                "gaps": source.gaps,
                "fresh_for_private_planning": true,
                "fresh_for_public_claim": false,
                "freshness_note": "private PRD snapshot only; refresh primary source and pin release-candidate commit before public claim"
            })
        })
        .collect()
}

pub(crate) fn refreshed_source_currency_report(
    refresh_rows: Vec<serde_json::Value>,
    refresh_method: &str,
    refresh_input: Option<&Path>,
    release_candidate_id: &str,
) -> serde_json::Value {
    let mut refresh_by_tool = HashMap::<String, serde_json::Value>::new();
    for row in refresh_rows {
        if let Some(tool) = row["tool"].as_str().filter(|tool| !tool.trim().is_empty()) {
            refresh_by_tool.insert(tool.trim().to_string(), row);
        }
    }

    let mut rows = Vec::<serde_json::Value>::new();
    let mut refresh_errors = Vec::<serde_json::Value>::new();
    for source in COMPETITOR_SOURCES {
        let refresh = refresh_by_tool.get(source.tool);
        let refreshed = |key, default| {
            refresh
                .and_then(|row| row[key].as_str())
                .unwrap_or(default)
                .trim()
                .to_string()
        };
        let source_commit = refreshed("source_commit", "");
        let source_date = refreshed("source_date", COMPETITOR_SOURCE_DATE);
        let refresh_error = refresh
            .and_then(|row| row["refresh_error"].as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        if refresh.is_none() {
            refresh_errors.push(json!({
                "tool": source.tool,
                "url": source.url,
                "refresh_error": "missing refreshed source row"
            }));
        } else if let Some(error) = refresh_error.as_deref() {
            refresh_errors.push(json!({
                "tool": source.tool,
                "url": source.url,
                "refresh_error": error
            }));
        }
        let row_fresh = refresh.is_some()
            && refresh_error.is_none()
            && !source_date.is_empty()
            && source_commit_is_release_candidate_pin(&source_commit);
        rows.push(json!({
            "tool": source.tool,
            "url": source.url,
            "source_date": source_date,
            "source_commit": source_commit,
            "claimed_scope": source.claimed_scope,
            "issue_pr_themes": source.issue_pr_themes,
            "strengths": source.strengths,
            "gaps": source.gaps,
            "fresh_for_private_planning": true,
            "fresh_for_public_claim": row_fresh,
            "refresh_method": refresh_method,
            "refresh_error": refresh_error,
            "freshness_note": if row_fresh {
                "release-candidate source pin refreshed from primary source"
            } else {
                "source refresh incomplete; do not use for public claim"
            }
        }));
    }

    let pin_audit = source_commit_pin_audit(&rows);
    let pin_status = &pin_audit["source_commit_pin_status"];
    let all_pinned = pin_status["pinned"].as_u64() == Some(rows.len() as u64)
        && pin_status["missing"].as_u64() == Some(0)
        && pin_status["unpinned"].as_u64() == Some(0);
    let fresh_for_public_claim = all_pinned
        && refresh_errors.is_empty()
        && rows.iter().all(|row| row["fresh_for_public_claim"] == true);
    let blocked_reasons = if fresh_for_public_claim {
        vec![
            "public claims require benchmark, recovery, task-success, OS, adapter, and release approval gates to agree",
        ]
    } else {
        vec![
            "source ledger requires same-release-candidate refresh",
            "source ledger requires release-candidate commit pins from refreshed primary sources",
            "source refresh incomplete",
            "public claims require benchmark, recovery, task-success, and approval gates to agree",
        ]
    };

    json!({
        "schema_version": "tokenzero.source_currency.v1",
        "status": "blocked",
        "ok": false,
        "exit_code": 0,
        "release_candidate_id": release_candidate_id,
        "source_date": source_refresh_date(),
        "source_snapshot": "release-candidate source refresh",
        "refresh_method": refresh_method,
        "refresh_input": refresh_input.map(|path| path.display().to_string()),
        "fresh_for_private_planning": true,
        "fresh_for_public_claim": fresh_for_public_claim,
        "public_claims_approved": false,
        "release_publication_allowed": false,
        "blocked_reasons": blocked_reasons,
        "source_commit_pin_status": pin_audit["source_commit_pin_status"].clone(),
        "unpinned_source_rows": pin_audit["unpinned_source_rows"].clone(),
        "refresh_errors": refresh_errors,
        "rows": rows
    })
}

pub(crate) fn read_source_refresh_rows(path: &Path) -> Result<Vec<serde_json::Value>> {
    let bytes = fs::read(path)
        .with_context(|| format!("reading source refresh ledger {}", path.display()))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing source refresh ledger {}", path.display()))?;
    value["rows"]
        .as_array()
        .or_else(|| value.as_array())
        .cloned()
        .context("source refresh ledger must be a JSON array or an object with a rows array")
}

pub(crate) fn git_head_source_refresh_rows() -> Vec<serde_json::Value> {
    COMPETITOR_SOURCES
        .iter()
        .map(|source| match git_ls_remote_head(source.url) {
            Ok(source_commit) => json!({
                "tool": source.tool,
                "source_date": source_refresh_date(),
                "source_commit": source_commit
            }),
            Err(error) => json!({
                "tool": source.tool,
                "source_date": source_refresh_date(),
                "source_commit": "",
                "refresh_error": error.to_string()
            }),
        })
        .collect()
}

/// Upper bound for one remote pin lookup; a hung remote must not stall the
/// whole source-currency refresh.
const LS_REMOTE_TIMEOUT: Duration = Duration::from_secs(30);

fn git_ls_remote_head(url: &str) -> Result<String> {
    let argv = ["git", "ls-remote", url, "HEAD"]
        .map(str::to_string)
        .to_vec();
    let output = tokenzero_runtime::run_command(&argv, None, None, None, LS_REMOTE_TIMEOUT, false)
        .with_context(|| format!("running git ls-remote HEAD for {url}"))?;
    if output.timed_out {
        anyhow::bail!(
            "git ls-remote HEAD timed out after {}s for {url}",
            LS_REMOTE_TIMEOUT.as_secs()
        );
    }
    if !output.ok {
        let stderr = output.stderr.trim();
        anyhow::bail!("git ls-remote HEAD failed for {url}: {stderr}");
    }
    let source_commit = output
        .stdout
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim();
    if !source_commit_is_release_candidate_pin(source_commit) {
        anyhow::bail!("git ls-remote HEAD did not return a commit pin for {url}");
    }
    Ok(source_commit.to_string())
}

fn source_refresh_date() -> String {
    std::env::var("TOKENZERO_SOURCE_REFRESH_DATE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| COMPETITOR_SOURCE_DATE.to_string())
}

fn source_commit_pin_audit(rows: &[serde_json::Value]) -> serde_json::Value {
    let mut pinned = 0usize;
    let mut missing = 0usize;
    let mut unpinned_source_rows = Vec::<serde_json::Value>::new();
    for row in rows {
        let source_commit = row["source_commit"].as_str().unwrap_or_default().trim();
        if source_commit.is_empty() {
            missing += 1;
        } else if source_commit_is_release_candidate_pin(source_commit) {
            pinned += 1;
        } else {
            unpinned_source_rows.push(json!({
                "tool": row["tool"],
                "url": row["url"],
                "source_commit": source_commit
            }));
        }
    }
    json!({
        "source_commit_pin_status": {
            "pinned": pinned,
            "missing": missing,
            "unpinned": unpinned_source_rows.len()
        },
        "unpinned_source_rows": unpinned_source_rows
    })
}

pub(crate) fn source_commit_is_release_candidate_pin(value: &str) -> bool {
    let trimmed = value.trim();
    (7..=64).contains(&trimmed.len()) && trimmed.bytes().all(|byte| byte.is_ascii_hexdigit())
}
