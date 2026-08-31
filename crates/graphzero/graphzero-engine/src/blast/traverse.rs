//! Blast-radius graph traversal, retrieval neighborhood, covering tests, silent risk.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use rayon::prelude::*;

use crate::accounting::accounting_for_evidence_refs;

use super::parse::parse_intent;
use super::types::{
    BLAST_SCHEMA_VERSION, BlastCoverageFooter, BlastError, BlastRadiusCapsule, BreakSite,
    CoveringTest, EdgeProvenance, RetrievalEdge, RetrievalNeighborhood, RetrievalNode, SilentRisk,
};
use graphzero_coverage::confidence_algebra::blast_finding_score;
use graphzero_coverage::{CoverageCertificate, Gap, GapReason, now_timestamp};
use graphzero_store::store::csr::{CsrAdjacency, Edge, ReverseIndex, edge_kind};
use graphzero_store::store::format::SpanEntry;
use graphzero_store::store::query::QueryEngine;
use graphzero_store::store::refs::blob_span_ref;
use graphzero_store::store::symbol_table::SymbolTable;
use graphzero_store::{Snapshot, Tier};
use serde_json::{Value, json};

pub(super) fn hex_blob(blob_hashes: &[[u8; 32]], idx: u32) -> Result<String, BlastError> {
    graphzero_store::hex_blob_hash(blob_hashes, idx).map_err(|err| BlastError::MalformedIndex {
        blob_idx: err.blob_idx,
        blob_hash_count: err.blob_hash_count,
    })
}

fn build_certificate(tier_a: f64, tier_b: f64, tier_c: f64) -> CoverageCertificate {
    let mut cert = CoverageCertificate::new(now_timestamp());
    cert.tier_a_pct = tier_a;
    cert.tier_b_pct = tier_b;
    cert.tier_c_pct = tier_c;
    cert.freshness_verified = true;
    if tier_b < 100.0 {
        cert.gaps.push(Gap::new(
            graphzero_store::BlobId::new("tier_b_partial"),
            Tier::B,
            GapReason::NotIndexed,
        ));
    }
    if tier_c < 100.0 {
        cert.gaps.push(Gap::new(
            graphzero_store::BlobId::new("tier_c_partial"),
            Tier::C,
            GapReason::NotIndexed,
        ));
    }
    cert
}

fn certificate_json(cert: &CoverageCertificate) -> Value {
    let gaps: Vec<Value> = cert
        .gaps
        .iter()
        .map(|g| {
            json!({
                "blob_id": g.blob_id.0,
                "tier": g.tier.to_string(),
                "reason": g.reason.to_string(),
            })
        })
        .collect();
    json!({
        "tier_a_pct": cert.tier_a_pct,
        "tier_b_pct": cert.tier_b_pct,
        "tier_c_pct": cert.tier_c_pct,
        "freshness_verified": cert.freshness_verified,
        "gaps": gaps,
        "break_site_score": "path_min_edge_confidence_times_tier_a",
        "silent_risk": {
            "class": "heuristic",
            "not": "proof_of_risk"
        },
    })
}

#[tracing::instrument(skip_all, fields(budget, intent_len = intent.len()))]
pub fn blast_radius(
    snapshot: &Snapshot,
    intent: &str,
    budget: usize,
) -> Result<BlastRadiusCapsule, BlastError> {
    blast_radius_with_depth(snapshot, intent, budget, 4)
}

