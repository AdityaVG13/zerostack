//! Versioned store-format migrations (ZS-OPS-004).
//!
//! The on-disk store format carries an explicit version record at
//! `<store_root>/format_version`. Reads detect that version; a version beyond
//! the known set is refused loudly (fail closed, never guessed). A migration
//! run applies an ordered, deterministic chain of steps
//! (`v(n) -> v(n+1)`, transform function pointer -- no environment, no
//! clocks), each persisted as an immutable marker before the format version
//! advances, and emits a [`MigrationReceiptV1`] binding old/new format-state
//! roots, the transform digest, and the validation digest of the final state.
//!
//! Idempotency: a step whose marker already exists is not re-applied (crash
//! between transform and version advance is recovered, not repeated), and a
//! store already at the target version is a no-op. Today the production
//! registry is empty (format v1 is current), so the only production outcome
//! is [`MigrationErrorV1::SchemaVersionMismatch`] for future versions; the
//! runner itself is exercised by fixture steps in the unit tests.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zero_abi::{DigestV1, canonical_json, sha256};

use crate::fs_replace::atomic_write_file;
use crate::gc::{gc_contract_digest_hex, gc_join};

/// Schema version of the format-version record.
pub const STORE_FORMAT_SCHEMA_VERSION_V1: u16 = 1;
/// Current on-disk store format version.
pub const STORE_FORMAT_VERSION_CURRENT_V1: u32 = 1;
/// Highest store format version the production registry knows. Any on-disk
/// version above this is refused loudly.
pub const STORE_FORMAT_MAX_KNOWN_VERSION_V1: u32 = 1;
/// Schema version of a migration receipt.
pub const MIGRATION_RECEIPT_SCHEMA_VERSION_V1: u16 = 1;
/// File name of the format-version record, relative to the store root.
pub const STORE_FORMAT_VERSION_FILENAME: &str = "format_version";

pub const FORMAT_VERSION_DOMAIN_V1: &[u8] = b"zerostack.store_format.version.v1\0";
pub const MIGRATION_RECEIPT_DOMAIN_V1: &[u8] = b"zerostack.store_format.migration_receipt.v1\0";
pub const MIGRATION_STEP_DOMAIN_V1: &[u8] = b"zerostack.store_format.migration_step.v1\0";
pub const MIGRATION_MARKER_DOMAIN_V1: &[u8] = b"zerostack.store_format.migration_marker.v1\0";

/// Explicit on-disk store-format version record.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StoreFormatVersionV1 {
    pub schema_version: u16,
    pub format_version: u32,
}
impl StoreFormatVersionV1 {
    pub fn new(format_version: u32) -> Self {
        Self {
            schema_version: STORE_FORMAT_SCHEMA_VERSION_V1,
            format_version,
        }
    }
    pub fn validate(&self) -> Result<(), MigrationErrorV1> {
        if self.schema_version != STORE_FORMAT_SCHEMA_VERSION_V1 {
            return Err(MigrationErrorV1::UnsupportedRecordSchema(self.schema_version));
        }
        if self.format_version == 0 {
            return Err(MigrationErrorV1::TornOrNoncanonicalRecord(
                "format version zero is reserved and never valid".into(),
            ));
        }
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, MigrationErrorV1> {
        self.validate()?;
        canonical_record_bytes(self)
    }
    /// Deterministic format-state root digest: the domain digest of the
    /// canonical version record, used as the migration receipt's old/new root.
    pub fn state_digest(&self) -> Result<DigestV1, MigrationErrorV1> {
        Ok(domain_digest(FORMAT_VERSION_DOMAIN_V1, &self.canonical_bytes()?))
    }
}

/// Migration failure classes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MigrationErrorV1 {
    Io(String),
    TornOrNoncanonicalRecord(String),
    /// The format-version record itself carries an unknown schema.
    UnsupportedRecordSchema(u16),
    /// The on-disk store format is newer than anything the runner knows.
    SchemaVersionMismatch {
        detected: u32,
        max_supported: u32,
    },
    InvalidStepChain(String),
    ImmutableMarkerConflict(String),
    TransformFailed(String),
    ReceiptConflict(String),
}
impl std::fmt::Display for MigrationErrorV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(message) => write!(f, "io: {message}"),
            Self::TornOrNoncanonicalRecord(message) => {
                write!(f, "torn_or_noncanonical_record: {message}")
            }
            Self::UnsupportedRecordSchema(version) => {
                write!(f, "unsupported_record_schema: {version}")
            }
            Self::SchemaVersionMismatch {
                detected,
                max_supported,
            } => write!(
                f,
                "schema_version_mismatch: store format {detected} exceeds the supported maximum {max_supported}"
            ),
            Self::InvalidStepChain(message) => write!(f, "invalid_step_chain: {message}"),
            Self::ImmutableMarkerConflict(message) => {
                write!(f, "immutable_marker_conflict: {message}")
            }
            Self::TransformFailed(message) => write!(f, "transform_failed: {message}"),
            Self::ReceiptConflict(message) => write!(f, "receipt_conflict: {message}"),
        }
    }
}
impl std::error::Error for MigrationErrorV1 {}

