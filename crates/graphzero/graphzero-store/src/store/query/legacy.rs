//! Symbol query matching and pending WAL merge helpers.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};

use super::super::csr::CsrAdjacency;
use super::super::delta_log::read_all_segments;
use super::super::format::SpanEntry;
use super::super::indexer::paths_file_name;
use super::super::manifest::SnapshotEntry;
use super::super::refs::blob_span_ref;
use super::super::symbol_table::SymbolTable;
use super::freshness::render_def_staleness;
use super::snapshot::Snapshot;
use super::spans::span_range;
use super::types::{CapsuleDef, CapsuleEdge, CapsuleMatch, PathRecord, PendingFacts};
use crate::ContentHash;

const ZERO_BLOB: [u8; 32] = [0; 32];

pub fn blob_at(blob_hashes: &[[u8; 32]], idx: u32) -> &[u8; 32] {
    blob_hashes.get(idx as usize).unwrap_or(&ZERO_BLOB)
}

pub fn hex_blob(bytes: &[u8; 32]) -> String {
    crate::fast_hex_32(bytes)
}

pub fn coverage_ratios(
    snapshot_tier_counts: [usize; 3],
    pending_a: usize,
    total: usize,
) -> (f64, f64, f64) {
    if total == 0 {
        return (0.0, 0.0, 0.0);
    }
    let denom = total as f64;
    (
        (snapshot_tier_counts[0] + pending_a) as f64 / denom,
        snapshot_tier_counts[1] as f64 / denom,
        snapshot_tier_counts[2] as f64 / denom,
    )
}

pub struct QueryRepairParts<'a> {
    pub table: SymbolTable<'a>,
    /// v2: borrowed mmap; v1: owned upgrade (see `ShardView::spans`).
    pub spans: std::borrow::Cow<'a, [SpanEntry]>,
    pub csr: CsrAdjacency<'a>,
    pub evidence: std::borrow::Cow<'a, [SpanEntry]>,
    pub blob_hashes: &'a [[u8; 32]],
    pub cov_bits: &'a [u8],
}

pub fn query_repair_parts<'a>(
    view: &'a super::super::hot_path::ShardView<'a>,
) -> anyhow::Result<QueryRepairParts<'a>> {
    let coverage = view.coverage()?;
    Ok(QueryRepairParts {
        table: SymbolTable::from_view(view)?,
        spans: view.spans()?,
        csr: CsrAdjacency::new(view.edges()?),
        evidence: view.edge_evidence()?,
        blob_hashes: coverage.blob_hashes,
        cov_bits: coverage.bits,
    })
}

pub fn symbol_candidate_ids(table: &SymbolTable<'_>, symbol: &str) -> Vec<u32> {
    let mut ids = table
        .get(symbol)
        .map(|id| vec![id])
        .unwrap_or_else(|| table.prefix_search(symbol).take(5).collect());
    ids.dedup();
    ids
}

pub fn pending_tier_a(pending: &PendingFacts) -> usize {
    pending.blobs.values().filter(|b| **b & 0b001 != 0).count()
}

#[allow(clippy::too_many_arguments)]
pub fn capsule_match_for_symbol(
    snapshot: &Snapshot,
    table: &SymbolTable<'_>,
    spans: &[SpanEntry],
    csr: &CsrAdjacency<'_>,
    evidence: &[SpanEntry],
    blob_hashes: &[[u8; 32]],
    id: u32,
    check_freshness: bool,
) -> CapsuleMatch {
    let name = table.name(id).unwrap_or("").to_string();
    let defs = span_range(spans, id)
        .iter()
        .map(|s| {
            let hash_hex = hex_blob(blob_at(blob_hashes, s.blob_idx));
            let (path, stale) = render_def_staleness(snapshot, &hash_hex, check_freshness);
            let evidence_ref = blob_span_ref(&hash_hex, s.start, s.end);
            let _ =
                crate::link_emitted_symbol_view(crate::EntityViewKind::Read, &name, &evidence_ref);
            CapsuleDef {
                evidence_ref,
                path,
                stale,
            }
        })
        .collect::<Vec<_>>();
    let mut edges = Vec::new();
    let base = csr.edge_base(id);
    for (i, edge) in csr.edges(id).enumerate() {
        let to = table.name(edge.target).unwrap_or("").to_string();
        let ev = evidence.get(base + i).copied().unwrap_or_default();
        edges.push(CapsuleEdge {
            kind: edge.kind,
            to,
            confidence: edge.confidence as f64 / 255.0,
            evidence_ref: blob_span_ref(
                &hex_blob(blob_at(blob_hashes, ev.blob_idx)),
                ev.start,
                ev.end,
            ),
            source: None,
        });
    }
    CapsuleMatch { name, defs, edges }
}

