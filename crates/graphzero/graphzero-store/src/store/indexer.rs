//! Repo indexer: walks the worktree, extracts tier-A facts via `graphzero-extract` (tree-sitter),
//! converts `BlobFacts` into store `DefRecord` / `EdgeRecord`, and falls back to lexical defs when
//! parse fails.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, LazyLock,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use graphzero_extract::{
    BlobFacts, BlobInput, EdgeKind as ExtractEdgeKind, Language, NodeKind as ExtractNodeKind,
    detect::detect_language, engine::extract_tier_a, queries::QuerySet,
    typed_fusion::fuse_installed_typed_edges,
};
use rayon::prelude::*;

use crate::{ContentHash, Tier};

use super::blob_store::BlobStore;
use super::coverage::CoverageBitmap;
use super::csr::{CsrBuilder, edge_kind};
use super::entity::{
    PublishedEntityIndex, SymbolSpanMint, defining_content_digest, mint_symbol_spans,
    register_entity_records, slice_defining_bytes, write_published_entities,
};
use super::format::{SpanEntry, TrigramPosting, symbol_kind};
use super::indexer_walk::{looks_binary, rel_path_string, walk_files};
use super::lock::WriterLock;
use super::manifest::{Manifest, SnapshotEntry};
use super::path_safety::file_name_to_str;
use super::refs::blob_span_ref;
use super::shard::{ShardBuilder, TARGET_SHARD_SIZE};
use super::symbol_table::SymbolTableBuilder;
use super::trigram::{extract_trigrams, sort_postings};

pub const DEFAULT_EDGE_CONFIDENCE: u8 = 179; // 0.7 * 255

/// Host-timed cold-index phase breakdown (env `GRAPHZERO_INDEX_PHASE_TIMING=1`). When the env is
/// set, production `op_index` attaches this map as `phases` on the domain result and eprints a
/// `graphzero_index_phase_timing` JSON line.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct IndexPhaseTimings {
    pub walk_ms: f64,
    pub extract_ms: f64,
    /// Content-addressed blob body writes (`put_nosync` path). Dual-write SharedCas durability
    /// is deferred to [`Self::blob_sync_ms`]; do not treat this as including per-object fsync.
    pub blob_put_ms: f64,
    /// Batch barrier wall for pending flat + cas-local paths.
    pub blob_sync_ms: f64,
    /// Blob paths drained through the barrier (flat + cas-local
    /// dual-write). Attribution for IOPS; independent of wall ms.
    pub blob_fsync_count: u64,
    pub scan_ms: f64,
    pub assemble_ms: f64,
    pub history_ms: f64,
    pub sidecar_ms: f64,
    pub write_snapshot_ms: f64,
    pub manifest_publish_ms: f64,
    pub fingerprint_save_ms: f64,
    pub total_ms: f64,
    pub warm_shortcircuit: bool,
    pub file_count: usize,
}

thread_local! {
    static PHASE_TIMINGS: RefCell<Option<IndexPhaseTimings>> = const { RefCell::new(None) };
}

/// True when `GRAPHZERO_INDEX_PHASE_TIMING` is set (cold + incremental phase clocks).
/// Also true when `GRAPHZERO_STAGE_HISTOGRAM` is set so samples feed the HDR sink.
pub(crate) fn phase_timing_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var_os("GRAPHZERO_INDEX_PHASE_TIMING").is_some()
            || std::env::var_os(crate::store::stage_hist::STAGE_HISTOGRAM_ENV).is_some()
            || std::env::var_os(crate::store::perf_profile::PERF_PROFILE_ENV).is_some()
    })
}

/// True when `GRAPHZERO_PROFILE_SENTINELS` is set: never-inline `_profile_*`
/// frames pin stage boundaries on samply/perf/xctrace stacks under LTO.
fn profile_sentinels_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("GRAPHZERO_PROFILE_SENTINELS").is_some())
}

// Flamegraph stage sentinels: named never-inline frames around hot index phases.
// Called only when `GRAPHZERO_PROFILE_SENTINELS` is set (see `profiled_*`).
#[inline(never)]
fn _profile_walk<R>(f: impl FnOnce() -> R) -> R {
    f()
}
#[inline(never)]
fn _profile_extract<R>(f: impl FnOnce() -> R) -> R {
    f()
}
#[inline(never)]
fn _profile_blob_put<R>(f: impl FnOnce() -> R) -> R {
    f()
}
#[inline(never)]
fn _profile_scan<R>(f: impl FnOnce() -> R) -> R {
    f()
}
#[inline(never)]
fn _profile_assemble<R>(f: impl FnOnce() -> R) -> R {
    f()
}

#[inline(always)]
fn profiled_walk<R>(f: impl FnOnce() -> R) -> R {
    if profile_sentinels_enabled() {
        _profile_walk(f)
    } else {
        f()
    }
}
#[inline(always)]
fn profiled_extract<R>(f: impl FnOnce() -> R) -> R {
    if profile_sentinels_enabled() {
        _profile_extract(f)
    } else {
        f()
    }
}
#[inline(always)]
fn profiled_blob_put<R>(f: impl FnOnce() -> R) -> R {
    if profile_sentinels_enabled() {
        _profile_blob_put(f)
    } else {
        f()
    }
}
#[inline(always)]
fn profiled_scan<R>(f: impl FnOnce() -> R) -> R {
    if profile_sentinels_enabled() {
        _profile_scan(f)
    } else {
        f()
    }
}
#[inline(always)]
fn profiled_assemble<R>(f: impl FnOnce() -> R) -> R {
    if profile_sentinels_enabled() {
        _profile_assemble(f)
    } else {
        f()
    }
}

fn phase_ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn phase_begin() {
    if !phase_timing_enabled() {
        return;
    }
    PHASE_TIMINGS.with(|slot| {
        *slot.borrow_mut() = Some(IndexPhaseTimings::default());
    });
}

fn phase_add(mut f: impl FnMut(&mut IndexPhaseTimings)) {
    if !phase_timing_enabled() {
        return;
    }
    PHASE_TIMINGS.with(|slot| {
        if let Some(t) = slot.borrow_mut().as_mut() {
            f(t);
        }
    });
}

/// Take timings recorded by the last `index_repo` / `collect` under phase timing.
pub fn take_index_phase_timings() -> Option<IndexPhaseTimings> {
    PHASE_TIMINGS.with(|slot| slot.borrow_mut().take())
}

/// Push current index phase walls into the opt-in stage histogram sink.
fn record_index_phases_to_hist() {
    if !crate::store::stage_hist::stage_histogram_enabled()
        && !crate::store::perf_profile::perf_profile_enabled()
    {
        return;
    }
    PHASE_TIMINGS.with(|slot| {
        if let Some(t) = slot.borrow().as_ref() {
            crate::store::stage_hist::record_index_phases(t);
        }
    });
}

const EXTRACT_FILE_NODE_ID: u32 = 0xFFFF_FFFE;
static SHARED_QUERY_SET: LazyLock<QuerySet> = LazyLock::new(QuerySet::new);

/// Rolling extraction-reuse sidecar (`records_latest.json` in the store root). Maps every indexed
/// path to its fingerprint (mtime_nanos + size + content hash) and the per-blob extraction output,
/// so a re-index can skip tree-sitter for unchanged files.
const RECORDS_SIDECAR_VERSION: u32 = 1;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct SidecarDef {
    name: String,
    kind: u8,
    start: u32,
    end: u32,
    block_start: u32,
    block_end: u32,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct SidecarEdge {
    src: String,
    dst: String,
    kind: u8,
    confidence: u8,
    start: u32,
    end: u32,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct SidecarFile {
    path: String,
    mtime_nanos: u128,
    size: u64,
    hash: String,
    tier_bits: u8,
    content_len: usize,
    defs: Vec<SidecarDef>,
    tier_a: Vec<SidecarEdge>,
    scan: Vec<SidecarEdge>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct RecordsSidecar {
    version: u32,
    /// Highest log `generation_nanos` incorporated into this tip. Used so load can apply
    /// only newer append-log deltas without rewriting the tip on every 1-file reindex (``).
    #[serde(default)]
    generation_nanos: i64,
    known_sig: String,
    files: Vec<SidecarFile>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct RecordsSidecarLogEntry {
    version: u32,
    generation_nanos: i64,
    known_sig: String,
    files: Vec<SidecarFile>,
    tombstones: Vec<String>,
}

fn records_sidecar_path(store_root: &Path) -> PathBuf {
    store_root.join("records_latest.jsonl")
}

fn legacy_records_sidecar_path(store_root: &Path) -> PathBuf {
    store_root.join("records_latest.json")
}

/// Max append-log deltas applied on load before compacting tip. Amortizes full
/// tip serialize+zstd+fsync across many 1-file watch reindexes (c7dmu).
const RECORDS_TIP_COMPACT_AFTER_DELTAS: usize = 64;

/// Rewrite tip when a single append touches at least this many paths (cold /
/// large batch). Small incremental deltas stay log-only until compact.
const RECORDS_TIP_REWRITE_PATH_THRESHOLD: usize = 32;

fn read_records_sidecar_tip(store_root: &Path) -> Option<RecordsSidecar> {
    let bytes = fs::read(records_sidecar_tip_path(store_root)).ok()?;
    let decoded = if bytes.starts_with(b"{") {
        bytes
    } else {
        zstd::decode_all(bytes.as_slice()).ok()?
    };
    let side: RecordsSidecar = serde_json::from_slice(&decoded).ok()?;
    (side.version == RECORDS_SIDECAR_VERSION).then_some(side)
}

fn apply_records_log_entry(side: &mut RecordsSidecar, entry: RecordsSidecarLogEntry) {
    side.known_sig = entry.known_sig;
    side.generation_nanos = entry.generation_nanos;
    let mut by_path: BTreeMap<String, SidecarFile> =
        side.files.drain(..).map(|f| (f.path.clone(), f)).collect();
    for tombstone in entry.tombstones {
        by_path.remove(&tombstone);
    }
    for file in entry.files {
        by_path.insert(file.path.clone(), file);
    }
    side.files = by_path.into_values().collect();
}

fn load_records_sidecar(store_root: &Path) -> Option<RecordsSidecar> {
    // Prefer tip, then fold only newer append-log rows (same crash-window idea
    // as graph_history tip+log). Avoids full tip rewrite on every 1-file delta.
    if let Some(mut side) = read_records_sidecar_tip(store_root) {
        let mut deltas = 0usize;
        if let Ok(text) = fs::read_to_string(records_sidecar_path(store_root)) {
            for line in text.lines() {
                let Ok(entry) = serde_json::from_str::<RecordsSidecarLogEntry>(line) else {
                    continue;
                };
                if entry.version != RECORDS_SIDECAR_VERSION {
                    continue;
                }
                if entry.generation_nanos <= side.generation_nanos {
                    continue;
                }
                apply_records_log_entry(&mut side, entry);
                deltas += 1;
            }
        }
        if deltas >= RECORDS_TIP_COMPACT_AFTER_DELTAS {
            let _ = write_records_sidecar_tip(store_root, &side);
        }
        return Some(side);
    }

    let mut known_sig = String::new();
    let mut generation_nanos = 0i64;
    let mut files_by_path: BTreeMap<String, SidecarFile> = BTreeMap::new();
    if let Ok(text) = fs::read_to_string(records_sidecar_path(store_root)) {
        for line in text.lines() {
            let Ok(entry) = serde_json::from_str::<RecordsSidecarLogEntry>(line) else {
                continue;
            };
            if entry.version != RECORDS_SIDECAR_VERSION {
                continue;
            }
            known_sig = entry.known_sig;
            generation_nanos = entry.generation_nanos;
            for tombstone in entry.tombstones {
                files_by_path.remove(&tombstone);
            }
            for file in entry.files {
                files_by_path.insert(file.path.clone(), file);
            }
        }
        let side = RecordsSidecar {
            version: RECORDS_SIDECAR_VERSION,
            generation_nanos,
            known_sig,
            files: files_by_path.into_values().collect(),
        };
        let _ = write_records_sidecar_tip(store_root, &side);
        return Some(side);
    }

    let text = fs::read_to_string(legacy_records_sidecar_path(store_root)).ok()?;
    let side: RecordsSidecar = serde_json::from_str(&text).ok()?;
    (side.version == RECORDS_SIDECAR_VERSION).then(|| {
        let _ = write_records_sidecar_tip(store_root, &side);
        side
    })
}

fn records_sidecar_tip_path(store_root: &Path) -> PathBuf {
    store_root.join("records_latest_tip.json")
}

fn write_records_sidecar_tip(store_root: &Path, side: &RecordsSidecar) -> Result<()> {
    let text = serde_json::to_vec(side).context("serialize records sidecar tip")?;
    let compressed =
        zstd::encode_all(text.as_slice(), 1).context("compress records sidecar tip")?;
    let target = records_sidecar_tip_path(store_root);
    let tmp = target.with_extension("tmp");
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
            .with_context(|| format!("create records sidecar tip {}", tmp.display()))?;
        f.write_all(&compressed)
            .with_context(|| format!("write records sidecar tip {}", tmp.display()))?;
        f.sync_data()
            .with_context(|| format!("fsync records sidecar tip {}", tmp.display()))?;
    }
    fs::rename(&tmp, &target)
        .with_context(|| format!("publish records sidecar tip {}", target.display()))?;
    if let Ok(dir) = File::open(store_root) {
        let _ = dir.sync_all();
    }
    Ok(())
}

fn encode_records_sidecar_log_entry(entry: &RecordsSidecarLogEntry) -> Result<String> {
    encode_records_sidecar_log_entry_with(entry, serde_json::to_string)
}

fn encode_records_sidecar_log_entry_with(
    entry: &RecordsSidecarLogEntry,
    encode: impl FnOnce(&RecordsSidecarLogEntry) -> serde_json::Result<String>,
) -> Result<String> {
    encode(entry).context("serialize records sidecar log entry")
}

fn append_records_sidecar_log(
    store_root: &Path,
    prior: Option<&RecordsSidecar>,
    known_sig: String,
    files: Vec<SidecarFile>,
) -> Result<()> {
    let prior_by_path: BTreeMap<&str, &SidecarFile> = prior
        .map(|s| s.files.iter().map(|f| (f.path.as_str(), f)).collect())
        .unwrap_or_default();
    let current_paths: BTreeSet<String> = files.iter().map(|f| f.path.clone()).collect();
    let mut changed = Vec::new();
    for file in files {
        let keep_existing = prior_by_path.get(file.path.as_str()).is_some_and(|old| {
            old.mtime_nanos == file.mtime_nanos
                && old.size == file.size
                && old.hash == file.hash
                && old.tier_bits == file.tier_bits
                && old.content_len == file.content_len
        });
        if !keep_existing {
            changed.push(file);
        }
    }
    let tombstones: Vec<String> = prior_by_path
        .keys()
        .filter(|path| !current_paths.contains(**path))
        .map(|path| (*path).to_string())
        .collect();
    if changed.is_empty()
        && tombstones.is_empty()
        && prior.is_some_and(|p| p.known_sig == known_sig)
    {
        return Ok(());
    }
    let generation_nanos = now_nanos();
    let entry = RecordsSidecarLogEntry {
        version: RECORDS_SIDECAR_VERSION,
        generation_nanos,
        known_sig: known_sig.clone(),
        files: changed,
        tombstones,
    };
    let line = encode_records_sidecar_log_entry(&entry)?;
    let target = records_sidecar_path(store_root);
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&target)
        .with_context(|| format!("open records sidecar append log {}", target.display()))?;
    writeln!(file, "{line}")
        .with_context(|| format!("append records sidecar log {}", target.display()))?;

    // Full tip rewrite is O(|sidecar files|) serialize+zstd+fsync. Skip it for
    // small incremental deltas when a tip already exists; load folds newer log
    // rows onto the tip (and compacts after RECORDS_TIP_COMPACT_AFTER_DELTAS).
    let path_delta = entry.files.len() + entry.tombstones.len();
    let rewrite_tip = prior.is_none() || path_delta >= RECORDS_TIP_REWRITE_PATH_THRESHOLD;
    if rewrite_tip {
        let tip = RecordsSidecar {
            version: RECORDS_SIDECAR_VERSION,
            generation_nanos,
            known_sig,
            files: {
                let mut by_path: BTreeMap<String, SidecarFile> = prior
                    .map(|s| {
                        s.files
                            .iter()
                            .cloned()
                            .map(|f| (f.path.clone(), f))
                            .collect()
                    })
                    .unwrap_or_default();
                for tombstone in &entry.tombstones {
                    by_path.remove(tombstone);
                }
                for file in entry.files {
                    by_path.insert(file.path.clone(), file);
                }
                by_path.into_values().collect()
            },
        };
        write_records_sidecar_tip(store_root, &tip)?;
    }
    Ok(())
}

const GRAPH_HISTORY_VERSION: u32 = 1;

/// Maximum retained `graph_history.jsonl` delta lines. Tip checkpoint is independent; this bounds
/// append-log growth and full-log scans for historical queries. Oldest deltas are dropped on prune.
pub const GRAPH_HISTORY_LOG_MAX_GENERATIONS: usize = 256;

/// Soft byte ceiling for `graph_history.jsonl` before prune (4 MiB).
pub const GRAPH_HISTORY_LOG_MAX_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, serde::Serialize, serde::Deserialize, Default)]
struct GraphHistoryEntry {
    version: u32,
    generation_nanos: i64,
    #[serde(default)]
    commit: Option<String>,
    #[serde(default)]
    author: Option<String>,
    appeared_nodes: Vec<String>,
    vanished_nodes: Vec<String>,
    appeared_edges: Vec<String>,
    vanished_edges: Vec<String>,
    /// Full checkpoints are optional on log lines.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    state_nodes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    state_edges: Vec<String>,
}

fn graph_history_path(store_root: &Path) -> PathBuf {
    store_root.join("graph_history.jsonl")
}

fn graph_history_tip_path(store_root: &Path) -> PathBuf {
    store_root.join("graph_history_tip.json")
}

/// Sidecar recording the highest generation present in the append log. Written on every
/// successful append so tip authority checks are O(1) and do not `read_to_string` the full JSONL.
fn graph_history_log_max_path(store_root: &Path) -> PathBuf {
    store_root.join("graph_history_log_max")
}

fn write_graph_history_log_max(store_root: &Path, generation_nanos: i64) -> Result<()> {
    let target = graph_history_log_max_path(store_root);
    let tmp = target.with_extension("tmp");
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
            .with_context(|| format!("create graph history log max {}", tmp.display()))?;
        write!(f, "{generation_nanos}")
            .with_context(|| format!("write graph history log max {}", tmp.display()))?;
        f.sync_data()
            .with_context(|| format!("fsync graph history log max {}", tmp.display()))?;
    }
    fs::rename(&tmp, &target)
        .with_context(|| format!("publish graph history log max {}", target.display()))?;
    Ok(())
}

fn read_graph_history_log_max(store_root: &Path) -> Option<i64> {
    let text = fs::read_to_string(graph_history_log_max_path(store_root)).ok()?;
    text.trim().parse().ok()
}

/// Persist full tip checkpoint (serialize + zstd + fsync + rename + dir
/// sync). Wall cost scales with tip state size (see scale curve). Not a
/// delta write: callers pass the full current `state_nodes`/`state_edges`.
fn write_graph_history_tip(store_root: &Path, entry: &GraphHistoryEntry) -> Result<()> {
    let text = serde_json::to_vec(entry).context("serialize graph history tip")?;
    let compressed = zstd::encode_all(text.as_slice(), 1).context("compress graph history tip")?;
    let target = graph_history_tip_path(store_root);
    let tmp = target.with_extension("tmp");
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
            .with_context(|| format!("create graph history tip {}", tmp.display()))?;
        f.write_all(&compressed)
            .with_context(|| format!("write graph history tip {}", tmp.display()))?;
        f.sync_data()
            .with_context(|| format!("fsync graph history tip {}", tmp.display()))?;
    }
    fs::rename(&tmp, &target)
        .with_context(|| format!("publish graph history tip {}", target.display()))?;
    // Durable rename: directory fsync makes the tip name visible after crash.
    if let Ok(dir) = File::open(store_root) {
        let _ = dir.sync_all();
    }
    Ok(())
}

fn read_graph_history_tip(store_root: &Path) -> Option<GraphHistoryEntry> {
    let bytes = fs::read(graph_history_tip_path(store_root)).ok()?;
    let decoded = if bytes.starts_with(b"{") {
        bytes
    } else {
        zstd::decode_all(bytes.as_slice()).ok()?
    };
    let entry: GraphHistoryEntry = serde_json::from_slice(&decoded).ok()?;
    (entry.version == GRAPH_HISTORY_VERSION).then_some(entry)
}