/// Result of one applied migration step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MigrationStepOutcomeV1 {
    /// Digest of the store state the transform produced, used as the
    /// validation digest of the step.
    pub validation_digest: DigestV1,
}

/// A deterministic `v(n) -> v(n+1)` store-format transform. The transform is
/// a plain function pointer so a step sequence is reproducible by
/// construction (no captured environment, no wall clock).
pub type MigrationTransformV1 = fn(&Path) -> Result<MigrationStepOutcomeV1, MigrationErrorV1>;

/// One ordered migration step. `to_version` must be exactly `from_version + 1`.
#[derive(Clone, Copy)]
pub struct MigrationStepV1 {
    pub from_version: u32,
    pub to_version: u32,
    pub transform_name: &'static str,
    transform: MigrationTransformV1,
}
impl MigrationStepV1 {
    pub fn new(
        from_version: u32,
        to_version: u32,
        transform_name: &'static str,
        transform: MigrationTransformV1,
    ) -> Self {
        Self {
            from_version,
            to_version,
            transform_name,
            transform,
        }
    }
    pub fn validate(&self) -> Result<(), MigrationErrorV1> {
        if self.from_version == 0 || self.to_version != self.from_version + 1 {
            return Err(MigrationErrorV1::InvalidStepChain(
                "steps must advance exactly one version (v(n) -> v(n+1))".into(),
            ));
        }
        if self.transform_name.is_empty() {
            return Err(MigrationErrorV1::InvalidStepChain(
                "transform name must not be empty".into(),
            ));
        }
        Ok(())
    }
    /// Digest of this step's descriptor, aggregated into the receipt's
    /// transform digest.
    pub fn descriptor_digest(&self) -> Result<DigestV1, MigrationErrorV1> {
        self.validate()?;
        let descriptor = format!("{}|{}|{}", self.from_version, self.to_version, self.transform_name);
        Ok(domain_digest(MIGRATION_STEP_DOMAIN_V1, descriptor.as_bytes()))
    }
    fn marker_path(&self, store_root: &Path) -> PathBuf {
        gc_join(
            store_root,
            &[
                "migrations",
                &format!("{}-{}-{}.json", self.transform_name, self.from_version, self.to_version),
            ],
        )
    }
}

