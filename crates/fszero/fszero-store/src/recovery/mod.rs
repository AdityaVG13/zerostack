use fsqlite::{Connection, ConnectionEnv, Row, SqliteValue};
use sha2::{Digest, Sha256};

use fszero_core::zeroref::{
    EMITTED_SCHEME, LineEndPolicy, ZeroRef, ZeroRefError, ZeroRefErrorClass,
};
use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod chunk_index;
mod durable_integrity;
pub use durable_integrity::{
    SnapshotGcEntry, StoreGcPlan, snapshot_retention_budget, store_gc_apply, store_gc_plan,
};
mod edit_intent;
mod mutation_log;
mod pack;
mod payload;
mod ref_index;
mod sql_explain;
mod sql_profile;
mod worlds;
use pack::*;
use ref_index::*;
pub use sql_explain::{
    HotSqlEntry, SqlExplainCapture, capture_hot_sql_explains, hot_sql_catalog,
    maybe_capture_sql_explains, sql_explain_env_enabled, sql_explain_status_json,
    write_sql_explain_artifacts,
};
pub use sql_profile::{
    SqlProfileRow, reset_sql_profile, sql_profile_env_enabled, sql_profile_json, sql_profile_top,
};

/// Prepared-statement cache counters from fsqlite-core hot-path profile.
/// These are process-global atomics, not per-RecoveryStore payload cache metrics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PreparedCacheMetrics {
    pub hits: u64,
    pub misses: u64,
}

impl PreparedCacheMetrics {
    pub fn total(self) -> u64 {
        self.hits.saturating_add(self.misses)
    }
    pub fn hit_rate(self) -> Option<f64> {
        let t = self.total();
        if t == 0 {
            None
        } else {
            Some(self.hits as f64 / t as f64)
        }
    }
}

/// Env gate: `FSZERO_FSQLITE_PREPARED_CACHE_PROFILE=1` enables fsqlite hot-path
/// counters so prepared_cache hits/misses accumulate for sampling.
pub fn prepared_cache_profile_env_enabled() -> bool {
    match std::env::var("FSZERO_FSQLITE_PREPARED_CACHE_PROFILE") {
        Ok(v) => {
            let t = v.trim();
            t == "1"
                || t.eq_ignore_ascii_case("true")
                || t.eq_ignore_ascii_case("yes")
                || t.eq_ignore_ascii_case("on")
        }
        Err(_) => false,
    }
}

/// Enable fsqlite hot-path profiling when the env gate is set (idempotent).
pub fn ensure_prepared_cache_profile() {
    if prepared_cache_profile_env_enabled() {
        fsqlite_core::connection::set_hot_path_profile_enabled(true);
    }
}

/// Snapshot prepared-statement cache hit/miss counters from fsqlite-core.
pub fn prepared_cache_metrics() -> PreparedCacheMetrics {
    ensure_prepared_cache_profile();
    let snap = fsqlite_core::connection::hot_path_profile_snapshot();
    PreparedCacheMetrics {
        hits: snap.parser.prepared_cache_hits,
        misses: snap.parser.prepared_cache_misses,
    }
}

/// JSON object for telemetry / doctor when sampling is on (empty object if off).
pub fn prepared_cache_metrics_json() -> serde_json::Value {
    if !prepared_cache_profile_env_enabled() {
        return serde_json::json!({"enabled": false});
    }
    let m = prepared_cache_metrics();
    serde_json::json!({
        "enabled": true,
        "prepared_cache_hits": m.hits,
        "prepared_cache_misses": m.misses,
        "hit_rate": m.hit_rate(),
        "source": "fsqlite_core::hot_path_profile_snapshot.parser",
        "note": "process-global; distinct from RecoveryStore payload cache_hits",
    })
}

/// Why a cache entry was not reused for Q99 accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMissCause {
    /// A minimum dependency content root no longer resolves in CAS.
    DependencyRootChanged,
    /// Completeness witness / toolchain root failed verification.
    WitnessUnverifiable,
    /// Full query-cache wipe (no cone-scoped eviction).
    CoarseWipe,
}

/// Snapshot of hits plus split miss causes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CacheMissCauseSnapshot {
    pub hits: u64,
    pub misses: u64,
    pub dependency_root_changed: u64,
    pub witness_unverifiable: u64,
    pub coarse_wipe: u64,
}

impl CacheMissCauseSnapshot {
    pub fn to_json(self) -> serde_json::Value {
        serde_json::json!({
            "hits": self.hits,
            "misses": self.misses,
            "miss_causes": {
                "dependency_root_changed": self.dependency_root_changed,
                "witness_unverifiable": self.witness_unverifiable,
                "coarse_wipe": self.coarse_wipe,
            },
            "note": "cause counts are attributed misses; cold key-absent misses may only appear in misses",
        })
    }
}

/// Env-gated durable store open phase JSON. Set `FSZERO_STORE_OPEN_PHASES=1` to emit
/// one stderr line with `integrity_gate_us`, `sqlite_open_us`, `pack_us`, `ast_us`,
/// `maintenance_us`, and `total_us` (plus mode / path). Off by default -- no product cost.
fn store_open_phases_enabled() -> bool {
    match std::env::var("FSZERO_STORE_OPEN_PHASES") {
        Ok(v) => {
            let t = v.trim();
            t == "1"
                || t.eq_ignore_ascii_case("true")
                || t.eq_ignore_ascii_case("yes")
                || t.eq_ignore_ascii_case("on")
        }
        Err(_) => false,
    }
}

fn emit_store_open_phases(fields: serde_json::Value) {
    if !store_open_phases_enabled() {
        return;
    }
    eprintln!("{fields}");
}

// Public API (return type of query_mutations); re-export even if unused by name in-crate.
pub use chunk_index::{ChunkIndexReport, ChunkInvalidation, StoredChunk};
#[allow(unused_imports)]
pub use mutation_log::MutationRow;

pub const MAX_TRANSIENT_PAYLOADS: usize = 256;

/// How long a durable open retries while another process holds the store. Contention is normal when
/// two engines share a root; only a store that stays busy past this wall is treated as a real
/// failure.
const DEFAULT_DURABLE_BUSY_WALL_MS: u64 = 5_000;

fn durable_busy_wall() -> std::time::Duration {
    let ms = [
        "FSZERO_DURABLE_BUSY_WALL_MS",
        "ZEROSTACK_DURABLE_BUSY_WALL_MS",
    ]
    .iter()
    .find_map(|key| std::env::var(key).ok()?.trim().parse::<u64>().ok())
    .unwrap_or(DEFAULT_DURABLE_BUSY_WALL_MS);
    std::time::Duration::from_millis(ms.max(1))
}

/// How long a SINGLE gate attempt waits on the writer lock. This must stay well under the total
/// wall.
pub(super) fn durable_busy_attempt_wait() -> std::time::Duration {
    (durable_busy_wall() / 8).clamp(
        std::time::Duration::from_millis(25),
        std::time::Duration::from_millis(250),
    )
}
/// Return true when input claims a product ZeroRef scheme.
fn claims_zeroref(r: &str) -> bool {
    r.starts_with("z://")
        || r.starts_with("fz://")
        || r.starts_with("gz://")
        || r.starts_with("tz://")
}

/// Named payload keys receive the highest recovery priority.
const NAMED_PAYLOAD_KEYS: [&str; 6] = [
    "read",
    "stat",
    "budget_evidence",
    "ls_manifest",
    "search",
    "last_cert",
];

const SQL_SELECT_PAYLOAD_KEYS: &str = "SELECT key FROM payloads";
const SQL_SELECT_PAYLOAD_KV: &str = "SELECT key, value FROM payloads";
const SQL_SELECT_PAYLOAD_EXISTS: &str = "SELECT 1 FROM payloads WHERE key = ?1 LIMIT 1";
const MAX_BATCH_PAYLOAD_KEY_CACHE: usize = 65_536;
/// Per connection: 16,384 default-size (4 KiB) pages = 64 MiB. FrankenSQLite otherwise
/// defaults to 262,144 pages (about 1 GiB) and retains that pool after heavy phases. Two
/// live recovery connections must remain inside the process-wide 256 MiB steady-idle gate.
const FSQLITE_PAGE_BUFFER_MAX: usize = 16_384;

fn recovery_connection_env() -> ConnectionEnv {
    let mut env = ConnectionEnv::default();
    env.set_page_buffer_max(FSQLITE_PAGE_BUFFER_MAX);
    env
}

const SQL_INSERT_PAYLOAD_KV: &str = "INSERT OR REPLACE INTO payloads (key, value) VALUES (?1, ?2)";
const SQL_DELETE_PAYLOAD_KEY: &str = "DELETE FROM payloads WHERE key = ?1";
const SQL_PRAGMA_WAL_CHECKPOINT_TRUNCATE: &str = "PRAGMA wal_checkpoint(TRUNCATE)";

/// Env gate: `FSZERO_WAL_CHECKPOINT_PROFILE=1` records + emits wal_checkpoint
/// TRUNCATE wall_us and page counts after end_batch / maintain_wal_cadence.
fn wal_checkpoint_profile_enabled() -> bool {
    match std::env::var("FSZERO_WAL_CHECKPOINT_PROFILE") {
        Ok(v) => matches!(v.as_str(), "1" | "true" | "TRUE" | "on" | "yes"),
        Err(_) => false,
    }
}

const SQL_INSERT_MEMORY_PATHS_IGNORE: &str = "INSERT OR IGNORE INTO memory_paths (path, store_key, content_ref, updated_ts) VALUES (?1, ?2, ?3, ?4)";
const SQL_INSERT_MEMORY_PATHS_REPLACE: &str = "INSERT OR REPLACE INTO memory_paths (path, store_key, content_ref, updated_ts) VALUES (?1, ?2, ?3, ?4)";
const SQL_DELETE_MEMORY_PATHS_BY_STORE_KEY: &str = "DELETE FROM memory_paths WHERE store_key = ?1";
const SQL_DELETE_MEMORY_PATHS_BY_PATH: &str = "DELETE FROM memory_paths WHERE path = ?1";
const SQL_SELECT_MEMORY_PATH_EXISTS: &str = "SELECT 1 FROM memory_paths WHERE path = ?1 LIMIT 1";
const SQL_SELECT_MEMORY_PATHS_ORDERED: &str = "SELECT path FROM memory_paths ORDER BY path ASC";
const SQL_SELECT_PAYLOAD_VALUE_BY_KEY: &str = "SELECT value FROM payloads WHERE key = ?1";
const SQL_INSERT_PAYLOAD_LRU: &str =
    "INSERT OR REPLACE INTO payload_lru (key, tick) VALUES (?1, ?2)";
const SQL_DELETE_PAYLOAD_LRU: &str = "DELETE FROM payload_lru WHERE key = ?1";
const SQL_INSERT_META_KV: &str = "INSERT OR REPLACE INTO meta (k, v) VALUES (?1, ?2)";
const SQL_SELECT_INTEGRITY_STATE: &str =
    "SELECT violations, detail FROM integrity_state WHERE id = 1";
const SQL_INSERT_INTEGRITY_STATE: &str =
    "INSERT OR REPLACE INTO integrity_state (id, violations, detail) VALUES (1, ?1, ?2)";
const SQL_INSERT_FACT: &str = "INSERT OR REPLACE INTO facts (subject_ref, predicate, object_ref, evidence_ref, version, agent) VALUES (?1, ?2, ?3, ?4, ?5, ?6)";
const SQL_SELECT_FACTS_BY_SUBJECT: &str = "SELECT predicate, object_ref, evidence_ref, version, agent FROM facts WHERE subject_ref = ?1 ORDER BY version, predicate";
const SQL_SELECT_MEMORY_PATHS_PREFIX: &str =
    "SELECT path FROM memory_paths WHERE path = ?1 OR path LIKE ?2 ESCAPE '\\' ORDER BY path ASC";
const SQL_COUNT_TRANSIENT_PAYLOADS: &str = "SELECT COUNT(*) FROM payloads WHERE key LIKE 'seq/%'";
const SQL_DELETE_TRANSIENT_OVERFLOW: &str = "DELETE FROM payloads WHERE key IN (SELECT p.key FROM payloads p LEFT JOIN payload_lru l ON p.key = l.key WHERE p.key LIKE 'seq/%' ORDER BY COALESCE(l.tick, 0), p.key LIMIT ?1)";
const SQL_DELETE_ORPHAN_TRANSIENT_LRU: &str =
    "DELETE FROM payload_lru WHERE key LIKE 'seq/%' AND key NOT IN (SELECT key FROM payloads)";
