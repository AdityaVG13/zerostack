//! Generic `zerostack.cas-gc.v1` reachability and collection state machine.
//!
//! This module owns only store metadata and immutable CAS lifecycle. It has no
//! engine-specific authority; engines publish roots, pins, and leases here.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;

use crate::cas::{CasError, SharedCas};
use crate::fs_replace::atomic_write_file;
use crate::{LOCK_DEADLINE, StoreLock};

pub const GC_SCHEMA_VERSION: &str = "zerostack.cas-gc.v1";
/// Hard bounds keep malformed metadata from turning collection into an
/// unbounded allocation or path traversal surface.
pub const GC_MAX_RECORD_BYTES: u64 = 32 * 1024 * 1024;
pub const GC_MAX_BLOB_HASHES: usize = 65_536;
pub const GC_MAX_REPORT_OBJECTS: usize = 65_536;
pub const GC_MAX_EVIDENCE_ITEMS: usize = 256;
const GC_EVIDENCE_TRUNCATED: &str = "evidence truncated at GC_MAX_EVIDENCE_ITEMS";
const GC_ENGINES: &[&str] = &["tokenzero", "fszero", "graphzero"];
pub const GC_RECORD_TYPE_REACHABILITY: &str = "reachability-snapshot";
pub const GC_RECORD_TYPE_PIN: &str = "pin";
pub const GC_RECORD_TYPE_LEASE: &str = "lease";
pub const GC_RECORD_TYPE_DRY_RUN: &str = "dry-run-report";
pub const GC_RECORD_TYPE_SWEEP_PROGRESS: &str = "sweep-progress";
pub const GC_MIN_GRACE_SECONDS: u64 = 60;
pub const DEFAULT_GC_REPORT_LIMIT: usize = 32;

fn require_gc_engine(engine: &str) -> Result<(), GcError> {
    if GC_ENGINES.contains(&engine) {
        Ok(())
    } else {
        Err(GcError::Policy(format!("invalid engine {engine}")))
    }
}

#[derive(Debug, Error)]
pub enum GcError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("schema violation: {0}")]
    SchemaViolation(String),
    #[error("corrupt metadata at {path}: {reason}")]
    CorruptMetadata { path: PathBuf, reason: String },
    #[error("uncertain metadata: {0}")]
    UncertainMetadata(String),
    #[error("policy violation: {0}")]
    Policy(String),
    #[error("fault injected")]
    FaultInjected,
}

impl From<CasError> for GcError {
    fn from(err: CasError) -> Self {
        match err {
            CasError::Io(message) => GcError::Io(io::Error::other(message)),
            CasError::DigestMismatch { .. } => {
                corrupt(Path::new(""), "CAS object corruption".into())
            }
            CasError::PolicyDenied(message) => GcError::Policy(message),
            CasError::Malformed(message) => GcError::SchemaViolation(message),
            CasError::NotFound => GcError::UncertainMetadata("CAS object not found".into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReachabilitySnapshot {
    pub schema_version: String,
    pub record_type: String,
    pub engine: String,
    pub project_id: String,
    pub epoch: u64,
    pub published_at: String,
    pub blob_hashes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinRecord {
    pub schema_version: String,
    pub record_type: String,
    pub engine: String,
    pub project_id: String,
    pub pin_id: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub blob_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseOwner {
    pub pid: u64,
    pub host: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRecord {
    pub schema_version: String,
    pub record_type: String,
    pub engine: String,
    pub project_id: String,
    pub operation_id: String,
    pub epoch: u64,
    pub owner: LeaseOwner,
    pub started_at: String,
    pub expires_at: String,
    pub grace_seconds: u64,
    pub blob_hashes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GcVerdict {
    Retain,
    Collect,
    RetainUncertain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcCandidate {
    pub blob_hash: String,
    pub verdict: GcVerdict,
    pub reason_codes: Vec<String>,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DryRunReport {
    pub schema_version: String,
    pub record_type: String,
    pub run_id: String,
    pub store_root: String,
    pub evaluated_at: String,
    pub objects: Vec<GcCandidate>,
}

/// See [`GcConfig::before_unlink`].
pub type BeforeUnlinkHook = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone)]
pub struct GcConfig {
    pub run_id: String,
    pub grace_seconds: u64,
    pub min_age_seconds: u64,
    pub apply: bool,
    pub now: SystemTime,
    pub fault_after_deletes: Option<usize>,
    /// Maximum completed JSON reports retained in `gc/reports`.
    pub report_limit: usize,
    /// Test seam: invoked with each hash immediately before it is unlinked,
    /// while the exclusive coordinator lock is held. Lets a regression pin the
    /// sweeper at the exact TOCTOU window without relying on timing
    /// (zerostack-rhd). Always `None` in production.
    pub before_unlink: Option<BeforeUnlinkHook>,
}

impl std::fmt::Debug for GcConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GcConfig")
            .field("run_id", &self.run_id)
            .field("grace_seconds", &self.grace_seconds)
            .field("min_age_seconds", &self.min_age_seconds)
            .field("apply", &self.apply)
            .field("now", &self.now)
            .field("fault_after_deletes", &self.fault_after_deletes)
            .field("report_limit", &self.report_limit)
            .field("before_unlink", &self.before_unlink.is_some())
            .finish()
    }
}

impl Default for GcConfig {
    fn default() -> Self {
        Self {
            run_id: "gc-run".into(),
            grace_seconds: GC_MIN_GRACE_SECONDS,
            min_age_seconds: 0,
            apply: false,
            now: SystemTime::now(),
            fault_after_deletes: None,
            report_limit: DEFAULT_GC_REPORT_LIMIT,
            before_unlink: None,
        }
    }
}

pub fn project_id(store_root: &Path) -> Result<String, GcError> {
    let canonical = store_root.canonicalize().map_err(GcError::Io)?;
    Ok(content_sha256_hex(canonical.to_string_lossy().as_bytes()))
}

fn ensure_real_directory_tree(dir: &Path) -> io::Result<()> {
    // RCH and developer installs can place the checkout below a symlinked
    // machine-level prefix. Confinement starts at this store's nearest `gc`
    // namespace, not at the filesystem root.
    let gc_root = dir
        .ancestors()
        .find(|candidate| candidate.file_name().and_then(|name| name.to_str()) == Some("gc"))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("GC path has no gc namespace: {}", dir.display()),
            )
        })?;

    fn ensure_real_dir(path: &Path) -> io::Result<()> {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("GC path component is not a real directory: {}", path.display()),
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => match fs::create_dir(path) {
                Ok(()) => Ok(()),
                Err(create_error) if create_error.kind() == io::ErrorKind::AlreadyExists => {
                    ensure_real_dir(path)
                }
                Err(create_error) => Err(create_error),
            },
            Err(error) => Err(error),
        }
    }

    ensure_real_dir(gc_root)?;
    let relative = dir.strip_prefix(gc_root).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("GC path escapes namespace: {}", dir.display()),
        )
    })?;
    let mut current = gc_root.to_path_buf();
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("GC path has invalid component: {}", dir.display()),
            ));
        }
        current.push(component.as_os_str());
        ensure_real_dir(&current)?;
    }
    Ok(())
}

pub fn gc_atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if bytes.len() as u64 > GC_MAX_RECORD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("GC record exceeds {GC_MAX_RECORD_BYTES} bytes"),
        ));
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    ensure_real_directory_tree(parent)?;
    atomic_write_file(path, bytes)
}

