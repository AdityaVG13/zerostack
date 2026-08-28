//! GraphZero domain and catalog helpers for the hub-owned MCP transport.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerMode {
    Mcp,
}

impl ServerMode {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "mcp" => Ok(Self::Mcp),
            "codemode" => anyhow::bail!(
                "standalone CodeMode server retired; model execution is ZeroKernel (`z.find`, `z.read`)"
            ),
            other => anyhow::bail!("mode must be mcp, got {other}"),
        }
    }
}

fn store_root(repo: &Path) -> PathBuf {
    crate::commands::store_root(repo)
}

fn default_repo(params: &Value) -> Result<PathBuf> {
    let repo = params
        .get("repo")
        .or_else(|| params.get("path"))
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    PathBuf::from(repo).canonicalize().context("repo path")
}

fn tool_catalog(_mode: ServerMode) -> Vec<Value> {
    crate::mcp_catalog::lean_tool_catalog()
}

/// Public tools/list for product tests and o2uq conformance (graphzero-o2uq.5/7).
pub fn mcp_tools_list(mode: ServerMode) -> Vec<Value> {
    tool_catalog(mode)
}

/// Public tools/call payload helper over the shipped hub-owned MCP path.
///
/// Returns the MCP result payload shape (content + isError).
pub fn mcp_tools_call(mode: ServerMode, name: &str, arguments: Value) -> Value {
    let params = json!({ "name": name, "arguments": arguments });
    match call_tool(mode, name, &params) {
        Ok(result) => result,
        Err(error) => mcp_tools_call_anyhow_error_result(name, &error),
    }
}

fn mcp_text_result(text: String) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false,
    })
}

fn mcp_json_result(value: Value) -> Value {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": value,
        "isError": false,
    })
}

fn mcp_error_result(message: String, hint: String, data: Option<Value>) -> Value {
    let mut payload = json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true,
        "hint": hint,
    });
    if let (Some(d), Some(obj)) = (data, payload.as_object_mut()) {
        obj.insert("data".into(), d);
    }
    payload
}

/// CLI equivalent for agents when an MCP `tools/call` fails (stderr-style JSON uses the same `hint` field).
fn mcp_cli_hint_for_tool_name(tool_name: &str, message: &str) -> String {
    if message.contains("not in the lean MCP catalog") {
        return match tool_name {
            "stats" => "graphzero stats --repo .".into(),
            "compact" => "graphzero compact --repo .".into(),
            "graphzero_publish" => "graphzero publish --file <batch.json> --repo .".into(),
            _ => "graphzero agent-triage or graphzero capabilities".into(),
        };
    }
    match tool_name {
        "orient" => {
            if message.contains("orient surface") {
                "graphzero orient --surface symbol|context|callers|deps|outline|hot|changes|word --name|--query <…> --repo .".into()
            } else {
                "graphzero orient --surface symbol --name <SYMBOL> --repo . (or --surface context --query <task>)".into()
            }
        }
        "search" => "graphzero search --query <TEXT> --repo .".into(),
        "snap" => "graphzero snap <SYMBOL> --budget 1 --repo .".into(),
        "expand" => "graphzero expand <gz://ref> --repo .".into(),
        "index" => "graphzero index [PATH]".into(),
        "blast" | "blast_intent" => "graphzero blast --intent <TEXT> --repo .".into(),
        "reserve" => {
            "graphzero reserve declare|check|release|query --repo . (MCP action declare|check|release|list)".into()
        }
        s if s.starts_with("semantic_reserve_") => {
            "graphzero reserve declare|check|release|query --repo . (MCP action declare|check|release|list)".into()
        }
        "verify" | "verify_claim" => {
            "graphzero verify <TARGET> --claim no_remaining_callers|no_outgoing_calls|no_remaining_references|no_remaining_dependencies|symbol_removed --repo .".into()
        }
        "gz_execute_code" | "gz_codemode_search" | "gz_codemode_describe" => {
            "retired; use ZeroKernel z.find".into()
        }
        s if graphzero_engine::SURFACE_NAMES.contains(&s) => {
            format!("graphzero query-surface {s} --name|--query|--path <…> --repo .")
        }
        _ if graphzero_engine::SURFACE_NAMES.iter().any(|n| message.contains(n) && message.contains("unknown")) => {
            "graphzero orient --surface symbol|context|… (orient is MCP-only; surfaces are not named orient)".into()
        }
        _ => "graphzero agent-triage or graphzero capabilities".into(),
    }
}