const SQL_UPDATE_PAYLOAD_VALUE: &str = "UPDATE payloads SET value = ?2 WHERE key = ?1";
const SQL_SELECT_PAYLOAD_VALUES: &str = "SELECT value FROM payloads";
const SQL_SELECT_META_V: &str = "SELECT v FROM meta WHERE k = ?1";
const SQL_SELECT_LIVE_REFS: &str = "SELECT key FROM payloads UNION SELECT pre_ref FROM mutation_log UNION SELECT post_ref FROM mutation_log UNION SELECT pre_ref FROM edit_intents WHERE pre_ref != '' UNION SELECT post_ref FROM edit_intents WHERE post_ref != '' UNION SELECT subject_ref FROM facts UNION SELECT object_ref FROM facts UNION SELECT evidence_ref FROM facts UNION SELECT cert_ref FROM worlds UNION SELECT cert_ref FROM world_edits UNION SELECT content_ref FROM memory_paths";

/// Shared ZeroRef unrecoverable error (CAS corrupt/io + integrity fallthrough).
#[inline]
fn zeroref_unrecoverable(class: ZeroRefErrorClass, msg: impl Into<String>) -> ZeroRefError {
    ZeroRefError::new(class, msg)
}

/// Nanoseconds since UNIX epoch (0 if clock is unavailable).
pub fn unix_epoch_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Milliseconds since UNIX epoch (0 if clock is unavailable).
pub fn unix_epoch_millis() -> u128 {
    unix_epoch_nanos() / 1_000_000
}

/// Seconds since UNIX epoch (0 if clock is unavailable).
pub fn unix_epoch_secs() -> i64 {
    (unix_epoch_nanos() / 1_000_000_000) as i64
}

/// Shared corrective error for execution-scoped seq refs (expand path).
#[inline]
pub fn seq_ref_scoped_err(r: &str) -> String {
    format!(
        "seq_ref_scoped: {r} (seq/ keys are execution-scoped; expand a z://blob/ ref from the result instead)"
    )
}

/// Shared not-found after all expand tiers (session + recovery).
#[inline]
pub fn ref_not_found_err(r: &str) -> String {
    format!("ref_not_found: {r} (tiers tried: explicit/env-cache, current-root-store, ref-index)")
}

/// Durable open variants sharing conn/pack/ast field init.
#[derive(Debug, Clone, Copy)]
enum DurableOpenMode {
    CreateOrOpen,
    ExistingWithMaintenance,
    ExistingLight,
}

/// Typed durable-open failure retained through retry classification.
#[derive(Debug)]
enum DurableOpenError {
    Gate(durable_integrity::GateError),
    Other(String),
}

impl std::fmt::Display for DurableOpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gate(error) => write!(f, "store open failed: {error}"),
            Self::Other(message) => write!(f, "{message}"),
        }
    }
}

pub struct RecoveryStore {
    pub conn: Connection,
    pub(super) next_id: u32,
    pub(super) last_store_error: Option<String>,
    store_writes: Cell<u64>,
    /// Set when the current explicit transaction writes durable state.
    exec_txn_durable_dirty: Cell<bool>,
    cache_hits: Cell<u64>,
    cache_misses: Cell<u64>,
    bytes_materialized: Cell<u64>,
    /// Misses attributed to a dependency-root CAS gap.
    cache_miss_dependency_root: Cell<u64>,
    /// Misses attributed to witness/toolchain unverifiable.
    cache_miss_witness: Cell<u64>,
    /// Misses from full search/compound wipe (coarse invalidation).
    cache_miss_coarse_wipe: Cell<u64>,
    /// Batch-mode write buffer: payloads staged here and flushed to sqlite in
    /// sorted key order (see try_put_key). `None` outside begin/end_batch.
    pending_payloads: Option<BTreeMap<String, Arc<[u8]>>>,
    pending_bytes: usize,
    pub exec_txn_active: Cell<bool>,
    /// In-memory key cache populated during begin_batch to avoid a SQL
    /// page-read per has_payload check during bulk indexing.
    payload_key_cache: Option<HashSet<String>>,
    /// Blob sidecar for durable stores; `None` for an in-memory store.
    pack: Option<PackFile>,
    /// Pack generation this handle opened. Writers re-check it while holding
    /// SQLite's write transaction before every packed append.
    pack_generation: i64,
    /// SQLite store path for durable stores. Used only for cross-process ref-index recovery.
    db_path: Option<PathBuf>,
    /// Instance-local opt-out for isolated conformance fixtures. Product stores
    /// still obey the normal FSZERO_REF_INDEX process configuration.
    ref_index_disabled: bool,
    /// Set when a durable pack locator cannot be read (torn/short pack).
    /// Surfaced by `expand_with_tiers` as `pack_torn:` instead of a silent miss.
    pack_integrity_error: Cell<Option<String>>,
    /// Last immediate put's pack barrier timing for phase metrics.
    last_pack_sync_us: Option<u64>,
    last_pack_dirty: bool,
    last_put_bytes: usize,
    /// Last `wal_checkpoint(TRUNCATE)` duration and page counts.
    last_wal_checkpoint_us: Option<u64>,
    last_wal_checkpoint_log: Option<i64>,
    last_wal_checkpoint_checkpointed: Option<i64>,
    /// Full-hash blob rows removed by open-time torn-pack repair. Retained for
    /// this handle so an immediate CAS migration records them as `missing`.
    repaired_torn_pack_blobs: Vec<String>,
    /// Rows inspected by this handle's bounded open maintenance (test evidence).
    open_pack_rows_scanned: usize,
    open_memory_rows_scanned: usize,
    /// Rebuildable AST rows use SQLite directly because intermediary inserts dominated cold indexing.
    /// Callers use this field for AST operations without `RecoveryStore` wrappers.
    pub ast: super::ast_store::AstStore,
    /// Canonical shared CAS tier: consulted FIRST for blob
    /// reads when attached; mint dual-writes into it. None = feature off
    /// (the blobs/ dir under the store root is the explicit opt-in).
    cas: Option<super::cas::CasStore>,
    /// Loud-failure channel: count + last detail of every integrity violation
    /// seen on the read path — blob hash mismatches, torn pack locators,
    /// unparseable ref-index lines. Read via integrity_report; never silently cleared.
    integrity_violations: Cell<u64>,
    last_integrity_error: std::cell::RefCell<Option<String>>,
    /// Buffered access_log rows (same semantics as per-op INSERT; flushed on
    /// query, watermark, or drop). Cuts CheapRead autocommit tax without
    /// changing hot/recent/coaccess once flushed.
    pub pending_access: std::cell::RefCell<Vec<(i64, String, String, String, i64)>>,
}

impl Default for RecoveryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl RecoveryStore {
    /// Initialize fields shared by in-memory and durable stores.
    fn from_parts(
        conn: Connection,
        next_id: u32,
        pack: Option<PackFile>,
        pack_generation: i64,
        db_path: Option<PathBuf>,
        ast: super::ast_store::AstStore,
    ) -> Self {
        Self {
            conn,
            next_id,
            last_store_error: None,
            store_writes: Cell::new(0),
            exec_txn_durable_dirty: Cell::new(true),
            cache_hits: Cell::new(0),
            cache_misses: Cell::new(0),
            cache_miss_dependency_root: Cell::new(0),
            cache_miss_witness: Cell::new(0),
            cache_miss_coarse_wipe: Cell::new(0),
            bytes_materialized: Cell::new(0),
            pending_payloads: None,
            pending_bytes: 0,
            exec_txn_active: Cell::new(false),
            payload_key_cache: None,
            pack,
            pack_generation,
            db_path,
            ref_index_disabled: false,
            pack_integrity_error: Cell::new(None),
            last_pack_sync_us: None,
            last_pack_dirty: false,
            last_put_bytes: 0,
            last_wal_checkpoint_us: None,
            last_wal_checkpoint_log: None,
            last_wal_checkpoint_checkpointed: None,
            repaired_torn_pack_blobs: Vec::new(),
            open_pack_rows_scanned: 0,
            open_memory_rows_scanned: 0,
            ast,
            cas: None,
            integrity_violations: Cell::new(0),
            last_integrity_error: std::cell::RefCell::new(None),
            pending_access: std::cell::RefCell::new(Vec::new()),
        }
    }

    pub fn new() -> Self {
        let conn = Connection::open_with_env(":memory:", recovery_connection_env())
            .expect("fsqlite :memory:");
        sql_profile::maybe_install_sql_profile(&conn);
        init_tables(&conn);
        let next_id = load_next_id(&conn).unwrap_or(1);
        Self::from_parts(
            conn,
            next_id,
            None,
            0,
            None,
            super::ast_store::AstStore::memory(),
        )
    }

    pub fn with_durable(db_path: impl AsRef<Path>) -> Self {
        Self::try_with_durable(db_path).unwrap_or_else(|e| panic!("{e}"))
    }

    pub fn try_with_durable(db_path: impl AsRef<Path>) -> Result<Self, String> {
        Self::open_durable_store_retrying(db_path.as_ref(), DurableOpenMode::CreateOrOpen)
    }

    fn try_open_existing_durable(db_path: &Path) -> Result<Self, String> {
        Self::try_open_existing_durable_with_options(db_path, true)
    }

    /// True when a durable-open error is SQLite busy/locked contention rather than a fatal condition.
    /// Busy is transient by construction — another process holds the store right now — so the caller
    /// must retry instead of failing closed.
    fn is_transient_busy(error: &DurableOpenError) -> bool {
        match error {
            DurableOpenError::Gate(gate) => gate.is_busy(),
            DurableOpenError::Other(message) => {
                let lower = message.to_ascii_lowercase();
                lower.contains("database is busy") || lower.contains("database is locked")
            }
        }
    }

    /// Open a durable store, retrying while another process holds it. Without this, two engines
    /// on one root race on `store.sqlite3` and the loser takes a hard error on a condition
    /// SQLite defines as retryable. The budget is bounded so a genuinely wedged store still surfaces.
    fn open_durable_store_retrying(db_path: &Path, mode: DurableOpenMode) -> Result<Self, String> {
        // The loop owns the TOTAL budget; each gate attempt gets only a slice
        // of it (see `durable_busy_attempt_wait`).
        let deadline = std::time::Instant::now() + durable_busy_wall();
        let mut backoff = std::time::Duration::from_millis(5);
        loop {
            let error = match Self::open_durable_store(db_path, mode) {
                Ok(store) => return Ok(store),
                Err(error) => error,
            };
            if !Self::is_transient_busy(&error) || std::time::Instant::now() >= deadline {
                return Err(error.to_string());
            }
            let sleep_for = backoff.min(std::time::Duration::from_millis(100));
            let t0 = std::time::Instant::now();
            std::thread::sleep(sleep_for);
            crate::runtime_metrics::record_durable_open_busy_wait(t0.elapsed().as_micros() as u64);
            backoff = backoff.saturating_mul(2);
        }
    }

    /// Open an existing durable store. When `run_open_maintenance` is false, skip
    /// pack repair + memory backfill (full-table scans). Expand via ref-index must
    /// use the light path — otherwise each miss reloads a multi-MB store and pegs CPU.
    fn try_open_existing_durable_with_options(
        db_path: &Path,
        run_open_maintenance: bool,
    ) -> Result<Self, String> {
        let mode = if run_open_maintenance {
            DurableOpenMode::ExistingWithMaintenance
        } else {
            DurableOpenMode::ExistingLight
        };
        Self::open_durable_store_retrying(db_path, mode)
    }

