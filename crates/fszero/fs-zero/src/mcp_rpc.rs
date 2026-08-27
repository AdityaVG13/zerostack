//! Shared JSON-RPC helpers for FSZero MCP surfaces (stdio + HTTP).

use crate::core::{ExecutionPath, FSZeroSession, record_opt_in_visible_accounting};
use serde_json::{Value, json};

/// Re-export canonical tool `$schema` URL (owned by `core::operation_schemas`).
pub use crate::core::JSON_SCHEMA_2020_12;
pub const TOOLS_LIST_TTL_MS: u64 = 3_600_000;

pub fn success_response(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

pub fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// JSON-RPC error with structured `data` (kind / retryable / …).
pub fn error_response_with_data(id: Value, code: i64, message: &str, data: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message, "data": data}})
}

/// Sort tools by name for deterministic `tools/list` (SEP-2549).
pub fn tool_names_sorted(mut tools: Vec<Value>) -> Vec<Value> {
    tools.sort_by(|a, b| {
        let an = a.get("name").and_then(Value::as_str).unwrap_or("");
        let bn = b.get("name").and_then(Value::as_str).unwrap_or("");
        an.cmp(bn)
    });
    tools
}

pub fn tools_list_result(tools: Vec<Value>) -> Value {
    json!({"tools": tools, "ttlMs": TOOLS_LIST_TTL_MS, "cacheScope": "server"})
}

pub fn server_discover_result(
    protocol_version: &str,
    server_name: &str,
    server_description: &str,
    stateless: bool,
) -> Value {
    let mut caps = json!({"tools": {"listChanged": false}, "resources": {}});
    if stateless {
        caps["server"] = json!({"discover": {}});
    }
    json!({
        "protocolVersion": protocol_version, "capabilities": caps,
        "serverInfo": { "name": server_name, "version": env!("CARGO_PKG_VERSION"), "description": server_description, }
    })
}

pub fn resource_list_result(ttl_ms: u64) -> Value {
    json!({
        "resources": [
            {"uri": "fz://recovery/read", "name": "Last read payload",
             "description": "Expand the most recent fszero.read recovery key", "mimeType": "text/plain"},
            {"uri": "fz://recovery/search", "name": "Last search hits",
             "description": "Expand the most recent fszero.search recovery key", "mimeType": "text/plain"}],
        "ttlMs": ttl_ms, "cacheScope": "server"
    })
}

pub fn resource_read_result(sess: &mut FSZeroSession, uri: &str) -> Result<Value, String> {
    let key = uri
        .strip_prefix("fz://recovery/")
        .ok_or_else(|| format!("unknown resource uri: {uri}"))?;
    let bytes = sess
        .expand(key)
        .ok_or_else(|| format!("resource not found: {uri}"))?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(json!({ "contents": [{ "uri": uri, "mimeType": "text/plain", "text": text }] }))
}

pub fn ack_tool_result(
    sess: &mut FSZeroSession,
    ack: &str,
    ok: bool,
    detail: Option<&str>,
) -> Value {
    let refs = wire_refs(sess, ok, detail);
    // Errors carry the corrective detail on the wire: a bare ack like "X0"
    // gives an agent nothing to act on, which defeats the guidance-error
    // contract. Success keeps the one-token ack.
    let text = match detail {
        Some(detail) if !ok => {
            let clipped: String = detail.chars().take(220).collect();
            format!("{ack} {clipped}")
        }
        _ => ack.to_string(),
    };
    let mut structured =
        json!({ "ack": ack, "ok": ok, "refs": refs, "durable_degraded": sess.durable_degraded });
    if !ok {
        if let Some(detail) = detail {
            let clipped: String = detail.chars().take(500).collect();
            structured["error"] = json!(clipped);
        }
    }
    let result = json!({
        "content": [{"type": "text", "text": text}],
        "isError": !ok, "structuredContent": structured
    });
    if let Ok(encoded) = serde_json::to_string(&result) {
        crate::core::runtime_metrics::record_serialization(encoded.len());
    }
    record_opt_in_visible_accounting(
        ExecutionPath::Mcp,
        sess.store_db_path(),
        &result.to_string(),
        &text,
    );
    result
}

/// Attach observed accounting to non-plan tool responses.
pub fn attach_observed_tool_measurement(
    result: &mut Value,
    payloads: &[Vec<u8>],
    operations_total: u64,
    misses: u64,
) {
    use sha2::Digest as _;
    let mut seen = std::collections::HashSet::<[u8; 32]>::new();
    let mut bytes_materialized = 0u64;
    for payload in payloads {
        let digest: [u8; 32] = sha2::Sha256::digest(payload).into();
        if seen.insert(digest) {
            bytes_materialized = bytes_materialized.saturating_add(payload.len() as u64);
        }
    }
    let visible = result
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default();
    let covered = operations_total.saturating_sub(misses.min(operations_total));
    let status = if misses == 0 {
        "measured"
    } else if covered > 0 {
        "partial"
    } else {
        "unmeasured"
    };
    let telemetry = json!({"kind":"tool.execute","bytes_materialized":bytes_materialized,"raw_token_estimate":bytes_materialized.div_ceil(4),"visible_bytes":visible.len(),"visible_token_estimate":visible.len().div_ceil(4),"token_estimator":"estimator:utf8-bytes-div-4","measurement_coverage":{"status":status,"stage":"wire","operations_covered":covered,"operations_total":operations_total,"bytes":"observed","materialization_basis":"unique-content(payloads)","tokens":"estimated","misses":misses,"degraded_reasons":if misses>0{json!([{"kind":"recovery_expand_miss","count":misses}])}else{json!([])}}});
    if let Some(structured) = result
        .get_mut("structuredContent")
        .and_then(Value::as_object_mut)
    {
        structured.insert("telemetry".into(), telemetry);
    }
}

