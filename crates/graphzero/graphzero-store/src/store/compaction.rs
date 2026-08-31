//! Tiered compaction: folds wal segments into a new
//! snapshot published via atomic rename. Triggers: segment count > 10 or
//! dirty ratio > 0.3.

use std::fs;
use std::io;
use std::path::Path;

use anyhow::{Error, Result};

use crate::ContentHash;

use super::delta_log::{DeltaLog, read_all_segments};
use super::indexer::{
    BlobMeta, DefRecord, EdgeRecord, IndexData, cleanup, maybe_crash, paths_file_name,
    prune_manifest_to_retained_snapshots, write_snapshot,
};
use super::lock::WriterLock;
use super::manifest::{Manifest, SnapshotEntry};
use super::query::PendingFacts;
use super::shard::ShardReader;
use super::symbol_table::SymbolTable;

pub const MAX_SEGMENTS: usize = 10;
pub const MAX_DIRTY_RATIO: f64 = 0.3;

pub struct CompactionStats {
    pub segment_count: usize,
    pub dirty_ratio: f64,
    /// Total bytes of regular files under the store root. Store growth is the
    /// thing an operator must be able to see before deciding to compact, and
    /// it is not derivable from segment_count alone.
    pub store_bytes: u64,
}

/// Total size of every regular file under `dir`, recursively. Unreadable
/// entries are skipped: a size report must never fail an otherwise healthy
/// stats call.
pub fn store_bytes(dir: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            total += store_bytes(&entry.path());
        } else if meta.is_file() {
            total += meta.len();
        }
    }
    total
}

/// Inspect wal state against the latest snapshot.
pub fn stats(store_root: &Path) -> Result<CompactionStats> {
    let wal_dir = store_root.join("wal");
    let manifest = Manifest::load(store_root)?;
    let folded: std::collections::BTreeSet<u64> = manifest
        .latest()
        .map(|s| s.segment_ids.iter().copied().collect())
        .unwrap_or_default();
    let mut segment_count = 0usize;
    let mut pending_blobs = 0usize;
    if wal_dir.is_dir() {
        for (id, entries) in read_all_segments(&wal_dir)? {
            if folded.contains(&id) {
                continue;
            }
            segment_count += 1;
            pending_blobs += PendingFacts::from_entries(&entries).blobs.len();
        }
    }
    let base_blobs = manifest
        .latest()
        .map(|_| snapshot_blob_count(store_root, &manifest))
        .unwrap_or(Ok(0))?;
    let dirty_ratio = if base_blobs == 0 {
        if pending_blobs > 0 { 1.0 } else { 0.0 }
    } else {
        pending_blobs as f64 / base_blobs as f64
    };
    Ok(CompactionStats {
        segment_count,
        dirty_ratio,
        store_bytes: store_bytes(store_root),
    })
}

fn snapshot_blob_count(store_root: &Path, manifest: &Manifest) -> Result<usize> {
    let Some(entry) = manifest.latest() else {
        return Ok(0);
    };
    let global = store_root
        .join("shards")
        .join(super::indexer::global_file_name(entry.snapshot_id));
    let reader = ShardReader::open(&global)?;
    let view = reader.view()?;
    Ok(view.coverage()?.blob_hashes.len())
}

/// True when either trigger fires.
pub fn should_compact(store_root: &Path) -> Result<bool> {
    let s = stats(store_root)?;
    Ok(s.segment_count > MAX_SEGMENTS || s.dirty_ratio > MAX_DIRTY_RATIO)
}

/// Reconstruct IndexData from the published snapshot (global file + paths
/// sidecar). Trigram postings re-derive from symbol names, so no blob
/// content is needed.
pub fn load_base(store_root: &Path) -> Result<IndexData> {
    let manifest = Manifest::load(store_root)?;
    let Some(entry) = manifest.latest() else {
        return Ok(IndexData::default());
    };
    let shards_dir = store_root.join("shards");
    let reader =
        ShardReader::open(&shards_dir.join(super::indexer::global_file_name(entry.snapshot_id)))?;
    let view = reader.view()?;
    let parts = base_snapshot_parts(&view)?;
    let blob_at = |idx: u32| content_hash_at(parts.blob_hashes, idx);

    let mut data = IndexData::default();
    append_path_sidecar_blobs(&mut data, &shards_dir, entry.snapshot_id);

    data.defs.extend(parts.spans.iter().filter_map(|span| {
        let name = parts.table.name(span.symbol_id)?;
        let sym = parts.table.entry(span.symbol_id)?;
        Some(DefRecord {
            name: name.to_string(),
            kind: sym.kind,
            blob: blob_at(span.blob_idx),
            start: span.start,
            end: span.end,
            block_start: span.block_start,
            block_end: span.block_end,
        })
    }));

    append_base_edges(&mut data, &parts);
    Ok(data)
}

