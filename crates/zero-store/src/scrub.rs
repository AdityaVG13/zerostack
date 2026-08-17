//! Periodic integrity scrub pass over CAS blobs (ZS-STORE-007).
//!
//! A scrub pass re-hashes every (idle) object and compares the digest against
//! the identity recorded in its path. Verification is on-access only
//! ([`crate::SharedCas::get_verified`]); this pass closes the gap for blobs
//! that are never read again: corruption is found before reuse.
//!
//! Fail-loud law: a corrupt object is **quarantined** (moved to
//! `<store_root>/gc/quarantine/`, never silently repaired and never deleted)
//! and reported in the receipt with its identity. The receipt is persisted
//! under `<store_root>/gc/scrubs/<producer>/<operation_id>.json` and its
//! digest binds the findings.
//!
//! Residency model (L2/L3 design hook): quarantine keeps the body recoverable,
//! so a scrub finding is an **L3 physical-residency loss with the L2 logical
//! record retained** -- the causal identity is never re-derived. The hub
//! composition layer (zsx-core) is where this receipt is wired into
//! `LayerValidityLedger` (mark L3 loss / complete refetch); engines do not
//! import each other, so zero-store itself carries no ledger dependency.
//!
//! Scheduling is the host's job (daemonless law): a session invokes
//! [`run_scrub`] with an idle-age filter for background passes.

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use zero_abi::{Sha256Digest, canonical_json, sha256};
use zero_ref::is_full_lower_hex;

use crate::cas::{CAS_MAX_OBJECT_BYTES, CasError, SharedCas};
use crate::gc::{gc_atomic_write, gc_contract_digest_hex, gc_join, is_valid_pin_id};
use crate::gc_lock::{LOCK_DEADLINE, StoreLock};

/// Schema version of a scrub receipt.
pub const SCRUB_SCHEMA_VERSION: u16 = 1;
/// Maximum objects examined in one pass, so a background pass is bounded.
pub const SCRUB_MAX_OBJECTS_PER_PASS: usize = 262_144;
/// Objects larger than this are refused by the pass (aligned with the CAS
/// size policy).
pub const SCRUB_MAX_OBJECT_BYTES: u64 = CAS_MAX_OBJECT_BYTES;

const SCRUB_RECEIPT_DOMAIN: &[u8] = b"zerostack.scrub.receipt\0";

/// Configuration of one scrub pass.
#[derive(Clone, Copy, Debug)]
pub struct ScrubConfig {
    /// Upper bound on objects examined (defaults to
    /// [`SCRUB_MAX_OBJECTS_PER_PASS`]).
    pub max_objects: Option<usize>,
    /// Per-object size ceiling (defaults to
    /// [`SCRUB_MAX_OBJECT_BYTES`]).
    pub max_object_bytes: u64,
    /// Only objects idle for at least this long are verified. `None` verifies
    /// every object. Background passes use a nonzero idle age so active
    /// writers are never raced by the pass.
    pub idle_older_than: Option<Duration>,
}
impl Default for ScrubConfig {
    fn default() -> Self {
        Self {
            max_objects: None,
            max_object_bytes: SCRUB_MAX_OBJECT_BYTES,
            idle_older_than: None,
        }
    }
}

/// Classification of one scrub finding.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrubFindingKind {
    /// Bytes did not hash to the recorded identity; the body was moved to
    /// quarantine (L3 loss, L2 record retained).
    CorruptQuarantined,
    /// The object vanished between enumeration and verification (a concurrent
    /// collector, never corruption).
    Unavailable,
    /// The object could not be verified because of a store I/O error.
    IoError,
}

/// One problem found by a scrub pass.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScrubFinding {
    pub identity: String,
    pub kind: ScrubFindingKind,
    pub detail: String,
}

