//! Truthful aggregate Program receipts across FSZero, GraphZero, and TokenZero.
//!
//! A Program-level aggregate may report success only when every required engine
//! (FSZero, GraphZero, TokenZero) carries distinct, digest-bound evidence for
//! every required evidence class (planner, worker, MCP, lifecycle, GC). This
//! module is pure, sync, and no-I/O; `verify` is a cold receipt-validation
//! path. It never certifies semantic sufficiency (T6) and never widens the
//! engine or class vocabulary silently — a new member requires a schema
//! successor.
//!
//! `verify` fails closed: any missing engine, missing surface, duplicate slot,
//! unknown engine, zero digest, noncanonical ordering, or forged `receipt_head`
//! returns a documented typed failure that can never count as Program success.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fmt;
use zero_abi::{ArtifactOwnerV1, DigestV1, canonical_json, sha256};

pub const AGGREGATE_PROGRAM_SCHEMA_VERSION: u16 = 1;
pub const AGGREGATE_PROGRAM_MAX_CANONICAL_BYTES: usize = 64 * 1024;
pub const AGGREGATE_PROGRAM_MAX_SOURCE_HEADS: usize = 64;
pub const AGGREGATE_PROGRAM_MAX_REPOSITORY_BYTES: usize = 64;
pub const AGGREGATE_PROGRAM_MIN_HEAD_BYTES: usize = 40;
pub const AGGREGATE_PROGRAM_MAX_HEAD_BYTES: usize = 64;
pub const AGGREGATE_PROGRAM_REQUIRED_ENGINES: [ArtifactOwnerV1; 3] = [
    ArtifactOwnerV1::FsZero,
    ArtifactOwnerV1::GraphZero,
    ArtifactOwnerV1::TokenZero,
];
pub const AGGREGATE_PROGRAM_EVIDENCE_CLASSES: [AggregateEvidenceClassV1; 5] = [
    AggregateEvidenceClassV1::Planner,
    AggregateEvidenceClassV1::Worker,
    AggregateEvidenceClassV1::Mcp,
    AggregateEvidenceClassV1::Lifecycle,
    AggregateEvidenceClassV1::Gc,
];

const AGGREGATE_PROGRAM_CONTRACT_DOMAIN_V1: &[u8] = b"zerostack.aggregate_program_receipt.v1\0";
const AGGREGATE_PROGRAM_RECEIPT_DOMAIN_V1: &[u8] = b"zerostack.aggregate_program_receipt.head.v1\0";

/// Distinct evidence classes an aggregate Program must carry per engine.
///
/// Each class stays a separate digest-bound slot; merging classes would hide
/// which surface contributed (or failed to contribute) evidence.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateEvidenceClassV1 {
    Planner,
    Worker,
    Mcp,
    Lifecycle,
    Gc,
}

impl AggregateEvidenceClassV1 {
    pub const ALL: [Self; 5] = AGGREGATE_PROGRAM_EVIDENCE_CLASSES;

    /// Stable canonical order index (also the required slot sort order).
    pub const fn index(self) -> usize {
        match self {
            Self::Planner => 0,
            Self::Worker => 1,
            Self::Mcp => 2,
            Self::Lifecycle => 3,
            Self::Gc => 4,
        }
    }
}

/// Exact source repository head bound into the aggregate receipt. The shape
/// mirrors the two-phase `SourceHead` contract (repository 1..=64
/// `[A-Za-z0-9._-]` bytes; head 40..=64 lowercase hex) so aggregates and
/// Program receipts can never disagree on what a source head is.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AggregateSourceHeadV1 {
    pub repository: String,
    pub head: String,
}

/// One distinct evidence slot: a class plus the digest of that class's evidence
/// record (planner plan, worker receipt, MCP trace, lifecycle record, GC
/// record). The digest is bound into the receipt head, so evidence cannot be
/// substituted after sealing.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceSlotV1 {
    pub class: AggregateEvidenceClassV1,
    pub evidence_digest: DigestV1,
}

/// Per-engine evidence: exactly one slot per required class, in canonical order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EngineEvidenceV1 {
    pub engine: ArtifactOwnerV1,
    pub slots: Vec<EvidenceSlotV1>,
}