fn gc_join(store_root: &Path, parts: &[&str]) -> PathBuf {
    parts
        .iter()
        .fold(store_root.join("gc"), |p, part| p.join(part))
}

fn gc_record_path(store_root: &Path, subdir: &str, record: &impl GcRecord, id: &str) -> PathBuf {
    let (_, _, engine, project) = record.header();
    gc_join(
        store_root,
        &[subdir, engine, project, &format!("{id}.json")],
    )
}

fn validate_run_id(run_id: &str) -> Result<(), GcError> {
    if is_valid_pin_id(run_id) {
        Ok(())
    } else {
        Err(GcError::SchemaViolation(
            "run_id must be non-empty, <=128 chars, start with alphanumeric, and contain only alphanumeric, '.', '_', or '-'".into(),
        ))
    }
}

fn days_in_month(year: i64, month: u32) -> Option<u32> {
    Some(match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => return None,
    })
}

fn civil_to_days(year: i64, month: u32, day: u32) -> i64 {
    let (mut y, mut m) = (year, month as i64);
    if m <= 2 {
        y -= 1;
        m += 12;
    }
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day as i64 - 1;
    era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month as u32, day as u32)
}

fn parse_rfc3339(s: &str) -> Option<SystemTime> {
    if s.len() < 20
        || s.as_bytes()[4] != b'-'
        || s.as_bytes()[7] != b'-'
        || s.as_bytes()[10] != b'T'
        || s.as_bytes()[13] != b':'
        || s.as_bytes()[16] != b':'
    {
        return None;
    }
    let year: i64 = s[0..4].parse().ok()?;
    let month: u32 = s[5..7].parse().ok()?;
    let day: u32 = s[8..10].parse().ok()?;
    let hour: u32 = s[11..13].parse().ok()?;
    let minute: u32 = s[14..16].parse().ok()?;
    let second: u32 = s[17..19].parse().ok()?;
    if !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 60 || day == 0 {
        return None;
    }
    if day > days_in_month(year, month)? {
        return None;
    }
    let mut rest = &s[19..];
    let nanos = if let Some(frac) = rest.strip_prefix('.') {
        let n = frac.chars().take_while(|c| c.is_ascii_digit()).count();
        if n == 0 {
            return None;
        }
        let take = n.min(9);
        rest = &frac[n..];
        frac[..take].parse::<u64>().ok()? * 10u64.pow(9 - take as u32)
    } else {
        0
    };
    let offset = if rest.eq_ignore_ascii_case("Z") {
        0
    } else if rest.len() == 6
        && (rest.starts_with('+') || rest.starts_with('-'))
        && rest.as_bytes()[3] == b':'
    {
        let sign = if rest.starts_with('+') { 1i64 } else { -1 };
        let oh: i64 = rest[1..3].parse().ok()?;
        let om: i64 = rest[4..6].parse().ok()?;
        if oh > 23 || om > 59 {
            return None;
        }
        sign * (oh * 3600 + om * 60)
    } else {
        return None;
    };
    let local = civil_to_days(year, month, day) * 86400
        + hour as i64 * 3600
        + minute as i64 * 60
        + second as i64;
    let utc = local.checked_sub(offset)?;
    (utc >= 0).then(|| UNIX_EPOCH + std::time::Duration::new(utc as u64, nanos as u32))
}

