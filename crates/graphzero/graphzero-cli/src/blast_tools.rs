//! P4.1 blast-radius CLI/MCP shared handler.

use std::path::Path;

use anyhow::{Context, Result};
use graphzero_engine::blast::{
    PlannedEdit, SpeculativeBlastRequest, impact_before_edit, parse_intent, retrieval_neighborhood,
};
use graphzero_store::Snapshot;
use serde_json::{Value, json};

pub fn run_blast(
    store_root: &Path,
    repo_root: &Path,
    intent: &str,
    budget: usize,
    depth: u32,
) -> Result<String> {
    // Shared typed domain dispatcher (graphzero-o2uq.2).
    let args = json!({
        "intent": intent,
        "budget": budget,
        "depth": depth,
    });
    let ctx = graphzero_engine::EngineContext::for_paths(
        repo_root.to_path_buf(),
        store_root.to_path_buf(),
        graphzero_engine::AdapterKind::Cli,
    );
    let result = graphzero_engine::dispatch(&ctx, "blast", &args)
        .map_err(|e| anyhow::anyhow!("{e:?}: {}", e.message))?;
    match result.value {
        Value::String(s) => Ok(s),
        other => serde_json::to_string(&other).context("serialize blast domain result"),
    }
}

pub fn run_neighborhood(
    store_root: &Path,
    repo_root: &Path,
    seeds: &[String],
    hops: u32,
    budget: usize,
) -> Result<String> {
    let snapshot = Snapshot::open_cached(store_root, Some(repo_root))?;
    let neighborhood = retrieval_neighborhood(&snapshot, seeds, hops, budget)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    serde_json::to_string(&neighborhood).context("serialize retrieval neighborhood")
}

pub fn run_speculative_blast(
    store_root: &Path,
    repo_root: &Path,
    intent: &str,
    budget: usize,
    world_ref: &str,
    world_envelope: Option<&str>,
    planned_edits: &[String],
    focus_symbols: &[String],
) -> Result<String> {
    // Validate/bind the envelope before any snapshot graph work so unknown
    // majors and mismatched world refs fail loudly (fszero-riz7 contract).
    let world_ref = graphzero_engine::bind_world_envelope(world_ref, world_envelope)
        .map_err(|e| anyhow::anyhow!("{e:#}"))?;
    let snapshot = Snapshot::open_cached(store_root, Some(repo_root))?;
    let parsed = parse_intent(intent);
    let mut focus = focus_symbols.to_vec();
    if focus.is_empty() {
        let target = parsed.target_symbol.context(
            "--world-ref/--world-envelope requires --focus when --intent has no parseable target symbol",
        )?;
        focus.push(target);
    }
    let request = SpeculativeBlastRequest {
        world_ref,
        // Already bound above; keep the request envelope-free so the binder
        // runs exactly once per call.
        world_envelope: None,
        focus_symbols: focus,
        planned_edits: planned_edits
            .iter()
            .map(|raw| parse_planned_edit(raw))
            .collect::<Result<Vec<_>>>()?,
    };
    let report =
        impact_before_edit(&snapshot, request, budget).map_err(|e| anyhow::anyhow!("{e}"))?;
    serde_json::to_string(&report).context("serialize speculative blast report")
}

fn parse_planned_edit(raw: &str) -> Result<PlannedEdit> {
    let (path, rest) = raw
        .split_once("::")
        .context("planned edit must be path::before=>after")?;
    let (before, after) = rest
        .split_once("=>")
        .context("planned edit must be path::before=>after")?;
    Ok(PlannedEdit {
        path: path.to_string(),
        before: before.to_string(),
        after: after.to_string(),
    })
}
