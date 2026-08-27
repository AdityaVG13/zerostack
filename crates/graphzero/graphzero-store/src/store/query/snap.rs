//! Snap routing, git empirical capsules, and the `snap()` entry point.

use anyhow::{Result, bail};

use super::super::csr::CsrAdjacency;
use super::super::refs::blob_span_ref;
use super::super::session::{
    apply_seen_to_destinations, clear_session_state, default_seen_provider,
};
use super::super::symbol_table::SymbolTable;
use super::budget::tokens_for_str;
use super::capsule_json::render_query_capsule_json;
use super::file_target::{TARGET_INLINE_TOP_HITS, file_target_for_evidence};
use super::lexical::graph_proximity_boost;
use super::snapshot::Snapshot;
use super::types::{
    BudgetLedger, Capsule, CapsuleDef, CoverageCertificate, DestinationRef, ExportArtifact,
    ExportFormat, QueryCapsule, RouteDiagnostics, SnapRoute,
};
use crate::{EntityViewKind, link_emitted_symbol_view};

pub fn normalize_snap_query(query: &str) -> String {
    super::types::normalize_snap_query(query)
}

/// Clears session dedup state (integration tests).
#[doc(hidden)]
pub fn clear_snap_session_cache() {
    clear_session_state();
}

fn canonical_source_ref(snapshot: &Snapshot, def: &CapsuleDef) -> Option<String> {
    let raw = def.evidence_ref.strip_prefix("gz://blob/")?;
    let (hash, span) = raw.split_once("#B")?;
    let (start, end) = span.split_once('-')?;
    let start = start.parse::<usize>().ok()?;
    let end = end.parse::<usize>().ok()?;
    let bytes = snapshot.blob_bytes(hash)?;
    if start >= bytes.len() || end > bytes.len() || start >= end {
        return None;
    }
    let line_start = bytes[..start]
        .iter()
        .rposition(|&byte| byte == b'\n')
        .map_or(0, |offset| offset + 1);
    let line_end = bytes[end..]
        .iter()
        .position(|&byte| byte == b'\n')
        .map_or(bytes.len(), |offset| end + offset + 1);
    Some(format!("gz://blob/{hash}#B{line_start}-{line_end}"))
}

fn destinations_from_capsule_matches(
    snapshot: &Snapshot,
    matches: &[super::types::CapsuleMatch],
) -> Vec<DestinationRef> {
    matches
        .iter()
        .enumerate()
        .map(|(rank, m)| {
            let first_def = m.defs.iter().min_by_key(|d| match d.path.as_deref() {
                Some(path) if path.starts_with("src/") || path.contains("/src/") => 0,
                Some(path) if path.contains("test") => 2,
                Some(path) if path.contains("benchmark") => 3,
                Some(_) => 1,
                None => 4,
            });
            let evidence = first_def
                .and_then(|d| canonical_source_ref(snapshot, d))
                .or_else(|| first_def.map(|d| d.evidence_ref.clone()))
                .unwrap_or_else(|| format!("gz://node/{}", m.name));
            let node = evidence.clone();
            let _ = link_emitted_symbol_view(EntityViewKind::Read, &m.name, &evidence);
            let _ = link_emitted_symbol_view(EntityViewKind::Node, &m.name, &node);
            let hit = file_target_for_evidence(
                snapshot,
                &evidence,
                "def",
                Some(&m.name),
                rank < TARGET_INLINE_TOP_HITS,
            );
            DestinationRef {
                destination_ref: node,
                evidence_ref: evidence,
                label: m.name.clone(),
                path: first_def.and_then(|d| d.path.clone()),
                target: hit.as_ref().map(|h| h.target.clone()),
                kind: hit.as_ref().map(|h| h.kind.clone()),
                symbol: hit.as_ref().map(|h| h.symbol.clone()),
                content: hit
                    .as_ref()
                    .filter(|h| !h.content.is_empty())
                    .map(|h| h.content.clone()),
            }
        })
        .collect()
}

fn apply_session_dedup(
    session: Option<&str>,
    destinations: &mut Vec<DestinationRef>,
) -> crate::SessionDedupStats {
    let seen = default_seen_provider();
    apply_seen_to_destinations(session, &seen, destinations)
}