fn mcp_tools_call_error_text(tool_name: &str, message: &str) -> String {
    serde_json::to_string(&json!({
        "error": message,
        "hint": mcp_cli_hint_for_tool_name(tool_name, message),
    }))
    .unwrap_or_else(|_| {
        format!(
            "{{\"error\":\"{}\",\"hint\":\"graphzero agent-triage\"}}",
            message.replace('\\', "\\\\").replace('"', "\\\"")
        )
    })
}

fn mcp_tools_call_error_result(tool_name: &str, message: &str) -> Value {
    let text = mcp_tools_call_error_text(tool_name, message);
    let hint = mcp_cli_hint_for_tool_name(tool_name, message);
    mcp_error_result(text, hint, None)
}

fn mcp_tools_call_anyhow_error_result(tool_name: &str, error: &anyhow::Error) -> Value {
    if let Some(McpDomainError(domain)) = error.downcast_ref::<McpDomainError>() {
        return mcp_tools_call_domain_error_result(tool_name, domain);
    }
    let message = error.to_string();
    if let Some((kind, retryable, message)) = extract_domain_fields_from_display(&message) {
        return mcp_tools_call_domain_error_result(
            tool_name,
            &graphzero_engine::DomainError {
                kind,
                message,
                retryable,
                op: Some(tool_name.to_string()),
                recovery_ref: None,
            },
        );
    }
    mcp_tools_call_error_result(tool_name, &message)
}

/// Typed domain error → one MCP error envelope (kind/retryable stable).
fn mcp_tools_call_domain_error_result(
    tool_name: &str,
    err: &graphzero_engine::DomainError,
) -> Value {
    let hint = mcp_cli_hint_for_tool_name(tool_name, &err.message);
    let mut payload = crate::fastmcp_adapter::typed_error_payload(err);
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("hint".into(), json!(hint));
    }
    let text = serde_json::to_string(&payload).unwrap_or_else(|_| err.message.clone());
    mcp_error_result(text, hint, Some(payload))
}

fn tool_arguments(params: &Value) -> &Value {
    params
        .get("arguments")
        .filter(|v| !v.is_null())
        .unwrap_or(params)
}

fn mcp_result_text(result: &Value) -> Result<&str> {
    result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("text"))
        .and_then(Value::as_str)
        .context("MCP tool result missing content[0].text string")
}