    /// Shared durable open (create-or-open / existing+maintain / light expand).
    fn open_durable_store(db_path: &Path, mode: DurableOpenMode) -> Result<Self, DurableOpenError> {
        // Env-gated timers only; when off, no Instant work on the open path.
        let phases_on = store_open_phases_enabled();
        let t0 = phases_on.then(std::time::Instant::now);
        let mut mark = t0;
        let mut phase_us = [0u64; 5]; // gate, sqlite, pack, ast, maintenance
        let mut phase_i = 0usize;
        let mut tick = || {
            if let (Some(t0v), Some(m)) = (t0.as_ref(), mark.as_mut()) {
                let now = std::time::Instant::now();
                let us = now.duration_since(*m).as_micros().min(u128::from(u64::MAX)) as u64;
                *m = now;
                if phase_i < phase_us.len() {
                    phase_us[phase_i] = us;
                    phase_i += 1;
                }
                let _ = t0v;
            }
        };

        let mut exists = db_path.is_file();
        if matches!(
            mode,
            DurableOpenMode::ExistingWithMaintenance | DurableOpenMode::ExistingLight
        ) && !exists
        {
            return Err(DurableOpenError::Other(format!(
                "store missing: {}",
                db_path.display()
            )));
        }
        if !exists && matches!(mode, DurableOpenMode::CreateOrOpen) {
            let orphaned_namespace = ["-fsqlite-ns-gate", "-fsqlite-ns-use"]
                .iter()
                .any(|suffix| PathBuf::from(format!("{}{}", db_path.display(), suffix)).is_file());
            if orphaned_namespace {
                durable_integrity::reset_live_store_after_destructive(
                    db_path,
                    "orphaned fsqlite namespace sidecars without a live database",
                )
                .map_err(DurableOpenError::Other)?;
            }
        }
        let integrity_guard = if exists {
            match durable_integrity::gate_existing_store(db_path) {
                Ok(guard) => Some(guard),
                Err(gate)
                    if matches!(mode, DurableOpenMode::CreateOrOpen)
                        && gate.is_resettable_live_file() =>
                {
                    durable_integrity::reset_live_store_after_destructive(
                        db_path,
                        &gate.to_string(),
                    )
                    .map_err(DurableOpenError::Other)?;
                    exists = db_path.is_file();
                    None
                }
                Err(gate) => return Err(DurableOpenError::Gate(gate)),
            }
        } else {
            None
        };
        tick();
        let p = db_path.to_string_lossy().to_string();
        let conn = Connection::open_with_env(&p, recovery_connection_env()).map_err(|e| {
            DurableOpenError::Other(format!("fsqlite durable open failed for {p}: {e}"))
        })?;
        sql_profile::maybe_install_sql_profile(&conn);
        drop(integrity_guard);
        init_tables(&conn);
        tick();
        let pack_generation = load_pack_gen(&conn);
        let pack_path = pack_gen_path(db_path, pack_generation);
        let pack = match mode {
            DurableOpenMode::CreateOrOpen => PackFile::open(&pack_path),
            DurableOpenMode::ExistingWithMaintenance | DurableOpenMode::ExistingLight => {
                PackFile::open_existing(&pack_path)
            }
        };
        tick();
        let ast = if matches!(mode, DurableOpenMode::ExistingLight) {
            super::ast_store::AstStore::memory()
        } else {
            super::ast_store::AstStore::open(&ast_path_for_db(db_path))
                .unwrap_or_else(|_| super::ast_store::AstStore::memory())
        };
        tick();
        let next_id = match mode {
            DurableOpenMode::CreateOrOpen => load_next_id(&conn).unwrap_or(1),
            // Existing expand path does not mint seq ids from this handle.
            DurableOpenMode::ExistingWithMaintenance | DurableOpenMode::ExistingLight => 1,
        };
        let mut store = Self::from_parts(
            conn,
            next_id,
            pack,
            pack_generation,
            Some(db_path.to_path_buf()),
            ast,
        );
        store.restore_integrity_report();
        // hub-aligned store schema version stamp + skew check.
        if let Err(error) = store.ensure_store_schema_version() {
            return Err(DurableOpenError::Other(error));
        }
        if !matches!(mode, DurableOpenMode::ExistingLight) {
            store.repair_pack_locators_on_open();
            store.backfill_memory_paths();
            if let Err(error) = store.prune_transient_payloads() {
                store.last_store_error = Some(error);
            }
            store.maybe_reclaim_store_pages();
        }
        tick();
        if let Some(t0v) = t0 {
            let total_us = t0v.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
            let mode_s = match mode {
                DurableOpenMode::CreateOrOpen => "create_or_open",
                DurableOpenMode::ExistingWithMaintenance => "existing_with_maintenance",
                DurableOpenMode::ExistingLight => "existing_light",
            };
            emit_store_open_phases(serde_json::json!({
                "store_open_phases_us": {
                    "integrity_gate_us": phase_us[0],
                    "sqlite_open_us": phase_us[1],
                    "pack_us": phase_us[2],
                    "ast_us": phase_us[3],
                    "maintenance_us": phase_us[4],
                    "total_us": total_us,
                },
                "mode": mode_s,
                "existed": exists,
                "path": p,
            }));
        }
        Ok(store)
    }

    /// True when this store is backed by an on-disk SQLite file.
    pub fn is_durable(&self) -> bool {
        self.db_path.is_some()
    }

    /// Advance the integrity-gate mutation epoch after a durable COMMIT. Read-only opens must not call
    /// this: the epoch is the attestation identity, so bumping it forces the next open through
    /// `integrity_check`.
    pub(super) fn note_durable_mutation(&self) {
        let Some(path) = &self.db_path else {
            return;
        };
        if let Err(error) = durable_integrity::bump_mutation_epoch(path) {
            durable_integrity::invalidate_attestation(path);
            eprintln!(
                "fszero: mutation-epoch bump failed after durable commit: {error}; integrity attestation invalidated"
            );
        }
    }

    /// Current `PRAGMA synchronous` as SQLite's numeric mode (2 == FULL). Exposed so
    /// durability tests can assert that the transient-commit relaxation is scoped and always restored.
    pub fn synchronous_pragma(&self) -> Option<i64> {
        match self.conn.query("PRAGMA synchronous").ok()?.first()?.get(0) {
            Some(SqliteValue::Integer(n)) => Some(*n),
            Some(SqliteValue::Text(s)) => match s.as_str() {
                "FULL" => Some(2),
                "NORMAL" => Some(1),
                "OFF" => Some(0),
                _ => None,
            },
            _ => None,
        }
    }

    /// Current `PRAGMA busy_timeout` in milliseconds.
    /// Set at open from `durable_busy_wall`; used by contention tests and diagnostics.
    pub fn busy_timeout_ms(&self) -> Option<i64> {
        match self.conn.query("PRAGMA busy_timeout").ok()?.first()?.get(0) {
            Some(SqliteValue::Integer(n)) => Some(*n),
            Some(SqliteValue::Text(s)) => s.parse().ok(),
            _ => None,
        }
    }

    /// Rebind a stale process handle to the generation selected by durable
    /// metadata. Callers hold SQLite's write transaction, so the generation
    /// cannot rotate again between this check and their append.
    pub(super) fn refresh_pack_generation(&mut self, generation: i64) -> Result<(), String> {
        if generation == self.pack_generation {
            return Ok(());
        }
        let db_path = self
            .db_path
            .as_ref()
            .ok_or_else(|| "pack refresh: in-memory store".to_string())?;
        let path = pack_gen_path(db_path, generation);
        let pack = PackFile::open_existing(&path).ok_or_else(|| {
            format!(
                "pack refresh: active generation missing: {}",
                path.display()
            )
        })?;
        self.pack = Some(pack);
        self.pack_generation = generation;
        Ok(())
    }

    /// Reclaim free SQLite pages after a large delete/GC so a 100k-file
    /// project store does not stay sparse forever. Incremental vacuum only:
    /// a full VACUUM would rewrite the whole file on every open.
    fn maybe_reclaim_store_pages(&mut self) {
        let page_count = query_i64(&self.conn, "PRAGMA page_count").unwrap_or(0);
        let freelist = query_i64(&self.conn, "PRAGMA freelist_count").unwrap_or(0);
        if page_count < 256 || freelist * 8 < page_count {
            return;
        }
        let _ = self.conn.execute("PRAGMA incremental_vacuum(256)");
    }

    /// Validate only locators committed since the last durable watermark.
    /// Legacy stores and shortened/rotated packs are conservatively rescanned
    /// in 256-row pages. Queue + watermark changes share the payload transaction.
    fn repair_pack_locators_on_open(&mut self) {
        if self.pack.is_none() {
            return;
        }
        if self.conn.execute("BEGIN IMMEDIATE").is_err() {
            return;
        }
        let generation = load_pack_gen(&self.conn);
        if let Err(e) = self.refresh_pack_generation(generation) {
            let _ = self.conn.execute("ROLLBACK");
            self.last_store_error = Some(e);
            return;
        }
        let pack_len = match self.pack.as_ref().map(PackFile::current_len) {
            Some(Ok(pack_len)) => pack_len,
            Some(Err(e)) => {
                let _ = self.conn.execute("ROLLBACK");
                self.last_store_error = Some(e);
                return;
            }
            None => {
                let _ = self.conn.execute("ROLLBACK");
                self.last_store_error = Some("pack validation: active pack missing".to_string());
                return;
            }
        };
        let repaired_start = self.repaired_torn_pack_blobs.len();
        let mismatched = query_i64_params(
            &self.conn,
            "SELECT COUNT(*) FROM pack_validation_pending WHERE generation != ?1",
            &[sql_int(generation)],
        )
        .unwrap_or(1);
        let full = meta_i64(&self.conn, "pack_validation_version") != Some(1)
            || meta_i64(&self.conn, "pack_validated_generation") != Some(generation)
            || meta_i64(&self.conn, "pack_validated_len")
                .is_none_or(|n| n < 0 || pack_len < n as u64)
            || mismatched != 0;
        let result = if full {
            self.validate_all_pack_rows(pack_len)
        } else {
            self.validate_pending_pack_rows(generation, pack_len)
        }
        .and_then(|()| {
            self.exec_params_ctx(
                "DELETE FROM pack_validation_pending",
                &[],
                "clear validated pack queue",
            )
        })
        .and_then(|()| self.put_meta_i64("pack_validation_version", 1))
        .and_then(|()| self.put_meta_i64("pack_validated_generation", generation))
        .and_then(|()| {
            self.put_meta_i64("pack_validated_len", pack_len.min(i64::MAX as u64) as i64)
        });
        match result {
            Ok(()) => {
                if let Err(e) = self.conn.execute("COMMIT") {
                    let _ = self.conn.execute("ROLLBACK");
                    self.repaired_torn_pack_blobs.truncate(repaired_start);
                    self.last_store_error = Some(format!("pack validation commit: {e}"));
                } else {
                    self.note_durable_mutation();
                }
            }
            Err(e) => {
                let _ = self.conn.execute("ROLLBACK");
                self.repaired_torn_pack_blobs.truncate(repaired_start);
                self.last_store_error = Some(e);
            }
        }
    }

    fn validate_all_pack_rows(&mut self, pack_len: u64) -> Result<(), String> {
        let mut cursor: Option<String> = None;
        loop {
            let rows = match cursor.as_deref() {
                Some(after) => self.conn.query_with_params(
                    "SELECT key, value FROM payloads WHERE key > ?1 ORDER BY key LIMIT 256",
                    &[sql_text(after)],
                ),
                None => self
                    .conn
                    .query("SELECT key, value FROM payloads ORDER BY key LIMIT 256"),
            }
            .map_err(|e| format!("pack validation scan: {e}"))?;
            if rows.is_empty() {
                break;
            }
            for row in &rows {
                let Some((key, value)) = text_blob0_1(row) else {
                    continue;
                };
                cursor = Some(key.to_string());
                self.open_pack_rows_scanned = self.open_pack_rows_scanned.saturating_add(1);
                if decode_packed_locator(value)
                    .is_some_and(|(offset, len)| offset.saturating_add(u64::from(len)) > pack_len)
                {
                    self.remove_torn_pack_row(key, pack_len)?;
                }
            }
            if rows.len() < 256 {
                break;
            }
        }
        Ok(())
    }

    fn validate_pending_pack_rows(&mut self, generation: i64, pack_len: u64) -> Result<(), String> {
        let mut cursor = String::new();
        loop {
            let rows = self.conn.query_with_params("SELECT key, offset, len FROM pack_validation_pending WHERE generation = ?1 AND key > ?2 ORDER BY key LIMIT 256", &[sql_int(generation), sql_text(&cursor)]).map_err(|e| format!("pack pending scan: {e}"))?;
            if rows.is_empty() {
                break;
            }
            for row in &rows {
                let Some(key) = text_col_opt(row, 0) else {
                    continue;
                };
                cursor = key.clone();
                self.open_pack_rows_scanned = self.open_pack_rows_scanned.saturating_add(1);
                let offset = int_col_opt(row, 1).unwrap_or(i64::MAX).max(0) as u64;
                let len = int_col_opt(row, 2).unwrap_or(i64::MAX).max(0) as u64;
                if offset.saturating_add(len) > pack_len {
                    self.remove_torn_pack_row(&key, pack_len)?;
                }
            }
            if rows.len() < 256 {
                break;
            }
        }
        Ok(())
    }

    fn remove_torn_pack_row(&mut self, key: &str, pack_len: u64) -> Result<(), String> {
        self.note_integrity(format!(
            "torn_pack: {key} locator extended past pack EOF {pack_len}; removed during open repair"
        ));
        let params = [sql_text(key)];
        self.exec_params_ctx(SQL_DELETE_PAYLOAD_KEY, &params, "remove torn payload")?;
        if super::cas::full_blob_hash(key).is_some() {
            self.repaired_torn_pack_blobs.push(key.to_string());
        }
        if key.starts_with("mem://") {
            self.exec_params_ctx(
                SQL_DELETE_MEMORY_PATHS_BY_STORE_KEY,
                &params,
                "remove torn memory path",
            )?;
        }
        self.clear_payload_open_maintenance(key);
        Ok(())
    }