#[tracing::instrument(skip_all, fields(budget, max_depth, intent_len = intent.len()))]
pub fn blast_radius_with_depth(
    snapshot: &Snapshot,
    intent: &str,
    budget: usize,
    max_depth: u32,
) -> Result<BlastRadiusCapsule, BlastError> {
    let parsed = parse_intent(intent);
    let target = parsed
        .target_symbol
        .clone()
        .ok_or_else(|| BlastError::Parse(parsed.error.unwrap_or_else(|| "parse failed".into())))?;
    let target_ref = parsed
        .target_ref
        .clone()
        .unwrap_or_else(|| format!("node/{target}"));
    let _ = graphzero_store::link_emitted_symbol_view(
        graphzero_store::EntityViewKind::Blast,
        &target,
        &target_ref,
    );

    let view = snapshot
        .global_view()
        .map_err(|e| BlastError::Store(e.to_string()))?;
    let table = SymbolTable::from_view(&view).map_err(|e| BlastError::Store(e.to_string()))?;
    let target_id = table
        .get(&target)
        .ok_or_else(|| BlastError::SymbolNotFound(target.clone()))?;

    let csr = CsrAdjacency::new(view.edges().map_err(|e| BlastError::Store(e.to_string()))?);
    let evidence = view
        .edge_evidence()
        .map_err(|e| BlastError::Store(e.to_string()))?;
    let blob_hashes = view
        .coverage()
        .map_err(|e| BlastError::Store(e.to_string()))?
        .blob_hashes;
    let capsule = QueryEngine::warm(snapshot, &target, budget)
        .map_err(|e| BlastError::Store(e.to_string()))?;
    let tier_a = capsule.tier_a * 100.0;
    let tier_b = capsule.tier_b * 100.0;
    let tier_c = capsule.tier_c * 100.0;

    let mut break_sites = enumerate_break_sites(
        &table,
        &csr,
        snapshot
            .blast_reverse_index()
            .map_err(|e| BlastError::Store(e.to_string()))?,
        &evidence,
        blob_hashes,
        target_id,
        tier_a,
        max_depth,
    )?;
    let frecency = crate::query_surface::frecency::RankCtx::load(snapshot);
    break_sites.sort_by(|a, b| {
        frecency
            .score(&a.symbol, &a.evidence_ref)
            .total_cmp(&frecency.score(&b.symbol, &b.evidence_ref))
            .then_with(|| b.confidence.total_cmp(&a.confidence))
            .then_with(|| a.symbol.cmp(&b.symbol))
            .then_with(|| a.hop.cmp(&b.hop))
    });
    // Snap-to-file targets: every blast hit carries the canonical path
    // target, intent metadata, and an inlined window for the top hits.
    for (rank, site) in break_sites.iter_mut().enumerate() {
        let Some(hit) = graphzero_store::file_target_for_evidence(
            snapshot,
            &site.evidence_ref,
            "blast",
            Some(&site.symbol),
            rank < graphzero_store::TARGET_INLINE_TOP_HITS,
        ) else {
            continue;
        };
        site.target = Some(hit.target);
        site.kind = Some(hit.kind);
        site.sym = Some(hit.symbol);
        if !hit.content.is_empty() {
            site.content = Some(hit.content);
        }
    }

    let impacted_symbols: Vec<String> =
        break_sites.iter().map(|site| site.symbol.clone()).collect();
    let covering_tests = collect_covering_tests(snapshot, &target, &impacted_symbols);
    let silent_risk = collect_silent_risk(snapshot)?;

    let cert = build_certificate(tier_a, tier_b, tier_c);

    let mut accounting_refs: Vec<String> = capsule
        .matches
        .iter()
        .find(|m| m.name == target)
        .map(|m| m.defs.iter().map(|d| d.evidence_ref.clone()).collect())
        .unwrap_or_default();
    accounting_refs.extend(break_sites.iter().map(|site| site.evidence_ref.clone()));
    accounting_refs.extend(covering_tests.iter().map(|test| test.evidence_ref.clone()));
    let accounting = accounting_for_evidence_refs(
        snapshot,
        "blast_unaffected_files",
        accounting_refs.iter(),
        "blast selected target, break-site, and covering-test evidence; other indexed files are unaffected by this intent",
    );

    Ok(BlastRadiusCapsule {
        schema_version: BLAST_SCHEMA_VERSION,
        intent: intent.to_string(),
        target_ref,
        target_symbol: target,
        break_sites,
        covering_tests,
        silent_risk,
        coverage: BlastCoverageFooter {
            tier_a_percent: tier_a,
            tier_b_percent: tier_b,
            tier_c_percent: tier_c,
            freshness_verified: true,
            snapshot_id: snapshot.entry.snapshot_id,
        },
        certificate: certificate_json(&cert),
        accounting,
        next_cursor: None,
    })
}

