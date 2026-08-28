use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use graphzero_store::ContentHash;
use graphzero_store::store::csr::edge_kind;
use graphzero_store::store::indexer::{EdgeRecord, IndexData};
use scip::types as scip_types;

use crate::decode::decode_scip_bytes;
use crate::types::{ScipDecoded, TierBEdge, TierBResolution, TierBSource};

pub const TIER_B_CONFIDENCE: f64 = 1.0;
pub const TIER_B_UNRESOLVED_CONFIDENCE: f64 = 0.75;

/// Plan produced by decoding SCIP prior to merging into a snapshot.
#[derive(Clone, Debug)]
pub struct ScipIngestPlan {
    pub summary: ScipDecoded,
    pub edges: Vec<TierBEdge>,
    /// Blob hashes that should receive tier-B coverage bits.
    pub touched_blobs: Vec<ContentHash>,
}

pub fn scip_facts_from_bytes(
    bytes: &[u8],
    blob_by_path: &BTreeMap<String, (ContentHash, Vec<u8>)>,
) -> Result<ScipIngestPlan> {
    let (index, summary) = decode_scip_bytes(bytes)?;
    Ok(scip_facts_from_decoded(index, summary, blob_by_path))
}

/// Build a Tier-B plan from an already-decoded SCIP index and a path→blob map.
pub fn scip_facts_from_decoded(
    index: scip_types::Index,
    summary: crate::types::ScipDecoded,
    blob_by_path: &BTreeMap<String, (ContentHash, Vec<u8>)>,
) -> ScipIngestPlan {
    let mut edges = Vec::new();
    let mut touched = BTreeMap::<ContentHash, ()>::new();

    for doc in &index.documents {
        let Some((blob, content)) = blob_by_path.get(&doc.relative_path) else {
            continue;
        };
        touched.insert(*blob, ());
        let display_of: BTreeMap<&str, &str> = doc
            .symbols
            .iter()
            .map(|s| (s.symbol.as_str(), s.display_name.as_str()))
            .collect();
        collect_symbol_edges(doc, *blob, content, &display_of, &mut edges);
        collect_occurrence_edges(doc, *blob, content, &display_of, &mut edges);
    }

    ScipIngestPlan {
        summary,
        edges,
        touched_blobs: touched.keys().copied().collect(),
    }
}

/// Load blob bytes for selected relative paths from the store CAS using the
/// path table already present on collected [`IndexData`].
///
/// Avoids a second worktree walk + re-hash after `indexer::collect` (713dg).
/// Only paths requested are read from CAS; missing CAS bodies are skipped.
pub fn blob_map_from_index(
    store_root: &Path,
    data: &IndexData,
    paths: impl IntoIterator<Item = impl AsRef<str>>,
) -> Result<BTreeMap<String, (ContentHash, Vec<u8>)>> {
    use graphzero_store::BlobStore;

    let path_to_hash: BTreeMap<&str, ContentHash> = data
        .blobs
        .iter()
        .map(|(hash, meta)| (meta.path.as_str(), *hash))
        .collect();
    let store = BlobStore::open(store_root)?;
    let mut out = BTreeMap::new();
    for path in paths {
        let path = path.as_ref();
        if out.contains_key(path) {
            continue;
        }
        let Some(hash) = path_to_hash.get(path).copied() else {
            continue;
        };
        let Some(content) = store.get(&hash)? else {
            continue;
        };
        out.insert(path.to_string(), (hash, content));
    }
    Ok(out)
}

fn collect_symbol_edges(
    doc: &scip_types::Document,
    blob: ContentHash,
    content: &[u8],
    display_of: &BTreeMap<&str, &str>,
    edges: &mut Vec<TierBEdge>,
) {
    for sym in &doc.symbols {
        let src_name = sym.display_name.as_str();
        for rel in &sym.relationships {
            let Some(dst_name) = display_of.get(rel.symbol.as_str()).copied() else {
                continue;
            };
            let Some((start, end)) = span_for_name(content, src_name) else {
                continue;
            };
            edges.push(TierBEdge {
                src: src_name.to_string(),
                dst: dst_name.to_string(),
                kind: edge_kind::REFS,
                confidence: TIER_B_CONFIDENCE,
                resolution: TierBResolution::SymbolWitness,
                source: TierBSource::Scip,
                blob,
                start,
                end,
            });
        }
    }
}

fn collect_occurrence_edges(
    doc: &scip_types::Document,
    blob: ContentHash,
    content: &[u8],
    display_of: &BTreeMap<&str, &str>,
    edges: &mut Vec<TierBEdge>,
) {
    for occ in &doc.occurrences {
        let Some((start, end)) = occurrence_byte_span(content, occ, display_of) else {
            continue;
        };
        let (name, confidence, resolution) = match display_of.get(occ.symbol.as_str()).copied() {
            Some(name) => (name, TIER_B_CONFIDENCE, TierBResolution::SymbolWitness),
            None => (
                occ.symbol.as_str(),
                TIER_B_UNRESOLVED_CONFIDENCE,
                TierBResolution::UnresolvedOccurrence,
            ),
        };
        edges.push(TierBEdge {
            src: doc.relative_path.clone(),
            dst: name.to_string(),
            kind: edge_kind::REFS,
            confidence,
            resolution,
            source: TierBSource::Scip,
            blob,
            start,
            end,
        });
    }
}