#[allow(clippy::too_many_arguments)]
fn build_query_capsule(
    normalized: String,
    budget: usize,
    route: SnapRoute,
    mut destinations: Vec<DestinationRef>,
    symbol_capsule: &Capsule,
    mut diagnostics: RouteDiagnostics,
    session: Option<&str>,
    check_freshness: bool,
    snapshot: &Snapshot,
) -> QueryCapsule {
    diagnostics.removed_count = {
        let stats = apply_session_dedup(session, &mut destinations);
        diagnostics.byte_deduped = stats.byte_deduped;
        diagnostics.entity_deduped = stats.entity_deduped;
        stats.session_deduped
    };
    let json_preview = render_query_capsule_json(
        &normalized,
        budget,
        route,
        &destinations,
        symbol_capsule,
        &diagnostics,
        false,
        0,
        None,
        snapshot.semantic_tier_percent(),
    );
    let used = tokens_for_str(&json_preview);
    let truncated = used > budget;
    let omitted_count = if truncated {
        symbol_capsule
            .matches
            .len()
            .saturating_sub(destinations.len())
    } else {
        0
    };
    if truncated && destinations.len() > 1 {
        destinations.truncate(1);
    }
    QueryCapsule {
        schema_version: 1,
        query: normalized,
        budget,
        route,
        destinations,
        coverage: CoverageCertificate {
            tier_a: symbol_capsule.tier_a,
            tier_b: symbol_capsule.tier_b,
            tier_c: symbol_capsule.tier_c,
            semantic_tier_percent: snapshot.semantic_tier_percent(),
            // Reuse the verdict the capsule's query already computed
            // (graphzero perf): re-running the full per-file scan here doubled
            // the freshness cost of every snap op for the identical answer.
            freshness_verified: check_freshness && symbol_capsule.freshness.events.is_empty(),
        },
        diagnostics,
        ledger: BudgetLedger {
            requested_budget: budget,
            used_budget: used.min(budget),
            remaining_budget: budget.saturating_sub(used.min(budget)),
            truncated,
            omitted_count,
        },
        snapshot_id: symbol_capsule.snapshot_id,
    }
}

/// Probe which snap route would resolve `query` without building a full capsule.
pub fn probe_snap_route(snapshot: &Snapshot, query: &str) -> Result<(SnapRoute, String)> {
    let normalized = normalize_snap_query(query);
    if normalized.is_empty() {
        bail!("empty snap query");
    }
    let (route, _, symbol) = resolve_snap_route(snapshot, &normalized)?;
    Ok((route, symbol))
}

/// `snap(query, budget)` library entry (P1.1 G-001).
pub fn snap(
    snapshot: &Snapshot,
    query: &str,
    budget: usize,
    session: Option<&str>,
    check_freshness: bool,
) -> Result<QueryCapsule> {
    let normalized = normalize_snap_query(query);
    if normalized.is_empty() {
        bail!("empty snap query");
    }
    if let Some(capsule) = snapshot.git_empirical_capsule(&normalized, budget, check_freshness)? {
        return Ok(capsule);
    }
    let (route, mut diagnostics, symbol) = resolve_snap_route(snapshot, &normalized)?;
    let symbol_capsule = snapshot.query(&symbol, budget, check_freshness)?;
    let mut destinations = destinations_from_capsule_matches(snapshot, &symbol_capsule.matches);
    if route == SnapRoute::Semantic
        && let Some((semantic_destinations, semantic_diagnostics)) =
            semantic_route_destinations(snapshot, &normalized, budget)
    {
        destinations = semantic_destinations;
        diagnostics = semantic_diagnostics;
    }
    Ok(build_query_capsule(
        normalized,
        budget,
        route,
        destinations,
        &symbol_capsule,
        diagnostics,
        session,
        check_freshness,
        snapshot,
    ))
}

fn resolve_snap_route(
    snapshot: &Snapshot,
    query: &str,
) -> Result<(SnapRoute, RouteDiagnostics, String)> {
    let view = snapshot.global_view()?;
    let table = SymbolTable::from_view(&view)?;
    let diagnostics = RouteDiagnostics::default();
    if table.get(query).is_some() {
        return Ok((SnapRoute::Symbol, diagnostics, query.to_string()));
    }
    if table.prefix_search(query).take(1).next().is_some() && query.contains("::") {
        return Ok((SnapRoute::Symbol, diagnostics, query.to_string()));
    }
    let mut diagnostics = diagnostics;
    diagnostics.symbol_route = Some("miss");
    // pass table to avoid repeated global_view + from_view in trigram path
    if let Some(hit) = trigram_symbol_hit_from_table(&table, query) {
        diagnostics.notes.push("symbol_route_miss".into());
        return Ok((SnapRoute::Trigram, diagnostics, hit));
    }
    diagnostics.degraded_tiers.push("semantic");
    diagnostics.notes.push("semantic_degraded".into());
    Ok((SnapRoute::Semantic, diagnostics, query.to_string()))
}

fn first_symbol_name_containing(table: &SymbolTable, needle: &str) -> Option<String> {
    // early return: first match (no O(N) collect+sort+dedup allocs on fallback path)
    (0..table.len() as u32)
        .filter_map(|id| {
            let name = table.name(id)?;
            name.contains(needle).then(|| name.to_string())
        })
        .next()
}