fn enumerate_break_sites(
    table: &SymbolTable,
    csr: &CsrAdjacency<'_>,
    reverse: &ReverseIndex,
    evidence: &[SpanEntry],
    blob_hashes: &[[u8; 32]],
    target_id: u32,
    tier_a: f64,
    max_depth: u32,
) -> Result<Vec<BreakSite>, BlastError> {
    let mut depth: HashMap<u32, u32> = HashMap::new();
    let mut predecessor: HashMap<u32, (u32, usize)> = HashMap::new();
    let mut queue: VecDeque<u32> = VecDeque::new();
    queue.push_back(target_id);
    depth.insert(target_id, 0);

    while let Some(node) = queue.pop_front() {
        let hop = depth[&node];
        if hop >= max_depth {
            continue;
        }
        for &(src, edge_idx) in reverse.callers(node) {
            if depth.contains_key(&src) {
                continue;
            }
            depth.insert(src, hop + 1);
            predecessor.insert(src, (node, edge_idx as usize));
            queue.push_back(src);
        }
    }

    let mut break_sites = Vec::new();
    // Collect HashMap entries into a deterministic order (hop, id) before
    // building break_sites so the pre-sort vector is stable across runs.
    let mut depth_entries: Vec<(u32, u32)> = depth.into_iter().collect();
    depth_entries.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    for (id, hop) in depth_entries {
        if id == target_id || hop == 0 {
            continue;
        }
        let sym = table.name(id).unwrap_or("").to_string();
        let incoming = reverse.callers(id);
        let (best_conf, best_edge_idx) =
            select_best_incoming_edge(incoming.iter().filter_map(|&(src, edge_idx)| {
                incoming_edge_confidence(csr, src, edge_idx as usize)
            }));
        let best_ref = if let Some(edge_idx) = best_edge_idx {
            let ev = evidence.get(edge_idx).copied().unwrap_or_default();
            let hash_hex = hex_blob(blob_hashes, ev.blob_idx)?;
            blob_span_ref(&hash_hex, ev.start, ev.end)
        } else {
            format!("node/{sym}")
        };
        // Score is path-min along BFS provenance, not best incoming at this node.
        // `best_conf` / `best_edge_idx` only choose which evidence_ref to display.
        let path_confs = path_edge_confidences(csr, &predecessor, id, target_id);
        let score = if path_confs.is_empty() {
            blast_finding_score(&[best_conf], tier_a)
        } else {
            blast_finding_score(&path_confs, tier_a)
        };
        let provenance = provenance_path(
            table,
            csr,
            evidence,
            blob_hashes,
            &predecessor,
            id,
            target_id,
        )?;
        break_sites.push(BreakSite {
            symbol: sym.clone(),
            evidence_ref: best_ref.clone(),
            confidence: score,
            tier: "A".into(),
            hop,
            provenance,
            target: None,
            kind: None,
            sym: None,
            content: None,
        });
        let _ = graphzero_store::link_emitted_symbol_view(
            graphzero_store::EntityViewKind::Blast,
            &sym,
            &best_ref,
        );
    }

    break_sites.sort_by(|a, b| {
        b.confidence
            .total_cmp(&a.confidence)
            .then_with(|| a.symbol.cmp(&b.symbol))
            .then_with(|| a.hop.cmp(&b.hop))
    });
    Ok(break_sites)
}

fn select_best_incoming_edge<I>(incoming: I) -> (f64, Option<usize>)
where
    I: IntoIterator<Item = (usize, u8)>,
{
    let mut best_confidence = 0u8;
    let mut best_edge_idx = None;
    for (edge_idx, confidence) in incoming {
        if best_edge_idx.is_none() || confidence >= best_confidence {
            best_confidence = confidence;
            best_edge_idx = Some(edge_idx);
        }
    }
    (best_confidence as f64 / 255.0, best_edge_idx)
}

