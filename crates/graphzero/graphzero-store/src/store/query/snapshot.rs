//! Opened snapshot: mmap reader, pending WAL merge, symbol query, and repair.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::{ContentHash, Tier};

use super::super::coverage::CoverageBitmap;
use super::super::indexer::{extract_defs, global_file_name, shard_file_name};
use super::super::manifest::{Manifest, SnapshotEntry};
use super::super::shard::ShardReader;
use super::capsule_json::{render_budgeted_capsule, render_query_capsule_json};
use super::freshness::{
    collect_stale_from_indexed_defs, collect_stale_when_symbol_missing, merge_repaired_def_batch,
    snapshot_staleness_diagnostic,
};
use super::legacy::{
    capsule_match_for_symbol, coverage_ratios, merge_pending_defs_edges, merge_wal_into_pending,
    pending_tier_a, query_repair_parts, symbol_candidate_ids,
};
use super::locate::LocateIndex;
use super::name_bigram::NameBigramIndex;
use super::snap_edit::SnapEditIndex;
use super::types::{
    BudgetLedger, Capsule, CoverageCertificate, DestinationRef, FreshnessDiagnostics, PathRecord,
    PendingFacts, QueryCapsule, RouteDiagnostics, SnapRoute,
};
use crate::store::csr::edge_kind;
use crate::store::symbol_table::SymbolTable;
use crate::{CsrAdjacency, ReverseIndex};

/// Process-wide warm snapshot cache that avoids reopening shards and replaying the WAL on each call.
const SNAPSHOT_CACHE_CAP: usize = 4;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct SnapCacheKey {
    snapshot_id: u64,
    global_hash: u64,
    wal_fingerprint: u64,
}

struct SnapCacheEntry {
    store_root: PathBuf,
    key: SnapCacheKey,
    snap: Arc<Snapshot>,
}

fn snapshot_cache() -> &'static Mutex<Vec<SnapCacheEntry>> {
    static CACHE: OnceLock<Mutex<Vec<SnapCacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::with_capacity(SNAPSHOT_CACHE_CAP)))
}

/// In-flight cold opens for `open_cached` misses (single-flight
/// coalescing). Concurrent openers for the same `(store_root, tip key)`
/// wait on one `Snapshot::open` instead of stampeding mmap/WAL/hydrate work.
struct OpenFlight {
    /// `None` while the leader is opening; `Some` when finished (Ok or shared error text).
    state: Mutex<Option<std::result::Result<Arc<Snapshot>, String>>>,
    cv: Condvar,
}

fn open_flights() -> &'static Mutex<HashMap<(PathBuf, SnapCacheKey), Arc<OpenFlight>>> {
    static FLIGHTS: OnceLock<Mutex<HashMap<(PathBuf, SnapCacheKey), Arc<OpenFlight>>>> =
        OnceLock::new();
    FLIGHTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn wal_fingerprint(wal_dir: &Path) -> u64 {
    let Ok(rd) = fs::read_dir(wal_dir) else {
        return 0;
    };
    let mut acc: u64 = 0;
    for ent in rd.flatten() {
        let Ok(meta) = ent.metadata() else {
            continue;
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        acc = acc
            .wrapping_mul(1315423911)
            .wrapping_add(meta.len())
            .wrapping_add(modified);
    }
    acc
}

fn cache_key_for(store_root: &Path, entry: &SnapshotEntry) -> SnapCacheKey {
    SnapCacheKey {
        snapshot_id: entry.snapshot_id,
        global_hash: entry.global_hash,
        wal_fingerprint: wal_fingerprint(&store_root.join("wal")),
    }
}

/// An opened, queryable snapshot. Warm mode holds this across queries.
pub struct Snapshot {
    pub store_root: PathBuf,
    pub repo_root: Option<PathBuf>,
    pub entry: SnapshotEntry,
    global: ShardReader,
    paths: HashMap<ContentHash, PathRecord>,
    locate_index: OnceLock<Result<LocateIndex, String>>,
    /// Cached verdict for the dense, name-sorted symbol-table invariant.
    locate_fast_path: OnceLock<bool>,
    /// Snapshot-scoped verified-blob cache (graphzero blob-read hot path). `read_blob_at_path` re-reads
    /// AND re-hashes the object on every `get_hex`; blast's file-target pass reads one blob per break
    /// site, so a single op paid N full SHA-256 verifications of the same small set of files.
    blob_cache: std::sync::OnceLock<std::sync::Mutex<HashMap<String, std::sync::Arc<Vec<u8>>>>>,
    snap_edit_index: OnceLock<Result<SnapEditIndex, String>>,
    snapshot_cov_counts: OnceLock<Result<[usize; 3], String>>,
    /// Lazy per-view reverse adjacency. Building all three up front tripled peak
    /// reverse-index RSS (CALLS edges lived in all/blast/calls). Each view is
    /// O(E) once on first use of that API only.
    reverse_all: OnceLock<Result<ReverseIndex, String>>,
    reverse_blast: OnceLock<Result<ReverseIndex, String>>,
    reverse_calls: OnceLock<Result<ReverseIndex, String>>,
    precomp_test_paths: OnceLock<Vec<String>>,
    name_bigram: OnceLock<Result<NameBigramIndex, String>>,
    lexical_semantic: OnceLock<Result<super::lexical::LexicalSemanticIndex, String>>,
    precomp_silent_risks: OnceLock<Result<Vec<(String, String, String)>, String>>,
    pub(crate) pending: PendingFacts,
}

/// Host-timed Snapshot::open / open_cached stage breakdown (env `GRAPHZERO_OPEN_PHASE_TIMING=1`).
/// When the env is set, each open records wall-ms per stage and callers may drain via
/// [`take_open_phase_timings`]. When off: no Instant clocks.
#[derive(Clone, Debug, Default, Serialize)]
pub struct OpenPhaseTimings {
    pub compact_ms: f64,
    pub manifest_ms: f64,
    pub shard_open_ms: f64,
    pub wal_merge_ms: f64,
    pub paths_ms: f64,
    pub hydrate_ms: f64,
    pub total_ms: f64,
    /// True when `open_cached` returned a process-cache hit (no cold open).
    pub cache_hit: bool,
}

thread_local! {
    static OPEN_PHASE_TIMINGS: RefCell<Option<OpenPhaseTimings>> = const { RefCell::new(None) };
}

/// True when `GRAPHZERO_OPEN_PHASE_TIMING` is set (store-open stage clocks).
/// Also true when `GRAPHZERO_STAGE_HISTOGRAM` is set so samples feed the HDR sink.
/// Checked each call so tests can enable without process-once OnceLock stickiness.
pub fn open_phase_timing_enabled() -> bool {
    std::env::var_os("GRAPHZERO_OPEN_PHASE_TIMING").is_some()
        || crate::store::stage_hist::stage_histogram_enabled()
        || crate::store::perf_profile::perf_profile_enabled()
}

fn open_phase_ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn open_phase_begin() {
    if !open_phase_timing_enabled() {
        return;
    }
    OPEN_PHASE_TIMINGS.with(|slot| {
        *slot.borrow_mut() = Some(OpenPhaseTimings::default());
    });
}

fn open_phase_add(mut f: impl FnMut(&mut OpenPhaseTimings)) {
    if !open_phase_timing_enabled() {
        return;
    }
    OPEN_PHASE_TIMINGS.with(|slot| {
        if let Some(t) = slot.borrow_mut().as_mut() {
            f(t);
        }
    });
}

/// Take timings recorded by the last `Snapshot::open` / `open_cached` under phase timing.
pub fn take_open_phase_timings() -> Option<OpenPhaseTimings> {
    OPEN_PHASE_TIMINGS.with(|slot| slot.borrow_mut().take())
}

fn record_open_phases_to_hist() {
    if !crate::store::stage_hist::stage_histogram_enabled()
        && !crate::store::perf_profile::perf_profile_enabled()
    {
        return;
    }
    OPEN_PHASE_TIMINGS.with(|slot| {
        if let Some(t) = slot.borrow().as_ref() {
            crate::store::stage_hist::record_open_phases(t);
        }
    });
}

/// Whether this open call owns the phase-timing slot (`Standalone`) or continues
/// a parent `open_cached` measurement (`Continue` -- do not reset / set total).
#[derive(Clone, Copy)]
enum OpenTimingMode {
    Standalone,
    Continue,
}

/// True when `GRAPHZERO_PROFILE_SENTINELS` is set (flamegraph stage frames).
fn profile_sentinels_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("GRAPHZERO_PROFILE_SENTINELS").is_some())
}