/// Complete, contract-bound receipt of one scrub pass.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScrubReceipt {
    pub schema_version: u16,
    pub record_type: String,
    pub store_contract_digest: String,
    pub store_root: String,
    /// Session/ledger identity that requested the pass.
    pub producer_id: String,
    pub operation_id: String,
    pub started_at_unix_ns: u64,
    pub completed_at_unix_ns: u64,
    pub objects_scanned: u64,
    pub objects_verified: u64,
    pub objects_corrupt_quarantined: u64,
    pub objects_unavailable: u64,
    pub objects_io_error: u64,
    pub objects_skipped_idle: u64,
    pub findings: Vec<ScrubFinding>,
}
impl ScrubReceipt {
    pub fn validate(&self) -> Result<(), ScrubError> {
        if self.schema_version != SCRUB_SCHEMA_VERSION {
            return Err(ScrubError::SchemaViolation(
                "unsupported scrub receipt schema version".into(),
            ));
        }
        if self.record_type != "scrub" {
            return Err(ScrubError::SchemaViolation(
                "scrub receipt record_type is not 'scrub'".into(),
            ));
        }
        if self.store_contract_digest != gc_contract_digest_hex() {
            return Err(ScrubError::SchemaViolation(
                "scrub receipt store contract digest is not current".into(),
            ));
        }
        if !is_valid_pin_id(&self.producer_id) || !is_valid_pin_id(&self.operation_id) {
            return Err(ScrubError::SchemaViolation(
                "scrub receipt producer or operation identity is invalid".into(),
            ));
        }
        let classified = self.objects_verified
            + self.objects_corrupt_quarantined
            + self.objects_unavailable
            + self.objects_io_error;
        if classified != self.objects_scanned {
            return Err(ScrubError::SchemaViolation(
                "scrub receipt counts do not add up to objects_scanned".into(),
            ));
        }
        let expected_findings = self.objects_corrupt_quarantined
            + self.objects_unavailable
            + self.objects_io_error;
        if self.findings.len() as u64 != expected_findings {
            return Err(ScrubError::SchemaViolation(
                "scrub receipt finding count disagrees with its counters".into(),
            ));
        }
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ScrubError> {
        self.validate()?;
        let value = serde_json::to_value(self)
            .map_err(|error| ScrubError::SchemaViolation(error.to_string()))?;
        Ok(canonical_json(&value).into_bytes())
    }
    pub fn digest(&self) -> Result<Sha256Digest, ScrubError> {
        let bytes = self.canonical_bytes()?;
        let mut bound = Vec::with_capacity(SCRUB_RECEIPT_DOMAIN.len() + 8 + bytes.len());
        bound.extend_from_slice(SCRUB_RECEIPT_DOMAIN);
        bound.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
        bound.extend_from_slice(&bytes);
        Ok(Sha256Digest::from_bytes(sha256(&bound)))
    }
}

/// Engine-neutral scrub error.
#[derive(Debug)]
pub enum ScrubError {
    SchemaViolation(String),
    LockDenied(String),
    /// The pass bound refused a store with more objects than `max`; raise the
    /// bound or page the pass. Loud by design: a bounded pass never silently
    /// scans a subset.
    EnumerationExceedsBound { max: usize },
    Io(String),
}

impl std::fmt::Display for ScrubError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SchemaViolation(message) => write!(f, "schema_violation: {message}"),
            Self::LockDenied(message) => write!(f, "lock_denied: {message}"),
            Self::EnumerationExceedsBound { max } => {
                write!(f, "enumeration_exceeds_bound: store has more than {max} objects")
            }
            Self::Io(message) => write!(f, "io: {message}"),
        }
    }
}
impl std::error::Error for ScrubError {}