// hoisted to avoid a 2nd view/from_view when the table is already built in resolve (perf)
fn trigram_symbol_hit_from_table(table: &SymbolTable, query: &str) -> Option<String> {
    if query.is_empty() {
        return None;
    }
    first_symbol_name_containing(table, query)
}

/// Lexical semantic tier: BM25 + graph-proximity rerank over symbol-span
/// chunks. Returns `Some((destinations, diagnostics))` when the tier has
/// at least one hit; `None` falls back to the degraded semantic path.
fn semantic_route_destinations(
    snapshot: &Snapshot,
    query: &str,
    budget: usize,
) -> Option<(Vec<DestinationRef>, RouteDiagnostics)> {
    let index = snapshot.lexical_semantic_index().ok()?;
    let k = budget.max(1).min(32);
    let mut hits = index.search(query, k);
    if hits.is_empty() {
        return None;
    }
    if let Ok(view) = snapshot.global_view() {
        let csr = CsrAdjacency::new(view.edges().ok()?);
        graph_proximity_boost(&mut hits, &csr);
    }
    let table = SymbolTable::from_view(&snapshot.global_view().ok()?).ok()?;

    let mut destinations = Vec::with_capacity(hits.len());
    for (rank, hit) in hits.iter().enumerate() {
        let name = table.name(hit.symbol_id).unwrap_or("").to_string();
        let hex = crate::fast_hex_32(&hit.blob);
        let evidence_ref = blob_span_ref(&hex, hit.start, hit.end);
        let path = snapshot.path_for_blob(&hex).map(|r| r.path.clone());
        let _ = link_emitted_symbol_view(EntityViewKind::Read, &name, &evidence_ref);
        let _ = link_emitted_symbol_view(EntityViewKind::Node, &name, &evidence_ref);
        let target = file_target_for_evidence(
            snapshot,
            &evidence_ref,
            "ref",
            Some(&name),
            rank < TARGET_INLINE_TOP_HITS,
        );
        destinations.push(DestinationRef {
            destination_ref: evidence_ref.clone(),
            evidence_ref,
            label: name,
            path,
            target: target.as_ref().map(|h| h.target.clone()),
            kind: target.as_ref().map(|h| h.kind.clone()),
            symbol: target.as_ref().map(|h| h.symbol.clone()),
            content: target
                .as_ref()
                .filter(|h| !h.content.is_empty())
                .map(|h| h.content.clone()),
        });
    }

    let mut diagnostics = RouteDiagnostics::default();
    diagnostics.symbol_route = Some("miss");
    // Clear semantic_degraded: the lexical tier served destinations.
    // diagnostics.degraded_tiers stays empty (semantic no longer degraded).
    diagnostics.notes.push("lexical_semantic_served".into());

    // Verify round-trip honesty: every destination's evidence span must
    // exist in the snapshot's blob store. Drop any that don't.
    destinations.retain(|d| {
        if let Some(raw) = d.evidence_ref.strip_prefix("gz://blob/") {
            if let Some((hash, span)) = raw.split_once("#B") {
                if let Some((start, end)) = span.split_once('-') {
                    if let (Ok(s), Ok(e)) = (start.parse::<usize>(), end.parse::<usize>()) {
                        if let Some(bytes) = snapshot.blob_bytes(hash) {
                            return s < bytes.len() && e <= bytes.len() && s < e;
                        }
                    }
                }
            }
        }
        false
    });

    if destinations.is_empty() {
        return None;
    }

    Some((destinations, diagnostics))
}

// --- snap --to-file / export support (perf-focused, reuse spill/expand, atomic write) ---

use anyhow::Context;
use std::fs;
use std::io::Write;
use std::path::Path;

/// Pure-std atomic write: mkdir + write .tmp next to target + rename. Minimal overhead.
/// No extra deps; safe for CLI/MCP concurrent-ish use (tmp name per-pid).
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("mkdir for export {}", parent.display()))?;
    }
    let fname = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "export".into());
    // pid + nanos for lower collision (inspired CBM cross-plat atomic + graphify tmp)
    let pid = std::process::id();
    let uniq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() % 1_000_000)
        .unwrap_or(0);
    let tmp = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".{}.{}.{}.tmp", fname, pid, uniq));
    {
        let mut f = fs::File::create(&tmp)
            .with_context(|| format!("create tmp for export {}", path.display()))?;
        f.write_all(bytes)?;
        f.sync_all()?; // durability for handoff / committed artifacts (CBM-style)
    }
    fs::rename(&tmp, path).with_context(|| format!("atomic rename export {}", path.display()))?;
    // best-effort cleanup ignored on success path
    Ok(())
}

