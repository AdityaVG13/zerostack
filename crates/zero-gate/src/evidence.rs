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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::{
        AppliedGcEvidenceV1, GcProducerEpochV1, GcReport, LifecycleReport, LifecycleState,
        McpReport, PlannerReport, ProgramUsage, WorkerClosureKind, WorkerReport,
        mcp_evidence_digest,
    };
    use serde_json::json;
    use std::collections::BTreeMap;
    use tempfile::TempDir;
    use zero_abi::EngineIdentity;
    use zero_store::{
        GC_RECORD_TYPE_DRY_RUN, GC_SCHEMA_VERSION, GcCandidate, GcRunReceipt, GcRunState,
        GcVerdict, gc_contract_digest_hex,
    };

    fn program_id() -> [u8; 32] {
        sha256(b"program-evidence-test")
    }

    fn applied_gc(id: [u8; 32]) -> GcReport {
        let hashes = (0..7)
            .map(|index| format!("{index:064x}"))
            .collect::<Vec<_>>();
        let receipt = GcRunReceipt {
            schema_version: GC_SCHEMA_VERSION.into(),
            record_type: GC_RECORD_TYPE_DRY_RUN.into(),
            store_contract_digest: gc_contract_digest_hex(),
            run_id: "evidence-test-applied".into(),
            store_root: "/tmp/evidence-test-applied".into(),
            evaluated_at: "2026-08-11T00:00:00.000Z".into(),
            apply: true,
            state: GcRunState::Complete,
            objects: hashes
                .iter()
                .map(|hash| GcCandidate {
                    blob_hash: hash.clone(),
                    verdict: GcVerdict::Collect,
                    reason_codes: vec!["no-live-reference".into()],
                    evidence: vec!["test applied receipt".into()],
                })
                .collect(),
            planned: hashes.clone(),
            deleted: hashes,
        };
        let epochs = [
            EngineIdentity::FsZero,
            EngineIdentity::GraphZero,
            EngineIdentity::TokenZero,
        ]
        .into_iter()
        .map(|engine| GcProducerEpochV1 { engine, epoch: 1 })
        .collect();
        let applied = AppliedGcEvidenceV1::new(receipt, epochs, 4096).unwrap();
        GcReport::new_applied(1, id, applied)
    }

    fn report_values(seed: u8) -> BTreeMap<&'static str, Value> {
        let id = program_id();
        let tools = sha256(&[seed; 8]);
        let mcp = McpReport::new(1, id, 2, 5, tools);
        let planner = PlannerReport::new(1, id, sha256(&[seed; 4]), 3);
        let worker = WorkerReport::new(
            1,
            id,
            sha256(&[seed; 5]),
            3,
            WorkerClosureKind::Commit,
            mcp_evidence_digest(2, 5, tools),
            sha256(&[seed; 6]),
            sha256(&[seed; 7]),
            ProgramUsage {
                cpu_ns: 100,
                memory_bytes: 1024,
                io_bytes: 512,
            },
        );
        let lifecycle = LifecycleReport::new(1, id, 5, 3, LifecycleState::Closed);
        let gc = applied_gc(id);
        let mut map = BTreeMap::new();
        map.insert(
            "planner",
            serde_json::to_value(&planner).expect("planner serializes"),
        );
        map.insert(
            "worker",
            serde_json::to_value(&worker).expect("worker serializes"),
        );
        map.insert("mcp", serde_json::to_value(&mcp).expect("mcp serializes"));
        map.insert(
            "lifecycle",
            serde_json::to_value(&lifecycle).expect("lifecycle serializes"),
        );
        map.insert("gc", serde_json::to_value(&gc).expect("gc serializes"));
        map
    }

    /// Builds one sealed artifact envelope: `artifact_sha256` is the digest
    /// over the canonical JSON with the sha field zeroed, and `artifact_bytes`
    /// is the exact final byte length. Mirrors `artifact_digest` exactly.
    fn sealed_artifact_bytes(mut value: Value) -> Vec<u8> {
        value["artifact_sha256"] = json!("0".repeat(64));
        // artifact_bytes is part of the digest; fix its final value first
        // (the sha field is fixed-width, so patching the digest cannot change
        // the byte length).
        let mut length = 0u64;
        for _ in 0..4 {
            value["artifact_bytes"] = json!(length);
            let canonical = canonical_json(&value);
            let next = canonical.len() as u64;
            if next == length {
                break;
            }
            length = next;
        }
        let sha = sha256_hex(canonical_json(&value).as_bytes());
        value["artifact_sha256"] = json!(sha);
        canonical_json(&value).into_bytes()
    }

    fn artifact_bytes(
        class: EvidenceClassV1,
        report: &Value,
        source_head: &str,
        hub_head: &str,
    ) -> Vec<u8> {
        sealed_artifact_bytes(json!({
            "contract": class.contract(),
            "schema_version": 1,
            "source_head": source_head,
            "hub_head": hub_head,
            "artifact_sha256": "0".repeat(64),
            "artifact_bytes": 0,
            "report": report.clone(),
        }))
    }

    fn head(byte: u8) -> String {
        format!("{:02x}", byte).repeat(20)
    }

    fn source_head() -> String {
        head(0x11)
    }
    fn hub_head() -> String {
        head(0x22)
    }
    fn engine_head() -> String {
        head(0x33)
    }

    fn files_for(
        engine: &str,
        seed: u8,
        source: &str,
        hub: &str,
        base: &Path,
    ) -> BTreeMap<String, PathBuf> {
        let dir = base.join(engine);
        std::fs::create_dir_all(&dir).expect("create artifact dir");
        let values = report_values(seed);
        let mut files = BTreeMap::new();
        for class in EvidenceClassV1::ALL {
            let path = dir.join(format!("{}.json", class.key()));
            let bytes = artifact_bytes(class, &values[class.key()], source, hub);
            std::fs::write(&path, &bytes).expect("write artifact");
            files.insert(class.key().to_owned(), path);
        }
        files
    }

    fn engine_source(head: &str, files: BTreeMap<String, PathBuf>) -> EngineEvidenceSourceV1 {
        EngineEvidenceSourceV1 {
            head: head.to_owned(),
            files,
        }
    }

    fn valid_manifest(base: &Path) -> ProgramEvidenceManifestV1 {
        let source = source_head();
        let hub = hub_head();
        let mut engines = BTreeMap::new();
        for (index, engine) in EngineIdV1::ALL.iter().enumerate() {
            engines.insert(
                engine.key().to_owned(),
                engine_source(
                    &engine_head(),
                    files_for(engine.key(), index as u8 + 1, &source, &hub, base),
                ),
            );
        }
        ProgramEvidenceManifestV1 {
            version: 1,
            source_head: source,
            hub_head: hub,
            assembly_manifest_digest: "ab".repeat(32),
            engines,
        }
    }

    fn loader_for(
        files: &BTreeMap<PathBuf, Vec<u8>>,
    ) -> impl Fn(&Path) -> Result<Vec<u8>, ProgramEvidenceErrorV1> + '_ {
        move |path: &Path| {
            files
                .get(path)
                .cloned()
                .ok_or_else(|| ProgramEvidenceErrorV1::io(format!("missing {}", path.display())))
        }
    }

    fn read_all(manifest: &ProgramEvidenceManifestV1) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        for source in manifest.engines.values() {
            for path in source.files.values() {
                let bytes = std::fs::read(path).expect("read artifact");
                files.insert(path.clone(), bytes);
            }
        }
        files
    }

    /// Re-seals an artifact file after mutation so the declared sha/bytes
    /// again bind the (mutated) exact bytes.
    fn rewrite_artifact(path: &Path, value: Value) {
        std::fs::write(path, sealed_artifact_bytes(value)).unwrap();
    }

    #[test]
    fn valid_evidence_assembles_into_a_verified_receipt() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        let files = read_all(&manifest);
        let receipt = assemble_program_evidence(&manifest, loader_for(&files)).expect("assembles");
        receipt.verify().expect("receipt verifies");
        assert_eq!(receipt.engines.len(), 3);
        assert_eq!(receipt.source_repository_heads.len(), 4);
        assert_eq!(receipt.source_repository_heads[0].repository, "ZeroStack");
        assert_eq!(receipt.source_repository_heads[0].head, hub_head());
        assert_ne!(receipt.program_digest, DigestV1::ZERO);
        // The aggregate program digest is derived from real proof digests.
        let again = assemble_program_evidence(&manifest, loader_for(&files)).expect("assembles");
        assert_eq!(again.program_digest, receipt.program_digest);
    }

    #[test]
    fn derived_program_digest_changes_with_real_evidence() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        let files = read_all(&manifest);
        let baseline = assemble_program_evidence(&manifest, loader_for(&files)).unwrap();
        // Different planner plan digest for TokenZero -> different proof ->
        // different derived aggregate digest (never a fixed success digest).
        let changed = manifest.clone();
        let path = changed.engines["tz"].files["planner"].clone();
        let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["report"] = serde_json::to_value(&PlannerReport::new(
            1,
            program_id(),
            sha256(b"different-plan"),
            3,
        ))
        .unwrap();
        rewrite_artifact(&path, value);
        let files = read_all(&changed);
        let derived = assemble_program_evidence(&changed, loader_for(&files)).unwrap();
        assert_ne!(derived.program_digest, baseline.program_digest);
    }

    #[test]
    fn missing_engine_can_never_assemble() {
        let base = TempDir::new().unwrap();
        let mut manifest = valid_manifest(base.path());
        manifest.engines.remove("tz");
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::MissingEngine
        );
    }

    #[test]
    fn unknown_engine_can_never_assemble() {
        let base = TempDir::new().unwrap();
        let mut manifest = valid_manifest(base.path());
        manifest.engines.insert(
            "xx".into(),
            engine_source(
                &engine_head(),
                files_for("xx", 1, &source_head(), &hub_head(), base.path()),
            ),
        );
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::UnknownEngine
        );
    }

    #[test]
    fn missing_evidence_class_can_never_assemble() {
        let base = TempDir::new().unwrap();
        let mut manifest = valid_manifest(base.path());
        manifest.engines.get_mut("fz").unwrap().files.remove("gc");
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::MissingEvidenceClass
        );
    }

    #[test]
    fn unknown_evidence_class_can_never_assemble() {
        let base = TempDir::new().unwrap();
        let mut manifest = valid_manifest(base.path());
        let existing = manifest.engines["fz"].files["planner"].clone();
        manifest
            .engines
            .get_mut("fz")
            .unwrap()
            .files
            .insert("telemetry".into(), existing);
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::UnknownEvidenceClass
        );
    }

    #[test]
    fn partial_evidence_artifact_fails_closed() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        // The manifest names the artifact but the loader cannot produce it.
        let mut files = read_all(&manifest);
        files.remove(&manifest.engines["gz"].files["lifecycle"].clone());
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::ArtifactIo
        );
    }

    #[test]
    fn stale_hub_head_fails_closed() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        // Re-collect one artifact bound to a different hub head.
        let path = manifest.engines["fz"].files["mcp"].clone();
        let bytes = artifact_bytes(
            EvidenceClassV1::Mcp,
            &report_values(1)["mcp"],
            &manifest.source_head,
            &head(0x99),
        );
        std::fs::write(&path, &bytes).unwrap();
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::StaleHead
        );
    }

    #[test]
    fn stale_source_head_fails_closed() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        let path = manifest.engines["gz"].files["worker"].clone();
        let bytes = artifact_bytes(
            EvidenceClassV1::Worker,
            &report_values(2)["worker"],
            &head(0x88),
            &manifest.hub_head,
        );
        std::fs::write(&path, &bytes).unwrap();
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::StaleHead
        );
    }

    #[test]
    fn tampered_artifact_digest_fails_closed() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        let path = manifest.engines["tz"].files["gc"].clone();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.push(b'\n'); // tamper after collection: declared digest no longer binds
        std::fs::write(&path, &bytes).unwrap();
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::ArtifactDigestMismatch
        );
    }

    #[test]
    fn same_length_noncanonical_artifact_fails_closed() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        let path = manifest.engines["tz"].files["gc"].clone();
        let original = std::fs::read(&path).unwrap();
        let value: Value = serde_json::from_slice(&original).unwrap();
        let keys = [
            "schema_version",
            "contract",
            "source_head",
            "hub_head",
            "artifact_sha256",
            "artifact_bytes",
            "report",
        ];
        let fields: Vec<String> = keys
            .iter()
            .map(|key| {
                format!(
                    "{}:{}",
                    serde_json::to_string(key).unwrap(),
                    canonical_json(&value[*key])
                )
            })
            .collect();
        let reordered = format!("{{{}}}", fields.join(",")).into_bytes();
        assert_eq!(reordered.len(), original.len());
        assert_ne!(reordered, original);
        std::fs::write(&path, reordered).unwrap();
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::NonCanonicalArtifact
        );
    }

    #[test]
    fn contract_mismatch_fails_closed() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        // A planner slot claiming the worker contract.
        let path = manifest.engines["fz"].files["planner"].clone();
        let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["contract"] = json!("zerostack.program.worker.v1");
        rewrite_artifact(&path, value);
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::ContractMismatch
        );
    }

    #[test]
    fn malformed_report_shape_fails_closed() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        let path = manifest.engines["fz"].files["lifecycle"].clone();
        let bytes = artifact_bytes(
            EvidenceClassV1::Lifecycle,
            &json!({}),
            &manifest.source_head,
            &manifest.hub_head,
        );
        std::fs::write(&path, &bytes).unwrap();
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::MalformedReport
        );
    }

    #[test]
    fn forged_report_digest_fails_closed() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        let path = manifest.engines["fz"].files["planner"].clone();
        let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        // Flip one digest byte: the report no longer binds its own fields.
        let digest = value["report"]["digest"].as_array().unwrap().clone();
        let mut flipped = digest;
        flipped[0] = json!(flipped[0].as_u64().unwrap() ^ 0xff);
        value["report"]["digest"] = Value::Array(flipped);
        rewrite_artifact(&path, value);
        let files = read_all(&manifest);
        let error = assemble_program_evidence(&manifest, loader_for(&files)).unwrap_err();
        match error.failure_code() {
            ProgramEvidenceFailureV1::ProgramAssembly(ProgramAssemblyError::MalformedReport(_)) => {
            }
            other => panic!("expected malformed report assembly failure, got {other:?}"),
        }
    }

    #[test]
    fn artifact_schema_version_mismatch_fails_closed() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        let path = manifest.engines["fz"].files["planner"].clone();
        let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["schema_version"] = json!(2);
        rewrite_artifact(&path, value);
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::SchemaVersionMismatch
        );
    }

    #[test]
    fn manifest_version_mismatch_fails_closed() {
        let base = TempDir::new().unwrap();
        let mut manifest = valid_manifest(base.path());
        manifest.version = 2;
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::ManifestVersionMismatch
        );
    }

    #[test]
    fn invalid_manifest_heads_fail_closed() {
        let base = TempDir::new().unwrap();
        let mut manifest = valid_manifest(base.path());
        manifest.hub_head = "not-a-head".into();
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::InvalidHead
        );
    }

    #[test]
    fn invalid_assembly_manifest_digest_fails_closed() {
        let base = TempDir::new().unwrap();
        let mut manifest = valid_manifest(base.path());
        manifest.assembly_manifest_digest = "zz".repeat(32);
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::InvalidAssemblyManifestDigest
        );
    }

    #[test]
    fn manifest_round_trips_canonically() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        let bytes = manifest.canonical_bytes().unwrap();
        let decoded = ProgramEvidenceManifestV1::from_canonical_bytes(&bytes).unwrap();
        assert_eq!(decoded, manifest);
        // Noncanonical key order is rejected.
        let text = String::from_utf8(bytes)
            .unwrap()
            .replace("\"version\":1", "\"version\": 1");
        assert!(ProgramEvidenceManifestV1::from_canonical_bytes(text.as_bytes()).is_err());
    }

    #[test]
    fn valid_head_rejects_non_lowercase_hex() {
        assert!(valid_head(&"a".repeat(40)));
        assert!(valid_head(&"b".repeat(64)));
        assert!(!valid_head(&"A".repeat(40)));
        assert!(!valid_head(&"a".repeat(39)));
        assert!(!valid_head(&"g".repeat(40)));
    }
}