/// The aggregate Program receipt. Fields are public so fixtures can be decoded;
/// `verify` is the sole authority over truthfulness.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AggregateProgramReceiptV1 {
    pub schema_version: u16,
    pub program_digest: DigestV1,
    pub assembly_manifest_digest: DigestV1,
    pub source_repository_heads: Vec<AggregateSourceHeadV1>,
    pub engines: Vec<EngineEvidenceV1>,
    pub receipt_head: DigestV1,
}

impl AggregateProgramReceiptV1 {
    /// Builds a sealed receipt from validated engine evidence. `verify` runs
    /// first; `receipt_head` is computed canonically over every other field.
    pub fn new(
        program_digest: DigestV1,
        assembly_manifest_digest: DigestV1,
        source_repository_heads: Vec<AggregateSourceHeadV1>,
        engines: Vec<EngineEvidenceV1>,
    ) -> Result<Self, AggregateProgramErrorV1> {
        let mut receipt = Self {
            schema_version: AGGREGATE_PROGRAM_SCHEMA_VERSION,
            program_digest,
            assembly_manifest_digest,
            source_repository_heads,
            engines,
            receipt_head: DigestV1::ZERO,
        };
        receipt.receipt_head = receipt.compute_receipt_head()?;
        receipt.verify()?;
        Ok(receipt)
    }

    /// Fail-closed verification. Returns `Ok(())` only when every required
    /// engine and every required evidence class is present, distinct,
    /// canonical, nonzero, and the receipt head matches the canonical body.
    pub fn verify(&self) -> Result<(), AggregateProgramErrorV1> {
        if self.schema_version != AGGREGATE_PROGRAM_SCHEMA_VERSION {
            return Err(aggregate_error(
                AggregateProgramFailureCodeV1::SchemaVersionMismatch,
                format!(
                    "aggregate receipt schema version {} is not current ({})",
                    self.schema_version, AGGREGATE_PROGRAM_SCHEMA_VERSION
                ),
            ));
        }
        if is_zero(&self.program_digest) {
            return Err(aggregate_error(
                AggregateProgramFailureCodeV1::ZeroDigest,
                "aggregate receipt program_digest is zero",
            ));
        }
        if is_zero(&self.assembly_manifest_digest) {
            return Err(aggregate_error(
                AggregateProgramFailureCodeV1::ZeroDigest,
                "aggregate receipt assembly_manifest_digest is zero",
            ));
        }
        validate_source_heads(&self.source_repository_heads)?;

        // Engine coverage: exactly {FsZero, GraphZero, TokenZero}, no more.
        let mut seen = BTreeSet::new();
        for engine in &self.engines {
            if !seen.insert(engine.engine) {
                return Err(aggregate_error(
                    AggregateProgramFailureCodeV1::DuplicateEngine,
                    format!("engine {:?} appears more than once", engine.engine),
                ));
            }
        }
        for engine in &seen {
            if !AGGREGATE_PROGRAM_REQUIRED_ENGINES.contains(engine) {
                return Err(aggregate_error(
                    AggregateProgramFailureCodeV1::UnknownEngine,
                    format!("engine set contains non-aggregate engine {:?}", engine),
                ));
            }
        }
        for engine in AGGREGATE_PROGRAM_REQUIRED_ENGINES {
            if !seen.contains(&engine) {
                return Err(aggregate_error(
                    AggregateProgramFailureCodeV1::MissingEngine,
                    format!("required engine {:?} has no evidence", engine),
                ));
            }
        }
        if !self
            .engines
            .windows(2)
            .all(|pair| pair[0].engine < pair[1].engine)
        {
            return Err(aggregate_error(
                AggregateProgramFailureCodeV1::NonCanonicalEncoding,
                "engine evidence is not in canonical engine order",
            ));
        }

        // Surface coverage: exactly the five classes per engine, distinct and
        // canonically ordered.
        for engine in &self.engines {
            let mut seen_classes = BTreeSet::new();
            for slot in &engine.slots {
                if !seen_classes.insert(slot.class) {
                    return Err(aggregate_error(
                        AggregateProgramFailureCodeV1::DuplicateEvidenceSlot,
                        format!(
                            "engine {:?} repeats evidence class {:?}",
                            engine.engine, slot.class
                        ),
                    ));
                }
                if is_zero(&slot.evidence_digest) {
                    return Err(aggregate_error(
                        AggregateProgramFailureCodeV1::ZeroDigest,
                        format!(
                            "engine {:?} evidence class {:?} has a zero digest",
                            engine.engine, slot.class
                        ),
                    ));
                }
            }
            if seen_classes.len() != AGGREGATE_PROGRAM_EVIDENCE_CLASSES.len() {
                return Err(aggregate_error(
                    AggregateProgramFailureCodeV1::MissingEvidenceClass,
                    format!(
                        "engine {:?} carries {} evidence slots, expected {}",
                        engine.engine,
                        engine.slots.len(),
                        AGGREGATE_PROGRAM_EVIDENCE_CLASSES.len()
                    ),
                ));
            }
            for (index, slot) in engine.slots.iter().enumerate() {
                if slot.class.index() != index {
                    return Err(aggregate_error(
                        AggregateProgramFailureCodeV1::NonCanonicalEncoding,
                        format!(
                            "engine {:?} evidence slots are not in canonical class order",
                            engine.engine
                        ),
                    ));
                }
            }
        }

        if self.receipt_head != self.compute_receipt_head()? {
            return Err(aggregate_error(
                AggregateProgramFailureCodeV1::ReceiptHeadMismatch,
                "aggregate receipt head does not match its canonical body",
            ));
        }
        Ok(())
    }

