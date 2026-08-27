//! Publish a snapshot after merging SCIP into collected index data.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use graphzero_store::store::indexer::{self, IndexData};
use graphzero_store::store::manifest::{Manifest, SnapshotEntry};

use crate::decode::decode_scip_bytes;
use crate::ingest::{apply_scip_to_index, blob_map_from_index, scip_facts_from_decoded};

/// Index repo, merge SCIP file, publish new snapshot.
pub fn ingest_scip_publish(
    repo_root: &Path,
    store_root: &Path,
    scip_path: &Path,
) -> Result<(SnapshotEntry, usize, usize)> {
    fs::create_dir_all(store_root)?;
    let _lock =
        graphzero_store::store::lock::WriterLock::acquire(store_root).context("writer lock")?;
    let mut data = indexer::collect(repo_root, store_root)?;
    let edge_count = merge_scip_into_index(store_root, scip_path, &mut data)?;
    let (entry, tier_b_blobs) = publish_index_snapshot(repo_root, store_root, &data)?;
    Ok((entry, edge_count, tier_b_blobs))
}

fn merge_scip_into_index(
    store_root: &Path,
    scip_path: &Path,
    data: &mut IndexData,
) -> Result<usize> {
    let bytes =
        fs::read(scip_path).with_context(|| format!("read SCIP {}", scip_path.display()))?;
    // Single decode; load only document paths from CAS via collect's path table
    // (no second worktree walk / re-hash — graphzero-713dg).
    let (index, summary) = decode_scip_bytes(&bytes)?;
    let needed: Vec<&str> = index
        .documents
        .iter()
        .map(|doc| doc.relative_path.as_str())
        .collect();
    let blobs = blob_map_from_index(store_root, data, needed)?;
    let plan = scip_facts_from_decoded(index, summary, &blobs);
    let edge_count = plan.edges.len();
    apply_scip_to_index(data, &plan);
    Ok(edge_count)
}

fn publish_index_snapshot(
    repo_root: &Path,
    store_root: &Path,
    data: &IndexData,
) -> Result<(SnapshotEntry, usize)> {
    let mut manifest = Manifest::load(store_root)?;
    let snapshot_id = manifest.latest().map_or(1, |s| s.snapshot_id + 1);
    let segment_ids = load_wal_segment_ids(store_root)?;
    let written = indexer::write_snapshot(store_root, data, snapshot_id, segment_ids.clone())?;
    append_and_trim_snapshots(&mut manifest, written.entry.clone());
    manifest.atomic_publish(store_root)?;
    graphzero_store::store::git::record_head_snapshot(
        store_root,
        repo_root,
        written.entry.snapshot_id,
    )?;
    indexer::cleanup(store_root, &manifest, &segment_ids)?;
    let tier_b_blobs = tier_b_count_from_data(data);
    Ok((written.entry, tier_b_blobs))
}

fn load_wal_segment_ids(store_root: &Path) -> Result<Vec<u64>> {
    let wal_dir = store_root.join("wal");
    if !wal_dir.is_dir() {
        return Ok(Vec::new());
    }
    graphzero_store::store::delta_log::DeltaLog::segment_ids(&wal_dir)
}

fn append_and_trim_snapshots(manifest: &mut Manifest, entry: SnapshotEntry) {
    manifest.snapshots.push(entry);
    manifest.snapshots.sort_by_key(|s| s.snapshot_id);
    while manifest.snapshots.len() > 2 {
        manifest.snapshots.remove(0);
    }
}

pub fn tier_b_count_from_data(data: &IndexData) -> usize {
    data.blobs
        .values()
        .filter(|m| m.tier_bits & 0b010 != 0)
        .count()
}