struct BaseSnapshotParts<'a> {
    table: SymbolTable<'a>,
    spans: Vec<super::format::SpanEntry>,
    csr: super::csr::CsrAdjacency<'a>,
    evidence: Vec<super::format::SpanEntry>,
    blob_hashes: &'a [[u8; 32]],
}

fn base_snapshot_parts<'a>(
    view: &'a super::hot_path::ShardView<'a>,
) -> Result<BaseSnapshotParts<'a>> {
    Ok(BaseSnapshotParts {
        table: SymbolTable::from_view(view)?,
        spans: view.spans()?.into_owned(),
        csr: super::csr::CsrAdjacency::new(view.edges()?),
        evidence: view.edge_evidence()?.into_owned(),
        blob_hashes: view.coverage()?.blob_hashes,
    })
}

fn content_hash_at(blob_hashes: &[[u8; 32]], idx: u32) -> ContentHash {
    blob_hashes
        .get(idx as usize)
        .copied()
        .map(ContentHash)
        .unwrap_or(ContentHash([0; 32]))
}

fn edge_record_from_base(
    parts: &BaseSnapshotParts<'_>,
    src: u32,
    edge_idx: usize,
    edge: super::csr::Edge,
) -> EdgeRecord {
    let ev = parts.evidence.get(edge_idx).copied().unwrap_or_default();
    EdgeRecord {
        src: parts.table.name(src).unwrap_or("").to_string(),
        dst: parts.table.name(edge.target).unwrap_or("").to_string(),
        kind: edge.kind,
        confidence: edge.confidence,
        blob: content_hash_at(parts.blob_hashes, ev.blob_idx),
        start: ev.start,
        end: ev.end,
    }
}

fn append_base_edges(data: &mut IndexData, parts: &BaseSnapshotParts<'_>) {
    for src in 0..parts.table.len() as u32 {
        let base = parts.csr.edge_base(src);
        data.edges.extend(
            parts
                .csr
                .edges(src)
                .enumerate()
                .map(|(i, edge)| edge_record_from_base(parts, src, base + i, edge)),
        );
    }
}

struct PathSidecarRecord {
    hash: ContentHash,
    meta: BlobMeta,
}

fn parse_path_sidecar_line(line: &str) -> Option<PathSidecarRecord> {
    let mut parts = line.splitn(5, ' ');
    let hash_hex = parts.next()?;
    let mtime_nanos = parts.next()?;
    let size = parts.next()?;
    let tier_bits = parts.next()?;
    let path = parts.next()?;
    let hash = ContentHash::from_hex(hash_hex)?;
    let size = size.parse().unwrap_or(0);
    let path = path.to_string();
    let mtime_nanos = mtime_nanos.parse().unwrap_or(0);
    let tier_bits = tier_bits.parse().unwrap_or(0);
    Some(PathSidecarRecord {
        hash,
        meta: BlobMeta {
            path,
            mtime_nanos,
            size,
            tier_bits,
            content_len: size as usize,
        },
    })
}

fn append_path_sidecar_blobs(data: &mut IndexData, shards_dir: &Path, snapshot_id: u64) {
    let paths_txt =
        std::fs::read_to_string(shards_dir.join(paths_file_name(snapshot_id))).unwrap_or_default();
    for PathSidecarRecord { hash, meta } in paths_txt.lines().filter_map(parse_path_sidecar_line) {
        data.blobs.insert(hash, meta);
        data.blob_order.push(hash);
    }
}

