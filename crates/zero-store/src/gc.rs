//! Canonical `zerostack.cas-gc.v2` protocol with read-only v1 compatibility.
//!
//! This module owns only store metadata and immutable CAS lifecycle. It has no
//! engine-specific authority; engines publish roots, pins, and leases here.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;

use crate::cas::{CasError, PutOutcome, SharedCas};
use crate::fs_replace::atomic_write_file;
use crate::{LOCK_DEADLINE, LockMode, StoreLock};
use zero_abi::zbf::{
    ZBF_CONTAINER_FLAG_V1, ZBF_HEADER_LEN_V1, ZBF_MAGIC_V1, ZBF_MAX_CHILDREN_V1, ZBF_MAX_DEPTH_V1,
    ZBF_MAX_OBJECT_BYTES_V1, ZBF_SCHEMA_MAJOR_V1, ZBF_SCHEMA_MINOR_V1,
};

pub const GC_SCHEMA_VERSION_V1: &str = "zerostack.cas-gc.v1";
pub const GC_SCHEMA_VERSION: &str = "zerostack.cas-gc.v2";
/// Hard bounds keep malformed metadata from turning collection into an
/// unbounded allocation or path traversal surface.
pub const GC_MAX_RECORD_BYTES: u64 = 32 * 1024 * 1024;
pub const GC_MAX_BLOB_HASHES: usize = 65_536;
pub const GC_MAX_REPORT_OBJECTS: usize = 65_536;
pub const GC_MAX_EVIDENCE_ITEMS: usize = 256;
const GC_EVIDENCE_TRUNCATED: &str = "evidence truncated at GC_MAX_EVIDENCE_ITEMS";
pub const GC_MAX_PRODUCER_ID_BYTES: usize = 64;
pub const GC_MAX_PRODUCER_NAMESPACES: usize = 1_024;
pub const GC_MAX_OWNER_HOST_BYTES: usize = 255;
pub const GC_RECORD_TYPE_REACHABILITY: &str = "reachability-snapshot";
pub const GC_RECORD_TYPE_PIN: &str = "pin";
pub const GC_RECORD_TYPE_LEASE: &str = "lease";
pub const GC_RECORD_TYPE_DRY_RUN: &str = "gc-run-receipt";
pub const GC_RECORD_TYPE_SWEEP_PROGRESS: &str = "sweep-progress";
pub const GC_RECORD_TYPE_REPAIR: &str = "repair-receipt";
/// The only refs carrier the GC proof reads: ZBF container children.
///
/// Refs are always content-derived from verified object bytes; no metadata
/// record can widen or narrow the reachable set.
pub const GC_REFS_FORMAT: &str = "zbf-container-children";
pub const GC_MIN_GRACE_SECONDS: u64 = 60;
pub const DEFAULT_GC_REPORT_LIMIT: usize = 32;

/// Machine-readable semantics bound into every v2 GC record.
pub fn gc_contract_manifest() -> serde_json::Value {
    serde_json::json!({
        "schema_version": GC_SCHEMA_VERSION,
        "legacy_read_versions": [GC_SCHEMA_VERSION_V1],
        "legacy_read_record_types": [
            GC_RECORD_TYPE_REACHABILITY,
            GC_RECORD_TYPE_PIN,
            GC_RECORD_TYPE_LEASE
        ],
        "store": {
            "cas_layout": crate::CAS_LAYOUT,
            "cas_layout_version": crate::CAS_LAYOUT_VERSION,
            "digest": "sha256-lowercase-hex"
        },
        "producer_id": {
            "max_bytes": GC_MAX_PRODUCER_ID_BYTES,
            "grammar": "[a-z0-9][a-z0-9._-]*[a-z0-9] (single alphanumeric allowed)"
        },
        "records": {
            "reachability": GC_RECORD_TYPE_REACHABILITY,
            "pin": GC_RECORD_TYPE_PIN,
            "lease": GC_RECORD_TYPE_LEASE,
            "run_receipt": GC_RECORD_TYPE_DRY_RUN,
            "sweep_progress": GC_RECORD_TYPE_SWEEP_PROGRESS,
            "repair_receipt": GC_RECORD_TYPE_REPAIR
        },
        "bounds": {
            "record_bytes": GC_MAX_RECORD_BYTES,
            "producer_namespaces": GC_MAX_PRODUCER_NAMESPACES,
            "blob_hashes": GC_MAX_BLOB_HASHES,
            "report_objects": GC_MAX_REPORT_OBJECTS,
            "evidence_items": GC_MAX_EVIDENCE_ITEMS,
            "owner_host_bytes": GC_MAX_OWNER_HOST_BYTES,
            "minimum_grace_seconds": GC_MIN_GRACE_SECONDS
        },
        "safety": {
            "cas_publish_lock": "shared",
            "metadata_publish_lock": "exclusive",
            "collector_lock": "exclusive-through-recheck-and-mutation",
            "unknown_metadata": "retain-uncertain",
            "snapshot_epoch": "strictly-monotonic-per-producer-project",
            "repair": "quarantine-before-republish",
            "leased_publish": "lease-before-object-under-exclusive-lock",
            "expired_pin": "does-not-retain",
            "lock_namespace": "real-directory-and-regular-file-only"
        },
        "reachability": {
            "roots": "reachability-snapshot blob_hashes",
            "refs": "content-derived-from-verified-object-bytes",
            "refs_format": GC_REFS_FORMAT,
            "closure": "transitive-from-roots-pins-and-leases",
            "fail_closed": "corrupt-or-incomplete-refs-evidence-retains-uncertain-and-never-commits"
        }
    })
}

