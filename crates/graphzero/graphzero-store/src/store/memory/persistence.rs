//! Persist, load, export, and import of durable memory facts.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::ContentHash;
use crate::store::path_safety::validate_safe_id;
use crate::store::query::Snapshot;

use super::anchors::{looks_like_path, resolve_anchors};
use super::types::{
    MAX_ANCHORS, MAX_FACT_TEXT, MEMORY_EXPORT_SCHEMA, MemoryExport, MemoryFact, MemoryIndex,
    MemoryKind, RememberInput,
};

pub fn mem_dir(store_root: &Path) -> PathBuf {
    store_root.join("mem")
}

pub fn mem_ref(id: &str) -> String {
    format!("gz://mem/{id}")
}

pub fn remember_fact(snapshot: &Snapshot, input: RememberInput) -> Result<MemoryFact> {
    let text = input.text.trim();
    if text.is_empty() {
        bail!("remember: text required");
    }
    if text.chars().count() > MAX_FACT_TEXT {
        bail!("remember: text exceeds {MAX_FACT_TEXT} chars");
    }
    if input.anchors.len() > MAX_ANCHORS {
        bail!("remember: too many anchors");
    }
    let anchors: Vec<String> = input
        .anchors
        .into_iter()
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
        .collect();
    let supersedes = normalize_supersedes(input.supersedes)?;
    let resolutions = resolve_anchors(snapshot, &anchors)?;
    let body_preview = serde_json::json!({
        "text": text,
        "anchors": anchors,
        "kind": input.kind,
        "supersedes": supersedes,
    });
    let id = ContentHash::of(serde_json::to_string(&body_preview)?.as_bytes()).to_hex();
    let ts = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_secs(),
        Err(_) => 0,
    };
    let fact = MemoryFact {
        id: id.clone(),
        ts,
        kind: MemoryKind::parse(input.kind.as_deref()),
        text: text.to_string(),
        anchors,
        anchor_resolutions: resolutions,
        supersedes,
    };
    persist_fact(&snapshot.store_root, &fact)?;
    Ok(fact)
}

pub fn persist_fact(store_root: &Path, fact: &MemoryFact) -> Result<()> {
    let dir = mem_dir(store_root);
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", fact.id));
    fs::write(path, serde_json::to_vec_pretty(fact)?)?;
    Ok(())
}

pub fn load_fact(store_root: &Path, id: &str) -> Result<MemoryFact> {
    validate_safe_id(id, "gz://mem")?;
    let path = mem_dir(store_root).join(format!("{id}.json"));
    let bytes = fs::read(path).context("mem fact not found")?;
    serde_json::from_slice(&bytes).context("parse mem fact")
}

pub fn export_memory(store_root: &Path, active_only: bool) -> Result<MemoryExport> {
    let idx = MemoryIndex::load(store_root)?;
    let facts = if active_only {
        idx.active_facts().into_iter().cloned().collect()
    } else {
        idx.all_facts().into_iter().cloned().collect()
    };
    Ok(MemoryExport {
        schema: MEMORY_EXPORT_SCHEMA,
        active_only,
        facts,
    })
}

pub fn import_memory(store_root: &Path, export: &MemoryExport) -> Result<usize> {
    if export.schema != MEMORY_EXPORT_SCHEMA {
        bail!("unsupported memory export schema {}", export.schema);
    }
    let mut written = 0;
    for fact in &export.facts {
        validate_safe_id(&fact.id, "gz://mem")?;
        persist_fact(store_root, fact)?;
        written += 1;
    }
    Ok(written)
}

pub(super) fn normalize_supersedes(ids: Vec<String>) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for raw in ids {
        let trimmed = raw.trim();
        let id = match trimmed.strip_prefix("gz://mem/") {
            Some(stripped) => stripped,
            None => trimmed,
        }
        .to_string();
        if id.is_empty() || !seen.insert(id.clone()) {
            continue;
        }
        validate_safe_id(&id, "gz://mem")?;
        out.push(id);
    }
    Ok(out)
}

pub(super) fn index_fact(
    by_path: &mut HashMap<String, Vec<String>>,
    by_symbol: &mut HashMap<String, Vec<String>>,
    fact: &MemoryFact,
) {
    for res in &fact.anchor_resolutions {
        if let Some(p) = &res.path {
            by_path.entry(p.clone()).or_default().push(fact.id.clone());
        }
        if let Some(s) = &res.symbol {
            by_symbol
                .entry(s.clone())
                .or_default()
                .push(fact.id.clone());
        }
    }
    for anchor in &fact.anchors {
        if looks_like_path(anchor) {
            by_path
                .entry(anchor.clone())
                .or_default()
                .push(fact.id.clone());
        } else {
            by_symbol
                .entry(anchor.clone())
                .or_default()
                .push(fact.id.clone());
        }
    }
}