pub fn merge_pending_defs_edges(
    snapshot: &Snapshot,
    symbol: &str,
    matches: &mut Vec<CapsuleMatch>,
) {
    for (name, blob, start, end) in &snapshot.pending.defs {
        if name == symbol || name.starts_with(symbol) {
            let def = CapsuleDef {
                evidence_ref: blob_span_ref(&hex_blob(blob), *start, *end),
                path: snapshot
                    .paths()
                    .get(&ContentHash::from_bytes(*blob))
                    .map(|p| p.path.clone()),
                stale: false,
            };
            if let Some(m) = matches.iter_mut().find(|m| &m.name == name) {
                m.defs.push(def);
            } else {
                matches.push(CapsuleMatch {
                    name: name.clone(),
                    defs: vec![def],
                    edges: Vec::new(),
                });
            }
        }
    }
    for (src, dst, kind, conf, blob, start, end, source) in &snapshot.pending.edges {
        if let Some(m) = matches.iter_mut().find(|m| &m.name == src) {
            m.edges.push(CapsuleEdge {
                kind: *kind,
                to: dst.clone(),
                confidence: *conf as f64 / 255.0,
                evidence_ref: blob_span_ref(&hex_blob(blob), *start, *end),
                source: source.clone(),
            });
        }
    }
}

pub fn load_path_records(
    shards_dir: &Path,
    snapshot_id: u64,
) -> Result<HashMap<ContentHash, PathRecord>> {
    let path = shards_dir.join(paths_file_name(snapshot_id));
    let txt = std::fs::read_to_string(&path)
        .with_context(|| format!("read snapshot paths {}", path.display()))?;
    let line_count = txt.bytes().filter(|&b| b == b'\n').count().max(1);
    // Keys are ContentHash (32 bytes), not 64-hex Strings — open footprint was
    // ~2 owned Strings per blob (graphzero-a4t6p).
    let mut paths = HashMap::with_capacity(line_count);
    for (line_index, line) in txt.lines().enumerate() {
        let mut parts = line.splitn(5, ' ');
        let (Some(hash), Some(mtime), Some(size), Some(tier), Some(path)) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            bail!("malformed path record at line {}", line_index + 1);
        };
        let Some(hash_key) = ContentHash::from_hex(hash) else {
            bail!("invalid blob hash at line {}", line_index + 1);
        };
        paths.insert(
            hash_key,
            PathRecord {
                mtime_nanos: mtime
                    .parse()
                    .with_context(|| format!("invalid mtime at line {}", line_index + 1))?,
                size: size
                    .parse()
                    .with_context(|| format!("invalid size at line {}", line_index + 1))?,
                tier_bits: tier
                    .parse()
                    .with_context(|| format!("invalid tier bits at line {}", line_index + 1))?,
                path: path.to_string(),
            },
        );
    }
    Ok(paths)
}

pub fn merge_wal_into_pending(
    entry: &SnapshotEntry,
    wal_dir: &Path,
) -> anyhow::Result<PendingFacts> {
    let mut pending = PendingFacts::default();
    if !wal_dir.is_dir() {
        return Ok(pending);
    }
    let dir_empty = std::fs::read_dir(wal_dir)
        .map(|mut d| d.next().is_none())
        .unwrap_or(true);
    if dir_empty {
        return Ok(pending);
    }
    let folded: std::collections::BTreeSet<u64> = entry.segment_ids.iter().copied().collect();
    for (id, entries) in read_all_segments(wal_dir)? {
        if !folded.contains(&id) {
            let facts = PendingFacts::from_entries(&entries);
            pending.defs.extend(facts.defs);
            pending.edges.extend(facts.edges);
            pending.blobs.extend(facts.blobs);
        }
    }
    Ok(pending)
}
