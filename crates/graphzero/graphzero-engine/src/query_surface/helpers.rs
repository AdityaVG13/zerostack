use std::collections::HashSet;

use graphzero_store::span_range;
use graphzero_store::store::absence::{AbsenceConfig, absence};
use graphzero_store::store::blob_store::BlobStore;
use graphzero_store::store::csr::{CsrAdjacency, edge_kind};
use graphzero_store::store::format::{SpanEntry, symbol_kind};
use graphzero_store::store::query::{Capsule, CapsuleEdge, QueryEngine};
use graphzero_store::store::refs::blob_span_ref;
use graphzero_store::store::symbol_table::SymbolTable;
use graphzero_store::{ContentHash, Snapshot};
use serde_json::{Value, json};

use super::QuerySurfaceRouter;
use super::types::*;

pub(super) fn checked_blob_hash(
    blob_hashes: &[[u8; 32]],
    blob_idx: u32,
) -> Result<String, QuerySurfaceError> {
    graphzero_store::hex_blob_hash(blob_hashes, blob_idx).map_err(|err| {
        QuerySurfaceError::MalformedIndex {
            blob_idx: err.blob_idx,
            blob_hash_count: err.blob_hash_count,
        }
    })
}

impl Default for QuerySurfaceResponse {
    fn default() -> Self {
        Self {
            schema_version: 1,
            surface: String::new(),
            coverage: CoverageFooter {
                tier_a: 0.0,
                tier_b: 0.0,
                tier_c: 0.0,
                freshness_verified: false,
                snapshot_id: 0,
            },
            decl_ref: None,
            symbol: None,
            edges: Vec::new(),
            outline: Vec::new(),
            skeleton: String::new(),
            skeletons: Vec::new(),
            delta: None,
            rows: Vec::new(),
            hits: Vec::new(),
            reading_set: Vec::new(),
            reading_set_closure: None,
            capsule: None,
            absence_certificate: None,
            refs_footer: Vec::new(),
            full_ref: None,
            truncated: None,
            accounting: None,
            error: None,
            next: Vec::new(),
            next_cursor: None,
        }
    }
}

pub(super) fn callers_not_found_response(
    snapshot: &Snapshot,
    callee: &str,
    budget: usize,
) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
    let capsule = QueryEngine::warm(snapshot, callee, budget)
        .unwrap_or_else(|_| empty_capsule(callee, snapshot));
    let cert = absence(snapshot, callee, AbsenceConfig::default()).ok();
    Ok(QuerySurfaceResponse {
        schema_version: 1,
        surface: "callers".into(),
        coverage: QuerySurfaceRouter::footer(snapshot, &capsule)?,
        absence_certificate: cert.and_then(|c| serde_json::from_str(&c.to_json()).ok()),
        error: Some("SYMBOL_NOT_FOUND".into()),
        ..Default::default()
    })
}

pub(super) fn collect_call_edges(
    snapshot: &graphzero_store::Snapshot,
    table: &SymbolTable,
    csr: &CsrAdjacency<'_>,
    evidence: &[SpanEntry],
    blob_hashes: &[[u8; 32]],
    target_id: u32,
) -> Result<Vec<GraphEdge>, QuerySurfaceError> {
    let rev = snapshot
        .calls_reverse_index()
        .map_err(|_| QuerySurfaceError::EvidenceMissing)?;
    let mut edges = Vec::new();
    let to = table.name(target_id).unwrap_or("").to_string();
    for &(src, edge_idx) in rev.callers(target_id) {
        let edge_idx = edge_idx as usize;
        let edge = csr
            .edges(src)
            .nth(edge_idx - csr.edge_base(src))
            .filter(|e| e.target == target_id && e.kind == edge_kind::CALLS);
        let Some(edge) = edge else {
            continue;
        };
        let ev = evidence.get(edge_idx).copied().unwrap_or_default();
        let from = table.name(src).unwrap_or("").to_string();
        let hash_hex = checked_blob_hash(blob_hashes, ev.blob_idx)?;
        let evidence_ref = blob_span_ref(&hash_hex, ev.start, ev.end);
        if evidence_ref.is_empty() {
            return Err(QuerySurfaceError::EvidenceMissing);
        }
        edges.push(GraphEdge {
            kind: "calls".into(),
            to: to.clone(),
            from: Some(from),
            confidence: edge.confidence as f64 / 255.0,
            evidence_ref,
            source: "tier_a".into(),
        });
    }
    Ok(edges)
}

