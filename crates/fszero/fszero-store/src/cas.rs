//! Canonical shared content-addressed store.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Directory under the store root that holds the canonical CAS.
/// Bound to hub `zero_store::BLOBS_DIR` so FSZero never forks the layout name.
pub const CAS_DIR_NAME: &str = zero_store::BLOBS_DIR;
const ALGO_DIR: &str = "sha256";
/// Layout contract version for capability negotiation (canonical ADR §10) peers
/// whose layout version differs would publish objects at different paths and must
/// refuse interop before payload work. Matches hub `zero_store::CAS_LAYOUT_VERSION`.
pub const CAS_LAYOUT_VERSION: u64 = zero_store::CAS_LAYOUT_VERSION;
/// Maximum object size enforced by the canonical CAS.
pub const CAS_MAX_OBJECT_BYTES: u64 = zero_store::CAS_MAX_OBJECT_BYTES;

const DEFAULT_GC_GRACE_SECS: u64 = 7 * 24 * 60 * 60;

/// Frozen TokenZero coordinator schema id (zerostack.cas-gc.legacy).
pub const GC_SCHEMA_VERSION: &str = "zerostack.cas-gc.legacy";
/// Reachability snapshot record type per shared-CAS GC schema.
pub const GC_RECORD_TYPE_REACHABILITY: &str = "reachability-snapshot";
/// FSZero engine namespace under `gc/roots/<engine>/…`.
pub const GC_ENGINE_FSZERO: &str = "fszero";

/// Human/machine-readable layout template, composed from the SAME directory constants
/// [`CasStore::object_path`] uses, so the advertised layout and the real on-disk layout cannot drift.
pub fn cas_layout() -> String {
    // Interop contract is the hub layout string (blobs/sha256/<hh>/<hash>).
    zero_store::CAS_LAYOUT.to_string()
}

/// Typed CAS failures. Corruption is a distinct class from I/O and from a
/// clean miss — callers must keep them distinguishable end to end.
#[derive(Debug)]
pub enum CasError {
    /// Input hash is not exactly 64 lowercase hex characters.
    Malformed(String),
    /// Object not present in this CAS root (clean miss).
    Missing(String),
    /// Filesystem failure (permissions, transient I/O). Never a verdict
    /// about the bytes themselves.
    Io {
        hash: String,
        context: String,
        source: std::io::Error,
    },
    /// Bytes on disk do not match the content address. The object was NOT
    /// served (get) / NOT overwritten (put).
    Corrupt { hash: String, detail: String },
    /// Eviction refused by the 99% slack guard: deleting this
    /// weight would drop retained resident mass below 99% of demanded mass.
    /// Refusal happens BEFORE any state change (zero side effects).
    EvictionRefused {
        resident_mass: u64,
        demanded_mass: u64,
        evict_weight: u64,
        slack_ppm: i64,
    },
    /// Validity-ledger failure: the logical record could not
    /// be written/read. Eviction never proceeds without a validity record.
    Validity(String),
    /// Replication failure: a declared replica could not
    /// receive the blob before eviction; the local copy is retained.
    Replication(String),
}

impl fmt::Display for CasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CasError::Malformed(h) => {
                write!(f, "malformed hash (need exactly 64 lowercase hex): {h}")
            }
            CasError::Missing(h) => write!(f, "object missing: sha256/{h}"),
            CasError::Io {
                hash,
                context,
                source,
            } => write!(f, "io failure ({context}) for sha256/{hash}: {source}"),
            CasError::Corrupt { hash, detail } => {
                write!(f, "corrupt object sha256/{hash}: {detail}")
            }
            CasError::EvictionRefused {
                resident_mass,
                demanded_mass,
                evict_weight,
                slack_ppm,
            } => write!(
                f,
                "cas eviction refused: retained mass {resident_mass} - {evict_weight} would fall below the 99% floor of demanded {demanded_mass} (slack {slack_ppm}ppm)"
            ),
            CasError::Validity(msg) => write!(f, "validity ledger: {msg}"),
            CasError::Replication(msg) => write!(f, "replication: {msg}"),
        }
    }
}

impl std::error::Error for CasError {}

/// Result of a successful `put`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CasPutOutcome {
    pub hash: String,
    pub created: bool,
}