#[inline(never)]
fn _profile_snapshot_open<R>(f: impl FnOnce() -> R) -> R {
    f()
}

impl Snapshot {
    #[tracing::instrument(skip_all, fields(store_root = %store_root.display(), has_repo = repo_root.is_some()))]
    pub fn open(store_root: &Path, repo_root: Option<&Path>) -> Result<Self> {
        Self::open_with_timing(store_root, repo_root, OpenTimingMode::Standalone)
    }

    fn open_with_timing(
        store_root: &Path,
        repo_root: Option<&Path>,
        timing_mode: OpenTimingMode,
    ) -> Result<Self> {
        let body = || {
            let time_phases = open_phase_timing_enabled();
            if matches!(timing_mode, OpenTimingMode::Standalone) {
                open_phase_begin();
            }
            let total_start = time_phases.then(Instant::now);

            // Refuse newer-major snapshot schema stamps before mmap work.
            crate::store::schema_version::admit_snapshot_schema_stamp(store_root)
                .context("admit snapshot schema version stamp")?;
            let shards_dir = store_root.join("shards");
            let mut last_err = None;
            let mut opened = None;
            for _ in 0..16 {
                let manifest_start = time_phases.then(Instant::now);
                let manifest = Manifest::load(store_root)?;
                if let Some(start) = manifest_start {
                    open_phase_add(|t| t.manifest_ms += open_phase_ms(start.elapsed()));
                }
                let Some(entry) = manifest.latest().cloned() else {
                    bail!(
                        "no snapshot published; index the workspace through ZeroKernel before querying"
                    );
                };
                let global_path = shards_dir.join(global_file_name(entry.snapshot_id));
                let shard_start = time_phases.then(Instant::now);
                match ShardReader::open(&global_path) {
                    Ok(global) => {
                        if let Some(start) = shard_start {
                            open_phase_add(|t| t.shard_open_ms += open_phase_ms(start.elapsed()));
                        }
                        opened = Some((entry, global));
                        break;
                    }
                    Err(e) => {
                        if let Some(start) = shard_start {
                            open_phase_add(|t| t.shard_open_ms += open_phase_ms(start.elapsed()));
                        }
                        last_err =
                            Some(e.context(format!("open global file {}", global_path.display())));
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                }
            }
            let Some((entry, global)) = opened else {
                return Err(last_err.unwrap_or_else(|| anyhow::anyhow!("snapshot open failed")));
            };

            let wal_start = time_phases.then(Instant::now);
            let pending = merge_wal_into_pending(&entry, &store_root.join("wal"))?;
            if let Some(start) = wal_start {
                open_phase_add(|t| t.wal_merge_ms += open_phase_ms(start.elapsed()));
            }

            let paths_start = time_phases.then(Instant::now);
            let paths = super::legacy::load_path_records(&shards_dir, entry.snapshot_id)?;
            if let Some(start) = paths_start {
                open_phase_add(|t| t.paths_ms += open_phase_ms(start.elapsed()));
            }

            // Defer entity hydration until first registry use. Query paths that never
            // touch entity views skip the sidecar parse cost.

            if matches!(timing_mode, OpenTimingMode::Standalone) {
                if let Some(start) = total_start {
                    open_phase_add(|t| t.total_ms = open_phase_ms(start.elapsed()));
                }
            }

            Ok(Self {
                store_root: store_root.to_path_buf(),
                repo_root: repo_root.map(|p| p.to_path_buf()),
                entry,
                global,
                paths,
                locate_index: OnceLock::new(),
                locate_fast_path: OnceLock::new(),
                blob_cache: OnceLock::new(),
                snap_edit_index: OnceLock::new(),
                snapshot_cov_counts: OnceLock::new(),
                reverse_all: OnceLock::new(),
                reverse_blast: OnceLock::new(),
                reverse_calls: OnceLock::new(),
                precomp_test_paths: OnceLock::new(),
                name_bigram: OnceLock::new(),
                lexical_semantic: OnceLock::new(),
                precomp_silent_risks: OnceLock::new(),
                pending,
            })
        };
        if profile_sentinels_enabled() {
            _profile_snapshot_open(body)
        } else {
            body()
        }
    }

    /// Compact an oversized WAL, then reuse a process-cached snapshot when tip identity is unchanged.
    #[tracing::instrument(skip_all, fields(store_root = %store_root.display(), has_repo = repo_root.is_some()))]
    pub fn open_cached(store_root: &Path, repo_root: Option<&Path>) -> Result<Arc<Self>> {
        Self::open_cached_with_compactor(store_root, repo_root, || {
            super::super::compaction::compact_on_open_if_needed(store_root)
        })
    }

    fn open_cached_with_compactor(
        store_root: &Path,
        repo_root: Option<&Path>,
        compact: impl FnOnce() -> Result<Option<u64>>,
    ) -> Result<Arc<Self>> {
        let time_phases = open_phase_timing_enabled();
        open_phase_begin();
        let total_start = time_phases.then(Instant::now);

        let compact_start = time_phases.then(Instant::now);
        if let Err(error) = compact()
            && !super::super::compaction::is_read_only_store_error(&error)
        {
            return Err(error);
        }
        if let Some(start) = compact_start {
            open_phase_add(|t| t.compact_ms += open_phase_ms(start.elapsed()));
        }

        let manifest_start = time_phases.then(Instant::now);
        let manifest = Manifest::load(store_root)?;
        if let Some(start) = manifest_start {
            open_phase_add(|t| t.manifest_ms += open_phase_ms(start.elapsed()));
        }
        let Some(entry) = manifest.latest().cloned() else {
            bail!("no snapshot published; index the workspace through ZeroKernel before querying");
        };
        let key = cache_key_for(store_root, &entry);
        if let Ok(guard) = snapshot_cache().lock() {
            if let Some(hit) = guard
                .iter()
                .find(|e| e.store_root == store_root && e.key == key)
            {
                if let Some(start) = total_start {
                    open_phase_add(|t| {
                        t.cache_hit = true;
                        t.total_ms = open_phase_ms(start.elapsed());
                    });
                    record_open_phases_to_hist();
                } else {
                    open_phase_add(|t| t.cache_hit = true);
                }
                return Ok(Arc::clone(&hit.snap));
            }
        }

        // Single-flight: one cold open per (store_root, tip); concurrent misses wait (0td2d).
        let flight_key = (store_root.to_path_buf(), key);
        let (flight, is_leader) = match open_flights().lock() {
            Ok(mut map) => {
                if let Some(existing) = map.get(&flight_key) {
                    (Arc::clone(existing), false)
                } else {
                    let flight = Arc::new(OpenFlight {
                        state: Mutex::new(None),
                        cv: Condvar::new(),
                    });
                    map.insert(flight_key.clone(), Arc::clone(&flight));
                    (flight, true)
                }
            }
            Err(_) => {
                // Poisoned flight map: degrade to non-coalesced open (still correct).
                let snap = Arc::new(Self::open_with_timing(
                    store_root,
                    repo_root,
                    OpenTimingMode::Continue,
                )?);
                if let Some(start) = total_start {
                    open_phase_add(|t| t.total_ms = open_phase_ms(start.elapsed()));
                }
                Self::insert_open_cache(store_root, key, &snap);
                return Ok(snap);
            }
        };

        if !is_leader {
            let mut guard = flight.state.lock().unwrap_or_else(|e| e.into_inner());
            while guard.is_none() {
                guard = flight.cv.wait(guard).unwrap_or_else(|e| e.into_inner());
            }
            match guard.as_ref().expect("open flight finished") {
                Ok(snap) => {
                    if let Some(start) = total_start {
                        open_phase_add(|t| {
                            // Shared flight result -- no cold stages on this caller.
                            t.cache_hit = true;
                            t.total_ms = open_phase_ms(start.elapsed());
                        });
                        record_open_phases_to_hist();
                    } else {
                        open_phase_add(|t| t.cache_hit = true);
                    }
                    return Ok(Arc::clone(snap));
                }
                Err(msg) => return Err(anyhow!("{msg}")),
            }
        }

        // Another completed open may fill the cache after the leader's miss check.
        if let Ok(guard) = snapshot_cache().lock() {
            if let Some(hit) = guard
                .iter()
                .find(|e| e.store_root == store_root && e.key == key)
            {
                let snap = Arc::clone(&hit.snap);
                drop(guard);
                Self::finish_open_flight(&flight, &flight_key, Ok(Arc::clone(&snap)));
                if let Some(start) = total_start {
                    open_phase_add(|t| {
                        t.cache_hit = true;
                        t.total_ms = open_phase_ms(start.elapsed());
                    });
                    record_open_phases_to_hist();
                } else {
                    open_phase_add(|t| t.cache_hit = true);
                }
                return Ok(snap);
            }
        }

        let open_result =
            Self::open_with_timing(store_root, repo_root, OpenTimingMode::Continue).map(Arc::new);
        match open_result {
            Ok(snap) => {
                if let Some(start) = total_start {
                    open_phase_add(|t| t.total_ms = open_phase_ms(start.elapsed()));
                }
                Self::insert_open_cache(store_root, key, &snap);
                Self::finish_open_flight(&flight, &flight_key, Ok(Arc::clone(&snap)));
                Ok(snap)
            }
            Err(err) => {
                Self::finish_open_flight(&flight, &flight_key, Err(err.to_string()));
                Err(err)
            }
        }
    }

    fn insert_open_cache(store_root: &Path, key: SnapCacheKey, snap: &Arc<Self>) {
        if let Ok(mut guard) = snapshot_cache().lock() {
            guard.retain(|e| e.store_root != store_root);
            if guard.len() >= SNAPSHOT_CACHE_CAP {
                guard.remove(0);
            }
            guard.push(SnapCacheEntry {
                store_root: store_root.to_path_buf(),
                key,
                snap: Arc::clone(snap),
            });
            record_open_phases_to_hist();
        }
    }

    fn finish_open_flight(
        flight: &OpenFlight,
        flight_key: &(PathBuf, SnapCacheKey),
        result: std::result::Result<Arc<Self>, String>,
    ) {
        {
            let mut guard = flight.state.lock().unwrap_or_else(|e| e.into_inner());
            *guard = Some(result);
            flight.cv.notify_all();
        }
        if let Ok(mut map) = open_flights().lock() {
            map.remove(flight_key);
        }
    }

    /// Drop cached snapshots (tests / after index publish).
    pub fn clear_open_cache() {
        if let Ok(mut guard) = snapshot_cache().lock() {
            guard.clear();
        }
    }

    /// Drop only the cached entry for `store_root`, leaving other stores warm.
    pub fn invalidate_open_cache_for(store_root: &Path) {
        if let Ok(mut guard) = snapshot_cache().lock() {
            guard.retain(|e| e.store_root != store_root);
        }
    }

    pub fn used_mmap(&self) -> bool {
        self.global.used_mmap()
    }

    pub fn symbol_count(&self) -> Result<usize> {
        Ok(super::super::symbol_table::SymbolTable::from_view(&self.global.view()?)?.len())
    }

    pub fn global_view(&self) -> Result<super::super::hot_path::ShardView<'_>> {
        self.global.view()
    }

