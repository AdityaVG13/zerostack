//! Contract footprints used to detect reservation overlap.

use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Context, Result};
use graphzero_store::Snapshot;
use graphzero_store::parse_intent;
use graphzero_store::store::csr::{CsrAdjacency, ReverseIndex, edge_kind};
use graphzero_store::store::format::SpanEntry;
use graphzero_store::store::refs::blob_span_ref;
use graphzero_store::store::symbol_table::SymbolTable;
use serde::{Deserialize, Serialize};

const MAX_REVERSE_CALL_HOPS: u32 = 4;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FootprintSnapshot {
    pub footprint_ref: String,
    pub target_symbol: String,
    pub contract_nodes: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub tier_a_percent: f64,
}

fn parse_target_symbol(intent: &str) -> Result<String> {
    let parsed = parse_intent(intent);
    parsed
        .target_symbol
        .ok_or_else(|| anyhow::anyhow!(parsed.error.unwrap_or_else(|| "parse failed".into())))
}

fn reverse_call_depth(csr: &CsrAdjacency, target_id: u32) -> HashMap<u32, u32> {
    let rev = ReverseIndex::build(csr, Some(edge_kind::CALLS));
    let mut depth: HashMap<u32, u32> = HashMap::new();
    let mut queue = VecDeque::new();
    queue.push_back(target_id);
    depth.insert(target_id, 0);

    while let Some(node) = queue.pop_front() {
        let hop = depth[&node];
        if hop >= MAX_REVERSE_CALL_HOPS {
            continue;
        }
        for &(src, _) in rev.callers(node) {
            if depth.contains_key(&src) {
                continue;
            }
            depth.insert(src, hop + 1);
            queue.push_back(src);
        }
    }
    depth
}

fn blob_hash_hex(blob_hashes: &[[u8; 32]], blob_idx: u32) -> String {
    blob_hashes
        .get(blob_idx as usize)
        .map(graphzero_store::fast_hex_32)
        .unwrap_or_default()
}

fn best_evidence_for_node(
    rev: &ReverseIndex,
    evidence: &[SpanEntry],
    blob_hashes: &[[u8; 32]],
    node_id: u32,
    node_ref: &str,
) -> String {
    let mut best_ref = node_ref.to_string();
    for &(_src, edge_idx) in rev.callers(node_id) {
        let ev: SpanEntry = evidence.get(edge_idx as usize).copied().unwrap_or_default();
        let hash_hex = blob_hash_hex(blob_hashes, ev.blob_idx);
        let span = blob_span_ref(&hash_hex, ev.start, ev.end);
        if !span.is_empty() {
            best_ref = span;
        }
    }
    best_ref
}

fn contract_nodes_and_evidence(
    depth: &HashMap<u32, u32>,
    table: &SymbolTable,
    rev: &ReverseIndex,
    evidence: &[SpanEntry],
    blob_hashes: &[[u8; 32]],
    target_id: u32,
) -> (Vec<String>, Vec<String>) {
    let mut contract_nodes = Vec::new();
    let mut evidence_refs = Vec::new();
    for (id, hop) in depth {
        let sym = table.name(*id).unwrap_or("").to_string();
        if sym.is_empty() {
            continue;
        }
        let node_ref = format!("node/{sym}");
        contract_nodes.push(node_ref.clone());
        let best_ref = best_evidence_for_node(rev, evidence, blob_hashes, *id, &node_ref);
        if *hop > 0 || *id == target_id {
            evidence_refs.push(best_ref);
        }
    }
    contract_nodes.sort();
    contract_nodes.dedup();
    evidence_refs.sort();
    evidence_refs.dedup();
    (contract_nodes, evidence_refs)
}

fn with_resolved_target_graph<R>(
    snapshot: &Snapshot,
    target: &str,
    run: impl FnOnce(
        &SymbolTable<'_>,
        &CsrAdjacency<'_>,
        &ReverseIndex,
        &[SpanEntry],
        &[[u8; 32]],
        u32,
    ) -> Result<R>,
) -> Result<R> {
    let view = snapshot.global_view().context("global view")?;
    let table = SymbolTable::from_view(&view).context("symbol table")?;
    let target_id = table
        .get(target)
        .ok_or_else(|| anyhow::anyhow!("symbol not found: {target}"))?;
    let csr = CsrAdjacency::new(view.edges().context("edges")?);
    let rev = ReverseIndex::build(&csr, Some(edge_kind::CALLS));
    let evidence = view.edge_evidence().context("evidence")?;
    let blob_hashes = view.coverage().context("coverage")?.blob_hashes;
    run(&table, &csr, &rev, &evidence, blob_hashes, target_id)
}

