//! Shared CAS + GC compatibility adapter over the canonical `zero_store` hub.
//!
//! TokenZero-specific surface kept here (bead tokenzero-5emy):
//! - engine cache-path/namespace adapter logic (`<shared-root>/<engine>/...`)
//! - the `SharedCasError` taxonomy and the `SharedCas` wrapper
//! - lease lifecycle sugar (`publish_leased` / `release_lease`) over the hub
//!   atomic `put_leased` transaction and `remove_lease_record`
//! - `is_pinned`: a conservative pin-set query (the hub exposes no read API
//!   for the pin set, so this walks `gc/pins` with the hub `PinRecord` schema)
//!
//! Everything else -- GC record schemas, validation, mark/sweep, dry-run
//! reports, sweep progress, repair receipts, report pruning, contract
//! digests -- is delegated to the canonical `zero_store` implementation at the
//! pinned hub rev `bd721f7fc4866b24dec0c552da3d96bd8d816fbc`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

// Canonical hub GC surface (schema, records, sweep, repair, validation).
pub use zero_store::gc_project_id as project_id;
pub use zero_store::{
    BeforeUnlinkHook, DEFAULT_GC_REPORT_LIMIT, DryRunReport, GC_MAX_BLOB_HASHES,
    GC_MAX_EVIDENCE_ITEMS, GC_MAX_OWNER_HOST_BYTES, GC_MAX_PRODUCER_ID_BYTES,
    GC_MAX_PRODUCER_NAMESPACES, GC_MAX_RECORD_BYTES, GC_MAX_REPORT_OBJECTS, GC_MIN_GRACE_SECONDS,
    GC_RECORD_TYPE_DRY_RUN, GC_RECORD_TYPE_LEASE, GC_RECORD_TYPE_PIN, GC_RECORD_TYPE_REACHABILITY,
    GC_RECORD_TYPE_REPAIR, GC_RECORD_TYPE_SWEEP_PROGRESS, GC_REFS_FORMAT, GC_SCHEMA_VERSION,
    GC_SCHEMA_VERSION_LEGACY, GcCandidate, GcConfig, GcError, GcRunReceipt, GcRunState, GcVerdict,
    LeaseOwner, LeaseRecord, PinRecord, ReachabilitySnapshot, RepairReceipt,
    current_reachability_snapshot, gc_contract_digest_hex, gc_contract_manifest,
    gc_repair_receipt_digest_hex, gc_report_digest_hex, publish_lease_record, publish_pin_record,
    publish_reachability_snapshot, refs_from_verified_bytes, remove_lease_record,
    remove_pin_record, repair_object, repair_object_receipted, run_gc, validate_dry_run_report,
    validate_repair_receipt,
};

/// TokenZero producer namespace for hub GC records.
pub const GC_ENGINE_TOKENZERO: &str = "tokenzero";
/// Upper bound so `now + lease` cannot overflow SystemTime (tokenzero-mb70).
pub const MAX_LEASE_SECONDS: u64 = 30 * 24 * 60 * 60;
/// Upper bound for sweep grace; `u64::MAX` would make `checked_add` fail and
/// keep every expired lease inside grace forever.
pub const MAX_GC_GRACE_SECONDS: u64 = 7 * 24 * 60 * 60;
const GC_ENGINES: &[&str] = &["tokenzero", "fszero", "graphzero"];

/// Floor at the hub grace minimum, cap so expiry arithmetic cannot overflow.
pub fn clamp_lease_seconds(lease_seconds: u64) -> u64 {
    lease_seconds.clamp(GC_MIN_GRACE_SECONDS, MAX_LEASE_SECONDS)
}

/// Same clamp for sweep `grace_seconds`.
pub fn clamp_grace_seconds(grace_seconds: u64) -> u64 {
    grace_seconds.clamp(GC_MIN_GRACE_SECONDS, MAX_GC_GRACE_SECONDS)
}