    /// Cached ReverseIndex for callers (precomp for blast / snap-to-file handoff).
    /// Builds on first use from view + CSR. Sub-1ms after warm.
    pub fn semantic_tier_percent(&self) -> f64 {
        if let Some(Ok(index)) = self.lexical_semantic.get() {
            return index.coverage_percent();
        }
        if let Some(pct) = super::lexical::LexicalSemanticIndex::published_coverage_percent(
            &self.store_root.join("shards"),
            self.entry.snapshot_id,
        ) {
            return pct;
        }
        super::super::semantic::semantic_tier_percent_for_shards(&self.shard_paths())
    }

    /// Lexical semantic tier index (GZLX). Loads the published sidecar when
    /// present; legacy snapshots build once from the store and persist the
    /// sidecar best-effort so later cold opens skip the rebuild.
    pub fn lexical_semantic_index(&self) -> Result<&super::lexical::LexicalSemanticIndex> {
        use super::lexical::LexicalSemanticIndex;
        match self.lexical_semantic.get_or_init(|| {
            let shards = self.store_root.join("shards");
            match LexicalSemanticIndex::try_load_published(&shards, self.entry.snapshot_id) {
                Ok(Some(index)) => Ok(index),
                Ok(None) => {
                    let index = LexicalSemanticIndex::build_from_snapshot(self)
                        .map_err(|e| e.to_string())?;
                    let _ = LexicalSemanticIndex::write_published(
                        &shards,
                        self.entry.snapshot_id,
                        &index,
                    );
                    Ok(index)
                }
                Err(e) => Err(e.to_string()),
            }
        }) {
            Ok(index) => Ok(index),
            Err(err) => Err(anyhow!("lexical semantic index failed: {err}")),
        }
    }