/// Lowercase SHA-256 of canonical JSON for [`gc_contract_manifest`].
pub fn gc_contract_digest_hex() -> String {
    zero_abi::contract_digest_hex(&gc_contract_manifest())
}
fn is_valid_producer_id(producer_id: &str) -> bool {
    let bytes = producer_id.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= GC_MAX_PRODUCER_ID_BYTES
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn require_gc_producer(producer_id: &str) -> Result<(), GcError> {
    if is_valid_producer_id(producer_id) {
        Ok(())
    } else {
        Err(GcError::Policy(format!(
            "invalid producer id {producer_id}"
        )))
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
#[serde(deny_unknown_fields)]
pub struct ReachabilitySnapshot {
    pub schema_version: String,
    pub record_type: String,
    /// Producer namespace. The `engine` wire key remains for v1 compatibility.
    pub engine: String,
    pub project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_contract_digest: Option<String>,
    pub epoch: u64,
    pub published_at: String,
    pub blob_hashes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinRecord {
    pub schema_version: String,
    pub record_type: String,
    /// Producer namespace. The `engine` wire key remains for v1 compatibility.
    pub engine: String,
    pub project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_contract_digest: Option<String>,
    pub pin_id: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub blob_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseOwner {
    pub pid: u64,
    pub host: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseRecord {
    pub schema_version: String,
    pub record_type: String,
    /// Producer namespace. The `engine` wire key remains for v1 compatibility.
    pub engine: String,
    pub project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_contract_digest: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GcRunState {
    Evaluated,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GcCandidate {
    pub blob_hash: String,
    pub verdict: GcVerdict,
    pub reason_codes: Vec<String>,
    pub evidence: Vec<String>,
}

/// Complete, contract-bound receipt for a dry run or applied sweep.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DryRunReport {
    pub schema_version: String,
    pub record_type: String,
    pub store_contract_digest: String,
    pub run_id: String,
    pub store_root: String,
    pub evaluated_at: String,
    pub apply: bool,
    pub state: GcRunState,
    pub objects: Vec<GcCandidate>,
    pub planned: Vec<String>,
    pub deleted: Vec<String>,
}

pub type GcRunReceipt = DryRunReport;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairReceipt {
    pub schema_version: String,
    pub record_type: String,
    pub store_contract_digest: String,
    pub producer_id: String,
    pub project_id: String,
    pub operation_id: String,
    pub blob_hash: String,
    pub repaired: bool,
    pub quarantined: bool,
    pub completed_at: String,
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
                format!(
                    "GC path component is not a real directory: {}",
                    path.display()
                ),
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

pub(crate) fn gc_join(store_root: &Path, parts: &[&str]) -> PathBuf {
    parts
        .iter()
        .fold(store_root.join("gc"), |p, part| p.join(part))
}

fn gc_record_path(store_root: &Path, subdir: &str, record: &impl GcRecord, id: &str) -> PathBuf {
    let (_, _, engine, project, _) = record.header();
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

/// Structurally extract the transitive content-derived refs of one verified
/// CAS object.
///
/// Refs are the digests of every object embedded in a ZBF container (direct
/// and nested children). The input must already be verified against its CAS
/// identity: refs are derived from content, never from metadata.
///
/// Fail-closed contract:
/// - Bytes that cannot be a ZBF object (wrong magic) are leaves with no refs.
/// - Bytes that carry the ZBF magic but violate the ZBF structure (size beyond
///   the ZBF object bound, truncated header or children, unknown flags,
///   unsupported schema, nonzero reserved bytes, payload length mismatch,
///   payload digest mismatch, or a ref set beyond [`GC_MAX_BLOB_HASHES`]) are
///   corrupt refs evidence: the referenced set is unknown, so a collector must
///   fail closed (retain uncertain) and must never collect on this object's
///   subtree.
pub fn refs_from_verified_bytes(bytes: &[u8]) -> Result<Vec<String>, String> {
    if bytes.len() < ZBF_MAGIC_V1.len() || &bytes[..ZBF_MAGIC_V1.len()] != ZBF_MAGIC_V1.as_slice() {
        return Ok(Vec::new());
    }
    let mut refs = Vec::new();
    let mut seen = BTreeSet::new();
    collect_zbf_refs(bytes, 0, &mut refs, &mut seen)?;
    Ok(refs)
}

/// Structural ZBF walk: every embedded object contributes its digest, and
/// container payloads are walked recursively under the ZBF depth bound.
///
/// Only structure needed to *name* refs is validated: magic, schema version,
/// flags, reserved bytes, payload length, and payload digest. Kind and owner
/// do not change the referenced set and are intentionally not part of refs
/// evidence. The durable profile and assembly manifest are unknown to the
/// collector and are deliberately not checked here.
fn collect_zbf_refs(
    bytes: &[u8],
    depth: u16,
    refs: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
) -> Result<(), String> {
    if depth > ZBF_MAX_DEPTH_V1 {
        return Err(format!("ZBF nesting exceeds {ZBF_MAX_DEPTH_V1}"));
    }
    if bytes.len() as u64 > ZBF_MAX_OBJECT_BYTES_V1 {
        return Err(format!(
            "ZBF object of {} bytes exceeds {ZBF_MAX_OBJECT_BYTES_V1}",
            bytes.len()
        ));
    }
    if bytes.len() < ZBF_HEADER_LEN_V1 {
        return Err(format!(
            "ZBF object shorter than {ZBF_HEADER_LEN_V1}-byte header"
        ));
    }
    if &bytes[..ZBF_MAGIC_V1.len()] != ZBF_MAGIC_V1.as_slice() {
        return Err("embedded object is missing the ZBF magic".into());
    }
    let schema_major = be_u16(&bytes[8..10]);
    let schema_minor = be_u16(&bytes[10..12]);
    if schema_major != ZBF_SCHEMA_MAJOR_V1 || schema_minor != ZBF_SCHEMA_MINOR_V1 {
        return Err(format!(
            "unsupported ZBF schema {schema_major}.{schema_minor}"
        ));
    }
    let flags = bytes[15];
    if flags & !ZBF_CONTAINER_FLAG_V1 != 0 {
        return Err(format!("unknown ZBF flags {flags:#04x}"));
    }
    if bytes[184..192].iter().any(|byte| *byte != 0) {
        return Err("ZBF reserved header bytes are nonzero".into());
    }
    let payload_len = be_u64(&bytes[16..24]);
    let expected_total = (ZBF_HEADER_LEN_V1 as u64)
        .checked_add(payload_len)
        .ok_or_else(|| "ZBF payload length overflows".to_string())?;
    if expected_total != bytes.len() as u64 {
        return Err(format!(
            "ZBF payload length {payload_len} does not match object size {}",
            bytes.len()
        ));
    }
    let payload = &bytes[ZBF_HEADER_LEN_V1..];
    if content_sha256_hex(payload) != lower_hex(&bytes[152..184]) {
        return Err("ZBF payload digest mismatch".into());
    }
    if flags == 0 {
        return Ok(());
    }
    if payload.len() < 4 {
        return Err("ZBF container payload is shorter than its child count".into());
    }
    let count = be_u32(&payload[..4]);
    if count > ZBF_MAX_CHILDREN_V1 {
        return Err(format!(
            "ZBF child count {count} exceeds {ZBF_MAX_CHILDREN_V1}"
        ));
    }
    let mut offset = 4usize;
    for _ in 0..count {
        if payload.len() - offset < 8 {
            return Err("ZBF container child length is truncated".into());
        }
        let child_len = be_u64(&payload[offset..offset + 8]);
        offset += 8;
        if child_len < ZBF_HEADER_LEN_V1 as u64 {
            return Err("ZBF child is shorter than the fixed header".into());
        }
        let child_len = usize::try_from(child_len)
            .map_err(|_| "ZBF child length does not fit usize".to_string())?;
        if payload.len() - offset < child_len {
            return Err("ZBF container child bytes are truncated".into());
        }
        let child = &payload[offset..offset + child_len];
        offset += child_len;
        let child_hash = content_sha256_hex(child);
        if !seen.contains(&child_hash) {
            if seen.len() >= GC_MAX_BLOB_HASHES {
                return Err(format!("refs exceed {GC_MAX_BLOB_HASHES}"));
            }
            seen.insert(child_hash.clone());
            refs.push(child_hash);
        }
        collect_zbf_refs(child, depth + 1, refs, seen)?;
    }
    if offset != payload.len() {
        return Err("ZBF container payload has trailing bytes".into());
    }
    Ok(())
}

fn be_u16(bytes: &[u8]) -> u16 {
    (u16::from(bytes[0]) << 8) | u16::from(bytes[1])
}

fn be_u32(bytes: &[u8]) -> u32 {
    (u32::from(bytes[0]) << 24)
        | (u32::from(bytes[1]) << 16)
        | (u32::from(bytes[2]) << 8)
        | u32::from(bytes[3])
}

fn be_u64(bytes: &[u8]) -> u64 {
    (u64::from(bytes[0]) << 56)
        | (u64::from(bytes[1]) << 48)
        | (u64::from(bytes[2]) << 40)
        | (u64::from(bytes[3]) << 32)
        | (u64::from(bytes[4]) << 24)
        | (u64::from(bytes[5]) << 16)
        | (u64::from(bytes[6]) << 8)
        | u64::from(bytes[7])
}

fn is_valid_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

pub(crate) fn is_valid_pin_id(s: &str) -> bool {
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
    fn header(&self) -> (&str, &str, &str, &str, Option<&str>);
}

macro_rules! impl_gc_record {
    ($T:ty) => {
        impl GcRecord for $T {
            fn header(&self) -> (&str, &str, &str, &str, Option<&str>) {
                (
                    &self.schema_version,
                    &self.record_type,
                    &self.engine,
                    &self.project_id,
                    self.store_contract_digest.as_deref(),
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
        return Err(corrupt(
            path,
            format!("metadata exceeds {GC_MAX_RECORD_BYTES} bytes"),
        ));
    }
    let file = File::open(path).map_err(GcError::Io)?;
    let capacity = metadata.len().min(GC_MAX_RECORD_BYTES) as usize + 1;
    let mut bytes = Vec::with_capacity(capacity);
    file.take(GC_MAX_RECORD_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(GcError::Io)?;
    if bytes.len() as u64 > GC_MAX_RECORD_BYTES {
        return Err(corrupt(
            path,
            format!("metadata exceeds {GC_MAX_RECORD_BYTES} bytes"),
        ));
    }
    serde_json::from_slice(&bytes).map_err(GcError::Json)
}

struct BoundedJsonBuffer {
    bytes: Vec<u8>,
    max: usize,
    exceeded: bool,
}

impl Write for BoundedJsonBuffer {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        let available = self.max.saturating_sub(self.bytes.len());
        if input.len() > available {
            self.exceeded = true;
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "serialized GC record exceeds byte bound",
            ));
        }
        self.bytes.extend_from_slice(input);
        Ok(input.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn serialize_gc_json<T: Serialize>(value: &T) -> Result<Vec<u8>, GcError> {
    let mut output = BoundedJsonBuffer {
        bytes: Vec::new(),
        max: GC_MAX_RECORD_BYTES as usize,
        exceeded: false,
    };
    if let Err(error) = serde_json::to_writer_pretty(&mut output, value) {
        if output.exceeded {
            return Err(GcError::Policy(format!(
                "serialized GC record exceeds {GC_MAX_RECORD_BYTES} bytes"
            )));
        }
        return Err(GcError::Json(error));
    }
    Ok(output.bytes)
}

fn write_gc_json<T: Serialize>(path: &Path, value: &T) -> Result<(), GcError> {
    gc_atomic_write(path, &serialize_gc_json(value)?).map_err(GcError::Io)
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
    let (schema_version, record_type, producer_id, project_id, contract_digest) = record.header();
    if record_type != expected_type {
        return Err(corrupt(path, format!("record_type {record_type}")));
    }
    match schema_version {
        GC_SCHEMA_VERSION_V1 if contract_digest.is_none() => {}
        GC_SCHEMA_VERSION => {
            let expected = gc_contract_digest_hex();
            if contract_digest != Some(expected.as_str()) {
                return Err(corrupt(path, "store_contract_digest mismatch".into()));
            }
        }
        GC_SCHEMA_VERSION_V1 => {
            return Err(corrupt(
                path,
                "legacy record unexpectedly binds a v2 store contract".into(),
            ));
        }
        _ => {
            return Err(corrupt(
                path,
                format!("unsupported schema_version {schema_version}"),
            ));
        }
    }
    if !is_valid_producer_id(producer_id) {
        return Err(corrupt(path, format!("invalid producer id {producer_id}")));
    }
    if !is_valid_hash(project_id) {
        return Err(corrupt(path, "invalid project_id".into()));
    }
    validate_namespace(path, producer_id, project_id)
}

fn read_reachability_snapshot(path: &Path) -> Result<ReachabilitySnapshot, GcError> {
    let snap: ReachabilitySnapshot = read_gc_json(path)?;
    validate_record_common(&snap, path, GC_RECORD_TYPE_REACHABILITY)?;
    require_min(snap.epoch, 1, path, "epoch")?;
    require_rfc3339(&snap.published_at, path, "published_at")?;
    if snap.blob_hashes.len() > GC_MAX_BLOB_HASHES {
        return Err(corrupt(
            path,
            format!("too many blob hashes (max {GC_MAX_BLOB_HASHES})"),
        ));
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
    let created_at =
        parse_rfc3339(&pin.created_at).ok_or_else(|| corrupt(path, "invalid created_at".into()))?;
    if let Some(exp) = pin.expires_at.as_deref() {
        let expires_at =
            parse_rfc3339(exp).ok_or_else(|| corrupt(path, "invalid expires_at".into()))?;
        if expires_at < created_at {
            return Err(corrupt(path, "expires_at precedes created_at".into()));
        }
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
    if lease.owner.pid == 0 {
        return Err(corrupt(path, "owner.pid must be >= 1".into()));
    }
    if lease.owner.host.is_empty()
        || lease.owner.host.len() > GC_MAX_OWNER_HOST_BYTES
        || lease.owner.host.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(corrupt(path, "invalid owner.host".into()));
    }
    let started_at = parse_rfc3339(&lease.started_at)
        .ok_or_else(|| corrupt(path, "invalid started_at".into()))?;
    let expires_at = parse_rfc3339(&lease.expires_at)
        .ok_or_else(|| corrupt(path, "invalid expires_at".into()))?;
    if expires_at < started_at {
        return Err(corrupt(path, "expires_at precedes started_at".into()));
    }
    require_min(
        lease.grace_seconds,
        GC_MIN_GRACE_SECONDS,
        path,
        "grace_seconds",
    )?;
    if lease.blob_hashes.len() > GC_MAX_BLOB_HASHES {
        return Err(corrupt(
            path,
            format!("too many blob hashes (max {GC_MAX_BLOB_HASHES})"),
        ));
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
    global_evidence: BTreeSet<String>,
}

fn push_bounded(values: &mut Vec<String>, value: String) {
    if values.iter().any(|existing| existing == &value) {
        return;
    }
    if values.len() < GC_MAX_EVIDENCE_ITEMS {
        values.push(value);
    } else if !values
        .iter()
        .any(|existing| existing == GC_EVIDENCE_TRUNCATED)
    {
        values[GC_MAX_EVIDENCE_ITEMS - 1] = GC_EVIDENCE_TRUNCATED.into();
    }
}

fn push_bounded_set(values: &mut BTreeSet<String>, value: String) {
    values.insert(value);
    if values.len() <= GC_MAX_EVIDENCE_ITEMS {
        return;
    }
    values.insert(GC_EVIDENCE_TRUNCATED.into());
    while values.len() > GC_MAX_EVIDENCE_ITEMS {
        let removable = values
            .iter()
            .rev()
            .find(|item| item.as_str() != GC_EVIDENCE_TRUNCATED)
            .cloned()
            .expect("evidence bound always leaves a removable item");
        values.remove(&removable);
    }
}

fn mark_hash(state: &mut MarkState, hash: &str, reason: &str, evidence: &str) {
    if state.live.len() >= GC_MAX_REPORT_OBJECTS && !state.live.contains_key(hash) {
        mark_uncertain(
            state,
            format!("live hash traversal exceeded {GC_MAX_REPORT_OBJECTS}"),
        );
        return;
    }
    let meta = state.live.entry(hash.to_string()).or_default();
    meta.0.insert(reason.to_string());
    push_bounded_set(&mut meta.1, evidence.to_string());
}

fn mark_uncertain(state: &mut MarkState, evidence: String) {
    state.uncertain = true;
    push_bounded_set(&mut state.global_evidence, evidence);
}

const GC_MAX_PROJECT_NAMESPACES: usize = GC_MAX_REPORT_OBJECTS;

/// Require a real (non-symlink) directory entry with a UTF-8 name. The noun
/// keeps producer vs project error texts distinct.
fn require_dir_utf8_name(entry: &fs::DirEntry, noun: &str) -> Result<String, GcError> {
    if !entry.file_type()?.is_dir() {
        return Err(corrupt(
            &entry.path(),
            format!("GC {noun} namespace is not a real directory"),
        ));
    }
    let name = entry.file_name();
    let name = name
        .to_str()
        .ok_or_else(|| corrupt(&entry.path(), format!("GC {noun} namespace is not UTF-8")))?;
    Ok(name.to_string())
}

/// Walk the project namespaces under one producer directory, enforcing the
/// shared project id and traversal-cap rules.
fn walk_project_namespaces(
    engine_dir: &Path,
    project_count: &mut usize,
    f: &mut impl FnMut(&Path) -> Result<(), GcError>,
) -> Result<(), GcError> {
    for project_entry in fs::read_dir(engine_dir)? {
        let project_entry = project_entry?;
        let project_id = require_dir_utf8_name(&project_entry, "project")?;
        if !is_valid_hash(&project_id) {
            return Err(corrupt(
                &project_entry.path(),
                "invalid project_id namespace".into(),
            ));
        }
        *project_count = project_count.saturating_add(1);
        if *project_count > GC_MAX_PROJECT_NAMESPACES {
            return Err(GcError::Policy(format!(
                "GC project traversal exceeds {GC_MAX_PROJECT_NAMESPACES}"
            )));
        }
        f(&project_entry.path())?;
    }
    Ok(())
}

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
    let mut producer_count = 0usize;
    let mut project_count = 0usize;
    for engine_entry in fs::read_dir(&dir)? {
        let engine_entry = engine_entry?;
        let producer_id = require_dir_utf8_name(&engine_entry, "producer")?;
        if !is_valid_producer_id(&producer_id) {
            return Err(corrupt(
                &engine_entry.path(),
                format!("invalid producer id {producer_id}"),
            ));
        }
        producer_count = producer_count.saturating_add(1);
        if producer_count > GC_MAX_PRODUCER_NAMESPACES {
            return Err(GcError::Policy(format!(
                "GC producer traversal exceeds {GC_MAX_PRODUCER_NAMESPACES}"
            )));
        }
        walk_project_namespaces(&engine_entry.path(), &mut project_count, &mut f)?;
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
                .is_some_and(|expires_at| expires_at <= now)
            {
                return;
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

/// Load reachability roots into the mark state. Missing roots metadata is
/// uncertain, never a free pass to collect.
fn load_reachability_roots(store_root: &Path, state: &mut MarkState) -> Result<(), GcError> {
    if !store_root.join("gc").join("roots").is_dir() {
        mark_uncertain(
            state,
            "missing gc/roots directory; reachability metadata absent".into(),
        );
        return Ok(());
    }
    let mut saw_any_project = false;
    walk_gc_projects(store_root, "roots", |project_dir| {
        saw_any_project = true;
        let current = project_dir.join("current.json");
        if !current.is_file() {
            mark_uncertain(
                state,
                format!("missing reachability snapshot {}", current.display()),
            );
            return Ok(());
        }
        match read_reachability_snapshot(&current) {
            Ok(snap) => {
                let evidence = format!("root {} epoch {}", current.display(), snap.epoch);
                for h in &snap.blob_hashes {
                    mark_hash(state, h, "reachability-root", &evidence);
                }
            }
            Err(err) => mark_uncertain(state, format!("{}: {err}", current.display())),
        }
        Ok(())
    })?;
    if !saw_any_project {
        mark_uncertain(
            state,
            "gc/roots has no project namespaces; reachability metadata absent".into(),
        );
    }
    Ok(())
}

/// Apply one lease to the mark state: active and in-grace leases retain their
/// hashes; stale leases outside grace retain with unverified-liveness
/// uncertainty.
fn apply_lease_liveness(
    path: &Path,
    lease: &LeaseRecord,
    state: &mut MarkState,
    now: SystemTime,
    grace_seconds: u64,
) {
    let expires = parse_rfc3339(&lease.expires_at).unwrap_or(now);
    let grace_end = expires.checked_add(std::time::Duration::from_secs(
        lease.grace_seconds.max(grace_seconds),
    ));
    let active = now <= expires;
    let in_grace = !active && grace_end.is_none_or(|end| now < end);
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
}

fn load_mark_state(
    store_root: &Path,
    cas: &SharedCas,
    now: SystemTime,
    grace_seconds: u64,
) -> Result<MarkState, GcError> {
    let mut state = MarkState::default();
    load_reachability_roots(store_root, &mut state)?;
    load_all_pins(store_root, &mut state, now)?;
    walk_gc_records(
        store_root,
        "leases",
        &mut state,
        read_lease_record,
        |path, lease, state| apply_lease_liveness(path, &lease, state, now, grace_seconds),
    )?;
    trace_refs(cas, &mut state)?;
    Ok(state)
}

/// Extend liveness to the transitive content-derived refs closure.
///
/// Every live seed (reachability root, pin, or lease) is read once and
/// verified, then its embedded child digests are marked live with `ref-child`
/// evidence. Fail-closed evidence rules:
/// - A seed absent from the CAS is incomplete evidence: its refs cannot be
///   evaluated, so the run is uncertain and collection must not commit.
/// - A seed that cannot be verified (digest mismatch, non-regular entry, I/O
///   or policy failure) is corrupt evidence: the run is uncertain.
/// - Verified bytes carrying the ZBF magic but violating the ZBF structure are
///   corrupt refs evidence: the referenced set is unknown, so the run is
///   uncertain. No size shortcut is used: a shrunken or oversized corrupt
///   file is indistinguishable from a leaf without reading and verifying it.
fn trace_refs(cas: &SharedCas, state: &mut MarkState) -> Result<(), GcError> {
    let seeds: Vec<String> = state.live.keys().cloned().collect();
    for seed in seeds {
        let bytes = match cas.get_verified(&seed) {
            Ok(bytes) => bytes,
            Err(CasError::NotFound) => {
                mark_uncertain(
                    state,
                    format!("reachable seed {seed} missing from CAS; refs evidence incomplete"),
                );
                continue;
            }
            Err(error) => {
                mark_uncertain(state, format!("refs seed {seed} unreadable: {error}"));
                continue;
            }
        };
        match refs_from_verified_bytes(&bytes) {
            Ok(refs) => {
                for child in refs {
                    mark_hash(state, &child, "ref-child", &format!("ref from {seed}"));
                }
            }
            Err(reason) => {
                mark_uncertain(state, format!("refs evidence corrupt for {seed}: {reason}"));
            }
        }
    }
    Ok(())
}

fn build_dry_run_report(
    store_root: &Path,
    run_id: &str,
    cas: &SharedCas,
    state: &MarkState,
    min_age_seconds: u64,
    now: SystemTime,
    apply: bool,
) -> Result<DryRunReport, GcError> {
    let mut objects = Vec::new();
    for hash in cas.list_objects_bounded(GC_MAX_REPORT_OBJECTS)? {
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
                state.global_evidence.iter().cloned().collect(),
            )
        } else {
            let young = fs::metadata(cas.object_path(&hash))
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .map(|modified| {
                    now.duration_since(modified).unwrap_or_default().as_secs() < min_age_seconds
                })
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
                    vec!["no reachable root, pin, lease, or ref".into()],
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
        return Err(GcError::Policy(format!(
            "report exceeds {GC_MAX_REPORT_OBJECTS} objects"
        )));
    }
    objects.sort_by(|left, right| left.blob_hash.cmp(&right.blob_hash));
    let planned = objects
        .iter()
        .filter(|object| object.verdict == GcVerdict::Collect)
        .map(|object| object.blob_hash.clone())
        .collect();
    Ok(DryRunReport {
        schema_version: GC_SCHEMA_VERSION.to_string(),
        record_type: GC_RECORD_TYPE_DRY_RUN.to_string(),
        store_contract_digest: gc_contract_digest_hex(),
        run_id: run_id.to_string(),
        store_root: crate::store_root::absolutize(store_root)
            .to_string_lossy()
            .into_owned(),
        evaluated_at: format_system_time(now),
        apply,
        state: GcRunState::Evaluated,
        objects,
        planned,
        deleted: Vec::new(),
    })
}

fn sweep_plan_digest(run_id: &str, store_root: &str, objects: &[String]) -> String {
    zero_abi::contract_digest_hex(&serde_json::json!({
        "domain": "zerostack.gc-sweep-plan.v1",
        "store_contract_digest": gc_contract_digest_hex(),
        "run_id": run_id,
        "store_root": store_root,
        "objects": objects,
    }))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SweepProgress {
    schema_version: String,
    record_type: String,
    store_contract_digest: String,
    run_id: String,
    store_root: String,
    evaluated_at: String,
    plan_digest: String,
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
    if progress.store_contract_digest != gc_contract_digest_hex() {
        return Err(corrupt(path, "store_contract_digest mismatch".into()));
    }
    validate_run_id(&progress.run_id).map_err(|error| corrupt(path, error.to_string()))?;
    if progress.store_root.is_empty() {
        return Err(corrupt(path, "store_root empty".into()));
    }
    require_rfc3339(&progress.evaluated_at, path, "evaluated_at")?;
    if progress.state != "sweeping" {
        return Err(corrupt(
            path,
            format!("invalid sweep state {}", progress.state),
        ));
    }
    if progress.objects.len() > GC_MAX_REPORT_OBJECTS {
        return Err(corrupt(
            path,
            format!("too many sweep objects (max {GC_MAX_REPORT_OBJECTS})"),
        ));
    }
    if progress.deleted.len() > progress.objects.len() {
        return Err(corrupt(path, "deleted set exceeds sweep object set".into()));
    }
    let object_set: BTreeSet<_> = progress.objects.iter().cloned().collect();
    let deleted_set: BTreeSet<_> = progress.deleted.iter().cloned().collect();
    if object_set.len() != progress.objects.len() || deleted_set.len() != progress.deleted.len() {
        return Err(corrupt(path, "duplicate sweep hash".into()));
    }
    if !deleted_set.is_subset(&object_set) {
        return Err(corrupt(path, "deleted hash is outside sweep plan".into()));
    }
    for hash in &progress.objects {
        require_hash(hash, path, "blob hash")?;
    }
    let expected_plan =
        sweep_plan_digest(&progress.run_id, &progress.store_root, &progress.objects);
    if progress.plan_digest != expected_plan {
        return Err(corrupt(path, "sweep plan digest mismatch".into()));
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
        if !entry.file_type()?.is_file() {
            continue;
        }
        if !name.ends_with(".json") {
            continue;
        }
        if name.ends_with(".progress.json") {
            continue;
        }
        let modified = entry.metadata()?.modified().unwrap_or(UNIX_EPOCH);
        if reports.len() >= GC_MAX_REPORT_OBJECTS {
            return Err(GcError::Policy(format!(
                "GC report namespace exceeds {GC_MAX_REPORT_OBJECTS} records"
            )));
        }
        reports.push((modified, name.to_owned(), path));
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
    let store_root_key = crate::store_root::absolutize(store_root)
        .to_string_lossy()
        .into_owned();
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

    let state = load_mark_state(store_root, &cas, config.now, config.grace_seconds)?;
    let report = build_dry_run_report(
        store_root,
        &config.run_id,
        &cas,
        &state,
        config.min_age_seconds,
        config.now,
        config.apply,
    )?;
    let report_path = gc_join(store_root, &["reports", &format!("{}.json", config.run_id)]);
    write_gc_json(&report_path, &report)?;
    prune_gc_reports(store_root, config.report_limit, &report_path)?;
    if !config.apply {
        return Ok(report);
    }

    let current_to_delete: Vec<String> = report
        .objects
        .iter()
        .filter(|object| object.verdict == GcVerdict::Collect)
        .map(|object| object.blob_hash.clone())
        .collect();
    let mut deleted = Vec::new();
    if let Some(progress) = prior_progress.as_ref() {
        for hash in &progress.deleted {
            match fs::symlink_metadata(cas.object_path(hash)) {
                Ok(metadata) if metadata.file_type().is_file() => {}
                Ok(_) => {
                    return Err(GcError::CorruptMetadata {
                        path: cas.object_path(hash),
                        reason: "sweep progress deletion replaced by a non-regular entry".into(),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    deleted.push(hash.clone());
                }
                Err(error) => return Err(GcError::Io(error)),
            }
        }
    }
    let (to_delete, plan_digest) = if let Some(progress) = prior_progress.as_ref() {
        let deleted_set: BTreeSet<_> = deleted.iter().collect();
        let expected_remaining: Vec<_> = progress
            .objects
            .iter()
            .filter(|hash| !deleted_set.contains(hash))
            .cloned()
            .collect();
        if expected_remaining != current_to_delete {
            return Err(GcError::SchemaViolation(
                "existing sweep progress does not match the current remaining plan".into(),
            ));
        }
        (progress.objects.clone(), progress.plan_digest.clone())
    } else {
        let digest = sweep_plan_digest(&config.run_id, &store_root_key, &current_to_delete);
        (current_to_delete, digest)
    };
    let persist = |deleted: &[String]| -> Result<(), GcError> {
        write_gc_json(
            &progress_path,
            &SweepProgress {
                schema_version: GC_SCHEMA_VERSION.to_string(),
                record_type: GC_RECORD_TYPE_SWEEP_PROGRESS.to_string(),
                store_contract_digest: gc_contract_digest_hex(),
                run_id: config.run_id.clone(),
                store_root: store_root_key.clone(),
                evaluated_at: report.evaluated_at.clone(),
                plan_digest: plan_digest.clone(),
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
        let re_state = load_mark_state(store_root, &cas, config.now, config.grace_seconds)?;
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
    final_report.state = GcRunState::Complete;
    final_report.planned = to_delete.clone();
    final_report.deleted = deleted.clone();
    for obj in &mut final_report.objects {
        if obj.verdict != GcVerdict::Collect {
            continue;
        }
        if deleted_set.contains(&obj.blob_hash) {
            push_bounded(&mut obj.evidence, "deleted by this sweep".into());
            continue;
        }
        obj.verdict = GcVerdict::RetainUncertain;
        obj.reason_codes = vec!["uncertain-metadata".into()];
        obj.evidence = vec!["re-check before delete showed a live reference or uncertainty".into()];
    }
    let final_value = serde_json::to_value(&final_report)?;
    validate_dry_run_report(&final_value)?;
    write_gc_json(&report_path, &final_report)?;
    prune_gc_reports(store_root, config.report_limit, &report_path)?;
    remove_gc_record(&progress_path)?;
    Ok(final_report)
}

const DRY_RUN_FIELDS: &[&str] = &[
    "schema_version",
    "record_type",
    "store_contract_digest",
    "run_id",
    "store_root",
    "evaluated_at",
    "apply",
    "state",
    "objects",
    "planned",
    "deleted",
];
const CANDIDATE_FIELDS: &[&str] = &["blob_hash", "verdict", "reason_codes", "evidence"];
const REPAIR_RECEIPT_FIELDS: &[&str] = &[
    "schema_version",
    "record_type",
    "store_contract_digest",
    "producer_id",
    "project_id",
    "operation_id",
    "blob_hash",
    "repaired",
    "quarantined",
    "completed_at",
];
const REASON_CODES: &[&str] = &[
    "reachability-root",
    "ref-child",
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
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| GcError::SchemaViolation(field.into()))?;
    if items.len() > GC_MAX_EVIDENCE_ITEMS {
        return Err(GcError::SchemaViolation(format!(
            "{field} exceeds {GC_MAX_EVIDENCE_ITEMS} items"
        )));
    }
    let reasons = field == "reason_codes";
    if reasons && items.is_empty() {
        return Err(GcError::SchemaViolation("reason_codes empty".into()));
    }
    let mut seen = BTreeSet::new();
    for item in items {
        let item = item.as_str().ok_or_else(|| {
            GcError::SchemaViolation(if reasons { "reason_code" } else { "evidence" }.into())
        })?;
        if !reasons && item.is_empty() {
            return Err(GcError::SchemaViolation("empty evidence".into()));
        }
        if allow.is_some_and(|allowed| !allowed.contains(&item)) {
            return Err(GcError::SchemaViolation(format!("reason_code {item}")));
        }
        if !seen.insert(item) {
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

/// Validate one bounded, deduplicated hash list field (`planned` / `deleted`)
/// and return its set. Cross-set invariants stay with the caller.
fn validate_hash_list(
    value: &serde_json::Value,
    field: &str,
) -> Result<BTreeSet<String>, GcError> {
    let items = value
        .get(field)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| GcError::SchemaViolation(field.into()))?;
    if items.len() > GC_MAX_REPORT_OBJECTS {
        return Err(GcError::SchemaViolation(format!(
            "{field} exceeds {GC_MAX_REPORT_OBJECTS}"
        )));
    }
    let mut seen = BTreeSet::new();
    for hash in items {
        let hash = hash
            .as_str()
            .ok_or_else(|| GcError::SchemaViolation(format!("{field} hash")))?;
        if !is_valid_hash(hash) || !seen.insert(hash.to_string()) {
            return Err(GcError::SchemaViolation(format!(
                "invalid or duplicate {field} hash"
            )));
        }
    }
    Ok(seen)
}

/// Validate one candidate object entry: exact keys, unique valid hash,
/// verdict vocabulary, and its bounded reason/evidence lists.
fn validate_candidate_object(
    object: &serde_json::Value,
    seen_hashes: &mut BTreeSet<String>,
    collect_hashes: &mut BTreeSet<String>,
) -> Result<(), GcError> {
    exact_keys(object, CANDIDATE_FIELDS, "extra object keys")?;
    let blob_hash = require_str(object, "blob_hash")?;
    if !is_valid_hash(blob_hash) || !seen_hashes.insert(blob_hash.to_string()) {
        return Err(GcError::SchemaViolation(
            "invalid or duplicate blob_hash".into(),
        ));
    }
    let object_verdict = require_str(object, "verdict")?;
    if !matches!(object_verdict, "retain" | "collect" | "retain-uncertain") {
        return Err(GcError::SchemaViolation("verdict".into()));
    }
    if object_verdict == "collect" {
        collect_hashes.insert(blob_hash.to_string());
    }
    validate_list(object, "reason_codes", Some(REASON_CODES))?;
    validate_list(object, "evidence", None)?;
    Ok(())
}

pub fn validate_dry_run_report(value: &serde_json::Value) -> Result<(), GcError> {
    serialize_gc_json(value)?;
    exact_keys(value, DRY_RUN_FIELDS, "extra top-level keys")?;
    if value
        .get("schema_version")
        .and_then(serde_json::Value::as_str)
        != Some(GC_SCHEMA_VERSION)
    {
        return Err(GcError::SchemaViolation("schema_version".into()));
    }
    if value.get("record_type").and_then(serde_json::Value::as_str) != Some(GC_RECORD_TYPE_DRY_RUN)
    {
        return Err(GcError::SchemaViolation("record_type".into()));
    }
    if require_str(value, "store_contract_digest")? != gc_contract_digest_hex() {
        return Err(GcError::SchemaViolation("store_contract_digest".into()));
    }
    validate_run_id(require_str(value, "run_id")?)?;
    if require_str(value, "store_root")?.is_empty() {
        return Err(GcError::SchemaViolation("store_root empty".into()));
    }
    if parse_rfc3339(require_str(value, "evaluated_at")?).is_none() {
        return Err(GcError::SchemaViolation("evaluated_at".into()));
    }
    let apply = value
        .get("apply")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| GcError::SchemaViolation("apply".into()))?;
    let state = require_str(value, "state")?;
    if !matches!(state, "evaluated" | "complete") {
        return Err(GcError::SchemaViolation("state".into()));
    }
    if !apply && state == "complete" {
        return Err(GcError::SchemaViolation(
            "dry run cannot have complete state".into(),
        ));
    }
    let objects = value
        .get("objects")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| GcError::SchemaViolation("objects".into()))?;
    if objects.len() > GC_MAX_REPORT_OBJECTS {
        return Err(GcError::SchemaViolation(format!(
            "objects exceeds {GC_MAX_REPORT_OBJECTS}"
        )));
    }
    let mut seen_hashes = BTreeSet::new();
    let mut collect_hashes = BTreeSet::new();
    for object in objects {
        validate_candidate_object(object, &mut seen_hashes, &mut collect_hashes)?;
    }
    let seen_planned = validate_hash_list(value, "planned")?;
    if state == "evaluated" && seen_planned != collect_hashes {
        return Err(GcError::SchemaViolation(
            "evaluated planned set differs from collect candidates".into(),
        ));
    }
    if !collect_hashes.is_subset(&seen_planned) {
        return Err(GcError::SchemaViolation(
            "collect candidate is absent from planned set".into(),
        ));
    }
    let seen_deleted = validate_hash_list(value, "deleted")?;
    if !seen_deleted.is_subset(&seen_planned) {
        return Err(GcError::SchemaViolation(
            "deleted hash is absent from planned set".into(),
        ));
    }
    if seen_planned
        .iter()
        .any(|hash| !seen_hashes.contains(hash) && !seen_deleted.contains(hash))
    {
        return Err(GcError::SchemaViolation(
            "planned hash is absent from both objects and deleted".into(),
        ));
    }
    if (!apply || state != "complete") && !seen_deleted.is_empty() {
        return Err(GcError::SchemaViolation(
            "only a complete applied run may report deletions".into(),
        ));
    }
    Ok(())
}

/// Digest every frozen run-receipt field using canonical JSON.
pub fn gc_report_digest_hex(report: &DryRunReport) -> Result<String, GcError> {
    serialize_gc_json(report)?;
    let value = serde_json::to_value(report)?;
    validate_dry_run_report(&value)?;
    Ok(zero_abi::contract_digest_hex(&value))
}

/// Validate one immutable repair receipt against the frozen GC contract.
pub fn validate_repair_receipt(value: &serde_json::Value) -> Result<(), GcError> {
    serialize_gc_json(value)?;
    exact_keys(value, REPAIR_RECEIPT_FIELDS, "extra repair receipt keys")?;
    if require_str(value, "schema_version")? != GC_SCHEMA_VERSION {
        return Err(GcError::SchemaViolation("schema_version".into()));
    }
    if require_str(value, "record_type")? != GC_RECORD_TYPE_REPAIR {
        return Err(GcError::SchemaViolation("record_type".into()));
    }
    if require_str(value, "store_contract_digest")? != gc_contract_digest_hex() {
        return Err(GcError::SchemaViolation("store_contract_digest".into()));
    }
    require_gc_producer(require_str(value, "producer_id")?)?;
    if !is_valid_hash(require_str(value, "project_id")?) {
        return Err(GcError::SchemaViolation("project_id".into()));
    }
    require_schema_field(
        is_valid_pin_id(require_str(value, "operation_id")?),
        "operation_id",
    )?;
    if !is_valid_hash(require_str(value, "blob_hash")?) {
        return Err(GcError::SchemaViolation("blob_hash".into()));
    }
    let repaired = value
        .get("repaired")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| GcError::SchemaViolation("repaired".into()))?;
    let quarantined = value
        .get("quarantined")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| GcError::SchemaViolation("quarantined".into()))?;
    if quarantined && !repaired {
        return Err(GcError::SchemaViolation(
            "quarantined repair receipt must report repaired".into(),
        ));
    }
    if parse_rfc3339(require_str(value, "completed_at")?).is_none() {
        return Err(GcError::SchemaViolation("completed_at".into()));
    }
    Ok(())
}

/// Digest every frozen repair-receipt field using canonical JSON.
pub fn gc_repair_receipt_digest_hex(receipt: &RepairReceipt) -> Result<String, GcError> {
    serialize_gc_json(receipt)?;
    let value = serde_json::to_value(receipt)?;
    validate_repair_receipt(&value)?;
    Ok(zero_abi::contract_digest_hex(&value))
}

/// Publish one complete producer-owned reachability set.
///
/// An empty `blob_hashes` slice is an explicit declaration that this producer
/// and project retain no CAS objects. Epochs are strictly monotonic, including
/// across legacy v1 records.
/// Require that publishing at `path` would move the reachability epoch
/// strictly forward. A missing snapshot admits any epoch >= 1.
fn require_strictly_newer_epoch(path: &Path, epoch: u64) -> Result<(), GcError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => Err(corrupt(
            path,
            "reachability snapshot is not a regular file".into(),
        )),
        Ok(_) => {
            let existing = read_reachability_snapshot(path)?;
            if epoch <= existing.epoch {
                return Err(GcError::SchemaViolation(format!(
                    "epoch {epoch} must be strictly greater than current {}",
                    existing.epoch
                )));
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(GcError::Io(error)),
    }
}

pub fn publish_reachability_snapshot(
    store_root: &Path,
    producer_id: &str,
    project_id: &str,
    epoch: u64,
    blob_hashes: &[String],
) -> Result<PathBuf, GcError> {
    require_gc_producer(producer_id)?;
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
    if let Some(hash) = blob_hashes.iter().find(|hash| !is_valid_hash(hash)) {
        return Err(GcError::Policy(format!("invalid hash {hash}")));
    }
    let _coord = StoreLock::sweep(store_root, LOCK_DEADLINE).map_err(GcError::Io)?;
    let path = gc_join(
        store_root,
        &["roots", producer_id, project_id, "current.json"],
    );
    require_strictly_newer_epoch(&path, epoch)?;
    let mut hashes = blob_hashes.to_vec();
    hashes.sort_unstable();
    hashes.dedup();
    write_gc_json(
        &path,
        &ReachabilitySnapshot {
            schema_version: GC_SCHEMA_VERSION.to_string(),
            record_type: GC_RECORD_TYPE_REACHABILITY.to_string(),
            engine: producer_id.to_string(),
            project_id: project_id.to_string(),
            store_contract_digest: Some(gc_contract_digest_hex()),
            epoch,
            published_at: format_system_time(SystemTime::now()),
            blob_hashes: hashes,
        },
    )?;
    Ok(path)
}

/// Read the current validated snapshot for one producer and project.
pub fn current_reachability_snapshot(
    store_root: &Path,
    producer_id: &str,
    project_id: &str,
) -> Result<Option<ReachabilitySnapshot>, GcError> {
    require_gc_producer(producer_id)?;
    require_schema_field(is_valid_hash(project_id), "project_id")?;
    let path = gc_join(
        store_root,
        &["roots", producer_id, project_id, "current.json"],
    );
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            read_reachability_snapshot(&path).map(Some)
        }
        Ok(_) => Err(corrupt(
            &path,
            "reachability snapshot is not a regular file".into(),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(GcError::Io(error)),
    }
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

fn require_store_contract(contract_digest: Option<&str>) -> Result<(), GcError> {
    require_schema_field(
        contract_digest == Some(gc_contract_digest_hex().as_str()),
        "store_contract_digest",
    )
}

fn validate_pin_record(store_root: &Path, pin: &PinRecord) -> Result<PathBuf, GcError> {
    require_schema(&pin.schema_version, &pin.record_type, GC_RECORD_TYPE_PIN)?;
    require_store_contract(pin.store_contract_digest.as_deref())?;
    require_gc_producer(&pin.engine)?;
    require_schema_field(is_valid_hash(&pin.project_id), "project_id")?;
    require_schema_field(is_valid_pin_id(&pin.pin_id), "pin_id")?;
    require_schema_field(is_valid_hash(&pin.blob_hash), "blob_hash")?;
    let created_at = parse_rfc3339(&pin.created_at)
        .ok_or_else(|| GcError::SchemaViolation("created_at".into()))?;
    if let Some(expires_at) = pin.expires_at.as_deref() {
        let expires_at = parse_rfc3339(expires_at)
            .ok_or_else(|| GcError::SchemaViolation("expires_at".into()))?;
        require_schema_field(expires_at >= created_at, "expires_at precedes created_at")?;
    }
    Ok(gc_record_path(store_root, "pins", pin, &pin.pin_id))
}

pub fn publish_pin_record(store_root: &Path, pin: &PinRecord) -> Result<PathBuf, GcError> {
    let path = validate_pin_record(store_root, pin)?;
    let _coord = StoreLock::sweep(store_root, LOCK_DEADLINE).map_err(GcError::Io)?;
    write_gc_json(&path, pin)?;
    Ok(path)
}

/// Remove a previously published pin. This operation is idempotent.
pub fn remove_pin_record(
    store_root: &Path,
    producer_id: &str,
    project_id: &str,
    pin_id: &str,
) -> Result<(), GcError> {
    require_gc_producer(producer_id)?;
    require_schema_field(is_valid_hash(project_id), "project_id")?;
    require_schema_field(is_valid_pin_id(pin_id), "pin_id")?;
    let path = gc_join(
        store_root,
        &["pins", producer_id, project_id, &format!("{pin_id}.json")],
    );
    let _coord = StoreLock::sweep(store_root, LOCK_DEADLINE).map_err(GcError::Io)?;
    remove_gc_record(&path)
}

pub fn publish_lease_record(store_root: &Path, lease: &LeaseRecord) -> Result<PathBuf, GcError> {
    let path = validate_lease_record(store_root, lease)?;
    let coord = StoreLock::sweep(store_root, LOCK_DEADLINE).map_err(GcError::Io)?;
    publish_lease_record_locked(store_root, lease, &path, &coord)
}

fn publish_lease_record_locked(
    store_root: &Path,
    lease: &LeaseRecord,
    path: &Path,
    guard: &StoreLock,
) -> Result<PathBuf, GcError> {
    if guard.mode() != LockMode::Exclusive || !guard.is_for_store_root(store_root) {
        return Err(GcError::Policy(
            "lease publication requires this store's exclusive coordinator lock".into(),
        ));
    }
    validate_next_lease_epoch(path, lease.epoch)?;
    write_gc_json(path, lease)?;
    Ok(path.to_path_buf())
}
fn validate_next_lease_epoch(path: &Path, epoch: u64) -> Result<(), GcError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            Err(corrupt(path, "lease record is not a regular file".into()))
        }
        Ok(_) => {
            let existing = read_lease_record(path)?;
            if epoch <= existing.epoch {
                Err(GcError::SchemaViolation(format!(
                    "lease epoch {epoch} must be strictly greater than current {}",
                    existing.epoch
                )))
            } else {
                Ok(())
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(GcError::Io(error)),
    }
}

fn validate_lease_record(store_root: &Path, lease: &LeaseRecord) -> Result<PathBuf, GcError> {
    require_schema(
        &lease.schema_version,
        &lease.record_type,
        GC_RECORD_TYPE_LEASE,
    )?;
    require_store_contract(lease.store_contract_digest.as_deref())?;
    require_gc_producer(&lease.engine)?;
    require_schema_field(is_valid_hash(&lease.project_id), "project_id")?;
    require_schema_field(is_valid_pin_id(&lease.operation_id), "operation_id")?;
    require_schema_field(lease.epoch >= 1, "epoch")?;
    require_schema_field(lease.blob_hashes.len() <= GC_MAX_BLOB_HASHES, "blob_hashes")?;
    require_schema_field(
        lease.blob_hashes.iter().all(|hash| is_valid_hash(hash)),
        "blob_hash",
    )?;
    require_schema_field(lease.owner.pid >= 1, "owner.pid")?;
    require_schema_field(
        !lease.owner.host.is_empty()
            && lease.owner.host.len() <= GC_MAX_OWNER_HOST_BYTES
            && !lease.owner.host.bytes().any(|byte| byte.is_ascii_control()),
        "owner.host",
    )?;
    if lease.grace_seconds < GC_MIN_GRACE_SECONDS {
        return Err(GcError::SchemaViolation(format!(
            "grace_seconds < {}",
            GC_MIN_GRACE_SECONDS
        )));
    }
    let started_at = parse_rfc3339(&lease.started_at)
        .ok_or_else(|| GcError::SchemaViolation("started_at".into()))?;
    let expires_at = parse_rfc3339(&lease.expires_at)
        .ok_or_else(|| GcError::SchemaViolation("expires_at".into()))?;
    require_schema_field(expires_at >= started_at, "expires_at precedes started_at")?;
    Ok(gc_record_path(
        store_root,
        "leases",
        lease,
        &lease.operation_id,
    ))
}

/// Remove a lease after its object is committed into a newer reachability snapshot.
pub fn remove_lease_record(
    store_root: &Path,
    producer_id: &str,
    project_id: &str,
    operation_id: &str,
) -> Result<(), GcError> {
    require_gc_producer(producer_id)?;
    require_schema_field(is_valid_hash(project_id), "project_id")?;
    require_schema_field(is_valid_pin_id(operation_id), "operation_id")?;
    let path = gc_join(
        store_root,
        &[
            "leases",
            producer_id,
            project_id,
            &format!("{operation_id}.json"),
        ],
    );
    let _coord = StoreLock::sweep(store_root, LOCK_DEADLINE).map_err(GcError::Io)?;
    remove_gc_record(&path)
}

fn remove_gc_record(path: &Path) -> Result<(), GcError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

impl SharedCas {
    /// Publish bytes and their protecting lease in one exclusive coordinator
    /// transaction. The object cannot become visible to a sweep without the
    /// valid lease also becoming visible.
    #[allow(clippy::too_many_arguments)]
    pub fn put_leased(
        &self,
        bytes: &[u8],
        producer_id: &str,
        project_id: &str,
        operation_id: &str,
        epoch: u64,
        owner: LeaseOwner,
        lease_seconds: u64,
    ) -> Result<PutOutcome, GcError> {
        let now = SystemTime::now();
        let lease_duration =
            std::time::Duration::from_secs(lease_seconds.max(GC_MIN_GRACE_SECONDS));
        let expires_at = now
            .checked_add(lease_duration)
            .ok_or_else(|| GcError::Policy("lease expiry overflows system time".into()))?;
        let lease = LeaseRecord {
            schema_version: GC_SCHEMA_VERSION.to_string(),
            record_type: GC_RECORD_TYPE_LEASE.to_string(),
            engine: producer_id.to_string(),
            project_id: project_id.to_string(),
            store_contract_digest: Some(gc_contract_digest_hex()),
            operation_id: operation_id.to_string(),
            epoch,
            owner,
            started_at: format_system_time(now),
            expires_at: format_system_time(expires_at),
            grace_seconds: GC_MIN_GRACE_SECONDS,
            blob_hashes: vec![content_sha256_hex(bytes)],
        };
        let lease_path = validate_lease_record(self.store_root(), &lease)?;
        let coord = StoreLock::sweep(self.store_root(), LOCK_DEADLINE).map_err(GcError::Io)?;
        validate_next_lease_epoch(&lease_path, epoch)?;
        publish_lease_record_locked(self.store_root(), &lease, &lease_path, &coord)?;
        let outcome = self.put_in_lock(bytes, crate::CAS_MAX_OBJECT_BYTES, &coord)?;
        debug_assert_eq!(
            lease.blob_hashes.as_slice(),
            std::slice::from_ref(&outcome.hash)
        );
        Ok(outcome)
    }
}

/// Verify an immutable object and restore it from authoritative bytes.
/// Corrupt content is quarantined before replacement. This compatibility API
/// returns only whether bytes changed; use [`repair_object_receipted`] when an
/// auditable producer receipt is required.
pub fn repair_object(store_root: &Path, blob_hash: &str, bytes: &[u8]) -> Result<bool, GcError> {
    validate_repair_bytes(blob_hash, bytes)?;
    let cas = SharedCas::open(store_root.to_path_buf());
    let lock = StoreLock::sweep(store_root, LOCK_DEADLINE).map_err(GcError::Io)?;
    repair_object_with_guard(&cas, &lock, blob_hash, bytes).map(|(repaired, _)| repaired)
}

/// Repair one object and persist an immutable producer-scoped receipt.
pub fn repair_object_receipted(
    store_root: &Path,
    producer_id: &str,
    project_id: &str,
    operation_id: &str,
    blob_hash: &str,
    bytes: &[u8],
) -> Result<RepairReceipt, GcError> {
    require_gc_producer(producer_id)?;
    require_schema_field(is_valid_hash(project_id), "project_id")?;
    require_schema_field(is_valid_pin_id(operation_id), "operation_id")?;
    validate_repair_bytes(blob_hash, bytes)?;
    let receipt_path = gc_join(
        store_root,
        &[
            "repairs",
            producer_id,
            project_id,
            &format!("{operation_id}.json"),
        ],
    );
    let cas = SharedCas::open(store_root.to_path_buf());
    let lock = StoreLock::sweep(store_root, LOCK_DEADLINE).map_err(GcError::Io)?;
    if fs::symlink_metadata(&receipt_path).is_ok() {
        return Err(GcError::Policy(format!(
            "repair operation {operation_id} already has a receipt"
        )));
    }
    let (repaired, quarantined) = repair_object_with_guard(&cas, &lock, blob_hash, bytes)?;
    let receipt = RepairReceipt {
        schema_version: GC_SCHEMA_VERSION.to_string(),
        record_type: GC_RECORD_TYPE_REPAIR.to_string(),
        store_contract_digest: gc_contract_digest_hex(),
        producer_id: producer_id.to_string(),
        project_id: project_id.to_string(),
        operation_id: operation_id.to_string(),
        blob_hash: blob_hash.to_string(),
        repaired,
        quarantined,
        completed_at: format_system_time(SystemTime::now()),
    };
    write_gc_json(&receipt_path, &receipt)?;
    Ok(receipt)
}

fn validate_repair_bytes(blob_hash: &str, bytes: &[u8]) -> Result<(), GcError> {
    if !is_valid_hash(blob_hash) {
        return Err(GcError::SchemaViolation("blob_hash".into()));
    }
    if content_sha256_hex(bytes) != blob_hash {
        return Err(GcError::CorruptMetadata {
            path: PathBuf::from(blob_hash),
            reason: "repair bytes do not match blob hash".into(),
        });
    }
    Ok(())
}

fn repair_object_with_guard(
    cas: &SharedCas,
    lock: &StoreLock,
    blob_hash: &str,
    bytes: &[u8],
) -> Result<(bool, bool), GcError> {
    if lock.mode() != LockMode::Exclusive || !lock.is_for_store_root(cas.store_root()) {
        return Err(GcError::Policy(
            "repair requires this store's exclusive coordinator lock".into(),
        ));
    }
    let quarantined = match cas.get_verified(blob_hash) {
        Ok(_) => return Ok((false, false)),
        Err(CasError::NotFound) => false,
        Err(CasError::DigestMismatch { .. }) => {
            cas.quarantine_object(blob_hash, lock)?;
            true
        }
        Err(error) => return Err(error.into()),
    };
    let outcome = cas.put_in_lock(bytes, crate::CAS_MAX_OBJECT_BYTES, lock)?;
    if outcome.hash != blob_hash {
        return Err(GcError::CorruptMetadata {
            path: PathBuf::from(blob_hash),
            reason: "repair publication produced a different digest".into(),
        });
    }
    Ok((true, quarantined))
}

#[cfg(test)]
#[path = "../../../tests/rust/zero-store/unit/gc.rs"]
mod tests;