    /// Canonical JSON bytes of the full receipt, including `receipt_head`.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, AggregateProgramErrorV1> {
        canonical_value(self)
    }

    /// Decodes canonical JSON bytes. Encoding is checked; truthfulness is
    /// established by the caller through `verify`.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AggregateProgramErrorV1> {
        if bytes.len() > AGGREGATE_PROGRAM_MAX_CANONICAL_BYTES {
            return Err(aggregate_error(
                AggregateProgramFailureCodeV1::CanonicalPayloadTooLarge,
                "aggregate receipt canonical payload exceeds its byte bound",
            ));
        }
        let value: Value =
            serde_json::from_slice(bytes).map_err(|error| json_error(error.to_string()))?;
        if canonical_json(&value).as_bytes() != bytes {
            return Err(aggregate_error(
                AggregateProgramFailureCodeV1::NonCanonicalEncoding,
                "aggregate receipt bytes are not canonical sorted-key JSON",
            ));
        }
        serde_json::from_value(value).map_err(|error| json_error(error.to_string()))
    }

    fn compute_receipt_head(&self) -> Result<DigestV1, AggregateProgramErrorV1> {
        let body = json!({
            "schema_version": self.schema_version,
            "program_digest": self.program_digest,
            "assembly_manifest_digest": self.assembly_manifest_digest,
            "source_repository_heads": self.source_repository_heads,
            "engines": self.engines,
        });
        let canonical = canonical_json(&body);
        let mut bytes =
            Vec::with_capacity(AGGREGATE_PROGRAM_RECEIPT_DOMAIN_V1.len() + canonical.len());
        bytes.extend_from_slice(AGGREGATE_PROGRAM_RECEIPT_DOMAIN_V1);
        bytes.extend_from_slice(canonical.as_bytes());
        Ok(DigestV1::from_bytes(sha256(&bytes)))
    }
}

/// Canonical contract manifest for the aggregate receipt schema.
pub fn aggregate_program_contract_manifest_v1() -> Value {
    json!({
        "artifact_profile": "zbf_1_portable_strict",
        "contract_version": AGGREGATE_PROGRAM_SCHEMA_VERSION,
        "name": "zerostack.aggregate_program_receipt.v1",
        "engines": AGGREGATE_PROGRAM_REQUIRED_ENGINES,
        "evidence_classes": AGGREGATE_PROGRAM_EVIDENCE_CLASSES,
        "negative_space": [
            "semantic_sufficiency_of_a_summary",
            "per_engine_authority",
            "native_filesystem_durability",
            "planner_or_worker_or_mcp_implementation",
            "lifecycle_or_gc_implementation",
            "universal_compression_ratio_claims",
        ],
        "receipt_bindings": [
            "schema_version",
            "program_digest",
            "assembly_manifest_digest",
            "source_repository_heads",
            "engines",
            "engine",
            "class",
            "evidence_digest",
            "receipt_head",
        ],
        "failure_semantics": "any missing engine or surface is a typed failure and can never count as Program success",
    })
}