/// Format `t` as second-precision UTC RFC3339 (`YYYY-MM-DDTHH:MM:SSZ`).
pub(crate) fn format_system_time(t: SystemTime) -> String {
    let seconds = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let rem = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Lowercase hex encoding of raw bytes (no separators).
pub(crate) fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

/// Full 64-char lowercase SHA-256 hex digest of `bytes`.
pub(crate) fn content_sha256_hex(bytes: &[u8]) -> String {
    lower_hex(Sha256::digest(bytes).as_ref())
}

fn is_valid_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn is_valid_pin_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

fn validate_namespace(path: &Path, engine: &str, project_id: &str) -> Result<(), GcError> {
    let components: Vec<&str> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    if components.len() < 4 {
        return Err(corrupt(path, format!("path too short: {}", path.display())));
    }
    let (path_engine, path_project) = (
        components[components.len() - 3],
        components[components.len() - 2],
    );
    if path_engine != engine {
        return Err(corrupt(
            path,
            format!("engine mismatch: path {path_engine}, record {engine}"),
        ));
    }
    if path_project != project_id {
        return Err(corrupt(
            path,
            format!("project_id mismatch: path {path_project}, record {project_id}"),
        ));
    }
    Ok(())
}

fn corrupt(path: &Path, reason: String) -> GcError {
    GcError::CorruptMetadata {
        path: path.to_path_buf(),
        reason,
    }
}

fn require_rfc3339(s: &str, path: &Path, field: &str) -> Result<(), GcError> {
    parse_rfc3339(s)
        .map(|_| ())
        .ok_or_else(|| corrupt(path, format!("invalid {field}")))
}

fn require_hash(s: &str, path: &Path, field: &str) -> Result<(), GcError> {
    is_valid_hash(s)
        .then_some(())
        .ok_or_else(|| corrupt(path, format!("invalid {field} {s}")))
}

fn require_min(value: u64, min: u64, path: &Path, field: &str) -> Result<(), GcError> {
    (value >= min)
        .then_some(())
        .ok_or_else(|| corrupt(path, format!("{field} {value} < {min}")))
}

trait GcRecord {
    fn header(&self) -> (&str, &str, &str, &str);
}

macro_rules! impl_gc_record {
    ($T:ty) => {
        impl GcRecord for $T {
            fn header(&self) -> (&str, &str, &str, &str) {
                (
                    &self.schema_version,
                    &self.record_type,
                    &self.engine,
                    &self.project_id,
                )
            }
        }
    };
}

impl_gc_record!(ReachabilitySnapshot);
impl_gc_record!(PinRecord);
impl_gc_record!(LeaseRecord);

fn read_gc_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, GcError> {
    let metadata = fs::symlink_metadata(path).map_err(GcError::Io)?;
    if !metadata.file_type().is_file() {
        return Err(corrupt(path, "metadata is not a regular file".into()));
    }
    if metadata.len() > GC_MAX_RECORD_BYTES {
        return Err(corrupt(path, format!("metadata exceeds {GC_MAX_RECORD_BYTES} bytes")));
    }
    let file = File::open(path).map_err(GcError::Io)?;
    let mut bytes = Vec::with_capacity((GC_MAX_RECORD_BYTES as usize).saturating_add(1));
    file.take(GC_MAX_RECORD_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(GcError::Io)?;
    if bytes.len() as u64 > GC_MAX_RECORD_BYTES {
        return Err(corrupt(path, format!("metadata exceeds {GC_MAX_RECORD_BYTES} bytes")));
    }
    serde_json::from_slice(&bytes).map_err(GcError::Json)
}

fn write_gc_json<T: Serialize>(path: &Path, value: &T) -> Result<(), GcError> {
    let bytes = serde_json::to_vec_pretty(value)?;
    if bytes.len() as u64 > GC_MAX_RECORD_BYTES {
        return Err(GcError::Policy(format!(
            "serialized GC record exceeds {GC_MAX_RECORD_BYTES} bytes"
        )));
    }
    gc_atomic_write(path, &bytes).map_err(GcError::Io)
}

fn validate_record_schema(
    schema_version: &str,
    record_type: &str,
    path: &Path,
    expected_type: &str,
) -> Result<(), GcError> {
    let reason = if schema_version != GC_SCHEMA_VERSION {
        Some(format!("unsupported schema_version {schema_version}"))
    } else if record_type != expected_type {
        Some(format!("record_type {record_type}"))
    } else {
        None
    };
    reason.map_or(Ok(()), |reason| Err(corrupt(path, reason)))
}

fn validate_record_common<R: GcRecord>(
    record: &R,
    path: &Path,
    expected_type: &str,
) -> Result<(), GcError> {
    let (schema_version, record_type, engine, project_id) = record.header();
    validate_record_schema(schema_version, record_type, path, expected_type)?;
    if !GC_ENGINES.contains(&engine) {
        return Err(corrupt(path, format!("invalid engine {engine}")));
    }
    validate_namespace(path, engine, project_id)
}

fn read_reachability_snapshot(path: &Path) -> Result<ReachabilitySnapshot, GcError> {
    let snap: ReachabilitySnapshot = read_gc_json(path)?;
    validate_record_common(&snap, path, GC_RECORD_TYPE_REACHABILITY)?;
    require_min(snap.epoch, 1, path, "epoch")?;
    require_rfc3339(&snap.published_at, path, "published_at")?;
    if snap.blob_hashes.len() > GC_MAX_BLOB_HASHES {
        return Err(corrupt(path, format!("too many blob hashes (max {GC_MAX_BLOB_HASHES})")));
    }
    for h in &snap.blob_hashes {
        require_hash(h, path, "blob hash")?;
    }
    Ok(snap)
}

fn read_pin_record(path: &Path) -> Result<PinRecord, GcError> {
    let pin: PinRecord = read_gc_json(path)?;
    validate_record_common(&pin, path, GC_RECORD_TYPE_PIN)?;
    if !is_valid_pin_id(&pin.pin_id) {
        return Err(corrupt(path, format!("invalid pin_id {}", pin.pin_id)));
    }
    require_rfc3339(&pin.created_at, path, "created_at")?;
    if let Some(exp) = pin.expires_at.as_deref() {
        require_rfc3339(exp, path, "expires_at")?;
    }
    require_hash(&pin.blob_hash, path, "blob_hash")?;
    Ok(pin)
}

fn read_lease_record(path: &Path) -> Result<LeaseRecord, GcError> {
    let lease: LeaseRecord = read_gc_json(path)?;
    validate_record_common(&lease, path, GC_RECORD_TYPE_LEASE)?;
    if !is_valid_pin_id(&lease.operation_id) {
        return Err(corrupt(
            path,
            format!("invalid operation_id {}", lease.operation_id),
        ));
    }
    require_min(lease.epoch, 1, path, "epoch")?;
    require_rfc3339(&lease.started_at, path, "started_at")?;
    require_rfc3339(&lease.expires_at, path, "expires_at")?;
    require_min(
        lease.grace_seconds,
        GC_MIN_GRACE_SECONDS,
        path,
        "grace_seconds",
    )?;
    if lease.blob_hashes.len() > GC_MAX_BLOB_HASHES {
        return Err(corrupt(path, format!("too many blob hashes (max {GC_MAX_BLOB_HASHES})")));
    }
    for h in &lease.blob_hashes {
        require_hash(h, path, "blob hash")?;
    }
    Ok(lease)
}

#[derive(Debug, Default)]
struct MarkState {
    live: BTreeMap<String, (BTreeSet<String>, BTreeSet<String>)>,
    uncertain: bool,
    global_evidence: Vec<String>,
}

fn push_bounded(values: &mut Vec<String>, value: String) {
    if values.iter().any(|existing| existing == &value) {
        return;
    }
    if values.len() < GC_MAX_EVIDENCE_ITEMS {
        values.push(value);
    } else if !values.iter().any(|existing| existing == GC_EVIDENCE_TRUNCATED) {
        values[GC_MAX_EVIDENCE_ITEMS - 1] = GC_EVIDENCE_TRUNCATED.into();
    }
}

fn push_bounded_set(values: &mut BTreeSet<String>, value: String) {
    if values.contains(&value) {
        return;
    }
    if values.len() < GC_MAX_EVIDENCE_ITEMS {
        values.insert(value);
    } else if !values.contains(GC_EVIDENCE_TRUNCATED) {
        values.pop_last();
        values.insert(GC_EVIDENCE_TRUNCATED.into());
    }
}

fn mark_hash(state: &mut MarkState, hash: &str, reason: &str, evidence: &str) {
    if state.live.len() >= GC_MAX_REPORT_OBJECTS && !state.live.contains_key(hash) {
        mark_uncertain(state, format!("live hash traversal exceeded {GC_MAX_REPORT_OBJECTS}"));
        return;
    }
    let meta = state.live.entry(hash.to_string()).or_default();
    meta.0.insert(reason.to_string());
    push_bounded_set(&mut meta.1, evidence.to_string());
}

fn mark_uncertain(state: &mut MarkState, evidence: String) {
    state.uncertain = true;
    push_bounded(&mut state.global_evidence, evidence);
}

const GC_MAX_PROJECT_NAMESPACES: usize = GC_MAX_REPORT_OBJECTS;

fn walk_gc_projects(
    store_root: &Path,
    subdir: &str,
    mut f: impl FnMut(&Path) -> Result<(), GcError>,
) -> Result<(), GcError> {
    let dir = store_root.join("gc").join(subdir);
    let dir_meta = match fs::symlink_metadata(&dir) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(GcError::Io(error)),
    };
    if !dir_meta.file_type().is_dir() {
        return Err(corrupt(&dir, "GC namespace is not a real directory".into()));
    }
    let mut project_count = 0usize;
    for engine_entry in fs::read_dir(&dir)? {
        let engine_entry = engine_entry?;
        let engine_type = engine_entry.file_type()?;
        if !engine_type.is_dir() {
            return Err(corrupt(
                &engine_entry.path(),
                "GC engine namespace is not a real directory".into(),
            ));
        }
        for project_entry in fs::read_dir(engine_entry.path())? {
            let project_entry = project_entry?;
            let project_type = project_entry.file_type()?;
            if !project_type.is_dir() {
                return Err(corrupt(
                    &project_entry.path(),
                    "GC project namespace is not a real directory".into(),
                ));
            }
            project_count = project_count.saturating_add(1);
            if project_count > GC_MAX_PROJECT_NAMESPACES {
                return Err(GcError::Policy(format!(
                    "GC project traversal exceeds {GC_MAX_PROJECT_NAMESPACES}"
                )));
            }
            f(&project_entry.path())?;
        }
    }
    Ok(())
}

fn count_gc_json_entry(count: &mut usize) -> Result<(), GcError> {
    if *count >= GC_MAX_BLOB_HASHES {
        return Err(GcError::Policy(format!(
            "GC JSON traversal exceeds {GC_MAX_BLOB_HASHES} entries"
        )));
    }
    *count += 1;
    Ok(())
}

fn walk_gc_json(
    store_root: &Path,
    subdir: &str,
    mut f: impl FnMut(&Path) -> Result<(), GcError>,
) -> Result<(), GcError> {
    let mut json_count = 0usize;
    walk_gc_projects(store_root, subdir, |project_dir| {
        for entry in fs::read_dir(project_dir)? {
            let entry = entry?;
            let entry_type = entry.file_type()?;
            if !entry_type.is_file() {
                return Err(corrupt(
                    &entry.path(),
                    "GC metadata entry is not a real file".into(),
                ));
            }
            let path = entry.path();
            if path.extension().and_then(|v| v.to_str()) == Some("json") {
                count_gc_json_entry(&mut json_count)?;
                f(&path)?;
            }
        }
        Ok(())
    })
}

fn walk_gc_records<T>(
    store_root: &Path,
    subdir: &str,
    state: &mut MarkState,
    read: fn(&Path) -> Result<T, GcError>,
    mut visit: impl FnMut(&Path, T, &mut MarkState),
) -> Result<(), GcError> {
    walk_gc_json(store_root, subdir, |path| {
        match read(path) {
            Ok(record) => visit(path, record, state),
            Err(err) => mark_uncertain(state, format!("{}: {err}", path.display())),
        }
        Ok(())
    })
}
fn load_all_pins(store_root: &Path, state: &mut MarkState, now: SystemTime) -> Result<(), GcError> {
    walk_gc_records(
        store_root,
        "pins",
        state,
        read_pin_record,
        |path, pin, state| {
            if pin
                .expires_at
                .as_deref()
                .and_then(parse_rfc3339)
                .is_some_and(|exp| exp <= now)
            {
                mark_uncertain(
                    state,
                    format!(
                        "expired pin {} retained on clock uncertainty",
                        path.display()
                    ),
                );
            }
            mark_hash(
                state,
                &pin.blob_hash,
                "pin",
                &format!("pin {}", path.display()),
            );
        },
    )
}

fn load_mark_state(
    store_root: &Path,
    now: SystemTime,
    grace_seconds: u64,
) -> Result<MarkState, GcError> {
    let mut state = MarkState::default();
    if !store_root.join("gc").join("roots").is_dir() {
        mark_uncertain(
            &mut state,
            "missing gc/roots directory; reachability metadata absent".into(),
        );
    } else {
        let mut saw_any_project = false;
        walk_gc_projects(store_root, "roots", |project_dir| {
            saw_any_project = true;
            let current = project_dir.join("current.json");
            if !current.is_file() {
                mark_uncertain(
                    &mut state,
                    format!("missing reachability snapshot {}", current.display()),
                );
                return Ok(());
            }
            match read_reachability_snapshot(&current) {
                Ok(snap) => {
                    let evidence = format!("root {} epoch {}", current.display(), snap.epoch);
                    for h in &snap.blob_hashes {
                        mark_hash(&mut state, h, "reachability-root", &evidence);
                    }
                }
                Err(err) => mark_uncertain(&mut state, format!("{}: {err}", current.display())),
            }
            Ok(())
        })?;
        if !saw_any_project {
            mark_uncertain(
                &mut state,
                "gc/roots has no project namespaces; reachability metadata absent".into(),
            );
        }
    }
    load_all_pins(store_root, &mut state, now)?;
    walk_gc_records(
        store_root,
        "leases",
        &mut state,
        read_lease_record,
        |path, lease, state| {
            let expires = parse_rfc3339(&lease.expires_at).unwrap_or(now);
            let grace_end =
                expires + std::time::Duration::from_secs(lease.grace_seconds.max(grace_seconds));
            let active = now <= expires;
            let in_grace = !active && now < grace_end;
            let reason = if active {
                "active-lease"
            } else {
                "stale-lease-grace"
            };
            let evidence = if active {
                format!("lease {}", path.display())
            } else if in_grace {
                format!("lease {} inside grace", path.display())
            } else {
                format!("lease {} retained on uncertain liveness", path.display())
            };
            if !active && !in_grace {
                mark_uncertain(
                    state,
                    format!(
                        "lease {} stale outside grace; owner liveness unverified",
                        path.display()
                    ),
                );
            }
            for h in &lease.blob_hashes {
                mark_hash(state, h, reason, &evidence);
            }
        },
    )?;
    Ok(state)
}

fn build_dry_run_report(
    store_root: &Path,
    run_id: &str,
    cas: &SharedCas,
    state: &MarkState,
    min_age_seconds: u64,
    now: SystemTime,
) -> Result<DryRunReport, GcError> {
    let mut objects = Vec::new();
    for hash in cas.list_objects()? {
        let (verdict, mut reasons, evidence) = if let Some(meta) = state.live.get(&hash) {
            (
                GcVerdict::Retain,
                meta.0.iter().cloned().collect(),
                meta.1.iter().cloned().collect(),
            )
        } else if state.uncertain {
            (
                GcVerdict::RetainUncertain,
                vec!["uncertain-metadata".into()],
                state.global_evidence.clone(),
            )
        } else {
            let young = fs::metadata(cas.object_path(&hash))
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|m| now.duration_since(m).unwrap_or_default().as_secs() < min_age_seconds)
                .unwrap_or(true);
            if young {
                (
                    GcVerdict::RetainUncertain,
                    vec!["uncertain-metadata".into()],
                    vec![format!("object younger than {min_age_seconds} seconds")],
                )
            } else {
                (
                    GcVerdict::Collect,
                    vec!["no-live-reference".into()],
                    vec!["no reachable root, pin, or lease".into()],
                )
            }
        };
        if reasons.is_empty() {
            reasons.push("uncertain-metadata".into());
        }
        let mut bounded_evidence = Vec::new();
        for item in evidence {
            push_bounded(&mut bounded_evidence, item);
        }
        objects.push(GcCandidate {
            blob_hash: hash,
            verdict,
            reason_codes: reasons,
            evidence: bounded_evidence,
        });
    }
    if objects.len() > GC_MAX_REPORT_OBJECTS {
        return Err(GcError::Policy(format!("report exceeds {GC_MAX_REPORT_OBJECTS} objects")));
    }
    objects.sort_by(|a, b| a.blob_hash.cmp(&b.blob_hash));
    Ok(DryRunReport {
        schema_version: GC_SCHEMA_VERSION.to_string(),
        record_type: GC_RECORD_TYPE_DRY_RUN.to_string(),
        run_id: run_id.to_string(),
        store_root: store_root.to_string_lossy().into_owned(),
        evaluated_at: format_system_time(SystemTime::now()),
        objects,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SweepProgress {
    schema_version: String,
    record_type: String,
    run_id: String,
    store_root: String,
    evaluated_at: String,
    objects: Vec<String>,
    deleted: Vec<String>,
    state: String,
}

fn read_sweep_progress(path: &Path) -> Result<SweepProgress, GcError> {
    let progress: SweepProgress = read_gc_json(path)?;
    validate_record_schema(
        &progress.schema_version,
        &progress.record_type,
        path,
        GC_RECORD_TYPE_SWEEP_PROGRESS,
    )?;
    if progress.run_id.is_empty() {
        return Err(GcError::SchemaViolation("run_id empty".into()));
    }
    if progress.store_root.is_empty() {
        return Err(GcError::SchemaViolation("store_root empty".into()));
    }
    if progress.objects.len() > GC_MAX_BLOB_HASHES {
        return Err(corrupt(
            path,
            format!("too many sweep objects (max {GC_MAX_BLOB_HASHES})"),
        ));
    }
    if progress.deleted.len() > GC_MAX_BLOB_HASHES {
        return Err(corrupt(
            path,
            format!("too many deleted hashes (max {GC_MAX_BLOB_HASHES})"),
        ));
    }
    for h in progress.objects.iter().chain(progress.deleted.iter()) {
        require_hash(h, path, "blob hash")?;
    }
    Ok(progress)
}

fn prune_gc_reports(store_root: &Path, keep: usize, current: &Path) -> Result<(), GcError> {
    let keep = keep.max(1);
    let reports_dir = store_root.join("gc").join("reports");
    let mut reports = Vec::new();
    for entry in fs::read_dir(&reports_dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if entry.file_type()?.is_file()
            && name.ends_with(".json")
            && !name.ends_with(".progress.json")
        {
            let modified = entry.metadata()?.modified().unwrap_or(UNIX_EPOCH);
            reports.push((modified, name.to_owned(), path));
        }
    }
    reports.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    while reports.len() > keep {
        let index = reports
            .iter()
            .position(|(_, _, path)| path != current)
            .unwrap_or(0);
        let (_, _, path) = reports.remove(index);
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn run_gc(store_root: &Path, config: &GcConfig) -> Result<DryRunReport, GcError> {
    validate_run_id(&config.run_id)?;
    let coord = StoreLock::sweep(store_root, LOCK_DEADLINE).map_err(GcError::Io)?;
    let cas = SharedCas::open(store_root.to_path_buf());
    let store_root_key = store_root.to_string_lossy().into_owned();
    let progress_path = gc_join(
        store_root,
        &["reports", &format!("{}.progress.json", config.run_id)],
    );
    let prior_progress = if progress_path.is_file() {
        let progress = read_sweep_progress(&progress_path)?;
        if progress.run_id != config.run_id {
            return Err(GcError::SchemaViolation(format!(
                "progress run_id {} does not match config {}",
                progress.run_id, config.run_id
            )));
        }
        if progress.store_root != store_root_key {
            return Err(GcError::SchemaViolation(format!(
                "progress store_root {} does not match {}",
                progress.store_root, store_root_key
            )));
        }
        Some(progress)
    } else {
        None
    };

    let state = load_mark_state(store_root, config.now, config.grace_seconds)?;
    let report = build_dry_run_report(
        store_root,
        &config.run_id,
        &cas,
        &state,
        config.min_age_seconds,
        config.now,
    )?;
    let report_path = gc_join(store_root, &["reports", &format!("{}.json", config.run_id)]);
    write_gc_json(&report_path, &report)?;
    prune_gc_reports(store_root, config.report_limit, &report_path)?;
    if !config.apply {
        return Ok(report);
    }

    let mut deleted: Vec<String> = prior_progress
        .as_ref()
        .map(|p| {
            p.deleted
                .iter()
                .filter(|h| !cas.contains(h))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let to_delete: Vec<String> = report
        .objects
        .iter()
        .filter(|o| o.verdict == GcVerdict::Collect)
        .map(|o| o.blob_hash.clone())
        .collect();
    let persist = |deleted: &[String]| -> Result<(), GcError> {
        write_gc_json(
            &progress_path,
            &SweepProgress {
                schema_version: GC_SCHEMA_VERSION.to_string(),
                record_type: GC_RECORD_TYPE_SWEEP_PROGRESS.to_string(),
                run_id: config.run_id.clone(),
                store_root: store_root_key.clone(),
                evaluated_at: report.evaluated_at.clone(),
                objects: to_delete.clone(),
                deleted: deleted.to_vec(),
                state: "sweeping".into(),
            },
        )
    };
    persist(&deleted)?;

    for hash in &to_delete {
        if deleted.contains(hash) {
            continue;
        }
        let re_state = load_mark_state(store_root, config.now, config.grace_seconds)?;
        if re_state.live.contains_key(hash) || re_state.uncertain {
            continue;
        }
        if let Some(hook) = config.before_unlink.as_ref() {
            hook(hash);
        }
        cas.remove_object(hash, &coord)?;
        deleted.push(hash.clone());
        persist(&deleted)?;
        if config.fault_after_deletes == Some(deleted.len()) {
            return Err(GcError::FaultInjected);
        }
    }

    let deleted_set: BTreeSet<_> = deleted.iter().cloned().collect();
    let mut final_report = report.clone();
    for obj in &mut final_report.objects {
        if obj.verdict != GcVerdict::Collect {
            continue;
        }
        if deleted_set.contains(&obj.blob_hash) {
            obj.evidence.push("deleted by this sweep".into());
            continue;
        }
        obj.verdict = GcVerdict::RetainUncertain;
        obj.reason_codes = vec!["uncertain-metadata".into()];
        obj.evidence = vec!["re-check before delete showed a live reference or uncertainty".into()];
    }
    write_gc_json(&report_path, &final_report)?;
    prune_gc_reports(store_root, config.report_limit, &report_path)?;
    let _ = fs::remove_file(&progress_path);
    Ok(final_report)
}

const DRY_RUN_FIELDS: &[&str] = &[
    "schema_version",
    "record_type",
    "run_id",
    "store_root",
    "evaluated_at",
    "objects",
];
const CANDIDATE_FIELDS: &[&str] = &["blob_hash", "verdict", "reason_codes", "evidence"];
const REASON_CODES: &[&str] = &[
    "reachability-root",
    "pin",
    "active-lease",
    "stale-lease-grace",
    "shared-root",
    "unknown-version",
    "corrupt-metadata",
    "uncertain-metadata",
    "unpublished-temp",
    "namespace-isolation",
    "no-live-reference",
];

fn require_str<'a>(value: &'a serde_json::Value, field: &str) -> Result<&'a str, GcError> {
    value
        .get(field)
        .and_then(|v| v.as_str())
        .ok_or_else(|| GcError::SchemaViolation(field.into()))
}

fn exact_keys(value: &serde_json::Value, fields: &[&str], err: &str) -> Result<(), GcError> {
    let obj = value
        .as_object()
        .ok_or_else(|| GcError::SchemaViolation(err.into()))?;
    let keys: BTreeSet<_> = obj.keys().cloned().collect();
    let expected: BTreeSet<_> = fields.iter().map(|s| (*s).to_string()).collect();
    for field in fields {
        if !keys.contains(*field) {
            return Err(GcError::SchemaViolation(format!("missing {field}")));
        }
    }
    if keys != expected {
        return Err(GcError::SchemaViolation(format!(
            "{err}: {:?}",
            keys.difference(&expected)
        )));
    }
    Ok(())
}

fn validate_list(
    value: &serde_json::Value,
    field: &str,
    allow: Option<&[&str]>,
) -> Result<(), GcError> {
    let items = value
        .get(field)
        .and_then(|v| v.as_array())
        .ok_or_else(|| GcError::SchemaViolation(field.into()))?;
    if items.len() > GC_MAX_EVIDENCE_ITEMS {
        return Err(GcError::SchemaViolation(format!(
            "{field} exceeds {GC_MAX_EVIDENCE_ITEMS} items"
        )));
    }
    if field == "reason_codes" && items.is_empty() {
        return Err(GcError::SchemaViolation("reason_codes empty".into()));
    }
    let reasons = field == "reason_codes";
    let mut seen = BTreeSet::new();
    for item in items {
        let s = item.as_str().ok_or_else(|| {
            GcError::SchemaViolation(if reasons { "reason_code" } else { "evidence" }.into())
        })?;
        if !reasons && s.is_empty() {
            return Err(GcError::SchemaViolation("empty evidence".into()));
        }
        if allow.is_some_and(|a| !a.contains(&s)) {
            return Err(GcError::SchemaViolation(format!("reason_code {s}")));
        }
        if !seen.insert(s) {
            return Err(GcError::SchemaViolation(
                if reasons {
                    "duplicate reason_code"
                } else {
                    "duplicate evidence"
                }
                .into(),
            ));
        }
    }
    Ok(())
}

pub fn validate_dry_run_report(value: &serde_json::Value) -> Result<(), GcError> {
    exact_keys(value, DRY_RUN_FIELDS, "extra top-level keys")?;
    if value.get("schema_version").and_then(|v| v.as_str()) != Some(GC_SCHEMA_VERSION) {
        return Err(GcError::SchemaViolation("schema_version".into()));
    }
    if value.get("record_type").and_then(|v| v.as_str()) != Some(GC_RECORD_TYPE_DRY_RUN) {
        return Err(GcError::SchemaViolation("record_type".into()));
    }
    validate_run_id(require_str(value, "run_id")?)?;
    if require_str(value, "store_root")?.is_empty() {
        return Err(GcError::SchemaViolation("store_root empty".into()));
    }
    if parse_rfc3339(require_str(value, "evaluated_at")?).is_none() {
        return Err(GcError::SchemaViolation("evaluated_at".into()));
    }
    let objects = value
        .get("objects")
        .and_then(|v| v.as_array())
        .ok_or_else(|| GcError::SchemaViolation("objects".into()))?;
    if objects.len() > GC_MAX_REPORT_OBJECTS {
        return Err(GcError::SchemaViolation(format!(
            "objects exceeds {GC_MAX_REPORT_OBJECTS}"
        )));
    }
    let mut seen_hashes = BTreeSet::new();
    for obj in objects {
        exact_keys(obj, CANDIDATE_FIELDS, "extra object keys")?;
        let blob_hash = require_str(obj, "blob_hash")?;
        if !seen_hashes.insert(blob_hash.to_string()) {
            return Err(GcError::SchemaViolation("duplicate blob_hash".into()));
        }
        if !is_valid_hash(require_str(obj, "blob_hash")?) {
            return Err(GcError::SchemaViolation("blob_hash".into()));
        }
        if !matches!(
            require_str(obj, "verdict")?,
            "retain" | "collect" | "retain-uncertain"
        ) {
            return Err(GcError::SchemaViolation("verdict".into()));
        }
        validate_list(obj, "reason_codes", Some(REASON_CODES))?;
        validate_list(obj, "evidence", None)?;
    }
    Ok(())
}

pub fn publish_reachability_snapshot(
    store_root: &Path,
    engine: &str,
    project_id: &str,
    epoch: u64,
    blob_hashes: &[String],
) -> Result<PathBuf, GcError> {
    require_gc_engine(engine)?;
    if !is_valid_hash(project_id) {
        return Err(GcError::SchemaViolation("project_id".into()));
    }
    if epoch == 0 {
        return Err(GcError::SchemaViolation("epoch must be >= 1".into()));
    }
    if blob_hashes.len() > GC_MAX_BLOB_HASHES {
        return Err(GcError::SchemaViolation(format!(
            "blob_hashes exceeds {GC_MAX_BLOB_HASHES}"
        )));
    }
    if let Some(h) = blob_hashes.iter().find(|h| !is_valid_hash(h)) {
        return Err(GcError::Policy(format!("invalid hash {h}")));
    }
    let _coord = StoreLock::sweep(store_root, LOCK_DEADLINE).map_err(GcError::Io)?;
    let path = gc_join(store_root, &["roots", engine, project_id, "current.json"]);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(corrupt(&path, "reachability snapshot is not a regular file".into()));
        }
        Ok(_) => {
            let existing = read_reachability_snapshot(&path)?;
            if epoch <= existing.epoch {
                return Err(GcError::SchemaViolation(format!(
                    "epoch {epoch} must be strictly greater than current {}",
                    existing.epoch
                )));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(GcError::Io(error)),
    }
    let mut hashes = blob_hashes.to_vec();
    hashes.sort_unstable();
    hashes.dedup();
    write_gc_json(
        &path,
        &ReachabilitySnapshot {
            schema_version: GC_SCHEMA_VERSION.to_string(),
            record_type: GC_RECORD_TYPE_REACHABILITY.to_string(),
            engine: engine.to_string(),
            project_id: project_id.to_string(),
            epoch,
            published_at: format_system_time(SystemTime::now()),
            blob_hashes: hashes,
        },
    )?;
    Ok(path)
}

fn require_schema_field(valid: bool, field: &str) -> Result<(), GcError> {
    valid
        .then_some(())
        .ok_or_else(|| GcError::SchemaViolation(field.into()))
}

fn require_schema(schema_version: &str, record_type: &str, expected: &str) -> Result<(), GcError> {
    require_schema_field(schema_version == GC_SCHEMA_VERSION, "schema_version")?;
    require_schema_field(record_type == expected, "record_type")
}

pub fn publish_pin_record(store_root: &Path, pin: &PinRecord) -> Result<PathBuf, GcError> {
    require_schema(&pin.schema_version, &pin.record_type, GC_RECORD_TYPE_PIN)?;
    require_gc_engine(&pin.engine)?;
    require_schema_field(is_valid_hash(&pin.project_id), "project_id")?;
    require_schema_field(is_valid_pin_id(&pin.pin_id), "pin_id")?;
    require_schema_field(is_valid_hash(&pin.blob_hash), "blob_hash")?;
    let path = gc_record_path(store_root, "pins", pin, &pin.pin_id);
    let _coord = StoreLock::sweep(store_root, LOCK_DEADLINE).map_err(GcError::Io)?;
    write_gc_json(&path, pin)?;
    Ok(path)
}

/// Remove a previously published pin. Idempotent: a pin that is already gone is
/// not an error, since callers unpin on resolution paths that may be retried.
///
/// Takes the same exclusive coordinator lock as `publish_pin_record` so a sweep
/// cannot observe a half-removed pin set.
pub fn remove_pin_record(
    store_root: &Path,
    engine: &str,
    project_id: &str,
    pin_id: &str,
) -> Result<(), GcError> {
    require_gc_engine(engine)?;
    require_schema_field(is_valid_hash(project_id), "project_id")?;
    require_schema_field(is_valid_pin_id(pin_id), "pin_id")?;
    let path = gc_join(
        store_root,
        &["pins", engine, project_id, &format!("{pin_id}.json")],
    );
    let _coord = StoreLock::sweep(store_root, LOCK_DEADLINE).map_err(GcError::Io)?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

pub fn publish_lease_record(store_root: &Path, lease: &LeaseRecord) -> Result<PathBuf, GcError> {
    let path = validate_lease_record(store_root, lease)?;
    let _coord = StoreLock::sweep(store_root, LOCK_DEADLINE).map_err(GcError::Io)?;
    write_gc_json(&path, lease)?;
    Ok(path)
}

/// Write a lease assuming the caller already holds a GC coordination lock.
/// `publish_leased` runs under the SHARED lock, so it must not re-acquire the
/// exclusive one here.
pub(crate) fn publish_lease_record_locked(
    store_root: &Path,
    lease: &LeaseRecord,
) -> Result<PathBuf, GcError> {
    let path = validate_lease_record(store_root, lease)?;
    write_gc_json(&path, lease)?;
    Ok(path)
}

fn validate_lease_record(store_root: &Path, lease: &LeaseRecord) -> Result<PathBuf, GcError> {
    require_schema(
        &lease.schema_version,
        &lease.record_type,
        GC_RECORD_TYPE_LEASE,
    )?;
    require_gc_engine(&lease.engine)?;
    require_schema_field(is_valid_hash(&lease.project_id), "project_id")?;
    require_schema_field(is_valid_pin_id(&lease.operation_id), "operation_id")?;
    // Writer and reader must agree. read_lease_record requires epoch >= 1, so
    // accepting 0 here would let a caller persist a lease that the sweep then
    // discards as corrupt -- protection that silently does not exist.
    require_schema_field(lease.epoch >= 1, "epoch")?;
    require_schema_field(
        lease.blob_hashes.len() <= GC_MAX_BLOB_HASHES,
        "blob_hashes",
    )?;
    if lease.grace_seconds < GC_MIN_GRACE_SECONDS {
        return Err(GcError::SchemaViolation(format!(
            "grace_seconds < {}",
            GC_MIN_GRACE_SECONDS
        )));
    }
    for (field, stamp) in [
        ("expires_at", &lease.expires_at),
        ("started_at", &lease.started_at),
    ] {
        if parse_rfc3339(stamp).is_none() {
            return Err(GcError::SchemaViolation((*field).into()));
        }
    }
    require_schema_field(
        lease.blob_hashes.iter().all(|h| is_valid_hash(h)),
        "blob_hash",
    )?;
    Ok(gc_record_path(
        store_root,
        "leases",
        lease,
        &lease.operation_id,
    ))
}


/// Verify an immutable object and restore it from authoritative bytes when it
/// is absent or corrupt. Corrupt content is removed only under the exclusive
/// coordination lock; replacement uses the normal publish protocol.
pub fn repair_object(store_root: &Path, blob_hash: &str, bytes: &[u8]) -> Result<bool, GcError> {
    if !is_valid_hash(blob_hash) {
        return Err(GcError::SchemaViolation("blob_hash".into()));
    }
    if content_sha256_hex(bytes) != blob_hash {
        return Err(GcError::CorruptMetadata {
            path: PathBuf::from(blob_hash),
            reason: "repair bytes do not match blob hash".into(),
        });
    }
    let cas = SharedCas::open(store_root.to_path_buf());
    match cas.get_verified(blob_hash) {
        Ok(_) => return Ok(false),
        Err(CasError::NotFound) => {}
        Err(CasError::DigestMismatch { .. }) => {
            let lock = StoreLock::sweep(store_root, LOCK_DEADLINE).map_err(GcError::Io)?;
            match cas.remove_object(blob_hash, &lock) {
                Ok(()) | Err(CasError::NotFound) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(error) => return Err(error.into()),
    }
    cas.put(bytes).map_err(GcError::from)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Mutex};
    use std::thread;

    fn setup_rooted_store() -> (tempfile::TempDir, SharedCas, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let cas = SharedCas::open(dir.path().to_path_buf());
        let root = cas.put(b"live root").unwrap();
        let project = project_id(dir.path()).unwrap();
        publish_reachability_snapshot(
            dir.path(), "tokenzero", &project, 1, std::slice::from_ref(&root),
        ).unwrap();
        (dir, cas, project, root)
    }

    fn verdict(report: &DryRunReport, hash: &str) -> GcVerdict {
        report.objects.iter().find(|object| object.blob_hash == hash)
            .unwrap().verdict
    }

    #[test]
    fn live_root_and_unreferenced_object_are_classified() {
        let (dir, cas, _, root) = setup_rooted_store();
        let orphan = cas.put(b"orphan").unwrap();
        let report = run_gc(dir.path(), &GcConfig::default()).unwrap();
        assert_eq!(verdict(&report, &root), GcVerdict::Retain);
        assert_eq!(verdict(&report, &orphan), GcVerdict::Collect);
    }

    #[test]
    fn pins_and_leases_preserve_unrooted_objects() {
        let (dir, cas, project, _) = setup_rooted_store();
        let pinned = cas.put(b"pinned").unwrap();
        publish_pin_record(dir.path(), &PinRecord {
            schema_version: GC_SCHEMA_VERSION.into(), record_type: "pin".into(),
            engine: "tokenzero".into(), project_id: project.clone(), pin_id: "pin-1".into(),
            created_at: format_system_time(SystemTime::now()), expires_at: None,
            blob_hash: pinned.clone(),
        }).unwrap();
        let leased = cas.put(b"leased").unwrap();
        publish_lease_record(dir.path(), &LeaseRecord {
            schema_version: GC_SCHEMA_VERSION.into(), record_type: "lease".into(),
            engine: "tokenzero".into(), project_id: project, operation_id: "op-1".into(),
            epoch: 1, owner: LeaseOwner { pid: 1, host: "test".into() },
            started_at: format_system_time(SystemTime::now()),
            expires_at: format_system_time(SystemTime::now() + std::time::Duration::from_secs(300)),
            grace_seconds: GC_MIN_GRACE_SECONDS, blob_hashes: vec![leased.clone()],
        }).unwrap();
        let report = run_gc(dir.path(), &GcConfig::default()).unwrap();
        assert_eq!(verdict(&report, &pinned), GcVerdict::Retain);
        assert_eq!(verdict(&report, &leased), GcVerdict::Retain);
    }

    #[test]
    fn faulted_sweep_resumes_from_progress_record() {
        let (dir, cas, _, _) = setup_rooted_store();
        let first = cas.put(b"first orphan").unwrap();
        let second = cas.put(b"second orphan").unwrap();
        let failed = run_gc(dir.path(), &GcConfig {
            run_id: "resume-1".into(), apply: true, fault_after_deletes: Some(1),
            ..GcConfig::default()
        });
        assert!(matches!(failed, Err(GcError::FaultInjected)));
        assert!(dir.path().join("gc/reports/resume-1.progress.json").is_file());
        let resumed = run_gc(dir.path(), &GcConfig {
            run_id: "resume-1".into(), apply: true, ..GcConfig::default()
        }).unwrap();
        assert!(!cas.contains(&first));
        assert!(!cas.contains(&second));
        assert!(resumed.objects.iter().filter(|o| o.blob_hash == first || o.blob_hash == second)
            .all(|o| o.evidence.iter().any(|e| e.contains("deleted by this sweep"))));
        assert!(!dir.path().join("gc/reports/resume-1.progress.json").exists());
    }

    #[test]
    fn stale_epoch_and_bad_version_fail_closed() {
        let (dir, _, project, _) = setup_rooted_store();
        let stale = publish_reachability_snapshot(
            dir.path(), "tokenzero", &project, 1, &[],
        ).unwrap_err();
        assert!(matches!(stale, GcError::SchemaViolation(_)));
        let path = dir.path().join("gc/roots/tokenzero").join(&project).join("current.json");
        fs::write(&path, br#"{"schema_version":"zerostack.cas-gc.v999","record_type":"reachability-snapshot"}"#).unwrap();
        let report = run_gc(dir.path(), &GcConfig::default()).unwrap();
        assert!(report.objects.iter().all(|object| object.verdict == GcVerdict::RetainUncertain));
    }

    #[test]
    fn malformed_metadata_is_uncertain_not_collectable() {
        let (dir, cas, project, _) = setup_rooted_store();
        let orphan = cas.put(b"uncertain orphan").unwrap();
        let pin_dir = dir.path().join("gc/pins/tokenzero").join(&project);
        fs::create_dir_all(&pin_dir).unwrap();
        fs::write(pin_dir.join("bad.json"), b"not-json").unwrap();
        let report = run_gc(dir.path(), &GcConfig::default()).unwrap();
        assert_eq!(verdict(&report, &orphan), GcVerdict::RetainUncertain);
    }

    #[test]
    fn publish_is_blocked_during_sweep_unlink_window() {
        let (dir, cas, _, _) = setup_rooted_store();
        let payload = b"race payload";
        let hash = cas.put(payload).unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let published = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let published_in_hook = Arc::clone(&published);
        let config = GcConfig {
            run_id: "race-1".into(), apply: true,
            now: SystemTime::now() + std::time::Duration::from_secs(86_400),
            before_unlink: Some(Arc::new(move |_| {
                entered_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
                assert!(!published_in_hook.load(std::sync::atomic::Ordering::SeqCst));
            })),
            ..GcConfig::default()
        };
        let gc_root = dir.path().to_path_buf();
        let gc = thread::spawn(move || run_gc(&gc_root, &config));
        entered_rx.recv().unwrap();
        let publish_root = dir.path().to_path_buf();
        let published_flag = Arc::clone(&published);
        let publisher = thread::spawn(move || {
            let result = SharedCas::open(publish_root).put(payload);
            published_flag.store(true, std::sync::atomic::Ordering::SeqCst);
            result
        });
        thread::sleep(std::time::Duration::from_millis(100));
        assert!(!published.load(std::sync::atomic::Ordering::SeqCst));
        release_tx.send(()).unwrap();
        gc.join().unwrap().unwrap();
        assert_eq!(publisher.join().unwrap().unwrap(), hash);
    }

    #[test]
    fn repair_replaces_corrupt_object_and_rejects_wrong_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let cas = SharedCas::open(dir.path().to_path_buf());
        let bytes = b"repair me";
        let hash = cas.put(bytes).unwrap();
        fs::write(cas.object_path(&hash), b"corrupt").unwrap();
        assert!(repair_object(dir.path(), &hash, bytes).unwrap());
        assert_eq!(cas.get_verified(&hash).unwrap(), bytes);
        assert!(repair_object(dir.path(), &hash, bytes).is_ok_and(|changed| !changed));
        assert!(repair_object(dir.path(), &hash, b"wrong").is_err());
    }

    #[test]
    fn writer_and_progress_bounds_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let project = project_id(dir.path()).unwrap();
        let too_many = vec!["a".repeat(64); GC_MAX_BLOB_HASHES + 1];
        assert!(matches!(
            publish_reachability_snapshot(dir.path(), "tokenzero", &project, 1, &too_many),
            Err(GcError::SchemaViolation(_))
        ));
        let lease = LeaseRecord {
            schema_version: GC_SCHEMA_VERSION.into(), record_type: "lease".into(),
            engine: "tokenzero".into(), project_id: project, operation_id: "op".into(),
            epoch: 1, owner: LeaseOwner { pid: 1, host: "test".into() },
            started_at: format_system_time(SystemTime::now()),
            expires_at: format_system_time(SystemTime::now() + std::time::Duration::from_secs(300)),
            grace_seconds: GC_MIN_GRACE_SECONDS, blob_hashes: too_many,
        };
        assert!(matches!(publish_lease_record(dir.path(), &lease), Err(GcError::SchemaViolation(_))));
        let progress = dir.path().join("gc/reports/bounds.progress.json");
        let hashes = vec!["a".repeat(64); GC_MAX_BLOB_HASHES + 1];
        fs::create_dir_all(progress.parent().unwrap()).unwrap();
        fs::write(&progress, serde_json::json!({
            "schema_version": GC_SCHEMA_VERSION,
            "record_type": "sweep-progress", "run_id": "bounds", "store_root": dir.path(),
            "evaluated_at": format_system_time(SystemTime::now()), "objects": hashes,
            "deleted": [], "state": "sweeping"
        }).to_string()).unwrap();
        assert!(matches!(run_gc(dir.path(), &GcConfig { run_id: "bounds".into(), ..GcConfig::default() }), Err(GcError::CorruptMetadata { .. })));
    }

    #[test]
    fn json_entry_counter_enforces_global_bound() {
        let mut count = GC_MAX_BLOB_HASHES;
        assert!(matches!(count_gc_json_entry(&mut count), Err(GcError::Policy(_))));
        count = GC_MAX_BLOB_HASHES - 1;
        count_gc_json_entry(&mut count).unwrap();
        assert_eq!(count, GC_MAX_BLOB_HASHES);
    }

    #[test]
    fn report_bounds_fail_closed() {
        let mut objects = Vec::with_capacity(GC_MAX_REPORT_OBJECTS + 1);
        for index in 0..=GC_MAX_REPORT_OBJECTS {
            objects.push(serde_json::json!({
                "blob_hash": format!("{index:064x}"), "verdict": "collect",
                "reason_codes": ["no-live-reference"], "evidence": ["none"]
            }));
        }
        let report = serde_json::json!({
            "schema_version": GC_SCHEMA_VERSION, "record_type": "dry-run-report",
            "run_id": "bounds", "store_root": "/tmp/store",
            "evaluated_at": "2026-01-01T00:00:00Z", "objects": objects
        });
        assert!(matches!(validate_dry_run_report(&report), Err(GcError::SchemaViolation(_))));
        let evidence = vec![serde_json::Value::String("e".into()); GC_MAX_EVIDENCE_ITEMS + 1];
        let report = serde_json::json!({
            "schema_version": GC_SCHEMA_VERSION, "record_type": "dry-run-report",
            "run_id": "bounds", "store_root": "/tmp/store",
            "evaluated_at": "2026-01-01T00:00:00Z", "objects": [{
                "blob_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "verdict": "collect", "reason_codes": ["no-live-reference"], "evidence": evidence
            }]
        });
        assert!(matches!(validate_dry_run_report(&report), Err(GcError::SchemaViolation(_))));
    }

    #[test]
    fn concurrent_atomic_writes_get_unique_temps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gc/reports/record.json");
        let mut workers = Vec::new();
        for index in 0..8 {
            let path = path.clone();
            workers.push(thread::spawn(move || {
                gc_atomic_write(&path, format!("{index}").as_bytes()).unwrap();
            }));
        }
        for worker in workers { worker.join().unwrap(); }
        assert!(path.is_file());
        assert!(fs::read_dir(path.parent().unwrap()).unwrap().all(|entry| {
            let name = entry.unwrap().file_name();
            !name.to_string_lossy().ends_with(".tmp")
        }));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_gc_namespace_is_uncertain_and_not_followed() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let cas = SharedCas::open(dir.path().to_path_buf());
        let orphan = cas.put(b"orphan outside metadata").unwrap();
        let sentinel = external.path().join("sentinel");
        fs::write(&sentinel, b"unchanged").unwrap();
        fs::create_dir_all(dir.path().join("gc")).unwrap();
        symlink(external.path(), dir.path().join("gc/roots")).unwrap();
        let result = run_gc(dir.path(), &GcConfig::default());
        assert!(matches!(result, Err(GcError::CorruptMetadata { .. })));
        assert!(publish_reachability_snapshot(
            dir.path(), "tokenzero", &"a".repeat(64), 1, std::slice::from_ref(&orphan)
        ).is_err());
        assert!(cas.contains(&orphan));
        assert!(!external.path().join("gc").exists());
        assert_eq!(fs::read(&sentinel).unwrap(), b"unchanged");
    }
}