/// Unlink lease records whose expiry plus grace is already in the past.
/// Leaves unreadable or non-canonical stamps in place (fail closed).
pub fn prune_stale_lease_records(store_root: &Path, now: SystemTime, grace_seconds: u64) -> u64 {
    let grace = clamp_grace_seconds(grace_seconds);
    let Some(cutoff) = now.checked_sub(std::time::Duration::from_secs(grace)) else {
        return 0;
    };
    let cutoff_stamp = format_system_time(cutoff);
    let leases_root = store_root.join("gc").join("leases");
    prune_expired_records(&leases_root, &cutoff_stamp)
}

fn prune_expired_records(root: &Path, cutoff_stamp: &str) -> u64 {
    let Ok(producers) = fs::read_dir(root) else {
        return 0;
    };
    let mut removed = 0_u64;
    for producer in producers.flatten() {
        if !producer.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(projects) = fs::read_dir(producer.path()) else {
            continue;
        };
        for project in projects.flatten() {
            if !project.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let Ok(entries) = fs::read_dir(project.path()) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                let Ok(text) = fs::read_to_string(&path) else {
                    continue;
                };
                let Ok(lease) = serde_json::from_str::<LeaseRecord>(&text) else {
                    continue;
                };
                if canonical_utc_expired(&lease.expires_at, cutoff_stamp)
                    && fs::remove_file(&path).is_ok()
                {
                    removed = removed.saturating_add(1);
                }
            }
        }
    }
    removed
}

#[derive(Debug, Error)]
pub enum SharedCasError {
    #[error("object not found")]
    NotFound,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("corruption: object does not match expected hash")]
    Corruption,
    #[error("policy violation")]
    Policy,
    #[error("invalid hash: {0}")]
    InvalidHash(String),
    #[error("gc record error: {0}")]
    Gc(String),
}

fn map_cas_error(error: zero_store::CasError) -> SharedCasError {
    match error {
        zero_store::CasError::NotFound => SharedCasError::NotFound,
        zero_store::CasError::Io(message) => SharedCasError::Io(io::Error::other(message)),
        zero_store::CasError::DigestMismatch { .. } => SharedCasError::Corruption,
        zero_store::CasError::PolicyDenied(_) | zero_store::CasError::Malformed(_) => {
            SharedCasError::Policy
        }
    }
}

fn map_gc_error(error: zero_store::GcError) -> SharedCasError {
    match error {
        zero_store::GcError::Io(error) => SharedCasError::Io(error),
        other => SharedCasError::Gc(other.to_string()),
    }
}

#[derive(Debug, Clone)]
pub struct SharedCas {
    inner: zero_store::SharedCas,
}

impl SharedCas {
    pub fn new(root: PathBuf) -> Self {
        Self {
            inner: zero_store::SharedCas::open(root),
        }
    }

    pub fn resolve_cache_root(cache_path: &Path) -> Option<PathBuf> {
        let engine_dir = cache_path.parent()?;
        (engine_dir.file_name()? == "tokenzero")
            .then(|| engine_dir.parent().map(Path::to_path_buf))
            .flatten()
    }

    pub fn attach_root_for_cache_path(cache_path: &Path) -> PathBuf {
        Self::resolve_cache_root(cache_path)
            .or_else(|| cache_path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| cache_path.to_path_buf())
    }

    pub fn sibling_engine_cache_path(cache_path: &Path, engine: &str) -> Option<PathBuf> {
        let engine_dir = cache_path.parent()?;
        let name = engine_dir.file_name()?.to_str()?;
        if !GC_ENGINES.contains(&name) {
            return None;
        }
        Some(
            engine_dir
                .parent()?
                .join(engine)
                .join("recovery-cache.json"),
        )
    }

    pub fn detect_from_cache_path(cache_path: &Path) -> Option<Self> {
        let unified_root = Self::resolve_cache_root(cache_path);
        let is_unified = unified_root.is_some();
        let root = unified_root.unwrap_or_else(|| Self::attach_root_for_cache_path(cache_path));
        (is_unified || root.join("blobs").is_dir()).then(|| Self::new(root))
    }

