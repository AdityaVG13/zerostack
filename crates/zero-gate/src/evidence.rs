//! Production Program evidence assembly for FSZero, GraphZero, and TokenZero.
//!
//! A Program-level aggregate may be sealed only from *real, collected*
//! evidence: for every required engine (FSZero, GraphZero, TokenZero) this
//! assembler reads five distinct evidence artifacts — planner, codemode
//! raw-worker, MCP, lifecycle, and applied-GC — and validates each one against
//! its contract, its digest, and its provenance before it is allowed to
//! contribute:
//!
//! - the artifact's digest must bind its exact bytes: `artifact_sha256` is the
//!   SHA-256 over the canonical JSON of the artifact with its own
//!   `artifact_sha256` field zeroed (the codebase's self-digest convention,
//!   like `receipt_head`), and `artifact_bytes` must equal the file's exact
//!   byte length — so a tampered file can never count;
//! - the declared `contract` must be the exact contract of its evidence class;
//! - `source_head` and `hub_head` must equal the manifest's explicit source
//!   head and current hub head (stale or foreign evidence fails closed);
//! - the report inside must parse as the class's report shape and its
//!   self-binding digest must recompute from its fields;
//! - all five classes must be present, with one program identity, committed
//!   closure, matched step counts, lifecycle closure, and applied GC.
//!
//! Only then is the per-engine [`ProgramProof`] constructed, and only then is
//! the [`AggregateProgramReceipt`] built and verified. The aggregate
//! `program_digest` is *derived* from the three real engine proof digests —
//! this module never synthesizes a fixed success digest, and there is no
//! fixture fallback: if any engine or class is missing, partial, stale, or
//! digest-mismatched, assembly fails closed with a typed error.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use zero_abi::{ArtifactOwner, Sha256Digest, canonical_json, sha256, sha256_hex};

use crate::aggregate::{
    AggregateProgramError, AggregateProgramReceipt, AggregateSourceHead, EngineEvidence,
    EvidenceSlot,
};
use crate::program::{ProgramAssemblyError, ProgramProof, ProgramReports};

pub const PROGRAM_EVIDENCE_SCHEMA_VERSION: u16 = 1;
pub const PROGRAM_EVIDENCE_MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub const PROGRAM_EVIDENCE_MIN_HEAD_BYTES: usize = 40;
pub const PROGRAM_EVIDENCE_MAX_HEAD_BYTES: usize = 64;
/// The hub repository name bound into every aggregate receipt.
pub const PROGRAM_EVIDENCE_HUB_REPOSITORY: &str = "ZeroStack";

/// Domain separating the aggregate `program_digest` from its three engine
/// proof digests. The digest is derived from real evidence, never fixed.
const PROGRAM_EVIDENCE_PROGRAM_DOMAIN: &[u8] = b"zerostack.aggregate_program.program\0";

/// Distinct evidence classes an engine must contribute, in canonical order.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EvidenceClass {
    Planner,
    Worker,
    Mcp,
    Lifecycle,
    Gc,
}

impl EvidenceClass {
    pub const ALL: [Self; 5] = [
        Self::Planner,
        Self::Worker,
        Self::Mcp,
        Self::Lifecycle,
        Self::Gc,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Planner => "planner",
            Self::Worker => "worker",
            Self::Mcp => "mcp",
            Self::Lifecycle => "lifecycle",
            Self::Gc => "gc",
        }
    }

    /// The exact contract label this class's artifacts must declare.
    pub const fn contract(self) -> &'static str {
        match self {
            Self::Planner => "zerostack.program.planner",
            Self::Worker => "zerostack.program.worker",
            Self::Mcp => "zerostack.program.mcp",
            Self::Lifecycle => "zerostack.program.lifecycle",
            Self::Gc => "zerostack.program.gc",
        }
    }

    pub const fn aggregate_class(self) -> crate::aggregate::AggregateEvidenceClass {
        match self {
            Self::Planner => crate::aggregate::AggregateEvidenceClass::Planner,
            Self::Worker => crate::aggregate::AggregateEvidenceClass::Worker,
            Self::Mcp => crate::aggregate::AggregateEvidenceClass::Mcp,
            Self::Lifecycle => crate::aggregate::AggregateEvidenceClass::Lifecycle,
            Self::Gc => crate::aggregate::AggregateEvidenceClass::Gc,
        }
    }
}

/// The three engines a Program aggregate requires, in canonical order.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineId {
    FsZero,
    GraphZero,
    TokenZero,
}

