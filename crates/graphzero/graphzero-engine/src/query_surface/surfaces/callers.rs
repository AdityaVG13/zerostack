use std::collections::VecDeque;

use graphzero_store::Snapshot;
use graphzero_store::store::csr::{CsrAdjacency, edge_kind};
use graphzero_store::store::query::QueryEngine;
use graphzero_store::store::symbol_table::SymbolTable;

use super::super::QuerySurfaceRouter;
use super::super::helpers::{
    callers_not_found_response, checked_blob_hash, collect_call_edges, empty_capsule,
};
use super::super::types::*;

impl QuerySurfaceRouter {
    pub(super) fn callers(
        snapshot: &Snapshot,
        req: &QuerySurfaceRequest,
        budget: usize,
    ) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
        let callee = req
            .name
            .as_deref()
            .or(req.query.as_deref())
            .ok_or(QuerySurfaceError::MissingArgument("name"))?;
        let view = snapshot
            .global_view()
            .map_err(|_| QuerySurfaceError::SymbolNotFound(callee.into()))?;
        let table = SymbolTable::from_view(&view)
            .map_err(|_| QuerySurfaceError::SymbolNotFound(callee.into()))?;
        let Some(target_id) = table.get(callee) else {
            return callers_not_found_response(snapshot, callee, budget);
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
        let edges = collect_call_edges(snapshot, &table, &csr, &evidence, blob_hashes, target_id)?;
        let capsule = QueryEngine::warm(snapshot, callee, budget)
            .unwrap_or_else(|_| empty_capsule(callee, snapshot));
        Ok(QuerySurfaceResponse {
            schema_version: 1,
            surface: "callers".into(),
            coverage: Self::footer(snapshot, &capsule)?,
            symbol: Some(callee.to_string()),
            edges,
            ..Default::default()
        })
    }

    pub(super) fn callpath(
        snapshot: &Snapshot,
        req: &QuerySurfaceRequest,
        budget: usize,
    ) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
        let from = req
            .name
            .as_deref()
            .ok_or(QuerySurfaceError::MissingArgument("name"))?;
        let to = req
            .query
            .as_deref()
            .ok_or(QuerySurfaceError::MissingArgument("query"))?;
        let view = snapshot
            .global_view()
            .map_err(|_| QuerySurfaceError::SymbolNotFound(from.into()))?;
        let table = SymbolTable::from_view(&view)
            .map_err(|_| QuerySurfaceError::SymbolNotFound(from.into()))?;
        let Some(source_id) = table.get(from) else {
            return Err(QuerySurfaceError::SymbolNotFound(from.into()));
        };
        let Some(target_id) = table.get(to) else {
            return Err(QuerySurfaceError::SymbolNotFound(to.into()));
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
        let path_edges =
            shortest_call_path(&table, &csr, &evidence, blob_hashes, source_id, target_id)?;
        let capsule = QueryEngine::warm(snapshot, from, budget)
            .unwrap_or_else(|_| empty_capsule(from, snapshot));
        Ok(QuerySurfaceResponse {
            schema_version: 1,
            surface: "callpath".into(),
            coverage: Self::footer(snapshot, &capsule)?,
            symbol: Some(format!("{from}->{to}")),
            edges: path_edges,
            ..Default::default()
        })
    }
}

fn shortest_call_path(
    table: &SymbolTable,
    csr: &CsrAdjacency<'_>,
    evidence: &[graphzero_store::store::format::SpanEntry],
    blob_hashes: &[[u8; 32]],
    source_id: u32,
    target_id: u32,
) -> Result<Vec<GraphEdge>, QuerySurfaceError> {
    if source_id == target_id {
        return Ok(Vec::new());
    }
    let n = csr.num_symbols();
    if source_id as usize >= n || target_id as usize >= n {
        return Ok(Vec::new());
    }
    let Some(prev) = bfs_call_predecessors(csr, source_id, target_id, n) else {
        return Ok(Vec::new());
    };
    let chain = reconstruct_call_chain(&prev, source_id, target_id);
    materialize_call_path_edges(table, csr, evidence, blob_hashes, &chain)
}

fn bfs_call_predecessors(
    csr: &CsrAdjacency<'_>,
    source_id: u32,
    target_id: u32,
    n: usize,
) -> Option<Vec<Option<(u32, usize)>>> {
    let mut prev: Vec<Option<(u32, usize)>> = vec![None; n];
    let mut seen = vec![false; n];
    let mut queue = VecDeque::new();
    seen[source_id as usize] = true;
    queue.push_back(source_id);
    while let Some(src) = queue.pop_front() {
        let base = csr.edge_base(src);
        for (offset, edge) in csr.edges(src).enumerate() {
            if edge.kind != edge_kind::CALLS {
                continue;
            }
            let dst = edge.target;
            if dst as usize >= n || seen[dst as usize] {
                continue;
            }
            seen[dst as usize] = true;
            prev[dst as usize] = Some((src, base + offset));
            if dst == target_id {
                return Some(prev);
            }
            queue.push_back(dst);
        }
    }
    if seen[target_id as usize] {
        Some(prev)
    } else {
        None
    }
}

fn reconstruct_call_chain(
    prev: &[Option<(u32, usize)>],
    source_id: u32,
    target_id: u32,
) -> Vec<(u32, u32, usize)> {
    let mut chain = Vec::new();
    let mut current = target_id;
    while current != source_id {
        let Some((src, edge_idx)) = prev[current as usize] else {
            break;
        };
        chain.push((src, current, edge_idx));
        current = src;
    }
    chain.reverse();
    chain
}

fn materialize_call_path_edges(
    table: &SymbolTable,
    csr: &CsrAdjacency<'_>,
    evidence: &[graphzero_store::store::format::SpanEntry],
    blob_hashes: &[[u8; 32]],
    chain: &[(u32, u32, usize)],
) -> Result<Vec<GraphEdge>, QuerySurfaceError> {
    let mut out = Vec::with_capacity(chain.len());
    for &(src, dst, edge_idx) in chain {
        let edge = csr
            .edges(src)
            .nth(edge_idx - csr.edge_base(src))
            .ok_or(QuerySurfaceError::EvidenceMissing)?;
        let ev = evidence.get(edge_idx).copied().unwrap_or_default();
        let hash_hex = checked_blob_hash(blob_hashes, ev.blob_idx)?;
        let evidence_ref = graphzero_store::store::refs::blob_span_ref(&hash_hex, ev.start, ev.end);
        if evidence_ref.is_empty() {
            return Err(QuerySurfaceError::EvidenceMissing);
        }
        out.push(GraphEdge {
            kind: "calls".into(),
            to: table.name(dst).unwrap_or("").to_string(),
            from: Some(table.name(src).unwrap_or("").to_string()),
            confidence: edge.confidence as f64 / 255.0,
            evidence_ref: evidence_ref.clone(),
            source: "tier_a".into(),
        });
        if let Some(dst_name) = table.name(dst) {
            let _ = graphzero_store::link_emitted_symbol_view(
                graphzero_store::EntityViewKind::Trace,
                dst_name,
                &evidence_ref,
            );
        }
    }
    Ok(out)
}
