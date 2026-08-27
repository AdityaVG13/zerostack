//! Stats command core.

use std::path::Path;

use anyhow::Result;
use graphzero_store::store::compaction;
use graphzero_store::{Snapshot, Tier};

use super::paths::{canonical_repo, store_root};

#[derive(Clone, Debug)]
pub struct StoreStats {
    pub snapshot_id: u64,
    pub symbols: usize,
    pub blobs: usize,
    pub tier_a: f64,
    pub pending_segments: usize,
    pub dirty_ratio: f64,
    pub store_bytes: u64,
}

pub fn collect(repo: &Path) -> Result<StoreStats> {
    let repo = canonical_repo(repo)?;
    let root = store_root(&repo);
    let snapshot = Snapshot::open(&root, Some(&repo))?;
    let cov = snapshot.coverage()?;
    let stats = compaction::stats(&root)?;
    Ok(StoreStats {
        snapshot_id: snapshot.entry.snapshot_id,
        symbols: snapshot.symbol_count()?,
        blobs: cov.blob_count(),
        tier_a: cov.ratio(Tier::A),
        pending_segments: stats.segment_count,
        dirty_ratio: stats.dirty_ratio,
        store_bytes: stats.store_bytes,
    })
}

pub fn to_json(s: &StoreStats) -> String {
    format!(
        "{{\"snapshot\":{},\"symbols\":{},\"blobs\":{},\"tier_a\":{:.4},\"pending_segments\":{},\"dirty_ratio\":{:.4},\"store_bytes\":{}}}",
        s.snapshot_id,
        s.symbols,
        s.blobs,
        s.tier_a,
        s.pending_segments,
        s.dirty_ratio,
        s.store_bytes
    )
}