/// Immutable per-step marker record, persisted before the format version
/// advances so a crash between transform and version write is recovered
/// without re-applying the transform.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationMarkerV1 {
    pub schema_version: u16,
    pub from_version: u32,
    pub to_version: u32,
    pub transform_name: String,
    pub validation_digest: DigestV1,
}
impl MigrationMarkerV1 {
    pub fn validate(&self) -> Result<(), MigrationErrorV1> {
        if self.schema_version != STORE_FORMAT_SCHEMA_VERSION_V1 {
            return Err(MigrationErrorV1::UnsupportedRecordSchema(self.schema_version));
        }
        if self.transform_name.is_empty()
            || self.from_version == 0
            || self.to_version != self.from_version + 1
            || self.validation_digest == DigestV1::ZERO
        {
            return Err(MigrationErrorV1::TornOrNoncanonicalRecord(
                "migration marker is incomplete".into(),
            ));
        }
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, MigrationErrorV1> {
        self.validate()?;
        canonical_record_bytes(self)
    }
    pub fn digest(&self) -> Result<DigestV1, MigrationErrorV1> {
        Ok(domain_digest(MIGRATION_MARKER_DOMAIN_V1, &self.canonical_bytes()?))
    }
}

/// Contract-bound receipt of one migration run: old/new format-state roots,
/// the digest of the ordered transforms applied, and the validation digest of
/// the final state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationReceiptV1 {
    pub schema_version: u16,
    pub record_type: String,
    pub store_contract_digest: String,
    pub old_version: u32,
    pub new_version: u32,
    /// Format-state root digest before the run (domain digest of the
    /// canonical version record).
    pub old_root: DigestV1,
    /// Format-state root digest after the run.
    pub new_root: DigestV1,
    /// Digest of the ordered step descriptors applied by this run (domain
    /// digest of the empty byte string when nothing was applied).
    pub transform_digest: DigestV1,
    /// Digest of the final validated store state (last marker's validation
    /// digest, or the new format-state root digest when no step applied).
    pub validation_digest: DigestV1,
    pub steps_applied: u32,
    pub applied_step_names: Vec<String>,
}
impl MigrationReceiptV1 {
    pub fn validate(&self) -> Result<(), MigrationErrorV1> {
        if self.schema_version != MIGRATION_RECEIPT_SCHEMA_VERSION_V1 {
            return Err(MigrationErrorV1::UnsupportedRecordSchema(self.schema_version));
        }
        if self.record_type != "migration" {
            return Err(MigrationErrorV1::TornOrNoncanonicalRecord(
                "migration receipt record_type is not 'migration'".into(),
            ));
        }
        if self.store_contract_digest != gc_contract_digest_hex() {
            return Err(MigrationErrorV1::TornOrNoncanonicalRecord(
                "migration receipt store contract digest is not current".into(),
            ));
        }
        if self.old_version == 0
            || self.new_version == 0
            || self.new_version < self.old_version
            || self.old_root == DigestV1::ZERO
            || self.new_root == DigestV1::ZERO
            || self.validation_digest == DigestV1::ZERO
            || self.applied_step_names.len() as u32 != self.steps_applied
        {
            return Err(MigrationErrorV1::TornOrNoncanonicalRecord(
                "migration receipt commitments disagree".into(),
            ));
        }
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, MigrationErrorV1> {
        self.validate()?;
        canonical_record_bytes(self)
    }
    pub fn digest(&self) -> Result<DigestV1, MigrationErrorV1> {
        Ok(domain_digest(MIGRATION_RECEIPT_DOMAIN_V1, &self.canonical_bytes()?))
    }
}

/// The production migration registry. Format v1 is current, so today it is
/// empty and the only production outcomes are no-ops and loud
/// [`MigrationErrorV1::SchemaVersionMismatch`] refusals. Future format
/// changes add their ordered steps here.
pub fn production_migration_steps_v1() -> Vec<MigrationStepV1> {
    Vec::new()
}

/// Detect the on-disk store format version. Absence of the version record
/// means the implicit v1 default (nothing is written by detection). A present
/// record is validated canonically; a torn record or an unknown record schema
/// fails loudly.
pub fn detect_store_format_version_v1(
    store_root: &Path,
) -> Result<Option<StoreFormatVersionV1>, MigrationErrorV1> {
    let path = store_root.join(STORE_FORMAT_VERSION_FILENAME);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(MigrationErrorV1::Io(error.to_string())),
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
        MigrationErrorV1::TornOrNoncanonicalRecord(format!("version record decode failed: {error}"))
    })?;
    if canonical_json(&value).as_bytes() != bytes {
        return Err(MigrationErrorV1::TornOrNoncanonicalRecord(
            "version record bytes are not canonical JSON".into(),
        ));
    }
    let version: StoreFormatVersionV1 = serde_json::from_value(value).map_err(|error| {
        MigrationErrorV1::TornOrNoncanonicalRecord(format!(
            "version record structure failed: {error}"
        ))
    })?;
    version.validate()?;
    Ok(Some(version))
}