fn span_matches_outline_path(
    snapshot: &Snapshot,
    rel: &str,
    hash_hex: &str,
    hash_for_path: Option<&str>,
) -> bool {
    if let Some(want) = hash_for_path {
        return hash_hex == want;
    }
    snapshot.path_for_blob(hash_hex).map(|p| p.path.as_str()) == Some(rel)
}

pub(super) fn outline_items_for_path(
    snapshot: &Snapshot,
    rel: &str,
    table: &SymbolTable,
    spans: &[SpanEntry],
    blob_hashes: &[[u8; 32]],
    hash_for_path: Option<&str>,
) -> Result<Vec<OutlineItem>, QuerySurfaceError> {
    use graphzero_store::BlobStore;

    use super::skeleton::byte_span_to_lines;

    let mut outline = Vec::new();
    let mut blob_cache: Option<(String, Vec<u8>)> = None;
    for id in 0..table.len() as u32 {
        let Some(sym_name) = table.name(id) else {
            continue;
        };
        let Some(entry) = table.entry(id) else {
            continue;
        };
        for span in span_range(spans, id) {
            let hash_hex = checked_blob_hash(blob_hashes, span.blob_idx)?;
            if !span_matches_outline_path(snapshot, rel, &hash_hex, hash_for_path) {
                continue;
            }
            let (name_start, name_end) = span.name_byte_range();
            let evidence_ref = blob_span_ref(&hash_hex, name_start, name_end);
            if evidence_ref.is_empty() {
                return Err(QuerySurfaceError::EvidenceMissing);
            }
            let (outline_start, outline_end) = span.outline_byte_range();
            let (start_line, end_line) = {
                let need_load = blob_cache
                    .as_ref()
                    .map(|(h, _)| h.as_str() != hash_hex.as_str())
                    .unwrap_or(true);
                if need_load {
                    let bytes = BlobStore::open(&snapshot.store_root)
                        .ok()
                        .and_then(|bs| bs.get_hex(&hash_hex).ok())
                        .flatten()
                        .unwrap_or_default();
                    blob_cache = Some((hash_hex.clone(), bytes));
                }
                let blob = &blob_cache.as_ref().unwrap().1;
                let (sl, el) = byte_span_to_lines(blob, outline_start, outline_end);
                (Some(sl), Some(el))
            };
            outline.push(OutlineItem {
                name: sym_name.to_string(),
                kind: kind_label(entry.kind),
                evidence_ref: evidence_ref.clone(),
                source: "tier_a".into(),
                start_line,
                end_line,
            });
            let _ = graphzero_store::link_emitted_symbol_view(
                graphzero_store::EntityViewKind::Diff,
                sym_name,
                &evidence_ref,
            );
        }
    }
    Ok(outline)
}