impl EngineId {
    pub const ALL: [Self; 3] = [Self::FsZero, Self::GraphZero, Self::TokenZero];

    pub const fn key(self) -> &'static str {
        match self {
            Self::FsZero => "fz",
            Self::GraphZero => "gz",
            Self::TokenZero => "tz",
        }
    }

    /// Repository name bound into the aggregate's source repository heads.
    pub const fn repository(self) -> &'static str {
        match self {
            Self::FsZero => "FSZero",
            Self::GraphZero => "GraphZero",
            Self::TokenZero => "TokenZero",
        }
    }

    pub const fn owner(self) -> ArtifactOwner {
        match self {
            Self::FsZero => ArtifactOwner::FsZero,
            Self::GraphZero => ArtifactOwner::GraphZero,
            Self::TokenZero => ArtifactOwner::TokenZero,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "fz" => Some(Self::FsZero),
            "gz" => Some(Self::GraphZero),
            "tz" => Some(Self::TokenZero),
            _ => None,
        }
    }
}

/// One engine's evidence sources: its exact repository head plus one artifact
/// file per evidence class.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EngineEvidenceSource {
    /// Exact repository head of this engine's source at collection time.
    pub head: String,
    /// Evidence artifact file per class (canonical class keys only).
    pub files: BTreeMap<String, PathBuf>,
}

/// The production assembly manifest: explicit source head, current hub head,
/// the assembly manifest digest to bind, and one evidence source per engine.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramEvidenceManifest {
    pub version: u16,
    /// Exact explicit source repository head the harness was checked out at.
    pub source_head: String,
    /// Current hub repository head at collection time.
    pub hub_head: String,
    /// Assembly manifest digest bound into the aggregate receipt (64 hex).
    pub assembly_manifest_digest: String,
    /// Exactly {fz, gz, tz} evidence sources.
    pub engines: BTreeMap<String, EngineEvidenceSource>,
}

impl ProgramEvidenceManifest {
    /// Decodes a canonical JSON manifest. Encoding is checked; the manifest is
    /// this assembler's sealed input and must be canonical like every receipt.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ProgramEvidenceError> {
        if bytes.len() > PROGRAM_EVIDENCE_MAX_MANIFEST_BYTES {
            return Err(evidence_error(
                ProgramEvidenceFailure::ManifestJson,
                "evidence manifest exceeds its byte bound",
            ));
        }
        let value: Value = serde_json::from_slice(bytes).map_err(|error| {
            evidence_error(ProgramEvidenceFailure::ManifestJson, error.to_string())
        })?;
        if canonical_json(&value).as_bytes() != bytes {
            return Err(evidence_error(
                ProgramEvidenceFailure::NonCanonicalManifest,
                "evidence manifest bytes are not canonical sorted-key JSON",
            ));
        }
        serde_json::from_value(value).map_err(|error| {
            evidence_error(ProgramEvidenceFailure::ManifestJson, error.to_string())
        })
    }

    /// Canonical JSON bytes of the manifest (used by the CLI and tests).
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ProgramEvidenceError> {
        let value = serde_json::to_value(self).map_err(|error| {
            evidence_error(ProgramEvidenceFailure::ManifestJson, error.to_string())
        })?;
        let canonical = canonical_json(&value);
        if canonical.len() > PROGRAM_EVIDENCE_MAX_MANIFEST_BYTES {
            return Err(evidence_error(
                ProgramEvidenceFailure::ManifestJson,
                "evidence manifest exceeds its byte bound",
            ));
        }
        Ok(canonical.into_bytes())
    }
}

/// One collected evidence artifact: the exact bytes are bound by
/// `artifact_sha256`/`artifact_bytes`, and the provenance must match the
/// manifest before the embedded report may contribute.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramEvidenceArtifact {
    /// Exact contract label of the evidence class (e.g.
    /// `zerostack.program.planner`).
    pub contract: String,
    pub schema_version: u16,
    /// Must equal the manifest's exact explicit source head.
    pub source_head: String,
    /// Must equal the manifest's current hub head.
    pub hub_head: String,
    /// SHA-256 (lowercase hex) over the canonical JSON of this artifact with
    /// its own `artifact_sha256` field zeroed (64 `0` chars). Any change to
    /// the contract, schema version, heads, byte length, or report breaks it.
    pub artifact_sha256: String,
    /// Exact byte length of this artifact file, bound into the digest.
    pub artifact_bytes: u64,
    /// The class-shaped report (planner/worker/mcp/lifecycle/gc fields).
    pub report: Value,
}