/// Max generation from a bounded tail read of the append log (not full O(G)). Each log
/// line is a compact delta (no full tip state), so the last complete JSON line sits in
/// a small trailing window. Partial first line after seek is skipped by parse failure.
fn last_log_line_generation(store_root: &Path) -> Option<i64> {
    use std::io::{Read, Seek, SeekFrom};

    let path = graph_history_path(store_root);
    let mut f = File::open(&path).ok()?;
    let len = f.metadata().ok()?.len();
    if len == 0 {
        return None;
    }
    const WINDOW: u64 = 16 * 1024;
    let window = WINDOW.min(len);
    f.seek(SeekFrom::End(-(window as i64))).ok()?;
    let mut buf = vec![0u8; window as usize];
    f.read_exact(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    for line in text.lines().rev().filter(|l| !l.trim().is_empty()) {
        if let Ok(entry) = serde_json::from_str::<GraphHistoryEntry>(line)
            && entry.version == GRAPH_HISTORY_VERSION
        {
            return Some(entry.generation_nanos);
        }
    }
    None
}

/// Max append-log generation without full-file scan when possible. Order: log-max sidecar,
/// then bounded tail parse, then full scan + sidecar backfill (legacy / corrupted tail only).
fn max_graph_history_log_generation(store_root: &Path) -> Option<i64> {
    let sidecar = read_graph_history_log_max(store_root);
    let tail = last_log_line_generation(store_root);
    match (sidecar, tail) {
        (Some(s), Some(t)) => Some(s.max(t)),
        (Some(s), None) => Some(s),
        (None, Some(t)) => {
            let _ = write_graph_history_log_max(store_root, t);
            Some(t)
        }
        (None, None) => {
            let text = fs::read_to_string(graph_history_path(store_root)).ok()?;
            let mut max_gen = None;
            for line in text.lines() {
                let Ok(entry) = serde_json::from_str::<GraphHistoryEntry>(line) else {
                    continue;
                };
                if entry.version != GRAPH_HISTORY_VERSION {
                    continue;
                }
                max_gen = Some(max_gen.map_or(entry.generation_nanos, |g: i64| {
                    g.max(entry.generation_nanos)
                }));
            }
            if let Some(g) = max_gen {
                let _ = write_graph_history_log_max(store_root, g);
            }
            max_gen
        }
    }
}

fn latest_graph_history_entry(store_root: &Path) -> Option<GraphHistoryEntry> {
    let tip = read_graph_history_tip(store_root);
    // O(1) authority check via log-max sidecar. Full JSONL scan
    // only when sidecar is missing (backfill) or tip lags the log.
    let max_log_gen = max_graph_history_log_generation(store_root);
    // Prefer tip only when it is at least as new as the append log. After a
    // crash window (log append ok, tip publish interrupted) the tip can lag;
    // reconstruct from tip checkpoint + newer log deltas and rewrite tip.
    if let Some(tip) = tip
        && max_log_gen.is_none_or(|g| tip.generation_nanos >= g)
    {
        return Some(tip);
    }
    let state = graph_state_at_generation(store_root, i64::MAX)
        .ok()
        .flatten()?;
    let entry = GraphHistoryEntry {
        version: GRAPH_HISTORY_VERSION,
        generation_nanos: state.generation_nanos,
        commit: None,
        author: None,
        appeared_nodes: Vec::new(),
        vanished_nodes: Vec::new(),
        appeared_edges: Vec::new(),
        vanished_edges: Vec::new(),
        state_nodes: state.nodes,
        state_edges: state.edges,
    };
    let _ = write_graph_history_tip(store_root, &entry);
    let _ = write_graph_history_log_max(store_root, entry.generation_nanos);
    Some(entry)
}

/// Drop oldest append-log lines when over generation or byte ceiling. Tip
/// checkpoint is unchanged; historical queries on pruned generations return
/// incomplete timelines (documented ceiling, not silent infinite retain).
fn prune_graph_history_log_if_needed(store_root: &Path) -> Result<()> {
    let path = graph_history_path(store_root);
    let meta = match fs::metadata(&path) {
        Ok(m) => m,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    let text = fs::read_to_string(&path)?;
    let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
    let over_gens = lines.len() > GRAPH_HISTORY_LOG_MAX_GENERATIONS;
    let over_bytes = meta.len() > GRAPH_HISTORY_LOG_MAX_BYTES;
    if !over_gens && !over_bytes {
        return Ok(());
    }

    // Keep the newest MAX_GENERATIONS lines (or enough to get under byte soft
    // ceiling while still retaining at least one generation when present).
    let mut keep_from = lines
        .len()
        .saturating_sub(GRAPH_HISTORY_LOG_MAX_GENERATIONS);
    if over_bytes {
        let mut acc = 0u64;
        let mut idx = lines.len();
        while idx > keep_from {
            let line = lines[idx - 1];
            let cost = (line.len() as u64) + 1;
            if acc + cost > GRAPH_HISTORY_LOG_MAX_BYTES && idx < lines.len() {
                break;
            }
            acc += cost;
            idx -= 1;
        }
        keep_from = idx;
    }
    if keep_from == 0 {
        return Ok(());
    }

    let retained: Vec<&str> = lines[keep_from..].to_vec();
    let mut out = retained.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    let tmp = path.with_extension("jsonl.tmp");
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
            .with_context(|| format!("create pruned graph history {}", tmp.display()))?;
        f.write_all(out.as_bytes())
            .with_context(|| format!("write pruned graph history {}", tmp.display()))?;
        f.sync_data()
            .with_context(|| format!("fsync pruned graph history {}", tmp.display()))?;
    }
    fs::rename(&tmp, &path)
        .with_context(|| format!("publish pruned graph history {}", path.display()))?;

    // Recompute max gen from retained lines (or clear if empty).
    let mut max_gen = None;
    for line in &retained {
        if let Ok(entry) = serde_json::from_str::<GraphHistoryEntry>(line)
            && entry.version == GRAPH_HISTORY_VERSION
        {
            max_gen = Some(max_gen.map_or(entry.generation_nanos, |g: i64| {
                g.max(entry.generation_nanos)
            }));
        }
    }
    if let Some(g) = max_gen {
        write_graph_history_log_max(store_root, g)?;
    } else if graph_history_log_max_path(store_root).exists() {
        let _ = fs::remove_file(graph_history_log_max_path(store_root));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EdgeLifecycleKind {
    Appeared,
    Vanished,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeLifecycleEvent {
    pub generation_nanos: i64,
    pub kind: EdgeLifecycleKind,
    pub edge_key: String,
}

fn edge_key_mentions(edge_key: &str, src: &str, dst: &str) -> bool {
    edge_key.rsplit('|').nth(1) == Some(src) && edge_key.rsplit('|').next() == Some(dst)
}

pub fn edge_lifecycle(store_root: &Path, src: &str, dst: &str) -> Result<Vec<EdgeLifecycleEvent>> {
    let text = match fs::read_to_string(graph_history_path(store_root)) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut events = Vec::new();
    for line in text.lines() {
        let entry: GraphHistoryEntry = serde_json::from_str(line)?;
        if entry.version != GRAPH_HISTORY_VERSION {
            continue;
        }
        for edge_key in entry.appeared_edges {
            if edge_key_mentions(&edge_key, src, dst) {
                events.push(EdgeLifecycleEvent {
                    generation_nanos: entry.generation_nanos,
                    kind: EdgeLifecycleKind::Appeared,
                    edge_key,
                });
            }
        }
        for edge_key in entry.vanished_edges {
            if edge_key_mentions(&edge_key, src, dst) {
                events.push(EdgeLifecycleEvent {
                    generation_nanos: entry.generation_nanos,
                    kind: EdgeLifecycleKind::Vanished,
                    edge_key,
                });
            }
        }
    }
    Ok(events)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StructuralChangeKind {
    NodeAppeared,
    NodeVanished,
    EdgeAppeared,
    EdgeVanished,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructuralChangeEvent {
    pub generation_nanos: i64,
    pub kind: StructuralChangeKind,
    pub entity_key: String,
    pub commit: Option<String>,
    pub author: Option<String>,
}

pub fn structural_timeline(
    store_root: &Path,
    entity_query: &str,
) -> Result<Vec<StructuralChangeEvent>> {
    let text = match fs::read_to_string(graph_history_path(store_root)) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut events = Vec::new();
    for line in text.lines() {
        let entry: GraphHistoryEntry = serde_json::from_str(line)?;
        if entry.version != GRAPH_HISTORY_VERSION {
            continue;
        }
        let mut push_matching = |items: Vec<String>, kind: StructuralChangeKind| {
            for entity_key in items {
                if entity_key.contains(entity_query) {
                    events.push(StructuralChangeEvent {
                        generation_nanos: entry.generation_nanos,
                        kind: kind.clone(),
                        entity_key,
                        commit: entry.commit.clone(),
                        author: entry.author.clone(),
                    });
                }
            }
        };
        push_matching(entry.appeared_nodes, StructuralChangeKind::NodeAppeared);
        push_matching(entry.vanished_nodes, StructuralChangeKind::NodeVanished);
        push_matching(entry.appeared_edges, StructuralChangeKind::EdgeAppeared);
        push_matching(entry.vanished_edges, StructuralChangeKind::EdgeVanished);
    }
    Ok(events)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoricalGraphState {
    pub generation_nanos: i64,
    pub nodes: Vec<String>,
    pub edges: Vec<String>,
}

fn edge_history_src_dst(edge_key: &str) -> Option<(&str, &str)> {
    let mut parts = edge_key.splitn(7, '|');
    let _kind = parts.next()?;
    let _confidence = parts.next()?;
    let _blob = parts.next()?;
    let _start = parts.next()?;
    let _end = parts.next()?;
    let src = parts.next()?;
    let dst = parts.next()?;
    Some((src, dst))
}

pub fn graph_state_at_generation(
    store_root: &Path,
    generation_nanos: i64,
) -> Result<Option<HistoricalGraphState>> {
    // Tip holds the compact current checkpoint for delta-only logs. Seed from it
    // when it is at or before the requested generation, then apply newer log
    // rows. This recovers the crash window where the log advanced past a stale tip.
    let tip = read_graph_history_tip(store_root);
    let tip_seed_gen = tip
        .as_ref()
        .and_then(|t| (t.generation_nanos <= generation_nanos).then_some(t.generation_nanos));

    let text = match fs::read_to_string(graph_history_path(store_root)) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(tip
                .filter(|t| t.generation_nanos <= generation_nanos)
                .map(|t| HistoricalGraphState {
                    generation_nanos: t.generation_nanos,
                    nodes: t.state_nodes,
                    edges: t.state_edges,
                }));
        }
        Err(err) => return Err(err.into()),
    };

    let mut nodes: BTreeSet<String> = BTreeSet::new();
    let mut edges: BTreeSet<String> = BTreeSet::new();
    let mut selected_gen = None;
    if let (Some(t), Some(seed_gen)) = (&tip, tip_seed_gen) {
        nodes = t.state_nodes.iter().cloned().collect();
        edges = t.state_edges.iter().cloned().collect();
        selected_gen = Some(seed_gen);
    }

    for line in text.lines() {
        let entry: GraphHistoryEntry = serde_json::from_str(line)?;
        if entry.version != GRAPH_HISTORY_VERSION || entry.generation_nanos > generation_nanos {
            continue;
        }
        // Tip already incorporates every log row at or before tip_seed_gen.
        if tip_seed_gen.is_some_and(|g| entry.generation_nanos <= g) {
            continue;
        }
        if !entry.state_nodes.is_empty() || !entry.state_edges.is_empty() {
            // Legacy checkpoint rows carry the full state.
            nodes = entry.state_nodes.into_iter().collect();
            edges = entry.state_edges.into_iter().collect();
        } else {
            for key in entry.vanished_nodes {
                nodes.remove(&key);
            }
            for key in entry.appeared_nodes {
                nodes.insert(key);
            }
            for key in entry.vanished_edges {
                edges.remove(&key);
            }
            for key in entry.appeared_edges {
                edges.insert(key);
            }
        }
        selected_gen = Some(entry.generation_nanos);
    }
    Ok(selected_gen.map(|generation_nanos| HistoricalGraphState {
        generation_nanos,
        nodes: nodes.into_iter().collect(),
        edges: edges.into_iter().collect(),
    }))
}

fn blast_radius_in_state(
    state: &HistoricalGraphState,
    seeds: &[String],
    depth: usize,
) -> Vec<String> {
    // Build reverse adjacency in O(|E|), then traverse each frontier in
    // O(frontier × degree) per hop.
    let mut reverse: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for edge in &state.edges {
        let Some((src, dst)) = edge_history_src_dst(edge) else {
            continue;
        };
        reverse.entry(dst).or_default().push(src);
    }

    let seed_set: BTreeSet<&str> = seeds.iter().map(String::as_str).collect();
    let mut frontier: BTreeSet<String> = seed_set.iter().map(|seed| (*seed).to_string()).collect();
    let mut impacted: BTreeSet<String> = BTreeSet::new();
    for _ in 0..depth {
        let mut next = BTreeSet::new();
        for node in &frontier {
            let Some(srcs) = reverse.get(node.as_str()) else {
                continue;
            };
            for src in srcs {
                if !seed_set.contains(src) && impacted.insert((*src).to_string()) {
                    next.insert((*src).to_string());
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    impacted.into_iter().collect()
}

pub fn blast_radius_at_generation(
    store_root: &Path,
    seeds: &[String],
    generation_nanos: i64,
    depth: usize,
) -> Result<Vec<String>> {
    let Some(state) = graph_state_at_generation(store_root, generation_nanos)? else {
        return Ok(Vec::new());
    };
    Ok(blast_radius_in_state(&state, seeds, depth))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerationBlast {
    pub generation_nanos: i64,
    pub impacted: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerationBlastDiff {
    pub before_generation_nanos: i64,
    pub after_generation_nanos: i64,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged: Vec<String>,
}

pub fn deterministic_blast_at_generation(
    store_root: &Path,
    seeds: &[String],
    generation_nanos: i64,
    depth: usize,
) -> Result<Option<GenerationBlast>> {
    let Some(state) = graph_state_at_generation(store_root, generation_nanos)? else {
        return Ok(None);
    };
    // Reuse the loaded state — do not re-parse history via blast_radius_at_generation.
    Ok(Some(GenerationBlast {
        generation_nanos: state.generation_nanos,
        impacted: blast_radius_in_state(&state, seeds, depth),
    }))
}

pub fn diff_blast_sets_between_generations(
    store_root: &Path,
    seeds: &[String],
    before_generation_nanos: i64,
    after_generation_nanos: i64,
    depth: usize,
) -> Result<Option<GenerationBlastDiff>> {
    let Some(before) =
        deterministic_blast_at_generation(store_root, seeds, before_generation_nanos, depth)?
    else {
        return Ok(None);
    };
    let Some(after) =
        deterministic_blast_at_generation(store_root, seeds, after_generation_nanos, depth)?
    else {
        return Ok(None);
    };
    let before_set: BTreeSet<String> = before.impacted.into_iter().collect();
    let after_set: BTreeSet<String> = after.impacted.into_iter().collect();
    Ok(Some(GenerationBlastDiff {
        before_generation_nanos: before.generation_nanos,
        after_generation_nanos: after.generation_nanos,
        added: after_set.difference(&before_set).cloned().collect(),
        removed: before_set.difference(&after_set).cloned().collect(),
        unchanged: before_set.intersection(&after_set).cloned().collect(),
    }))
}

fn node_history_key(def: &DefRecord) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        def.kind,
        def.blob.to_hex(),
        def.start,
        def.end,
        def.block_start,
        def.name
    )
}

fn edge_history_key(edge: &EdgeRecord) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}",
        edge.kind,
        edge.confidence,
        edge.blob.to_hex(),
        edge.start,
        edge.end,
        edge.src,
        edge.dst
    )
}

fn current_git_identity(repo_root: &Path) -> (Option<String>, Option<String>) {
    if !repo_root.join(".git").exists() {
        return (None, None);
    }
    let Ok(repo) = git2::Repository::discover(repo_root) else {
        return (None, None);
    };
    let Ok(commit) = repo.head().and_then(|head| head.peel_to_commit()) else {
        return (None, None);
    };
    (
        Some(commit.id().to_string()),
        commit.author().name().ok().map(|name| name.to_string()),
    )
}

fn append_temporal_graph_history(
    repo_root: &Path,
    store_root: &Path,
    data: &IndexData,
) -> Result<()> {
    let state_nodes: BTreeSet<String> = data.defs.iter().map(node_history_key).collect();
    let state_edges: BTreeSet<String> = data.edges.iter().map(edge_history_key).collect();
    let prior = latest_graph_history_entry(store_root).unwrap_or_default();
    let prior_nodes: BTreeSet<String> = prior.state_nodes.into_iter().collect();
    let prior_edges: BTreeSet<String> = prior.state_edges.into_iter().collect();
    let appeared_nodes: Vec<String> = state_nodes.difference(&prior_nodes).cloned().collect();
    let vanished_nodes: Vec<String> = prior_nodes.difference(&state_nodes).cloned().collect();
    let appeared_edges: Vec<String> = state_edges.difference(&prior_edges).cloned().collect();
    let vanished_edges: Vec<String> = prior_edges.difference(&state_edges).cloned().collect();
    if appeared_nodes.is_empty()
        && vanished_nodes.is_empty()
        && appeared_edges.is_empty()
        && vanished_edges.is_empty()
        && prior.version == GRAPH_HISTORY_VERSION
        && graph_history_tip_path(store_root).is_file()
    {
        // Unchanged graph: skip append/serialize (dominant warm-index CPU+RAM cost).
        return Ok(());
    }
    let (commit, author) = current_git_identity(repo_root);
    let tip = GraphHistoryEntry {
        version: GRAPH_HISTORY_VERSION,
        generation_nanos: now_nanos(),
        commit: commit.clone(),
        author: author.clone(),
        appeared_nodes: appeared_nodes.clone(),
        vanished_nodes: vanished_nodes.clone(),
        appeared_edges: appeared_edges.clone(),
        vanished_edges: vanished_edges.clone(),
        state_nodes: state_nodes.into_iter().collect(),
        state_edges: state_edges.into_iter().collect(),
    };
    // Append-only log stores deltas only; tip carries the compact current checkpoint.
    let log_entry = GraphHistoryEntry {
        version: GRAPH_HISTORY_VERSION,
        generation_nanos: tip.generation_nanos,
        commit,
        author,
        appeared_nodes,
        vanished_nodes,
        appeared_edges,
        vanished_edges,
        state_nodes: Vec::new(),
        state_edges: Vec::new(),
    };
    let mut json = serde_json::to_string(&log_entry).context("serialize graph history entry")?;
    json.push('\n');
    let target = graph_history_path(store_root);
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&target)
        .with_context(|| format!("open graph history log {}", target.display()))?;
    use std::io::Write;
    file.write_all(json.as_bytes())
        .with_context(|| format!("append graph history log {}", target.display()))?;
    // Log-max before tip: crash after this still prefers newer log (O(1) check).
    write_graph_history_log_max(store_root, tip.generation_nanos)?;
    write_graph_history_tip(store_root, &tip)?;
    prune_graph_history_log_if_needed(store_root)?;
    Ok(())
}

fn known_signature(known: &BTreeMap<String, ()>) -> String {
    // BTreeMap iterates sorted, so the signature is order-stable.
    let mut joined = String::new();
    for name in known.keys() {
        joined.push_str(name);
        joined.push('\n');
    }
    ContentHash::of(joined.as_bytes()).to_hex()
}

fn sidecar_def(d: &DefRecord) -> SidecarDef {
    SidecarDef {
        name: d.name.clone(),
        kind: d.kind,
        start: d.start,
        end: d.end,
        block_start: d.block_start,
        block_end: d.block_end,
    }
}

fn def_from_sidecar(d: &SidecarDef, blob: ContentHash) -> DefRecord {
    DefRecord {
        name: d.name.clone(),
        kind: d.kind,
        blob,
        start: d.start,
        end: d.end,
        block_start: d.block_start,
        block_end: d.block_end,
    }
}

/// Intern a symbol name for assemble-time edge dedup keys (one String per unique name).
#[inline]
fn intern_assemble_name(table: &mut HashMap<String, u32>, name: &str) -> u32 {
    if let Some(&id) = table.get(name) {
        return id;
    }
    let id = table.len() as u32;
    table.insert(name.to_string(), id);
    id
}

fn sidecar_edge(e: &EdgeRecord) -> SidecarEdge {
    SidecarEdge {
        src: e.src.clone(),
        dst: e.dst.clone(),
        kind: e.kind,
        confidence: e.confidence,
        start: e.start,
        end: e.end,
    }
}

fn edge_from_sidecar(e: &SidecarEdge, blob: ContentHash) -> EdgeRecord {
    EdgeRecord {
        src: e.src.clone(),
        dst: e.dst.clone(),
        kind: e.kind,
        confidence: e.confidence,
        blob,
        start: e.start,
        end: e.end,
    }
}

#[derive(Clone, Debug)]
pub struct BlobMeta {
    pub path: String,
    pub mtime_nanos: u128,
    pub size: u64,
    pub tier_bits: u8, // bit 0 = A, 1 = B, 2 = C
    pub content_len: usize,
}

impl BlobMeta {
    pub fn apply_to_coverage(&self, idx: usize, cov: &mut CoverageBitmap) {
        if self.tier_bits & 0b001 != 0 {
            cov.set(idx, Tier::A, true);
        }
        if self.tier_bits & 0b010 != 0 {
            cov.set(idx, Tier::B, true);
        }
        if self.tier_bits & 0b100 != 0 {
            cov.set(idx, Tier::C, true);
        }
    }
}

#[derive(Clone, Debug)]
pub struct DefRecord {
    pub name: String,
    pub kind: u8,
    pub blob: ContentHash,
    /// Identifier/name byte span.
    pub start: u32,
    pub end: u32,
    /// Full definition node; equals `start`/`end` when only the name is known.
    pub block_start: u32,
    pub block_end: u32,
}

fn def_record_to_span(d: &DefRecord, blob_idx: u32, symbol_id: u32) -> SpanEntry {
    SpanEntry {
        blob_idx,
        start: d.start,
        end: d.end,
        symbol_id,
        block_start: d.block_start,
        block_end: d.block_end,
    }
}

#[derive(Clone, Debug)]
pub struct EdgeRecord {
    pub src: String,
    pub dst: String,
    pub kind: u8,
    pub confidence: u8,
    pub blob: ContentHash,
    pub start: u32,
    pub end: u32,
}

/// In-memory index state, the unit both fresh indexing and compaction
/// replay produce before a snapshot is written.
#[derive(Default)]
pub struct IndexData {
    /// blob hash -> metadata, in path order via `blob_order`.
    pub blobs: BTreeMap<ContentHash, BlobMeta>,
    pub blob_order: Vec<ContentHash>,
    pub defs: Vec<DefRecord>,
    pub edges: Vec<EdgeRecord>,
}

impl Clone for IndexData {
    fn clone(&self) -> Self {
        Self {
            blobs: self.blobs.clone(),
            blob_order: self.blob_order.clone(),
            defs: self.defs.clone(),
            edges: self.edges.clone(),
        }
    }
}

/// Lexical tier-A fallback: definition keywords across Rust, Python,
/// JS/TS, Go. Used when `extract_tier_a` reports `parse_ok == false`.
pub fn extract_defs(blob: &ContentHash, content: &[u8]) -> Vec<DefRecord> {
    const KEYWORDS: &[(&str, u8)] = &[
        ("fn", symbol_kind::FUNCTION),
        ("def", symbol_kind::FUNCTION),
        ("function", symbol_kind::FUNCTION),
        ("func", symbol_kind::FUNCTION),
        ("struct", symbol_kind::TYPE),
        ("enum", symbol_kind::TYPE),
        ("trait", symbol_kind::TYPE),
        ("class", symbol_kind::TYPE),
        ("interface", symbol_kind::TYPE),
        ("mod", symbol_kind::MODULE),
        ("impl", symbol_kind::TYPE),
    ];
    let Ok(text) = std::str::from_utf8(content) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let tokens = tokenize(line);
        let mut i = 0;
        while i + 1 < tokens.len() {
            let (tok, _) = tokens[i];
            if let Some(&(_, kind)) = KEYWORDS.iter().find(|(k, _)| *k == tok) {
                let (name, name_off) = tokens[i + 1];
                if is_identifier(name) {
                    let start = (offset + name_off) as u32;
                    let end = (offset + name_off + name.len()) as u32;
                    out.push(DefRecord {
                        name: name.to_string(),
                        kind,
                        blob: *blob,
                        start,
                        end,
                        block_start: start,
                        block_end: end,
                    });
                }
                i += 2;
                continue;
            }
            i += 1;
        }
        offset += line.len();
    }
    out
}

fn is_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b':')
        && !s.bytes().next().unwrap().is_ascii_digit()
}

/// Tokenize a line into (identifier, byte_offset) pairs. Identifiers may
/// contain `::` path separators (Rust-style qualified names).
fn tokenize<'a>(line: &'a str) -> Vec<(&'a str, usize)> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphabetic() || b == b'_' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric()
                    || bytes[i] == b'_'
                    || (bytes[i] == b':' && i + 1 < bytes.len() && bytes[i + 1] == b':'))
            {
                if bytes[i] == b':' {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            out.push((&line[start..i], start));
        } else {
            i += 1;
        }
    }
    out
}