pub(super) fn merge_exact_symbol_search_hit(
    snapshot: &Snapshot,
    needle: &str,
    budget: usize,
    hits: &mut Vec<SearchHit>,
) {
    let Ok(view) = snapshot.global_view() else {
        return;
    };
    let Ok(table) = SymbolTable::from_view(&view) else {
        return;
    };
    let Some(id) = table.get(needle) else {
        return;
    };
    let Some(name) = table.name(id) else {
        return;
    };
    let capsule =
        QueryEngine::warm(snapshot, name, budget).unwrap_or_else(|_| empty_capsule(name, snapshot));
    let Some(m) = capsule.matches.first() else {
        return;
    };
    let Some(d) = m.defs.first() else {
        return;
    };
    let Some(sha) = content_sha256_for_evidence_ref(snapshot, &d.evidence_ref) else {
        return;
    };
    if hits.iter().any(|h| h.content_sha256 == sha) {
        return;
    }
    hits.push(SearchHit {
        label: name.to_string(),
        snippet: snippet_for_evidence_ref(snapshot, &d.evidence_ref),
        content_sha256: sha,
        evidence_ref: d.evidence_ref.clone(),
        source: "tier_a".into(),
    });
}
pub(super) fn validate_edge_refs(edges: &[CapsuleEdge]) -> Result<(), QuerySurfaceError> {
    for e in edges {
        if e.evidence_ref.is_empty() {
            return Err(QuerySurfaceError::EvidenceMissing);
        }
    }
    Ok(())
}

pub(super) fn empty_capsule(query: &str, snapshot: &Snapshot) -> Capsule {
    Capsule {
        query: query.to_string(),
        snapshot_id: snapshot.entry.snapshot_id,
        matches: Vec::new(),
        tier_a: 0.0,
        tier_b: 0.0,
        tier_c: 0.0,
        budget: 1,
        freshness: Default::default(),
    }
}

/// Byte cap for hit snippets; matched lines plus one context line each side.
const SNIPPET_MAX_BYTES: usize = 240;

/// Snippet rule: hits carry only the matched lines plus about one line of
/// context; the full payload stays behind `evidence_ref`. Whole-blob refs
/// (path hits) return an empty snippet — the label is the match.
fn snippet_for_evidence_ref(snapshot: &Snapshot, evidence_ref: &str) -> String {
    use super::skeleton::byte_span_to_lines;

    let Some(rest) = evidence_ref.strip_prefix("z://blob/") else {
        return String::new();
    };
    let (hash_hex, span) = rest.split_once("#B").unwrap_or((rest, "0-0"));
    let Some((Some(start), Some(end))) = span
        .split_once('-')
        .map(|(s, e)| (s.parse::<u32>().ok(), e.parse::<u32>().ok()))
    else {
        return String::new();
    };
    if start == 0 && end == 0 {
        return String::new();
    }
    let Some(bytes) = BlobStore::open(&snapshot.store_root)
        .ok()
        .and_then(|bs| bs.get_hex(hash_hex).ok())
        .flatten()
    else {
        return String::new();
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return String::new();
    };
    let (start_line, end_line) = byte_span_to_lines(&bytes, start, end);
    let lines: Vec<&str> = text.lines().collect();
    let lo = (start_line as usize).saturating_sub(2);
    let hi = (end_line as usize + 1).min(lines.len());
    if lo >= hi {
        return String::new();
    }
    let mut out = lines[lo..hi].join("\n");
    if out.len() > SNIPPET_MAX_BYTES {
        let mut cut = SNIPPET_MAX_BYTES;
        while cut > 0 && !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push('…');
    }
    out
}

fn content_sha256_for_evidence_ref(snapshot: &Snapshot, evidence_ref: &str) -> Option<String> {
    let rest = evidence_ref.strip_prefix("z://blob/")?;
    let (hash_hex, span) = rest.split_once("#B").unwrap_or((rest, "0-0"));
    let (start, end) = span.split_once('-').unwrap_or(("0", "0"));
    let start = start.parse::<usize>().ok()?;
    let end = end.parse::<usize>().ok()?;
    let bytes = BlobStore::open(&snapshot.store_root)
        .ok()?
        .get_hex(hash_hex)
        .ok()??;
    let slice = if start == 0 && end == 0 {
        bytes.as_slice()
    } else {
        bytes.get(start..end)?
    };
    Some(ContentHash::of(slice).to_hex())
}