/// Why Program evidence cannot be aggregated. Assembly fails closed: the first
/// violated invariant determines the error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProgramEvidenceFailure {
    ManifestVersionMismatch,
    InvalidHead,
    InvalidAssemblyManifestDigest,
    NonCanonicalManifest,
    ManifestJson,
    UnknownEngine,
    MissingEngine,
    UnknownEvidenceClass,
    MissingEvidenceClass,
    ArtifactIo,
    ArtifactJson,
    NonCanonicalArtifact,
    SchemaVersionMismatch,
    ContractMismatch,
    StaleHead,
    ArtifactDigestMismatch,
    MalformedReport,
    ProgramAssembly(ProgramAssemblyError),
    Aggregate(AggregateProgramError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProgramEvidenceError {
    code: ProgramEvidenceFailure,
    detail: String,
}

impl ProgramEvidenceError {
    pub const fn failure_code(&self) -> &ProgramEvidenceFailure {
        &self.code
    }
    pub fn detail(&self) -> &str {
        &self.detail
    }

    /// Constructs an artifact I/O failure (used by the CLI's loader).
    pub fn io(detail: impl Into<String>) -> Self {
        evidence_error(ProgramEvidenceFailure::ArtifactIo, detail)
    }
}

impl fmt::Display for ProgramEvidenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "program evidence assembly failed ({:?}): {}",
            self.code, self.detail
        )
    }
}
impl std::error::Error for ProgramEvidenceError {}

fn evidence_error(
    code: ProgramEvidenceFailure,
    detail: impl Into<String>,
) -> ProgramEvidenceError {
    ProgramEvidenceError {
        code,
        detail: detail.into(),
    }
}