/// Emits one call edge per source-destination pair when a known symbol is invoked inside a definition.
pub fn extract_edges(
    blob: &ContentHash,
    content: &[u8],
    known: &BTreeMap<String, ()>,
    local_defs: &[DefRecord],
) -> Vec<EdgeRecord> {
    let known_set: HashSet<&str> = known.keys().map(String::as_str).collect();
    extract_edges_with_known(blob, content, &known_set, local_defs)
}

fn extract_edges_with_known(
    blob: &ContentHash,
    content: &[u8],
    known: &HashSet<&str>,
    local_defs: &[DefRecord],
) -> Vec<EdgeRecord> {
    // Lexical `ident(` probe against the repo-global known def-name set.
    // Runs after tree-sitter extract so cross-file calls become edges.
    // Do not delete this pass to "save CPU" without a measured quality gate.
    let Ok(text) = std::str::from_utf8(content) else {
        return Vec::new();
    };
    // Sort defs by start for O(log n) enclosing-def lookup.
    let mut defs_by_start: Vec<&DefRecord> = local_defs.iter().collect();
    defs_by_start.sort_by_key(|d| d.start);
    let mut out = Vec::new();
    let mut seen: HashSet<(u32, &str)> = HashSet::new();
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        for (tok, tok_off) in tokenize(line) {
            let abs = offset + tok_off;
            let after = abs + tok.len();
            let calls_like = text.as_bytes().get(after) == Some(&b'(');
            if !calls_like || !known.contains(tok) {
                continue;
            }
            let src_idx = defs_by_start.partition_point(|d| (d.start as usize) <= abs);
            let src_def = src_idx
                .checked_sub(1)
                .and_then(|i| defs_by_start.get(i).copied());
            let src_name = src_def.map(|d| d.name.as_str());
            if src_name == Some(tok) {
                continue; // skip the definition itself / self loops
            }
            let src_start = src_def.map(|d| d.start).unwrap_or(u32::MAX);
            if !seen.insert((src_start, tok)) {
                continue;
            }
            let src = match src_def {
                Some(d) => d.name.clone(),
                None => format!("<file:{}>", &blob.to_hex()[..12]),
            };
            out.push(EdgeRecord {
                src,
                dst: tok.to_string(),
                kind: edge_kind::CALLS,
                confidence: DEFAULT_EDGE_CONFIDENCE,
                blob: *blob,
                start: abs as u32,
                end: after as u32,
            });
        }
        offset += line.len();
    }
    out
}

fn manifest_node(rel_path: &str) -> String {
    format!("<manifest:{rel_path}>")
}

fn normalized_manifest_target(base_dir: &Path, raw: &str) -> String {
    let joined = base_dir.join(raw);
    let mut out = Vec::new();
    for component in joined.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str().to_string_lossy().to_string()),
        }
    }
    out.join("/")
}

fn quoted_spans(line: &str, line_offset: usize) -> impl Iterator<Item = (&str, u32, u32)> {
    let mut spans = Vec::new();
    let mut rest = line;
    let mut consumed = 0usize;
    while let Some(start_rel) = rest.find('"') {
        let start = consumed + start_rel + 1;
        let after_start = &rest[start_rel + 1..];
        let Some(end_rel) = after_start.find('"') else {
            break;
        };
        let end = start + end_rel;
        spans.push((
            &line[start..end],
            (line_offset + start) as u32,
            (line_offset + end) as u32,
        ));
        consumed += start_rel + 1 + end_rel + 1;
        rest = &line[consumed..];
    }
    spans.into_iter()
}

fn bead_node(id: &str) -> String {
    format!("<bead:{id}>")
}

fn bead_ref_target(raw: &str) -> Option<String> {
    let trimmed = raw.trim_matches(|c: char| {
        c == char::from(96)
            || c == '"'
            || c == '\''
            || c == ','
            || c == '.'
            || c == ':'
            || c == ';'
            || c == ')'
            || c == '('
            || c == '['
            || c == ']'
    });
    if trimmed.is_empty() || trimmed.len() > 160 {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    let path_like = trimmed.contains('/')
        && (lower.ends_with(".rs")
            || lower.ends_with(".md")
            || lower.ends_with(".toml")
            || lower.ends_with(".json")
            || lower.ends_with(".jsonl")
            || lower.ends_with(".py")
            || lower.ends_with(".ts")
            || lower.ends_with(".tsx")
            || lower.ends_with(".js")
            || lower.ends_with(".sh")
            || trimmed.starts_with("crates/")
            || trimmed.starts_with("docs/")
            || trimmed.starts_with("benchmarks/"));
    if path_like {
        return Some(format!("<file:{trimmed}>"));
    }
    if (7..=40).contains(&trimmed.len()) && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(format!("<commit:{}>", trimmed.to_ascii_lowercase()));
    }
    let symbol_like = !trimmed.contains('/')
        && (trimmed.contains("::")
            || trimmed.chars().any(|c| c.is_ascii_uppercase())
            || trimmed.ends_with("()"));
    if symbol_like {
        return Some(format!("<symbol:{}>", trimmed.trim_end_matches("()")));
    }
    None
}

fn explicit_bead_refs(line: &str) -> Vec<(String, u32, u32)> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == 96
            && let Some(rel_end) = line[i + 1..].find(char::from(96))
        {
            let start = i + 1;
            let end = start + rel_end;
            if let Some(dst) = bead_ref_target(&line[start..end]) {
                out.push((dst, start as u32, end as u32));
            }
            i = end + 1;
            continue;
        }
        if bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'.' | b'_' | b'-') {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric()
                    || matches!(bytes[i], b'.' | b'_' | b'-' | b'/' | b':' | b'(' | b')'))
            {
                i += 1;
            }
            if let Some(dst) = bead_ref_target(&line[start..i]) {
                out.push((dst, start as u32, i as u32));
            }
            continue;
        }
        i += 1;
    }
    out.sort();
    out.dedup();
    out
}

fn api_surface_node(name: &str) -> String {
    format!("<api-surface:{name}>")
}

fn manifest_package_name(text: &str) -> Option<String> {
    let mut in_package = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_package = trimmed == "[package]";
            continue;
        }
        if in_package
            && trimmed.starts_with("name")
            && trimmed.contains('=')
            && let Some((raw, _, _)) = quoted_spans(line, 0).next()
        {
            return Some(raw.to_string());
        }
    }
    None
}

fn published_rust_item(line: &str) -> Option<(String, u8, usize, usize)> {
    let trimmed = line.trim_start();
    let leading = line.len() - trimmed.len();
    let rest = trimmed.strip_prefix("pub ")?;
    let keywords = [
        ("fn ", symbol_kind::FUNCTION),
        ("struct ", symbol_kind::TYPE),
        ("enum ", symbol_kind::TYPE),
        ("trait ", symbol_kind::TYPE),
        ("type ", symbol_kind::TYPE),
        ("mod ", symbol_kind::MODULE),
        ("const ", symbol_kind::OTHER),
        ("static ", symbol_kind::OTHER),
    ];
    for (keyword, kind) in keywords {
        if let Some(after_keyword) = rest.strip_prefix(keyword) {
            let name_start = leading + "pub ".len() + keyword.len();
            let name_len = after_keyword
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                .count();
            if name_len == 0 {
                return None;
            }
            let name = line[name_start..name_start + name_len].to_string();
            return Some((name, kind, name_start, name_start + name_len));
        }
    }
    None
}

fn append_rust_api_surface_edges(
    repo_root: &Path,
    files: &[PathBuf],
    data: &mut IndexData,
) -> Result<()> {
    let mut crates = Vec::new();
    for manifest in files
        .iter()
        .filter(|path| path.file_name().is_some_and(|name| name == "Cargo.toml"))
    {
        let content = match fs::read(manifest) {
            Ok(content) if !content.is_empty() && content.len() <= 4 * 1024 * 1024 => content,
            _ => continue,
        };
        let text = match std::str::from_utf8(&content) {
            Ok(text) => text,
            Err(_) => continue,
        };
        let Some(name) = manifest_package_name(text) else {
            continue;
        };
        let base = manifest.parent().unwrap_or(repo_root).to_path_buf();
        let rel = manifest
            .strip_prefix(repo_root)
            .unwrap_or(manifest)
            .to_string_lossy()
            .to_string();
        let blob = ContentHash::of(&content);
        crates.push((name.clone(), base.clone()));
        data.defs.push(DefRecord {
            name: api_surface_node(&name),
            kind: symbol_kind::MODULE,
            blob,
            start: 0,
            end: rel.len() as u32,
            block_start: 0,
            block_end: content.len() as u32,
        });
    }

    for (crate_name, base) in crates {
        let src_dir = base.join("src");
        let surface = api_surface_node(&crate_name);
        for path in files.iter().filter(|path| path.starts_with(&src_dir)) {
            let content = match fs::read(path) {
                Ok(content) if !content.is_empty() && content.len() <= 4 * 1024 * 1024 => content,
                _ => continue,
            };
            let blob = ContentHash::of(&content);
            if !data.blobs.contains_key(&blob) {
                continue;
            }
            let text = match std::str::from_utf8(&content) {
                Ok(text) => text,
                Err(_) => continue,
            };
            let mut offset = 0usize;
            for line in text.split_inclusive('\n') {
                if let Some((name, _kind, start, end)) = published_rust_item(line) {
                    data.edges.push(EdgeRecord {
                        src: surface.clone(),
                        dst: name,
                        kind: edge_kind::REFS,
                        confidence: DEFAULT_EDGE_CONFIDENCE,
                        blob,
                        start: (offset + start) as u32,
                        end: (offset + end) as u32,
                    });
                }
                offset += line.len();
            }
        }
    }
    Ok(())
}

fn append_bead_issue_edges(repo_root: &Path, data: &mut IndexData) -> Result<()> {
    let path = repo_root.join(".beads/issues.jsonl");
    let content = match fs::read(&path) {
        Ok(content) if !content.is_empty() && content.len() <= 4 * 1024 * 1024 => content,
        _ => return Ok(()),
    };
    let blob = ContentHash::of(&content);
    if !data.blobs.contains_key(&blob) {
        return Ok(());
    }
    let text = match std::str::from_utf8(&content) {
        Ok(text) => text,
        Err(_) => return Ok(()),
    };
    let mut seen_edges = BTreeSet::new();
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let parsed: serde_json::Value = match serde_json::from_str(line.trim()) {
            Ok(value) => value,
            Err(_) => {
                offset += line.len();
                continue;
            }
        };
        let Some(id) = parsed.get("id").and_then(|v| v.as_str()) else {
            offset += line.len();
            continue;
        };
        let Some(id_pos) = line.find(id) else {
            offset += line.len();
            continue;
        };
        let src = bead_node(id);
        data.defs.push(DefRecord {
            name: src.clone(),
            kind: symbol_kind::OTHER,
            blob,
            start: (offset + id_pos) as u32,
            end: (offset + id_pos + id.len()) as u32,
            block_start: offset as u32,
            block_end: (offset + line.len()) as u32,
        });
        for (dst, start, end) in explicit_bead_refs(line) {
            if dst == src || !seen_edges.insert((src.clone(), dst.clone(), start, end)) {
                continue;
            }
            data.edges.push(EdgeRecord {
                src: src.clone(),
                dst,
                kind: edge_kind::REFS,
                confidence: DEFAULT_EDGE_CONFIDENCE,
                blob,
                start: offset as u32 + start,
                end: offset as u32 + end,
            });
        }
        if let Some(title) = parsed.get("title").and_then(|v| v.as_str())
            && !title.is_empty()
        {
            data.edges.push(EdgeRecord {
                src,
                dst: format!(
                    "<bead-title:{}>",
                    title.chars().take(96).collect::<String>()
                ),
                kind: edge_kind::REFS,
                confidence: DEFAULT_EDGE_CONFIDENCE,
                blob,
                start: (offset + id_pos) as u32,
                end: (offset + id_pos + id.len()) as u32,
            });
        }
        offset += line.len();
    }
    Ok(())
}

fn append_cargo_manifest_edges(
    repo_root: &Path,
    files: &[PathBuf],
    data: &mut IndexData,
) -> Result<()> {
    for path in files
        .iter()
        .filter(|path| path.file_name().is_some_and(|name| name == "Cargo.toml"))
    {
        let rel = path
            .strip_prefix(repo_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        let content = match fs::read(path) {
            Ok(content) if !content.is_empty() && content.len() <= 4 * 1024 * 1024 => content,
            _ => continue,
        };
        let blob = ContentHash::of(&content);
        if !data.blobs.contains_key(&blob) {
            continue;
        }
        let text = match std::str::from_utf8(&content) {
            Ok(text) => text,
            Err(_) => continue,
        };
        let base_dir = Path::new(&rel).parent().unwrap_or_else(|| Path::new(""));
        let src = manifest_node(&rel);
        let mut in_workspace = false;
        let mut in_workspace_members = false;
        let mut offset = 0usize;
        for line in text.split_inclusive('\n') {
            let trimmed = line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                in_workspace = trimmed == "[workspace]";
                in_workspace_members = false;
            }
            let starts_members =
                in_workspace && trimmed.starts_with("members") && trimmed.contains('=');
            if starts_members && trimmed.contains('[') && !trimmed.contains(']') {
                in_workspace_members = true;
            }
            let records_members = starts_members || in_workspace_members;
            let records_path_dep =
                !records_members && trimmed.contains("path") && trimmed.contains('=');
            if records_members || records_path_dep {
                for (raw, start, end) in quoted_spans(line, offset) {
                    let normalized = normalized_manifest_target(base_dir, raw);
                    let dst = if records_members {
                        format!("<workspace-member:{normalized}>")
                    } else {
                        format!("<path-dependency:{normalized}>")
                    };
                    data.edges.push(EdgeRecord {
                        src: src.clone(),
                        dst,
                        kind: edge_kind::IMPORTS,
                        confidence: DEFAULT_EDGE_CONFIDENCE,
                        blob,
                        start,
                        end,
                    });
                    let target_manifest = repo_root.join(&normalized).join("Cargo.toml");
                    if let Ok(target_content) = fs::read_to_string(&target_manifest)
                        && let Some(package_name) = manifest_package_name(&target_content)
                    {
                        data.edges.push(EdgeRecord {
                            src: src.clone(),
                            dst: api_surface_node(&package_name),
                            kind: edge_kind::IMPORTS,
                            confidence: DEFAULT_EDGE_CONFIDENCE,
                            blob,
                            start,
                            end,
                        });
                    }
                }
            }
            if in_workspace_members && trimmed.contains(']') {
                in_workspace_members = false;
            }
            offset += line.len();
        }
    }
    Ok(())
}