pub(super) fn tier_c_surface(
    snapshot: &Snapshot,
    surface: &str,
    query_key: &str,
    budget: usize,
) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
    let capsule = match snapshot.git_empirical_capsule(query_key, budget, false) {
        Ok(Some(c)) => c,
        Ok(None) | Err(_) => {
            let empty = empty_capsule(query_key, snapshot);
            return Ok(QuerySurfaceResponse {
                schema_version: 1,
                surface: surface.into(),
                coverage: QuerySurfaceRouter::footer(snapshot, &empty)?,
                absence_certificate: absence(snapshot, query_key, AbsenceConfig::default())
                    .ok()
                    .and_then(|c| serde_json::from_str(&c.to_json()).ok()),
                rows: Vec::new(),
                ..Default::default()
            });
        }
    };
    let coverage = CoverageFooter {
        tier_a: capsule.coverage.tier_a,
        tier_b: capsule.coverage.tier_b,
        tier_c: capsule.coverage.tier_c,
        freshness_verified: capsule.coverage.freshness_verified,
        snapshot_id: capsule.snapshot_id,
    };
    if capsule.coverage.tier_c <= 0.0 {
        let cert = absence(snapshot, query_key, AbsenceConfig::default()).ok();
        return Ok(QuerySurfaceResponse {
            schema_version: 1,
            surface: surface.into(),
            coverage,
            absence_certificate: cert.and_then(|c| serde_json::from_str(&c.to_json()).ok()),
            rows: Vec::new(),
            ..Default::default()
        });
    }
    let rows = tier_c_rows_from_destinations(&capsule);
    Ok(QuerySurfaceResponse {
        schema_version: 1,
        surface: surface.into(),
        coverage,
        rows,
        ..Default::default()
    })
}

fn tier_c_rows_from_destinations(capsule: &graphzero_store::QueryCapsule) -> Vec<Value> {
    capsule
        .destinations
        .iter()
        .map(|d| {
            json!({
                "path": d.label,
                "score": d.label,
                "evidence_ref": d.evidence_ref,
                "source": "tier_c",
            })
        })
        .collect()
}

fn kind_label(kind: u8) -> String {
    match kind {
        symbol_kind::FUNCTION => "function".into(),
        symbol_kind::TYPE => "type".into(),
        symbol_kind::MODULE => "module".into(),
        _ => "symbol".into(),
    }
}

pub(super) fn outline_kind_name(kind: u8) -> String {
    kind_label(kind)
}

fn push_search_hit(
    snapshot: &Snapshot,
    hits: &mut Vec<SearchHit>,
    seen: &mut HashSet<String>,
    label: String,
    evidence_ref: String,
) -> Result<(), QuerySurfaceError> {
    // Soft-skip unresolvable evidence: empty search results must not become
    // Corrupt individual spans drop out as `EVIDENCE_MISSING`.
    if evidence_ref.is_empty() {
        return Ok(());
    }
    let Some(sha) = content_sha256_for_evidence_ref(snapshot, &evidence_ref) else {
        return Ok(());
    };
    if !seen.insert(sha.clone()) {
        return Ok(());
    }
    let _ = graphzero_store::link_emitted_symbol_view(
        graphzero_store::EntityViewKind::Grep,
        &label,
        &evidence_ref,
    );
    hits.push(SearchHit {
        label,
        snippet: snippet_for_evidence_ref(snapshot, &evidence_ref),
        content_sha256: sha,
        evidence_ref,
        source: "tier_a".into(),
    });
    Ok(())
}

