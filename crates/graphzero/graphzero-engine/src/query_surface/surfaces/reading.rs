use std::collections::VecDeque;

use crate::accounting::accounting_for_evidence_refs;

use graphzero_store::Snapshot;
use graphzero_store::store::csr::{CsrAdjacency, edge_kind};
use graphzero_store::store::query::QueryEngine;
use graphzero_store::store::symbol_table::SymbolTable;

use super::super::QuerySurfaceRouter;
use super::super::helpers::{checked_blob_hash, empty_capsule};
use super::super::types::*;

impl QuerySurfaceRouter {
    pub(super) fn reading_set(
        snapshot: &Snapshot,
        req: &QuerySurfaceRequest,
        budget: usize,
    ) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
        let name = req
            .name
            .as_deref()
            .or(req.query.as_deref())
            .ok_or(QuerySurfaceError::MissingArgument("name"))?;
        let view = snapshot
            .global_view()
            .map_err(|_| QuerySurfaceError::SymbolNotFound(name.into()))?;
        let table = SymbolTable::from_view(&view)
            .map_err(|_| QuerySurfaceError::SymbolNotFound(name.into()))?;
        let Some(target_id) = table.get(name) else {
            return Err(QuerySurfaceError::SymbolNotFound(name.into()));
        };
        let csr = CsrAdjacency::new(
            view.edges()
                .map_err(|_| QuerySurfaceError::EvidenceMissing)?,
        );
        let evidence = view
            .edge_evidence()
            .map_err(|_| QuerySurfaceError::EvidenceMissing)?;
        let blob_hashes = view
            .coverage()
            .map_err(|_| QuerySurfaceError::EvidenceMissing)?
            .blob_hashes;
        let mut entries = collect_reading_set(
            snapshot,
            &table,
            &csr,
            &evidence,
            blob_hashes,
            target_id,
            budget,
        )?;
        let capsule = QueryEngine::warm(snapshot, name, budget)
            .unwrap_or_else(|_| empty_capsule(name, snapshot));
        entries.sort_by(|a, b| {
            a.rank
                .cmp(&b.rank)
                .then_with(|| b.confidence.total_cmp(&a.confidence))
                .then_with(|| a.target.cmp(&b.target))
        });
        let coverage = Self::footer(snapshot, &capsule)?;
        let closure = reading_set_closure(&coverage);
        let evidence_refs: Vec<String> = entries
            .iter()
            .map(|entry| entry.evidence_ref.clone())
            .collect();
        let accounting = accounting_for_evidence_refs(
            snapshot,
            "reading_set_closure",
            evidence_refs.iter(),
            "reading_set closure selected required graph evidence and excluded the remaining indexed files",
        );
        Ok(QuerySurfaceResponse {
            schema_version: 1,
            surface: "reading_set".into(),
            coverage,
            symbol: Some(name.to_string()),
            reading_set: entries,
            reading_set_closure: Some(closure),
            accounting: Some(accounting),
            ..Default::default()
        })
    }
}

fn collect_reading_set(
    snapshot: &graphzero_store::Snapshot,
    table: &SymbolTable,
    csr: &CsrAdjacency<'_>,
    evidence: &[graphzero_store::store::format::SpanEntry],
    blob_hashes: &[[u8; 32]],
    target_id: u32,
    max_depth: usize,
) -> Result<Vec<ReadingSetEntry>, QuerySurfaceError> {
    let reverse_calls = snapshot
        .calls_reverse_index()
        .map_err(|_| QuerySurfaceError::EvidenceMissing)?;
    let view = ReadingSetView {
        table,
        evidence,
        blob_hashes,
    };
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    seen.insert(("target".to_string(), target_id));
    out.push(reading_entry(
        &view,
        ReadingEdge {
            symbol_id: target_id,
            evidence_node: target_id,
            edge_idx: 0,
            kind: "target",
            reason: "change target declaration",
            rank: 1,
            depth: Some(0),
            confidence: 1.0,
        },
    )?);
    collect_reading_callers(
        &view,
        csr,
        reverse_calls,
        target_id,
        max_depth.max(1) as u32,
        &mut seen,
        &mut out,
    )?;
    collect_reading_outgoing(&view, csr, target_id, &mut seen, &mut out)?;
    collect_reading_tests(&view, csr, table, target_id, &mut seen, &mut out)?;
    Ok(out)
}