    /// Attach the existing unified/sibling CAS, or create a local hub CAS at
    /// the cache root. Isolated stores use this so they never grow a second
    /// `<cache>.blobs/` tree.
    pub fn attach_for_cache_path(cache_path: &Path) -> Self {
        Self::detect_from_cache_path(cache_path)
            .unwrap_or_else(|| Self::new(Self::attach_root_for_cache_path(cache_path)))
    }

    pub fn root(&self) -> &Path {
        self.inner.root()
    }

    /// Publish `bytes` under the shared GC coordination lock, so a sweep
    /// cannot collect the object between publication and the caller's
    /// reference.
    pub fn publish(&self, bytes: &[u8]) -> Result<String, SharedCasError> {
        self.inner
            .put_outcome(bytes, zero_store::CAS_MAX_OBJECT_BYTES)
            .map(|outcome| outcome.hash)
            .map_err(map_cas_error)
    }

    /// Publish `bytes` and its protecting lease in one hub-owned atomic
    /// transaction (`SharedCas::put_leased` under the exclusive coordinator
    /// lock). The object cannot become sweep-visible without the live lease.
    ///
    /// `lease_seconds` is clamped to `[GC_MIN_GRACE_SECONDS, MAX_LEASE_SECONDS]`
    /// so `now + lease` cannot overflow SystemTime (tokenzero-mb70).
    pub fn publish_leased(
        &self,
        bytes: &[u8],
        project_id: &str,
        operation_id: &str,
        lease_seconds: u64,
    ) -> Result<String, SharedCasError> {
        let owner = LeaseOwner {
            pid: std::process::id() as u64,
            host: std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string()),
        };
        self.inner
            .put_leased(
                bytes,
                GC_ENGINE_TOKENZERO,
                project_id,
                operation_id,
                1,
                owner,
                clamp_lease_seconds(lease_seconds),
            )
            .map(|outcome| outcome.hash)
            .map_err(map_gc_error)
    }

    /// Release the lease taken by [`SharedCas::publish_leased`] once the
    /// caller has committed a root that makes the object reachable on its
    /// own. Idempotent.
    pub fn release_lease(
        &self,
        project_id: &str,
        operation_id: &str,
    ) -> Result<(), SharedCasError> {
        remove_lease_record(self.root(), GC_ENGINE_TOKENZERO, project_id, operation_id)
            .map_err(map_gc_error)
    }

    pub fn resolve(&self, full_hash: &str) -> Result<Vec<u8>, SharedCasError> {
        self.validate_hash(full_hash)?;
        self.inner.get_verified(full_hash).map_err(map_cas_error)
    }

    pub fn contains(&self, full_hash: &str) -> bool {
        self.validate_hash(full_hash).is_ok() && self.inner.contains(full_hash)
    }

    /// Whether the GC sweep would treat `full_hash` as pinned.
    ///
    /// Deliberately conservative: an unreadable pin set reports pinned, since
    /// the sweep must not collect while its own evidence is in doubt. Callers
    /// asserting the NEGATIVE therefore also prove the pin set is readable.
    /// Mirrors the hub sweep semantics: an expired pin does not protect.
    pub fn is_pinned(&self, full_hash: &str) -> bool {
        if !zero_ref::is_full_lower_hex(full_hash) {
            return false;
        }
        let now = format_system_time(SystemTime::now());
        let pins_root = self.root().join("gc").join("pins");
        match fs::symlink_metadata(&pins_root) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return crate::action_cache::action_cache_protects_hash(self.root(), full_hash);
            }
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) | Err(_) => return true,
        }
        let Ok(producers) = fs::read_dir(&pins_root) else {
            return true;
        };
        for producer in producers {
            let Ok(producer) = producer else { return true };
            if !producer.file_type().is_ok_and(|kind| kind.is_dir()) {
                return true;
            }
            let Ok(projects) = fs::read_dir(producer.path()) else {
                return true;
            };
            for project in projects {
                let Ok(project) = project else { return true };
                if !project.file_type().is_ok_and(|kind| kind.is_dir()) {
                    return true;
                }
                let Ok(entries) = fs::read_dir(project.path()) else {
                    return true;
                };
                for entry in entries {
                    let Ok(entry) = entry else { return true };
                    if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                        return true;
                    }
                    let path = entry.path();
                    if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                        continue;
                    }
                    let Ok(text) = fs::read_to_string(&path) else {
                        return true;
                    };
                    let Ok(pin) = serde_json::from_str::<PinRecord>(&text) else {
                        return true;
                    };
                    if pin.blob_hash == full_hash
                        && !pin
                            .expires_at
                            .as_deref()
                            .is_some_and(|expires_at| canonical_utc_expired(expires_at, &now))
                    {
                        return true;
                    }
                }
            }
        }
        crate::action_cache::action_cache_protects_hash(self.root(), full_hash)
    }

    pub fn list_objects(&self) -> Result<Vec<String>, SharedCasError> {
        self.inner.list_objects().map_err(map_cas_error)
    }

    /// Restore one corrupt object from authoritative bytes, delegating to the
    /// hub repair path (quarantine + verified republish).
    pub fn repair_object(&self, full_hash: &str, bytes: &[u8]) -> Result<bool, SharedCasError> {
        self.validate_hash(full_hash)?;
        let actual = content_sha256_hex(bytes);
        if actual != full_hash {
            return Err(SharedCasError::InvalidHash(format!(
                "provided bytes hash to {actual}, expected {full_hash}"
            )));
        }
        repair_object(self.root(), full_hash, bytes).map_err(map_gc_error)
    }

    fn validate_hash(&self, full_hash: &str) -> Result<(), SharedCasError> {
        zero_ref::is_full_lower_hex(full_hash)
            .then_some(())
            .ok_or_else(|| SharedCasError::InvalidHash(full_hash.into()))
    }
}