/// One GC run's honest accounting: what went cold, what was refused, what
/// was replicated, and the mass numbers behind the slack decision.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CasGcReport {
    pub scanned: u64,
    /// Blobs whose bytes were deleted (each was marked L3-cold first).
    pub removed: u64,
    /// Blobs marked L3-cold this run (validity record retained).
    pub went_cold: u64,
    pub retained_marked: u64,
    pub retained_young: u64,
    /// Evictions refused by the slack guard; zero side effects each.
    pub slack_refused: u64,
    /// Blobs retained because a declared replica could not receive them
    /// (replicate-before-evict failed; local copy kept).
    pub replica_errors: u64,
    /// Blobs whose bytes were newly published to at least one replica
    /// before eviction.
    pub replicated: u64,
    /// Blobs retained because their L3-cold validity record could not be
    /// written (never evict without a record).
    pub coldmark_failed: u64,
    /// Blobs whose file could not be removed after marking cold.
    pub delete_failed: u64,
    /// Stale reachability-root snapshots (legacy roots) removed.
    pub legacy_roots_removed: u64,
    /// Reachability-root snapshots retained (live or too young or unprovable).
    pub legacy_roots_kept: u64,
    /// Total on-disk blob bytes at scan time.
    pub resident_mass: u64,
    /// Demanded mass the slack floor is computed from (live-set bytes;
    /// ledger sizes for live hashes whose bytes are no longer resident).
    pub demanded_mass: u64,
    /// `sigma = W_R - 0.99W` in PPM of demanded mass (negative when the
    /// store is already below its floor).
    pub slack_ppm: i64,
}

/// Refuse eviction when retained resident mass would fall below 99% of demand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvictionSlackGuard {
    resident_mass: u64,
    demanded_mass: u64,
}

/// PPM helper: `numerator / denominator * 1_000_000`, saturating (mirrors
/// hub `residency::ppm_of`).
fn ppm_of(numerator: u64, denominator: u64) -> u64 {
    if denominator == 0 {
        return u64::MAX;
    }
    let scaled = numerator.saturating_mul(1_000_000);
    scaled / denominator
}

impl EvictionSlackGuard {
    pub fn new(resident_mass: u64, demanded_mass: u64) -> Self {
        Self {
            resident_mass,
            demanded_mass,
        }
    }

    pub fn resident_mass(&self) -> u64 {
        self.resident_mass
    }

    pub fn demanded_mass(&self) -> u64 {
        self.demanded_mass
    }

    /// `sigma = W_R - 0.99W` in PPM of demanded mass (can be negative).
    pub fn slack_ppm(&self) -> i64 {
        if self.demanded_mass == 0 {
            return 0;
        }
        let floor = ppm_of(self.demanded_mass * 99 / 100, self.demanded_mass);
        let resident_ppm = ppm_of(self.resident_mass, self.demanded_mass);
        resident_ppm as i64 - floor as i64
    }

    /// Guard one eviction decision: evicting `evict_weight` must keep
    /// retained resident mass at or above 99% of demanded mass. Refusal is
    /// fail-loud with zero side effects (no state was touched).
    pub fn guard_eviction(&self, evict_weight: u64) -> Result<(), CasError> {
        let floor = self.demanded_mass * 99 / 100;
        let after = self.resident_mass.saturating_sub(evict_weight);
        if after < floor {
            return Err(CasError::EvictionRefused {
                resident_mass: self.resident_mass,
                demanded_mass: self.demanded_mass,
                evict_weight,
                slack_ppm: self.slack_ppm(),
            });
        }
        Ok(())
    }
}

/// Handle to one canonical CAS root. Plain value, no globals; cheap to
/// construct, safe to share across threads (`&self` API only).
pub struct CasStore {
    hub: zero_store::SharedCas,
    blobs_root: PathBuf,
    temp_seq: AtomicU64,
}

fn map_hub_err(hash: &str, err: zero_store::CasError) -> CasError {
    match err {
        zero_store::CasError::NotFound => CasError::Missing(hash.to_string()),
        zero_store::CasError::Io(message) => {
            io_err(hash, message.clone(), std::io::Error::other(message))
        }
        zero_store::CasError::DigestMismatch { actual, .. } => corrupt(
            hash,
            format!("stored bytes hash {actual}; reported, not served"),
        ),
        zero_store::CasError::Malformed(message) => CasError::Malformed(message),
        zero_store::CasError::PolicyDenied(message) => {
            io_err(hash, message.clone(), std::io::Error::other(message))
        }
    }
}

pub fn is_full_lower_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Full-hash `z://blob/<64-hex>` key → hash slice (shared recovery/CAS root filter).
#[inline]
pub fn full_blob_hash(key: &str) -> Option<&str> {
    key.strip_prefix("z://blob/")
        .filter(|h| is_full_lower_hex(h))
}

#[inline]
fn corrupt(hash: &str, detail: String) -> CasError {
    CasError::Corrupt {
        hash: hash.to_string(),
        detail,
    }
}