    /// Versioned, bounded legacy migration. A committed cursor resumes after a
    /// crash; current writers additionally enqueue mem:// keys atomically.
    fn backfill_memory_paths(&mut self) {
        if self.conn.execute("BEGIN IMMEDIATE").is_err() {
            return;
        }
        let result = self.backfill_memory_paths_batch();
        match result {
            Ok(()) => {
                if let Err(e) = self.conn.execute("COMMIT") {
                    let _ = self.conn.execute("ROLLBACK");
                    self.last_store_error = Some(format!("memory backfill commit: {e}"));
                } else {
                    self.note_durable_mutation();
                }
            }
            Err(e) => {
                let _ = self.conn.execute("ROLLBACK");
                self.last_store_error = Some(e);
            }
        }
    }

    fn backfill_memory_paths_batch(&mut self) -> Result<(), String> {
        let state = self
            .conn
            .query_with_params(
                "SELECT version, cursor FROM store_migrations WHERE name = 'memory_paths'",
                &[],
            )
            .map_err(|e| format!("memory migration state: {e}"))?;
        let version = state
            .first()
            .and_then(|row| int_col_opt(row, 0))
            .unwrap_or(0);
        let cursor = state
            .first()
            .and_then(|row| text_col_opt(row, 1))
            .unwrap_or_default();
        if version < 1 {
            let rows = self.conn.query_with_params("SELECT key FROM payloads WHERE key LIKE 'mem://%' AND key > ?1 ORDER BY key LIMIT 257", &[sql_text(&cursor)]).map_err(|e| format!("memory legacy scan: {e}"))?;
            let keys: Vec<String> = rows.iter().filter_map(|row| text_col_opt(row, 0)).collect();
            let take = keys.len().min(256);
            for key in keys.iter().take(take) {
                self.backfill_one_memory_key(key)?;
            }
            let more = keys.len() > 256;
            let next = keys
                .get(take.saturating_sub(1))
                .map(String::as_str)
                .unwrap_or(&cursor);
            return self.exec_params_ctx("INSERT OR REPLACE INTO store_migrations (name, version, cursor) VALUES ('memory_paths', ?1, ?2)", &[sql_int(if more { 0 } else { 1 }), sql_text(if more { next } else { "" })], "persist memory migration cursor");
        }
        let rows = self
            .conn
            .query("SELECT store_key FROM memory_backfill_pending ORDER BY store_key LIMIT 256")
            .map_err(|e| format!("memory pending scan: {e}"))?;
        let keys: Vec<String> = rows.iter().filter_map(|row| text_col_opt(row, 0)).collect();
        for key in keys {
            self.backfill_one_memory_key(&key)?;
            self.exec_params_ctx(
                "DELETE FROM memory_backfill_pending WHERE store_key = ?1",
                &[sql_text(&key)],
                "clear memory backfill queue",
            )?;
        }
        Ok(())
    }

    fn backfill_one_memory_key(&mut self, key: &str) -> Result<(), String> {
        self.open_memory_rows_scanned = self.open_memory_rows_scanned.saturating_add(1);
        let Some(bytes) = self.get_payload(key) else {
            return Err(format!("memory backfill payload unavailable: {key}"));
        };
        let content_ref = format!(
            "z://blob/{}",
            fszero_core::hexutil::sha256_hex_of(Sha256::digest(&bytes).into())
        );
        self.exec_params_ctx(
            SQL_INSERT_MEMORY_PATHS_IGNORE,
            &[
                sql_text(key.trim_start_matches("mem://")),
                sql_text(key),
                sql_text(&content_ref),
                sql_int(unix_epoch_secs()),
            ],
            "backfill memory path",
        )
    }

    pub(super) fn track_payload_open_maintenance(
        &mut self,
        key: &str,
        row: &[u8],
    ) -> Result<(), String> {
        let params = [sql_text(key)];
        if let Some((offset, len)) = decode_packed_locator(row) {
            self.exec_params_ctx("INSERT OR REPLACE INTO pack_validation_pending (key, generation, offset, len) VALUES (?1, ?2, ?3, ?4)", &[sql_text(key), sql_int(self.pack_generation), sql_int(offset.min(i64::MAX as u64) as i64), sql_int(i64::from(len))], "queue pack validation")?;
        } else {
            self.exec_params_ctx(
                "DELETE FROM pack_validation_pending WHERE key = ?1",
                &params,
                "clear pack validation",
            )?;
        }
        if key.starts_with("mem://") {
            self.exec_params_ctx(
                "INSERT OR IGNORE INTO memory_backfill_pending (store_key) VALUES (?1)",
                &params,
                "queue memory backfill",
            )?;
        }
        Ok(())
    }

    fn clear_payload_open_maintenance(&mut self, key: &str) {
        let params = [sql_text(key)];
        let _ = self.exec_params(
            "DELETE FROM pack_validation_pending WHERE key = ?1",
            &params,
        );
        let _ = self.exec_params(
            "DELETE FROM memory_backfill_pending WHERE store_key = ?1",
            &params,
        );
    }

    /// Public open for session store-map expand.
    pub fn try_open_existing_durable_pub(db_path: &Path) -> Result<Self, String> {
        Self::try_open_existing_durable(db_path)
    }

    /// Attach the canonical shared CAS tier. Reads consult it
    /// first; mints dual-write into it.
    pub fn attach_cas(&mut self, cas: super::cas::CasStore) {
        self.cas = Some(cas);
    }

    /// Keep an isolated fixture from publishing or consulting cross-root refs.
    pub fn disable_ref_index(&mut self) {
        self.ref_index_disabled = true;
    }

    /// Detect blobs/ under the effective store root and attach when present.
    pub fn attach_cas_if_detected(&mut self, root: &std::path::Path) {
        if let Some(store_root) = super::zerostack_store::zerostack_store_or_detect(root) {
            if let Some(cas) = super::cas::CasStore::detect(&store_root) {
                self.attach_cas(cas);
            }
        }
    }

    /// Whether the canonical shared CAS tier is attached.
    pub fn cas_attached(&self) -> bool {
        self.cas.is_some()
    }

    /// Live writability probe of the attached CAS (`None` when detached).
    /// See [`super::cas::CasStore::probe_writable`].
    pub fn cas_writable(&self) -> Option<bool> {
        self.cas.as_ref().map(|cas| cas.probe_writable())
    }

    /// Leftover CAS temp objects (`0` when CAS is detached).
    pub fn cas_tmp_object_count(&self) -> u64 {
        self.cas
            .as_ref()
            .map(|cas| cas.tmp_object_count())
            .unwrap_or(0)
    }

    fn live_cas_hashes(&self) -> Result<HashSet<String>, String> {
        let query = SQL_SELECT_LIVE_REFS;
        let rows = self
            .conn
            .query(query)
            .map_err(|e| format!("query CAS roots: {e}"))?;
        let mut hashes = HashSet::new();
        for row in rows {
            let Some(reference) = text_col_opt(&row, 0) else {
                continue;
            };
            if let Some(hash) = super::cas::full_blob_hash(&reference) {
                hashes.insert(hash.to_string());
            }
        }
        Ok(hashes)
    }

    /// Full FSZero reachability root set from live references.
    fn reachability_root_hashes(&self) -> Result<HashSet<String>, String> {
        self.live_cas_hashes()
    }

    /// Publish `gc/roots/fszero/<project_id>/current.json` for the live root set.
    /// No-op when CAS is detached.
    pub fn publish_cas_gc_roots(&self) -> Result<Option<super::cas::GcRootsPublish>, String> {
        let Some(cas) = self.cas.as_ref() else {
            return Ok(None);
        };
        let roots = self.reachability_root_hashes()?;
        let published = cas.publish_gc_roots(roots)?;
        Ok(Some(published))
    }

    /// Explicit FSZero mark-and-sweep entry point. Query failures abort rather
    /// than risk collecting a live shared object. Publishes FSZero reachability
    /// roots before sweeping so peer collectors see the live set.
    pub fn run_cas_gc(&self) -> Result<super::cas::CasGcReport, String> {
        let Some(cas) = self.cas.as_ref() else {
            return Ok(super::cas::CasGcReport::default());
        };
        let marked = self.reachability_root_hashes()?;
        // Best-effort publish: a roots I/O failure must not block local GC of
        // unreferenced objects, but peers then retain-on-uncertainty for missing
        // metadata. Log via Err only when the caller surfaces the report path.
        let _ = cas.publish_gc_roots(marked.iter().cloned());
        cas.gc(&marked, super::cas::gc_grace_from_env())
    }

    fn run_cas_gc_if_due(&self) -> Result<Option<super::cas::CasGcReport>, String> {
        let Some(cas) = self.cas.as_ref() else {
            return Ok(None);
        };
        let marker = cas.blobs_root().with_extension("gc.last");
        let recent = std::fs::metadata(&marker)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|mtime| std::time::SystemTime::now().duration_since(mtime).ok())
            .is_some_and(|age| age < std::time::Duration::from_secs(24 * 60 * 60));
        if recent {
            return Ok(None);
        }
        let report = self.run_cas_gc()?;
        std::fs::write(
            &marker,
            b"fszero-cas-gc
",
        )
        .map_err(|e| format!("write CAS GC marker {}: {e}", marker.display()))?;
        Ok(Some(report))
    }
}

/// Hard resource ceilings keep explicit and opportunistic compaction from
/// turning one request into an unbounded live-set rewrite.
const PACK_COMPACTION_MAX_ROWS: usize = 262_144;
const PACK_COMPACTION_MAX_LIVE_BYTES: u64 = 512 * 1024 * 1024;

fn ensure_pack_compaction_bounds(rows: usize, live_bytes: u64) -> Result<(), String> {
    if rows > PACK_COMPACTION_MAX_ROWS {
        return Err(format!(
            "compact_pack: live-set row bound exceeded ({rows} > {PACK_COMPACTION_MAX_ROWS})"
        ));
    }
    if live_bytes > PACK_COMPACTION_MAX_LIVE_BYTES {
        return Err(format!(
            "compact_pack: live-set byte bound exceeded ({live_bytes} > {PACK_COMPACTION_MAX_LIVE_BYTES})"
        ));
    }
    Ok(())
}

struct PackedExtent {
    key: String,
    offset: u64,
    len: u32,
}

struct PreparedPackRotation {
    old_len: u64,
    new_pack: PackFile,
    moves: Vec<(String, Vec<u8>)>,
    dropped: Vec<String>,
}

fn inventory_packed_rows(
    rows: Vec<Row>,
    pack_len: u64,
) -> Result<(Vec<PackedExtent>, Vec<String>), String> {
    let mut live = Vec::new();
    let mut dropped = Vec::new();
    let mut live_bytes = 0u64;
    let mut packed_rows = 0usize;
    for row in rows {
        let Some((key, value)) = text_blob0_1(&row) else {
            continue;
        };
        let Some((offset, len)) = decode_packed_locator(value) else {
            continue;
        };
        packed_rows += 1;
        ensure_pack_compaction_bounds(packed_rows, live_bytes)?;
        if offset.saturating_add(len as u64) > pack_len {
            dropped.push(key.to_string());
            continue;
        }
        live_bytes = live_bytes.saturating_add(len as u64);
        ensure_pack_compaction_bounds(packed_rows, live_bytes)?;
        live.push(PackedExtent {
            key: key.to_string(),
            offset,
            len,
        });
    }
    Ok((live, dropped))
}

fn rewrite_packed_rows(
    old_pack: &PackFile,
    new_pack: &mut PackFile,
    live: Vec<PackedExtent>,
    mut dropped: Vec<String>,
) -> Result<(Vec<(String, Vec<u8>)>, Vec<String>), String> {
    let mut moves = Vec::with_capacity(live.len());
    for extent in live {
        let Some(bytes) = old_pack.read(extent.offset, extent.len) else {
            dropped.push(extent.key);
            continue;
        };
        let (offset, len) = new_pack
            .append(&bytes)
            .ok_or_else(|| "compact_pack: append failed".to_string())?;
        moves.push((extent.key, encode_packed_locator(offset, len)));
    }
    Ok((moves, dropped))
}

fn remove_stale_pack_generation(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "compact_pack: remove stale {}: {error}",
            path.display()
        )),
    }
}