/// Merge pending wal facts into base IndexData. Blobs re-indexed in the wal
/// supersede their snapshot facts (dirty replacement).
pub fn merge_pending(base: &mut IndexData, pending: &PendingFacts) {
    use std::collections::BTreeSet;
    let dirty: BTreeSet<[u8; 32]> = pending.blobs.keys().copied().collect();
    base.defs.retain(|d| !dirty.contains(&d.blob.0));
    base.edges.retain(|e| !dirty.contains(&e.blob.0));
    for (hash, bits) in &pending.blobs {
        let h = ContentHash(*hash);
        if let Some(meta) = base.blobs.get_mut(&h) {
            meta.tier_bits = *bits;
        } else {
            base.blobs.insert(
                h,
                BlobMeta {
                    path: String::new(),
                    mtime_nanos: 0,
                    size: 0,
                    tier_bits: *bits,
                    content_len: 0,
                },
            );
            base.blob_order.push(h);
        }
    }
    let mut seen_defs = std::collections::HashSet::new();
    for (name, blob, start, end) in &pending.defs {
        if !seen_defs.insert((name.as_str(), *blob, *start, *end)) {
            continue;
        }
        base.defs.push(DefRecord {
            name: name.clone(),
            kind: super::format::symbol_kind::OTHER,
            blob: ContentHash(*blob),
            start: *start,
            end: *end,
            block_start: *start,
            block_end: *end,
        });
    }
    let mut seen_edges = std::collections::HashSet::new();
    for (src, dst, kind, conf, blob, start, end, source) in &pending.edges {
        if !seen_edges.insert((
            src.as_str(),
            dst.as_str(),
            *kind,
            *conf,
            *blob,
            *start,
            *end,
            source.as_deref(),
        )) {
            continue;
        }
        base.edges.push(EdgeRecord {
            src: src.clone(),
            dst: dst.clone(),
            kind: *kind,
            confidence: *conf,
            blob: ContentHash(*blob),
            start: *start,
            end: *end,
        });
    }
}

struct CompactionWork {
    snapshot_id: u64,
    pending: PendingFacts,
    new_segments: Vec<u64>,
    all_folded_segments: Vec<u64>,
    base_blob_count: usize,
}

fn folded_segments(manifest: &Manifest) -> std::collections::BTreeSet<u64> {
    manifest
        .latest()
        .map(|s| s.segment_ids.iter().copied().collect())
        .unwrap_or_default()
}

fn unfolded_segment_ids(
    wal_dir: &Path,
    folded: &std::collections::BTreeSet<u64>,
) -> Result<Vec<u64>> {
    if !wal_dir.is_dir() {
        return Ok(Vec::new());
    }
    Ok(DeltaLog::segment_ids(wal_dir)?
        .into_iter()
        .filter(|id| !folded.contains(id))
        .collect())
}

fn read_pending_segments(wal_dir: &Path, segment_ids: &[u64]) -> Result<PendingFacts> {
    let requested: std::collections::BTreeSet<u64> = segment_ids.iter().copied().collect();
    let mut pending = PendingFacts::default();
    for (id, entries) in read_all_segments(wal_dir)? {
        if !requested.contains(&id) {
            continue;
        }
        let facts = PendingFacts::from_entries(&entries);
        pending.defs.extend(facts.defs);
        pending.edges.extend(facts.edges);
        pending.blobs.extend(facts.blobs);
    }
    Ok(pending)
}

fn dirty_ratio(base_blob_count: usize, dirty_blob_count: usize) -> f64 {
    if base_blob_count == 0 {
        if dirty_blob_count > 0 { 1.0 } else { 0.0 }
    } else {
        dirty_blob_count as f64 / base_blob_count as f64
    }
}

/// True only for availability failures where reads may safely replay the WAL.
pub fn is_read_only_store_error(error: &Error) -> bool {
    error.chain().any(|cause| {
        cause.downcast_ref::<io::Error>().is_some_and(|io_error| {
            matches!(
                io_error.kind(),
                io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
            )
        })
    })
}