fn incoming_edge_confidence(
    csr: &CsrAdjacency<'_>,
    src: u32,
    edge_idx: usize,
) -> Option<(usize, u8)> {
    let base = csr.edge_base(src);
    let offset = edge_idx.checked_sub(base)?;
    csr.edges(src)
        .nth(offset)
        .filter(|edge| is_blast_edge(edge.kind))
        .map(|edge| (edge_idx, edge.confidence))
}

/// Confidences on the BFS tree path from `node` back to `target_id`.
/// Used for path-min scoring; missing hops are omitted (min over observed edges).
fn path_edge_confidences(
    csr: &CsrAdjacency<'_>,
    predecessor: &HashMap<u32, (u32, usize)>,
    mut node: u32,
    target_id: u32,
) -> Vec<f64> {
    let mut confs = Vec::new();
    while node != target_id {
        let Some(&(next, edge_idx)) = predecessor.get(&node) else {
            break;
        };
        if let Some((_, confidence)) = incoming_edge_confidence(csr, node, edge_idx) {
            confs.push(confidence as f64 / 255.0);
        }
        node = next;
    }
    confs
}

fn provenance_path(
    table: &SymbolTable,
    csr: &CsrAdjacency<'_>,
    evidence: &[SpanEntry],
    blob_hashes: &[[u8; 32]],
    predecessor: &HashMap<u32, (u32, usize)>,
    mut node: u32,
    target_id: u32,
) -> Result<Vec<EdgeProvenance>, BlastError> {
    let mut out = Vec::new();
    while node != target_id {
        let Some(&(next, edge_idx)) = predecessor.get(&node) else {
            break;
        };
        let edge = csr
            .edges(node)
            .nth(edge_idx - csr.edge_base(node))
            .unwrap_or(Edge {
                target: next,
                kind: edge_kind::CALLS,
                confidence: 0,
            });
        let ev = evidence.get(edge_idx).copied().unwrap_or_default();
        let hash_hex = hex_blob(blob_hashes, ev.blob_idx)?;
        let evidence_ref = blob_span_ref(&hash_hex, ev.start, ev.end);
        let from_symbol = table.name(node).unwrap_or("").to_string();
        let to_symbol = table.name(next).unwrap_or("").to_string();
        out.push(EdgeProvenance {
            kind: classify_cross_repo_edge(&from_symbol, &to_symbol),
            edge_kind: edge_kind_name(edge.kind).to_string(),
            from_symbol,
            to_symbol,
            evidence_ref,
        });
        node = next;
    }
    Ok(out)
}

fn edge_kind_name(kind: u8) -> &'static str {
    if kind == edge_kind::CALLS {
        "calls"
    } else if kind == edge_kind::REFS {
        "refs"
    } else if kind == edge_kind::IMPORTS {
        "imports"
    } else {
        "other"
    }
}

fn classify_cross_repo_edge(from: &str, to: &str) -> String {
    if from.starts_with("<manifest:") && to.starts_with("<workspace-member:") {
        "workspace_edge".into()
    } else if from.starts_with("<api-surface:") || to.starts_with("<api-surface:") {
        "api_surface_edge".into()
    } else {
        "symbol_edge".into()
    }
}

fn is_blast_edge(kind: u8) -> bool {
    kind == edge_kind::CALLS || kind == edge_kind::REFS || kind == edge_kind::IMPORTS
}

pub fn retrieval_neighborhood(
    snapshot: &Snapshot,
    seeds: &[String],
    max_hops: u32,
    budget: usize,
) -> Result<RetrievalNeighborhood, BlastError> {
    if seeds.is_empty() {
        return Err(BlastError::Parse(
            "at least one seed symbol is required".into(),
        ));
    }
    let loaded = load_retrieval_graph(snapshot)?;
    let (seed_ids, normalized_seeds) = resolve_retrieval_seeds(&loaded.table, seeds)?;
    let (hops, edges) = walk_retrieval_neighborhood(&loaded, &seed_ids, max_hops, budget.max(1))?;
    Ok(finish_retrieval_neighborhood(
        &loaded.table,
        normalized_seeds,
        max_hops,
        &seed_ids,
        hops,
        edges,
    ))
}