impl RecoveryStore {
    /// Pack sidecar GC: copy every live packed payload into a fresh generation, fsync the file and its
    /// directory entry, then publish every locator plus `pack_gen` in one FULL-synchronous SQLite
    /// commit. The SQLite writer lock serializes rotation with packed appends.
    pub fn compact_pack(&mut self) -> Result<(u64, u64), String> {
        if self.pending_payloads.is_some() {
            return Err("compact_pack: refusing mid-batch".to_string());
        }
        let db_path = self
            .db_path
            .clone()
            .ok_or_else(|| "compact_pack: in-memory store".to_string())?;
        self.conn
            .execute("BEGIN IMMEDIATE")
            .map_err(|e| format!("compact_pack begin: {e}"))?;
        let staged = self.stage_pack_rotation(&db_path);
        let (old_generation, prepared) = match staged {
            Ok(staged) => staged,
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK");
                return Err(error);
            }
        };
        self.commit_pack_rotation()?;
        Ok(self.install_pack_rotation(&db_path, old_generation, prepared))
    }

    fn stage_pack_rotation(
        &mut self,
        db_path: &Path,
    ) -> Result<(i64, PreparedPackRotation), String> {
        let old_generation = load_pack_gen(&self.conn);
        self.refresh_pack_generation(old_generation)?;
        let prepared = self.prepare_pack_rotation(db_path, old_generation)?;
        for (key, locator) in &prepared.moves {
            self.exec_params_ctx(
                SQL_UPDATE_PAYLOAD_VALUE,
                &[sql_text(key), SqliteValue::Blob(Arc::from(locator.clone()))],
                "compact_pack",
            )?;
        }
        for key in &prepared.dropped {
            self.exec_params_ctx(
                SQL_DELETE_PAYLOAD_KEY,
                &[sql_text(key)],
                "compact_pack drop torn payload",
            )?;
        }
        self.put_meta_i64("pack_gen", old_generation + 1)?;
        self.put_meta_i64("pack_validation_version", 1)?;
        self.put_meta_i64("pack_validated_generation", old_generation + 1)?;
        self.put_meta_i64(
            "pack_validated_len",
            prepared.new_pack.len.min(i64::MAX as u64) as i64,
        )?;
        self.exec_params_ctx(
            "DELETE FROM pack_validation_pending",
            &[],
            "compact_pack validation reset",
        )?;
        Ok((old_generation, prepared))
    }

    fn commit_pack_rotation(&mut self) -> Result<(), String> {
        match self.conn.execute("COMMIT") {
            Ok(_) => {
                self.note_durable_mutation();
                Ok(())
            }
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK");
                Err(format!("compact_pack commit: {error}"))
            }
        }
    }

    fn install_pack_rotation(
        &mut self,
        db_path: &Path,
        old_generation: i64,
        prepared: PreparedPackRotation,
    ) -> (u64, u64) {
        let old_len = prepared.old_len;
        let new_len = prepared.new_pack.len;
        let dropped = prepared.dropped.len();
        self.pack = Some(prepared.new_pack);
        self.pack_generation = old_generation + 1;
        if dropped > 0 {
            self.note_integrity(format!(
                "pack_gc: dropped {dropped} unreadable (torn) packed payload(s) during compaction"
            ));
        }
        // Bounded cleanup: retire exactly the prior active generation. A
        // crash orphan at the next generation is replaced on the next run.
        let _ = std::fs::remove_file(pack_gen_path(db_path, old_generation));
        (old_len, new_len)
    }

    fn prepare_pack_rotation(
        &self,
        db_path: &Path,
        old_generation: i64,
    ) -> Result<PreparedPackRotation, String> {
        let old_pack = self
            .pack
            .as_ref()
            .ok_or_else(|| "compact_pack: active pack unavailable".to_string())?;
        old_pack.lock_exclusive()?;
        let prepared = self.build_pack_rotation(db_path, old_generation, old_pack);
        old_pack.unlock();
        prepared
    }

    fn build_pack_rotation(
        &self,
        db_path: &Path,
        old_generation: i64,
        old_pack: &PackFile,
    ) -> Result<PreparedPackRotation, String> {
        let old_len = old_pack.current_len()?;
        let new_path = pack_gen_path(db_path, old_generation + 1);
        remove_stale_pack_generation(&new_path)?;
        let mut new_pack = PackFile::create_fresh(&new_path)
            .ok_or_else(|| format!("compact_pack: cannot create {}", new_path.display()))?;
        let rows = self
            .conn
            .query(SQL_SELECT_PAYLOAD_KV)
            .map_err(|e| format!("compact_pack inventory: {e}"))?;
        let (live, dropped) = inventory_packed_rows(rows, old_len)?;
        let (moves, dropped) = rewrite_packed_rows(old_pack, &mut new_pack, live, dropped)?;
        new_pack.sync_all()?;
        sync_parent_dir(&new_path)?;
        Ok(PreparedPackRotation {
            old_len,
            new_pack,
            moves,
            dropped,
        })
    }

    /// Live packed bytes vs pack file length — the GC policy input.
    pub fn pack_report(&self) -> (u64, u64) {
        let pack_len = self.pack.as_ref().map(|p| p.len).unwrap_or(0);
        let mut live = 0u64;
        if let Ok(rows) = self.conn.query(SQL_SELECT_PAYLOAD_VALUES) {
            for row in rows {
                if let Some(SqliteValue::Blob(value)) = row.get(0)
                    && let Some((_, len)) = decode_packed_locator(value)
                {
                    live += len as u64;
                }
            }
        }
        (live, pack_len)
    }

    /// Re-open the durable connection in place so subsequent reads observe other processes' committed
    /// writes: a connection opened before another process's index build commit can serve stale manifest
    /// reads, making a blocked single-indexer loser re-run the entire cold build the winner.
    pub fn reopen_durable(&mut self) -> bool {
        if self.pending_payloads.is_some() {
            return false;
        }
        let Some(db_path) = self.db_path.clone() else {
            return false;
        };
        match Self::try_with_durable(&db_path) {
            Ok(mut fresh) => {
                fresh.store_writes.set(self.store_writes.get());
                fresh.cache_hits.set(self.cache_hits.get());
                fresh.cache_misses.set(self.cache_misses.get());
                fresh
                    .cache_miss_dependency_root
                    .set(self.cache_miss_dependency_root.get());
                fresh.cache_miss_witness.set(self.cache_miss_witness.get());
                fresh
                    .cache_miss_coarse_wipe
                    .set(self.cache_miss_coarse_wipe.get());
                fresh.bytes_materialized.set(self.bytes_materialized.get());
                fresh.next_id = fresh.next_id.max(self.next_id);
                fresh.cas = self.cas.take();
                *self = fresh;
                true
            }
            Err(e) => {
                self.last_store_error = Some(e);
                false
            }
        }
    }
}

/// Real-SQLite AST sidecar next to the store DB (store.sqlite3.ast).
fn ast_path_for_db(db_path: &Path) -> std::path::PathBuf {
    let mut os = db_path.as_os_str().to_owned();
    os.push(".ast");
    std::path::PathBuf::from(os)
}

/// Probe-before-ALTER: only add a column when the SELECT against it fails.
fn ensure_column(conn: &Connection, probe_sql: &str, alter_sql: &str) {
    if conn.execute(probe_sql).is_err() {
        let _ = conn.execute(alter_sql);
    }
}

const AST_NODES_DDL: &str = "CREATE TABLE IF NOT EXISTS ast_nodes (id INTEGER PRIMARY KEY, file_key TEXT, kind TEXT, span_start INTEGER, span_end INTEGER, symbol TEXT, parent INTEGER, version INTEGER DEFAULT 0)";

fn init_tables(conn: &Connection) {
    // 64MB page cache: bulk indexing of multi-hundred-MB corpora thrashes a small cache into a
    // read-modify-write storm (profiled: 26% of a fresh 23k-file (208MB) scale index was pager
    // re-reads). Memory is transient and bounded.
    let _ = conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA temp_store=MEMORY; PRAGMA cache_size=-65536;\
         CREATE TABLE IF NOT EXISTS payloads (key TEXT PRIMARY KEY, value BLOB);\
         CREATE TABLE IF NOT EXISTS payload_lru (key TEXT PRIMARY KEY, tick INTEGER);\
         CREATE INDEX IF NOT EXISTS idx_payload_lru_tick ON payload_lru(tick);\
         CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v INTEGER);\
         CREATE TABLE IF NOT EXISTS integrity_state (id INTEGER PRIMARY KEY, violations INTEGER NOT NULL, detail TEXT NOT NULL);CREATE TABLE IF NOT EXISTS edit_intents (id INTEGER PRIMARY KEY, root TEXT NOT NULL, path TEXT NOT NULL, state TEXT NOT NULL, pre BLOB NOT NULL, post BLOB NOT NULL, pre_ref TEXT NOT NULL, post_ref TEXT NOT NULL, pre_mtime_ns INTEGER NOT NULL, pre_mode INTEGER NOT NULL, pre_xattrs TEXT NOT NULL, created_ns INTEGER NOT NULL);\
         CREATE TABLE IF NOT EXISTS mutation_log (seq INTEGER PRIMARY KEY, ts INTEGER NOT NULL, op TEXT NOT NULL, path TEXT NOT NULL, pre_ref TEXT NOT NULL, post_ref TEXT NOT NULL, created INTEGER NOT NULL DEFAULT 0, session_window INTEGER NOT NULL DEFAULT 0, agent TEXT NOT NULL DEFAULT '', pre_mtime_ns INTEGER NOT NULL DEFAULT 0, pre_mode INTEGER NOT NULL DEFAULT -1, pre_xattrs TEXT NOT NULL DEFAULT '');",
    );
    // Steady-state multi-writer wait policy:
    // stock/fsqlite default is fail-immediate on busy; that made begin_txn_core
    // fail-open without waiting. Align with durable_busy_wall (env-overridable).
    let busy_ms = durable_busy_wall().as_millis().max(1);
    let _ = conn.execute(&format!("PRAGMA busy_timeout={busy_ms}"));
    // Legacy migration (same probe-before-ALTER discipline as ast_nodes
    // below): pre-fidelity stores lack the mtime/mode columns.
    ensure_column(
        conn,
        "SELECT pre_mtime_ns FROM mutation_log LIMIT 0",
        "ALTER TABLE mutation_log ADD COLUMN pre_mtime_ns INTEGER NOT NULL DEFAULT 0",
    );
    ensure_column(
        conn,
        "SELECT pre_mode FROM mutation_log LIMIT 0",
        "ALTER TABLE mutation_log ADD COLUMN pre_mode INTEGER NOT NULL DEFAULT -1",
    );
    ensure_column(
        conn,
        "SELECT pre_xattrs FROM mutation_log LIMIT 0",
        "ALTER TABLE mutation_log ADD COLUMN pre_xattrs TEXT NOT NULL DEFAULT ''",
    );
    let _ = conn.execute("CREATE INDEX IF NOT EXISTS idx_mutation_log_path ON mutation_log(path)");
    let _ = conn.execute(AST_NODES_DDL);
    // Legacy migration: pre-version stores lack the column. Probe before altering — fsqlite
    // accepts a duplicate ADD COLUMN silently (stock SQLite errors), which writes a malformed
    // schema (two `version` columns) that spec-compliant readers then refuse to prepare against.
    ensure_column(
        conn,
        "SELECT version FROM ast_nodes LIMIT 0",
        "ALTER TABLE ast_nodes ADD COLUMN version INTEGER DEFAULT 0",
    );
    // Repair: stores written before the probe-before-ALTER discipline above can carry a malformed
    // ast_nodes schema (duplicate `version` column).
    if let Ok(rows) =
        conn.query("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'ast_nodes'")
        && let Some(sql) = rows.first().and_then(|row| text_col_opt(row, 0))
        && sql.matches("version").count() > 1
    {
        let _ = conn.execute("DROP TABLE ast_nodes");
        let _ = conn.execute(AST_NODES_DDL);
    }
    // Durable speculative worlds + mem:// path index + remaining AST/facts indexes.
    let _ = conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_ast_nodes_symbol ON ast_nodes(kind, symbol);\
         CREATE INDEX IF NOT EXISTS idx_ast_nodes_file ON ast_nodes(file_key, version);\
         CREATE TABLE IF NOT EXISTS ast_edges (src INTEGER, dst INTEGER, kind TEXT);\
         CREATE TABLE IF NOT EXISTS call_edges (file_key TEXT, caller TEXT, callee TEXT, line INTEGER, version INTEGER DEFAULT 0, PRIMARY KEY(file_key, caller, callee, line, version));\
         CREATE INDEX IF NOT EXISTS idx_call_edges_callee ON call_edges(callee, version);\
         CREATE TABLE IF NOT EXISTS facts (subject_ref TEXT, predicate TEXT, object_ref TEXT, evidence_ref TEXT, version INTEGER, agent TEXT, PRIMARY KEY(subject_ref, predicate, object_ref, evidence_ref, version, agent));\
         CREATE INDEX IF NOT EXISTS idx_facts_subject ON facts(subject_ref, predicate);\
         CREATE TABLE IF NOT EXISTS worlds (wid TEXT PRIMARY KEY, state TEXT NOT NULL, cert_ref TEXT NOT NULL, created_ts INTEGER NOT NULL, session_window INTEGER NOT NULL DEFAULT 0);\
         CREATE TABLE IF NOT EXISTS world_edits (wid TEXT NOT NULL, ord INTEGER NOT NULL, path TEXT NOT NULL, cert_ref TEXT NOT NULL, PRIMARY KEY (wid, ord));\
         CREATE INDEX IF NOT EXISTS idx_worlds_state ON worlds(state);\
         CREATE TABLE IF NOT EXISTS memory_paths (path TEXT PRIMARY KEY, store_key TEXT NOT NULL, content_ref TEXT NOT NULL, updated_ts INTEGER NOT NULL);\
         CREATE INDEX IF NOT EXISTS idx_memory_paths_prefix ON memory_paths(path);\
         CREATE TABLE IF NOT EXISTS pack_validation_pending (key TEXT PRIMARY KEY, generation INTEGER NOT NULL, offset INTEGER NOT NULL, len INTEGER NOT NULL);\
         CREATE TABLE IF NOT EXISTS store_migrations (name TEXT PRIMARY KEY, version INTEGER NOT NULL, cursor TEXT NOT NULL);\
         CREATE TABLE IF NOT EXISTS memory_backfill_pending (store_key TEXT PRIMARY KEY);\
         CREATE TABLE IF NOT EXISTS chunk_blobs (digest TEXT PRIMARY KEY, content_ref TEXT NOT NULL, len INTEGER NOT NULL);\
         CREATE TABLE IF NOT EXISTS file_chunks (path TEXT NOT NULL, ordinal INTEGER NOT NULL, start_byte INTEGER NOT NULL, end_byte INTEGER NOT NULL, digest TEXT NOT NULL, content_ref TEXT NOT NULL, PRIMARY KEY(path, ordinal));\
         CREATE INDEX IF NOT EXISTS idx_file_chunks_digest ON file_chunks(digest);",);
    super::access_log::init_access_log_table(conn);
}