/// Emits source-derived `BUILD_DEPENDS`, `SCHEMA_DEPENDS`, and `EFFECT_MAY_TOUCH` edges.
/// Manifests govern package sources; literal includes bind indexed files; build and
/// environment effects conservatively cover siblings. Markers alone emit no edges.
fn append_declared_dependency_edges(
    repo_root: &Path,
    files: &[PathBuf],
    data: &mut IndexData,
) -> Result<()> {
    const MAX_BYTES: usize = 4 * 1024 * 1024;

    let rel_of = |path: &Path| -> Option<String> {
        let rel = path.strip_prefix(repo_root).unwrap_or(path);
        let text = rel.to_str()?;
        Some(text.trim_start_matches("./").to_string())
    };
    let indexed_paths: HashSet<String> =
        data.blobs.values().map(|meta| meta.path.clone()).collect();

    // Rust source scan: include! targets, set_var/env::var effect markers.
    #[derive(Default)]
    struct FileScan {
        blob: Option<ContentHash>,
        include: Vec<(String, u32, u32)>,
        set_var_span: Option<(u32, u32)>,
        reads_env_var: bool,
    }
    let mut scanned: HashMap<String, FileScan> = HashMap::new();
    for path in files
        .iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
    {
        let Some(rel) = rel_of(path) else { continue };
        if !indexed_paths.contains(&rel) {
            continue;
        }
        let content = match fs::read(path) {
            Ok(content) if !content.is_empty() && content.len() <= MAX_BYTES => content,
            _ => continue,
        };
        let blob = ContentHash::of(&content);
        if !data.blobs.contains_key(&blob) {
            continue;
        }
        let mut scan = FileScan {
            blob: Some(blob),
            ..FileScan::default()
        };
        let dir = Path::new(&rel).parent().unwrap_or_else(|| Path::new(""));
        let mut i = 0usize;
        while i + b"include!".len() <= content.len() {
            if &content[i..i + b"include!".len()] != b"include!" {
                i += 1;
                continue;
            }
            let start = i as u32;
            let mut j = i + b"include!".len();
            while j < content.len() && matches!(content[j], b' ' | b'\t' | b'\n' | b'\r') {
                j += 1;
            }
            let literal_start = if j < content.len() && content[j] == b'(' {
                j += 1;
                while j < content.len() && matches!(content[j], b' ' | b'\t' | b'\n' | b'\r') {
                    j += 1;
                }
                if j < content.len() && content[j] == b'"' {
                    j + 1
                } else {
                    j
                }
            } else {
                j
            };
            if literal_start < content.len() && content[literal_start - 1] == b'"' {
                let mut end = literal_start;
                while end < content.len() && content[end] != b'"' {
                    if content[end] == b'\\' {
                        end += 1;
                    }
                    end += 1;
                }
                if end < content.len() {
                    let literal =
                        String::from_utf8_lossy(&content[literal_start..end]).into_owned();
                    let target = normalized_manifest_target(dir, &literal);
                    if indexed_paths.contains(&target) {
                        scan.include.push((target, start, end as u32 + 1));
                    }
                    i = end + 1;
                    continue;
                }
            }
            i += b"include!".len();
        }
        let bytes = &content;
        let mut from = 0usize;
        while let Some(off) = bytes[from..]
            .windows(b"set_var(".len())
            .position(|w| w == b"set_var(")
        {
            let abs = from + off;
            if scan.set_var_span.is_none() {
                scan.set_var_span = Some((abs as u32, (abs + b"set_var(".len()) as u32));
            }
            from = abs + b"set_var(".len();
        }
        if bytes.windows(b"env::var(".len()).any(|w| w == b"env::var(") {
            scan.reads_env_var = true;
        }
        scanned.insert(rel, scan);
    }

    // Manifests: Cargo (package + workspace members) and non-Cargo package
    // manifests. Non-Cargo manifests are path-based only (no content scan).
    struct CargoManifest {
        rel: String,
        blob: ContentHash,
        package_line: (u32, u32),
        members: Vec<(String, u32, u32)>,
        is_workspace: bool,
    }
    let mut cargo_manifests: Vec<CargoManifest> = Vec::new();
    let mut cargo_dirs: HashSet<PathBuf> = HashSet::new();
    let mut non_cargo_manifests: Vec<(String, &'static [&'static str], ContentHash)> = Vec::new();
    for path in files
        .iter()
        .filter(|path| path.file_name().is_some_and(|name| name == "Cargo.toml"))
    {
        let Some(rel) = rel_of(path) else { continue };
        let content = match fs::read(path) {
            Ok(content) if !content.is_empty() && content.len() <= MAX_BYTES => content,
            _ => continue,
        };
        let blob = ContentHash::of(&content);
        if !data.blobs.contains_key(&blob) {
            continue;
        }
        let text = match std::str::from_utf8(&content) {
            Ok(text) => text,
            Err(_) => continue,
        };
        let base_dir = Path::new(&rel).parent().unwrap_or_else(|| Path::new(""));
        let mut manifest = CargoManifest {
            rel: rel.clone(),
            blob,
            package_line: (0, 1),
            members: Vec::new(),
            is_workspace: false,
        };
        let mut has_package = false;
        let mut in_workspace = false;
        let mut in_workspace_members = false;
        let mut offset = 0usize;
        for line in text.split_inclusive('\n') {
            let trimmed = line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                in_workspace = trimmed == "[workspace]";
                in_workspace_members = false;
                if trimmed.starts_with("[package]") && manifest.package_line == (0, 1) {
                    manifest.package_line =
                        (offset as u32, (offset + line.trim_end().len()) as u32);
                }
            }
            if trimmed == "[package]" || trimmed.starts_with("[package.") {
                has_package = true;
            }
            let starts_members =
                in_workspace && trimmed.starts_with("members") && trimmed.contains('=');
            if starts_members && trimmed.contains('[') && !trimmed.contains(']') {
                in_workspace_members = true;
            }
            let records_members = starts_members || in_workspace_members;
            if records_members {
                for (raw, start, end) in quoted_spans(line, offset) {
                    let normalized = normalized_manifest_target(base_dir, raw);
                    manifest.members.push((normalized, start, end));
                }
            }
            if in_workspace_members && trimmed.contains(']') {
                in_workspace_members = false;
            }
            offset += line.len();
        }
        manifest.is_workspace = in_workspace;
        if has_package || in_workspace {
            if let Some(dir) = path.parent() {
                cargo_dirs.insert(dir.to_path_buf());
            }
            cargo_manifests.push(manifest);
        }
    }
    let non_cargo_kinds: &[(&str, &[&str])] = &[
        ("package.json", &["js", "mjs", "cjs", "ts", "tsx", "jsx"]),
        ("pyproject.toml", &["py"]),
        ("setup.py", &["py"]),
        ("go.mod", &["go"]),
    ];
    for (name, exts) in non_cargo_kinds {
        for path in files
            .iter()
            .filter(|path| path.file_name().is_some_and(|n| n == *name))
        {
            let Some(rel) = rel_of(path) else { continue };
            let content = match fs::read(path) {
                Ok(content) if !content.is_empty() && content.len() <= MAX_BYTES => content,
                _ => continue,
            };
            let blob = ContentHash::of(&content);
            if !data.blobs.contains_key(&blob) {
                continue;
            }
            non_cargo_manifests.push((rel, exts, blob));
        }
    }

    // Package sources under a Cargo manifest dir, excluding nested packages.
    let package_sources = |manifest_abs: &Path, cargo_dirs: &HashSet<PathBuf>| -> Vec<&PathBuf> {
        let dir = manifest_abs.parent().unwrap_or_else(|| Path::new(""));
        files
            .iter()
            .filter(|path| {
                path != &manifest_abs
                    && path.extension().is_some_and(|ext| ext == "rs")
                    && path.starts_with(dir)
            })
            .filter(|path| {
                let Ok(rest) = path.strip_prefix(dir) else {
                    return false;
                };
                let mut sub = dir.to_path_buf();
                for component in rest.components() {
                    sub.push(component);
                    if sub != **path && cargo_dirs.contains(&sub) {
                        return false;
                    }
                }
                true
            })
            .collect()
    };

    for manifest in &cargo_manifests {
        let manifest_abs = repo_root.join(&manifest.rel);
        let src_node = format!("<manifest:{}>", manifest.rel);
        if manifest.package_line != (0, 1) {
            for source in package_sources(&manifest_abs, &cargo_dirs) {
                let Some(src_rel) = rel_of(source) else {
                    continue;
                };
                data.edges.push(EdgeRecord {
                    src: src_node.clone(),
                    dst: format!("<file:{src_rel}>"),
                    kind: edge_kind::BUILD_DEPENDS,
                    confidence: DEFAULT_EDGE_CONFIDENCE,
                    blob: manifest.blob,
                    start: manifest.package_line.0,
                    end: manifest.package_line.1,
                });
            }
        }
        if manifest.is_workspace {
            for (member, start, end) in &manifest.members {
                let member_manifest = format!("{member}/Cargo.toml");
                if !indexed_paths.contains(&member_manifest) {
                    continue;
                }
                data.edges.push(EdgeRecord {
                    src: src_node.clone(),
                    dst: format!("<manifest:{member_manifest}>"),
                    kind: edge_kind::BUILD_DEPENDS,
                    confidence: DEFAULT_EDGE_CONFIDENCE,
                    blob: manifest.blob,
                    start: *start,
                    end: *end,
                });
            }
        }
    }

    for (rel, exts, blob) in &non_cargo_manifests {
        let dir = Path::new(rel).parent().unwrap_or_else(|| Path::new(""));
        let dir_abs = repo_root.join(dir);
        let src_node = format!("<manifest:{rel}>");
        for path in files.iter().filter(|path| {
            path.parent().is_some_and(|parent| parent == dir_abs)
                && path
                    .extension()
                    .is_some_and(|ext| exts.contains(&ext.to_str().unwrap_or("")))
        }) {
            let Some(dst_rel) = rel_of(path) else {
                continue;
            };
            if !indexed_paths.contains(&dst_rel) {
                continue;
            }
            data.edges.push(EdgeRecord {
                src: src_node.clone(),
                dst: format!("<file:{dst_rel}>"),
                kind: edge_kind::BUILD_DEPENDS,
                confidence: DEFAULT_EDGE_CONFIDENCE,
                blob: *blob,
                start: 0,
                end: 1,
            });
        }
    }

    // SchemaDepends: literal include! targets resolvable to indexed paths.
    for (rel, scan) in &scanned {
        let src_node = format!("<file:{rel}>");
        for (target, start, end) in &scan.include {
            data.edges.push(EdgeRecord {
                src: src_node.clone(),
                dst: format!("<file:{target}>"),
                kind: edge_kind::SCHEMA_DEPENDS,
                confidence: DEFAULT_EDGE_CONFIDENCE,
                blob: scan.blob.unwrap_or(ContentHash::of(b"")),
                start: *start,
                end: *end,
            });
        }
    }

    // EffectMayTouch: build.rs may touch any sibling crate source; a file
    // calling set_var( may touch sibling crate sources that read env::var(.
    for manifest in &cargo_manifests {
        let manifest_abs = repo_root.join(&manifest.rel);
        let dir_abs = manifest_abs.parent().unwrap_or_else(|| Path::new(""));
        let sources: Vec<String> = package_sources(&manifest_abs, &cargo_dirs)
            .into_iter()
            .filter_map(|path| rel_of(path))
            .collect();
        let build_rs = dir_abs.join("build.rs");
        if let Some(build_rel) = rel_of(&build_rs)
            && let Some(build_scan) = scanned.get(&build_rel)
            && let Some(blob) = build_scan.blob
        {
            let src_node = format!("<file:{build_rel}>");
            for src_rel in &sources {
                if src_rel == &build_rel {
                    continue;
                }
                data.edges.push(EdgeRecord {
                    src: src_node.clone(),
                    dst: format!("<file:{src_rel}>"),
                    kind: edge_kind::EFFECT_MAY_TOUCH,
                    confidence: DEFAULT_EDGE_CONFIDENCE,
                    blob,
                    start: 0,
                    end: 1,
                });
            }
        }
        for (setvar_rel, setvar_scan) in &scanned {
            if !sources.iter().any(|rel| rel == setvar_rel) {
                continue;
            }
            let Some((start, end)) = setvar_scan.set_var_span else {
                continue;
            };
            let Some(blob) = setvar_scan.blob else {
                continue;
            };
            let src_node = format!("<file:{setvar_rel}>");
            for reader_rel in &sources {
                if reader_rel == setvar_rel {
                    continue;
                }
                if !scanned
                    .get(reader_rel)
                    .is_some_and(|reader| reader.reads_env_var)
                {
                    continue;
                }
                data.edges.push(EdgeRecord {
                    src: src_node.clone(),
                    dst: format!("<file:{reader_rel}>"),
                    kind: edge_kind::EFFECT_MAY_TOUCH,
                    confidence: DEFAULT_EDGE_CONFIDENCE,
                    blob,
                    start,
                    end,
                });
            }
        }
    }

    Ok(())
}

fn extract_tier_a_records(
    blob: &ContentHash,
    rel_path: &str,
    content: &[u8],
    queries: &QuerySet,
) -> (Vec<DefRecord>, Vec<EdgeRecord>, bool) {
    let input = BlobInput {
        path_hint: Some(rel_path),
        content,
        hash: *blob,
    };
    let mut facts = extract_tier_a(&input, queries);
    if !facts.parse_ok {
        return (extract_defs(blob, content), Vec::new(), false);
    }
    // Typed-edge fusion: when an LSP-backed resolver is installed, replace name-matched
    // structural call edges with call-accurate typed edges. Structural-only when no resolver is installed.
    fuse_installed_typed_edges(&mut facts, Some(rel_path), content);

    let defs = facts
        .nodes
        .iter()
        .map(|node| DefRecord {
            name: node.name.clone(),
            kind: symbol_kind_from_extract(node.kind),
            blob: *blob,
            start: node.span_start,
            end: node.span_end,
            block_start: node.block_start,
            block_end: node.block_end,
        })
        .collect();
    let edges = facts_to_edges(&facts, rel_path);
    (defs, edges, true)
}

fn symbol_kind_from_extract(kind: ExtractNodeKind) -> u8 {
    match kind {
        ExtractNodeKind::Function | ExtractNodeKind::Method => symbol_kind::FUNCTION,
        ExtractNodeKind::Struct
        | ExtractNodeKind::Enum
        | ExtractNodeKind::Trait
        | ExtractNodeKind::Type
        | ExtractNodeKind::Class
        | ExtractNodeKind::Interface => symbol_kind::TYPE,
        ExtractNodeKind::Module => symbol_kind::MODULE,
        ExtractNodeKind::Variable => symbol_kind::OTHER,
    }
}

fn edge_kind_from_extract(kind: ExtractEdgeKind) -> u8 {
    match kind {
        ExtractEdgeKind::Calls => edge_kind::CALLS,
        ExtractEdgeKind::Imports => edge_kind::IMPORTS,
        ExtractEdgeKind::Contains | ExtractEdgeKind::Implements => edge_kind::REFS,
    }
}

fn facts_to_edges(facts: &BlobFacts, rel_path: &str) -> Vec<EdgeRecord> {
    let mut id_to_name: HashMap<u32, &str> = HashMap::with_capacity(facts.nodes.len() + 1);
    for node in &facts.nodes {
        id_to_name.insert(node.id, node.name.as_str());
    }
    for node in &facts.path_nodes {
        id_to_name.insert(node.id, node.path.as_str());
    }
    facts
        .edges
        .iter()
        .filter_map(|edge| {
            // Definitions already encode file -> symbol containment with byte spans.
            // Do not duplicate every def as a REFS edge in the CSR; it bloats the
            // mmap index and does not improve callers/deps/context queries.
            if edge.kind == ExtractEdgeKind::Contains {
                return None;
            }
            let src = if edge.src == EXTRACT_FILE_NODE_ID {
                format!("<file:{}>", rel_path)
            } else {
                id_to_name.get(&edge.src)?.to_string()
            };
            let dst = if edge.dst == EXTRACT_FILE_NODE_ID {
                format!("<file:{}>", rel_path)
            } else {
                id_to_name.get(&edge.dst)?.to_string()
            };
            Some(EdgeRecord {
                src,
                dst,
                kind: edge_kind_from_extract(edge.kind),
                confidence: super::publish::confidence_to_u8_clamped(edge.confidence),
                blob: edge.evidence.blob_hash,
                start: edge.evidence.start,
                end: edge.evidence.end,
            })
        })
        .collect()
}

fn include_git_history_index() -> bool {
    std::env::var("GRAPHZERO_INCLUDE_GIT_HISTORY")
        .ok()
        .as_deref()
        == Some("1")
}

static EXTRACTION_CALL_COUNT: AtomicUsize = AtomicUsize::new(0);

pub fn extraction_call_count() -> usize {
    EXTRACTION_CALL_COUNT.load(Ordering::SeqCst)
}

pub fn reset_extraction_call_count() {
    EXTRACTION_CALL_COUNT.store(0, Ordering::SeqCst);
}

/// Re-point manifest to a matching snapshot without calling `collect`.
pub fn try_repoint_active(store_root: &Path, repo_root: &Path) -> Result<Option<SnapshotEntry>> {
    let id = super::git::repoint_active_snapshot(store_root, repo_root)?;
    let Some(id) = id else {
        return Ok(None);
    };
    let manifest = Manifest::load(store_root)?;
    Ok(manifest
        .snapshots
        .iter()
        .find(|s| s.snapshot_id == id)
        .cloned())
}