pub fn wire_refs(sess: &mut FSZeroSession, ok: bool, detail: Option<&str>) -> Vec<String> {
    let mut refs = detail.map(extract_refs).unwrap_or_default();
    if !ok && refs.is_empty() {
        if let Some(detail) = detail {
            refs.push(sess.recovery.put("mcp_error", detail.as_bytes()));
        }
    }
    refs
}

pub fn extract_refs(detail: &str) -> Vec<String> {
    detail
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | ')' | '(' | '[' | ']'))
        .filter_map(|part| {
            let start = part.find("fz://")?;
            let raw = &part[start..];
            let end = raw
                .find(|c: char| {
                    c.is_whitespace() || matches!(c, ',' | ';' | ')' | '(' | '[' | ']' | '"')
                })
                .unwrap_or(raw.len());
            Some(raw[..end].trim_end_matches('.').to_string())
        })
        .collect()
}

std::thread_local! {
    /// Startup-only root override for this process thread. `resolve_root` or
    /// `resolve_cli_root` consumes it before session workers are spawned.
    static EXPLICIT_ROOT: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
}

fn take_explicit_root() -> Option<std::path::PathBuf> {
    EXPLICIT_ROOT.with(|root| root.borrow_mut().take())
}

/// Parse `--root PATH` or `--root=PATH` from argv (any position).
pub fn parse_root_flag(args: &[String]) -> Option<std::path::PathBuf> {
    crate::packaging::flag_value(args, &["--root"]).map(std::path::PathBuf::from)
}

/// Shared CLI root: explicit `--root` / `--root=` → `FSZERO_ROOT` → cwd (or `.` if cwd fails).
/// An explicit flag always wins over the inherited environment so a parent's
/// `FSZERO_ROOT` cannot silently redirect a child launched with `--root`.
pub fn resolve_cli_root(args: &[String]) -> std::path::PathBuf {
    if let Some(p) = parse_root_flag(args) {
        // Discard any earlier startup override when argv supplies the source
        // of truth. Product entry points validate/canonicalize argv first.
        let _ = take_explicit_root();
        return p;
    }
    if let Some(p) = take_explicit_root() {
        return p;
    }
    if let Ok(v) = std::env::var("FSZERO_ROOT") {
        if !v.is_empty() {
            return std::path::PathBuf::from(v);
        }
    }
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

/// Install an explicit workspace root for the next session construction.
/// Canonicalizes when the path exists. Spawned children receive the returned
/// path through argv or `Command::env`; this function never mutates process env.
pub fn install_explicit_root(
    root: impl AsRef<std::path::Path>,
) -> Result<std::path::PathBuf, String> {
    let path = root.as_ref();
    if path.as_os_str().is_empty() {
        return Err("empty --root / FSZERO_ROOT".to_string());
    }
    let resolved = if path.exists() {
        std::fs::canonicalize(path).map_err(|e| format!("bad root {}: {e}", path.display()))?
    } else {
        // Allow not-yet-created fixtures only if parent is real; keep the
        // absolute form so children do not resolve relative to a foreign cwd.
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|e| format!("cwd unreadable while resolving root: {e}"))?
                .join(path)
        }
    };
    EXPLICIT_ROOT.with(|root| *root.borrow_mut() = Some(resolved.clone()));
    Ok(resolved)
}

pub fn resolve_root() -> Result<std::path::PathBuf, String> {
    if let Some(root) = take_explicit_root() {
        refuse_home_root(&root)?;
        return Ok(root);
    }
    // Precedence: inherited FSZERO_ROOT > dev sample workspace > process cwd.
    // MCP clients launch stdio servers with cwd = the project directory, so
    // defaulting to cwd makes one machine-wide registration work for every
    // repo without per-project env plumbing. The home directory is refused:
    // it means the client gave us no meaningful cwd, and indexing $HOME is
    // never what anyone wants.
    //
    // Multi-project: the durable store may live under ZEROSTACK_STORE_ROOT
    // (shared). That must NOT override the workspace root for FS ops.
    if let Ok(root) = std::env::var("FSZERO_ROOT") {
        let path = std::path::PathBuf::from(root);
        if path.as_os_str().is_empty() {
            return Err("FSZERO_ROOT is empty".to_string());
        }
        let resolved = if path.exists() {
            std::fs::canonicalize(&path)
                .map_err(|e| format!("FSZERO_ROOT canonicalize failed: {e}"))?
        } else {
            path
        };
        refuse_home_root(&resolved)?;
        return Ok(resolved);
    }
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        let sample = std::path::PathBuf::from(manifest).join("sample_workspace");
        if sample.is_dir() {
            refuse_home_root(&sample)?;
            return Ok(sample);
        }
    }
    let cwd = std::env::current_dir()
        .map_err(|e| format!("FSZERO_ROOT unset and cwd unreadable: {e}"))?;
    refuse_home_root(&cwd)?;
    Ok(cwd)
}

/// Uniform home refuse, applied on every resolve path (env, dev sample, cwd):
/// a bare `$HOME` root is never intended and hides a missing/incorrect root.
fn refuse_home_root(path: &std::path::Path) -> Result<(), String> {
    let Some(home) = dirs_home() else {
        return Ok(());
    };
    let same = path == home
        || std::fs::canonicalize(path)
            .ok()
            .zip(std::fs::canonicalize(&home).ok())
            .is_some_and(|(a, b)| a == b);
    if same {
        return Err("FSZERO_ROOT is required when launched from the home directory (refusing to index $HOME); pass --root /your/repo, which takes precedence over FSZERO_ROOT".to_string());
    }
    Ok(())
}

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}