    /// Tier-C snapshot hooks for `hot` and `changes`.
    pub fn git_empirical_capsule(
        &self,
        query: &str,
        budget: usize,
        check_freshness: bool,
    ) -> Result<Option<QueryCapsule>> {
        let route = match query {
            "hot" => SnapRoute::Hot,
            "changes" => SnapRoute::Changes,
            _ => return Ok(None),
        };
        let state = super::super::git_empirical::load_state(&self.store_root)?.unwrap_or_default();
        let path_hashes = self
            .repo_root
            .as_ref()
            .map(|r| super::super::git_empirical::path_to_content_hash(r))
            .transpose()?
            .unwrap_or_default();
        let top = super::super::git_empirical::hot_top_with_hashes(
            &state.churn,
            &path_hashes,
            super::super::git_empirical::HOT_TOP_K,
        );
        let tier_c = super::super::git_empirical::tier_c_coverage_fraction(&state);
        let mut destinations = Vec::new();
        for (i, hp) in top.iter().take(5).enumerate() {
            if budget < 64 && i > 0 {
                break;
            }
            let evidence = if hp.content_sha256.is_empty() {
                format!("path/{}", hp.path)
            } else {
                format!("z://blob/{}", hp.content_sha256)
            };
            destinations.push(DestinationRef {
                destination_ref: format!("path/{}", hp.path),
                evidence_ref: evidence,
                label: format!("{}:{:.1}", hp.path, hp.churn_score),
                path: Some(hp.path.clone()),
                target: None,
                kind: None,
                symbol: None,
                content: None,
            });
        }
        let diagnostics = RouteDiagnostics::default();
        let coverage_capsule = Capsule {
            query: query.to_string(),
            snapshot_id: self.entry.snapshot_id,
            matches: Vec::new(),
            tier_a: 0.0,
            tier_b: 0.0,
            tier_c,
            budget,
            freshness: FreshnessDiagnostics {
                check_freshness,
                ..Default::default()
            },
        };
        let json_preview = render_query_capsule_json(
            query,
            budget,
            route,
            &destinations,
            &coverage_capsule,
            &diagnostics,
            false,
            0,
            None,
            self.semantic_tier_percent(),
        );
        let used = json_preview.len().div_ceil(4);
        Ok(Some(QueryCapsule {
            schema_version: 1,
            query: query.to_string(),
            budget,
            route,
            destinations,
            coverage: CoverageCertificate {
                tier_a: 0.0,
                tier_b: 0.0,
                tier_c,
                semantic_tier_percent: self.semantic_tier_percent(),
                freshness_verified: check_freshness && self.staleness_diagnostic().is_none(),
            },
            diagnostics,
            ledger: BudgetLedger {
                requested_budget: budget,
                used_budget: used.min(budget),
                remaining_budget: budget.saturating_sub(used.min(budget)),
                truncated: used > budget,
                omitted_count: 0,
            },
            snapshot_id: self.entry.snapshot_id,
        }))
    }