fn blast_store_err(err: impl ToString) -> BlastError {
    BlastError::Store(err.to_string())
}

struct RetrievalGraph<'a> {
    table: SymbolTable<'a>,
    csr: CsrAdjacency<'a>,
    reverse: &'a ReverseIndex,
    evidence: std::borrow::Cow<'a, [SpanEntry]>,
    blob_hashes: &'a [[u8; 32]],
}

fn load_retrieval_graph(snapshot: &Snapshot) -> Result<RetrievalGraph<'_>, BlastError> {
    let view = snapshot.global_view().map_err(blast_store_err)?;
    let table = SymbolTable::from_view(&view).map_err(blast_store_err)?;
    let csr = CsrAdjacency::new(view.edges().map_err(blast_store_err)?);
    let reverse = snapshot.reverse_index().map_err(blast_store_err)?;
    let evidence = view.edge_evidence().map_err(blast_store_err)?;
    let blob_hashes = view.coverage().map_err(blast_store_err)?.blob_hashes;
    Ok(RetrievalGraph {
        table,
        csr,
        reverse,
        evidence,
        blob_hashes,
    })
}

fn resolve_retrieval_seeds(
    table: &SymbolTable<'_>,
    seeds: &[String],
) -> Result<(Vec<u32>, Vec<String>), BlastError> {
    let mut seed_ids = Vec::with_capacity(seeds.len());
    let mut normalized_seeds = Vec::with_capacity(seeds.len());
    for seed in seeds {
        let id = table
            .get(seed)
            .ok_or_else(|| BlastError::SymbolNotFound(seed.clone()))?;
        seed_ids.push(id);
        normalized_seeds.push(seed.clone());
    }
    Ok((seed_ids, normalized_seeds))
}

fn seed_retrieval_queue(seed_ids: &[u32]) -> (HashMap<u32, u32>, VecDeque<u32>) {
    let mut hops = HashMap::new();
    let mut queue = VecDeque::new();
    for id in seed_ids {
        if hops.insert(*id, 0).is_none() {
            queue.push_back(*id);
        }
    }
    (hops, queue)
}

fn walk_retrieval_neighborhood(
    graph: &RetrievalGraph<'_>,
    seed_ids: &[u32],
    max_hops: u32,
    edge_budget: usize,
) -> Result<(HashMap<u32, u32>, Vec<RetrievalEdge>), BlastError> {
    let (mut hops, mut queue) = seed_retrieval_queue(seed_ids);
    let mut edge_keys = BTreeSet::new();
    let mut edges = Vec::new();
    while let Some(node) = queue.pop_front() {
        let hop = hops[&node];
        if hop >= max_hops || edges.len() >= edge_budget {
            continue;
        }
        expand_retrieval_outgoing(
            &graph.csr,
            &graph.table,
            &graph.evidence,
            graph.blob_hashes,
            node,
            hop,
            edge_budget,
            &mut hops,
            &mut queue,
            &mut edges,
            &mut edge_keys,
        )?;
        if edges.len() >= edge_budget {
            break;
        }
        expand_retrieval_incoming(
            &graph.csr,
            graph.reverse,
            &graph.table,
            &graph.evidence,
            graph.blob_hashes,
            node,
            hop,
            edge_budget,
            &mut hops,
            &mut queue,
            &mut edges,
            &mut edge_keys,
        )?;
        if edges.len() >= edge_budget {
            break;
        }
    }
    Ok((hops, edges))
}

fn cmp_retrieval_node(a: &RetrievalNode, b: &RetrievalNode) -> std::cmp::Ordering {
    a.hop.cmp(&b.hop).then_with(|| a.symbol.cmp(&b.symbol))
}