/// Head grammar shared with the two-phase `SourceHead` contract: 40..=64
/// lowercase hex bytes.
pub fn valid_head(value: &str) -> bool {
    (PROGRAM_EVIDENCE_MIN_HEAD_BYTES..=PROGRAM_EVIDENCE_MAX_HEAD_BYTES).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Production Program evidence assembly. `load` returns the exact artifact
/// file bytes for a manifest path. Fails closed on any missing, partial,
/// stale, or digest-mismatched input; the returned receipt is verified.
pub fn assemble_program_evidence(
    manifest: &ProgramEvidenceManifest,
    load: impl Fn(&Path) -> Result<Vec<u8>, ProgramEvidenceError>,
) -> Result<AggregateProgramReceipt, ProgramEvidenceError> {
    if manifest.version != PROGRAM_EVIDENCE_SCHEMA_VERSION {
        return Err(evidence_error(
            ProgramEvidenceFailure::ManifestVersionMismatch,
            format!(
                "evidence manifest version {} is not current ({})",
                manifest.version, PROGRAM_EVIDENCE_SCHEMA_VERSION
            ),
        ));
    }
    if !valid_head(&manifest.source_head) {
        return Err(evidence_error(
            ProgramEvidenceFailure::InvalidHead,
            "manifest source_head is not 40..=64 lowercase hex",
        ));
    }
    if !valid_head(&manifest.hub_head) {
        return Err(evidence_error(
            ProgramEvidenceFailure::InvalidHead,
            "manifest hub_head is not 40..=64 lowercase hex",
        ));
    }
    let assembly_manifest_digest =
        Sha256Digest::from_hex(&manifest.assembly_manifest_digest).map_err(|_| {
            evidence_error(
                ProgramEvidenceFailure::InvalidAssemblyManifestDigest,
                "manifest assembly_manifest_digest is not 64 lowercase hex",
            )
        })?;
    if assembly_manifest_digest == Sha256Digest::ZERO {
        return Err(evidence_error(
            ProgramEvidenceFailure::InvalidAssemblyManifestDigest,
            "manifest assembly_manifest_digest is zero",
        ));
    }

    // Engine coverage: exactly {fz, gz, tz}, no more.
    for key in manifest.engines.keys() {
        if EngineId::parse(key).is_none() {
            return Err(evidence_error(
                ProgramEvidenceFailure::UnknownEngine,
                format!("manifest names non-aggregate engine {key:?}"),
            ));
        }
    }
    for engine in EngineId::ALL {
        if !manifest.engines.contains_key(engine.key()) {
            return Err(evidence_error(
                ProgramEvidenceFailure::MissingEngine,
                format!("required engine {:?} has no evidence source", engine.key()),
            ));
        }
    }

    let mut source_repository_heads = vec![AggregateSourceHead {
        repository: PROGRAM_EVIDENCE_HUB_REPOSITORY.into(),
        head: manifest.hub_head.clone(),
    }];

    let mut engine_evidence = Vec::with_capacity(EngineId::ALL.len());
    let mut proofs = Vec::with_capacity(EngineId::ALL.len());

    for engine in EngineId::ALL {
        let source = &manifest.engines[engine.key()];
        if !valid_head(&source.head) {
            return Err(evidence_error(
                ProgramEvidenceFailure::InvalidHead,
                format!(
                    "engine {:?} head is not 40..=64 lowercase hex",
                    engine.key()
                ),
            ));
        }
        source_repository_heads.push(AggregateSourceHead {
            repository: engine.repository().into(),
            head: source.head.clone(),
        });

        // Class coverage: exactly the five classes, no more. Each artifact is
        // loaded once, validated against its contract/digest/provenance, and
        // parsed into its class-shaped report.
        for key in source.files.keys() {
            if !EvidenceClass::ALL.iter().any(|class| class.key() == key) {
                return Err(evidence_error(
                    ProgramEvidenceFailure::UnknownEvidenceClass,
                    format!(
                        "engine {:?} names unknown evidence class {key:?}",
                        engine.key()
                    ),
                ));
            }
        }
        let mut reports = ProgramReports::new();
        for class in EvidenceClass::ALL {
            let Some(path) = source.files.get(class.key()) else {
                return Err(evidence_error(
                    ProgramEvidenceFailure::MissingEvidenceClass,
                    format!(
                        "engine {:?} has no {:?} evidence artifact",
                        engine.key(),
                        class.key()
                    ),
                ));
            };
            let bytes = load(path)?;
            let artifact: ProgramEvidenceArtifact =
                serde_json::from_slice(&bytes).map_err(|error| {
                    evidence_error(
                        ProgramEvidenceFailure::ArtifactJson,
                        format!(
                            "engine {:?} {:?} artifact is not valid JSON: {error}",
                            engine.key(),
                            class.key()
                        ),
                    )
                })?;
            validate_artifact(engine, class, &artifact, &bytes, manifest)?;
            reports = attach_report(reports, engine, class, &artifact)?;
        }

        // Slots bind each class's validated self-digest into the aggregate;
        // they are taken before `assemble` consumes the reports.
        let slots: Vec<EvidenceSlot> = EvidenceClass::ALL
            .iter()
            .map(|class| EvidenceSlot {
                class: class.aggregate_class(),
                evidence_digest: Sha256Digest::from_bytes(report_digest(*class, &reports)),
            })
            .collect();

        let proof = reports.assemble().map_err(|error| {
            evidence_error(
                ProgramEvidenceFailure::ProgramAssembly(error),
                format!("engine {:?} reports cannot assemble", engine.key()),
            )
        })?;

        engine_evidence.push(EngineEvidence {
            engine: engine.owner(),
            slots,
        });
        proofs.push(proof);
    }

    let program_digest = derive_aggregate_program_digest(&proofs);
    AggregateProgramReceipt::new(
        program_digest,
        assembly_manifest_digest,
        source_repository_heads,
        engine_evidence,
    )
    .map_err(|error| {
        evidence_error(
            ProgramEvidenceFailure::Aggregate(error.clone()),
            error.to_string(),
        )
    })
}

/// Validates one artifact against its contract, digest, and provenance.
fn validate_artifact(
    engine: EngineId,
    class: EvidenceClass,
    artifact: &ProgramEvidenceArtifact,
    bytes: &[u8],
    manifest: &ProgramEvidenceManifest,
) -> Result<(), ProgramEvidenceError> {
    if artifact.schema_version != PROGRAM_EVIDENCE_SCHEMA_VERSION {
        return Err(evidence_error(
            ProgramEvidenceFailure::SchemaVersionMismatch,
            format!(
                "engine {:?} {:?} artifact schema version {} is not current ({})",
                engine.key(),
                class.key(),
                artifact.schema_version,
                PROGRAM_EVIDENCE_SCHEMA_VERSION
            ),
        ));
    }
    if artifact.contract != class.contract() {
        return Err(evidence_error(
            ProgramEvidenceFailure::ContractMismatch,
            format!(
                "engine {:?} {:?} artifact declares contract {:?}, expected {:?}",
                engine.key(),
                class.key(),
                artifact.contract,
                class.contract()
            ),
        ));
    }
    if artifact.source_head != manifest.source_head || artifact.hub_head != manifest.hub_head {
        return Err(evidence_error(
            ProgramEvidenceFailure::StaleHead,
            format!(
                "engine {:?} {:?} artifact provenance is stale or foreign: source {:?}/{:?}, hub {:?}/{:?}",
                engine.key(),
                class.key(),
                artifact.source_head,
                manifest.source_head,
                artifact.hub_head,
                manifest.hub_head
            ),
        ));
    }
    if artifact.artifact_bytes != bytes.len() as u64
        || !valid_sha256(&artifact.artifact_sha256)
        || artifact_digest(artifact) != artifact.artifact_sha256
    {
        return Err(evidence_error(
            ProgramEvidenceFailure::ArtifactDigestMismatch,
            format!(
                "engine {:?} {:?} artifact digest does not match its exact bytes",
                engine.key(),
                class.key()
            ),
        ));
    }
    let value = serde_json::to_value(artifact).map_err(|error| {
        evidence_error(
            ProgramEvidenceFailure::ArtifactJson,
            format!("validated artifact cannot serialize: {error}"),
        )
    })?;
    if canonical_json(&value).as_bytes() != bytes {
        return Err(evidence_error(
            ProgramEvidenceFailure::NonCanonicalArtifact,
            format!(
                "engine {:?} {:?} artifact bytes are not canonical sorted-key JSON",
                engine.key(),
                class.key()
            ),
        ));
    }
    Ok(())
}

/// SHA-256 over the canonical JSON of the artifact with its own
/// `artifact_sha256` field zeroed. The digest field cannot digest itself, so
/// it is excluded the same way `receipt_head` is excluded from a receipt body.
fn artifact_digest(artifact: &ProgramEvidenceArtifact) -> String {
    let mut value = serde_json::to_value(artifact).expect("artifact serializes");
    value["artifact_sha256"] = Value::String("0".repeat(64));
    sha256_hex(canonical_json(&value).as_bytes())
}

/// Parses one validated artifact's report into its class slot.
fn attach_report(
    mut reports: ProgramReports,
    engine: EngineId,
    class: EvidenceClass,
    artifact: &ProgramEvidenceArtifact,
) -> Result<ProgramReports, ProgramEvidenceError> {
    reports = match class {
        EvidenceClass::Planner => reports.planner(parse_report(engine, class, &artifact.report)?),
        EvidenceClass::Worker => reports.worker(parse_report(engine, class, &artifact.report)?),
        EvidenceClass::Mcp => reports.mcp(parse_report(engine, class, &artifact.report)?),
        EvidenceClass::Lifecycle => {
            reports.lifecycle(parse_report(engine, class, &artifact.report)?)
        }
        EvidenceClass::Gc => reports.gc(parse_report(engine, class, &artifact.report)?),
    };
    Ok(reports)
}

fn parse_report<T>(
    engine: EngineId,
    class: EvidenceClass,
    value: &Value,
) -> Result<T, ProgramEvidenceError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value::<T>(value.clone()).map_err(|error| {
        evidence_error(
            ProgramEvidenceFailure::MalformedReport,
            format!(
                "engine {:?} {:?} report is not shaped like {:?}: {error}",
                engine.key(),
                class.key(),
                class.contract()
            ),
        )
    })
}

