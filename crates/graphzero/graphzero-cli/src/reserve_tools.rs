//! P5.2 semantic reservation CLI/MCP helpers — thin facades over domain dispatch.
//!
//! All transport-neutral reserve semantics live in
//! `graphzero_engine::dispatch(..., "reserve", ...)` (graphzero-o2uq.2).

use std::path::Path;

use anyhow::{Context, Result};
use graphzero_reserve::IntentOperation;
use serde_json::{Value, json};

fn domain_reserve(store_root: &Path, repo_root: &Path, args: Value) -> Result<Value> {
    let ctx = graphzero_engine::EngineContext::for_paths(
        repo_root.to_path_buf(),
        store_root.to_path_buf(),
        graphzero_engine::AdapterKind::Cli,
    );
    let result = graphzero_engine::dispatch(&ctx, "reserve", &args)
        .map_err(|e| anyhow::anyhow!("{}", e.message))?;
    // Prefer inner result object (stable machine contract); fall back to envelope.
    Ok(result.value.get("result").cloned().unwrap_or(result.value))
}

pub fn parse_intent_ops(value: &Value) -> Result<Vec<IntentOperation>> {
    let arr = value
        .as_array()
        .context("intent_ops must be a JSON array")?;
    let mut out = Vec::new();
    for item in arr {
        let kind = item
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("change_signature")
            .to_string();
        let target_symbol = item
            .get("target_symbol")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let intent_text = item
            .get("intent_text")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        out.push(IntentOperation {
            kind,
            target_symbol,
            intent_text,
        });
    }
    Ok(out)
}

pub fn intent_ops_from_intent_file(path: &Path) -> Result<Vec<IntentOperation>> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let v: Value = serde_json::from_str(&raw).context("intent json")?;
    if let Some(ops) = v.get("intent_ops") {
        return parse_intent_ops(ops);
    }
    parse_intent_ops(&v)
}

pub fn run_declare(
    store_root: &Path,
    repo_root: &Path,
    agent_id: &str,
    intent_ops: Vec<IntentOperation>,
    ttl_seconds: u64,
) -> Result<String> {
    let args = json!({
        "action": "declare",
        "agent_id": agent_id,
        "intent_ops": intent_ops,
        "ttl_seconds": ttl_seconds,
    });
    let value = domain_reserve(store_root, repo_root, args)?;
    Ok(serde_json::to_string_pretty(&value)?)
}

pub fn run_check(
    store_root: &Path,
    repo_root: &Path,
    agent_id: &str,
    intent_ops: &[IntentOperation],
    acquire: bool,
    ttl_seconds: Option<u64>,
) -> Result<String> {
    let mut args = json!({
        "action": "check",
        "agent_id": agent_id,
        "intent_ops": intent_ops,
        "acquire": acquire,
    });
    if let Some(ttl) = ttl_seconds {
        args.as_object_mut()
            .unwrap()
            .insert("ttl_seconds".into(), json!(ttl));
    }
    let value = domain_reserve(store_root, repo_root, args)?;
    Ok(serde_json::to_string_pretty(&value)?)
}

pub fn run_release(
    store_root: &Path,
    repo_root: &Path,
    agent_id: &str,
    reservation_id: &str,
) -> Result<String> {
    let args = json!({
        "action": "release",
        "agent_id": agent_id,
        "reservation_id": reservation_id,
    });
    let value = domain_reserve(store_root, repo_root, args)?;
    Ok(serde_json::to_string(&value)?)
}

pub fn run_query(store_root: &Path, repo_root: &Path) -> Result<String> {
    let args = json!({ "action": "list" });
    let value = domain_reserve(store_root, repo_root, args)?;
    Ok(serde_json::to_string_pretty(&value)?)
}

/// Legacy MCP tool name entry: routes through domain reserve (no local semantics).
pub fn reserve_from_mcp(
    name: &str,
    args: &Value,
    store_root: &Path,
    repo_root: &Path,
) -> Result<String> {
    let action = match name {
        "semantic_reserve_declare" | "declare" => "declare",
        "semantic_reserve_check" | "check" => "check",
        "semantic_reserve_release" | "release" => "release",
        "semantic_reserve_query" | "list" | "query" => "list",
        _ => anyhow::bail!("unknown reserve tool {name}"),
    };
    let mut routed = args.clone();
    if let Some(obj) = routed.as_object_mut() {
        obj.insert("action".into(), json!(action));
    } else {
        routed = json!({ "action": action });
    }
    let value = domain_reserve(store_root, repo_root, routed)?;
    // Pretty for declare/check/list; compact release is already object.
    if action == "release" {
        Ok(serde_json::to_string(&value)?)
    } else {
        Ok(serde_json::to_string_pretty(&value)?)
    }
}