/// Fail-closed compatibility gate: refuse (loudly) any on-disk store format
/// version above the production maximum. Callers that intend to migrate use
/// [`run_store_migrations_v1`] instead.
pub fn ensure_format_supported_v1(store_root: &Path) -> Result<StoreFormatVersionV1, MigrationErrorV1> {
    let detected = detect_store_format_version_v1(store_root)?;
    let version = detected.unwrap_or_else(|| StoreFormatVersionV1::new(STORE_FORMAT_VERSION_CURRENT_V1));
    if version.format_version > STORE_FORMAT_MAX_KNOWN_VERSION_V1 {
        return Err(MigrationErrorV1::SchemaVersionMismatch {
            detected: version.format_version,
            max_supported: STORE_FORMAT_MAX_KNOWN_VERSION_V1,
        });
    }
    Ok(version)
}

/// Run the ordered, idempotent migration chain `steps` on `store_root`.
///
/// - The on-disk version is detected first; a version above the union of the
///   production maximum and every step's target is refused loudly.
/// - Steps must form a contiguous ascending chain; each step applies at most
///   once (a persisted marker of the same step is honored as already applied).
/// - The receipt is persisted under `<store_root>/gc/migrations/receipts/`.
pub fn run_store_migrations_v1(
    store_root: &Path,
    steps: &[MigrationStepV1],
) -> Result<MigrationReceiptV1, MigrationErrorV1> {
    let detected = detect_store_format_version_v1(store_root)?;
    let current = detected
        .map(|version| version.format_version)
        .unwrap_or(STORE_FORMAT_VERSION_CURRENT_V1);
    let max_known = steps
        .iter()
        .map(|step| step.to_version)
        .max()
        .unwrap_or(STORE_FORMAT_MAX_KNOWN_VERSION_V1)
        .max(STORE_FORMAT_MAX_KNOWN_VERSION_V1);
    if current > max_known {
        return Err(MigrationErrorV1::SchemaVersionMismatch {
            detected: current,
            max_supported: max_known,
        });
    }
    for step in steps {
        step.validate()?;
    }
    for pair in steps.windows(2) {
        if pair[0].to_version != pair[1].from_version {
            return Err(MigrationErrorV1::InvalidStepChain(
                "steps must be ordered and contiguous (v(n) -> v(n+1))".into(),
            ));
        }
    }
    if let Some(step) = steps.first()
        && step.from_version < current
        && current < step.to_version
    {
        return Err(MigrationErrorV1::InvalidStepChain(
            "store version lies inside a step boundary (torn marker state)".into(),
        ));
    }

    let target = steps
        .last()
        .map(|step| step.to_version)
        .unwrap_or(current);
    let old_root = StoreFormatVersionV1::new(current).state_digest()?;

    let mut steps_applied: u32 = 0;
    let mut applied_step_names: Vec<String> = Vec::new();
    let mut applied_descriptor_digests: Vec<DigestV1> = Vec::new();
    let mut validation_digest = StoreFormatVersionV1::new(current).state_digest()?;

    for step in steps {
        if step.to_version <= current {
            // Already at or past this step's target: nothing to apply.
            continue;
        }
        let marker_path = step.marker_path(store_root);
        match read_marker(&marker_path)? {
            Some(marker) => {
                if marker.from_version != step.from_version
                    || marker.to_version != step.to_version
                    || marker.transform_name != step.transform_name
                {
                    return Err(MigrationErrorV1::ImmutableMarkerConflict(format!(
                        "marker at '{}' does not describe step {} -> {} ({})",
                        marker_path.display(),
                        step.from_version,
                        step.to_version,
                        step.transform_name
                    )));
                }
                validation_digest = marker.validation_digest;
            }
            None => {
                let outcome = (step.transform)(store_root).map_err(|error| {
                    MigrationErrorV1::TransformFailed(format!(
                        "step {} -> {} ({}) failed: {error}",
                        step.from_version, step.to_version, step.transform_name
                    ))
                })?;
                if outcome.validation_digest == DigestV1::ZERO {
                    return Err(MigrationErrorV1::TransformFailed(
                        "transform returned a zero validation digest".into(),
                    ));
                }
                let marker = MigrationMarkerV1 {
                    schema_version: STORE_FORMAT_SCHEMA_VERSION_V1,
                    from_version: step.from_version,
                    to_version: step.to_version,
                    transform_name: step.transform_name.to_string(),
                    validation_digest: outcome.validation_digest,
                };
                atomic_write_file(&marker_path, &marker.canonical_bytes()?)
                    .map_err(|error| MigrationErrorV1::Io(error.to_string()))?;
                validation_digest = marker.validation_digest;
            }
        }
        applied_step_names.push(step.transform_name.to_string());
        applied_descriptor_digests.push(step.descriptor_digest()?);
        steps_applied += 1;
    }

    let new_version = target;
    let new_root = StoreFormatVersionV1::new(new_version).state_digest()?;
    let transform_digest = if applied_descriptor_digests.is_empty() {
        domain_digest(MIGRATION_STEP_DOMAIN_V1, b"")
    } else {
        let mut bound = Vec::new();
        for digest in &applied_descriptor_digests {
            bound.extend_from_slice(digest.as_bytes());
        }
        domain_digest(MIGRATION_STEP_DOMAIN_V1, &bound)
    };

    if new_version != current {
        let version = StoreFormatVersionV1::new(new_version);
        let version_bytes = version.canonical_bytes()?;
        atomic_write_file(&store_root.join(STORE_FORMAT_VERSION_FILENAME), &version_bytes)
            .map_err(|error| MigrationErrorV1::Io(error.to_string()))?;
    }

    let receipt = MigrationReceiptV1 {
        schema_version: MIGRATION_RECEIPT_SCHEMA_VERSION_V1,
        record_type: "migration".into(),
        store_contract_digest: gc_contract_digest_hex(),
        old_version: current,
        new_version,
        old_root,
        new_root,
        transform_digest,
        validation_digest,
        steps_applied,
        applied_step_names,
    };
    receipt.validate()?;
    let receipt_bytes = receipt.canonical_bytes()?;
    let receipt_digest = receipt.digest()?;
    let receipt_path = gc_join(
        store_root,
        &[
            "migrations",
            "receipts",
            &format!(
                "{current}-{new_version}-{}.json",
                &receipt_digest.to_hex()[..8]
            ),
        ],
    );
    persist_immutable(&receipt_path, &receipt_bytes)?;
    Ok(receipt)
}