fn load_next_id(conn: &Connection) -> Option<u32> {
    if let Some(v) = meta_i64(conn, "next_id") {
        return Some((v as u32).max(1));
    }
    let mut max = 0u32;
    if let Ok(rows) = conn.query(SQL_SELECT_PAYLOAD_KEYS) {
        for row in rows {
            if let Some(ks) = text_col_opt(&row, 0) {
                if let Some(last) = ks.rsplit('/').next() {
                    if let Ok(n) = last.parse::<u32>() {
                        max = max.max(n);
                    }
                }
            }
        }
    }
    if max > 0 { Some(max + 1) } else { None }
}

pub fn text_col_opt(row: &Row, idx: usize) -> Option<String> {
    row.get(idx).and_then(|v| match v {
        SqliteValue::Text(t) => Some(t.as_str().to_string()),
        _ => None,
    })
}

pub fn text_col(row: &Row, idx: usize) -> String {
    text_col_opt(row, idx).unwrap_or_default()
}

pub fn int_col_opt(row: &Row, idx: usize) -> Option<i64> {
    row.get(idx).and_then(|v| match v {
        SqliteValue::Integer(i) => Some(*i),
        _ => None,
    })
}

pub fn int_col(row: &Row, idx: usize) -> i64 {
    int_col_opt(row, idx).unwrap_or_default()
}

#[inline]
fn text_blob0_1(row: &Row) -> Option<(&str, &[u8])> {
    match (row.get(0), row.get(1)) {
        (Some(SqliteValue::Text(k)), Some(SqliteValue::Blob(v))) => Some((k.as_str(), v.as_ref())),
        _ => None,
    }
}

#[inline]
pub fn sql_text(s: &str) -> SqliteValue {
    SqliteValue::Text(s.into())
}

#[inline]
pub fn sql_int(n: i64) -> SqliteValue {
    SqliteValue::Integer(n)
}

/// First-column i64 from a SELECT (None on fail / empty / non-integer).
pub fn query_i64(conn: &Connection, sql: &str) -> Option<i64> {
    query_i64_params(conn, sql, &[])
}

fn query_rows(conn: &Connection, sql: &str, params: &[SqliteValue]) -> Option<Vec<fsqlite::Row>> {
    let rows = if params.is_empty() {
        conn.query(sql)
    } else {
        conn.query_with_params(sql, params)
    };
    rows.ok()
}

pub fn query_i64_params(conn: &Connection, sql: &str, params: &[SqliteValue]) -> Option<i64> {
    query_rows(conn, sql, params)?
        .first()
        .and_then(|r| int_col_opt(r, 0))
}

/// Read meta k as i64 (None if missing/non-integer/query fail).
pub fn meta_i64(conn: &Connection, k: &str) -> Option<i64> {
    query_i64_params(conn, SQL_SELECT_META_V, &[sql_text(k)])
}

/// Collect column-0 text from a SELECT (empty on query failure).
fn query_text0(conn: &Connection, sql: &str) -> Vec<String> {
    query_text0_params(conn, sql, &[])
}

fn query_text0_params(conn: &Connection, sql: &str, params: &[SqliteValue]) -> Vec<String> {
    query_rows(conn, sql, params)
        .into_iter()
        .flatten()
        .filter_map(|row| text_col_opt(&row, 0))
        .collect()
}

pub fn recovery_key_priority(key: &str) -> u8 {
    if NAMED_PAYLOAD_KEYS.contains(&key) || key.ends_with("/bytes") {
        0
    } else if key.ends_with("/ref") {
        1
    } else if key.starts_with("z://blob/") {
        2
    } else {
        3
    }
}

impl RecoveryStore {
    /// Parameterized write inside an optional explicit txn (no statement savepoint).
    pub(super) fn exec_params(&mut self, sql: &str, params: &[SqliteValue]) -> Result<(), String> {
        self.conn
            .execute_with_params_skip_statement_savepoint_in_explicit_txn(sql, params)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub(super) fn exec_params_ctx(
        &mut self,
        sql: &str,
        params: &[SqliteValue],
        ctx: impl std::fmt::Display,
    ) -> Result<(), String> {
        self.exec_params(sql, params)
            .map_err(|e| format!("{ctx}: {e}"))
    }

    /// Upsert a single integer meta row (`k` → `v`).
    pub fn put_meta_i64(&mut self, k: &str, v: i64) -> Result<(), String> {
        self.exec_params_ctx(
            SQL_INSERT_META_KV,
            &[sql_text(k), sql_int(v)],
            format!("store failed for {k}"),
        )
    }

    /// Stamp / check store schema version. Overall +
    /// per-segment (journal, bookmarks, quarantine) meta keys.
    pub fn ensure_store_schema_version(&mut self) -> Result<(), String> {
        use super::store_schema_version::{
            META_STORE_SCHEMA_VERSION, STORE_SCHEMA_VERSION, SchemaSkew, check_schema_skew,
        };
        let found = meta_i64(&self.conn, META_STORE_SCHEMA_VERSION).unwrap_or(0) as u32;
        match check_schema_skew(found) {
            SchemaSkew::Compatible | SchemaSkew::OlderMinor { .. } => {
                // Older minor: degrade gracefully (still open). Stamp missing segments.
                self.stamp_schema_segments()?;
                Ok(())
            }
            SchemaSkew::UpgradeRequired { expected, .. } => {
                self.put_meta_i64(META_STORE_SCHEMA_VERSION, expected as i64)?;
                self.stamp_schema_segments()?;
                let _ = STORE_SCHEMA_VERSION;
                Ok(())
            }
            SchemaSkew::DowngradeRefused { found, expected } => Err(format!(
                "store schema too new: found {found}, this binary expects {expected} (refusing silent downgrade)"
            )),
        }
    }

    fn stamp_schema_segments(&mut self) -> Result<(), String> {
        use super::store_schema_version::{
            SCHEMA_SEGMENTS, STORE_SCHEMA_VERSION, segment_meta_key,
        };
        for seg in SCHEMA_SEGMENTS {
            self.put_meta_i64(&segment_meta_key(seg), STORE_SCHEMA_VERSION as i64)?;
        }
        Ok(())
    }

    pub fn put(&mut self, kind: &str, data: &[u8]) -> String {
        let id = self.next_id;
        self.next_id += 1;
        let r = format!("seq/{}/{}", kind, id);
        if let Err(e) = self.try_put_key(&r, data) {
            self.last_store_error = Some(e);
            return format!("seq/{kind}/error");
        }
        r
    }

    /// Durable SQLite path when this store is on disk; `None` for in-memory.
    pub fn store_db_path(&self) -> Option<&Path> {
        self.db_path.as_deref()
    }

    pub fn put_content_ref(&mut self, data: &[u8]) -> String {
        match self.try_put_content_ref(data) {
            Ok(r) => r,
            Err(e) => {
                self.last_store_error = Some(e);
                "z://blob/error".to_string()
            }
        }
    }

    pub fn try_put_content_ref(&mut self, data: &[u8]) -> Result<String, String> {
        let mut h = Sha256::new();
        h.update(data);
        let hash = fszero_core::hexutil::sha256_hex_of(h.finalize().into());
        let r = format!("{}://blob/{hash}", EMITTED_SCHEME.as_str());
        // Warm hit: payload already durable — skip CAS dual-write and ref-index append (idempotent
        // identity; no new mint work). Still hash once so the address is content-bound ( warm path).
        if self.has_payload(&r) {
            return Ok(r);
        }
        self.try_put_key(&r, data)?;
        // The shared CAS deduplicates identical checkouts. The primary store already owns the
        // bytes, so a CAS write failure only costs sharing.
        if let Some(cas) = &self.cas {
            let _ = cas.put_prehashed(&hash, data);
        }
        self.append_ref_index(&r);
        Ok(r)
    }

    fn append_ref_index(&self, ref_id: &str) -> bool {
        if self.ref_index_disabled {
            return false;
        }
        let Some(store_path) = self.db_path.as_ref() else {
            return false;
        };
        let Some(shard) = ref_index_shard_path(ref_id) else {
            return false;
        };
        let Some(dir) = shard.parent() else {
            return false;
        };
        if ensure_ref_index_dir(dir).is_err() {
            return false;
        }
        let store_id = super::zerostack_store::store_id_for_db_path(store_path);
        let line = serde_json::json!({
            "ref_id": ref_id, "store_path": store_path, "store_id": store_id, "ts": unix_epoch_millis(),
        }).to_string();
        let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&shard)
        else {
            return false;
        };
        use std::io::Write;
        let mut record = line.into_bytes();
        record.push(b'\n');
        match file.write(&record) {
            Ok(n) if n == record.len() => {}
            _ => return false,
        }
        // Append changes mtime/len; drop cache so next lookup reloads.
        invalidate_ref_index_shard_cache(&shard);
        if file
            .metadata()
            .map(|m| m.len() > REF_INDEX_SHARD_MAX_BYTES)
            .unwrap_or(false)
        {
            let _ = write_compacted_ref_index_shard(&shard, false);
        }
        true
    }

    pub fn take_store_error(&mut self) -> Option<String> {
        self.last_store_error.take()
    }

    /// Test hook that queues a store fault for the next `take_store_error`.
    pub fn inject_store_error_for_test(&mut self, msg: impl Into<String>) {
        self.last_store_error = Some(msg.into());
    }

    /// Payload-store presence counters, not a 3C (compulsory/capacity/conflict) hit
    /// rate. `cache_hits` / `cache_misses` count whether a recovery payload key was
    /// already present. They are not SQLite page-cache stats and have no reuse-distance.
    pub fn metric_snapshot(&self) -> (u64, u64, u64, u64) {
        (
            self.cache_hits.get(),
            self.cache_misses.get(),
            self.store_writes.get(),
            self.bytes_materialized.get(),
        )
    }

    /// Payload cache hits/misses plus Q99 miss-cause split. Causes are additive detail under
    /// aggregate `cache_misses` when the miss path attributes a reason; unattributed cold
    /// misses only bump the aggregate. `coarse_wipe` counts entries dropped by full query-cache clear.
    pub fn cache_miss_cause_snapshot(&self) -> CacheMissCauseSnapshot {
        CacheMissCauseSnapshot {
            hits: self.cache_hits.get(),
            misses: self.cache_misses.get(),
            dependency_root_changed: self.cache_miss_dependency_root.get(),
            witness_unverifiable: self.cache_miss_witness.get(),
            coarse_wipe: self.cache_miss_coarse_wipe.get(),
        }
    }