/// Run one bounded scrub pass over the CAS under `store_root` and persist the
/// receipt. Runs under the exclusive sweep lock because quarantine requires
/// it; a concurrent collector cannot race the pass.
pub fn run_scrub(
    store_root: &Path,
    config: &ScrubConfig,
    producer_id: &str,
    operation_id: &str,
) -> Result<ScrubReceipt, ScrubError> {
    if !is_valid_pin_id(producer_id) {
        return Err(ScrubError::SchemaViolation(
            "producer_id is not a valid identity".into(),
        ));
    }
    if !is_valid_pin_id(operation_id) {
        return Err(ScrubError::SchemaViolation(
            "operation_id is not a valid identity".into(),
        ));
    }
    let cas = SharedCas::open(store_root.to_path_buf());
    let lock = StoreLock::sweep(store_root, LOCK_DEADLINE)
        .map_err(|error| ScrubError::LockDenied(error.to_string()))?;
    let max_objects = config.max_objects.unwrap_or(SCRUB_MAX_OBJECTS_PER_PASS);
    let started_at_unix_ns = now_unix_ns();

    let mut scanned: u64 = 0;
    let mut verified: u64 = 0;
    let mut corrupt_quarantined: u64 = 0;
    let mut unavailable: u64 = 0;
    let mut io_error: u64 = 0;
    let mut skipped_idle: u64 = 0;
    let mut findings: Vec<ScrubFinding> = Vec::new();

    let identities = cas.list_objects_bounded(max_objects).map_err(|error| match &error {
        CasError::Malformed(message) if message.starts_with("CAS object enumeration exceeds") => {
            ScrubError::EnumerationExceedsBound { max: max_objects }
        }
        _ => ScrubError::Io(error.to_string()),
    })?;
    for identity in identities {
        if !is_full_lower_hex(&identity) {
            continue;
        }
        if let Some(idle_older_than) = config.idle_older_than {
            let path = cas.object_path(&identity);
            let idle = fs::symlink_metadata(&path)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|mtime| SystemTime::now().duration_since(mtime).ok())
                .is_some_and(|age| age >= idle_older_than);
            if !idle {
                skipped_idle += 1;
                continue;
            }
        }
        scanned += 1;
        match cas.get_verified_limited(&identity, config.max_object_bytes) {
            Ok(_) => verified += 1,
            Err(CasError::DigestMismatch { .. }) => {
                cas.quarantine_object(&identity, &lock)
                    .map_err(|error| ScrubError::Io(error.to_string()))?;
                corrupt_quarantined += 1;
                findings.push(ScrubFinding {
                    identity: identity.clone(),
                    kind: ScrubFindingKind::CorruptQuarantined,
                    detail: "content digest did not match the recorded identity; body moved to quarantine"
                        .into(),
                });
            }
            Err(CasError::NotFound) => {
                unavailable += 1;
                findings.push(ScrubFinding {
                    identity,
                    kind: ScrubFindingKind::Unavailable,
                    detail: "object vanished between enumeration and verification".into(),
                });
            }
            Err(error) => {
                io_error += 1;
                findings.push(ScrubFinding {
                    identity,
                    kind: ScrubFindingKind::IoError,
                    detail: error.to_string(),
                });
            }
        }
    }

    let receipt = ScrubReceipt {
        schema_version: SCRUB_SCHEMA_VERSION,
        record_type: "scrub".into(),
        store_contract_digest: gc_contract_digest_hex(),
        store_root: store_root.to_string_lossy().into_owned(),
        producer_id: producer_id.to_string(),
        operation_id: operation_id.to_string(),
        started_at_unix_ns,
        completed_at_unix_ns: now_unix_ns(),
        objects_scanned: scanned,
        objects_verified: verified,
        objects_corrupt_quarantined: corrupt_quarantined,
        objects_unavailable: unavailable,
        objects_io_error: io_error,
        objects_skipped_idle: skipped_idle,
        findings,
    };
    receipt.validate()?;
    let bytes = receipt.canonical_bytes()?;
    let path = gc_join(
        store_root,
        &["scrubs", producer_id, &format!("{operation_id}.json")],
    );
    gc_atomic_write(&path, &bytes)
        .map_err(|error| ScrubError::Io(error.to_string()))?;
    Ok(receipt)
}

/// Read a persisted scrub receipt back with canonical and structural
/// validation, so a torn, non-canonical, or inconsistent receipt fails loudly
/// instead of being trusted. (Authenticity anchoring beyond structure -- for
/// example a registry of receipt digests -- is the caller's ledger, as with
/// every other receipt in this crate.)
pub fn read_scrub_receipt(
    store_root: &Path,
    producer_id: &str,
    operation_id: &str,
) -> Result<ScrubReceipt, ScrubError> {
    let path = gc_join(
        store_root,
        &["scrubs", producer_id, &format!("{operation_id}.json")],
    );
    let bytes = fs::read(&path).map_err(|error| ScrubError::Io(error.to_string()))?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| {
            ScrubError::SchemaViolation(format!("receipt decode failed: {error}"))
        })?;
    let canonical = canonical_json(&value);
    if canonical.as_bytes() != bytes {
        return Err(ScrubError::SchemaViolation(
            "receipt bytes are not canonical JSON".into(),
        ));
    }
    let receipt: ScrubReceipt = serde_json::from_value(value).map_err(|error| {
        ScrubError::SchemaViolation(format!("receipt structure failed: {error}"))
    })?;
    receipt.validate()?;
    Ok(receipt)
}

fn now_unix_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0)
}

