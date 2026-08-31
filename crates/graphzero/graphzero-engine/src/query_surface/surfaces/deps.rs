use graphzero_store::Snapshot;
use graphzero_store::store::csr::{CsrAdjacency, edge_kind};
use graphzero_store::store::query::QueryEngine;
use graphzero_store::store::symbol_table::SymbolTable;

use super::super::QuerySurfaceRouter;
use super::super::helpers::checked_blob_hash;
use super::super::types::*;

impl QuerySurfaceRouter {
    pub(super) fn deps(
        snapshot: &Snapshot,
        req: &QuerySurfaceRequest,
        budget: usize,
    ) -> Result<QuerySurfaceResponse, QuerySurfaceError> {
        let name = req
            .name
            .as_deref()
            .or(req.query.as_deref())
            .ok_or(QuerySurfaceError::MissingArgument("name"))?;
        let capsule = QueryEngine::warm(snapshot, name, budget)
            .map_err(|_| QuerySurfaceError::SymbolNotFound(name.into()))?;
        let mut edges = Vec::new();
        if let Some(m) = capsule.matches.iter().find(|m| m.name == name) {
            // Include import edges attributed directly to the symbol node.
            for e in &m.edges {
                if e.kind == edge_kind::IMPORTS {
                    push_import_edge(&mut edges, name, &e.to, e.confidence, &e.evidence_ref)?;
                }
            }
            // File-scope overapprox: extraction attributes `use` edges to the
            // `<file:path>` node, so gather the matched symbol's definition
            // file IMPORTS edges as a sound file-scope superset.
            for rel_path in m.defs.iter().filter_map(|d| d.path.as_deref()) {
                collect_file_import_edges(snapshot, name, rel_path, &mut edges)?;
            }
        }
        Ok(QuerySurfaceResponse {
            schema_version: 1,
            surface: "deps".into(),
            coverage: Self::footer(snapshot, &capsule)?,
            edges,
            ..Default::default()
        })
    }
}

fn push_import_edge(
    edges: &mut Vec<GraphEdge>,
    from: &str,
    to: &str,
    confidence: f64,
    evidence_ref: &str,
) -> Result<(), QuerySurfaceError> {
    if evidence_ref.is_empty() {
        return Err(QuerySurfaceError::EvidenceMissing);
    }
    // Deduplicate symbol-level and file-scope sources that name the same target.
    if edges
        .iter()
        .any(|e| e.to == to && e.evidence_ref == evidence_ref)
    {
        return Ok(());
    }
    edges.push(GraphEdge {
        kind: "imports".into(),
        to: to.to_string(),
        from: Some(from.to_string()),
        confidence: confidence.min(0.7),
        evidence_ref: evidence_ref.to_string(),
        source: "tier_a".into(),
    });
    Ok(())
}

/// Sound file-scope overapprox for deps: extraction attributes `use` edges to
/// the `<file:path>` node, so query the definition file's IMPORTS edges and
/// report them as dependencies of the queried symbol.
fn collect_file_import_edges(
    snapshot: &graphzero_store::Snapshot,
    from: &str,
    rel_path: &str,
    edges: &mut Vec<GraphEdge>,
) -> Result<(), QuerySurfaceError> {
    let view = snapshot
        .global_view()
        .map_err(|_| QuerySurfaceError::EvidenceMissing)?;
    let table = SymbolTable::from_view(&view).map_err(|_| QuerySurfaceError::EvidenceMissing)?;
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
    let Some(file_id) = table.get(&format!("<file:{rel_path}>")) else {
        return Ok(());
    };
    let base = csr.edge_base(file_id);
    for (offset, edge) in csr.edges(file_id).enumerate() {
        if edge.kind != edge_kind::IMPORTS {
            continue;
        }
        let Some(to) = table.name(edge.target) else {
            continue;
        };
        let ev = evidence.get(base + offset).copied().unwrap_or_default();
        let hash_hex = checked_blob_hash(blob_hashes, ev.blob_idx)?;
        let evidence_ref = graphzero_store::store::refs::blob_span_ref(&hash_hex, ev.start, ev.end);
        push_import_edge(
            edges,
            from,
            to,
            edge.confidence as f64 / 255.0,
            &evidence_ref,
        )?;
    }
    Ok(())
}