fn collect_reading_callers(
    view: &ReadingSetView<'_>,
    csr: &CsrAdjacency<'_>,
    reverse_calls: &graphzero_store::ReverseIndex,
    target_id: u32,
    depth_limit: u32,
    seen: &mut std::collections::BTreeSet<(String, u32)>,
    out: &mut Vec<ReadingSetEntry>,
) -> Result<(), QuerySurfaceError> {
    let mut queue = VecDeque::from([(target_id, 0u32)]);
    while let Some((node, depth)) = queue.pop_front() {
        if depth >= depth_limit {
            continue;
        }
        for &(caller, edge_idx) in reverse_calls.callers(node) {
            if !seen.insert(("caller".to_string(), caller)) {
                continue;
            }
            let edge_idx = edge_idx as usize;
            out.push(reading_entry(
                view,
                ReadingEdge {
                    symbol_id: caller,
                    evidence_node: node,
                    edge_idx,
                    kind: "caller",
                    reason: "direct/transitive caller that can break after target change",
                    rank: 10 + depth,
                    depth: Some(depth + 1),
                    confidence: edge_confidence(csr, caller, edge_idx),
                },
            )?);
            queue.push_back((caller, depth + 1));
        }
    }
    Ok(())
}

fn reading_outgoing_kind(kind: u8) -> Option<(&'static str, &'static str, u32)> {
    if kind == edge_kind::CALLS {
        Some(("callee", "callee contract used by change target", 30))
    } else if kind == edge_kind::IMPORTS {
        Some(("dependency", "import dependency used by change target", 40))
    } else if kind == edge_kind::REFS {
        Some(("type_ref", "type/reference in target signature or body", 20))
    } else {
        None
    }
}

fn collect_reading_outgoing(
    view: &ReadingSetView<'_>,
    csr: &CsrAdjacency<'_>,
    target_id: u32,
    seen: &mut std::collections::BTreeSet<(String, u32)>,
    out: &mut Vec<ReadingSetEntry>,
) -> Result<(), QuerySurfaceError> {
    for (offset, edge) in csr.edges(target_id).enumerate() {
        let Some((kind, reason, rank)) = reading_outgoing_kind(edge.kind) else {
            continue;
        };
        if !seen.insert((kind.to_string(), edge.target)) {
            continue;
        }
        out.push(reading_entry(
            view,
            ReadingEdge {
                symbol_id: edge.target,
                evidence_node: target_id,
                edge_idx: csr.edge_base(target_id) + offset,
                kind,
                reason,
                rank,
                depth: Some(1),
                confidence: edge.confidence as f64 / 255.0,
            },
        )?);
    }
    Ok(())
}

fn is_test_symbol_name(name: &str) -> bool {
    name.contains("test") || name.ends_with("_test") || name.starts_with("test_")
}

fn collect_reading_tests(
    view: &ReadingSetView<'_>,
    csr: &CsrAdjacency<'_>,
    table: &SymbolTable,
    target_id: u32,
    seen: &mut std::collections::BTreeSet<(String, u32)>,
    out: &mut Vec<ReadingSetEntry>,
) -> Result<(), QuerySurfaceError> {
    for candidate in 0..csr.num_symbols() as u32 {
        let Some(name) = table.name(candidate) else {
            continue;
        };
        if !is_test_symbol_name(name) {
            continue;
        }
        let base = csr.edge_base(candidate);
        for (offset, edge) in csr.edges(candidate).enumerate() {
            if edge.kind != edge_kind::CALLS || edge.target != target_id {
                continue;
            }
            if !seen.insert(("test".to_string(), candidate)) {
                continue;
            }
            out.push(reading_entry(
                view,
                ReadingEdge {
                    symbol_id: candidate,
                    evidence_node: target_id,
                    edge_idx: base + offset,
                    kind: "test",
                    reason: "test exercising the change target",
                    rank: 50,
                    depth: Some(1),
                    confidence: edge.confidence as f64 / 255.0,
                },
            )?);
        }
    }
    Ok(())
}