fn symbol_search_hits(
    snapshot: &Snapshot,
    table: &SymbolTable<'_>,
    spans: &[SpanEntry],
    blob_hashes: &[[u8; 32]],
    needle: &str,
    budget: usize,
    hits: &mut Vec<SearchHit>,
    seen: &mut HashSet<String>,
    candidate_ids: Option<&[u32]>,
) -> Result<bool, QuerySurfaceError> {
    let cap = budget.max(1);
    match candidate_ids {
        Some(cands) => {
            for &id in cands {
                if push_matching_symbol_hits(
                    snapshot,
                    table,
                    spans,
                    blob_hashes,
                    needle,
                    id,
                    cap,
                    hits,
                    seen,
                )? {
                    return Ok(true);
                }
            }
        }
        None => {
            for id in 0..table.len() as u32 {
                if push_matching_symbol_hits(
                    snapshot,
                    table,
                    spans,
                    blob_hashes,
                    needle,
                    id,
                    cap,
                    hits,
                    seen,
                )? {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn push_matching_symbol_hits(
    snapshot: &Snapshot,
    table: &SymbolTable<'_>,
    spans: &[SpanEntry],
    blob_hashes: &[[u8; 32]],
    needle: &str,
    id: u32,
    cap: usize,
    hits: &mut Vec<SearchHit>,
    seen: &mut HashSet<String>,
) -> Result<bool, QuerySurfaceError> {
    let Some(name) = table.name(id) else {
        return Ok(false);
    };
    if !name.contains(needle) {
        return Ok(false);
    }
    for span in span_range(spans, id) {
        let hash_hex = checked_blob_hash(blob_hashes, span.blob_idx)?;
        let evidence_ref = blob_span_ref(&hash_hex, span.start, span.end);
        push_search_hit(snapshot, hits, seen, name.to_string(), evidence_ref)?;
        if hits.len() >= cap {
            return Ok(true);
        }
    }
    Ok(false)
}

fn path_search_hits(
    snapshot: &Snapshot,
    needle: &str,
    hits: &mut Vec<SearchHit>,
    seen: &mut HashSet<String>,
    candidate_hashes: Option<&HashSet<String>>,
) -> Result<(), QuerySurfaceError> {
    for (hash, rec) in snapshot.path_records() {
        let hash_hex = hash.to_hex();
        if let Some(cands) = candidate_hashes {
            if !cands.contains(&hash_hex) {
                continue;
            }
        }
        if !rec.path.contains(needle) {
            continue;
        }
        let evidence_ref = format!("z://blob/{hash_hex}#B0-0");
        push_search_hit(snapshot, hits, seen, rec.path.clone(), evidence_ref)?;
    }
    Ok(())
}

pub(super) fn search_hits(
    snapshot: &Snapshot,
    needle: &str,
    budget: usize,
) -> Result<Vec<SearchHit>, QuerySurfaceError> {
    let view = snapshot
        .global_view()
        .map_err(|_| QuerySurfaceError::EvidenceMissing)?;
    let table = SymbolTable::from_view(&view).map_err(|_| QuerySurfaceError::EvidenceMissing)?;
    let spans = view
        .spans()
        .map_err(|_| QuerySurfaceError::EvidenceMissing)?;
    let blob_hashes = view
        .coverage()
        .map_err(|_| QuerySurfaceError::EvidenceMissing)?
        .blob_hashes;
    let mut hits = Vec::new();
    let mut seen = HashSet::new();

    let bigram = if graphzero_store::search_bigram_enabled() {
        snapshot.name_bigram_index().ok()
    } else {
        None
    };
    let sym_cands = bigram
        .as_ref()
        .and_then(|index| index.candidate_symbol_ids_for_budget(needle, Some(budget)));

    if symbol_search_hits(
        snapshot,
        &table,
        &spans,
        blob_hashes,
        needle,
        budget,
        &mut hits,
        &mut seen,
        sym_cands.as_deref(),
    )? {
        return Ok(hits);
    }
    // Path candidates only when symbol search did not fill the budget
    // (common-class parse* otherwise pays for unused hex HashSet build).
    let path_cands = bigram
        .as_ref()
        .and_then(|index| index.candidate_path_hashes_for_budget(needle, Some(budget)));
    path_search_hits(snapshot, needle, &mut hits, &mut seen, path_cands.as_ref())?;
    Ok(hits)
}
