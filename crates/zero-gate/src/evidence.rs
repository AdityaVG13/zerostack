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
//! the [`AggregateProgramReceiptV1`] built and verified. The aggregate
//! `program_digest` is *derived* from the three real engine proof digests —
//! this module never synthesizes a fixed success digest, and there is no
//! fixture fallback: if any engine or class is missing, partial, stale, or
//! digest-mismatched, assembly fails closed with a typed error.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use zero_abi::{ArtifactOwnerV1, DigestV1, canonical_json, sha256, sha256_hex};

use crate::aggregate::{
    AggregateProgramErrorV1, AggregateProgramReceiptV1, AggregateSourceHeadV1, EngineEvidenceV1,
    EvidenceSlotV1,
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
const PROGRAM_EVIDENCE_PROGRAM_DOMAIN: &[u8] = b"zerostack.aggregate_program.program.v1\0";

/// Distinct evidence classes an engine must contribute, in canonical order.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EvidenceClassV1 {
    Planner,
    Worker,
    Mcp,
    Lifecycle,
    Gc,
}

impl EvidenceClassV1 {
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
            Self::Planner => "zerostack.program.planner.v1",
            Self::Worker => "zerostack.program.worker.v1",
            Self::Mcp => "zerostack.program.mcp.v1",
            Self::Lifecycle => "zerostack.program.lifecycle.v1",
            Self::Gc => "zerostack.program.gc.v1",
        }
    }

    pub const fn aggregate_class(self) -> crate::aggregate::AggregateEvidenceClassV1 {
        match self {
            Self::Planner => crate::aggregate::AggregateEvidenceClassV1::Planner,
            Self::Worker => crate::aggregate::AggregateEvidenceClassV1::Worker,
            Self::Mcp => crate::aggregate::AggregateEvidenceClassV1::Mcp,
            Self::Lifecycle => crate::aggregate::AggregateEvidenceClassV1::Lifecycle,
            Self::Gc => crate::aggregate::AggregateEvidenceClassV1::Gc,
        }
    }
}

/// The three engines a Program aggregate requires, in canonical order.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineIdV1 {
    FsZero,
    GraphZero,
    TokenZero,
}

impl EngineIdV1 {
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