/// Render tiny minimal export (ref-first, reuse spill id if low budget).
/// Goal: <1ms + <512B for budget=1. Carries enough for handoff + expand.
fn render_minimal(capsule: &QueryCapsule, store_root: Option<&Path>) -> String {
    let ref_str = if capsule.budget <= 1 {
        // force spill side effect via existing path for ref (cheap)
        let full = capsule.to_json(store_root);
        // extract or rebuild q: from spill logic
        let id = super::budget::spill_id_for_json(store_root, &full);
        format!("q:{}", id)
    } else {
        format!("q:cap-{}", capsule.snapshot_id)
    };
    let canonical = format!("gz://query/{}", ref_str.trim_start_matches("q:"));
    let meta = serde_json::json!({
        "budget": capsule.budget,
        "route": capsule.route.as_str(),
        "created_ts": "now", // filled at export time; no chrono dep for perf/min deps
        "visible_tokens": capsule.ledger.used_budget,
        "full_tokens": capsule.ledger.requested_budget * 4, // rough
        "coverage": {
            "tier_a": capsule.coverage.tier_a,
            "tier_b": capsule.coverage.tier_b,
            "tier_c": capsule.coverage.tier_c,
            "semantic_tier_percent": capsule.coverage.semantic_tier_percent,
            "freshness_verified": capsule.coverage.freshness_verified,
        },
        "snapshot_id": capsule.snapshot_id,
    });
    // Note: chrono may not be dep; for skeleton use a static ts if compile issue.
    serde_json::json!({
        "schema": "gz-snap/v1",
        "ref": ref_str,
        "canonical_ref": canonical,
        "query": capsule.query,
        "snapshot_id": capsule.snapshot_id,
        "meta": meta,
        "destinations": capsule.destinations.iter().map(|d| serde_json::json!({
            "ref": d.destination_ref,
            "evidence_ref": d.evidence_ref,
            "label": d.label
        })).collect::<Vec<_>>(),
    })
    .to_string()
}

/// Render handoff MD (human+agent readable).
fn render_md(capsule: &QueryCapsule, _store_root: Option<&Path>) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "# GraphZero Snap Handoff\n\n**Query**: {}\n",
        capsule.query
    ));
    s.push_str(&format!(
        "**Ref**: q: (expand for full)\n**Snapshot**: {}\n**Budget**: {} | Route: {}\n",
        capsule.snapshot_id,
        capsule.budget,
        capsule.route.as_str()
    ));
    s.push_str(&format!(
        "**Coverage**: tier_a={:.4} ...\n\n",
        capsule.coverage.tier_a
    ));
    if !capsule.destinations.is_empty() {
        s.push_str("## Key Destinations\n");
        for d in &capsule.destinations {
            s.push_str(&format!("- {} (evidence {})\n", d.label, d.evidence_ref));
        }
    }
    s.push_str("\n## Next\n- expand <ref> | blast ... | reserve\n");
    s
}

/// Export the (already-budgeted) capsule to path using chosen format.
/// Atomic, reuses spill/expand paths, minimal added CPU when not exporting.
/// Returns artifact meta for CLI/MCP stdout (no full content).
pub fn export_capsule(
    capsule: &QueryCapsule,
    store_root: Option<&Path>,
    export_path: &Path,
    format: ExportFormat,
) -> Result<ExportArtifact> {
    let (bytes, ref_str) = match format {
        ExportFormat::Minimal => {
            let j = render_minimal(capsule, store_root);
            (j.into_bytes(), format!("q:{}", capsule.snapshot_id)) // placeholder refined later
        }
        ExportFormat::Capsule => {
            let j = capsule.to_json(store_root);
            (j.into_bytes(), format!("q:cap-{}", capsule.snapshot_id))
        }
        ExportFormat::Md => {
            let m = render_md(capsule, store_root);
            (m.into_bytes(), format!("md-snap-{}", capsule.snapshot_id))
        }
        ExportFormat::Zst => {
            let j = capsule.to_json(store_root);
            // real zstd for committed portable (inspired CBM .zst)
            let compressed =
                zstd::encode_all(j.as_bytes(), 3).with_context(|| "zstd compress for export")?;
            (compressed, format!("zst-cap-{}", capsule.snapshot_id))
        }
    };
    atomic_write(export_path, &bytes)?;
    let size = bytes.len() as u64;
    // improve ref_str for minimal using spill
    let final_ref = if matches!(format, ExportFormat::Minimal) {
        // re-compute using spill helper for accurate q:id
        let full_preview = capsule.to_json(store_root);
        format!(
            "q:{}",
            super::budget::spill_id_for_json(store_root, &full_preview)
        )
    } else {
        ref_str
    };
    Ok(ExportArtifact {
        path: export_path.to_path_buf(),
        size_bytes: size,
        ref_str: final_ref,
        format,
    })
}

// end snap-to-file export skeleton (compilable; perf: spill reuse + direct write path)