    /// Record an attributed cache miss (also increments aggregate `cache_misses`).
    pub fn note_cache_miss_cause(&self, cause: CacheMissCause) {
        self.cache_misses
            .set(self.cache_misses.get().saturating_add(1));
        match cause {
            CacheMissCause::DependencyRootChanged => {
                self.cache_miss_dependency_root
                    .set(self.cache_miss_dependency_root.get().saturating_add(1));
            }
            CacheMissCause::WitnessUnverifiable => {
                self.cache_miss_witness
                    .set(self.cache_miss_witness.get().saturating_add(1));
            }
            CacheMissCause::CoarseWipe => {
                self.cache_miss_coarse_wipe
                    .set(self.cache_miss_coarse_wipe.get().saturating_add(1));
            }
        }
    }

    /// Coarse wipe of `n` memo entries (one wipe event may drop many keys).
    pub fn note_coarse_wipe_misses(&self, n: u64) {
        if n == 0 {
            return;
        }
        self.cache_misses
            .set(self.cache_misses.get().saturating_add(n));
        self.cache_miss_coarse_wipe
            .set(self.cache_miss_coarse_wipe.get().saturating_add(n));
    }

    /// fsqlite engine prepared-statement LRU hits/misses (process-global counters).
    /// Distinct from payload-ref cache_hits in [`Self::metric_snapshot`].
    /// Requires fsqlite hot-path profile enabled (see [`ensure_prepared_cache_profile`]).
    pub fn prepared_cache_metric_snapshot(&self) -> PreparedCacheMetrics {
        let _ = self;
        prepared_cache_metrics()
    }

    /// Capture EXPLAIN / EXPLAIN QUERY PLAN for the hot RecoveryStore SQL catalog.
    pub fn capture_sql_explains(&self) -> Vec<SqlExplainCapture> {
        sql_explain::capture_hot_sql_explains(&self.conn)
    }

    /// Env-gated (`FSZERO_SQL_EXPLAIN=1`) plan dump to `out_dir` or default perf path.
    pub fn maybe_write_sql_explain_artifacts(
        &self,
        out_dir: Option<&std::path::Path>,
    ) -> Result<Option<std::path::PathBuf>, String> {
        sql_explain::maybe_capture_sql_explains(&self.conn, out_dir)
    }

    /// Record that the open transaction wrote state a reopen must still see.
    pub(super) fn mark_exec_txn_durable_dirty(&self) {
        self.exec_txn_durable_dirty.set(true);
    }

    /// Test hook for durable-dirty transaction state.
    pub fn exec_txn_durable_dirty_for_test(&self) -> bool {
        self.exec_txn_durable_dirty.get()
    }

    pub fn reset_metrics(&self) {
        self.cache_hits.set(0);
        self.cache_misses.set(0);
        self.store_writes.set(0);
        self.bytes_materialized.set(0);
        self.cache_miss_dependency_root.set(0);
        self.cache_miss_witness.set(0);
        self.cache_miss_coarse_wipe.set(0);
    }

    /// Restore durable integrity evidence before open-time repair adds any
    /// newly observed violations. Torn rows may be removed during repair, so
    /// the report must outlive the handle that detected them.
    fn restore_integrity_report(&self) {
        let Ok(rows) = self.conn.query(SQL_SELECT_INTEGRITY_STATE) else {
            return;
        };
        let Some(row) = rows.first() else {
            return;
        };
        if let Some(count) = int_col_opt(row, 0) {
            self.integrity_violations.set(count.max(0) as u64);
        }
        if let Some(detail) = text_col_opt(row, 1) {
            *self.last_integrity_error.borrow_mut() = Some(detail);
        }
    }

    /// Record an integrity violation loudly: counted, detailed, and surfaced through integrity_report
    /// / expand errors — a damaged record is reported and skipped, never served or silently absorbed.
    fn note_integrity(&self, detail: String) {
        let violations = self.integrity_violations.get().saturating_add(1);
        self.integrity_violations.set(violations);
        *self.last_integrity_error.borrow_mut() = Some(detail.clone());
        if self.db_path.is_some() {
            let _ = self.conn.execute_with_params(
                SQL_INSERT_INTEGRITY_STATE,
                &[
                    sql_int(violations.min(i64::MAX as u64) as i64),
                    sql_text(&detail),
                ],
            );
        }
    }

    /// (violations_seen, last_detail) for doctor/telemetry surfaces.
    pub fn integrity_report(&self) -> (u64, Option<String>) {
        (
            self.integrity_violations.get(),
            self.last_integrity_error.borrow().clone(),
        )
    }

    /// Shared store-health, peer-compatibility, and layout fields for root reports.
    pub fn root_report_store_fragments(
        &self,
        durable_degraded: bool,
        capabilities: &serde_json::Value,
    ) -> (String, serde_json::Value, serde_json::Value, Option<String>) {
        let layout_version = capabilities
            .pointer("/shared_cas/layout_version")
            .map(|value| value.to_string())
            .unwrap_or_else(|| zero_store::CAS_LAYOUT_VERSION.to_string());
        let (integrity_violations, last_integrity_error) = self.integrity_report();
        let store_health = serde_json::json!({
            "durable": self.store_db_path().is_some() && !durable_degraded,
            "cas_attached": self.cas_attached(),
            "cas_writable": self.cas_writable(),
            "integrity_violations": integrity_violations,
        });
        let peer_incompatibility = serde_json::json!({});
        (
            layout_version,
            store_health,
            peer_incompatibility,
            last_integrity_error,
        )
    }

    /// Content-addressed reads verify the complete object digest before the bytes are
    /// served through ZeroRef: a blob whose SHA-256 does not match its key is corruption;
    /// report it and treat as missing so outer tiers (ref-index) can supply a good copy.
    fn verified_blob(&self, key: &str, bytes: Vec<u8>) -> Option<Vec<u8>> {
        let Some(expected) = key.strip_prefix("z://blob/") else {
            return Some(bytes);
        };
        let actual = fszero_core::hexutil::sha256_hex_of(Sha256::digest(&bytes).into());
        if actual == expected {
            return Some(bytes);
        }
        self.note_integrity(format!("corrupt_payload: {key} sha256 mismatch (stored bytes hash {actual}); reported, not served" ));
        None
    }

    pub fn expand(&self, r: &str) -> Option<Vec<u8>> {
        self.expand_with_tiers(r).ok()
    }

    pub fn expand_with_tiers(&self, r: &str) -> Result<Vec<u8>, String> {
        self.pack_integrity_error.set(None);
        if r.starts_with("seq/") {
            return Err(seq_ref_scoped_err(r));
        }

        // Product-scheme refs use one parser and fragment algebra.
        if claims_zeroref(r) {
            return self.expand_zeroref(r).map_err(|e| e.to_string());
        }
        // Engine-local payload keys and view aliases stay in the recovery store.
        if let Some(payload) = self.expand_current_store(r) {
            return Ok(payload);
        }
        if let Some(err) = self.pack_integrity_error.take() {
            return Err(err);
        }
        if let Some(payload) = self.expand_from_ref_index(r) {
            return Ok(payload);
        }
        if let (n, Some(detail)) = self.integrity_report() {
            if n > 0 {
                return Err(format!("ref_unrecoverable: {r} ({detail})"));
            }
        }
        Err(ref_not_found_err(r))
    }

    /// Parse and resolve a full canonical ZeroRef before applying its fragment.
    /// Digest verification covers the complete object, so corruption outside the
    /// selected fragment still fails with `digest_mismatch`.
    pub fn expand_zeroref(&self, r: &str) -> Result<Vec<u8>, ZeroRefError> {
        let parsed = ZeroRef::parse(r)?;
        let canonical = parsed.to_string();
        let key = format!("z://blob/{}", parsed.hash);
        // Canonical CAS corruption is terminal; a clean miss falls through.
        if let Some(bytes) = self.expand_zeroref_from_cas(&parsed, &canonical)? {
            return Ok(bytes);
        }
        let violations_before = self.integrity_violations.get();
        let whole = self
            .expand_current_store(&key)
            .or_else(|| self.expand_from_ref_index(&key));
        let Some(bytes) = whole else {
            return Err(self.missing_zeroref_error(&parsed, &canonical, violations_before));
        };
        // Whole-object digest verification happens inside verify_and_select
        // (and the tiers above). Portable expand never clamps line-span ends
        // (golden `lines_end_past_count`; engine reads keep ClampEnd).
        Ok(parsed
            .verify_and_select_with_policy(&bytes, LineEndPolicy::Strict)?
            .to_vec())
    }

    fn missing_zeroref_error(
        &self,
        parsed: &ZeroRef,
        canonical: &str,
        violations_before: u64,
    ) -> ZeroRefError {
        if self.integrity_violations.get() > violations_before {
            if let Some(detail) = self.last_integrity_error.borrow().clone() {
                if detail.contains(parsed.hash.as_str()) {
                    let class = if detail.starts_with("corrupt_payload") {
                        ZeroRefErrorClass::DigestMismatch
                    } else {
                        ZeroRefErrorClass::Io
                    };
                    return zeroref_unrecoverable(
                        class,
                        format!("{canonical} unrecoverable ({detail})"),
                    );
                }
            }
        }
        let tiers = if self.cas.is_some() {
            "canonical-cas, explicit/env-cache, current-root-store, ref-index"
        } else {
            "explicit/env-cache, current-root-store, ref-index"
        };
        ZeroRefError::new(
            ZeroRefErrorClass::Missing,
            format!("{canonical} (tiers tried: {tiers})"),
        )
    }

    fn expand_zeroref_from_cas(
        &self,
        parsed: &ZeroRef,
        canonical: &str,
    ) -> Result<Option<Vec<u8>>, ZeroRefError> {
        let Some(cas) = &self.cas else {
            return Ok(None);
        };
        match cas.get(&parsed.hash) {
            Ok(bytes) => Ok(Some(
                parsed
                    .verify_and_select_with_policy(&bytes, LineEndPolicy::Strict)?
                    .to_vec(),
            )),
            Err(super::cas::CasError::Missing(_)) | Err(super::cas::CasError::Malformed(_)) => {
                Ok(None)
            }
            Err(error @ super::cas::CasError::Corrupt { .. }) => {
                self.note_integrity(format!("cas_corrupt: {error}"));
                Err(zeroref_unrecoverable(
                    ZeroRefErrorClass::DigestMismatch,
                    format!("ref_unrecoverable: {canonical} (cas_corrupt: {error})"),
                ))
            }
            Err(error @ super::cas::CasError::Io { .. }) => {
                self.note_integrity(format!("cas_io: {error}"));
                Err(zeroref_unrecoverable(
                    ZeroRefErrorClass::Io,
                    format!("ref_unrecoverable: {canonical} (cas_io: {error})"),
                ))
            }
            // Guard/ledger/replication failures are not produced by `get`
            // (they belong to put/GC/repair paths); if one ever surfaces
            // here it is an internal state problem, not a clean miss.
            Err(error @ super::cas::CasError::EvictionRefused { .. })
            | Err(error @ super::cas::CasError::Validity(_))
            | Err(error @ super::cas::CasError::Replication(_)) => {
                self.note_integrity(format!("cas_internal: {error}"));
                Err(zeroref_unrecoverable(
                    ZeroRefErrorClass::Io,
                    format!("ref_unrecoverable: {canonical} (cas_internal: {error})"),
                ))
            }
        }
    }

    fn expand_current_store(&self, key: &str) -> Option<Vec<u8>> {
        if key == "read"
            && let Some(ref_bytes) = self.get_payload("read/ref")
        {
            let content_ref = String::from_utf8_lossy(&ref_bytes);
            if content_ref.starts_with("z://blob/") {
                return self.expand_zeroref(&content_ref).ok();
            }
        }
        if let Some(payload) = self.get_payload(key) {
            return self.verified_blob(key, payload);
        }
        let alias_ref_key = if key == "read" {
            Some("read/ref".to_string())
        } else if let Some(view_id) = key
            .strip_prefix("view_")
            .and_then(|rest| rest.strip_suffix("/bytes"))
        {
            Some(format!("view_{view_id}/ref"))
        } else {
            key.strip_prefix('r')
                .and_then(|rest| rest.strip_suffix("/bytes"))
                .map(|view_id| format!("r{view_id}/ref"))
        };
        if let Some(ref_key) = alias_ref_key
            && let Some(ref_bytes) = self.get_payload(&ref_key)
        {
            let content_ref = String::from_utf8_lossy(&ref_bytes);
            if content_ref.starts_with("z://blob/") {
                return self.expand_zeroref(&content_ref).ok();
            }
        }
        None
    }