    pub fn shard_paths(&self) -> Vec<PathBuf> {
        let dir = self.store_root.join("shards");
        (0..self.entry.shard_hashes.len())
            .map(|i| dir.join(shard_file_name(self.entry.snapshot_id, i)))
            .collect()
    }

    pub fn coverage(&self) -> Result<CoverageBitmap> {
        let view = self.global.view()?;
        let cov = view.coverage()?;
        Ok(CoverageBitmap::from_packed(cov.blob_hashes.len(), cov.bits))
    }

    pub(crate) fn paths(&self) -> &HashMap<ContentHash, PathRecord> {
        &self.paths
    }

    /// Number of indexed path records (loc ids `1..=count` are paths).
    pub fn path_record_count(&self) -> usize {
        self.paths.len()
    }

    /// One-shot verdict that the locate fast-path invariant holds for this
    /// snapshot's published symbol table. Verified once, then cached.
    pub fn locate_fast_path_ok(&self) -> bool {
        *self.locate_fast_path.get_or_init(|| {
            self.global_view()
                .ok()
                .and_then(|view| SymbolTable::from_view(&view).ok())
                .map(|table| table.entries_dense_and_sorted())
                .unwrap_or(false)
        })
    }

    fn snapshot_cov_tier_counts(&self, cov_bits: &[u8], blob_count: usize) -> Result<[usize; 3]> {
        match self.snapshot_cov_counts.get_or_init(|| {
            CoverageBitmap::tier_counts_packed(cov_bits, blob_count).map_err(|err| err.to_string())
        }) {
            Ok(counts) => Ok(*counts),
            Err(err) => Err(anyhow!("coverage count failed: {err}")),
        }
    }

    pub fn path_for_blob(&self, hash_hex: &str) -> Option<&PathRecord> {
        let key = ContentHash::from_hex(hash_hex)?;
        self.paths().get(&key)
    }

    pub fn blob_bytes(&self, hash_hex: &str) -> Option<Vec<u8>> {
        // Snapshot-scoped memo: verified-once per (store, hash) per process. Integrity is
        // preserved — the first read still goes through `BlobStore::get_hex`, which rejects
        // digest mismatches; later reads of the same immutable object reuse those verified bytes.
        if let Some(cache) = self.blob_cache.get()
            && let Ok(guard) = cache.lock()
            && let Some(bytes) = guard.get(hash_hex)
        {
            return Some(bytes.as_ref().clone());
        }
        let bytes = crate::BlobStore::open(&self.store_root)
            .ok()?
            .get_hex(hash_hex)
            .ok()??;
        let cache = self
            .blob_cache
            .get_or_init(|| std::sync::Mutex::new(HashMap::new()));
        if let Ok(mut guard) = cache.lock() {
            if guard.len() >= 512 {
                guard.clear(); // simple cap: bounded RSS per snapshot
            }
            guard.insert(hash_hex.to_string(), std::sync::Arc::new(bytes.clone()));
        }
        Some(bytes)
    }

    /// Iterate path records. Keys are `ContentHash` (32 bytes); call `to_hex()`
    /// when a 64-hex string is required for refs or external APIs.
    pub fn path_records(&self) -> impl Iterator<Item = (ContentHash, &PathRecord)> {
        self.paths().iter().map(|(h, r)| (*h, r))
    }

    pub fn locate_index(&self) -> Result<&LocateIndex> {
        match self
            .locate_index
            .get_or_init(|| LocateIndex::build(self).map_err(|e| e.to_string()))
        {
            Ok(index) => Ok(index),
            Err(err) => Err(anyhow!("locate index build failed: {err}")),
        }
    }