/// Content-hash hex per indexed path using the same walk/filter rules as `collect`.
pub fn worktree_content_map(repo_root: &Path) -> Result<BTreeMap<String, String>> {
    let mut files = Vec::new();
    walk_files(repo_root, &mut files)?;
    let mut out = BTreeMap::new();
    for path in &files {
        let Ok(content) = fs::read(path) else {
            continue;
        };
        if content.is_empty() || looks_binary(&content) || content.len() > 4 * 1024 * 1024 {
            continue;
        }
        let rel = path
            .strip_prefix(repo_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        out.insert(rel, ContentHash::of(&content).to_hex());
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpeculativeFileOverlay {
    pub path: String,
    /// Replacement bytes for path; None removes the file from the speculative graph.
    pub content: Option<Vec<u8>>,
}

fn cert_field<'a>(cert: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    cert.lines()
        .find_map(|line| line.strip_prefix(&prefix).map(str::trim))
}

/// Convert an expanded FSZero edit certificate plus its expanded post bytes into a GraphZero
/// speculative overlay. Active FSZero world IDs remain owned by FSZero; this adapter consumes
/// the durable per-edit cert payload that a world exposes, preserving the no-disk-mutation boundary.
pub fn overlay_from_fszero_edit_cert(
    repo_root: &Path,
    cert: &str,
    post_content: Vec<u8>,
) -> Result<SpeculativeFileOverlay> {
    let path = cert_field(cert, "path").context("FSZero edit cert missing path")?;
    let path = Path::new(path);
    let rel = path
        .strip_prefix(repo_root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string();
    Ok(SpeculativeFileOverlay {
        path: normalize_overlay_path(&rel),
        content: Some(post_content),
    })
}

fn normalize_overlay_path(path: &str) -> String {
    Path::new(path)
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            std::path::Component::CurDir => None,
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Build an in-memory graph for a planned edit without mutating the worktree or store. This is the
/// store-level primitive behind FSZero-world blast: callers resolve a world into changed file
/// bytes, pass them here, and GraphZero parses that overlay as if it existed on disk.
pub fn collect_with_content_overlays(
    repo_root: &Path,
    overlays: &[SpeculativeFileOverlay],
) -> Result<IndexData> {
    let mut files = Vec::new();
    walk_files(repo_root, &mut files)?;
    let mut contents_by_rel: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for path in files {
        let rel = path
            .strip_prefix(repo_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        if let Ok(content) = fs::read(&path) {
            contents_by_rel.insert(rel, content);
        }
    }
    for overlay in overlays {
        let rel = normalize_overlay_path(&overlay.path);
        if rel.is_empty() {
            continue;
        }
        match &overlay.content {
            Some(content) => {
                contents_by_rel.insert(rel, content.clone());
            }
            None => {
                contents_by_rel.remove(&rel);
            }
        }
    }

    let queries: &QuerySet = &SHARED_QUERY_SET;
    let mut data = IndexData::default();
    let mut contents_by_hash: BTreeMap<ContentHash, Vec<u8>> = BTreeMap::new();
    let mut known: BTreeMap<String, ()> = BTreeMap::new();
    let mut defs_by_blob: BTreeMap<ContentHash, Vec<DefRecord>> = BTreeMap::new();
    let mut tier_a_edges_by_blob: BTreeMap<ContentHash, Vec<EdgeRecord>> = BTreeMap::new();

    for (rel, content) in contents_by_rel {
        if content.is_empty() || looks_binary(&content) || content.len() > 4 * 1024 * 1024 {
            continue;
        }
        let hash = ContentHash::of(&content);
        let (defs, tier_a_edges, parse_ok) = extract_tier_a_records(&hash, &rel, &content, queries);
        let tier_bits = if parse_ok { 0b001 } else { 0 };
        if data
            .blobs
            .insert(
                hash,
                BlobMeta {
                    path: rel,
                    mtime_nanos: 0,
                    size: content.len() as u64,
                    tier_bits,
                    content_len: content.len(),
                },
            )
            .is_none()
        {
            data.blob_order.push(hash);
        }
        for d in &defs {
            known.insert(d.name.clone(), ());
        }
        tier_a_edges_by_blob.insert(hash, tier_a_edges);
        defs_by_blob.insert(hash, defs);
        contents_by_hash.insert(hash, content);
    }

    let mut seen_edges: BTreeSet<(String, String, u8, ContentHash, u32, u32)> = BTreeSet::new();
    for hash in &data.blob_order {
        let local_defs = &defs_by_blob[hash];
        let scan_edges = contents_by_hash
            .get(hash)
            .map(|content| extract_edges(hash, content, &known, local_defs))
            .unwrap_or_default();
        let tier_a = tier_a_edges_by_blob.remove(hash).unwrap_or_default();
        for edge in tier_a.into_iter().chain(scan_edges) {
            let key = (
                edge.src.clone(),
                edge.dst.clone(),
                edge.kind,
                edge.blob,
                edge.start,
                edge.end,
            );
            if seen_edges.insert(key) {
                data.edges.push(edge);
            }
        }
        data.defs.extend(local_defs.iter().cloned());
    }
    Ok(data)
}

pub fn speculative_blast_from_overlays(
    repo_root: &Path,
    seeds: &[String],
    overlays: &[SpeculativeFileOverlay],
    depth: usize,
) -> Result<GenerationBlast> {
    let data = collect_with_content_overlays(repo_root, overlays)?;
    let state = HistoricalGraphState {
        generation_nanos: now_nanos(),
        nodes: data.defs.iter().map(node_history_key).collect(),
        edges: data.edges.iter().map(edge_history_key).collect(),
    };
    Ok(GenerationBlast {
        generation_nanos: state.generation_nanos,
        impacted: blast_radius_in_state(&state, seeds, depth),
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IncrementalCollectStats {
    /// Distinct changed/deleted worktree paths consumed by this patch.
    pub changed_paths: usize,
    /// Files whose tree-sitter facts were freshly extracted.
    pub reparsed_files: usize,
    /// True when a definition-name change forced repo-wide lexical call-edge refresh.
    pub refreshed_scan_edges: bool,
}

struct PendingHeldIndexData {
    records_generation_nanos: i64,
    known_sig: String,
    derived: DerivedLayerSizes,
}

pub struct IncrementalCollect {
    pub data: IndexData,
    pub stats: IncrementalCollectStats,
    pending_held: Option<PendingHeldIndexData>,
}

/// Host-timed incremental-index phase breakdown (env `GRAPHZERO_INDEX_PHASE_TIMING=1`).
/// Zero-filled when the flag is off -- no Instant clocks are taken.
#[derive(Clone, Debug, Default)]
pub struct IncrementalIndexTimings {
    pub collect_ms: f64,
    pub content_signature_ms: f64,
    pub write_snapshot_ms: f64,
    pub publish_ms: f64,
    pub signature_save_ms: f64,
    pub cleanup_ms: f64,
    pub total_ms: f64,
}

#[derive(Clone, Debug)]
pub struct IncrementalIndex {
    pub entry: SnapshotEntry,
    pub stats: IncrementalCollectStats,
    pub timings: IncrementalIndexTimings,
}

fn indexed_file_list(repo_root: &Path, sidecar_files: &[SidecarFile]) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = sidecar_files
        .iter()
        .map(|file| repo_root.join(&file.path))
        .collect();
    let bead_issues = repo_root.join(".beads/issues.jsonl");
    if bead_issues.is_file() && !files.iter().any(|path| path == &bead_issues) {
        files.push(bead_issues);
    }
    files
}

fn known_from_sidecar_files(sidecar_files: &[SidecarFile]) -> BTreeMap<String, ()> {
    let mut known = BTreeMap::new();
    for file in sidecar_files {
        for def in &file.defs {
            known.insert(def.name.clone(), ());
        }
    }
    known
}

// Process-held prior IndexData for watch delta materialize. Ownership
// moves out for a batch and returns only after durable publication;
// failures leave the slot empty so the next batch bootstraps from sidecars.
thread_local! {
    static HELD_INDEX_DATA: std::cell::RefCell<Option<HeldIndexData>> = const { std::cell::RefCell::new(None) };
}

struct HeldIndexData {
    store_root: PathBuf,
    snapshot_id: u64,
    records_generation_nanos: i64,
    known_sig: String,
    data: IndexData,
    derived: DerivedLayerSizes,
}

#[derive(Clone, Copy, Default)]
struct LayerSize {
    defs: usize,
    edges: usize,
}

#[derive(Clone, Copy, Default)]
struct DerivedLayerSizes {
    cargo: LayerSize,
    rust_api: LayerSize,
    bead: LayerSize,
    declared: LayerSize,
    tier_c: LayerSize,
    tier_c_enabled: bool,
    tier_c_head: Option<[u8; 20]>,
}

#[derive(Default)]
struct LayerRecords {
    defs: Vec<DefRecord>,
    edges: Vec<EdgeRecord>,
}

#[derive(Default)]
struct DerivedLayerRecords {
    cargo: LayerRecords,
    rust_api: LayerRecords,
    bead: LayerRecords,
    declared: LayerRecords,
    tier_c: LayerRecords,
}

fn held_store_key(store_root: &Path) -> PathBuf {
    fs::canonicalize(store_root).unwrap_or_else(|_| store_root.to_path_buf())
}

fn take_held_index_data(
    store_root: &Path,
    snapshot_id: u64,
    records_generation_nanos: i64,
) -> Option<HeldIndexData> {
    let held = HELD_INDEX_DATA.with(|slot| slot.borrow_mut().take())?;
    (held.store_root == held_store_key(store_root)
        && held.snapshot_id == snapshot_id
        && held.records_generation_nanos == records_generation_nanos)
        .then_some(held)
}

fn commit_held_index_data(
    store_root: &Path,
    snapshot_id: u64,
    records_generation_nanos: i64,
    known_sig: String,
    data: IndexData,
    derived: DerivedLayerSizes,
) {
    HELD_INDEX_DATA.with(|slot| {
        *slot.borrow_mut() = Some(HeldIndexData {
            store_root: held_store_key(store_root),
            snapshot_id,
            records_generation_nanos,
            known_sig,
            data,
            derived,
        });
    });
}

fn clear_held_index_data() {
    HELD_INDEX_DATA.with(|slot| *slot.borrow_mut() = None);
}

fn append_layer(
    data: &mut IndexData,
    append: impl FnOnce(&mut IndexData) -> Result<()>,
) -> Result<LayerSize> {
    let defs_before = data.defs.len();
    let edges_before = data.edges.len();
    append(data)?;
    Ok(LayerSize {
        defs: data.defs.len() - defs_before,
        edges: data.edges.len() - edges_before,
    })
}

fn tier_c_head_identity(repo_root: &Path) -> Option<[u8; 20]> {
    let repository = git2::Repository::discover(repo_root).ok()?;
    let oid = repository.head().ok()?.target()?;
    let mut identity = [0u8; 20];
    identity.copy_from_slice(oid.as_bytes());
    Some(identity)
}

fn append_all_derived_layers(
    repo_root: &Path,
    store_root: &Path,
    sidecar_files: &[SidecarFile],
    data: &mut IndexData,
) -> Result<DerivedLayerSizes> {
    let files = indexed_file_list(repo_root, sidecar_files);
    let cargo = append_layer(data, |data| {
        append_cargo_manifest_edges(repo_root, &files, data)
    })?;
    let rust_api = append_layer(data, |data| {
        append_rust_api_surface_edges(repo_root, &files, data)
    })?;
    let bead = append_layer(data, |data| append_bead_issue_edges(repo_root, data))?;
    let declared = append_layer(data, |data| {
        append_declared_dependency_edges(repo_root, &files, data)
    })?;
    let tier_c_enabled = include_git_history_index();
    let tier_c_head = tier_c_enabled
        .then(|| tier_c_head_identity(repo_root))
        .flatten();
    let tier_c = if tier_c_enabled {
        append_layer(data, |data| {
            super::git_empirical::append_tier_c_to_index(
                data,
                store_root,
                repo_root,
                super::git_empirical::DEFAULT_MAX_COMMITS,
            )
            .map(|_| ())
        })?
    } else {
        LayerSize::default()
    };
    Ok(DerivedLayerSizes {
        cargo,
        rust_api,
        bead,
        declared,
        tier_c,
        tier_c_enabled,
        tier_c_head,
    })
}

fn take_prefix<T>(values: &mut Vec<T>, count: usize) -> Option<Vec<T>> {
    (count <= values.len()).then(|| values.drain(..count).collect())
}

fn split_derived_layers(
    data: &mut IndexData,
    sizes: DerivedLayerSizes,
) -> Option<DerivedLayerRecords> {
    let total_defs = sizes
        .cargo
        .defs
        .checked_add(sizes.rust_api.defs)?
        .checked_add(sizes.bead.defs)?
        .checked_add(sizes.declared.defs)?
        .checked_add(sizes.tier_c.defs)?;
    let total_edges = sizes
        .cargo
        .edges
        .checked_add(sizes.rust_api.edges)?
        .checked_add(sizes.bead.edges)?
        .checked_add(sizes.declared.edges)?
        .checked_add(sizes.tier_c.edges)?;
    let defs_start = data.defs.len().checked_sub(total_defs)?;
    let edges_start = data.edges.len().checked_sub(total_edges)?;
    let mut defs = data.defs.split_off(defs_start);
    let mut edges = data.edges.split_off(edges_start);
    let mut take_layer = |size: LayerSize| {
        Some(LayerRecords {
            defs: take_prefix(&mut defs, size.defs)?,
            edges: take_prefix(&mut edges, size.edges)?,
        })
    };
    let records = DerivedLayerRecords {
        cargo: take_layer(sizes.cargo)?,
        rust_api: take_layer(sizes.rust_api)?,
        bead: take_layer(sizes.bead)?,
        declared: take_layer(sizes.declared)?,
        tier_c: take_layer(sizes.tier_c)?,
    };
    (defs.is_empty() && edges.is_empty()).then_some(records)
}

fn reuse_layer(data: &mut IndexData, records: LayerRecords) -> LayerSize {
    let size = LayerSize {
        defs: records.defs.len(),
        edges: records.edges.len(),
    };
    data.defs.extend(records.defs);
    data.edges.extend(records.edges);
    size
}

fn path_is_rust_api_input(rel: &str) -> bool {
    let path = Path::new(rel);
    path.file_name().is_some_and(|name| name == "Cargo.toml")
        || path
            .components()
            .any(|component| component.as_os_str() == "src")
}

fn same_edge_identity(left: &EdgeRecord, right: &EdgeRecord) -> bool {
    left.src == right.src
        && left.dst == right.dst
        && left.kind == right.kind
        && left.blob == right.blob
        && left.start == right.start
        && left.end == right.end
}

fn rust_api_crates(repo_root: &Path, files: &[PathBuf]) -> Vec<(PathBuf, String)> {
    files
        .iter()
        .filter(|path| path.file_name().is_some_and(|name| name == "Cargo.toml"))
        .filter_map(|manifest| {
            let content = fs::read(manifest).ok()?;
            if content.is_empty() || content.len() > 4 * 1024 * 1024 {
                return None;
            }
            let text = std::str::from_utf8(&content).ok()?;
            let name = manifest_package_name(text)?;
            Some((
                manifest.parent().unwrap_or(repo_root).to_path_buf(),
                api_surface_node(&name),
            ))
        })
        .collect()
}

fn patch_rust_api_layer(
    repo_root: &Path,
    files: &[PathBuf],
    changed_rels: &BTreeSet<String>,
    dropped_blobs: &BTreeSet<ContentHash>,
    prior: LayerRecords,
    data: &mut IndexData,
) -> LayerSize {
    let crates = rust_api_crates(repo_root, files);
    let LayerRecords {
        defs,
        edges: prior_edges,
    } = prior;
    let mut edges: Vec<EdgeRecord> = prior_edges
        .into_iter()
        .filter(|edge| !dropped_blobs.contains(&edge.blob))
        .collect();

    for rel in changed_rels
        .iter()
        .filter(|rel| path_is_rust_api_input(rel))
    {
        let path = repo_root.join(rel);
        let content = match fs::read(&path) {
            Ok(content) if !content.is_empty() && content.len() <= 4 * 1024 * 1024 => content,
            _ => continue,
        };
        let blob = ContentHash::of(&content);
        if !data.blobs.contains_key(&blob) {
            continue;
        }
        let Some((_, surface)) = crates
            .iter()
            .filter(|(base, _)| path.starts_with(base.join("src")))
            .max_by_key(|(base, _)| base.components().count())
        else {
            continue;
        };
        let text = match std::str::from_utf8(&content) {
            Ok(text) => text,
            Err(_) => continue,
        };
        let mut offset = 0usize;
        for line in text.split_inclusive('\n') {
            if let Some((name, _kind, start, end)) = published_rust_item(line) {
                edges.push(EdgeRecord {
                    src: surface.clone(),
                    dst: name,
                    kind: edge_kind::REFS,
                    confidence: DEFAULT_EDGE_CONFIDENCE,
                    blob,
                    start: (offset + start) as u32,
                    end: (offset + end) as u32,
                });
            }
            offset += line.len();
        }
    }

    let edge_blobs: BTreeSet<ContentHash> = edges.iter().map(|edge| edge.blob).collect();
    let blob_order: HashMap<ContentHash, (usize, String)> = data
        .blobs
        .iter()
        .filter(|(hash, _)| edge_blobs.contains(hash))
        .map(|(hash, meta)| {
            let path = repo_root.join(&meta.path);
            let crate_rank = crates
                .iter()
                .enumerate()
                .filter(|(_, (base, _))| path.starts_with(base.join("src")))
                .max_by_key(|(_, (base, _))| base.components().count())
                .map(|(rank, _)| rank)
                .unwrap_or(usize::MAX);
            (*hash, (crate_rank, meta.path.clone()))
        })
        .collect();
    edges.sort_by(|left, right| {
        blob_order
            .get(&left.blob)
            .cmp(&blob_order.get(&right.blob))
            .then_with(|| left.start.cmp(&right.start))
            .then_with(|| left.end.cmp(&right.end))
            .then_with(|| left.dst.cmp(&right.dst))
    });

    let size = LayerSize {
        defs: defs.len(),
        edges: edges.len(),
    };
    data.defs.extend(defs);
    data.edges.extend(edges);
    size
}

fn materialize_index_data_from_sidecar(
    repo_root: &Path,
    store_root: &Path,
    sidecar_files: &[SidecarFile],
    known_sig: &str,
    prior_known_sig: &str,
    changed_rels: &BTreeSet<String>,
    held: Option<HeldIndexData>,
) -> Result<(IndexData, bool, DerivedLayerSizes)> {
    if known_sig == prior_known_sig
        && let Some(held) = held
        && held.known_sig == known_sig
        && let Some((data, derived)) =
            try_delta_materialize(repo_root, store_root, sidecar_files, changed_rels, held)?
    {
        return Ok((data, false, derived));
    }

    let scan_reuse_ok = known_sig == prior_known_sig;
    let known = (!scan_reuse_ok).then(|| known_from_sidecar_files(sidecar_files));
    let mut data = IndexData::default();
    let mut seen_edges: BTreeSet<(String, String, u8, ContentHash, u32, u32)> = BTreeSet::new();
    let mut refreshed_scan_edges = false;

    for file in sidecar_files {
        let Some(hash) = ContentHash::from_hex(&file.hash) else {
            continue;
        };
        if data
            .blobs
            .insert(
                hash,
                BlobMeta {
                    path: file.path.clone(),
                    mtime_nanos: file.mtime_nanos,
                    size: file.size,
                    tier_bits: file.tier_bits,
                    content_len: file.content_len,
                },
            )
            .is_none()
        {
            data.blob_order.push(hash);
        }
        let defs: Vec<DefRecord> = file
            .defs
            .iter()
            .map(|definition| def_from_sidecar(definition, hash))
            .collect();
        let tier_a: Vec<EdgeRecord> = file
            .tier_a
            .iter()
            .map(|edge| edge_from_sidecar(edge, hash))
            .collect();
        let scan_edges: Vec<EdgeRecord> = if scan_reuse_ok {
            file.scan
                .iter()
                .map(|edge| edge_from_sidecar(edge, hash))
                .collect()
        } else {
            let path = repo_root.join(&file.path);
            match fs::read(&path) {
                Ok(content) if ContentHash::of(&content) == hash => {
                    refreshed_scan_edges = true;
                    extract_edges(&hash, &content, known.as_ref().unwrap(), &defs)
                }
                Ok(_) => anyhow::bail!(
                    "entity sidecar content changed during scan refresh: {}",
                    path.display()
                ),
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("read {} for known-signature scan refresh", path.display())
                    });
                }
            }
        };
        for edge in tier_a.into_iter().chain(scan_edges) {
            let key = (
                edge.src.clone(),
                edge.dst.clone(),
                edge.kind,
                edge.blob,
                edge.start,
                edge.end,
            );
            if seen_edges.insert(key) {
                data.edges.push(edge);
            }
        }
        data.defs.extend(defs);
    }

    let derived = append_all_derived_layers(repo_root, store_root, sidecar_files, &mut data)?;
    Ok((data, refreshed_scan_edges, derived))
}

/// Delta-apply changed sidecar files onto moved process-held IndexData.
fn try_delta_materialize(
    repo_root: &Path,
    store_root: &Path,
    sidecar_files: &[SidecarFile],
    changed_rels: &BTreeSet<String>,
    held: HeldIndexData,
) -> Result<Option<(IndexData, DerivedLayerSizes)>> {
    let mut hash_counts = BTreeMap::<ContentHash, usize>::new();
    for file in sidecar_files {
        let Some(hash) = ContentHash::from_hex(&file.hash) else {
            return Ok(None);
        };
        *hash_counts.entry(hash).or_default() += 1;
    }
    // IndexData keys blobs by content hash rather than path. Ambiguous shared
    // hashes need per-path refcounts, so retain the sound full fallback.
    if hash_counts.values().any(|count| *count > 1) {
        return Ok(None);
    }

    let HeldIndexData {
        mut data, derived, ..
    } = held;
    let Some(mut prior_layers) = split_derived_layers(&mut data, derived) else {
        return Ok(None);
    };
    let by_path: BTreeMap<&str, &SidecarFile> = sidecar_files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();

    let dropped_blobs: BTreeSet<ContentHash> = data
        .blobs
        .iter()
        .filter(|(_, meta)| changed_rels.contains(&meta.path))
        .map(|(hash, _)| *hash)
        .collect();
    data.blobs.retain(|hash, _| !dropped_blobs.contains(hash));
    data.blob_order.retain(|hash| !dropped_blobs.contains(hash));
    data.defs
        .retain(|definition| !dropped_blobs.contains(&definition.blob));
    data.edges
        .retain(|edge| !dropped_blobs.contains(&edge.blob));

    for rel in changed_rels {
        let Some(file) = by_path.get(rel.as_str()) else {
            continue;
        };
        let Some(hash) = ContentHash::from_hex(&file.hash) else {
            return Ok(None);
        };
        data.blobs.insert(
            hash,
            BlobMeta {
                path: file.path.clone(),
                mtime_nanos: file.mtime_nanos,
                size: file.size,
                tier_bits: file.tier_bits,
                content_len: file.content_len,
            },
        );
        let blob_position = data
            .blob_order
            .iter()
            .position(|existing| data.blobs[existing].path.as_str() > file.path.as_str())
            .unwrap_or(data.blob_order.len());
        data.blob_order.insert(blob_position, hash);

        let defs: Vec<DefRecord> = file
            .defs
            .iter()
            .map(|definition| def_from_sidecar(definition, hash))
            .collect();
        let def_position = data
            .defs
            .iter()
            .position(|definition| data.blobs[&definition.blob].path.as_str() > file.path.as_str())
            .unwrap_or(data.defs.len());
        data.defs.splice(def_position..def_position, defs);

        let mut new_edges = Vec::new();
        for edge in file
            .tier_a
            .iter()
            .chain(&file.scan)
            .map(|edge| edge_from_sidecar(edge, hash))
        {
            if !data
                .edges
                .iter()
                .any(|known| same_edge_identity(known, &edge))
                && !new_edges
                    .iter()
                    .any(|known| same_edge_identity(known, &edge))
            {
                new_edges.push(edge);
            }
        }
        let edge_position = data
            .edges
            .iter()
            .position(|edge| data.blobs[&edge.blob].path.as_str() > file.path.as_str())
            .unwrap_or(data.edges.len());
        data.edges.splice(edge_position..edge_position, new_edges);
    }

    let files = indexed_file_list(repo_root, sidecar_files);
    let refresh_cargo = changed_rels.iter().any(|rel| {
        Path::new(rel)
            .file_name()
            .is_some_and(|name| name == "Cargo.toml")
    });
    let refresh_rust_api = changed_rels.iter().any(|rel| {
        Path::new(rel)
            .components()
            .any(|component| component.as_os_str() == "src")
    });
    let refresh_bead = changed_rels.iter().any(|rel| rel == ".beads/issues.jsonl");
    let refresh_declared = changed_rels.iter().any(|rel| {
        const MANIFESTS: &[&str] = &[
            "Cargo.toml",
            "package.json",
            "pyproject.toml",
            "setup.py",
            "go.mod",
        ];
        const SOURCES: &[&str] = &["rs", "js", "mjs", "cjs", "ts", "tsx", "jsx", "py", "go"];
        let path = Path::new(rel);
        if path.file_name().is_some_and(|name| {
            let name = name.to_string_lossy();
            MANIFESTS.contains(&name.as_ref()) || name == "build.rs"
        }) {
            return true;
        }
        path.extension()
            .is_some_and(|ext| SOURCES.contains(&ext.to_string_lossy().as_ref()))
    });

    let cargo = if refresh_cargo {
        append_layer(&mut data, |data| {
            append_cargo_manifest_edges(repo_root, &files, data)
        })?
    } else {
        reuse_layer(&mut data, std::mem::take(&mut prior_layers.cargo))
    };
    let rust_api = if refresh_cargo {
        append_layer(&mut data, |data| {
            append_rust_api_surface_edges(repo_root, &files, data)
        })?
    } else if refresh_rust_api {
        patch_rust_api_layer(
            repo_root,
            &files,
            changed_rels,
            &dropped_blobs,
            std::mem::take(&mut prior_layers.rust_api),
            &mut data,
        )
    } else {
        reuse_layer(&mut data, std::mem::take(&mut prior_layers.rust_api))
    };
    let bead = if refresh_bead {
        append_layer(&mut data, |data| append_bead_issue_edges(repo_root, data))?
    } else {
        reuse_layer(&mut data, std::mem::take(&mut prior_layers.bead))
    };
    let declared = if refresh_declared {
        append_layer(&mut data, |data| {
            append_declared_dependency_edges(repo_root, &files, data)
        })?
    } else {
        reuse_layer(&mut data, std::mem::take(&mut prior_layers.declared))
    };
    let tier_c_enabled = include_git_history_index();
    let tier_c_head = tier_c_enabled
        .then(|| tier_c_head_identity(repo_root))
        .flatten();
    let tier_c = if tier_c_enabled != derived.tier_c_enabled || tier_c_head != derived.tier_c_head {
        if tier_c_enabled {
            append_layer(&mut data, |data| {
                super::git_empirical::append_tier_c_to_index(
                    data,
                    store_root,
                    repo_root,
                    super::git_empirical::DEFAULT_MAX_COMMITS,
                )
                .map(|_| ())
            })?
        } else {
            LayerSize::default()
        }
    } else {
        reuse_layer(&mut data, prior_layers.tier_c)
    };

    Ok(Some((
        data,
        DerivedLayerSizes {
            cargo,
            rust_api,
            bead,
            declared,
            tier_c,
            tier_c_enabled,
            tier_c_head,
        },
    )))
}

fn extract_sidecar_file(
    repo_root: &Path,
    store_root: &Path,
    git_repo: Option<&git2::Repository>,
    queries: &QuerySet,
    path: &Path,
) -> Result<Option<(SidecarFile, Vec<u8>)>> {
    let rel = rel_path_string(repo_root, path)?;
    let meta = match fs::metadata(path) {
        Ok(meta) if meta.is_file() => meta,
        Ok(_) => return Ok(None),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let content = match fs::read(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    if content.is_empty() || looks_binary(&content) || content.len() > 4 * 1024 * 1024 {
        return Ok(None);
    }
    let mtime_nanos = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let hash = ContentHash::of(&content);
    let blob_store = BlobStore::open(store_root)?;
    let mut in_git = false;
    if let Some(repo) = git_repo
        && let Ok(oid) = git2::Oid::hash_object(git2::ObjectType::Blob, &content)
        && repo.find_blob(oid).is_ok()
    {
        blob_store.record_git_oid(&hash, &oid.to_string())?;
        in_git = true;
    }
    if !in_git {
        blob_store.put(&content)?;
    }
    let (defs, tier_a_edges, parse_ok) = extract_tier_a_records(&hash, &rel, &content, queries);
    let tier_bits = if parse_ok { 0b001 } else { 0 };
    Ok(Some((
        SidecarFile {
            path: rel,
            mtime_nanos,
            size: meta.len(),
            hash: hash.to_hex(),
            tier_bits,
            content_len: content.len(),
            defs: defs.iter().map(sidecar_def).collect(),
            tier_a: tier_a_edges.iter().map(sidecar_edge).collect(),
            scan: Vec::new(),
        },
        content,
    )))
}

/// Patch the indexed graph for a small set of saved/deleted files. This is the watch-mode
/// primitive: it consumes the append-only records sidecar, reparses only the changed paths, reuses
/// all unchanged file facts, and emits the same IndexData shape as collect.
pub fn collect_changed_paths(
    repo_root: &Path,
    store_root: &Path,
    changed_paths: &[PathBuf],
) -> Result<IncrementalCollect> {
    collect_changed_paths_impl(repo_root, store_root, changed_paths, None, false)
}

fn collect_changed_paths_impl(
    repo_root: &Path,
    store_root: &Path,
    changed_paths: &[PathBuf],
    held: Option<HeldIndexData>,
    prepare_held: bool,
) -> Result<IncrementalCollect> {
    let Some(prior) = load_records_sidecar(store_root) else {
        return Ok(IncrementalCollect {
            data: collect(repo_root, store_root)?,
            stats: IncrementalCollectStats {
                changed_paths: changed_paths.len(),
                reparsed_files: 0,
                refreshed_scan_edges: true,
            },
            pending_held: None,
        });
    };

    let queries: &QuerySet = &SHARED_QUERY_SET;
    let git_repo = repo_root
        .join(".git")
        .exists()
        .then(|| git2::Repository::discover(repo_root).ok())
        .flatten();
    let mut files_by_path: BTreeMap<String, SidecarFile> = prior
        .files
        .iter()
        .cloned()
        .map(|file| (file.path.clone(), file))
        .collect();
    let mut changed_contents: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut changed = BTreeSet::new();
    let mut reparsed_files = 0usize;

    for raw_path in changed_paths {
        let path = if raw_path.is_absolute() {
            raw_path.clone()
        } else {
            repo_root.join(raw_path)
        };
        let rel = rel_path_string(repo_root, &path)?;
        if !changed.insert(rel.clone()) {
            continue;
        }
        match extract_sidecar_file(repo_root, store_root, git_repo.as_ref(), queries, &path)? {
            Some((file, content)) => {
                reparsed_files += 1;
                changed_contents.insert(file.path.clone(), content);
                files_by_path.insert(file.path.clone(), file);
            }
            None => {
                files_by_path.remove(&rel);
            }
        }
    }

    let sidecar_for_known: Vec<SidecarFile> = files_by_path.values().cloned().collect();
    let known = known_from_sidecar_files(&sidecar_for_known);
    let known_sig = known_signature(&known);
    let scan_reuse_ok = known_sig == prior.known_sig;

    for (rel, content) in changed_contents {
        let Some(file) = files_by_path.get_mut(&rel) else {
            continue;
        };
        let Some(hash) = ContentHash::from_hex(&file.hash) else {
            continue;
        };
        let defs: Vec<DefRecord> = file
            .defs
            .iter()
            .map(|d| def_from_sidecar(d, hash))
            .collect();
        file.scan = extract_edges(&hash, &content, &known, &defs)
            .iter()
            .map(sidecar_edge)
            .collect();
    }

    let mut sidecar_files: Vec<SidecarFile> = files_by_path.into_values().collect();
    sidecar_files.sort_by(|a, b| a.path.cmp(&b.path));
    let (data, refreshed_scan_edges, derived) = materialize_index_data_from_sidecar(
        repo_root,
        store_root,
        &sidecar_files,
        &known_sig,
        &prior.known_sig,
        &changed,
        held,
    )?;
    append_records_sidecar_log(store_root, Some(&prior), known_sig.clone(), sidecar_files)?;
    let records_generation_nanos = load_records_sidecar(store_root)
        .context("reload appended records sidecar")?
        .generation_nanos;
    Ok(IncrementalCollect {
        data,
        stats: IncrementalCollectStats {
            changed_paths: changed.len(),
            reparsed_files,
            refreshed_scan_edges: refreshed_scan_edges || !scan_reuse_ok,
        },
        pending_held: prepare_held.then_some(PendingHeldIndexData {
            records_generation_nanos,
            known_sig,
            derived,
        }),
    })
}

/// Collect IndexData from a repo worktree. Stores worktree-only blobs in
/// the blob store; tracked blobs resolve through the git fallback chain.
pub fn collect(repo_root: &Path, store_root: &Path) -> Result<IndexData> {
    EXTRACTION_CALL_COUNT.fetch_add(1, Ordering::SeqCst);
    let collect_t0 = Instant::now();
    let blob_store = BlobStore::open(store_root).context("open graph blob store")?;
    let git_repo = repo_root
        .join(".git")
        .exists()
        .then(|| git2::Repository::discover(repo_root).ok())
        .flatten();
    let mut files = Vec::new();
    let walk_t0 = Instant::now();
    profiled_walk(|| {
        walk_files(repo_root, &mut files).context("walk repository files")?;
        let bead_issues = repo_root.join(".beads/issues.jsonl");
        if bead_issues.is_file() {
            files.push(bead_issues);
        }
        Ok::<(), anyhow::Error>(())
    })?;
    let walk_ms = phase_ms(walk_t0.elapsed());
    phase_add(|t| {
        t.walk_ms += walk_ms;
        t.file_count = files.len();
    });

    let prior = load_records_sidecar(store_root);
    let prior_known_sig = prior
        .as_ref()
        .map(|s| s.known_sig.clone())
        .unwrap_or_default();
    let prior_by_path: BTreeMap<&str, &SidecarFile> = prior
        .as_ref()
        .map(|s| s.files.iter().map(|f| (f.path.as_str(), f)).collect())
        .unwrap_or_default();

    let queries: &QuerySet = &SHARED_QUERY_SET;
    let mut data = IndexData::default();
    // Keep only hashes between extraction batches. The scan re-reads content
    // from the blob store or worktree, bounding peak resident bytes.
    let mut extract_scan_hashes: BTreeSet<ContentHash> = BTreeSet::new();
    let mut known: BTreeMap<String, ()> = BTreeMap::new();
    let mut defs_by_blob: BTreeMap<ContentHash, Vec<DefRecord>> = BTreeMap::new();
    let mut tier_a_edges_by_blob: BTreeMap<ContentHash, Vec<EdgeRecord>> = BTreeMap::new();
    // Extraction reuse (see RecordsSidecar): scan edges + on-disk path per
    // fingerprint-unchanged blob, consumed in the edge pass below.
    let mut reused_scan_by_blob: BTreeMap<ContentHash, Vec<EdgeRecord>> = BTreeMap::new();
    let mut reused_paths: BTreeMap<ContentHash, PathBuf> = BTreeMap::new();
    let mut wrote_local_blob = false;
    let mut pending_paths: Vec<(PathBuf, String, u128, u64)> = Vec::new();

    for (file_index, path) in files.iter().enumerate() {
        if file_index % 16 == 0 {
            check_index_budget("index.collect")?;
        }
        let rel = path
            .strip_prefix(repo_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        let meta =
            fs::metadata(path).with_context(|| format!("read metadata {}", path.display()))?;
        let mtime_nanos = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);

        // Candidate for extraction reuse: mtime+size match prior sidecar.: still
        // read+hash before reuse — same-size rewrite with preserved mtime must not keep
        // stale defs/edges. Hash match skips tree-sitter; mismatch falls through to re-extract.
        if let Some(sf) = prior_by_path.get(rel.as_str())
            && mtime_nanos != 0
            && sf.mtime_nanos == mtime_nanos
            && sf.size == meta.len()
        {
            let content = match fs::read(path) {
                Ok(content) => content,
                Err(_) => {
                    pending_paths.push((path.clone(), rel, mtime_nanos, meta.len()));
                    continue;
                }
            };
            let hash = ContentHash::of(&content);
            if sf.hash == hash.to_hex() {
                if data
                    .blobs
                    .insert(
                        hash,
                        BlobMeta {
                            path: rel,
                            mtime_nanos,
                            size: meta.len(),
                            tier_bits: sf.tier_bits,
                            content_len: sf.content_len,
                        },
                    )
                    .is_none()
                {
                    data.blob_order.push(hash);
                }
                let defs: Vec<DefRecord> =
                    sf.defs.iter().map(|d| def_from_sidecar(d, hash)).collect();
                for d in &defs {
                    known.insert(d.name.clone(), ());
                }
                tier_a_edges_by_blob.insert(
                    hash,
                    sf.tier_a
                        .iter()
                        .map(|e| edge_from_sidecar(e, hash))
                        .collect(),
                );
                reused_scan_by_blob.insert(
                    hash,
                    sf.scan.iter().map(|e| edge_from_sidecar(e, hash)).collect(),
                );
                reused_paths.insert(hash, path.clone());
                defs_by_blob.insert(hash, defs);
                continue;
            }
        }

        pending_paths.push((path.clone(), rel, mtime_nanos, meta.len()));
    }

    // Parallel tree-sitter extraction in bounded batches to cap peak RAM.
    const EXTRACT_BATCH: usize = 48;
    let mut extract_ms = 0.0_f64;
    let mut blob_put_ms = 0.0_f64;
    for chunk in pending_paths.chunks(EXTRACT_BATCH) {
        let extract_t0 = Instant::now();
        let extracted: Vec<(
            String,
            u128,
            u64,
            Vec<u8>,
            ContentHash,
            Vec<DefRecord>,
            Vec<EdgeRecord>,
            bool,
        )> = profiled_extract(|| {
            let mut extracted: Vec<(
                String,
                u128,
                u64,
                Vec<u8>,
                ContentHash,
                Vec<DefRecord>,
                Vec<EdgeRecord>,
                bool,
            )> = chunk
                .par_iter()
                .filter_map(|(path, rel, mtime_nanos, size)| {
                    let content = fs::read(path).ok()?;
                    if content.is_empty()
                        || looks_binary(&content)
                        || content.len() > 4 * 1024 * 1024
                    {
                        return None;
                    }
                    let hash = ContentHash::of(&content);
                    let (defs, tier_a_edges, parse_ok) =
                        extract_tier_a_records(&hash, rel, &content, queries);
                    Some((
                        rel.clone(),
                        *mtime_nanos,
                        *size,
                        content,
                        hash,
                        defs,
                        tier_a_edges,
                        parse_ok,
                    ))
                })
                .collect();
            // Rayon execution order is not an identity contract. Stable path order
            // fixes blob/definition ID assignment even if parallel scheduling changes.
            extracted.sort_by(|left, right| left.0.cmp(&right.0));
            extracted
        });
        extract_ms += phase_ms(extract_t0.elapsed());

        // Classify git vs local puts serially (git2::Repository is !Sync), then
        // write unique local blobs in parallel — blob_put is ~45% of cold wall.
        let put_t0 = Instant::now();
        profiled_blob_put(|| -> Result<()> {
            // Git hashes the object-type header with the bytes, so an OID computed
            // as `Blob` can only identify a blob (barring a cryptographic collision).
            let git_odb = git_repo.as_ref().and_then(|repo| repo.odb().ok());
            // Dual-hash CPU: ContentHash (sha256) already ran in extract; git Oid
            // (sha1 over "blob <n>\0"+bytes) is independent and pure. Hash OIDs in
            // parallel; only odb.exists + record_git_oid need the !Sync ODB.
            let git_oids: Vec<Option<git2::Oid>> = if git_odb.is_some() {
                extracted
                    .par_iter()
                    .map(
                        |(_rel, _mtime, _size, content, _hash, _defs, _edges, _ok)| {
                            git2::Oid::hash_object(git2::ObjectType::Blob, content).ok()
                        },
                    )
                    .collect()
            } else {
                Vec::new()
            };
            let mut local_put_idx: Vec<usize> = Vec::new();
            let mut seen_put: HashSet<ContentHash> = HashSet::new();
            for (i, (_rel, _mtime, _size, _content, hash, _defs, _edges, _ok)) in
                extracted.iter().enumerate()
            {
                let mut in_git = false;
                if let Some(odb) = &git_odb
                    && let Some(oid) = git_oids.get(i).copied().flatten()
                    && odb.exists(oid)
                {
                    blob_store.record_git_oid(hash, &oid.to_string())?;
                    in_git = true;
                }
                if !in_git && seen_put.insert(*hash) {
                    local_put_idx.push(i);
                }
            }
            if !local_put_idx.is_empty() {
                wrote_local_blob = true;
                local_put_idx
                    .into_par_iter()
                    .try_for_each(|i| {
                        let (_rel, _mtime, _size, content, hash, _defs, _edges, _ok) =
                            &extracted[i];
                        blob_store.put_nosync_prehashed(*hash, content)?;
                        Ok::<(), anyhow::Error>(())
                    })
                    .context("write graph blobs")?;
            }

            for (rel, mtime_nanos, size, content, hash, defs, tier_a_edges, parse_ok) in extracted {
                let tier_bits = if parse_ok { 0b001 } else { 0 };
                if data
                    .blobs
                    .insert(
                        hash,
                        BlobMeta {
                            path: rel,
                            mtime_nanos,
                            size,
                            tier_bits,
                            content_len: content.len(),
                        },
                    )
                    .is_none()
                {
                    data.blob_order.push(hash);
                }
                for d in &defs {
                    known.insert(d.name.clone(), ());
                }
                tier_a_edges_by_blob.insert(hash, tier_a_edges);
                defs_by_blob.insert(hash, defs);
                extract_scan_hashes.insert(hash);
                // `content` dropped here — bytes already on disk via put or git.
            }
            Ok(())
        })?;
        blob_put_ms += phase_ms(put_t0.elapsed());
    }
    phase_add(|t| {
        t.extract_ms += extract_ms;
        t.blob_put_ms += blob_put_ms;
    });
    if wrote_local_blob {
        // The manifest must not publish before the unsynced blob reaches durable storage.
        maybe_crash("before_blob_sync");
        let pending_fsync = blob_store.pending_unsynced_count() as u64;
        let sync_t0 = Instant::now();
        blob_store.sync_all().context("sync graph blobs")?;
        phase_add(|t| {
            t.blob_sync_ms += phase_ms(sync_t0.elapsed());
            t.blob_fsync_count = t.blob_fsync_count.saturating_add(pending_fsync);
        });
        maybe_crash("after_blob_sync");
    }

    // Scan edges depend on the repo-global def-name set; prior results are valid only if that set is
    // unchanged. Otherwise unchanged files are re-read for the tokenize pass (still skipping
    // tree-sitter).
    let known_sig = known_signature(&known);
    let scan_reuse_ok = known_sig == prior_known_sig;
    let known_set: HashSet<&str> = known.keys().map(String::as_str).collect();

    // Parallel lexical scan for blobs that need it (new content or known-set drift).
    // Fresh extracts re-load bytes from CAS (preferred) or worktree so extract
    // batches can free content after put.
    let mut scan_jobs: Vec<(ContentHash, Vec<u8>)> = Vec::new();
    for hash in &data.blob_order {
        if extract_scan_hashes.contains(hash) {
            let content = match blob_store.get(hash) {
                Ok(Some(bytes)) => bytes,
                _ => {
                    let rel = data.blobs.get(hash).map(|m| m.path.as_str()).unwrap_or("");
                    match fs::read(repo_root.join(rel)) {
                        Ok(bytes) => bytes,
                        Err(_) => continue,
                    }
                }
            };
            // Skip worktree content whose hash differs from the indexed identity.
            if ContentHash::of(&content) != *hash {
                continue;
            }
            scan_jobs.push((*hash, content));
        } else if !scan_reuse_ok
            && let Some(content) = reused_paths.get(hash).and_then(|p| fs::read(p).ok())
        {
            scan_jobs.push((*hash, content));
        }
    }
    let scan_t0 = Instant::now();
    let mut scanned: HashMap<ContentHash, Vec<EdgeRecord>> = profiled_scan(|| {
        scan_jobs
            .into_par_iter()
            .map(|(hash, content)| {
                let local_defs = defs_by_blob.get(&hash).map(Vec::as_slice).unwrap_or(&[]);
                let edges = extract_edges_with_known(&hash, &content, &known_set, local_defs);
                (hash, edges)
            })
            .collect()
    });
    phase_add(|t| t.scan_ms += phase_ms(scan_t0.elapsed()));

    let assemble_t0 = Instant::now();
    let sidecar_files = profiled_assemble(|| -> Result<Vec<SidecarFile>> {
        let mut sidecar_files: Vec<SidecarFile> = Vec::with_capacity(data.blob_order.len());
        // Interned name IDs avoid cloning source and destination strings per edge.
        // Removing scan edges from the map avoids cloning every blob's edge list.
        let mut name_ids: HashMap<String, u32> = HashMap::new();
        let mut seen_edges: HashSet<(u32, u32, u8, ContentHash, u32, u32)> = HashSet::new();
        for hash in &data.blob_order {
            let local_defs = defs_by_blob.remove(hash).unwrap_or_default();
            // Prefer freshly scanned edges; otherwise fall back to reused sidecar scan.
            let scan_edges = scanned
                .remove(hash)
                .unwrap_or_else(|| reused_scan_by_blob.remove(hash).unwrap_or_default());
            let tier_a = tier_a_edges_by_blob.remove(hash).unwrap_or_default();
            {
                let meta = &data.blobs[hash];
                sidecar_files.push(SidecarFile {
                    path: meta.path.clone(),
                    mtime_nanos: meta.mtime_nanos,
                    size: meta.size,
                    hash: hash.to_hex(),
                    tier_bits: meta.tier_bits,
                    content_len: meta.content_len,
                    defs: local_defs.iter().map(sidecar_def).collect(),
                    tier_a: tier_a.iter().map(sidecar_edge).collect(),
                    scan: scan_edges.iter().map(sidecar_edge).collect(),
                });
            }
            for edge in tier_a.into_iter().chain(scan_edges) {
                let src_id = intern_assemble_name(&mut name_ids, &edge.src);
                let dst_id = intern_assemble_name(&mut name_ids, &edge.dst);
                let key = (src_id, dst_id, edge.kind, edge.blob, edge.start, edge.end);
                if seen_edges.insert(key) {
                    data.edges.push(edge);
                }
            }
            data.defs.extend(local_defs);
        }
        append_cargo_manifest_edges(repo_root, &files, &mut data).context("append Cargo edges")?;
        append_rust_api_surface_edges(repo_root, &files, &mut data)
            .context("append Rust API edges")?;
        append_bead_issue_edges(repo_root, &mut data).context("append bead edges")?;
        append_declared_dependency_edges(repo_root, &files, &mut data)
            .context("append declared dependency edges")?;

        if git_repo.is_some() && include_git_history_index() {
            super::git_empirical::append_tier_c_to_index(
                &mut data,
                store_root,
                repo_root,
                super::git_empirical::DEFAULT_MAX_COMMITS,
            )?;
        }
        Ok(sidecar_files)
    })?;
    phase_add(|t| t.assemble_ms += phase_ms(assemble_t0.elapsed()));
    let history_t0 = Instant::now();
    append_temporal_graph_history(repo_root, store_root, &data).context("append graph history")?;
    phase_add(|t| t.history_ms += phase_ms(history_t0.elapsed()));

    // Persist extraction reuse as an append-only latest-wins log. This keeps
    // incremental collects from rewriting the whole sidecar on every update.
    let sidecar_t0 = Instant::now();
    append_records_sidecar_log(store_root, prior.as_ref(), known_sig, sidecar_files)
        .context("append records sidecar")?;
    phase_add(|t| {
        t.sidecar_ms += phase_ms(sidecar_t0.elapsed());
        // collect-only total contribution when index_repo does not set total.
        if t.total_ms == 0.0 {
            t.total_ms = phase_ms(collect_t0.elapsed());
        }
    });
    record_index_phases_to_hist();
    Ok(data)
}

fn def_defining_range(d: &DefRecord) -> (u32, u32) {
    if d.block_end > d.block_start {
        (d.block_start, d.block_end)
    } else {
        (d.start, d.end)
    }
}

/// Mint entities using a shared per-blob content cache (shared with lexical
/// publish so multi-def files are CAS-fetched once).
fn mint_entities_from_defs_with_cache(
    shards_dir: &Path,
    snapshot_id: u64,
    data: &IndexData,
    blob_store: &BlobStore,
    content_cache: &mut BTreeMap<ContentHash, Option<Vec<u8>>>,
) -> Result<usize> {
    let mut mints: Vec<SymbolSpanMint> = Vec::with_capacity(data.defs.len());

    for d in &data.defs {
        if d.name.is_empty() {
            continue;
        }
        let content = content_cache
            .entry(d.blob)
            .or_insert_with(|| blob_store.get(&d.blob).ok().flatten());
        let Some(bytes) = content.as_deref() else {
            continue;
        };
        let (def_start, def_end) = def_defining_range(d);
        let Some(slice) = slice_defining_bytes(bytes, def_start, def_end) else {
            continue;
        };
        let Some(digest) = defining_content_digest(slice) else {
            continue;
        };
        let blob_hex = d.blob.to_hex();
        mints.push(SymbolSpanMint {
            symbol: d.name.clone(),
            content_digest: digest,
            node_ref: format!("node/{}", d.name),
            blob_span_ref: blob_span_ref(&blob_hex, d.start, d.end),
        });
    }

    if mints.is_empty() {
        let empty =
            PublishedEntityIndex::from_registry(snapshot_id, &super::entity::EntityRegistry::new());
        write_published_entities(shards_dir, snapshot_id, &empty)?;
        return Ok(0);
    }

    let (registry, _ids) = mint_symbol_spans(&mints)?;
    let index = PublishedEntityIndex::from_registry(snapshot_id, &registry);
    write_published_entities(shards_dir, snapshot_id, &index)?;
    register_entity_records(&index.entities);
    Ok(index.entities.len())
}

/// Built snapshot files ready for manifest publish.
pub struct WrittenSnapshot {
    pub entry: SnapshotEntry,
}

pub fn now_nanos() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

/// Versioned crash boundaries supported by the non-default chaos build.
pub const SUPPORTED_CRASH_POINTS: &[&str] = &[
    "before_blob_sync",
    "after_blob_sync",
    "after_shards",
    "before_rename",
    "after_publish",
];

#[cfg(feature = "crash-injection")]
#[derive(Clone, Debug)]
struct ArmedCrashPoint {
    point: String,
    capability_hash: ContentHash,
}

#[cfg(feature = "crash-injection")]
thread_local! {
    static ARMED_CRASH_POINT: RefCell<Option<ArmedCrashPoint>> = const { RefCell::new(None) };
}

/// Process-local crash authorization for one test thread. The guard is
/// available only in a `crash-injection` build and is deliberately not
/// `Send`. Environment variables alone cannot arm a fresh child process.
#[cfg(feature = "crash-injection")]
pub struct CrashAuthorizationGuard {
    _not_send: std::rc::Rc<()>,
}

#[cfg(feature = "crash-injection")]
impl Drop for CrashAuthorizationGuard {
    fn drop(&mut self) {
        ARMED_CRASH_POINT.with(|slot| *slot.borrow_mut() = None);
    }
}

/// Arm a known boundary with a per-run capability. The caller must pass the same capability as
/// `GRAPHZERO_CRASH_CAPABILITY` to the child. Inherited environment variables alone cannot arm a
/// crash because the child has no process-local guard.
#[cfg(feature = "crash-injection")]
pub fn authorize_crash_point(point: &str, capability: &str) -> Result<CrashAuthorizationGuard> {
    anyhow::ensure!(
        SUPPORTED_CRASH_POINTS.contains(&point),
        "unknown crash point: {point}"
    );
    anyhow::ensure!(
        (32..=256).contains(&capability.len()),
        "crash capability must contain 32..=256 bytes"
    );
    let authorization = ArmedCrashPoint {
        point: point.to_owned(),
        capability_hash: ContentHash::of(capability.as_bytes()),
    };
    ARMED_CRASH_POINT.with(|slot| -> Result<()> {
        let mut slot = slot
            .try_borrow_mut()
            .context("crash authorization is already borrowed")?;
        anyhow::ensure!(
            slot.is_none(),
            "a crash point is already armed on this thread"
        );
        *slot = Some(authorization);
        Ok(())
    })?;
    Ok(CrashAuthorizationGuard {
        _not_send: std::rc::Rc::new(()),
    })
}

/// Crash-injection hook for recovery testing. Default builds compile this to a
/// no-op. Chaos builds require all three gates: the non-default feature, a process-local
/// authorization guard, and a matching point plus capability passed explicitly to the intended child.
#[cfg(feature = "crash-injection")]
pub fn maybe_crash(point: &str) {
    if std::env::var("GRAPHZERO_CRASH_POINT").as_deref() != Ok(point) {
        return;
    }
    let Ok(capability) = std::env::var("GRAPHZERO_CRASH_CAPABILITY") else {
        return;
    };
    let capability_hash = ContentHash::of(capability.as_bytes());
    let authorized = ARMED_CRASH_POINT.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|armed| armed.point == point && armed.capability_hash == capability_hash)
    });
    if authorized {
        std::process::abort();
    }
}

#[cfg(not(feature = "crash-injection"))]
pub fn maybe_crash(_point: &str) {}

pub fn global_file_name(snapshot_id: u64) -> String {
    format!("global_{snapshot_id:08}.bin")
}

pub fn shard_file_name(snapshot_id: u64, idx: usize) -> String {
    format!("shard_{snapshot_id:08}_{idx:04}.bin")
}

pub fn paths_file_name(snapshot_id: u64) -> String {
    format!("paths_{snapshot_id:08}.txt")
}

/// Serialize IndexData into shards + global + paths sidecar, then publish the
/// manifest atomically. `segment_ids` lists wal segments folded into this
/// snapshot. Build the global symbol table and owned name→id map used by spans/CSR/sidecars.
fn write_snapshot_symbol_table(
    data: &IndexData,
) -> Result<crate::store::symbol_table::BuiltSymbolTable> {
    let mut stb = SymbolTableBuilder::new();
    for d in &data.defs {
        stb.insert(&d.name, d.kind, 0);
    }
    for e in &data.edges {
        stb.insert(&e.src, symbol_kind::OTHER, 0);
        stb.insert(&e.dst, symbol_kind::OTHER, 0);
    }
    stb.build()
}

/// Name → dense id without cloning every name into map keys.
fn symbol_id_map(names: &[String]) -> HashMap<&str, u32> {
    names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i as u32))
        .collect()
}

fn write_snapshot_blob_index(data: &IndexData) -> BTreeMap<ContentHash, u32> {
    data.blob_order
        .iter()
        .enumerate()
        .map(|(i, h)| (*h, i as u32))
        .collect()
}

fn write_snapshot_def_spans(
    data: &IndexData,
    blob_idx_of: &BTreeMap<ContentHash, u32>,
    id_of: &HashMap<&str, u32>,
) -> Vec<SpanEntry> {
    let mut spans: Vec<SpanEntry> = data
        .defs
        .iter()
        .map(|d| def_record_to_span(d, blob_idx_of[&d.blob], id_of[d.name.as_str()]))
        .collect();
    spans.sort_by_key(|s| s.symbol_id);
    spans
}

fn write_snapshot_csr_with_provenance(
    store_root: &Path,
    data: &IndexData,
    blob_idx_of: &BTreeMap<ContentHash, u32>,
    id_of: &HashMap<&str, u32>,
    symbol_count: usize,
) -> Result<crate::store::csr::BuiltCsr> {
    let mut csr_builder = CsrBuilder::new();
    let provenance_on = super::provenance::provenance_enabled();
    let blob_store = provenance_on
        .then(|| BlobStore::open(store_root))
        .transpose()?;
    let mut content_cache: BTreeMap<ContentHash, Option<Vec<u8>>> = BTreeMap::new();
    for e in &data.edges {
        csr_builder.add_edge_with_evidence(
            id_of[e.src.as_str()],
            id_of[e.dst.as_str()],
            e.kind,
            e.confidence,
            SpanEntry {
                blob_idx: blob_idx_of[&e.blob],
                start: e.start,
                end: e.end,
                symbol_id: id_of[e.dst.as_str()],
                block_start: 0,
                block_end: 0,
            },
        );
        if provenance_on {
            let digest = crate::fast_hex_32(&e.blob.0);
            let content = if let Some(ref store) = blob_store {
                content_cache
                    .entry(e.blob)
                    .or_insert_with(|| store.get(&e.blob).ok().flatten())
                    .as_deref()
            } else {
                None
            };
            let _ = super::provenance::attach_indexer_shard_edge_provenance(
                store_root, &digest, e.start, e.end, &e.src, &e.dst, e.kind, content,
            )?;
        }
    }
    if provenance_on {
        for d in &data.defs {
            let digest = crate::fast_hex_32(&d.blob.0);
            let content = if let Some(ref store) = blob_store {
                content_cache
                    .entry(d.blob)
                    .or_insert_with(|| store.get(&d.blob).ok().flatten())
                    .as_deref()
            } else {
                None
            };
            let _ = super::provenance::attach_def_span_provenance(
                store_root,
                &digest,
                d.start,
                d.end,
                d.block_start,
                d.block_end,
                &d.name,
                content,
            )?;
        }
    }
    Ok(csr_builder.build(symbol_count))
}

fn write_snapshot_coverage(data: &IndexData) -> (CoverageBitmap, Vec<[u8; 32]>) {
    let mut coverage = CoverageBitmap::new(data.blob_order.len());
    let mut coverage_blobs = Vec::with_capacity(data.blob_order.len());
    for (i, hash) in data.blob_order.iter().enumerate() {
        coverage_blobs.push(hash.0);
        let meta = &data.blobs[hash];
        meta.apply_to_coverage(i, &mut coverage);
    }
    (coverage, coverage_blobs)
}

fn write_snapshot_partition_blobs(data: &IndexData) -> Vec<Vec<ContentHash>> {
    let mut shard_groups: Vec<Vec<ContentHash>> = Vec::new();
    let mut group: Vec<ContentHash> = Vec::new();
    let mut group_bytes = 0usize;
    for hash in &data.blob_order {
        let len = data.blobs[hash].content_len;
        if !group.is_empty() && group_bytes + len > TARGET_SHARD_SIZE {
            shard_groups.push(std::mem::take(&mut group));
            group_bytes = 0;
        }
        group.push(*hash);
        group_bytes += len;
    }
    if !group.is_empty() {
        shard_groups.push(group);
    }
    shard_groups
}

fn write_snapshot_shard_files(
    shards_dir: &Path,
    data: &IndexData,
    snapshot_id: u64,
    shard_groups: &[Vec<ContentHash>],
) -> Result<(Vec<u64>, Vec<PathBuf>)> {
    let mut defs_by_blob: BTreeMap<ContentHash, Vec<&DefRecord>> = BTreeMap::new();
    for d in &data.defs {
        defs_by_blob.entry(d.blob).or_default().push(d);
    }
    let mut shard_hashes = Vec::with_capacity(shard_groups.len());
    let mut shard_paths = Vec::with_capacity(shard_groups.len());
    for (idx, blobs) in shard_groups.iter().enumerate() {
        let shard = build_shard(blobs, data, &defs_by_blob)?;
        let path = shards_dir.join(shard_file_name(snapshot_id, idx));
        shard_hashes.push(shard.write_to_with_sync(&path, false)?);
        shard_paths.push(path);
    }
    Ok((shard_hashes, shard_paths))
}

fn write_snapshot_fsync_barrier(paths: &[PathBuf]) -> Result<()> {
    for path in paths {
        let f = fs::OpenOptions::new()
            .write(true)
            .open(path)
            .with_context(|| format!("reopen shard for sync {}", path.display()))?;
        f.sync_data()
            .with_context(|| format!("fsync shard {}", path.display()))?;
    }
    Ok(())
}

fn write_snapshot_paths_sidecar(
    shards_dir: &Path,
    data: &IndexData,
    snapshot_id: u64,
) -> Result<()> {
    let mut paths_txt = String::new();
    for hash in &data.blob_order {
        let m = &data.blobs[hash];
        paths_txt.push_str(&format!(
            "{} {} {} {} {}\n",
            hash.to_hex(),
            m.mtime_nanos,
            m.size,
            m.tier_bits,
            m.path
        ));
    }
    fs::write(shards_dir.join(paths_file_name(snapshot_id)), paths_txt)?;
    Ok(())
}

fn write_snapshot_search_sidecars(
    store_root: &Path,
    shards_dir: &Path,
    data: &IndexData,
    snapshot_id: u64,
    symbol_names: &[String],
    id_of: &HashMap<&str, u32>,
) -> Result<()> {
    // Publish-time name-bigram sidecar.
    {
        let path_pairs: Vec<(String, String)> = data
            .blob_order
            .iter()
            .map(|hash| {
                let m = &data.blobs[hash];
                (hash.to_hex(), m.path.clone())
            })
            .collect();
        let bigram = crate::store::query::NameBigramIndex::build_from_names_and_paths(
            symbol_names,
            &path_pairs,
        )?;
        crate::store::query::NameBigramIndex::write_published(shards_dir, snapshot_id, &bigram)?;
    }
    // Publish-time lexical semantic sidecar. One CAS get (+sha256 re-verify) per
    // unique def blob, shared with entity mint below. Prior code re-fetched every
    // def — multi-def files paid O(|defs|) full object reads during write_snapshot.
    {
        use crate::store::query::lexical::{LexicalDocSource, LexicalIndexBuilder};
        let blob_store = BlobStore::open(store_root)?;
        let mut content_cache: BTreeMap<ContentHash, Option<Vec<u8>>> = BTreeMap::new();
        let mut builder = LexicalIndexBuilder::new();
        for d in &data.defs {
            let id = id_of[d.name.as_str()];
            let content = content_cache
                .entry(d.blob)
                .or_insert_with(|| blob_store.get(&d.blob).ok().flatten());
            let path = data.blobs.get(&d.blob).map(|m| m.path.as_str());
            builder.add_doc(&LexicalDocSource {
                symbol_id: id,
                name: &d.name,
                blob: d.blob.0,
                start: d.start,
                end: d.end,
                block_start: d.block_start,
                block_end: d.block_end,
                path,
                content: content.as_deref(),
            });
        }
        let index = builder.finish(symbol_names.len());
        crate::store::query::LexicalSemanticIndex::write_published(
            shards_dir,
            snapshot_id,
            &index,
        )?;
        mint_entities_from_defs_with_cache(
            shards_dir,
            snapshot_id,
            data,
            &blob_store,
            &mut content_cache,
        )?;
    }
    Ok(())
}

pub fn write_snapshot(
    store_root: &Path,
    data: &IndexData,
    snapshot_id: u64,
    segment_ids: Vec<u64>,
) -> Result<WrittenSnapshot> {
    let shards_dir = store_root.join("shards");
    fs::create_dir_all(&shards_dir)?;

    let mut symbols = write_snapshot_symbol_table(data)?;
    let blob_idx_of = write_snapshot_blob_index(data);
    let name_count = symbols.names.len();
    let (spans, csr) = {
        // Borrow names only for span/CSR id lookup — no BTreeMap of owned Strings.
        let id_of = symbol_id_map(&symbols.names);
        let spans = write_snapshot_def_spans(data, &blob_idx_of, &id_of);
        let csr =
            write_snapshot_csr_with_provenance(store_root, data, &blob_idx_of, &id_of, name_count)?;
        (spans, csr)
    };
    // Move names out before packing the global shard (on-disk uses
    // name_bytes). Avoids a third full Vec<String> clone for search sidecars.
    let symbol_names = std::mem::take(&mut symbols.names);
    let (coverage, coverage_blobs) = write_snapshot_coverage(data);
    let shard_groups = write_snapshot_partition_blobs(data);
    let (shard_hashes, mut shard_paths) =
        write_snapshot_shard_files(&shards_dir, data, snapshot_id, &shard_groups)?;

    let global = ShardBuilder {
        symbols,
        spans,
        csr,
        trigrams: Vec::new(),
        coverage_blobs,
        coverage,
    };
    let global_path = shards_dir.join(global_file_name(snapshot_id));
    let global_hash = global.write_to_with_sync(&global_path, false)?;
    shard_paths.push(global_path);
    // One durability barrier for all shard/global writes before manifest publish.
    write_snapshot_fsync_barrier(&shard_paths)?;

    write_snapshot_paths_sidecar(&shards_dir, data, snapshot_id)?;
    let id_of = symbol_id_map(&symbol_names);
    write_snapshot_search_sidecars(
        store_root,
        &shards_dir,
        data,
        snapshot_id,
        &symbol_names,
        &id_of,
    )?;

    maybe_crash("after_shards");

    let entry = SnapshotEntry {
        snapshot_id,
        timestamp_nanos: now_nanos(),
        global_hash,
        shard_hashes,
        segment_ids,
    };
    super::schema_version::write_snapshot_schema_stamp(store_root, snapshot_id)
        .context("write snapshot schema version stamp")?;
    Ok(WrittenSnapshot { entry })
}

/// Shards carry trigram postings + coverage for their blob range; the symbol graph
/// (symbols/spans/edges) lives only in the global file. Per-shard graph duplication blew the
/// size budget; all sections remain present in the format (empty), so readers are unchanged.
pub fn build_shard(
    blobs: &[ContentHash],
    data: &IndexData,
    defs_by_blob: &BTreeMap<ContentHash, Vec<&DefRecord>>,
) -> Result<ShardBuilder> {
    let empty_defs: Vec<&DefRecord> = Vec::new();
    let mut trigrams: BTreeMap<u32, TrigramPosting> = BTreeMap::new();
    let mut coverage = CoverageBitmap::new(blobs.len());
    let mut coverage_blobs = Vec::with_capacity(blobs.len());

    for (i, hash) in blobs.iter().enumerate() {
        coverage_blobs.push(hash.0);
        let meta = &data.blobs[hash];
        meta.apply_to_coverage(i, &mut coverage);
        for d in defs_by_blob.get(hash).unwrap_or(&empty_defs) {
            // Trigram postings over defined symbol names (size budget:
            // full-content trigrams cannot fit the 5% envelope; symbol-name
            // trigrams serve snap routing). blob_idx is shard-local.
            for (t, off) in extract_trigrams(d.name.as_bytes()) {
                trigrams.entry(t).or_insert(TrigramPosting {
                    trigram: t,
                    blob_idx: i as u32,
                    offset: d.start + off,
                });
            }
        }
    }
    Ok(ShardBuilder {
        symbols: SymbolTableBuilder::new().build()?,
        spans: Vec::new(),
        csr: CsrBuilder::new().build(0),
        trigrams: {
            let mut v: Vec<_> = trigrams.into_values().collect();
            sort_postings(&mut v);
            v
        },
        coverage_blobs,
        coverage,
    })
}

/// Cooperative stop for a live index: session cancel and/or wall deadline.
#[derive(Clone, Default)]
pub struct IndexBudget {
    deadline: Option<Instant>,
    cancelled: Option<Arc<AtomicBool>>,
}

impl IndexBudget {
    pub fn unbounded() -> Self {
        Self::default()
    }

    pub fn new(deadline: Option<Instant>, cancelled: Option<Arc<AtomicBool>>) -> Self {
        Self {
            deadline,
            cancelled,
        }
    }

    fn check(&self, op: &str) -> Result<()> {
        if self
            .cancelled
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
        {
            anyhow::bail!("{op} cancelled");
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            anyhow::bail!("{op} deadline exceeded");
        }
        Ok(())
    }
}

thread_local! {
    static ACTIVE_INDEX_BUDGET: RefCell<IndexBudget> = const { RefCell::new(IndexBudget {
        deadline: None,
        cancelled: None,
    }) };
}

fn check_index_budget(op: &str) -> Result<()> {
    ACTIVE_INDEX_BUDGET.with(|slot| slot.borrow().check(op))
}

struct IndexBudgetGuard {
    previous: IndexBudget,
}

impl IndexBudgetGuard {
    fn install(budget: IndexBudget) -> Self {
        let previous = ACTIVE_INDEX_BUDGET.with(|slot| slot.replace(budget));
        Self { previous }
    }
}

impl Drop for IndexBudgetGuard {
    fn drop(&mut self) {
        let previous = std::mem::take(&mut self.previous);
        ACTIVE_INDEX_BUDGET.with(|slot| {
            *slot.borrow_mut() = previous;
        });
    }
}

/// Full index: collect facts, write snapshot, publish manifest, clean up
/// superseded snapshot files and wal segments (publish semantics).
pub fn index_repo(repo_root: &Path, store_root: &Path) -> Result<SnapshotEntry> {
    index_repo_with_budget(repo_root, store_root, None, None)
}

/// Daemonless in-process reconciliation for ZeroKernel. It publishes the new
/// snapshot under the normal writer lock and performs no socket notification.
pub fn index_repo_in_process(
    repo_root: &Path,
    store_root: &Path,
    deadline: Option<Instant>,
    cancelled: Option<Arc<AtomicBool>>,
) -> Result<SnapshotEntry> {
    let _guard = IndexBudgetGuard::install(IndexBudget::new(deadline, cancelled));
    index_repo_locked(repo_root, store_root)
}

/// Forced daemonless repair for a snapshot already proven stale. Unlike the
/// normal reconciliation path, this bypasses warm and incremental caches and
/// always publishes a snapshot built from a full worktree collection.
pub fn repair_repo_in_process(
    repo_root: &Path,
    store_root: &Path,
    deadline: Option<Instant>,
    cancelled: Option<Arc<AtomicBool>>,
) -> Result<SnapshotEntry> {
    let _guard = IndexBudgetGuard::install(IndexBudget::new(deadline, cancelled));
    index_repo_locked_with_mode(repo_root, store_root, true)
}

/// Same as [`index_repo`], but stops when `cancelled` is set or `deadline` elapses.
pub fn index_repo_with_budget(
    repo_root: &Path,
    store_root: &Path,
    deadline: Option<Instant>,
    cancelled: Option<Arc<AtomicBool>>,
) -> Result<SnapshotEntry> {
    let _guard = IndexBudgetGuard::install(IndexBudget::new(deadline, cancelled));
    index_repo_locked(repo_root, store_root)
}

fn index_repo_locked(repo_root: &Path, store_root: &Path) -> Result<SnapshotEntry> {
    index_repo_locked_with_mode(repo_root, store_root, false)
}

fn index_repo_locked_with_mode(
    repo_root: &Path,
    store_root: &Path,
    force_full: bool,
) -> Result<SnapshotEntry> {
    check_index_budget("index")?;
    // Full reconciliation invalidates daemon-held incremental ownership.
    clear_held_index_data();
    fs::create_dir_all(store_root).context("create graph store")?;
    let _lock = WriterLock::acquire(store_root).context("acquire writer lock")?;
    let index_t0 = Instant::now();
    phase_begin();

    let mut manifest = Manifest::load(store_root).context("load graph manifest")?;
    let (warm_entry, fingerprint_scan) =
        try_fast_warm_index(repo_root, store_root, &manifest).context("scan warm index")?;
    if !force_full && let Some(entry) = warm_entry {
        phase_add(|t| {
            t.warm_shortcircuit = true;
            t.total_ms = phase_ms(index_t0.elapsed());
        });
        record_index_phases_to_hist();
        return Ok(entry);
    }

    check_index_budget("index.scan")?;
    let incremental_limit = (fingerprint_scan.files.len() / 10).max(1);
    let data = if !force_full
        && manifest.latest().is_some()
        && load_records_sidecar(store_root).is_some()
        && !fingerprint_scan.changed_paths.is_empty()
        && fingerprint_scan.changed_paths.len() <= incremental_limit
    {
        match collect_changed_paths(repo_root, store_root, &fingerprint_scan.changed_paths) {
            Ok(incremental) => incremental.data,
            Err(error) => {
                tracing::warn!(
                    error = %format!("{error:#}"),
                    "incremental graph refresh failed; rebuilding the full index"
                );
                collect(repo_root, store_root)
                    .context("collect repository after incremental refresh failure")?
            }
        }
    } else {
        collect(repo_root, store_root).context("collect repository")?
    };
    let content_sig = index_content_signature(&data);
    if !force_full
        && let Some(prev) = load_index_content_signature(store_root)
        && prev == content_sig
        && let Some(entry) = manifest.latest()
    {
        // Fully warm: graph bytes unchanged since last publish. Persist the
        // upgraded/stat-refreshed fingerprint even when no snapshot is emitted.
        save_worktree_fingerprint_files(store_root, fingerprint_scan.files)?;
        phase_add(|t| t.total_ms = phase_ms(index_t0.elapsed()));
        return Ok(entry.clone());
    }

    let snapshot_id = manifest.latest().map_or(1, |s| s.snapshot_id + 1);
    let wal_dir = store_root.join("wal");
    let segment_ids = if wal_dir.is_dir() {
        super::delta_log::DeltaLog::segment_ids(&wal_dir)?
    } else {
        Vec::new()
    };

    let write_t0 = Instant::now();
    let written = write_snapshot(store_root, &data, snapshot_id, segment_ids.clone())?;
    phase_add(|t| t.write_snapshot_ms += phase_ms(write_t0.elapsed()));
    maybe_crash("before_rename");

    let pub_t0 = Instant::now();
    manifest.snapshots.push(written.entry.clone());
    prune_manifest_to_retained_snapshots(store_root, &mut manifest)?;
    manifest.atomic_publish(store_root)?;
    // Append blob hashes to the MMR transparency log.
    {
        let mut tl =
            super::mmr::TransparencyLog::open(store_root).context("open transparency log")?;
        for hash in &data.blob_order {
            tl.append(hash.0);
        }
        tl.flush().context("flush transparency log")?;
    }
    super::git::record_head_snapshot(store_root, repo_root, written.entry.snapshot_id)?;
    maybe_crash("after_publish");
    phase_add(|t| t.manifest_publish_ms += phase_ms(pub_t0.elapsed()));
    let fp_t0 = Instant::now();
    save_index_content_signature(store_root, &content_sig)?;
    save_worktree_fingerprint_files(store_root, fingerprint_scan.files)?;
    phase_add(|t| {
        t.fingerprint_save_ms += phase_ms(fp_t0.elapsed());
        t.total_ms = phase_ms(index_t0.elapsed());
    });
    record_index_phases_to_hist();

    cleanup(store_root, &manifest, &segment_ids)?;
    Ok(written.entry)
}

/// Publish a durable snapshot from a supplied changed-path set.
pub fn index_changed_paths(
    repo_root: &Path,
    store_root: &Path,
    changed_paths: &[PathBuf],
) -> Result<IncrementalIndex> {
    index_changed_paths_locked(repo_root, store_root, changed_paths)
}

fn index_changed_paths_locked(
    repo_root: &Path,
    store_root: &Path,
    changed_paths: &[PathBuf],
) -> Result<IncrementalIndex> {
    // Same env gate as cold-index phase timing: no Instant cost when off.
    let time_phases = phase_timing_enabled();
    let total_start = time_phases.then(Instant::now);
    fs::create_dir_all(store_root).context("create graph store")?;
    let _lock = WriterLock::acquire(store_root).context("acquire writer lock")?;
    let mut manifest = Manifest::load(store_root).context("load graph manifest")?;
    let records_generation_nanos =
        load_records_sidecar(store_root).map(|records| records.generation_nanos);
    let held = manifest.latest().and_then(|entry| {
        records_generation_nanos
            .and_then(|generation| take_held_index_data(store_root, entry.snapshot_id, generation))
    });

    let collect_start = time_phases.then(Instant::now);
    let IncrementalCollect {
        data,
        stats,
        pending_held,
    } = collect_changed_paths_impl(repo_root, store_root, changed_paths, held, true)
        .context("collect watcher changed paths")?;
    let mut timings = IncrementalIndexTimings::default();
    if let Some(start) = collect_start {
        timings.collect_ms = phase_ms(start.elapsed());
    }

    let signature_start = time_phases.then(Instant::now);
    let content_sig = index_content_signature(&data);
    if let Some(start) = signature_start {
        timings.content_signature_ms = phase_ms(start.elapsed());
    }
    if let Some(previous) = load_index_content_signature(store_root)
        && previous == content_sig
        && let Some(entry) = manifest.latest()
    {
        refresh_changed_worktree_fingerprints(repo_root, store_root, changed_paths)?;
        if let Some(start) = total_start {
            timings.total_ms = phase_ms(start.elapsed());
        }
        if let Some(pending) = pending_held {
            commit_held_index_data(
                store_root,
                entry.snapshot_id,
                pending.records_generation_nanos,
                pending.known_sig,
                data,
                pending.derived,
            );
        }
        return Ok(IncrementalIndex {
            entry: entry.clone(),
            stats,
            timings,
        });
    }

    let snapshot_id = manifest
        .latest()
        .map_or(1, |snapshot| snapshot.snapshot_id + 1);
    let wal_dir = store_root.join("wal");
    let segment_ids = if wal_dir.is_dir() {
        super::delta_log::DeltaLog::segment_ids(&wal_dir)?
    } else {
        Vec::new()
    };

    let write_start = time_phases.then(Instant::now);
    let written = write_snapshot(store_root, &data, snapshot_id, segment_ids.clone())?;
    if let Some(start) = write_start {
        timings.write_snapshot_ms = phase_ms(start.elapsed());
    }

    let publish_start = time_phases.then(Instant::now);
    maybe_crash("before_rename");
    manifest.snapshots.push(written.entry.clone());
    prune_manifest_to_retained_snapshots(store_root, &mut manifest)?;
    manifest.atomic_publish(store_root)?;
    // Append blob hashes to the MMR transparency log.
    {
        let mut tl =
            super::mmr::TransparencyLog::open(store_root).context("open transparency log")?;
        for hash in &data.blob_order {
            tl.append(hash.0);
        }
        tl.flush().context("flush transparency log")?;
    }
    super::git::record_head_snapshot(store_root, repo_root, written.entry.snapshot_id)?;
    maybe_crash("after_publish");
    if let Some(start) = publish_start {
        timings.publish_ms = phase_ms(start.elapsed());
    }

    let signature_save_start = time_phases.then(Instant::now);
    save_index_content_signature(store_root, &content_sig)?;
    refresh_changed_worktree_fingerprints(repo_root, store_root, changed_paths)?;
    if let Some(start) = signature_save_start {
        timings.signature_save_ms = phase_ms(start.elapsed());
    }

    let cleanup_start = time_phases.then(Instant::now);
    cleanup(store_root, &manifest, &segment_ids)?;
    if let Some(start) = cleanup_start {
        timings.cleanup_ms = phase_ms(start.elapsed());
    }
    if let Some(start) = total_start {
        timings.total_ms = phase_ms(start.elapsed());
    }
    if let Some(pending) = pending_held {
        commit_held_index_data(
            store_root,
            written.entry.snapshot_id,
            pending.records_generation_nanos,
            pending.known_sig,
            data,
            pending.derived,
        );
    }
    Ok(IncrementalIndex {
        entry: written.entry,
        stats,
        timings,
    })
}

/// Warm checks persist byte hashes, but reuse them when high-resolution mtime
/// and size are unchanged. Changed/new files are read and hashed; deletions are
/// detected by the worktree walk.
const WORKTREE_FINGERPRINT_VERSION: u32 = 3;

#[derive(serde::Serialize, serde::Deserialize)]
struct WorktreeFingerprintFile {
    version: u32,
    /// CacheZero store schema major (absent/0 = legacy pre-stamp file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    schema_major: Option<u32>,
    /// CacheZero store schema minor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    schema_minor: Option<u32>,
    /// Producer identity (`graphzero-store@…`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    writer_version: Option<String>,
    files: Vec<WorktreeFingerprint>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct WorktreeFingerprint {
    path: String,
    #[serde(default)]
    mtime_nanos: u128,
    #[serde(default)]
    size: u64,
    content_hash: String,
}

struct WorktreeFingerprintScan {
    files: Vec<WorktreeFingerprint>,
    changed_paths: Vec<PathBuf>,
}

fn worktree_fingerprint_path(store_root: &Path) -> PathBuf {
    store_root.join("worktree_fingerprint.json")
}

fn is_graph_source_path(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if name == "issues.jsonl" {
        return true;
    }
    !matches!(detect_language(&path.to_string_lossy()), Language::Unknown)
}

fn git_index_stat_clean(index: &git2::Index, rel: &str, meta: &fs::Metadata) -> bool {
    let Some(entry) = index.get_path(Path::new(rel), 0) else {
        return false;
    };
    if u64::from(entry.file_size) != meta.len() {
        return false;
    }
    let Ok(modified) = meta.modified() else {
        return false;
    };
    let Ok(mtime) = modified.duration_since(UNIX_EPOCH) else {
        return false;
    };
    i32::try_from(mtime.as_secs()).ok() == Some(entry.mtime.seconds())
}

fn collect_worktree_fingerprints(
    repo_root: &Path,
    prior: Option<&[WorktreeFingerprint]>,
) -> Result<WorktreeFingerprintScan> {
    let mut files = Vec::new();
    walk_files(repo_root, &mut files)?;
    let bead_issues = repo_root.join(".beads/issues.jsonl");
    if bead_issues.is_file() {
        files.push(bead_issues);
    }
    files.retain(|path| is_graph_source_path(path));
    let prior_by_path: BTreeMap<&str, &WorktreeFingerprint> = prior
        .unwrap_or_default()
        .iter()
        .map(|fingerprint| (fingerprint.path.as_str(), fingerprint))
        .collect();
    // Capture metadata once per file and share it between the Git-clean pre-pass
    // and hash pass.
    let metas: Vec<(String, std::fs::Metadata)> = files
        .iter()
        .filter_map(|path| {
            let meta = fs::metadata(path).ok()?;
            let rel = path
                .strip_prefix(repo_root)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned();
            Some((rel, meta))
        })
        .collect();
    let git_index = git2::Repository::discover(repo_root)
        .ok()
        .and_then(|repo| repo.index().ok());
    let git_clean: HashSet<String> = git_index
        .as_ref()
        .map(|index| {
            metas
                .iter()
                .filter(|(rel, meta)| git_index_stat_clean(index, rel, meta))
                .map(|(rel, _)| rel.clone())
                .collect()
        })
        .unwrap_or_default();
    let hashed = files
        .par_iter()
        .enumerate()
        .map(
            |(file_index, path)| -> Result<(String, u128, u64, String)> {
                if file_index % 8 == 0 {
                    check_index_budget("index.fingerprint")?;
                }
                let meta = fs::metadata(path)?;
                let size = meta.len();
                let mtime_nanos = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                let rel = path
                    .strip_prefix(repo_root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();
                if size == 0 || size > 4 * 1024 * 1024 {
                    return Ok((rel, mtime_nanos, size, String::new()));
                }
                if let Some(prior) = prior_by_path.get(rel.as_str())
                    && prior.mtime_nanos == mtime_nanos
                    && prior.size == size
                    && git_clean.contains(&rel)
                {
                    return Ok((rel, mtime_nanos, size, prior.content_hash.clone()));
                }
                let content = fs::read(path).with_context(|| {
                    format!("read worktree file for fingerprint {}", path.display())
                })?;
                Ok((rel, mtime_nanos, size, ContentHash::of(&content).to_hex()))
            },
        )
        .collect::<Result<Vec<_>>>()?;
    let mut out = Vec::with_capacity(hashed.len());
    let mut changed_paths = Vec::new();
    let mut present = BTreeSet::new();
    for (rel, mtime_nanos, size, content_hash) in hashed {
        present.insert(rel.clone());
        let fingerprint = WorktreeFingerprint {
            path: rel.clone(),
            mtime_nanos,
            size,
            content_hash,
        };
        if prior_by_path.get(rel.as_str()).copied() != Some(&fingerprint) {
            changed_paths.push(PathBuf::from(&rel));
        }
        out.push(fingerprint);
    }
    for old in prior.unwrap_or_default() {
        if !present.contains(&old.path) {
            changed_paths.push(PathBuf::from(&old.path));
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    changed_paths.sort();
    Ok(WorktreeFingerprintScan {
        files: out,
        changed_paths,
    })
}

fn save_worktree_fingerprint_files(
    store_root: &Path,
    files: Vec<WorktreeFingerprint>,
) -> Result<()> {
    let stamp = super::schema_version::current_store_stamp();
    let payload = WorktreeFingerprintFile {
        version: WORKTREE_FINGERPRINT_VERSION,
        schema_major: Some(stamp.schema_major),
        schema_minor: Some(stamp.schema_minor),
        writer_version: Some(stamp.writer_version),
        files,
    };
    let text = serde_json::to_string(&payload).context("serialize worktree fingerprint")?;
    fs::write(worktree_fingerprint_path(store_root), text)
        .with_context(|| format!("write worktree fingerprint under {}", store_root.display()))
}

/// Refresh only watcher-reported paths after an incremental collect. The records sidecar is the
/// authority for which paths belong in the index; paths deleted or filtered by that collect are
/// removed from the fingerprint.
fn refresh_changed_worktree_fingerprints(
    repo_root: &Path,
    store_root: &Path,
    changed_paths: &[PathBuf],
) -> Result<()> {
    let text = match fs::read_to_string(worktree_fingerprint_path(store_root)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("read worktree fingerprint for refresh"),
    };
    let prior: WorktreeFingerprintFile =
        serde_json::from_str(&text).context("parse worktree fingerprint for refresh")?;
    if prior.version != WORKTREE_FINGERPRINT_VERSION {
        return Ok(());
    }
    // Newer-major fingerprint segments must not authorize warm/incremental reuse.
    if super::schema_version::admit_fingerprint_stamp(
        prior.schema_major,
        prior.schema_minor,
        prior.writer_version.as_deref(),
    )
    .is_err()
    {
        return Ok(());
    }
    let Some(sidecar) = load_records_sidecar(store_root) else {
        return Ok(());
    };
    let indexed_paths: BTreeSet<&str> = sidecar
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    let mut files: BTreeMap<String, WorktreeFingerprint> = prior
        .files
        .into_iter()
        .map(|fingerprint| (fingerprint.path.clone(), fingerprint))
        .collect();

    for raw_path in changed_paths {
        let path = if raw_path.is_absolute() {
            raw_path.clone()
        } else {
            repo_root.join(raw_path)
        };
        let rel = rel_path_string(repo_root, &path)?;
        if !indexed_paths.contains(rel.as_str()) || !path.is_file() {
            files.remove(&rel);
            continue;
        }
        let meta = fs::metadata(&path)
            .with_context(|| format!("read metadata for fingerprint refresh {}", path.display()))?;
        let mtime_nanos = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let content = fs::read(&path).with_context(|| {
            format!(
                "read worktree file for fingerprint refresh {}",
                path.display()
            )
        })?;
        files.insert(
            rel.clone(),
            WorktreeFingerprint {
                path: rel,
                mtime_nanos,
                size: meta.len(),
                content_hash: ContentHash::of(&content).to_hex(),
            },
        );
    }

    save_worktree_fingerprint_files(store_root, files.into_values().collect())
}

fn try_fast_warm_index(
    repo_root: &Path,
    store_root: &Path,
    manifest: &Manifest,
) -> Result<(Option<SnapshotEntry>, WorktreeFingerprintScan)> {
    let prior = fs::read_to_string(worktree_fingerprint_path(store_root))
        .ok()
        .and_then(|text| serde_json::from_str::<WorktreeFingerprintFile>(&text).ok());
    let compatible_prior = prior.as_ref().filter(|prior| {
        prior.version == WORKTREE_FINGERPRINT_VERSION
            && super::schema_version::admit_fingerprint_stamp(
                prior.schema_major,
                prior.schema_minor,
                prior.writer_version.as_deref(),
            )
            .is_ok()
    });
    let current = collect_worktree_fingerprints(
        repo_root,
        compatible_prior.map(|prior| prior.files.as_slice()),
    )?;
    let warm = manifest.latest().cloned().filter(|_| {
        load_index_content_signature(store_root).is_some()
            && compatible_prior.is_some_and(|prior| prior.files == current.files)
    });
    Ok((warm, current))
}

/// Content identity for warm publish short-circuit after collect.
/// Must hash blob order *and* def/edge payloads — counts alone collide when
/// two distinct graphs share the same blob set and cardinality.
fn index_content_signature(data: &IndexData) -> String {
    // Stream sorted definition and edge lines into SHA-256 without concatenating them.
    // Dropping each stage before the next bounds peak resident memory.
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();

    for hash in &data.blob_order {
        let hex = hash.to_hex();
        hasher.update(hex.as_bytes());
        hasher.update(b"\t");
        hasher.update(data.blobs[hash].path.as_bytes());
        hasher.update(b"\n");
    }
    // Sort so signature is order-independent and stable across collect shuffles.
    let mut def_lines: Vec<String> = data
        .defs
        .iter()
        .map(|d| {
            format!(
                "d\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                d.name,
                d.kind,
                d.blob.to_hex(),
                d.start,
                d.end,
                d.block_start,
                d.block_end
            )
        })
        .collect();
    def_lines.sort_unstable();
    for line in &def_lines {
        hasher.update(line.as_bytes());
    }
    drop(def_lines);

    let mut edge_lines: Vec<String> = data
        .edges
        .iter()
        .map(|e| {
            format!(
                "e\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                e.src,
                e.dst,
                e.kind,
                e.confidence,
                e.blob.to_hex(),
                e.start,
                e.end
            )
        })
        .collect();
    edge_lines.sort_unstable();
    for line in &edge_lines {
        hasher.update(line.as_bytes());
    }

    ContentHash::from_bytes(hasher.finalize().into()).to_hex()
}

fn index_content_signature_path(store_root: &Path) -> PathBuf {
    store_root.join("index_content_sig.txt")
}

fn load_index_content_signature(store_root: &Path) -> Option<String> {
    fs::read_to_string(index_content_signature_path(store_root))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn save_index_content_signature(store_root: &Path, sig: &str) -> Result<()> {
    fs::write(index_content_signature_path(store_root), sig).with_context(|| {
        format!(
            "write index content signature under {}",
            store_root.display()
        )
    })
}

/// Retain the two newest snapshots plus every snapshot pinned by a branch.
/// Branch pointers are durable navigation roots, not disposable cache hints.
pub(crate) fn prune_manifest_to_retained_snapshots(
    store_root: &Path,
    manifest: &mut Manifest,
) -> Result<()> {
    manifest
        .snapshots
        .sort_by_key(|snapshot| snapshot.snapshot_id);
    let branch_pins = super::git::branch_snapshot_ids(store_root)?;
    let recent: BTreeSet<u64> = manifest
        .snapshots
        .iter()
        .rev()
        .take(2)
        .map(|snapshot| snapshot.snapshot_id)
        .collect();
    manifest.snapshots.retain(|snapshot| {
        recent.contains(&snapshot.snapshot_id) || branch_pins.contains(&snapshot.snapshot_id)
    });
    Ok(())
}

const DEFAULT_TRANSIENT_RETAIN_COUNT: usize = 200;
const DEFAULT_TRANSIENT_RETAIN_DAYS: u64 = 7;
const RETENTION_THROTTLE_SECS: u64 = 3600;

fn retention_env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn collect_retention_pin_text(store_root: &Path) -> Result<Vec<(PathBuf, String)>> {
    fn append_dir(dir: &Path, out: &mut Vec<(PathBuf, String)>) -> Result<()> {
        if !dir.is_dir() {
            return Ok(());
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                append_dir(&path, out)?;
            } else if let Ok(bytes) = fs::read(&path) {
                out.push((path, String::from_utf8_lossy(&bytes).into_owned()));
            }
        }
        Ok(())
    }
    let mut text = Vec::new();
    append_dir(&store_root.join("queries"), &mut text)?;
    append_dir(&store_root.join("mem"), &mut text)?;
    append_dir(&store_root.join("gc").join("pins"), &mut text)?;
    Ok(text)
}

fn prune_retention_dir(
    dir: &Path,
    pins: &[(PathBuf, String)],
    retain_count: usize,
    retain_days: u64,
    directories: bool,
) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let now = SystemTime::now();
    let max_age = Duration::from_secs(retain_days.saturating_mul(86_400));
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() != directories {
            continue;
        }
        let modified = entry
            .metadata()?
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH);
        entries.push((entry.path(), modified));
    }
    entries.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));
    for (position, (path, modified)) in entries.into_iter().enumerate() {
        let name = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if pins
            .iter()
            .any(|(source, text)| source != &path && text.contains(name))
        {
            continue;
        }
        let too_old = now.duration_since(modified).unwrap_or_default() > max_age;
        if position >= retain_count || too_old {
            if directories {
                fs::remove_dir_all(&path)?;
            } else {
                fs::remove_file(&path)?;
            }
        }
    }
    Ok(())
}

