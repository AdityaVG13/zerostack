//! Index command core.

use std::path::Path;

use anyhow::Result;
use serde_json::{Value, json};

use super::paths::{canonical_repo, store_root};

#[derive(Clone, Debug, PartialEq)]
pub struct IndexOutcome {
    pub snapshot: u64,
    pub shards: usize,
    pub store: String,
    /// Present when `GRAPHZERO_INDEX_PHASE_TIMING` is set (from domain `phases`).
    pub phases: Option<Value>,
}

pub fn run(repo: &Path) -> Result<IndexOutcome> {
    let repo = canonical_repo(repo)?;
    let root = store_root(&repo);
    let ctx = graphzero_engine::EngineContext::for_paths(
        repo.clone(),
        root.clone(),
        graphzero_engine::AdapterKind::Cli,
    );
    let args = json!({ "path": repo.display().to_string() });
    let result = graphzero_engine::dispatch(&ctx, "index", &args)
        .map_err(|e| anyhow::anyhow!("{}", e.message))?;
    Ok(IndexOutcome {
        snapshot: result
            .value
            .get("snapshot")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        shards: result
            .value
            .get("shards")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        store: result
            .value
            .get("store")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| root.display().to_string()),
        phases: result.value.get("phases").cloned(),
    })
}

pub fn to_json(out: &IndexOutcome) -> String {
    let mut value = json!({
        "snapshot": out.snapshot,
        "shards": out.shards,
        "store": out.store,
    });
    if let Some(phases) = &out.phases {
        if let Some(obj) = value.as_object_mut() {
            obj.insert("phases".into(), phases.clone());
        }
    }
    value.to_string()
}