    /// Warm edit-anchor index. The first call builds from snapshot data (parallel); resolves are
    /// I/O-free afterwards.
    pub fn snap_edit_index(&self) -> Result<&SnapEditIndex> {
        match self
            .snap_edit_index
            .get_or_init(|| SnapEditIndex::build(self).map_err(|e| e.to_string()))
        {
            Ok(index) => Ok(index),
            Err(err) => Err(anyhow!("snap edit index build failed: {err}")),
        }
    }

    /// Build a reverse-index view from the mmap'd CSR. The shard view is only
    /// held for the duration of `build` (no long-lived borrow).
    fn build_reverse_index(
        &self,
        build: impl FnOnce(&CsrAdjacency<'_>) -> ReverseIndex,
    ) -> Result<ReverseIndex, String> {
        let view = self.global.view().map_err(|error| error.to_string())?;
        let edges = view.edges().map_err(|error| error.to_string())?;
        let csr = CsrAdjacency::new(edges);
        Ok(build(&csr))
    }

    pub fn reverse_index(&self) -> Result<&ReverseIndex> {
        match self
            .reverse_all
            .get_or_init(|| self.build_reverse_index(|csr| ReverseIndex::build(csr, None)))
        {
            Ok(index) => Ok(index),
            Err(error) => Err(anyhow!("reverse index build failed: {error}")),
        }
    }

    pub fn blast_reverse_index(&self) -> Result<&ReverseIndex> {
        match self.reverse_blast.get_or_init(|| {
            self.build_reverse_index(|csr| {
                ReverseIndex::build_filtered(csr, edge_kind::is_blast_kind)
            })
        }) {
            Ok(index) => Ok(index),
            Err(error) => Err(anyhow!("blast reverse index build failed: {error}")),
        }
    }

    pub fn calls_reverse_index(&self) -> Result<&ReverseIndex> {
        match self.reverse_calls.get_or_init(|| {
            self.build_reverse_index(|csr| ReverseIndex::build(csr, Some(edge_kind::CALLS)))
        }) {
            Ok(index) => Ok(index),
            Err(error) => Err(anyhow!("calls reverse index build failed: {error}")),
        }
    }

    /// Perf precomp for blast tests: OnceLock filtered test paths (avoid full scan+read_blob every blast)
    pub fn precomp_test_paths(&self) -> &[String] {
        self.precomp_test_paths.get_or_init(|| {
            self.path_records()
                .filter_map(|(_h, r)| {
                    let p = r.path.as_str();
                    if p.contains("test") || p.ends_with("cli.rs") {
                        Some(p.to_string())
                    } else {
                        None
                    }
                })
                .collect()
        })
    }

    /// fff-style name/path bigram index (OnceLock). Prefers publish-time GZNB
    /// sidecar when present; otherwise builds in-process (legacy snapshots).
    /// Snapshot replace on daemon reopen clears the OnceLock.
    pub fn name_bigram_index(&self) -> Result<&NameBigramIndex> {
        match self.name_bigram.get_or_init(|| {
            let shards = self.store_root.join("shards");
            match NameBigramIndex::try_load_published(&shards, self.entry.snapshot_id) {
                Ok(Some(idx)) => Ok(idx),
                Ok(None) => NameBigramIndex::build(self).map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            }
        }) {
            Ok(index) => Ok(index),
            Err(err) => Err(anyhow!("name bigram index build failed: {err}")),
        }
    }

    /// Cached silent-risk scan over indexed paths (intent-independent).
    /// Each tuple is `(kind, evidence_ref, detail)`.
    pub fn precomp_silent_risks(&self) -> Result<&[(String, String, String)]> {
        match self
            .precomp_silent_risks
            .get_or_init(|| scan_silent_risks(self))
        {
            Ok(risks) => Ok(risks.as_slice()),
            Err(err) => Err(anyhow!("silent risk precomp failed: {err}")),
        }
    }

    pub fn query(&self, symbol: &str, budget: usize, check_freshness: bool) -> Result<Capsule> {
        self.query_with_repair(symbol, budget, check_freshness, false)
    }

    fn capsule_matches_for_symbol_ids(
        &self,
        parts: &super::legacy::QueryRepairParts<'_>,
        ids: Vec<u32>,
        check_freshness: bool,
    ) -> Vec<super::types::CapsuleMatch> {
        ids.into_iter()
            .map(|id| {
                capsule_match_for_symbol(
                    self,
                    &parts.table,
                    parts.spans.as_ref(),
                    &parts.csr,
                    parts.evidence.as_ref(),
                    parts.blob_hashes,
                    id,
                    check_freshness,
                )
            })
            .collect()
    }

    pub fn query_with_repair(
        &self,
        symbol: &str,
        budget: usize,
        check_freshness: bool,
        repair_stale: bool,
    ) -> Result<Capsule> {
        let view = self.global.view()?;
        let parts = query_repair_parts(&view)?;
        let mut matches = self.capsule_matches_for_symbol_ids(
            &parts,
            symbol_candidate_ids(&parts.table, symbol),
            check_freshness,
        );
        if !self.pending.defs.is_empty() || !self.pending.edges.is_empty() {
            merge_pending_defs_edges(self, symbol, &mut matches);
        }

        let snapshot_counts =
            self.snapshot_cov_tier_counts(parts.cov_bits, parts.blob_hashes.len())?;
        let (tier_a, tier_b, tier_c) = coverage_ratios(
            snapshot_counts,
            pending_tier_a(&self.pending),
            parts.blob_hashes.len() + self.pending.blobs.len(),
        );
        let mut freshness = FreshnessDiagnostics {
            check_freshness,
            ..Default::default()
        };
        if check_freshness {
            if let Some(reason) = self.staleness_diagnostic() {
                freshness.events.push(reason);
            }
            if repair_stale {
                self.merge_repaired_symbols(symbol, &mut matches, &mut freshness)?;
            }
        }

        Ok(Capsule {
            query: symbol.to_string(),
            snapshot_id: self.entry.snapshot_id,
            matches,
            tier_a,
            tier_b,
            tier_c,
            budget,
            freshness,
        })
    }

    fn merge_repaired_symbols(
        &self,
        symbol: &str,
        matches: &mut Vec<super::types::CapsuleMatch>,
        diag: &mut FreshnessDiagnostics,
    ) -> Result<()> {
        let Some(repo) = self.repo_root.as_ref() else {
            diag.events.push("path_resolution_failed".into());
            return Ok(());
        };

        let mut checked_paths: BTreeSet<String> = BTreeSet::new();
        let mut stale_paths: BTreeSet<String> = BTreeSet::new();

        collect_stale_from_indexed_defs(
            self,
            repo,
            matches,
            &mut checked_paths,
            &mut stale_paths,
            diag,
        );
        collect_stale_when_symbol_missing(
            self,
            repo,
            symbol,
            matches,
            &mut checked_paths,
            &mut stale_paths,
            diag,
        );

        for rel in stale_paths {
            match self.refresh_file(&rel) {
                Ok(defs) => {
                    diag.reextract_count += 1;
                    diag.events.push(format!("reextract_complete:{rel}"));
                    merge_repaired_def_batch(symbol, &rel, defs, matches);
                }
                Err(e) => {
                    diag.events.push(format!("repair_failed:{}:{e}", rel));
                }
            }
        }

        Ok(())
    }

    pub fn refresh_file(&self, rel_path: &str) -> Result<Vec<(String, String, u32, u32)>> {
        let Some(repo) = self.repo_root.as_ref() else {
            return Ok(Vec::new());
        };
        let content = fs::read(repo.join(rel_path))?;
        let hash = ContentHash::of(&content);
        Ok(extract_defs(&hash, &content)
            .into_iter()
            .map(|d| (d.name, hash.to_hex(), d.start, d.end))
            .collect())
    }

    pub fn unindexed_blob_count(&self, tier: Tier) -> usize {
        let cov = self.coverage().ok();
        let total = cov.as_ref().map(|c| c.blob_count()).unwrap_or(0) + self.pending.blobs.len();
        if total == 0 {
            return 0;
        }
        let mut indexed = cov.as_ref().map(|c| c.tier_count(tier)).unwrap_or(0);
        for bits in self.pending.blobs.values() {
            let tier_bit = match tier {
                Tier::A => 0b001,
                Tier::B => 0b010,
                Tier::C => 0b100,
            };
            if *bits & tier_bit != 0 {
                indexed += 1;
            }
        }
        total.saturating_sub(indexed)
    }

    pub fn freshness_verified(&self) -> bool {
        self.repo_root.is_some() && self.staleness_diagnostic().is_none()
    }

    pub fn staleness_diagnostic(&self) -> Option<String> {
        if let Some(missing) = self.shard_paths().into_iter().find(|path| !path.is_file()) {
            return Some(format!("missing_snapshot_shard:{}", missing.display()));
        }
        let repo = self.repo_root.as_ref()?;
        snapshot_staleness_diagnostic(repo, self.paths())
    }

    pub fn first_unindexed_source_path(
        repo: &Path,
        indexed: &HashMap<ContentHash, PathRecord>,
    ) -> Option<String> {
        let indexed_paths: BTreeSet<String> = indexed.values().map(|r| r.path.clone()).collect();
        let src = repo.join("src");
        if !src.is_dir() {
            return None;
        }
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            let Ok(read) = fs::read_dir(&dir) else {
                continue;
            };
            for ent in read.flatten() {
                let path = ent.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let rel = path
                    .strip_prefix(repo)
                    .ok()
                    .and_then(|p| p.to_str())
                    .map(|s| s.replace('\\', "/"));
                let Some(rel) = rel else {
                    continue;
                };
                if rel.ends_with(".rs") && !indexed_paths.contains(&rel) {
                    return Some(rel);
                }
            }
        }
        None
    }
}

