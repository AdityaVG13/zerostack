//! L2/L3 layer validity ledger (ZS-CACHE-012/013 mirror, FSZero store side).
//!
//! The store's CAS is physical-only: blob loss historically meant logical
//! invalidity. This ledger gives the store the hub `LayerValidityLedger`
//! discipline (ZeroStack `crates/zerostack/zero-gate/src/residency.rs`):
//!
//! - A blob evicted by GC/eviction is marked **L3-cold**: the bytes are gone,
//!   but the logical validity record is retained (L2-valid, needs refetch).
//!   Eviction never destroys the record and never tombstones it.
//! - A re-put/refetch of identical bytes restores L3 with the **same
//!   identity** (the content hash); no rediscovery, no re-derivation.
//! - Records are serde-defaulted per field; an absent record file reads as
//!   an absent record, so pre-ledger stores behave exactly as before.
//!
//! Records live at `<store_root>/validity/<64-hex-hash>.json`, one small
//! JSON file per blob. Writes are atomic (hub `atomic_write_file`), so a
//! reader never observes a torn record.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Frozen schema id for the validity ledger (zerostack.validity).
pub const VALIDITY_SCHEMA_VERSION: &str = "zerostack.validity";
/// Directory under the store root holding per-blob validity records.
pub const VALIDITY_DIR: &str = "validity";

/// Typed validity-ledger failures.
#[derive(Debug)]
pub enum ValidityError {
    Io {
        hash: String,
        context: String,
        source: std::io::Error,
    },
    /// Hash is not exactly 64 lowercase hex chars.
    Malformed(String),
    /// A record violates the layer invariant (e.g. L3-cold without L2
    /// validity, which would mean eviction destroyed the logical record).
    CorruptLedger { hash: String, detail: String },
}

impl fmt::Display for ValidityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidityError::Io {
                hash,
                context,
                source,
            } => write!(f, "validity ledger io ({context}) for {hash}: {source}"),
            ValidityError::Malformed(h) => {
                write!(
                    f,
                    "validity ledger: malformed hash (need 64 lowercase hex): {h}"
                )
            }
            ValidityError::CorruptLedger { hash, detail } => {
                write!(f, "validity ledger corrupt for {hash}: {detail}")
            }
        }
    }
}

impl std::error::Error for ValidityError {}

/// One blob's per-layer validity record.
///
/// All fields are serde-defaulted so a record written by a different
/// (older/newer) schema version still deserializes; absent file == absent
/// record, which is exactly the pre-ledger store behaviour.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ValidityRecord {
    pub schema_version: String,
    /// Full 64-lowercase-hex content hash (the blob's identity).
    pub hash: String,
    /// Bytes published (0 when unknown). Used for eviction-slack mass
    /// accounting of live hashes whose bytes are no longer resident.
    pub size: u64,
    /// L2 validity: the logical record/identity is valid. Never cleared by
    /// eviction; a tombstone never deletes this record.
    pub l2_valid: bool,
    /// L3-cold: bytes are gone from the local CAS. L2 validity is preserved
    /// and a refetch of identical bytes restores L3 without rediscovery.
    pub l3_cold: bool,
    /// Unix seconds when the blob was marked L3-cold. Preserved across
    /// repeated marks so the grace window is not extended (mirrors
    /// TokenZero `cold_since_unix` discipline).
    pub cold_since_unix: Option<u64>,
}

impl ValidityRecord {
    /// A fresh publish: L2 valid, L3 valid (bytes just verified by the CAS).
    fn published(hash: &str, size: u64) -> Self {
        Self {
            schema_version: VALIDITY_SCHEMA_VERSION.to_string(),
            hash: hash.to_string(),
            size,
            l2_valid: true,
            l3_cold: false,
            cold_since_unix: None,
        }
    }

    /// A blob whose bytes were evicted. When no record existed (pre-ledger
    /// store), the record is created at eviction time: the identity (hash) is
    /// known, so L2 validity is preserved even for legacy blobs.
    fn cold(hash: &str, size: u64, now_unix: u64) -> Self {
        Self {
            schema_version: VALIDITY_SCHEMA_VERSION.to_string(),
            hash: hash.to_string(),
            size,
            l2_valid: true,
            l3_cold: true,
            cold_since_unix: Some(now_unix),
        }
    }