    fn expand_from_ref_index(&self, key: &str) -> Option<Vec<u8>> {
        // Only canonical durable refs enter the cross-root index.
        if self.ref_index_disabled || !ref_index_enabled() || !ref_indexable(key) {
            return None;
        }
        if let Some(shard) = ref_index_shard_path(key) {
            let (_, damaged) = read_ref_index_entries_reporting(&shard);
            if damaged > 0 {
                self.note_integrity(format!(
                    "ref_index_damaged: {damaged} unparseable line(s) in {} (valid lines still served)",
                    shard.display()
                ));
            }
        }
        for _ in 0..2 {
            let entry = lookup_ref_index_entry(key)?;
            if !entry.store_path.is_file() {
                prune_missing_ref_index_entries_for(key);
                continue;
            }
            // Light open only — never full-table repair/backfill on the expand
            // miss path. Verify cross-store bytes before serving.
            let remote =
                match Self::try_open_existing_durable_with_options(&entry.store_path, false) {
                    Ok(remote) => remote,
                    Err(_) => {
                        prune_missing_ref_index_entries_for(key);
                        continue;
                    }
                };
            let bytes = remote.expand_current_store(key)?;
            return self.verified_blob(key, bytes);
        }
        None
    }

    pub fn list_keys(&self) -> Vec<String> {
        let mut out = query_text0(&self.conn, SQL_SELECT_PAYLOAD_KEYS);
        out.sort_by_key(|key| recovery_key_priority(key));
        out
    }

    pub fn try_delete_key(&mut self, key: &str) -> Result<(), String> {
        self.delete_payload_and_lru(key)
    }

    /// Delete a payload row and its LRU tick entry.
    pub fn delete_payload_and_lru(&mut self, key: &str) -> Result<(), String> {
        let p = [sql_text(key)];
        self.exec_params_ctx(
            SQL_DELETE_PAYLOAD_KEY,
            &p,
            format!("delete failed for {key}"),
        )?;
        let _ = self.exec_params(SQL_DELETE_PAYLOAD_LRU, &p);
        self.clear_payload_open_maintenance(key);
        Ok(())
    }

    pub fn upsert_memory_path(
        &mut self,
        path: &str,
        store_key: &str,
        content_ref: &str,
    ) -> Result<(), String> {
        let ts = unix_epoch_secs();
        self.exec_params_ctx(
            SQL_INSERT_MEMORY_PATHS_REPLACE,
            &[
                sql_text(path),
                sql_text(store_key),
                sql_text(content_ref),
                sql_int(ts),
            ],
            "memory_paths upsert failed",
        )
    }

    pub fn delete_memory_path(&mut self, path: &str) -> Result<(), String> {
        self.exec_params_ctx(
            SQL_DELETE_MEMORY_PATHS_BY_PATH,
            &[sql_text(path)],
            "memory_paths delete failed",
        )
    }

    pub fn memory_path_exists(&self, path: &str) -> bool {
        self.conn
            .query_with_params(SQL_SELECT_MEMORY_PATH_EXISTS, &[sql_text(path)])
            .ok()
            .map(|rows| !rows.is_empty())
            .unwrap_or(false)
    }

    /// Paths under `prefix` (exact match or `prefix/` children). Empty prefix = all.
    pub fn list_memory_paths(&self, prefix: &str) -> Vec<String> {
        if prefix.is_empty() {
            return query_text0(&self.conn, SQL_SELECT_MEMORY_PATHS_ORDERED);
        }
        // Escape LIKE meta-chars so `_` / `%` in the prefix are literal.
        let like = format!("{}/%", super::path::escape_like_pattern(prefix));
        query_text0_params(
            &self.conn,
            SQL_SELECT_MEMORY_PATHS_PREFIX,
            &[sql_text(prefix), sql_text(&like)],
        )
    }
}

impl RecoveryStore {
    pub fn put_fact(
        &mut self,
        subject_ref: &str,
        predicate: &str,
        object_ref: &str,
        evidence_ref: &str,
        version: u64,
        agent: &str,
    ) {
        let _ = self.exec_params(
            SQL_INSERT_FACT,
            &[
                sql_text(subject_ref),
                sql_text(predicate),
                sql_text(object_ref),
                sql_text(evidence_ref),
                sql_int(version as i64),
                sql_text(agent),
            ],
        );
    }

    pub fn facts_for(&self, subject_ref: &str) -> Vec<String> {
        let Ok(rows) = self
            .conn
            .query_with_params(SQL_SELECT_FACTS_BY_SUBJECT, &[sql_text(subject_ref)])
        else {
            return Vec::new();
        };
        rows.into_iter()
            .map(|row| {
                format!(
                    "{} {} {} evidence={} v={} agent={}",
                    subject_ref,
                    text_col(&row, 0),
                    text_col(&row, 1),
                    text_col(&row, 2),
                    int_col(&row, 3),
                    text_col(&row, 4)
                )
            })
            .collect()
    }

    /// Symbol rows matching `pat` (LIKE-escaped, wrapped in `%…%`).
    pub fn query_symbols_like(
        &self,
        pat: &str,
        version: i64,
    ) -> Vec<(String, String, String, i64, i64)> {
        let escaped = super::path::escape_like_pattern(pat);
        self.ast
            .query_symbols_like(&format!("%{escaped}%"), version)
    }

    /// Function/method spans matching `pat` (drops kind from symbols_like).
    pub fn query_fns_like(&self, pat: &str, version: i64) -> Vec<(String, String, i64, i64)> {
        self.query_symbols_like(pat, version)
            .into_iter()
            .map(|(fk, sym, _kind, start, end)| (fk, sym, start, end))
            .collect()
    }

    /// AST node count (integration / doctor telemetry).
    pub fn ast_node_count(&self) -> i64 {
        self.ast.node_count()
    }

    /// Function span at any generation (latest wins).
    pub fn fn_span_any(&self, symbol: &str) -> Option<(String, i64, i64)> {
        self.ast.fn_span_any(symbol)
    }

    /// Function span at a specific generation.
    pub fn fn_span(&self, symbol: &str, version: i64) -> Option<(String, i64, i64)> {
        self.ast.fn_span(symbol, version)
    }

    /// Group many small writes into one transaction. fsqlite autocommits per statement otherwise, which
    /// turns index persistence into thousands of individual commits — 41% of a fresh-index run (16.7s
    /// on a 61-file corpus).
    pub fn begin_batch(&mut self) -> bool {
        self.begin_txn_core(true)
    }

    /// Benchmark-only bulk setup hook. Not part of shipped minimal-feature surfaces.
    #[cfg(feature = "dev-harness")]
    pub fn begin_benchmark_batch(&mut self) -> bool {
        self.begin_batch()
    }

    /// Finish a batch started by `begin_benchmark_batch`.
    #[cfg(feature = "dev-harness")]
    pub fn end_benchmark_batch(&mut self, began: bool) {
        self.end_batch(began);
    }

    /// Rows inspected by open maintenance for deterministic benchmark evidence.
    #[cfg(feature = "dev-harness")]
    pub fn open_maintenance_rows_scanned(&self) -> (usize, usize) {
        (self.open_pack_rows_scanned, self.open_memory_rows_scanned)
    }

    /// Shared BEGIN IMMEDIATE + pending overlay; optional bounded touched-key cache.
    fn begin_txn_core(&mut self, cache_keys: bool) -> bool {
        let _ = self.conn.execute("PRAGMA wal_autocheckpoint=0");
        self.ast.begin_bulk();
        let began = self.conn.execute("BEGIN IMMEDIATE").is_ok();
        if began {
            self.pending_payloads = Some(BTreeMap::new());
            if cache_keys {
                self.payload_key_cache = Some(HashSet::new());
            }
        }
        began
    }

    /// Run `PRAGMA wal_checkpoint(TRUNCATE)`, record wall_us + pages, optional emit.
    fn run_wal_checkpoint_truncate(&mut self, site: &'static str) {
        let t0 = std::time::Instant::now();
        // Prefer query so busy/log/checkpointed columns are available when the
        // engine returns them; fall back to execute if query is unsupported.
        let rows = self.conn.query(SQL_PRAGMA_WAL_CHECKPOINT_TRUNCATE);
        let us = t0.elapsed().as_micros() as u64;
        let (log_pages, checkpointed) = match rows {
            Ok(ref r) if !r.is_empty() => (int_col_opt(&r[0], 1), int_col_opt(&r[0], 2)),
            Ok(_) => (None, None),
            Err(_) => {
                let _ = self.conn.execute(SQL_PRAGMA_WAL_CHECKPOINT_TRUNCATE);
                (None, None)
            }
        };
        self.last_wal_checkpoint_us = Some(us);
        self.last_wal_checkpoint_log = log_pages;
        self.last_wal_checkpoint_checkpointed = checkpointed;
        if !wal_checkpoint_profile_enabled() {
            return;
        }
        let doc = serde_json::json!({
            "event": "wal_checkpoint_profile",
            "site": site,
            "mode": "TRUNCATE",
            "wall_us": us,
            "log_pages": log_pages,
            "checkpointed_pages": checkpointed,
        });
        eprintln!("{doc}");
    }

    pub fn last_wal_checkpoint_us(&self) -> Option<u64> {
        self.last_wal_checkpoint_us
    }

    pub fn end_batch(&mut self, began: bool) {
        // Nested index builds borrow the outer transaction. A false begin with
        // a live overlay leaves flush and commit to the outer owner.
        if !began && self.pending_payloads.is_some() {
            return;
        }
        self.ast.end_bulk();
        let flush_error = self.flush_pending_payloads().err();
        self.pending_payloads = None;
        self.pending_bytes = 0;
        self.payload_key_cache = None;
        if began {
            if let Some(e) = flush_error {
                let _ = self.conn.execute("ROLLBACK");
                eprintln!("fszero: bulk batch flush failed: {e}");
                self.last_store_error = Some(e.clone());
                self.note_integrity(format!("batch_flush_failed: {e}"));
            } else if let Err(e) = self.conn.execute("COMMIT") {
                // A silently-failed bulk COMMIT is the prime suspect for cross-process losers re-running cold builds
                // the whole batch — index rows, manifest — evaporates with no trace. Make it loud on both channels.
                let _ = self.conn.execute("ROLLBACK");
                eprintln!("fszero: bulk batch COMMIT failed: {e}");
                self.last_store_error = Some(format!("batch commit failed: {e}"));
                self.note_integrity(format!("batch_commit_failed: {e}"));
            } else {
                self.note_durable_mutation();
            }
        } else if let Some(e) = flush_error {
            self.last_store_error = Some(e);
        }
        self.run_wal_checkpoint_truncate("end_batch");
        let _ = self.conn.execute("PRAGMA wal_autocheckpoint=1000");
        // Opportunistic pack GC: batch boundaries are the rare,
        // already-heavy moments; reclaim when over half the pack is dead and
        // the file is big enough to matter.
        let (live, pack_len) = self.pack_report();
        if pack_len > 4 * 1024 * 1024 && live * 2 < pack_len {
            let _ = self.compact_pack();
        }
        if let Err(error) = self.run_cas_gc_if_due() {
            eprintln!("fszero: CAS GC skipped: {error}");
        }
    }

    /// Initialize a FULL-synchronous transaction for a group of related writes.
    fn initialize_exec_txn_state(&mut self) {
        self.exec_txn_durable_dirty.set(false);
        self.exec_txn_active.set(true);
    }

    pub fn begin_exec_txn(&mut self) -> bool {
        if self.pending_payloads.is_some() {
            return false;
        }
        // SQLite rejects synchronous changes inside a transaction. Establish
        // FULL before BEGIN so mutation and fallback paths are self-contained.
        if !self.set_synchronous("FULL") {
            self.last_store_error =
                Some("failed to establish synchronous=FULL before execution transaction".into());
            return false;
        }
        let began = self.begin_txn_core(false);
        if began {
            self.initialize_exec_txn_state();
        }
        began
    }

    fn clear_exec_txn_state(&mut self) {
        self.exec_txn_active.set(false);
        self.exec_txn_durable_dirty.set(true);
    }

    pub fn rollback_exec_txn(&mut self, began: bool) {
        if !began || !self.exec_txn_active.get() {
            return;
        }
        let _ = self.conn.execute("ROLLBACK");
        self.pending_payloads = None;
        self.pending_bytes = 0;
        self.clear_exec_txn_state();
    }

    pub fn commit_exec_txn(&mut self, began: bool) {
        if !began {
            return;
        }
        if let Err(error) = self.flush_pending_payloads() {
            self.last_store_error = Some(error);
            let _ = self.conn.execute("ROLLBACK");
            self.pending_payloads = None;
            self.pending_bytes = 0;
            self.clear_exec_txn_state();
            return;
        }
        self.pending_payloads = None;
        self.pending_bytes = 0;
        let durable = self.exec_txn_durable_dirty.get();
        if let Err(error) = self.conn.execute("COMMIT") {
            let _ = self.conn.execute("ROLLBACK");
            self.last_store_error = Some(format!("exec txn commit failed: {error}"));
        } else if durable {
            self.note_durable_mutation();
        }
        self.clear_exec_txn_state();
    }
}