/// Digest of the aggregate receipt contract (domain-prefixed, canonical JSON).
pub fn aggregate_program_contract_digest_v1() -> DigestV1 {
    let canonical = canonical_json(&aggregate_program_contract_manifest_v1());
    let mut bytes =
        Vec::with_capacity(AGGREGATE_PROGRAM_CONTRACT_DOMAIN_V1.len() + canonical.len());
    bytes.extend_from_slice(AGGREGATE_PROGRAM_CONTRACT_DOMAIN_V1);
    bytes.extend_from_slice(canonical.as_bytes());
    DigestV1::from_bytes(sha256(&bytes))
}

fn validate_source_heads(heads: &[AggregateSourceHeadV1]) -> Result<(), AggregateProgramErrorV1> {
    if heads.is_empty() || heads.len() > AGGREGATE_PROGRAM_MAX_SOURCE_HEADS {
        return Err(aggregate_error(
            AggregateProgramFailureCodeV1::InvalidSourceIdentity,
            format!(
                "source head count {} is outside the frozen bound (1..={})",
                heads.len(),
                AGGREGATE_PROGRAM_MAX_SOURCE_HEADS
            ),
        ));
    }
    let mut unique = BTreeSet::new();
    for source in heads {
        let repository_valid = !source.repository.is_empty()
            && source.repository.len() <= AGGREGATE_PROGRAM_MAX_REPOSITORY_BYTES
            && source
                .repository
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte));
        let head_valid = (AGGREGATE_PROGRAM_MIN_HEAD_BYTES..=AGGREGATE_PROGRAM_MAX_HEAD_BYTES)
            .contains(&source.head.len())
            && source.head.bytes().all(|byte| byte.is_ascii_hexdigit());
        if !repository_valid || !head_valid {
            return Err(aggregate_error(
                AggregateProgramFailureCodeV1::InvalidSourceIdentity,
                "source repository or head does not match the frozen grammar",
            ));
        }
        if !unique.insert((source.repository.as_str(), source.head.as_str())) {
            return Err(aggregate_error(
                AggregateProgramFailureCodeV1::InvalidSourceIdentity,
                "source repository heads must be unique",
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateProgramFailureCodeV1 {
    SchemaVersionMismatch,
    MissingEngine,
    UnknownEngine,
    DuplicateEngine,
    MissingEvidenceClass,
    DuplicateEvidenceSlot,
    ZeroDigest,
    NonCanonicalEncoding,
    ReceiptHeadMismatch,
    InvalidSourceIdentity,
    CanonicalPayloadTooLarge,
    Json,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AggregateProgramErrorV1 {
    code: AggregateProgramFailureCodeV1,
    detail: String,
}

impl AggregateProgramErrorV1 {
    pub const fn failure_code(&self) -> AggregateProgramFailureCodeV1 {
        self.code
    }
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for AggregateProgramErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "aggregate program receipt failed ({:?}): {}",
            self.code, self.detail
        )
    }
}
impl std::error::Error for AggregateProgramErrorV1 {}

fn aggregate_error(
    code: AggregateProgramFailureCodeV1,
    detail: impl Into<String>,
) -> AggregateProgramErrorV1 {
    AggregateProgramErrorV1 {
        code,
        detail: detail.into(),
    }
}

fn json_error(detail: String) -> AggregateProgramErrorV1 {
    aggregate_error(AggregateProgramFailureCodeV1::Json, detail)
}

fn canonical_value<T: Serialize>(value: &T) -> Result<Vec<u8>, AggregateProgramErrorV1> {
    let serialized: Value =
        serde_json::to_value(value).map_err(|error| json_error(error.to_string()))?;
    let canonical = canonical_json(&serialized);
    if canonical.len() > AGGREGATE_PROGRAM_MAX_CANONICAL_BYTES {
        return Err(aggregate_error(
            AggregateProgramFailureCodeV1::CanonicalPayloadTooLarge,
            "aggregate receipt canonical payload exceeds its byte bound",
        ));
    }
    Ok(canonical.into_bytes())
}

fn is_zero(digest: &DigestV1) -> bool {
    digest == &DigestV1::ZERO
}