fn prune_transient_artifacts_locked(store_root: &Path) -> Result<()> {
    let pins = collect_retention_pin_text(store_root)?;
    let retain_count = retention_env_usize(
        "GRAPHZERO_TRANSIENT_RETAIN_COUNT",
        DEFAULT_TRANSIENT_RETAIN_COUNT,
    );
    let retain_days = retention_env_usize(
        "GRAPHZERO_TRANSIENT_RETAIN_DAYS",
        DEFAULT_TRANSIENT_RETAIN_DAYS as usize,
    ) as u64;
    prune_retention_dir(
        &store_root.join("queries"),
        &pins,
        retain_count,
        retain_days,
        false,
    )
}

/// Opportunistically prune unpinned query spills.
/// The timestamp marker avoids taking the store lock on every query.
pub fn prune_transient_artifacts(store_root: &Path) -> Result<()> {
    let marker = store_root.join("retention_prune.timestamp");
    if marker
        .metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age < Duration::from_secs(RETENTION_THROTTLE_SECS))
    {
        return Ok(());
    }
    let _lock = WriterLock::acquire(store_root).context("acquire retention writer lock")?;
    prune_transient_artifacts_locked(store_root)?;
    fs::write(marker, b"")?;
    Ok(())
}

fn query_evidence_snapshot_ids(store_root: &Path) -> Result<Option<BTreeSet<u64>>> {
    let query_dir = store_root.join("queries");
    if !query_dir.is_dir() {
        return Ok(Some(BTreeSet::new()));
    }
    let mut pinned = BTreeSet::new();
    for entry in fs::read_dir(&query_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let bytes = fs::read(entry.path())?;
        // queries/ mixes JSON evidence with by-design raw-text query spills.
        // A malformed JSON object cannot prove which snapshot it pins. Retain
        // every snapshot rather than letting cleanup block index publication.
        let looks_like_object = bytes
            .iter()
            .find(|b| !b.is_ascii_whitespace())
            .is_some_and(|b| *b == b'{');
        if !looks_like_object {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(
                    path = %entry.path().display(),
                    %error,
                    "malformed query evidence; retaining all snapshots"
                );
                return Ok(None);
            }
        };
        if let Some(snapshot_id) = value
            .get("snapshot_id")
            .or_else(|| value.get("snapshot"))
            .and_then(serde_json::Value::as_u64)
        {
            pinned.insert(snapshot_id);
        }
    }
    Ok(Some(pinned))
}

