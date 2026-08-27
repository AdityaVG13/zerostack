//! P3.3 query surface MCP/CLI shared handlers.
//! Domain execution goes through `graphzero_engine::dispatch` (graphzero-o2uq.2).

use std::path::Path;

use anyhow::{Context, Result};
use graphzero_engine::query_surface::{QuerySurfaceRequest, SURFACE_NAMES};
use serde_json::Value;

pub(crate) fn normalize_agent_surface(surface: &str) -> String {
    surface.trim().to_lowercase()
}

pub fn run_query_surface(
    store_root: &Path,
    repo_root: &Path,
    req: QuerySurfaceRequest,
) -> Result<String> {
    let mut args = serde_json::Map::new();
    args.insert("surface".into(), Value::String(req.surface.clone()));
    if let Some(q) = &req.query {
        args.insert("query".into(), Value::String(q.clone()));
    }
    if let Some(n) = &req.name {
        args.insert("name".into(), Value::String(n.clone()));
    }
    if let Some(p) = &req.path {
        args.insert("path".into(), Value::String(p.clone()));
    }
    if let Some(b) = req.budget {
        args.insert("budget".into(), Value::from(b as u64));
    }
    if let Some(s) = &req.session {
        args.insert("session".into(), Value::String(s.clone()));
    }
    let ctx = graphzero_engine::EngineContext::for_paths(
        repo_root.to_path_buf(),
        store_root.to_path_buf(),
        graphzero_engine::AdapterKind::Cli,
    );
    let result = graphzero_engine::dispatch(&ctx, &req.surface, &Value::Object(args))
        .map_err(|e| anyhow::anyhow!("{}", e.message))?;
    match result.value {
        Value::String(s) => Ok(s),
        other => serde_json::to_string(&other).context("serialize domain result"),
    }
}

pub fn query_surface_from_json_args(surface: &str, args: &Value) -> QuerySurfaceRequest {
    QuerySurfaceRequest {
        surface: surface.to_string(),
        name: args
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        query: args
            .get("query")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| {
                args.get("symbol")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            }),
        path: args
            .get("path")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        budget: args
            .get("budget")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize),
        session: args
            .get("session")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        cursor: args
            .get("cursor")
            .or_else(|| args.get("next_cursor"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
    }
}

pub fn call_query_surface_mcp(
    name: &str,
    store_root: &Path,
    repo_root: &Path,
    args: &Value,
) -> Result<String> {
    if !SURFACE_NAMES.contains(&name) {
        anyhow::bail!(
            "{}",
            graphzero_engine::query_surface::unknown_surface_message(name)
        );
    }
    let req = query_surface_from_json_args(name, args);
    run_query_surface(store_root, repo_root, req).context("query surface execute")
}
