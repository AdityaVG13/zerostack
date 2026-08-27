//! Query symbols from installed pack shards (P5.5 FR-006 walking skeleton).

use std::path::Path;

use anyhow::Result;
use graphzero_store::store::csr::CsrAdjacency;
use graphzero_store::store::pack_registry::PackRegistry;
use graphzero_store::store::refs::blob_span_ref;
use graphzero_store::store::shard::ShardReader;
use graphzero_store::store::symbol_table::SymbolTable;

#[derive(Clone, Debug)]
pub struct PackSymbolHit {
    pub symbol: String,
    pub evidence_ref: String,
    pub edge_count: usize,
}

pub fn pack_tier_a_coverage(store_root: &Path) -> Result<f64> {
    let reg = PackRegistry::load(store_root)?;
    if reg.packs.is_empty() {
        return Ok(0.0);
    }
    let sum: f64 = reg.packs.iter().map(|p| p.tier_a_coverage).sum();
    Ok(sum / reg.packs.len() as f64)
}

/// Find a symbol and at least one outgoing edge with gz:// evidence in installed packs.
pub fn query_pack_symbol(store_root: &Path, symbol: &str) -> Result<Option<PackSymbolHit>> {
    let reg = PackRegistry::load(store_root)?;
    for pack in &reg.packs {
        let shard_dir = Path::new(&pack.shard_dir);
        if let Some(hit) = query_in_pack_dir(shard_dir, symbol)? {
            return Ok(Some(hit));
        }
    }
    Ok(None)
}

/// Find a symbol in a specific installed pack version.
pub fn query_pack_symbol_in_version(
    store_root: &Path,
    pack_id: &str,
    version: &str,
    symbol: &str,
) -> Result<Option<PackSymbolHit>> {
    let reg = PackRegistry::load(store_root)?;
    let Some(pack) = reg
        .packs
        .iter()
        .find(|pack| pack.pack_id == pack_id && pack.version == version)
    else {
        return Ok(None);
    };
    query_in_pack_dir(Path::new(&pack.shard_dir), symbol)
}

fn query_in_pack_dir(shard_dir: &Path, symbol: &str) -> Result<Option<PackSymbolHit>> {
    if !shard_dir.is_dir() {
        return Ok(None);
    }
    for entry in std::fs::read_dir(shard_dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "gzsh")
            && let Some(hit) = query_shard_file(&path, symbol)?
        {
            return Ok(Some(hit));
        }
    }
    Ok(None)
}

fn query_shard_file(path: &Path, symbol: &str) -> Result<Option<PackSymbolHit>> {
    let reader = ShardReader::open(path)?;
    let view = reader.view()?;
    let table = SymbolTable::from_view(&view)?;
    let id = table
        .get(symbol)
        .or_else(|| table.prefix_search(symbol).next());
    let Some(id) = id else {
        return Ok(None);
    };
    let spans = view.spans()?;
    let edges = view.edges()?;
    let csr = CsrAdjacency::new(edges);
    let evidence = view.edge_evidence()?;
    let blob_hashes = view.coverage()?.blob_hashes;
    let name = table.name(id).unwrap_or(symbol).to_string();
    let edge_count = csr.edges(id).count();
    let Some(evidence_ref) =
        evidence_ref_for_symbol(id, &name, edge_count, &spans, &evidence, blob_hashes)?
    else {
        return Ok(None);
    };
    Ok(Some(PackSymbolHit {
        symbol: name,
        evidence_ref,
        edge_count,
    }))
}

fn evidence_ref_for_symbol(
    symbol_id: u32,
    name: &str,
    edge_count: usize,
    spans: &[graphzero_store::store::format::SpanEntry],
    evidence: &[graphzero_store::store::format::SpanEntry],
    blob_hashes: &[[u8; 32]],
) -> Result<Option<String>, graphzero_store::BlobHashIndexError> {
    if let Some(span) = spans.iter().find(|span| span.symbol_id == symbol_id) {
        let hash = graphzero_store::hex_blob_hash(blob_hashes, span.blob_idx)?;
        return Ok(Some(blob_span_ref(&hash, span.start, span.end)));
    }
    if let Some(span) = evidence.first() {
        let hash = graphzero_store::hex_blob_hash(blob_hashes, span.blob_idx)?;
        return Ok(Some(blob_span_ref(&hash, span.start, span.end)));
    }
    if edge_count > 0 {
        return Ok(Some(format!("gz://node/{name}")));
    }
    Ok(None)
}