/// Remove snapshot files not referenced by the manifest or a branch and wal
/// segments folded into the latest snapshot. Stop-before-delete: only runs
/// after the manifest is durably published.
pub fn cleanup(store_root: &Path, manifest: &Manifest, folded_segments: &[u64]) -> Result<()> {
    let mut keep: BTreeSet<u64> = manifest.snapshots.iter().map(|s| s.snapshot_id).collect();
    keep.extend(super::git::branch_snapshot_ids(store_root)?);
    let Some(query_pins) = query_evidence_snapshot_ids(store_root)? else {
        tracing::warn!("skipping snapshot and WAL cleanup because query evidence is malformed");
        return Ok(());
    };
    keep.extend(query_pins);
    let shards_dir = store_root.join("shards");
    if shards_dir.is_dir() {
        for entry in fs::read_dir(&shards_dir)? {
            let entry = entry?;
            let file_name = entry.file_name();
            let name = file_name_to_str(&file_name, "snapshot cleanup scan")?;
            let snap_id = name
                .strip_prefix("shard_")
                .or_else(|| name.strip_prefix("global_"))
                .or_else(|| name.strip_prefix("paths_"))
                .or_else(|| name.strip_prefix("name_bigram_"))
                .or_else(|| name.strip_prefix("semantic_lexical_"))
                .or_else(|| name.strip_prefix("entities_"))
                .and_then(|s| s.split(['_', '.']).next())
                .and_then(|s| s.parse::<u64>().ok());
            if let Some(id) = snap_id
                && !keep.contains(&id)
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    let wal_dir = store_root.join("wal");
    if wal_dir.is_dir() {
        for id in folded_segments {
            let _ = fs::remove_file(wal_dir.join(format!("seg_{id:08}.log")));
        }
    }
    Ok(())
}