fn read_marker(path: &Path) -> Result<Option<MigrationMarkerV1>, MigrationErrorV1> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(MigrationErrorV1::Io(error.to_string())),
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
        MigrationErrorV1::TornOrNoncanonicalRecord(format!("marker decode failed: {error}"))
    })?;
    if canonical_json(&value).as_bytes() != bytes {
        return Err(MigrationErrorV1::TornOrNoncanonicalRecord(
            "marker bytes are not canonical JSON".into(),
        ));
    }
    let marker: MigrationMarkerV1 = serde_json::from_value(value).map_err(|error| {
        MigrationErrorV1::TornOrNoncanonicalRecord(format!("marker structure failed: {error}"))
    })?;
    marker.validate()?;
    Ok(Some(marker))
}

fn persist_immutable(path: &Path, bytes: &[u8]) -> Result<(), MigrationErrorV1> {
    match fs::read(path) {
        Ok(existing) if existing == bytes => Ok(()),
        Ok(_) => Err(MigrationErrorV1::ReceiptConflict(
            "an immutable record already exists at this path with different bytes".into(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            atomic_write_file(path, bytes).map_err(|error| MigrationErrorV1::Io(error.to_string()))
        }
        Err(error) => Err(MigrationErrorV1::Io(error.to_string())),
    }
}

fn canonical_record_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, MigrationErrorV1> {
    let value = serde_json::to_value(value)
        .map_err(|error| MigrationErrorV1::TornOrNoncanonicalRecord(error.to_string()))?;
    Ok(canonical_json(&value).into_bytes())
}

fn domain_digest(domain: &[u8], bytes: &[u8]) -> DigestV1 {
    let mut bound = Vec::with_capacity(domain.len() + 8 + bytes.len());
    bound.extend_from_slice(domain);
    bound.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    bound.extend_from_slice(bytes);
    DigestV1::from_bytes(sha256(&bound))
}