fn cmp_retrieval_edge(a: &RetrievalEdge, b: &RetrievalEdge) -> std::cmp::Ordering {
    a.hop
        .cmp(&b.hop)
        .then_with(|| a.from_symbol.cmp(&b.from_symbol))
        .then_with(|| a.to_symbol.cmp(&b.to_symbol))
        .then_with(|| a.edge_kind.cmp(&b.edge_kind))
}

fn finish_retrieval_neighborhood(
    table: &SymbolTable<'_>,
    seeds: Vec<String>,
    max_hops: u32,
    seed_ids: &[u32],
    hops: HashMap<u32, u32>,
    mut edges: Vec<RetrievalEdge>,
) -> RetrievalNeighborhood {
    let seed_set: BTreeSet<u32> = seed_ids.iter().copied().collect();
    let mut nodes: Vec<_> = hops
        .into_iter()
        .map(|(id, hop)| RetrievalNode {
            symbol: table.name(id).unwrap_or("").to_string(),
            seed: seed_set.contains(&id),
            hop,
        })
        .collect();
    nodes.sort_by(cmp_retrieval_node);
    edges.sort_by(cmp_retrieval_edge);
    RetrievalNeighborhood {
        schema_version: BLAST_SCHEMA_VERSION,
        seeds,
        max_hops,
        nodes,
        edges,
    }
}