fn tier_a_percent_from_coverage(coverage: Option<graphzero_store::CoverageBitmap>) -> f64 {
    let Some(bm) = coverage else {
        return 0.0;
    };
    let blob_count = bm.blob_count();
    if blob_count == 0 {
        return 0.0;
    }
    (bm.tier_a_count() as f64 / blob_count as f64) * 100.0
}

fn tier_a_percent(snapshot: &Snapshot) -> f64 {
    let coverage = snapshot
        .global_view()
        .ok()
        .and_then(|v| v.coverage().ok())
        .map(|c| graphzero_store::CoverageBitmap::from_packed(c.blob_hashes.len(), c.bits));
    tier_a_percent_from_coverage(coverage)
}

pub fn contract_footprint(snapshot: &Snapshot, intent: &str) -> Result<FootprintSnapshot> {
    let target = parse_target_symbol(intent)?;
    with_resolved_target_graph(
        snapshot,
        &target,
        |table, csr, rev, evidence, blob_hashes, target_id| {
            let depth = reverse_call_depth(csr, target_id);
            let (contract_nodes, evidence_refs) =
                contract_nodes_and_evidence(&depth, table, rev, evidence, blob_hashes, target_id);
            Ok(FootprintSnapshot {
                footprint_ref: format!("footprint/{target}"),
                target_symbol: target.clone(),
                contract_nodes,
                evidence_refs,
                tier_a_percent: tier_a_percent(snapshot),
            })
        },
    )
}

fn intent_text_for_op(op: &crate::schema::IntentOperation) -> String {
    op.intent_text.clone().unwrap_or_else(|| {
        op.target_symbol
            .as_ref()
            .map(|s| format!("change signature of {s}"))
            .unwrap_or_else(|| format!("change {}", op.kind))
    })
}

fn merge_footprint_into(
    nodes: &mut HashSet<String>,
    evidence: &mut HashSet<String>,
    tier_a: &mut f64,
    footprint_ref: &mut String,
    target: &mut String,
    fp: &FootprintSnapshot,
) {
    let incoming_tier_a = if fp.tier_a_percent.is_finite() {
        fp.tier_a_percent.clamp(0.0, 100.0)
    } else {
        0.0
    };
    *tier_a = tier_a.min(incoming_tier_a);
    if footprint_ref.is_empty() {
        *footprint_ref = fp.footprint_ref.clone();
        *target = fp.target_symbol.clone();
    }
    for n in &fp.contract_nodes {
        nodes.insert(n.clone());
    }
    for e in &fp.evidence_refs {
        evidence.insert(e.clone());
    }
}

pub fn footprint_from_intent_ops(
    snapshot: &Snapshot,
    ops: &[crate::schema::IntentOperation],
) -> Result<FootprintSnapshot> {
    let mut nodes = HashSet::new();
    let mut evidence = HashSet::new();
    let mut tier_a = 100.0f64;
    let mut target = String::new();
    let mut footprint_ref = String::new();
    for op in ops {
        let intent = intent_text_for_op(op);
        let fp = contract_footprint(snapshot, &intent)?;
        merge_footprint_into(
            &mut nodes,
            &mut evidence,
            &mut tier_a,
            &mut footprint_ref,
            &mut target,
            &fp,
        );
    }
    if footprint_ref.is_empty() {
        footprint_ref = "footprint/unknown".into();
    }
    let mut contract_nodes: Vec<_> = nodes.into_iter().collect();
    contract_nodes.sort();
    let mut evidence_refs: Vec<_> = evidence.into_iter().collect();
    evidence_refs.sort();
    Ok(FootprintSnapshot {
        footprint_ref,
        target_symbol: target,
        contract_nodes,
        evidence_refs,
        tier_a_percent: tier_a,
    })
}