fn snap_query_text(args: &Value) -> Result<String> {
    args.get("query")
        .or_else(|| args.get("symbol"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .context("snap requires query or symbol")
}

fn mcp_snap(args: &Value) -> Result<Value> {
    // Domain owns snap + optional export_path/format (single execution).
    // Adapter only frames the DomainResult for MCP transport.
    let domain = domain_dispatch_result("snap", args).map_err(domain_error_to_anyhow)?;
    let text = crate::fastmcp_adapter::serialize_domain_result(&domain);
    Ok(mcp_text_result(text))
}

fn mcp_remember(args: &Value) -> Result<Value> {
    domain_dispatch_mcp("remember", args)
}

fn mcp_recall(args: &Value) -> Result<Value> {
    domain_dispatch_mcp("recall", args)
}

fn mcp_expand(args: &Value) -> Result<Value> {
    domain_dispatch_mcp("expand", args)
}

fn mcp_index(args: &Value) -> Result<Value> {
    // Domain index via dispatcher; frame CLI-compatible JSON shape.
    let domain = domain_dispatch_result("index", args).map_err(domain_error_to_anyhow)?;
    let mut payload = json!({
        "snapshot": domain.value.get("snapshot").cloned().unwrap_or(json!(0)),
        "shards": domain.value.get("shards").cloned().unwrap_or(json!(0)),
        "store": domain
            .value
            .get("store")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    });
    // Forward env-gated index phase timings when op_index attached them.
    if let Some(phases) = domain.value.get("phases") {
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("phases".into(), phases.clone());
        }
    }
    Ok(mcp_text_result(payload.to_string()))
}

fn mcp_semantic_reserve(name: &str, args: &Value) -> Result<Value> {
    // Legacy aliases map into domain reserve (single dispatch entry).
    domain_dispatch_mcp(name, args)
}

fn mcp_blast_intent(args: &Value) -> Result<Value> {
    domain_dispatch_mcp("blast", args)
}

/// Domain host used by direct CLI recipe/JSON execution. This is not an MCP
/// server or meta-tool host; it only supplies the reserve callback that sits
/// above graphzero-engine in the dependency graph.
pub(crate) struct CliCodeModeHost {
    pub(crate) store_root: std::path::PathBuf,
    pub(crate) repo_root: std::path::PathBuf,
}

impl graphzero_engine::codemode::CodeModeHostOps for CliCodeModeHost {
    fn reserve(&self, action: &str, args: &Value) -> Result<Value, String> {
        let mut routed = args.clone();
        if let Some(obj) = routed.as_object_mut() {
            obj.insert("action".into(), json!(action));
        }
        let ctx = graphzero_engine::EngineContext::for_paths(
            self.repo_root.clone(),
            self.store_root.clone(),
            graphzero_engine::AdapterKind::CodeMode,
        );
        match graphzero_engine::dispatch(&ctx, "reserve", &routed) {
            Ok(result) => Ok(result.value.get("result").cloned().unwrap_or(result.value)),
            Err(error) => Err(error.message),
        }
    }
}

fn mcp_orient(args: &Value) -> Result<Value> {
    domain_dispatch_mcp("orient", args)
}

fn mcp_search(args: &Value) -> Result<Value> {
    domain_dispatch_mcp("search", args)
}

fn mcp_reserve(args: &Value) -> Result<Value> {
    // Domain owns all reserve semantics (graphzero-o2uq.2).
    domain_dispatch_mcp("reserve", args)
}

fn mcp_verify(args: &Value) -> Result<Value> {
    domain_dispatch_mcp("verify", args)
}

fn mcp_query_surface_tool(name: &str, args: &Value) -> Result<Value> {
    // CLI query-surface helper → same domain dispatcher (graphzero-o2uq.2).
    let repo = default_repo(args)?;
    let root = store_root(&repo);
    let text = crate::query_surface_tools::call_query_surface_mcp(name, &root, &repo, args)?;
    Ok(mcp_text_result(text))
}

/// Lean FastMCP / CLI MCP path: one typed domain dispatcher (graphzero-o2uq.2/5).
/// Transport framing only — no CodeMode nesting, no JSON-RPC re-entry.
///
/// Uses [`crate::fastmcp_adapter::dispatch_once`] so each tools/call performs exactly
/// one domain dispatch and records framework-only overhead above dispatcher cost.
/// Catalog gating for product tools happens at [`call_tool`] / FastMCP registration.
fn domain_dispatch_result(
    op: &str,
    args: &Value,
) -> Result<graphzero_engine::DomainResult, graphzero_engine::DomainError> {
    let repo = default_repo(args).map_err(|e| {
        graphzero_engine::DomainError::new(
            graphzero_engine::DomainErrorKind::Validation,
            e.to_string(),
        )
        .with_op(op)
    })?;
    let root = store_root(&repo);
    crate::fastmcp_adapter::dispatch_once(op, args, repo, root, std::time::Instant::now())
        .map(|ok| ok.result)
        .map_err(|e| e.error)
}

/// Carrier that preserves the full DomainError (including explicit `retryable`)
/// through `anyhow` without reconstructing from `kind.default_retryable()`.
#[derive(Clone, Debug)]
pub struct McpDomainError(pub graphzero_engine::DomainError);

impl std::fmt::Display for McpDomainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} [kind={}, retryable={}]",
            self.0.message,
            self.0.kind.as_str(),
            self.0.retryable
        )
    }
}

impl std::error::Error for McpDomainError {}

fn domain_error_to_anyhow(e: graphzero_engine::DomainError) -> anyhow::Error {
    anyhow::Error::new(McpDomainError(e))
}