/// The self-binding digest of one class's report, after validation.
fn report_digest(class: EvidenceClass, reports: &ProgramReports) -> [u8; 32] {
    match class {
        EvidenceClass::Planner => reports
            .planner_report()
            .expect("validated planner report")
            .digest(),
        EvidenceClass::Worker => reports
            .worker_report()
            .expect("validated worker report")
            .digest(),
        EvidenceClass::Mcp => reports.mcp_report().expect("validated mcp report").digest(),
        EvidenceClass::Lifecycle => reports
            .lifecycle_report()
            .expect("validated lifecycle report")
            .digest(),
        EvidenceClass::Gc => reports.gc_report().expect("validated gc report").digest(),
    }
}

/// Derives the aggregate program digest from the three real engine proof
/// digests, in canonical engine order. Never a fixed or synthesized value.
fn derive_aggregate_program_digest(proofs: &[ProgramProof]) -> Sha256Digest {
    let mut bytes = Vec::with_capacity(PROGRAM_EVIDENCE_PROGRAM_DOMAIN.len() + 32 * proofs.len());
    bytes.extend_from_slice(PROGRAM_EVIDENCE_PROGRAM_DOMAIN);
    for proof in proofs {
        bytes.extend_from_slice(&proof.program_digest());
    }
    Sha256Digest::from_bytes(sha256(&bytes))
}