fn occurrence_byte_span(
    content: &[u8],
    occ: &scip_types::Occurrence,
    display_of: &BTreeMap<&str, &str>,
) -> Option<(u32, u32)> {
    if occ.range.len() < 4 {
        return None;
    }
    let name = display_of
        .get(occ.symbol.as_str())
        .copied()
        .unwrap_or(occ.symbol.as_str());
    range_to_byte_span(content, &occ.range).or_else(|| span_for_name(content, name))
}

fn range_to_byte_span(content: &[u8], range: &[i32]) -> Option<(u32, u32)> {
    if range.len() < 4 {
        return None;
    }
    let s = byte_offset(content, range[0], range[1]);
    let e = byte_offset(content, range[2], range[3]);
    if s < e { Some((s, e)) } else { None }
}

fn span_for_name(content: &[u8], name: &str) -> Option<(u32, u32)> {
    let needle = name.as_bytes();
    if needle.is_empty() {
        return None;
    }
    content
        .windows(needle.len())
        .enumerate()
        .find(|(pos, window)| {
            *window == needle
                && (*pos == 0 || !is_identifier_byte(content[*pos - 1]))
                && (*pos + needle.len() == content.len()
                    || !is_identifier_byte(content[*pos + needle.len()]))
        })
        .map(|(pos, _)| (pos as u32, (pos + needle.len()) as u32))
}

fn is_identifier_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric() || !byte.is_ascii()
}

fn byte_offset(content: &[u8], line: i32, character: i32) -> u32 {
    let target_line = line.max(0) as usize;
    let target_col = character.max(0) as usize;
    let mut line_start = 0usize;

    for _ in 0..target_line {
        let Some(newline_offset) = content[line_start..].iter().position(|&b| b == b'\n') else {
            return content.len().min(u32::MAX as usize) as u32;
        };
        line_start += newline_offset + 1;
    }

    let line_end = content[line_start..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(content.len(), |newline_offset| line_start + newline_offset);
    (line_start + target_col)
        .min(line_end)
        .min(u32::MAX as usize) as u32
}

/// Merge tier-B edges into existing index data (FR-005, FR-006, FR-007).
pub fn apply_scip_to_index(data: &mut IndexData, plan: &ScipIngestPlan) {
    mark_tier_b_blobs(data, &plan.touched_blobs);
    data.edges = merge_edges_by_triple(&data.edges, &plan.edges);
}

fn mark_tier_b_blobs(data: &mut IndexData, touched: &[ContentHash]) {
    for hash in touched {
        if let Some(meta) = data.blobs.get_mut(hash) {
            meta.tier_bits |= 0b010;
        }
    }
}

fn merge_edges_by_triple(existing: &[EdgeRecord], incoming: &[TierBEdge]) -> Vec<EdgeRecord> {
    let mut by_triple: BTreeMap<(String, String, u8), EdgeRecord> = BTreeMap::new();
    for e in existing {
        by_triple.insert((e.src.clone(), e.dst.clone(), e.kind), e.clone());
    }
    for e in incoming {
        let rec = EdgeRecord {
            src: e.src.clone(),
            dst: e.dst.clone(),
            kind: e.kind,
            confidence: e.resolution.persisted_confidence(),
            blob: e.blob,
            start: e.start,
            end: e.end,
        };
        let key = (rec.src.clone(), rec.dst.clone(), rec.kind);
        match by_triple.entry(key) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(rec);
            }
            std::collections::btree_map::Entry::Occupied(mut entry)
                if rec.confidence > entry.get().confidence =>
            {
                entry.insert(rec);
            }
            std::collections::btree_map::Entry::Occupied(_) => {}
        }
    }
    by_triple.into_values().collect()
}

pub fn load_blob_map(repo_root: &Path) -> Result<BTreeMap<String, (ContentHash, Vec<u8>)>> {
    let mut out = BTreeMap::new();
    visit_rust_sources(repo_root, repo_root, &mut out)?;
    Ok(out)
}

fn visit_rust_sources(
    repo_root: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, (ContentHash, Vec<u8>)>,
) -> Result<()> {
    if is_ignored_source_dir(dir) {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            visit_rust_sources(repo_root, &path, out)?;
        } else if is_rust_source(&path) {
            insert_blob_for_path(repo_root, &path, out)?;
        }
    }
    Ok(())
}

fn is_ignored_source_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|s| s.to_str()),
        Some(".git" | ".graphzero" | "target")
    )
}

fn is_rust_source(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()) == Some("rs")
}

fn insert_blob_for_path(
    repo_root: &Path,
    path: &Path,
    out: &mut BTreeMap<String, (ContentHash, Vec<u8>)>,
) -> Result<()> {
    let content = std::fs::read(path)?;
    let rel = path
        .strip_prefix(repo_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let hash = ContentHash::of(&content);
    out.insert(rel, (hash, content));
    Ok(())
}