/// Parse kind **and** explicit retryable from the display suffix written by
/// [`McpDomainError`] / legacy string carriers.
fn extract_domain_fields_from_display(
    msg: &str,
) -> Option<(graphzero_engine::DomainErrorKind, bool, String)> {
    let start = msg.rfind("[kind=")?;
    let rest = &msg[start + 6..];
    let comma = rest.find(',')?;
    let kind = match &rest[..comma] {
        "validation" => graphzero_engine::DomainErrorKind::Validation,
        "policy" => graphzero_engine::DomainErrorKind::Policy,
        "sandbox" => graphzero_engine::DomainErrorKind::Sandbox,
        "runtime" => graphzero_engine::DomainErrorKind::Runtime,
        "substrate" => graphzero_engine::DomainErrorKind::Substrate,
        "busy" => graphzero_engine::DomainErrorKind::Busy,
        "approval" => graphzero_engine::DomainErrorKind::Approval,
        "cancelled" => graphzero_engine::DomainErrorKind::Cancelled,
        "deadline_exceeded" => graphzero_engine::DomainErrorKind::DeadlineExceeded,
        "not_found" => graphzero_engine::DomainErrorKind::NotFound,
        "unauthorized" => graphzero_engine::DomainErrorKind::Unauthorized,
        _ => return None,
    };
    let retry_key = "retryable=";
    let rpos = rest.find(retry_key)?;
    let rrest = &rest[rpos + retry_key.len()..];
    let rend = rrest.find(']').unwrap_or(rrest.len());
    let retryable = match rrest[..rend].trim() {
        "true" => true,
        "false" => false,
        _ => return None,
    };
    let message = msg[..start].trim_end().to_string();
    Some((kind, retryable, message))
}

fn domain_dispatch_mcp(op: &str, args: &Value) -> Result<Value> {
    let result = domain_dispatch_result(op, args).map_err(domain_error_to_anyhow)?;
    // remember historically used structured MCP content; others use text.
    if op == "remember" {
        return Ok(mcp_json_result(result.value));
    }
    // Reserve: surface the inner service JSON (stable machine contract), not envelope.
    if op == "reserve" || op.starts_with("semantic_reserve_") {
        let inner = result
            .value
            .get("result")
            .cloned()
            .unwrap_or(result.value.clone());
        let action = result
            .value
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let text = if action == "release" {
            serde_json::to_string(&inner)?
        } else {
            serde_json::to_string_pretty(&inner)?
        };
        return Ok(mcp_text_result(text));
    }
    // Single transport envelope (graphzero-o2uq.5): serialize once.
    let text = crate::fastmcp_adapter::serialize_domain_result(&result);
    Ok(mcp_text_result(text))
}

fn call_tool(mode: ServerMode, name: &str, params: &Value) -> Result<Value> {
    let args = tool_arguments(params);
    match (mode, name) {
        // Domain ops → thin wrappers over shared typed dispatcher (graphzero-o2uq.2/5).
        (ServerMode::Mcp, "orient") => mcp_orient(args),
        (ServerMode::Mcp, "search") => mcp_search(args),
        (ServerMode::Mcp, "recall") => mcp_recall(args),
        (ServerMode::Mcp, "blast") => mcp_blast_intent(args),
        (ServerMode::Mcp, "expand") => mcp_expand(args),
        (ServerMode::Mcp, "remember") => mcp_remember(args),
        (ServerMode::Mcp, "verify") => mcp_verify(args),
        // Snap keeps export_path MCP extension above the domain op.
        (ServerMode::Mcp, "snap") => mcp_snap(args),
        // Index CLI JSON shape preserved via commands::index.
        (ServerMode::Mcp, "index") => mcp_index(args),
        // Reserve: domain dispatcher owns semantics (legacy aliases too).
        (ServerMode::Mcp, "reserve") => mcp_reserve(args),
        (ServerMode::Mcp, "stats" | "compact" | "graphzero_publish") => {
            anyhow::bail!(
                "tool {name} is not in the lean MCP catalog; use CLI or index/blast/reserve/verify"
            )
        }
        (ServerMode::Mcp, _) if name.starts_with("semantic_reserve_") => {
            mcp_semantic_reserve(name, args)
        }
        (ServerMode::Mcp, other) if graphzero_engine::SURFACE_NAMES.contains(&other) => {
            mcp_query_surface_tool(other, args)
        }
        (ServerMode::Mcp, other) => {
            // Stable typed error for unknown lean-catalog tools (graphzero-o2uq.5).
            let err = crate::fastmcp_adapter::resolve_fastmcp_tool(other)
                .err()
                .unwrap_or_else(|| {
                    graphzero_engine::DomainError::new(
                        graphzero_engine::DomainErrorKind::NotFound,
                        format!("unknown tool {other}"),
                    )
                    .with_op(other)
                });
            Err(domain_error_to_anyhow(err))
        }
    }
}

/// FastMCP dispatch: calls the existing tool logic and returns the text payload.
/// Used by fastmcp_mode.rs to avoid re-implementing tool logic.
pub fn mcp_dispatch(name: &str, args: &Value) -> Result<String> {
    let result = call_tool(ServerMode::Mcp, name, args)?;
    Ok(mcp_result_text(&result)?.to_string())
}