fn edge_confidence(csr: &CsrAdjacency<'_>, src: u32, edge_idx: usize) -> f64 {
    let local = edge_idx.saturating_sub(csr.edge_base(src));
    csr.edges(src)
        .nth(local)
        .map(|e| e.confidence as f64 / 255.0)
        .unwrap_or(0.0)
}

/// Borrowed graph views shared by every reading-set entry.
struct ReadingSetView<'a> {
    table: &'a SymbolTable<'a>,
    evidence: &'a [graphzero_store::store::format::SpanEntry],
    blob_hashes: &'a [[u8; 32]],
}

/// Per-entry description of one reading-set row.
struct ReadingEdge<'a> {
    symbol_id: u32,
    evidence_node: u32,
    edge_idx: usize,
    kind: &'a str,
    reason: &'a str,
    rank: u32,
    depth: Option<u32>,
    confidence: f64,
}

fn reading_entry(
    view: &ReadingSetView<'_>,
    edge: ReadingEdge<'_>,
) -> Result<ReadingSetEntry, QuerySurfaceError> {
    let ReadingSetView {
        table,
        evidence,
        blob_hashes,
    } = *view;
    let ReadingEdge {
        symbol_id,
        evidence_node,
        edge_idx,
        kind,
        reason,
        rank,
        depth,
        confidence,
    } = edge;
    let ev = evidence.get(edge_idx).copied().unwrap_or_default();
    let evidence_ref = if kind == "target" {
        format!("gz://node/{}", table.name(symbol_id).unwrap_or(""))
    } else {
        let hash_hex = checked_blob_hash(blob_hashes, ev.blob_idx)?;
        graphzero_store::store::refs::blob_span_ref(&hash_hex, ev.start, ev.end)
    };
    if evidence_ref.is_empty() {
        return Err(QuerySurfaceError::EvidenceMissing);
    }
    Ok(ReadingSetEntry {
        target: table.name(symbol_id).unwrap_or("").to_string(),
        kind: kind.to_string(),
        rank,
        reason: if kind == "target" {
            reason.to_string()
        } else {
            format!(
                "{reason}; evidence edge from {}",
                table.name(evidence_node).unwrap_or("")
            )
        },
        evidence_ref,
        confidence,
        confidence_level: confidence_level(confidence).to_string(),
        depth,
    })
}

fn reading_set_closure(coverage: &CoverageFooter) -> ReadingSetClosure {
    let confidence_level = if coverage.tier_b > 0.0 {
        "typed"
    } else if coverage.tier_a > 0.0 {
        "structural"
    } else {
        "none"
    };
    ReadingSetClosure {
        confidence_level: confidence_level.into(),
        guarantee: "sufficient for statically-resolvable call/import/reference edges present in the indexed graph at the reported coverage tier".into(),
        sound_when: vec![
            "the target symbol resolves in the current snapshot".into(),
            "all affected sites are represented by indexed static call/import/reference edges".into(),
            "dynamic dispatch and type-qualified references are covered only when adapter-derived tier-B edges are present".into(),
        ],
        out_of_scope: vec![
            "reflection, macro expansion gaps, runtime string dispatch, generated code not indexed, and foreign code outside the snapshot".into(),
            "dynamic dispatch or overload resolution when no tier-B adapter edges were indexed".into(),
        ],
    }
}

fn confidence_level(confidence: f64) -> &'static str {
    if confidence >= 0.95 {
        "exact"
    } else if confidence >= 0.70 {
        "structural"
    } else {
        "heuristic"
    }
}