    /// Mirror of hub `LayerValidityLedger::validate`: an L3-cold record
    /// MUST retain L2 validity. A cold record without L2 validity means the
    /// logical record was destroyed by eviction -- corrupt, fail loud.
    pub fn validate(&self) -> Result<(), ValidityError> {
        if self.l3_cold && !self.l2_valid {
            return Err(ValidityError::CorruptLedger {
                hash: self.hash.clone(),
                detail:
                    "L3-cold record without L2 validity (eviction destroyed the logical record)"
                        .to_string(),
            });
        }
        Ok(())
    }
}

/// Ledger of per-blob validity records under `<store_root>/validity/`.
#[derive(Debug, Clone)]
pub struct ValidityLedger {
    root: PathBuf,
}

impl ValidityLedger {
    /// Open the ledger for a store root. Cheap; does no I/O until used.
    /// Pre-ledger stores simply have no records yet (absent file == absent
    /// record).
    pub fn open(store_root: &Path) -> Self {
        Self {
            root: store_root.join(VALIDITY_DIR),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn record_path(&self, hash: &str) -> PathBuf {
        self.root.join(hash)
    }

    /// Read one record; `Ok(None)` when no record exists (pre-ledger state).
    pub fn load(&self, hash: &str) -> Result<Option<ValidityRecord>, ValidityError> {
        if !super::cas::is_full_lower_hex(hash) {
            return Err(ValidityError::Malformed(hash.to_string()));
        }
        let path = self.record_path(hash);
        let Ok(bytes) = std::fs::read(&path) else {
            return Ok(None);
        };
        let record: ValidityRecord =
            serde_json::from_slice(&bytes).map_err(|e| ValidityError::CorruptLedger {
                hash: hash.to_string(),
                detail: format!("deserialize {}: {e}", path.display()),
            })?;
        record.validate()?;
        Ok(Some(record))
    }

    fn write(&self, record: &ValidityRecord) -> Result<(), ValidityError> {
        record.validate()?;
        let bytes = serde_json::to_vec(record).map_err(|e| ValidityError::CorruptLedger {
            hash: record.hash.clone(),
            detail: format!("serialize: {e}"),
        })?;
        let path = self.record_path(&record.hash);
        zero_store::atomic_write_file(&path, &bytes).map_err(|e| ValidityError::Io {
            hash: record.hash.clone(),
            context: format!("atomic write {}", path.display()),
            source: e,
        })
    }

    /// Publish (or refetch-restore) a blob whose bytes the CAS just verified
    /// and stored. A put of identical bytes for a cold record is a
    /// **complete refetch**: L3 restored, same identity, never rediscovered
    /// (mirror of hub `LayerValidityLedger::complete_refetch`).
    pub fn publish(&self, hash: &str, size: u64) -> Result<(), ValidityError> {
        let mut record = match self.load(hash)? {
            Some(record) => record,
            None => ValidityRecord::published(hash, size),
        };
        record.l2_valid = true;
        record.l3_cold = false;
        record.cold_since_unix = None;
        record.size = size;
        self.write(&record)
    }

    /// Declare L3 loss (bytes evicted). L2 validity is PRESERVED and the
    /// record is marked needs-refetch. Idempotent: a repeated mark keeps the
    /// original `cold_since_unix` so grace windows are not extended. A blob
    /// with no prior record gets one at eviction time (identity is known --
    /// the content hash -- so L2 validity is retained, never destroyed).
    pub fn mark_l3_cold(&self, hash: &str, size: u64, now_unix: u64) -> Result<(), ValidityError> {
        let Some(mut record) = self.load(hash)? else {
            // Blob with no prior record (pre-ledger store): create the
            // preserving record at eviction time -- the identity (hash) is
            // known, so L2 validity is retained, never destroyed.
            return self.write(&ValidityRecord::cold(hash, size, now_unix));
        };
        record.size = size;
        if record.l3_cold {
            // Idempotent: preserve the original cold timestamp.
            return Ok(());
        }
        record.l2_valid = true;
        record.l3_cold = true;
        record.cold_since_unix = Some(now_unix);
        self.write(&record)
    }
}