#[inline]
fn io_err(hash: &str, context: impl Into<String>, source: std::io::Error) -> CasError {
    CasError::Io {
        hash: hash.to_string(),
        context: context.into(),
        source,
    }
}

/// Env-gated CAS put/get phase timing. When `FSZERO_CAS_PHASES=1`,
/// emit one JSON line on stderr with microsecond stages and byte counts.
fn cas_phases_enabled() -> bool {
    match std::env::var("FSZERO_CAS_PHASES") {
        Ok(v) => matches!(v.as_str(), "1" | "true" | "TRUE" | "on" | "yes"),
        Err(_) => false,
    }
}

fn emit_cas_phases(fields: serde_json::Value) {
    if !cas_phases_enabled() {
        return;
    }
    eprintln!("{fields}");
}

impl CasStore {
    /// CAS rooted at an explicit `blobs/` directory.
    pub fn at_blobs_root(blobs_root: impl Into<PathBuf>) -> Self {
        let blobs_root = blobs_root.into();
        let store_root = blobs_root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| blobs_root.clone());
        Self {
            hub: zero_store::SharedCas::open(store_root),
            blobs_root,
            temp_seq: AtomicU64::new(0),
        }
    }

    /// CAS for a ZeroStack store root (`<store_root>/blobs`).
    pub fn for_store_root(store_root: &Path) -> Self {
        Self::at_blobs_root(store_root.join(CAS_DIR_NAME))
    }

    /// Activates the canonical CAS only when `blobs/` already exists under the store root.
    /// FSZero never creates the opt-in directory implicitly.
    pub fn detect(store_root: &Path) -> Option<Self> {
        let blobs = store_root.join(CAS_DIR_NAME);
        if blobs.is_dir() {
            Some(Self::at_blobs_root(blobs))
        } else {
            None
        }
    }

    pub fn blobs_root(&self) -> &Path {
        &self.blobs_root
    }

    /// Parent of the blobs directory and root of the ZeroStack store.
    pub fn store_root(&self) -> &Path {
        self.hub.store_root()
    }

    /// Return whether this process can create a temporary file in the blobs directory.
    pub fn probe_writable(&self) -> bool {
        if !self.blobs_root.is_dir() {
            return false;
        }
        let seq = self.temp_seq.fetch_add(1, Ordering::Relaxed);
        let path = self
            .blobs_root
            .join(format!(".cap-probe.{}.{seq}.tmp", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(f) => {
                drop(f);
                let _ = std::fs::remove_file(&path);
                true
            }
            Err(_) => false,
        }
    }

    /// Canonical object path for a validated hash.
    pub fn object_path(&self, hash: &str) -> Result<PathBuf, CasError> {
        if !is_full_lower_hex(hash) {
            return Err(CasError::Malformed(hash.to_string()));
        }
        Ok(self.hub.object_path(hash))
    }

    /// Store `bytes`, returning the content hash. Delegates publish to hub
    /// [`zero_store::SharedCas`]. Local GC still holds `blobs.gc.lock`.
    pub fn put(&self, bytes: &[u8]) -> Result<CasPutOutcome, CasError> {
        let hash = super::access_log::content_hash_bytes(bytes);
        self.put_prehashed(&hash, bytes)
    }

    /// Store bytes with a caller-computed hash while avoiding duplicate mint-path hashing.
    /// Always verifies the supplied digest before publication.
    pub fn put_prehashed(&self, hash: &str, bytes: &[u8]) -> Result<CasPutOutcome, CasError> {
        // Never trust the label: verify before lock, hub publish, or ledger.
        // A success-path remap would let a hub regression store bytes under
        // the true digest while publishing validity under the caller hash.
        let actual = super::access_log::content_hash_bytes(bytes);
        if hash != actual.as_str() {
            return Err(CasError::Malformed(format!(
                "put_prehashed: caller hash {hash} != sha256(bytes)"
            )));
        }
        let phases_on = cas_phases_enabled();
        let t_total = std::time::Instant::now();
        let _gc_guard = self.gc_publish_guard();
        let t_hub = std::time::Instant::now();
        let outcome = self
            .hub
            .put_prehashed(hash, bytes)
            .map_err(|err| map_hub_err(hash, err))?;
        // Layer validity: a put of identical bytes for a previously
        // evicted blob is a refetch that RESTORES L3 with the same identity -- never
        // rediscovery. A new blob publishes an L2-valid record. Digest equality was checked above.
        self.validity_ledger()
            .publish(hash, bytes.len() as u64)
            .map_err(|e| CasError::Validity(format!("publish {}: {e}", hash)))?;
        let hub_us = t_hub.elapsed().as_micros() as u64;
        if phases_on {
            emit_cas_phases(serde_json::json!({
                "cas_phases_us": {
                    "hub_put": hub_us,
                    "write": hub_us,
                    "full_sync": 0,
                    "rename": 0,
                },
                "bytes": bytes.len(),
                "created": outcome.created,
                "total_us": t_total.elapsed().as_micros() as u64,
            }));
        }
        Ok(CasPutOutcome {
            hash: outcome.hash,
            created: outcome.created,
        })
    }

    /// Read a whole object by exact full 64-lowercase-hex hash. The complete
    /// digest (which covers the full length) is verified before any byte is
    /// returned; damage is typed [`CasError::Corrupt`] and never served.
    pub fn get(&self, hash: &str) -> Result<Vec<u8>, CasError> {
        let phases_on = cas_phases_enabled();
        let t_total = std::time::Instant::now();
        if !is_full_lower_hex(hash) {
            return Err(CasError::Malformed(hash.to_string()));
        }
        let t_read = std::time::Instant::now();
        let bytes = self
            .hub
            .get_verified(hash)
            .map_err(|err| map_hub_err(hash, err))?;
        let read_us = t_read.elapsed().as_micros() as u64;
        let _ = self.hub.touch(hash);
        if phases_on {
            emit_cas_phases(serde_json::json!({
                "cas_phases_us": {
                    "read": read_us,
                    "verify": read_us,
                    "hub_get": read_us,
                },
                "bytes": bytes.len(),
                "op": "get",
                "total_us": t_total.elapsed().as_micros() as u64,
            }));
        }
        Ok(bytes)
    }

    /// `true` when an object file exists at the canonical path (no digest
    /// verification — use `get` for verified bytes).
    pub fn contains(&self, hash: &str) -> bool {
        self.hub.contains(hash)
    }

    /// Visit each published object entry under `blobs/sha256/*/*` (exact 64-hex names).
    fn for_each_object_entry(&self, mut f: impl FnMut(std::fs::DirEntry, &str)) {
        let Ok(shards) = std::fs::read_dir(self.blobs_root.join(ALGO_DIR)) else {
            return;
        };
        for shard in shards.flatten() {
            let Ok(entries) = std::fs::read_dir(shard.path()) else {
                continue;
            };
            for entry in entries.flatten() {
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if is_full_lower_hex(&name) {
                    f(entry, &name);
                }
            }
        }
    }

    /// Count published objects (temp files excluded). Walks the tree; meant
    /// for tests/benchmarks, not hot paths.
    pub fn object_count(&self) -> u64 {
        let mut n = 0u64;
        self.for_each_object_entry(|_, _| n += 1);
        n
    }

    /// Count inert orphaned `*.tmp` files; `get` never opens them.
    pub fn tmp_object_count(&self) -> u64 {
        let mut n = 0u64;
        let Ok(shards) = std::fs::read_dir(self.blobs_root.join(ALGO_DIR)) else {
            return 0;
        };
        for shard in shards.flatten() {
            let Ok(entries) = std::fs::read_dir(shard.path()) else {
                continue;
            };
            n += entries
                .flatten()
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.ends_with(".tmp"))
                })
                .count() as u64;
        }
        n
    }

    fn gc_lock_path(&self) -> PathBuf {
        self.blobs_root.with_extension("gc.lock")
    }

    /// Shared GC lock held through publication so concurrent [`Cas::gc`] cannot unlink
    /// the object being published.
    fn gc_publish_guard(&self) -> Option<std::fs::File> {
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.gc_lock_path())
            .ok()?;
        lock.lock_shared().ok()?;
        Some(lock)
    }

    /// Mark-and-sweep published objects. TokenZero shares this layout, so an object is removed only
    /// when FSZero has no reference or pin AND its mtime exceeds the grace window. FSZero refreshes
    /// mtime on read/republish; the grace interval is the conservative cross-engine safety boundary.
    pub fn gc(
        &self,
        marked: &HashSet<String>,
        grace: std::time::Duration,
    ) -> Result<CasGcReport, String> {
        self.gc_with_demand(marked, grace, None)
    }

    /// [`CasStore::gc`] with an explicit demanded-mass floor override. `None` derives
    /// demanded mass from the live set, using disk sizes for resident objects and ledger
    /// sizes for live objects whose bytes are already absent.
    pub fn gc_with_demand(
        &self,
        marked: &HashSet<String>,
        grace: std::time::Duration,
        demanded_mass: Option<u64>,
    ) -> Result<CasGcReport, String> {
        let lock_path = self.gc_lock_path();
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&lock_path)
            .map_err(|e| format!("open CAS GC lock {}: {e}", lock_path.display()))?;
        lock.try_lock()
            .map_err(|e| format!("cas_gc_busy: {}: {e}", lock_path.display()))?;

        let mut live = marked.clone();
        let store_root = self.store_root();
        for pins in [self.blobs_root.join("pins"), store_root.join("gc/pins")] {
            if pins.exists() {
                collect_pin_hashes(&pins, &mut live)?;
            }
        }
        // Cross-engine liveness: gc/roots (reachability snapshots) and
        // gc/leases (time-bounded protection) published by TokenZero/GraphZero.
        collect_gc_roots_hashes(store_root, &mut live);
        collect_gc_leases_hashes(store_root, &mut live);

        let mut report = CasGcReport::default();
        let now = std::time::SystemTime::now();

        // Mass accounting: resident = on-disk blob bytes at scan time;
        // demanded = live-set bytes (ledger sizes for live hashes whose
        // bytes are already gone, so a degraded store refuses evictions).
        let resident_mass = self.measure_resident_mass();
        let demanded = demanded_mass.unwrap_or_else(|| self.measure_demanded_mass(&live));
        let guard = EvictionSlackGuard::new(resident_mass, demanded);
        report.resident_mass = resident_mass;
        report.demanded_mass = demanded;
        report.slack_ppm = guard.slack_ppm();

        let replication = self
            .replication_config()
            .map_err(|e| format!("replication config: {e}"))?;
        let ledger = self.validity_ledger();
        let now_unix = unix_secs_now();
        let mut pending_evict_mass: u64 = 0;

        self.for_each_object_entry(|entry, hash| {
            report.scanned += 1;
            if live.contains(hash) {
                report.retained_marked += 1;
                return;
            }
            let old_enough = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|mtime| now.duration_since(mtime).ok())
                .is_some_and(|age| age >= grace);
            if !old_enough {
                report.retained_young += 1;
                return;
            }
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            // Slack guard first: a refusal is fail-loud with zero side effects -- the blob
            // is untouched (no replica copy, no cold mark, no delete). Cumulative across
            // the run so two individually legal evictions cannot jointly breach the floor.
            if let Err(e) = guard.guard_eviction(pending_evict_mass.saturating_add(size)) {
                report.slack_refused += 1;
                let _ = e; // typed refusal; counted in the report
                return;
            }
            // Replicate-before-evict: a declared replica that cannot receive
            // the bytes keeps the local copy (fail-loud, zero side effects).
            if replication.is_declared() {
                let bytes = match std::fs::read(entry.path()) {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        report.replica_errors += 1;
                        return;
                    }
                };
                match crate::replication::replicate_before_evict(
                    store_root,
                    &replication,
                    hash,
                    &bytes,
                ) {
                    Ok(written) => {
                        if written > 0 {
                            report.replicated += 1;
                        }
                    }
                    Err(e) => {
                        report.replica_errors += 1;
                        let _ = e;
                        return;
                    }
                }
            }
            // Mark L3-cold BEFORE deleting: never destroy bytes without a
            // validity record; never destroy the logical record.
            if let Err(e) = ledger.mark_l3_cold(hash, size, now_unix) {
                report.coldmark_failed += 1;
                let _ = e;
                return;
            }
            if std::fs::remove_file(entry.path()).is_ok() {
                report.removed += 1;
                report.went_cold += 1;
                pending_evict_mass = pending_evict_mass.saturating_add(size);
            } else {
                report.delete_failed += 1;
            }
        });

        // Sweep stale legacy reachability snapshots. Remove only root records,
        // never validity records or blob bytes.
        self.gc_legacy_roots_inner(grace, &mut report);
        Ok(report)
    }

    /// Sum of on-disk blob bytes at scan time (resident mass).
    fn measure_resident_mass(&self) -> u64 {
        let mut total = 0u64;
        self.for_each_object_entry(|entry, _| {
            if let Ok(meta) = entry.metadata() {
                total = total.saturating_add(meta.len());
            }
        });
        total
    }

    /// Sum of live-set bytes: on-disk size when resident; validity-ledger
    /// size when the bytes are already gone; 0 when unknown.
    fn measure_demanded_mass(&self, live: &HashSet<String>) -> u64 {
        let ledger = self.validity_ledger();
        let mut total = 0u64;
        for hash in live {
            if !is_full_lower_hex(hash) {
                continue;
            }
            if let Ok(meta) = std::fs::metadata(self.hub.object_path(hash)) {
                total = total.saturating_add(meta.len());
                continue;
            }
            if let Ok(Some(record)) = ledger.load(hash) {
                total = total.saturating_add(record.size);
            }
        }
        total
    }

    /// Sweep stale legacy reachability roots under
    /// `<store_root>/gc/roots/<engine>/<project>/`.
    pub fn gc_legacy_roots(&self, grace: std::time::Duration) -> Result<CasGcReport, String> {
        let mut report = CasGcReport::default();
        self.gc_legacy_roots_inner(grace, &mut report);
        Ok(report)
    }

    fn gc_legacy_roots_inner(&self, grace: std::time::Duration, report: &mut CasGcReport) {
        let roots_dir = self.store_root().join(zero_store::GC_DIR).join("roots");
        let Ok(engine_dirs) = std::fs::read_dir(&roots_dir) else {
            return;
        };
        let now = std::time::SystemTime::now();
        for engine_dir in engine_dirs.flatten() {
            let Ok(project_dirs) = std::fs::read_dir(engine_dir.path()) else {
                continue;
            };
            for project_dir in project_dirs.flatten() {
                let project_dir = project_dir.path();
                let current = project_dir.join("current.json");
                let old_enough = project_dir
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|mtime| now.duration_since(mtime).ok())
                    .is_some_and(|age| age >= grace);
                if !old_enough || !self.legacy_root_is_fully_absent(&current) {
                    report.legacy_roots_kept += 1;
                    continue;
                }
                if std::fs::remove_dir_all(&project_dir).is_ok() {
                    report.legacy_roots_removed += 1;
                } else {
                    report.legacy_roots_kept += 1;
                }
            }
        }
    }

    /// True when `current.json` is a readable snapshot whose every listed blob hash is
    /// absent from this CAS and whose list is non-empty. Missing/unparseable files and
    /// empty lists are never "fully absent" because uncertain legacy state is retained.
    fn legacy_root_is_fully_absent(&self, current: &Path) -> bool {
        let Ok(text) = std::fs::read_to_string(current) else {
            return false;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return false;
        };
        let Some(arr) = v.get("blob_hashes").and_then(|v| v.as_array()) else {
            return false;
        };
        if arr.is_empty() {
            return false;
        }
        arr.iter().all(|h| {
            let Some(h) = h.as_str() else {
                return false;
            };
            !is_full_lower_hex(h) || !self.contains(h)
        })
    }

    /// Validity ledger handle for this store root (creates nothing until
    /// first write; pre-ledger stores read as record-absent).
    pub fn validity_ledger(&self) -> crate::validity::ValidityLedger {
        crate::validity::ValidityLedger::open(self.store_root())
    }

    /// Read one blob's layer-validity record; `Ok(None)` when the blob was
    /// never published through this store (or predates the ledger).
    pub fn validity_record(
        &self,
        hash: &str,
    ) -> Result<Option<crate::validity::ValidityRecord>, CasError> {
        self.validity_ledger()
            .load(hash)
            .map_err(|e| CasError::Validity(format!("{e}")))
    }

    /// Load the replication declaration (`gc/replication.json`); missing
    /// file == no replication.
    pub fn replication_config(&self) -> Result<crate::replication::ReplicationConfig, CasError> {
        crate::replication::ReplicationConfig::load(self.store_root())
            .map_err(|e| CasError::Replication(format!("{e}")))
    }

    /// Repair a missing blob from the first declared replica that holds it
    /// . Re-publishes locally through the verified put path,
    /// which restores L3 validity in the ledger with the same identity.
    pub fn repair_from_replicas(
        &self,
        hash: &str,
    ) -> Result<crate::replication::RepairOutcome, CasError> {
        if !is_full_lower_hex(hash) {
            return Err(CasError::Malformed(hash.to_string()));
        }
        let config = self.replication_config()?;
        if !config.is_declared() {
            return Ok(crate::replication::RepairOutcome {
                restored: false,
                checked: 0,
            });
        }
        let outcome = crate::replication::repair_from_replicas(self.store_root(), &config, hash)
            .map_err(|e| CasError::Replication(format!("{e}")))?;
        if outcome.restored {
            // The restored bytes were verified by the hub put; complete the
            // refetch in the ledger so L3 validity is restored with the same
            // identity (mirror of CasStore::put_prehashed).
            let size = std::fs::metadata(self.hub.object_path(hash))
                .map(|m| m.len())
                .unwrap_or(0);
            self.validity_ledger()
                .publish(hash, size)
                .map_err(|e| CasError::Validity(format!("repair ledger restore: {e}")))?;
        }
        Ok(outcome)
    }

    /// Remove orphaned `*.tmp` files older than `max_age` across the CAS.
    /// Returns the count. Hub `put` reaps its own temp files; this walk
    /// covers engine-created temp files.
    pub fn sweep_stale_temps(&self, max_age: std::time::Duration) -> u64 {
        let mut removed = 0;
        let Ok(shards) = std::fs::read_dir(self.blobs_root.join(ALGO_DIR)) else {
            return 0;
        };
        for shard in shards.flatten() {
            removed += sweep_stale_temps_in(&shard.path(), max_age);
        }
        removed
    }
}