#[allow(clippy::too_many_arguments)]
fn expand_retrieval_outgoing(
    csr: &CsrAdjacency<'_>,
    table: &SymbolTable,
    evidence: &[SpanEntry],
    blob_hashes: &[[u8; 32]],
    node: u32,
    hop: u32,
    edge_budget: usize,
    hops: &mut HashMap<u32, u32>,
    queue: &mut VecDeque<u32>,
    edges: &mut Vec<RetrievalEdge>,
    edge_keys: &mut BTreeSet<(u32, u32, u8)>,
) -> Result<(), BlastError> {
    for edge_idx in outgoing_retrieval_edge_indices(csr, node) {
        let Some(edge) = edge_by_global_index(csr, node, edge_idx) else {
            continue;
        };
        push_retrieval_edge(
            edges,
            edge_keys,
            table,
            evidence,
            blob_hashes,
            node,
            edge.target,
            edge.kind,
            edge_idx,
            hop + 1,
            edge_budget,
        )?;
        if let std::collections::hash_map::Entry::Vacant(entry) = hops.entry(edge.target) {
            entry.insert(hop + 1);
            queue.push_back(edge.target);
        }
        if edges.len() >= edge_budget {
            break;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn expand_retrieval_incoming(
    csr: &CsrAdjacency<'_>,
    reverse: &ReverseIndex,
    table: &SymbolTable,
    evidence: &[SpanEntry],
    blob_hashes: &[[u8; 32]],
    node: u32,
    hop: u32,
    edge_budget: usize,
    hops: &mut HashMap<u32, u32>,
    queue: &mut VecDeque<u32>,
    edges: &mut Vec<RetrievalEdge>,
    edge_keys: &mut BTreeSet<(u32, u32, u8)>,
) -> Result<(), BlastError> {
    for &(src, edge_idx) in reverse.callers(node) {
        let edge_idx = edge_idx as usize;
        let Some(edge) = edge_by_global_index(csr, src, edge_idx) else {
            continue;
        };
        if !is_retrieval_edge(edge.kind) {
            continue;
        }
        push_retrieval_edge(
            edges,
            edge_keys,
            table,
            evidence,
            blob_hashes,
            src,
            node,
            edge.kind,
            edge_idx,
            hop,
            edge_budget,
        )?;
        if let std::collections::hash_map::Entry::Vacant(entry) = hops.entry(src) {
            entry.insert(hop + 1);
            queue.push_back(src);
        }
        if edges.len() >= edge_budget {
            break;
        }
    }
    Ok(())
}

fn outgoing_retrieval_edge_indices(csr: &CsrAdjacency<'_>, src: u32) -> Vec<usize> {
    let base = csr.edge_base(src);
    csr.edges(src)
        .enumerate()
        .filter_map(|(offset, edge)| {
            if is_retrieval_edge(edge.kind) {
                Some(base + offset)
            } else {
                None
            }
        })
        .collect()
}

fn is_retrieval_edge(kind: u8) -> bool {
    kind == edge_kind::CALLS || kind == edge_kind::IMPORTS
}

fn edge_by_global_index(csr: &CsrAdjacency<'_>, src: u32, edge_idx: usize) -> Option<Edge> {
    csr.edges(src)
        .nth(edge_idx.checked_sub(csr.edge_base(src))?)
}

#[allow(clippy::too_many_arguments)]
fn push_retrieval_edge(
    edges: &mut Vec<RetrievalEdge>,
    edge_keys: &mut BTreeSet<(u32, u32, u8)>,
    table: &SymbolTable,
    evidence: &[SpanEntry],
    blob_hashes: &[[u8; 32]],
    from: u32,
    to: u32,
    kind: u8,
    edge_idx: usize,
    hop: u32,
    edge_budget: usize,
) -> Result<(), BlastError> {
    if edges.len() >= edge_budget || !edge_keys.insert((from, to, kind)) {
        return Ok(());
    }
    let from_symbol = table.name(from).unwrap_or("").to_string();
    let to_symbol = table.name(to).unwrap_or("").to_string();
    let ev = evidence.get(edge_idx).copied().unwrap_or_default();
    let hash_hex = hex_blob(blob_hashes, ev.blob_idx)?;
    edges.push(RetrievalEdge {
        provenance_kind: classify_cross_repo_edge(&from_symbol, &to_symbol),
        edge_kind: edge_kind_name(kind).to_string(),
        evidence_ref: blob_span_ref(&hash_hex, ev.start, ev.end),
        from_symbol,
        to_symbol,
        hop,
    });
    Ok(())
}

fn collect_covering_tests(
    snapshot: &Snapshot,
    target: &str,
    impacted_symbols: &[String],
) -> Vec<CoveringTest> {
    // O(n) dedup via HashSet (was Vec::contains O(n^2)).
    let mut needles: Vec<&str> = Vec::new();
    let mut seen = HashSet::<&str>::new();
    for sym in std::iter::once(target).chain(impacted_symbols.iter().map(|s| s.as_str())) {
        let sym = sym.trim();
        if sym.len() >= MIN_COVERING_SYMBOL_LEN && seen.insert(sym) {
            needles.push(sym);
        }
    }
    if needles.is_empty() {
        return Vec::new();
    }

    // Partition once: simple identifier needles get a single O(|text|) pass with
    // HashSet lookup; complex needles (e.g. qualified `mod::sym`) keep find+boundary.
    let (simple_needles, complex_needles) = partition_covering_needles(&needles);

    // Invert only precomp test paths — not the full path table (O(|files|) HashMap).
    let test_paths = snapshot.precomp_test_paths();
    let test_set: HashSet<&str> = test_paths.iter().map(|p| p.as_str()).collect();
    let path_to_hash: HashMap<&str, String> = snapshot
        .path_records()
        .filter_map(|(hash, rec)| {
            let p = rec.path.as_str();
            test_set.contains(p).then_some((p, hash.to_hex()))
        })
        .collect();
    let mut out: Vec<CoveringTest> = test_paths
        .par_iter()
        .filter_map(|path| {
            let hash_hex = path_to_hash.get(path.as_str())?;
            let path_hit = needles.iter().any(|needle| path.contains(needle));
            let text_hit = if path_hit {
                false
            } else {
                // Snapshot-scoped verified cache + parallel per-file
                // read/hash/scan: 52 independent ~30KB blobs dominate blast
                // compute; rayon amortizes across cores (graphzero perf).
                snapshot
                    .blob_bytes(hash_hex)
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .map(|text| text_mentions_any_needle(&text, &simple_needles, &complex_needles))
                    .unwrap_or(false)
            };
            if path_hit || text_hit {
                Some(CoveringTest {
                    path_hint: path.to_string(),
                    evidence_ref: format!("z://blob/{hash_hex}#B0-0"),
                })
            } else {
                None
            }
        })
        .collect();
    out.sort_by(|a, b| a.path_hint.cmp(&b.path_hint));
    out.dedup_by(|a, b| a.path_hint == b.path_hint);
    out
}

/// Symbols shorter than this are too generic to attribute coverage to; matching
/// them would reintroduce a degenerate near-global covering set.
const MIN_COVERING_SYMBOL_LEN: usize = 4;

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Split needles into simple identifier tokens vs complex multi-token forms.
fn partition_covering_needles<'a>(needles: &[&'a str]) -> (HashSet<&'a str>, Vec<&'a str>) {
    let mut simple = HashSet::with_capacity(needles.len());
    let mut complex = Vec::new();
    for &n in needles {
        if !n.is_empty() && n.chars().all(is_ident_char) {
            simple.insert(n);
        } else {
            complex.push(n);
        }
    }
    (simple, complex)
}

/// True when `text` mentions any covering needle with identifier-boundary rules.
/// Simple needles: one token scan over the blob. Complex: per-needle find+boundary.
fn text_mentions_any_needle(text: &str, simple: &HashSet<&str>, complex: &[&str]) -> bool {
    if !simple.is_empty() && text_mentions_any_simple(text, simple) {
        return true;
    }
    complex
        .iter()
        .any(|needle| text_mentions_symbol(text, needle))
}

/// Single linear pass: emit identifier tokens and test membership in `needles`.
/// Equivalent to `needles.iter().any(|n| text_mentions_symbol(text, n))` when every
/// needle is a pure alnum/_ token (same boundary predicate as text_mentions_symbol).
fn text_mentions_any_simple(text: &str, needles: &HashSet<&str>) -> bool {
    let mut i = 0usize;
    let len = text.len();
    while i < len {
        let c = match text[i..].chars().next() {
            Some(c) => c,
            None => break,
        };
        let clen = c.len_utf8();
        if !is_ident_char(c) {
            i += clen;
            continue;
        }
        let start = i;
        i += clen;
        while i < len {
            let c2 = match text[i..].chars().next() {
                Some(c) => c,
                None => break,
            };
            if !is_ident_char(c2) {
                break;
            }
            i += c2.len_utf8();
        }
        if needles.contains(&text[start..i]) {
            return true;
        }
    }
    false
}

/// True when `text` references `symbol` as a whole identifier rather than as a
/// substring of a longer name.
fn text_mentions_symbol(text: &str, symbol: &str) -> bool {
    let bytes = text.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = text[from..].find(symbol) {
        let at = from + rel;
        let before_ok = text[..at]
            .chars()
            .next_back()
            .map(|c| !is_ident_char(c))
            .unwrap_or(true);
        let after = at + symbol.len();
        let after_ok = text[after..]
            .chars()
            .next()
            .map(|c| !is_ident_char(c))
            .unwrap_or(true);
        if before_ok && after_ok {
            return true;
        }
        from = at + symbol.len().max(1);
        if from >= bytes.len() {
            break;
        }
    }
    false
}

fn collect_silent_risk(snapshot: &Snapshot) -> Result<Vec<SilentRisk>, BlastError> {
    let cached = snapshot
        .precomp_silent_risks()
        .map_err(|e| BlastError::Store(e.to_string()))?;
    let mut risks: Vec<SilentRisk> = cached
        .iter()
        .map(|(kind, evidence_ref, detail)| SilentRisk {
            kind: kind.clone(),
            evidence_ref: evidence_ref.clone(),
            detail: detail.clone(),
            class: "heuristic".into(),
        })
        .collect();
    risks.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.evidence_ref.cmp(&b.evidence_ref))
    });
    Ok(risks)
}