impl Capsule {
    pub fn to_json(&self, store_root: Option<&Path>) -> String {
        super::capsule_json::capsule_to_json(self, store_root)
    }

    pub fn render(&self) -> String {
        render_budgeted_capsule(self)
    }
}

fn silent_risk_blob_ref(hash_hex: &str) -> String {
    format!("z://blob/{hash_hex}#B0-0")
}

fn cmp_silent_risk(
    a: &(String, String, String),
    b: &(String, String, String),
) -> std::cmp::Ordering {
    a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1))
}

fn maybe_push_silent_risk(
    risks: &mut Vec<(String, String, String)>,
    seen: &mut BTreeSet<String>,
    dedupe_key: String,
    kind: &str,
    hash_hex: &str,
    detail: String,
) {
    if !seen.insert(dedupe_key) {
        return;
    }
    risks.push((kind.into(), silent_risk_blob_ref(hash_hex), detail));
}

fn scan_text_silent_risks(
    risks: &mut Vec<(String, String, String)>,
    seen: &mut BTreeSet<String>,
    hash_hex: &str,
    path: &str,
    text: &str,
) {
    if (text.contains(".get(\"") || text.contains(".get('")) && text.contains("HashMap") {
        maybe_push_silent_risk(
            risks,
            seen,
            format!("string_key:{hash_hex}"),
            "string_key",
            hash_hex,
            format!("string-keyed lookup in {path}"),
        );
    }
    if text.contains("dyn ") || text.contains("Box<dyn") {
        maybe_push_silent_risk(
            risks,
            seen,
            format!("dynamic:{hash_hex}"),
            "dynamic_dispatch",
            hash_hex,
            format!("dynamic dispatch in {path}"),
        );
    }
}