    pub const fn owner(self) -> ArtifactOwnerV1 {
        match self {
            Self::FsZero => ArtifactOwnerV1::FsZero,
            Self::GraphZero => ArtifactOwnerV1::GraphZero,
            Self::TokenZero => ArtifactOwnerV1::TokenZero,
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
pub struct EngineEvidenceSourceV1 {
    /// Exact repository head of this engine's source at collection time.
    pub head: String,
    /// Evidence artifact file per class (canonical class keys only).
    pub files: BTreeMap<String, PathBuf>,
}

/// The production assembly manifest: explicit source head, current hub head,
/// the assembly manifest digest to bind, and one evidence source per engine.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramEvidenceManifestV1 {
    pub version: u16,
    /// Exact explicit source repository head the harness was checked out at.
    pub source_head: String,
    /// Current hub repository head at collection time.
    pub hub_head: String,
    /// Assembly manifest digest bound into the aggregate receipt (64 hex).
    pub assembly_manifest_digest: String,
    /// Exactly {fz, gz, tz} evidence sources.
    pub engines: BTreeMap<String, EngineEvidenceSourceV1>,
}

impl ProgramEvidenceManifestV1 {
    /// Decodes a canonical JSON manifest. Encoding is checked; the manifest is
    /// this assembler's sealed input and must be canonical like every receipt.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ProgramEvidenceErrorV1> {
        if bytes.len() > PROGRAM_EVIDENCE_MAX_MANIFEST_BYTES {
            return Err(evidence_error(
                ProgramEvidenceFailureV1::ManifestJson,
                "evidence manifest exceeds its byte bound",
            ));
        }
        let value: Value = serde_json::from_slice(bytes).map_err(|error| {
            evidence_error(ProgramEvidenceFailureV1::ManifestJson, error.to_string())
        })?;
        if canonical_json(&value).as_bytes() != bytes {
            return Err(evidence_error(
                ProgramEvidenceFailureV1::NonCanonicalManifest,
                "evidence manifest bytes are not canonical sorted-key JSON",
            ));
        }
        serde_json::from_value(value).map_err(|error| {
            evidence_error(ProgramEvidenceFailureV1::ManifestJson, error.to_string())
        })
    }

    /// Canonical JSON bytes of the manifest (used by the CLI and tests).
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ProgramEvidenceErrorV1> {
        let value = serde_json::to_value(self).map_err(|error| {
            evidence_error(ProgramEvidenceFailureV1::ManifestJson, error.to_string())
        })?;
        let canonical = canonical_json(&value);
        if canonical.len() > PROGRAM_EVIDENCE_MAX_MANIFEST_BYTES {
            return Err(evidence_error(
                ProgramEvidenceFailureV1::ManifestJson,
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
pub struct ProgramEvidenceArtifactV1 {
    /// Exact contract label of the evidence class (e.g.
    /// `zerostack.program.planner.v1`).
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
pub enum ProgramEvidenceFailureV1 {
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
    Aggregate(AggregateProgramErrorV1),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProgramEvidenceErrorV1 {
    code: ProgramEvidenceFailureV1,
    detail: String,
}

impl ProgramEvidenceErrorV1 {
    pub const fn failure_code(&self) -> &ProgramEvidenceFailureV1 {
        &self.code
    }
    pub fn detail(&self) -> &str {
        &self.detail
    }

    /// Constructs an artifact I/O failure (used by the CLI's loader).
    pub fn io(detail: impl Into<String>) -> Self {
        evidence_error(ProgramEvidenceFailureV1::ArtifactIo, detail)
    }
}

impl fmt::Display for ProgramEvidenceErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "program evidence assembly failed ({:?}): {}",
            self.code, self.detail
        )
    }
}
impl std::error::Error for ProgramEvidenceErrorV1 {}

fn evidence_error(
    code: ProgramEvidenceFailureV1,
    detail: impl Into<String>,
) -> ProgramEvidenceErrorV1 {
    ProgramEvidenceErrorV1 {
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
    manifest: &ProgramEvidenceManifestV1,
    load: impl Fn(&Path) -> Result<Vec<u8>, ProgramEvidenceErrorV1>,
) -> Result<AggregateProgramReceiptV1, ProgramEvidenceErrorV1> {
    if manifest.version != PROGRAM_EVIDENCE_SCHEMA_VERSION {
        return Err(evidence_error(
            ProgramEvidenceFailureV1::ManifestVersionMismatch,
            format!(
                "evidence manifest version {} is not current ({})",
                manifest.version, PROGRAM_EVIDENCE_SCHEMA_VERSION
            ),
        ));
    }
    if !valid_head(&manifest.source_head) {
        return Err(evidence_error(
            ProgramEvidenceFailureV1::InvalidHead,
            "manifest source_head is not 40..=64 lowercase hex",
        ));
    }
    if !valid_head(&manifest.hub_head) {
        return Err(evidence_error(
            ProgramEvidenceFailureV1::InvalidHead,
            "manifest hub_head is not 40..=64 lowercase hex",
        ));
    }
    let assembly_manifest_digest =
        DigestV1::from_hex(&manifest.assembly_manifest_digest).map_err(|_| {
            evidence_error(
                ProgramEvidenceFailureV1::InvalidAssemblyManifestDigest,
                "manifest assembly_manifest_digest is not 64 lowercase hex",
            )
        })?;
    if assembly_manifest_digest == DigestV1::ZERO {
        return Err(evidence_error(
            ProgramEvidenceFailureV1::InvalidAssemblyManifestDigest,
            "manifest assembly_manifest_digest is zero",
        ));
    }

    // Engine coverage: exactly {fz, gz, tz}, no more.
    for key in manifest.engines.keys() {
        if EngineIdV1::parse(key).is_none() {
            return Err(evidence_error(
                ProgramEvidenceFailureV1::UnknownEngine,
                format!("manifest names non-aggregate engine {key:?}"),
            ));
        }
    }
    for engine in EngineIdV1::ALL {
        if !manifest.engines.contains_key(engine.key()) {
            return Err(evidence_error(
                ProgramEvidenceFailureV1::MissingEngine,
                format!("required engine {:?} has no evidence source", engine.key()),
            ));
        }
    }

    let mut source_repository_heads = vec![AggregateSourceHeadV1 {
        repository: PROGRAM_EVIDENCE_HUB_REPOSITORY.into(),
        head: manifest.hub_head.clone(),
    }];

    let mut engine_evidence = Vec::with_capacity(EngineIdV1::ALL.len());
    let mut proofs = Vec::with_capacity(EngineIdV1::ALL.len());

    for engine in EngineIdV1::ALL {
        let source = &manifest.engines[engine.key()];
        if !valid_head(&source.head) {
            return Err(evidence_error(
                ProgramEvidenceFailureV1::InvalidHead,
                format!(
                    "engine {:?} head is not 40..=64 lowercase hex",
                    engine.key()
                ),
            ));
        }
        source_repository_heads.push(AggregateSourceHeadV1 {
            repository: engine.repository().into(),
            head: source.head.clone(),
        });

        // Class coverage: exactly the five classes, no more. Each artifact is
        // loaded once, validated against its contract/digest/provenance, and
        // parsed into its class-shaped report.
        for key in source.files.keys() {
            if !EvidenceClassV1::ALL.iter().any(|class| class.key() == key) {
                return Err(evidence_error(
                    ProgramEvidenceFailureV1::UnknownEvidenceClass,
                    format!(
                        "engine {:?} names unknown evidence class {key:?}",
                        engine.key()
                    ),
                ));
            }
        }
        let mut reports = ProgramReports::new();
        for class in EvidenceClassV1::ALL {
            let Some(path) = source.files.get(class.key()) else {
                return Err(evidence_error(
                    ProgramEvidenceFailureV1::MissingEvidenceClass,
                    format!(
                        "engine {:?} has no {:?} evidence artifact",
                        engine.key(),
                        class.key()
                    ),
                ));
            };
            let bytes = load(path)?;
            let artifact: ProgramEvidenceArtifactV1 =
                serde_json::from_slice(&bytes).map_err(|error| {
                    evidence_error(
                        ProgramEvidenceFailureV1::ArtifactJson,
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
        let slots: Vec<EvidenceSlotV1> = EvidenceClassV1::ALL
            .iter()
            .map(|class| EvidenceSlotV1 {
                class: class.aggregate_class(),
                evidence_digest: DigestV1::from_bytes(report_digest(*class, &reports)),
            })
            .collect();

        let proof = reports.assemble().map_err(|error| {
            evidence_error(
                ProgramEvidenceFailureV1::ProgramAssembly(error),
                format!("engine {:?} reports cannot assemble", engine.key()),
            )
        })?;

        engine_evidence.push(EngineEvidenceV1 {
            engine: engine.owner(),
            slots,
        });
        proofs.push(proof);
    }

    let program_digest = derive_aggregate_program_digest(&proofs);
    AggregateProgramReceiptV1::new(
        program_digest,
        assembly_manifest_digest,
        source_repository_heads,
        engine_evidence,
    )
    .map_err(|error| {
        evidence_error(
            ProgramEvidenceFailureV1::Aggregate(error.clone()),
            error.to_string(),
        )
    })
}

/// Validates one artifact against its contract, digest, and provenance.
fn validate_artifact(
    engine: EngineIdV1,
    class: EvidenceClassV1,
    artifact: &ProgramEvidenceArtifactV1,
    bytes: &[u8],
    manifest: &ProgramEvidenceManifestV1,
) -> Result<(), ProgramEvidenceErrorV1> {
    if artifact.schema_version != PROGRAM_EVIDENCE_SCHEMA_VERSION {
        return Err(evidence_error(
            ProgramEvidenceFailureV1::SchemaVersionMismatch,
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
            ProgramEvidenceFailureV1::ContractMismatch,
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
            ProgramEvidenceFailureV1::StaleHead,
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
            ProgramEvidenceFailureV1::ArtifactDigestMismatch,
            format!(
                "engine {:?} {:?} artifact digest does not match its exact bytes",
                engine.key(),
                class.key()
            ),
        ));
    }
    let value = serde_json::to_value(artifact).map_err(|error| {
        evidence_error(
            ProgramEvidenceFailureV1::ArtifactJson,
            format!("validated artifact cannot serialize: {error}"),
        )
    })?;
    if canonical_json(&value).as_bytes() != bytes {
        return Err(evidence_error(
            ProgramEvidenceFailureV1::NonCanonicalArtifact,
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
fn artifact_digest(artifact: &ProgramEvidenceArtifactV1) -> String {
    let mut value = serde_json::to_value(artifact).expect("artifact serializes");
    value["artifact_sha256"] = Value::String("0".repeat(64));
    sha256_hex(canonical_json(&value).as_bytes())
}

/// Parses one validated artifact's report into its class slot.
fn attach_report(
    mut reports: ProgramReports,
    engine: EngineIdV1,
    class: EvidenceClassV1,
    artifact: &ProgramEvidenceArtifactV1,
) -> Result<ProgramReports, ProgramEvidenceErrorV1> {
    reports = match class {
        EvidenceClassV1::Planner => reports.planner(parse_report(engine, class, &artifact.report)?),
        EvidenceClassV1::Worker => reports.worker(parse_report(engine, class, &artifact.report)?),
        EvidenceClassV1::Mcp => reports.mcp(parse_report(engine, class, &artifact.report)?),
        EvidenceClassV1::Lifecycle => {
            reports.lifecycle(parse_report(engine, class, &artifact.report)?)
        }
        EvidenceClassV1::Gc => reports.gc(parse_report(engine, class, &artifact.report)?),
    };
    Ok(reports)
}

fn parse_report<T>(
    engine: EngineIdV1,
    class: EvidenceClassV1,
    value: &Value,
) -> Result<T, ProgramEvidenceErrorV1>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value::<T>(value.clone()).map_err(|error| {
        evidence_error(
            ProgramEvidenceFailureV1::MalformedReport,
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
fn report_digest(class: EvidenceClassV1, reports: &ProgramReports) -> [u8; 32] {
    match class {
        EvidenceClassV1::Planner => reports
            .planner_report()
            .expect("validated planner report")
            .digest(),
        EvidenceClassV1::Worker => reports
            .worker_report()
            .expect("validated worker report")
            .digest(),
        EvidenceClassV1::Mcp => reports.mcp_report().expect("validated mcp report").digest(),
        EvidenceClassV1::Lifecycle => reports
            .lifecycle_report()
            .expect("validated lifecycle report")
            .digest(),
        EvidenceClassV1::Gc => reports.gc_report().expect("validated gc report").digest(),
    }
}

/// Derives the aggregate program digest from the three real engine proof
/// digests, in canonical engine order. Never a fixed or synthesized value.
fn derive_aggregate_program_digest(proofs: &[ProgramProof]) -> DigestV1 {
    let mut bytes = Vec::with_capacity(PROGRAM_EVIDENCE_PROGRAM_DOMAIN.len() + 32 * proofs.len());
    bytes.extend_from_slice(PROGRAM_EVIDENCE_PROGRAM_DOMAIN);
    for proof in proofs {
        bytes.extend_from_slice(&proof.program_digest());
    }
    DigestV1::from_bytes(sha256(&bytes))
}