/// Remove `*.tmp` files in one shard directory older than `max_age`;
/// returns the number removed. Temps are inert either way — `get` opens
/// only exact 64-hex object names.
fn sweep_stale_temps_in(shard: &Path, max_age: std::time::Duration) -> u64 {
    let mut removed = 0;
    let Ok(entries) = std::fs::read_dir(shard) else {
        return 0;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.ends_with(".tmp") {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age >= max_age);
        if stale && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

pub fn gc_grace_from_env() -> std::time::Duration {
    let seconds = std::env::var("FSZERO_CAS_GC_GRACE_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_GC_GRACE_SECS);
    std::time::Duration::from_secs(seconds)
}

fn collect_pin_hashes(path: &Path, hashes: &mut HashSet<String>) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("read CAS pins metadata {}: {e}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_dir() {
        let entries = std::fs::read_dir(path)
            .map_err(|e| format!("read CAS pins dir {}: {e}", path.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("read CAS pins entry: {e}"))?;
            collect_pin_hashes(&entry.path(), hashes)?;
        }
        return Ok(());
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("read CAS pins file {}: {e}", path.display()))?;
    for run in text.split(|ch: char| !ch.is_ascii_hexdigit() || ch.is_ascii_uppercase()) {
        if is_full_lower_hex(run) {
            hashes.insert(run.to_string());
        }
    }
    Ok(())
}

/// Collect blob hashes from zerostack.cas-gc.legacy reachability snapshots
/// at <store_root>/gc/roots/<engine>/<project_id>/current.json.
/// These are published by TokenZero and GraphZero to mark objects as live.
fn collect_gc_roots_hashes(store_root: &Path, hashes: &mut HashSet<String>) {
    let roots_dir = store_root.join("gc").join("roots");
    let Ok(engine_dirs) = std::fs::read_dir(&roots_dir) else {
        return;
    };
    for engine_dir in engine_dirs.flatten() {
        let Ok(project_dirs) = std::fs::read_dir(engine_dir.path()) else {
            continue;
        };
        for project_dir in project_dirs.flatten() {
            let current = project_dir.path().join("current.json");
            if let Ok(text) = std::fs::read_to_string(&current) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let Some(arr) = v.get("blob_hashes").and_then(|v| v.as_array()) {
                        for h in arr {
                            if let Some(s) = h.as_str() {
                                if is_full_lower_hex(s) {
                                    hashes.insert(s.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Collect blob hashes from zerostack.cas-gc.legacy lease records at
/// <store_root>/gc/leases/**. Only non-expired leases protect objects.
fn collect_gc_leases_hashes(store_root: &Path, hashes: &mut HashSet<String>) {
    let leases_dir = store_root.join("gc").join("leases");
    collect_gc_leases_recursive(&leases_dir, hashes);
}

fn collect_gc_leases_recursive(path: &Path, hashes: &mut HashSet<String>) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if metadata.is_dir() {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            collect_gc_leases_recursive(&entry.path(), hashes);
        }
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    // Only non-expired leases protect objects. RFC3339 timestamps in UTC
    // sort lexicographically, so a string comparison suffices without chrono.
    if let Some(expires_at) = v.get("expires_at").and_then(|v| v.as_str()) {
        let now = rfc3339_utc_now();
        if expires_at < now.as_str() {
            return;
        }
    }
    if let Some(arr) = v.get("blob_hashes").and_then(|v| v.as_array()) {
        for h in arr {
            if let Some(s) = h.as_str() {
                if is_full_lower_hex(s) {
                    hashes.insert(s.to_string());
                }
            }
        }
    }
}

/// Current wall-clock time in Unix seconds (for validity cold marks).
fn unix_secs_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Best-effort current UTC time in RFC3339 for lease expiry comparison.
fn rfc3339_utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86400;
    let rem = secs % 86400;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    // Civil-from-days algorithm (Howard Hinnant).
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, m, d, hour, min, sec
    )
}

/// Result of publishing one FSZero reachability snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcRootsPublish {
    /// Absolute path of the published `current.json`.
    pub path: PathBuf,
    /// Stable 64-hex project identity (SHA-256 of the store-root identity).
    pub project_id: String,
    /// Monotonic epoch written into the snapshot (>= 1).
    pub epoch: u64,
    /// Distinct blob hashes listed in the snapshot.
    pub blob_count: usize,
}

/// Stable `project_id` for the shared-CAS GC namespace: SHA-256 of the
/// canonical store-root path when available, otherwise of the display path.
/// Producer-owned identity; must be 64 lowercase hex and stable for the root.
pub fn gc_project_id(store_root: &Path) -> String {
    let identity = std::fs::canonicalize(store_root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| store_root.to_string_lossy().into_owned());
    super::access_log::content_hash_bytes(identity.as_bytes())
}

/// Path of the FSZero reachability snapshot for `project_id`.
pub fn gc_roots_current_path(store_root: &Path, project_id: &str) -> PathBuf {
    store_root
        .join(zero_store::GC_DIR)
        .join("roots")
        .join(GC_ENGINE_FSZERO)
        .join(project_id)
        .join("current.json")
}

/// Read the epoch from an existing `current.json`, or `0` when absent/unreadable.
fn read_gc_roots_epoch(path: &Path) -> u64 {
    let Ok(text) = std::fs::read_to_string(path) else {
        return 0;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return 0;
    };
    v.get("epoch").and_then(|e| e.as_u64()).unwrap_or(0)
}

/// Publish FSZero reachability roots at `<store_root>/gc/roots/fszero/<project_id>/current.json`
/// matching `zerostack.cas-gc.legacy` / `reachability-snapshot.schema.json`. Atomic publication:
/// unique sibling temp + fsync + rename (hub [`zero_store::atomic_write_file`]).
pub fn publish_fszero_gc_roots(
    store_root: &Path,
    blob_hashes: impl IntoIterator<Item = String>,
) -> Result<GcRootsPublish, String> {
    if store_root_has_unexpanded_tilde(store_root) {
        return Err("gc_roots: store root must not use unexpanded '~'".to_string());
    }
    let project_id = gc_project_id(store_root);
    if !is_full_lower_hex(&project_id) {
        return Err(format!("gc_roots: invalid project_id {project_id}"));
    }
    let mut hashes: Vec<String> = blob_hashes.into_iter().collect();
    for h in &hashes {
        if !is_full_lower_hex(h) {
            return Err(format!("gc_roots: invalid blob hash {h}"));
        }
    }
    hashes.sort_unstable();
    hashes.dedup();

    let path = gc_roots_current_path(store_root, &project_id);
    let prior = read_gc_roots_epoch(&path);
    let epoch = prior.saturating_add(1).max(1);

    let snapshot = serde_json::json!({
        "schema_version": GC_SCHEMA_VERSION,
        "record_type": GC_RECORD_TYPE_REACHABILITY,
        "engine": GC_ENGINE_FSZERO,
        "project_id": project_id,
        "epoch": epoch,
        "published_at": rfc3339_utc_now(),
        "blob_hashes": hashes,
    });
    let bytes =
        serde_json::to_vec_pretty(&snapshot).map_err(|e| format!("gc_roots: serialize: {e}"))?;
    zero_store::atomic_write_file(&path, &bytes)
        .map_err(|e| format!("gc_roots: atomic publish {}: {e}", path.display()))?;

    Ok(GcRootsPublish {
        path,
        project_id,
        epoch,
        blob_count: hashes.len(),
    })
}

impl CasStore {
    /// Publish this store's FSZero reachability roots for
    /// the given live set. See [`publish_fszero_gc_roots`].
    pub fn publish_gc_roots(
        &self,
        blob_hashes: impl IntoIterator<Item = String>,
    ) -> Result<GcRootsPublish, String> {
        publish_fszero_gc_roots(self.store_root(), blob_hashes)
    }
}

/// Open hub [`zero_store::SharedCas`] for a store root (blobs live under root).
/// Prefer this for new call sites that only need hub-neutral put/get/list.
pub fn hub_shared_cas(store_root: &Path) -> zero_store::SharedCas {
    zero_store::SharedCas::open(store_root)
}

/// True when a store-root path string is illegally unexpanded (`~` prefix). Store roots
/// must be absolute or repo-relative; bare `~` is never expanded silently.
pub fn store_root_has_unexpanded_tilde(path: &Path) -> bool {
    path.as_os_str().as_encoded_bytes().starts_with(b"~")
}