fn scan_path_silent_risks(
    risks: &mut Vec<(String, String, String)>,
    seen: &mut BTreeSet<String>,
    hash_hex: &str,
    path: &str,
) {
    if path.ends_with(".toml") || path.ends_with(".json") {
        maybe_push_silent_risk(
            risks,
            seen,
            format!("cross:{hash_hex}"),
            "cross_artifact",
            hash_hex,
            format!("cross-artifact config {path}"),
        );
    }
}

const SILENT_RISK_CACHE_SCHEMA: u32 = 1;
const SILENT_RISK_ALGORITHM: &str = "silent-risk";

#[derive(Debug, Deserialize, Serialize)]
struct SilentRiskCache {
    schema_version: u32,
    source_digest: String,
    payload_digest: String,
    risks: Vec<SilentRiskCacheEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct SilentRiskCacheEntry {
    kind: String,
    evidence_ref: String,
    detail: String,
}

fn digest_fields<'a>(fields: impl IntoIterator<Item = &'a str>) -> String {
    let mut bytes = Vec::new();
    for field in fields {
        bytes.extend_from_slice(&(field.len() as u64).to_le_bytes());
        bytes.extend_from_slice(field.as_bytes());
    }
    ContentHash::of(&bytes).to_hex()
}

fn silent_risk_source_digest(snapshot: &Snapshot) -> String {
    let mut records: Vec<(&str, String)> = snapshot
        .path_records()
        .map(|(hash, record)| (record.path.as_str(), hash.to_hex()))
        .collect();
    records.sort_unstable();
    digest_fields(
        std::iter::once(SILENT_RISK_ALGORITHM).chain(
            records
                .iter()
                .flat_map(|(path, hash)| [*path, hash.as_str()]),
        ),
    )
}

fn silent_risk_payload_digest(risks: &[(String, String, String)]) -> String {
    digest_fields(
        std::iter::once(SILENT_RISK_ALGORITHM).chain(risks.iter().flat_map(
            |(kind, evidence_ref, detail)| [kind.as_str(), evidence_ref.as_str(), detail.as_str()],
        )),
    )
}

fn silent_risk_cache_path(snapshot: &Snapshot) -> PathBuf {
    snapshot
        .store_root
        .join("query-cache")
        .join("silent-risks.json")
}

fn load_silent_risk_cache(
    snapshot: &Snapshot,
    source_digest: &str,
) -> Option<Vec<(String, String, String)>> {
    let bytes = fs::read(silent_risk_cache_path(snapshot)).ok()?;
    let cache: SilentRiskCache = serde_json::from_slice(&bytes).ok()?;
    if cache.schema_version != SILENT_RISK_CACHE_SCHEMA || cache.source_digest != source_digest {
        return None;
    }
    let mut risks: Vec<_> = cache
        .risks
        .into_iter()
        .map(|risk| (risk.kind, risk.evidence_ref, risk.detail))
        .collect();
    risks.sort_by(cmp_silent_risk);
    if silent_risk_payload_digest(&risks) != cache.payload_digest {
        return None;
    }
    Some(risks)
}

fn store_silent_risk_cache(
    snapshot: &Snapshot,
    source_digest: String,
    risks: &[(String, String, String)],
) -> Result<(), String> {
    let cache = SilentRiskCache {
        schema_version: SILENT_RISK_CACHE_SCHEMA,
        source_digest,
        payload_digest: silent_risk_payload_digest(risks),
        risks: risks
            .iter()
            .map(|(kind, evidence_ref, detail)| SilentRiskCacheEntry {
                kind: kind.clone(),
                evidence_ref: evidence_ref.clone(),
                detail: detail.clone(),
            })
            .collect(),
    };
    let bytes = serde_json::to_vec(&cache).map_err(|error| error.to_string())?;
    super::super::atomic_write_file(&silent_risk_cache_path(snapshot), &bytes)
        .map_err(|error| error.to_string())
}

fn scan_silent_risks(snapshot: &Snapshot) -> Result<Vec<(String, String, String)>, String> {
    use crate::store::blob_store::BlobStore;

    let source_digest = silent_risk_source_digest(snapshot);
    if let Some(risks) = load_silent_risk_cache(snapshot, &source_digest) {
        return Ok(risks);
    }

    let store = BlobStore::open(&snapshot.store_root).map_err(|e| e.to_string())?;
    let mut risks: Vec<(String, String, String)> = Vec::new();
    let mut seen = BTreeSet::new();
    for (hash, rec) in snapshot.path_records() {
        let path = rec.path.as_str();
        let hash_hex = hash.to_hex();
        let text = store
            .get_hex(&hash_hex)
            .ok()
            .flatten()
            .and_then(|bytes| String::from_utf8(bytes).ok());
        if let Some(text) = text.as_deref() {
            scan_text_silent_risks(&mut risks, &mut seen, &hash_hex, path, text);
        }
        scan_path_silent_risks(&mut risks, &mut seen, &hash_hex, path);
    }
    risks.sort_by(cmp_silent_risk);
    let _ = store_silent_risk_cache(snapshot, source_digest, &risks);
    Ok(risks)
}
