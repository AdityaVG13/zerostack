//! Fidelity ladder: L1–L4 graph view renders with token envelopes.

use graphzero_store::Snapshot;
use graphzero_store::store::format::symbol_kind;
use graphzero_store::store::query::{QueryEngine, tokens_for_str};
use graphzero_store::store::symbol_table::SymbolTable;
use serde_json::{Value, json};

use crate::blast::{blast_radius_with_depth, retrieval_neighborhood};

use super::QuerySurfaceRouter;
use super::helpers::{empty_capsule, outline_kind_name};
use super::types::*;

pub const L2_TOKEN_ENVELOPE: usize = 500;
pub const L3_TOKEN_ENVELOPE: usize = 2000;

impl QuerySurfaceRouter {
    pub(super) fn rg_l1(
        snapshot: &Snapshot,
        req: &QuerySurfaceRequest,
        budget: usize,
    ) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
        let name = req
            .name
            .as_deref()
            .or(req.query.as_deref())
            .ok_or(QuerySurfaceError::MissingArgument("name"))?;
        let (symbol, kind) = resolve_symbol_kind(snapshot, name)?;
        let capsule = QueryEngine::warm(snapshot, name, budget)
            .unwrap_or_else(|_| empty_capsule(name, snapshot));
        Ok(QuerySurfaceResponse {
            schema_version: 1,
            surface: "rg_l1".into(),
            coverage: Self::footer(snapshot, &capsule)?,
            symbol: Some(symbol),
            rows: vec![json!({ "symbol": name, "kind": kind })],
            ..Default::default()
        })
    }

    pub(super) fn rg_l2(
        snapshot: &Snapshot,
        req: &QuerySurfaceRequest,
        _budget: usize,
    ) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
        let _ = req;
        let view = snapshot
            .global_view()
            .map_err(|_| QuerySurfaceError::EvidenceMissing)?;
        let table =
            SymbolTable::from_view(&view).map_err(|_| QuerySurfaceError::EvidenceMissing)?;
        let mut outline = Vec::new();
        let mut rows = Vec::new();
        for id in 0..table.len() as u32 {
            let Some(name) = table.name(id) else {
                continue;
            };
            let kind_code = table
                .entry(id)
                .map(|e| e.kind)
                .unwrap_or(symbol_kind::FUNCTION);
            let kind = outline_kind_name(kind_code);
            outline.push(OutlineItem {
                name: name.to_string(),
                kind: kind.clone(),
                evidence_ref: format!("node/{name}"),
                source: "tier_a".into(),
                start_line: None,
                end_line: None,
            });
            rows.push(json!({ "module": module_of(name), "symbol": name, "kind": kind }));
        }
        outline.sort_by(|a, b| a.name.cmp(&b.name));
        rows.sort_by(|a, b| {
            let sa = a.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
            let sb = b.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
            sa.cmp(sb)
        });
        // Soft envelope: drop lowest-priority rows if over L2 budget tokens.
        while tokens_for_str(&serde_json::to_string(&rows).unwrap_or_default()) > L2_TOKEN_ENVELOPE
            && rows.len() > 1
        {
            rows.pop();
            outline.pop();
        }
        let capsule = empty_capsule("rg_l2", snapshot);
        let truncated =
            tokens_for_str(&serde_json::to_string(&rows).unwrap_or_default()) >= L2_TOKEN_ENVELOPE;
        Ok(QuerySurfaceResponse {
            schema_version: 1,
            surface: "rg_l2".into(),
            coverage: Self::footer(snapshot, &capsule)?,
            outline,
            rows,
            truncated: Some(truncated),
            ..Default::default()
        })
    }

    pub(super) fn rg_l3(
        snapshot: &Snapshot,
        req: &QuerySurfaceRequest,
        budget: usize,
    ) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
        let name = req
            .name
            .as_deref()
            .or(req.query.as_deref())
            .ok_or(QuerySurfaceError::MissingArgument("name"))?;
        let depth = budget.min(3).max(1) as u32;
        let report = blast_radius_with_depth(snapshot, name, budget.max(1), depth)
            .map_err(|_| QuerySurfaceError::SymbolNotFound(name.into()))?;
        let mut rows: Vec<Value> = report
            .break_sites
            .iter()
            .map(|site| {
                json!({
                    "symbol": site.symbol,
                    "hop": site.hop,
                    "evidence_ref": site.evidence_ref,
                })
            })
            .collect();
        let mut edges: Vec<GraphEdge> = report
            .break_sites
            .iter()
            .flat_map(|site| {
                site.provenance.iter().map(|p| GraphEdge {
                    kind: p.edge_kind.clone(),
                    to: p.to_symbol.clone(),
                    from: Some(p.from_symbol.clone()),
                    confidence: site.confidence,
                    evidence_ref: p.evidence_ref.clone(),
                    source: "tier_a".into(),
                })
            })
            .collect();
        let mut truncated = false;
        while tokens_for_str(&serde_json::to_string(&rows).unwrap_or_default()) > L3_TOKEN_ENVELOPE
            && !rows.is_empty()
        {
            rows.pop();
            truncated = true;
        }
        if truncated {
            edges.truncate(rows.len().saturating_mul(2));
        }
        let capsule = QueryEngine::warm(snapshot, name, budget)
            .unwrap_or_else(|_| empty_capsule(name, snapshot));
        Ok(QuerySurfaceResponse {
            schema_version: 1,
            surface: "rg_l3".into(),
            coverage: Self::footer(snapshot, &capsule)?,
            symbol: Some(name.to_string()),
            rows,
            edges,
            truncated: Some(truncated),
            ..Default::default()
        })
    }

    pub(super) fn rg_l4(
        snapshot: &Snapshot,
        req: &QuerySurfaceRequest,
        budget: usize,
    ) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
        let name = req
            .name
            .as_deref()
            .or(req.query.as_deref())
            .ok_or(QuerySurfaceError::MissingArgument("name"))?;
        let edge_budget = budget.max(1);
        let hops = 2u32;
        let neighborhood = retrieval_neighborhood(snapshot, &[name.to_string()], hops, edge_budget)
            .map_err(|_| QuerySurfaceError::SymbolNotFound(name.into()))?;
        let rows: Vec<Value> = neighborhood
            .nodes
            .iter()
            .map(|n| json!({ "symbol": n.symbol, "hop": n.hop, "seed": n.seed }))
            .collect();
        let edges: Vec<GraphEdge> = neighborhood
            .edges
            .iter()
            .map(|e| GraphEdge {
                kind: e.edge_kind.clone(),
                to: e.to_symbol.clone(),
                from: Some(e.from_symbol.clone()),
                confidence: 1.0,
                evidence_ref: e.evidence_ref.clone(),
                source: "tier_a".into(),
            })
            .collect();
        // L4 is budget-bound: hitting the edge budget is an explicit truncation.
        let truncated = neighborhood.edges.len() >= edge_budget;
        let capsule = QueryEngine::warm(snapshot, name, budget)
            .unwrap_or_else(|_| empty_capsule(name, snapshot));
        Ok(QuerySurfaceResponse {
            schema_version: 1,
            surface: "rg_l4".into(),
            coverage: Self::footer(snapshot, &capsule)?,
            symbol: Some(name.to_string()),
            rows,
            edges,
            truncated: Some(truncated),
            refs_footer: neighborhood
                .edges
                .iter()
                .map(|e| e.evidence_ref.clone())
                .collect(),
            ..Default::default()
        })
    }
}

fn resolve_symbol_kind(
    snapshot: &Snapshot,
    name: &str,
) -> Result<(String, String), QuerySurfaceError> {
    let view = snapshot
        .global_view()
        .map_err(|_| QuerySurfaceError::SymbolNotFound(name.into()))?;
    let table = SymbolTable::from_view(&view)
        .map_err(|_| QuerySurfaceError::SymbolNotFound(name.into()))?;
    let Some(id) = table.get(name) else {
        return Err(QuerySurfaceError::SymbolNotFound(name.into()));
    };
    let kind_code = table
        .entry(id)
        .map(|e| e.kind)
        .unwrap_or(symbol_kind::FUNCTION);
    Ok((name.to_string(), outline_kind_name(kind_code)))
}

fn module_of(symbol: &str) -> String {
    match symbol.rsplit_once("::") {
        Some((module, _)) => module.to_string(),
        None => "<root>".into(),
    }
}
