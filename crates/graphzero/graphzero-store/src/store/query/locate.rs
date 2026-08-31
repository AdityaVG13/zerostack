//! Ultra-compact file/symbol locate: 1-token shell (`g:<id>`), lossless expand. Stable loc ids are
//! assigned at snapshot open from sorted path/symbol tables. Expanding `g:N` resolves to the same
//! bytes as the canonical `z://blob/...` ref.

use std::collections::HashMap;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::store::format::SpanEntry;
use crate::store::refs::blob_span_ref;
use crate::store::symbol_table::SymbolTable;

use super::budget::{persist_query_json, tokens_for_str};
use super::snapshot::Snapshot;
use super::spans::span_range;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocateHit {
    pub loc_ref: String,
    pub canonical_ref: String,
    pub path: Option<String>,
    pub symbol: Option<String>,
    pub rank: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocateCapsule {
    pub schema_version: u32,
    pub query: String,
    pub kind: String,
    pub best: LocateHit,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<LocateHit>,
    pub snapshot_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail_ref: Option<String>,
}

#[derive(Clone, Debug)]
pub struct LocateEntry {
    pub loc_id: u32,
    pub canonical_ref: String,
    pub path: Option<String>,
    pub symbol: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct LocateIndex {
    pub path_to_loc: HashMap<String, u32>,
    pub symbol_to_loc: HashMap<String, u32>,
    pub by_id: HashMap<u32, LocateEntry>,
}

impl LocateIndex {
    pub fn build(snapshot: &Snapshot) -> Result<Self> {
        let view = snapshot.global_view()?;
        let table = SymbolTable::from_view(&view)?;
        let spans = view.spans()?;
        let blob_hashes = view.coverage()?.blob_hashes;

        let mut path_to_loc = HashMap::new();
        let mut symbol_to_loc = HashMap::new();
        let mut by_id = HashMap::new();

        let mut paths: Vec<_> = snapshot.path_records().collect();
        paths.sort_by_key(|(_, r)| r.path.as_str());

        let mut next_id = 1u32;
        for (hash, rec) in paths.iter() {
            let loc_id = next_id;
            next_id = next_id.saturating_add(1);
            let hash_hex = hash.to_hex();
            let canonical_ref = format!("z://blob/{hash_hex}");
            path_to_loc.insert(rec.path.clone(), loc_id);
            by_id.insert(
                loc_id,
                LocateEntry {
                    loc_id,
                    canonical_ref,
                    path: Some(rec.path.clone()),
                    symbol: None,
                },
            );
        }

        let mut symbols: Vec<u32> = (0..table.len() as u32).collect();
        symbols.sort_by_key(|&id| table.name(id).unwrap_or("").to_string());

        for id in symbols {
            let Some(name) = table.name(id) else {
                continue;
            };
            let loc_id = next_id;
            next_id = next_id.saturating_add(1);
            let canonical_ref = symbol_canonical_ref(id, &spans, blob_hashes, &table)?
                .unwrap_or_else(|| format!("node/{name}"));
            let path = hex_for_symbol_def(id, &spans, blob_hashes)?
                .and_then(|h| snapshot.path_for_blob(&h).map(|r| r.path.clone()));
            symbol_to_loc.insert(name.to_string(), loc_id);
            by_id.insert(
                loc_id,
                LocateEntry {
                    loc_id,
                    canonical_ref,
                    path,
                    symbol: Some(name.to_string()),
                },
            );
        }

        Ok(Self {
            path_to_loc,
            symbol_to_loc,
            by_id,
        })
    }

    pub fn entry(&self, loc_id: u32) -> Option<&LocateEntry> {
        self.by_id.get(&loc_id)
    }

    pub fn loc_ref(loc_id: u32) -> String {
        format!("g:{loc_id}")
    }
}

fn hex_for_symbol_def(
    sym_id: u32,
    spans: &[SpanEntry],
    blob_hashes: &[[u8; 32]],
) -> Result<Option<String>> {
    let Some(span) = span_range(spans, sym_id).first() else {
        return Ok(None);
    };
    Ok(Some(crate::hex_blob_hash(blob_hashes, span.blob_idx)?))
}

fn symbol_canonical_ref(
    sym_id: u32,
    spans: &[SpanEntry],
    blob_hashes: &[[u8; 32]],
    table: &SymbolTable,
) -> Result<Option<String>> {
    let Some(span) = span_range(spans, sym_id).first() else {
        return Ok(None);
    };
    let hash_hex = crate::hex_blob_hash(blob_hashes, span.blob_idx)?;
    let Some(name) = table.name(sym_id) else {
        return Ok(None);
    };
    let evidence_ref = blob_span_ref(&hash_hex, span.start, span.end);
    if evidence_ref.is_empty() {
        return Ok(Some(format!("node/{name}")));
    }
    Ok(Some(evidence_ref))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocateKind {
    Auto,
    Path,
    Symbol,
}

impl LocateKind {
    pub fn parse(s: &str) -> Self {
        match s {
            "path" => Self::Path,
            "symbol" => Self::Symbol,
            _ => Self::Auto,
        }
    }

    fn as_label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Path => "path",
            Self::Symbol => "symbol",
        }
    }
}

/// Resolve a locate query. Returns a capsule; use [`locate_shell`] for the 1-token wire form.
pub fn locate(snapshot: &Snapshot, query: &str, kind: LocateKind) -> Result<LocateCapsule> {
    let query = query.trim();
    if query.is_empty() {
        bail!("empty locate query");
    }
    let index = snapshot.locate_index()?;

    let (best, candidates, locate_kind) = match kind {
        LocateKind::Path => path_locate(index, snapshot, query)?,
        LocateKind::Symbol => symbol_locate(index, query)?,
        LocateKind::Auto => {
            if query.contains('/') || query.ends_with(".rs") || query.ends_with(".ts") {
                path_locate(index, snapshot, query)?
            } else if let Ok(r) = symbol_locate(index, query) {
                r
            } else {
                path_locate(index, snapshot, query)?
            }
        }
    };

    let mut capsule = LocateCapsule {
        schema_version: 1,
        query: query.to_string(),
        kind: locate_kind.as_label().to_string(),
        best: best.clone(),
        candidates: candidates.clone(),
        snapshot_id: snapshot.entry.snapshot_id,
        detail_ref: None,
    };

    if candidates.len() > 1 {
        let detail_json = serde_json::to_string(&capsule)?;
        if let Ok(id) = persist_query_json(&snapshot.store_root, &detail_json) {
            capsule.detail_ref = Some(format!("query/{id}"));
        }
    }

    Ok(capsule)
}

fn path_locate(
    index: &LocateIndex,
    snapshot: &Snapshot,
    query: &str,
) -> Result<(LocateHit, Vec<LocateHit>, LocateKind)> {
    if let Some(&loc_id) = index.path_to_loc.get(query) {
        let entry = index.entry(loc_id).expect("path loc");
        let hit = entry_to_hit(entry, 0);
        return Ok((hit.clone(), vec![hit], LocateKind::Path));
    }

    let mut matches: Vec<(usize, String, String)> = Vec::new();
    for (hash, rec) in snapshot.path_records() {
        if rec.path.ends_with(query) || rec.path.contains(query) {
            matches.push((rec.path.len(), rec.path.clone(), hash.to_hex()));
        }
    }
    matches.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    if matches.is_empty() {
        bail!("path not found: {query}");
    }

    let candidates: Vec<LocateHit> = matches
        .iter()
        .enumerate()
        .map(|(i, (_, path, hash_hex))| {
            let loc_id = index.path_to_loc.get(path).copied();
            let loc_ref = loc_id
                .map(LocateIndex::loc_ref)
                .unwrap_or_else(|| format!("z://blob/{hash_hex}"));
            let canonical_ref = format!("z://blob/{hash_hex}");
            LocateHit {
                loc_ref,
                canonical_ref,
                path: Some(path.clone()),
                symbol: None,
                rank: i,
            }
        })
        .collect();
    let best = candidates[0].clone();
    Ok((best, candidates, LocateKind::Path))
}

fn symbol_locate(
    index: &LocateIndex,
    query: &str,
) -> Result<(LocateHit, Vec<LocateHit>, LocateKind)> {
    let Some(&loc_id) = index.symbol_to_loc.get(query) else {
        bail!("symbol not found: {query}");
    };
    let entry = index.entry(loc_id).expect("symbol loc");
    let hit = entry_to_hit(entry, 0);
    Ok((hit.clone(), vec![hit], LocateKind::Symbol))
}

fn entry_to_hit(entry: &LocateEntry, rank: usize) -> LocateHit {
    LocateHit {
        loc_ref: LocateIndex::loc_ref(entry.loc_id),
        canonical_ref: entry.canonical_ref.clone(),
        path: entry.path.clone(),
        symbol: entry.symbol.clone(),
        rank,
    }
}

/// 1-token wire form for a symbol name when present in the locate index.
pub fn locate_shell_for_name(snapshot: &Snapshot, name: &str) -> Option<String> {
    // Fast path: the published symbol table is built from a BTreeMap, so its entries are already in
    // lexicographic name order — the same order LocateIndex::build uses when assigning symbol loc ids
    // after path records.
    if let Some(loc_id) = locate_loc_id_for_name(snapshot, name) {
        return Some(LocateIndex::loc_ref(loc_id));
    }
    let index = snapshot.locate_index().ok()?;
    index
        .symbol_to_loc
        .get(name)
        .map(|&id| LocateIndex::loc_ref(id))
}

/// Loc id for an exact symbol name via binary search over the name-sorted
/// symbol table. Returns `None` on any structural surprise so the caller can
/// fall back to the full index rather than mint a wrong `g:` ref.
fn locate_loc_id_for_name(snapshot: &Snapshot, name: &str) -> Option<u32> {
    if !snapshot.locate_fast_path_ok() {
        return None;
    }
    let view = snapshot.global_view().ok()?;
    let table = SymbolTable::from_view(&view).ok()?;
    let id = table.get(name)?;
    let path_count = u32::try_from(snapshot.path_record_count()).ok()?;
    // loc ids are 1-based: paths occupy 1..=path_count, symbols follow in
    // name order, so a dense name-sorted table puts `name` at path_count+id+1.
    path_count.checked_add(id)?.checked_add(1)
}

/// Compact query spill wire form: `q:<id>`.
pub fn query_shell(id: &str) -> String {
    format!("q:{id}")
}

/// 1-token wire form: `g:<loc_id>` only.
pub fn locate_shell(capsule: &LocateCapsule) -> String {
    capsule.best.loc_ref.clone()
}

pub fn locate_shell_tokens(capsule: &LocateCapsule) -> usize {
    tokens_for_str(&locate_shell(capsule))
}

/// Resolve `g:<id>` to its canonical bare ref.
pub fn canonical_ref_for_loc(snapshot: &Snapshot, loc_id: u32) -> Result<String> {
    let index = snapshot.locate_index()?;
    let entry = index
        .entry(loc_id)
        .ok_or_else(|| anyhow::anyhow!("unknown loc id {loc_id}"))?;
    Ok(entry.canonical_ref.clone())
}