/// Probe compaction work without taking the writer lock. Returns `Ok(None)` when there is
/// no unfolded WAL (nothing to compact). Used by `compact_on_open_if_needed` for the
/// unlocked threshold check so under-threshold opens never flock on `.graphzero/lock`.
fn try_prepare_compaction_work(store_root: &Path) -> Result<Option<CompactionWork>> {
    let manifest = Manifest::load(store_root)?;
    let snapshot_id = manifest.latest().map_or(1, |s| s.snapshot_id + 1);
    let wal_dir = store_root.join("wal");
    let folded = folded_segments(&manifest);
    let new_segments = unfolded_segment_ids(&wal_dir, &folded)?;
    if new_segments.is_empty() {
        return Ok(None);
    }
    let pending = read_pending_segments(&wal_dir, &new_segments)?;
    let mut all_folded_segments: Vec<u64> = folded.into_iter().collect();
    all_folded_segments.extend(new_segments.iter().copied());
    all_folded_segments.sort_unstable();
    all_folded_segments.dedup();
    let base_blob_count = manifest
        .latest()
        .map(|_| snapshot_blob_count(store_root, &manifest))
        .unwrap_or(Ok(0))?;
    Ok(Some(CompactionWork {
        snapshot_id,
        pending,
        new_segments,
        all_folded_segments,
        base_blob_count,
    }))
}

fn prepare_compaction_work(store_root: &Path) -> Result<CompactionWork> {
    try_prepare_compaction_work(store_root)?.ok_or_else(|| anyhow::anyhow!("nothing to compact"))
}

fn work_should_compact(work: &CompactionWork) -> bool {
    work.new_segments.len() > MAX_SEGMENTS
        || dirty_ratio(work.base_blob_count, work.pending.blobs.len()) > MAX_DIRTY_RATIO
}

fn publish_compacted_snapshot(
    store_root: &Path,
    entry: SnapshotEntry,
    new_segments: &[u64],
) -> Result<()> {
    let mut manifest = Manifest::load(store_root)?;
    manifest.snapshots.push(entry);
    prune_manifest_to_retained_snapshots(store_root, &mut manifest)?;
    manifest.atomic_publish(store_root)?;
    maybe_crash("after_publish");
    cleanup(store_root, &manifest, new_segments)
}

fn compact_prepared(store_root: &Path, work: CompactionWork) -> Result<u64> {
    let mut data = load_base(store_root)?;
    merge_pending(&mut data, &work.pending);
    let written = write_snapshot(
        store_root,
        &data,
        work.snapshot_id,
        work.all_folded_segments.clone(),
    )?;
    maybe_crash("before_rename");
    publish_compacted_snapshot(store_root, written.entry, &work.all_folded_segments)?;
    Ok(work.snapshot_id)
}

fn compact_locked(store_root: &Path) -> Result<u64> {
    compact_prepared(store_root, prepare_compaction_work(store_root)?)
}

/// Compact synchronously when an open observes either threshold. Thresholds are probed
/// **without** the exclusive writer lock so `open_cached` with under-threshold unfolded WAL does
/// not flock-wait on index/publish.
pub fn compact_on_open_if_needed(store_root: &Path) -> Result<Option<u64>> {
    // Unlocked probe: skip exclusive lock when WAL is
    // empty or under both segment-count and dirty-ratio limits.
    let Some(probe) = try_prepare_compaction_work(store_root)? else {
        return Ok(None);
    };
    if !work_should_compact(&probe) {
        return Ok(None);
    }

    let _lock = WriterLock::acquire(store_root)?;
    // Double-check under lock: another open may have compacted already.
    let Some(work) = try_prepare_compaction_work(store_root)? else {
        return Ok(None);
    };
    if !work_should_compact(&work) {
        return Ok(None);
    }
    compact_prepared(store_root, work).map(Some)
}

/// Run one compaction cycle: replay pending segments over the base
/// snapshot, write a new snapshot, publish atomically, clean up.
pub fn compact(store_root: &Path) -> Result<u64> {
    let _lock = WriterLock::acquire(store_root)?;
    compact_locked(store_root)
}

/// Append entries to the main delta log (incremental write path).
pub fn append_entries(store_root: &Path, entries: Vec<super::delta_log::DeltaEntry>) -> Result<()> {
    let _lock = WriterLock::acquire(store_root)?;
    let mut log = DeltaLog::open(store_root)?;
    for e in entries {
        log.append(e)?;
    }
    log.commit()
}