/// Full 64-char lowercase SHA-256 hex digest of `bytes` (hub implementation).
pub(crate) fn content_sha256_hex(bytes: &[u8]) -> String {
    zero_ref::content_hash_hex(bytes)
}

/// Lowercase hex encoding of raw bytes (no separators).
pub(crate) fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

/// True when a canonical hub pin stamp (`YYYY-MM-DDTHH:MM:SSZ`, second
/// precision UTC) is at or before `now`. Canonical stamps sort
/// lexicographically in chronological order; any other shape is treated as
/// live (conservative).
fn canonical_utc_expired(expires_at: &str, now: &str) -> bool {
    canonical_utc_fields(expires_at).is_some()
        && canonical_utc_fields(now).is_some()
        && expires_at.as_bytes() <= now.as_bytes()
}

fn canonical_utc_fields(stamp: &str) -> Option<(u32, u32, u32, u32, u32, u32)> {
    if !stamp.is_ascii()
        || stamp.len() != 20
        || stamp.as_bytes()[4] != b'-'
        || stamp.as_bytes()[7] != b'-'
        || stamp.as_bytes()[10] != b'T'
        || stamp.as_bytes()[13] != b':'
        || stamp.as_bytes()[16] != b':'
        || stamp.as_bytes()[19] != b'Z'
    {
        return None;
    }
    let fields = (
        stamp[0..4].parse().ok()?,
        stamp[5..7].parse().ok()?,
        stamp[8..10].parse().ok()?,
        stamp[11..13].parse().ok()?,
        stamp[14..16].parse().ok()?,
        stamp[17..19].parse().ok()?,
    );
    let (year, month, day, hour, minute, second) = fields;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    (day > 0 && day <= days && hour < 24 && minute < 60 && second <= 60).then_some(fields)
}

/// Format `t` as second-precision UTC RFC3339 (`YYYY-MM-DDTHH:MM:SSZ`),
/// matching the hub GC record stamp format.
pub(crate) fn format_system_time(t: SystemTime) -> String {
    let seconds = t
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let days = (seconds / 86_400) as i64;
    let rem = seconds % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Process-unique suffix for temp object names (`{nanos}-{counter}`).
pub(crate) fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{ts}-{n}")
}
