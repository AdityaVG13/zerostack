//! Trace-export pipeline, sealed benchmark manifests, and decision-boundary
//! annotations (ZS-OPS-003 / V6-R14).
//!
//! Every benchmark result must be reproducible from sealed manifests or
//! explicitly marked nonreproducible; decisions must carry boundary
//! annotations; cache events and invalidation reasons must be observable.
//! This module is the observability surface:
//!
//! - [`TraceExportJournalV1`] is a durable, append-only, chain-sealed trace
//!   export (JSONL under `<dir>/trace_records.jsonl` with a sealed head at
//!   `<dir>/trace_sealed_head`), reusing the exact fail-closed pattern of
//!   R6's `KernelEventJournalV1`: replay on open, torn-tail and tamper
//!   refusal, sealed-head verification. Every record chains to its parent
//!   record digest.
//! - [`TraceRecordV1`] carries a typed [`TraceEventKindV1`] (cache
//!   decisions, invalidations, verification outcomes, commits, executions,
//!   resource charges) and an optional [`DecisionBoundaryAnnotationV1`].
//!   Cache-decision records MUST carry the annotation (enforced by the
//!   typed constructor [`TraceRecordV1::cache_decision`]); the annotation
//!   names the boundary kind, the rationale, and the sealed decision
//!   digest -- this is the decision-boundary annotation artifact.
//! - [`DecisionBoundarySummaryV1`] summarizes every annotated boundary in
//!   an export into one sealed artifact.
//! - [`SealedBenchmarkManifestV1`] seals workload/engine/worker digests,
//!   canonical parameters, result and receipt digests, and a
//!   reproducibility statement (`Sealed` or explicit `NonReproducible`).
//!   [`export_benchmark_manifest_v1`] persists the manifest next to its
//!   seal; [`read_exported_benchmark_manifest_v1`] verifies the seal on
//!   read-back (tampering is loud).
//!
//! The pipeline entry point is [`export_trace_pipeline_v1`]: append a
//! bounded batch of records, seal, and return a sealed receipt.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use zero_abi::{DigestV1, canonical_json};

/// Schema version of trace-export artifacts.
pub const TRACE_EXPORT_SCHEMA_VERSION_V1: u16 = 1;
/// Domain tag bound into trace record digests.
pub const TRACE_EXPORT_DOMAIN_V1: &[u8] = b"zerostack.trace-export.v1\0";
/// Records file name inside an export directory.
pub const TRACE_EXPORT_RECORDS_FILE_V1: &str = "trace_records.jsonl";
/// Sealed-head file name inside an export directory.
pub const TRACE_EXPORT_SEALED_HEAD_FILE_V1: &str = "trace_sealed_head";
/// Benchmark manifest file name inside an export directory.
pub const TRACE_EXPORT_MANIFEST_FILE_V1: &str = "benchmark_manifest.json";
/// Maximum records exported in one pipeline call (bounded batches).
pub const TRACE_EXPORT_MAX_BATCH_RECORDS_V1: usize = 10_000;
/// Maximum canonical bytes of one trace record.
pub const TRACE_EXPORT_MAX_RECORD_BYTES_V1: usize = 64 * 1024;
/// ABI tag carried by trace-export artifacts.
pub const TRACE_EXPORT_ABI_VERSION_V1: &str = "v6-r14";

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, TraceExportErrorV1> {
    let json = serde_json::to_value(value)
        .map_err(|error| TraceExportErrorV1::InvalidRecord {
            seq: 0,
            detail: format!("artifact is not JSON-serializable: {error}"),
        })?;
    Ok(canonical_json(&json).into_bytes())
}

/// Typed trace event kinds. These are observability classes, finer-grained
/// than the authoritative `EventClassV1` (which remains the journal's
/// vocabulary); the export is the human- and tool-readable lens over the
/// same facts.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceEventKindV1 {
    CacheDecision,
    Invalidation,
    VerificationOutcome,
    Commit,
    Execution,
    ResourceCharge,
}

impl TraceEventKindV1 {
    pub const ALL: [TraceEventKindV1; 6] = [
        TraceEventKindV1::CacheDecision,
        TraceEventKindV1::Invalidation,
        TraceEventKindV1::VerificationOutcome,
        TraceEventKindV1::Commit,
        TraceEventKindV1::Execution,
        TraceEventKindV1::ResourceCharge,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TraceEventKindV1::CacheDecision => "cache_decision",
            TraceEventKindV1::Invalidation => "invalidation",
            TraceEventKindV1::VerificationOutcome => "verification_outcome",
            TraceEventKindV1::Commit => "commit",
            TraceEventKindV1::Execution => "execution",
            TraceEventKindV1::ResourceCharge => "resource_charge",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|kind| kind.as_str() == value)
    }
}

/// The decision-boundary kinds a trace record can annotate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionBoundaryKindV1 {
    CacheAdmission,
    CacheRefusal,
    Invalidation,
    VerificationAccepted,
    VerificationRejected,
    CommitAuthorized,
    CommitRefused,
    ExecutionAccepted,
    ExecutionRefused,
    ResourceCharge,
}

impl DecisionBoundaryKindV1 {
    pub const ALL: [DecisionBoundaryKindV1; 10] = [
        DecisionBoundaryKindV1::CacheAdmission,
        DecisionBoundaryKindV1::CacheRefusal,
        DecisionBoundaryKindV1::Invalidation,
        DecisionBoundaryKindV1::VerificationAccepted,
        DecisionBoundaryKindV1::VerificationRejected,
        DecisionBoundaryKindV1::CommitAuthorized,
        DecisionBoundaryKindV1::CommitRefused,
        DecisionBoundaryKindV1::ExecutionAccepted,
        DecisionBoundaryKindV1::ExecutionRefused,
        DecisionBoundaryKindV1::ResourceCharge,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            DecisionBoundaryKindV1::CacheAdmission => "cache_admission",
            DecisionBoundaryKindV1::CacheRefusal => "cache_refusal",
            DecisionBoundaryKindV1::Invalidation => "invalidation",
            DecisionBoundaryKindV1::VerificationAccepted => "verification_accepted",
            DecisionBoundaryKindV1::VerificationRejected => "verification_rejected",
            DecisionBoundaryKindV1::CommitAuthorized => "commit_authorized",
            DecisionBoundaryKindV1::CommitRefused => "commit_refused",
            DecisionBoundaryKindV1::ExecutionAccepted => "execution_accepted",
            DecisionBoundaryKindV1::ExecutionRefused => "execution_refused",
            DecisionBoundaryKindV1::ResourceCharge => "resource_charge",
        }
    }
}

/// One decision-boundary annotation: which boundary was crossed, why, and
/// the sealed digest of the decision artifact it refers to.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionBoundaryAnnotationV1 {
    pub boundary: DecisionBoundaryKindV1,
    pub rationale: String,
    pub decision_digest: DigestV1,
}

impl DecisionBoundaryAnnotationV1 {
    pub fn new(
        boundary: DecisionBoundaryKindV1,
        rationale: impl Into<String>,
        decision_digest: DigestV1,
    ) -> Result<Self, TraceExportErrorV1> {
        let annotation = Self {
            boundary,
            rationale: rationale.into(),
            decision_digest,
        };
        if annotation.rationale.is_empty() {
            return Err(TraceExportErrorV1::InvalidRecord {
                seq: 0,
                detail: "decision-boundary rationale must be nonempty".to_owned(),
            });
        }
        if annotation.decision_digest == DigestV1::ZERO {
            return Err(TraceExportErrorV1::InvalidRecord {
                seq: 0,
                detail: "decision digest must be nonzero".to_owned(),
            });
        }
        Ok(annotation)
    }
}

/// One trace record: typed kind, optional decision-boundary annotation,
/// payload root, issuing authority, and the parent-record digest that
/// chains the export.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TraceRecordV1 {
    pub schema_version: u16,
    pub seq: u64,
    pub kind: TraceEventKindV1,
    pub decision_boundary: Option<DecisionBoundaryAnnotationV1>,
    pub payload_root: String,
    pub authority: String,
    pub parent_record_digest: Option<DigestV1>,
    pub abi_version: String,
}

impl TraceRecordV1 {
    pub fn new(
        seq: u64,
        kind: TraceEventKindV1,
        decision_boundary: Option<DecisionBoundaryAnnotationV1>,
        payload_root: impl Into<String>,
        authority: impl Into<String>,
        parent_record_digest: Option<DigestV1>,
    ) -> Result<Self, TraceExportErrorV1> {
        let record = Self {
            schema_version: TRACE_EXPORT_SCHEMA_VERSION_V1,
            seq,
            kind,
            decision_boundary,
            payload_root: payload_root.into(),
            authority: authority.into(),
            parent_record_digest,
            abi_version: TRACE_EXPORT_ABI_VERSION_V1.to_owned(),
        };
        if record.payload_root.is_empty() || record.authority.is_empty() {
            return Err(TraceExportErrorV1::InvalidRecord {
                seq,
                detail: "trace payload root and authority must be nonempty".to_owned(),
            });
        }
        if record.kind == TraceEventKindV1::CacheDecision && record.decision_boundary.is_none() {
            return Err(TraceExportErrorV1::InvalidRecord {
                seq,
                detail: "cache-decision records require a decision-boundary annotation".to_owned(),
            });
        }
        if record.canonical_bytes()?.len() > TRACE_EXPORT_MAX_RECORD_BYTES_V1 {
            return Err(TraceExportErrorV1::InvalidRecord {
                seq,
                detail: "trace record exceeds the maximum canonical size".to_owned(),
            });
        }
        Ok(record)
    }

    /// Typed constructor for cache-decision records: the decision-boundary
    /// annotation is REQUIRED (compile-time enforced by the signature), so
    /// a cache event can never be exported without its boundary annotation.
    pub fn cache_decision(
        seq: u64,
        annotation: DecisionBoundaryAnnotationV1,
        payload_root: impl Into<String>,
        authority: impl Into<String>,
        parent_record_digest: Option<DigestV1>,
    ) -> Result<Self, TraceExportErrorV1> {
        Self::new(
            seq,
            TraceEventKindV1::CacheDecision,
            Some(annotation),
            payload_root,
            authority,
            parent_record_digest,
        )
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, TraceExportErrorV1> {
        canonical_bytes(self)
    }

    /// Content-derived record digest over the domain-tagged canonical bytes.
    pub fn record_digest(&self) -> Result<DigestV1, TraceExportErrorV1> {
        let mut tagged = Vec::with_capacity(TRACE_EXPORT_DOMAIN_V1.len() + 128);
        tagged.extend_from_slice(TRACE_EXPORT_DOMAIN_V1);
        tagged.extend_from_slice(&self.canonical_bytes()?);
        Ok(DigestV1::from_bytes(zero_abi::sha256(&tagged)))
    }
}

/// Loud, fail-closed errors for the trace export surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TraceExportErrorV1 {
    Io(String),
    InvalidRecord { seq: u64, detail: String },
    TornTail { seq: u64 },
    HeadMismatch { sealed: DigestV1, replayed: DigestV1 },
    InvalidManifest(String),
}

impl std::fmt::Display for TraceExportErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TraceExportErrorV1::Io(detail) => write!(formatter, "trace export I/O failure: {detail}"),
            TraceExportErrorV1::InvalidRecord { seq, detail } => {
                write!(formatter, "invalid trace record at seq {seq}: {detail}")
            }
            TraceExportErrorV1::TornTail { seq } => {
                write!(formatter, "torn trace tail at seq {seq}")
            }
            TraceExportErrorV1::HeadMismatch { sealed, replayed } => {
                write!(formatter, "trace head mismatch: sealed {sealed} != replayed {replayed}")
            }
            TraceExportErrorV1::InvalidManifest(detail) => {
                write!(formatter, "invalid benchmark manifest: {detail}")
            }
        }
    }
}

impl std::error::Error for TraceExportErrorV1 {}

/// The sealed-head marker persisted after a pipeline run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TraceSealedHeadV1 {
    pub schema_version: u16,
    pub head: DigestV1,
    pub abi_version: String,
}

impl TraceSealedHeadV1 {
    pub fn new(head: DigestV1) -> Self {
        Self {
            schema_version: TRACE_EXPORT_SCHEMA_VERSION_V1,
            head,
            abi_version: TRACE_EXPORT_ABI_VERSION_V1.to_owned(),
        }
    }
}

/// Sealed receipt of one trace-export pipeline run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TraceExportReceiptV1 {
    pub schema_version: u16,
    pub records: usize,
    pub head: DigestV1,
    pub sealed: bool,
    pub export_dir: String,
    pub abi_version: String,
}

impl TraceExportReceiptV1 {
    pub fn digest(&self) -> DigestV1 {
        let bytes = canonical_bytes(self).expect("trace export receipt is JSON-serializable");
        let mut tagged = Vec::with_capacity(TRACE_EXPORT_DOMAIN_V1.len() + 128);
        tagged.extend_from_slice(TRACE_EXPORT_DOMAIN_V1);
        tagged.extend_from_slice(&bytes);
        DigestV1::from_bytes(zero_abi::sha256(&tagged))
    }
}

/// A replayed trace export: the verified record chain plus its head.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceExportSnapshotV1 {
    pub records: Vec<TraceRecordV1>,
    pub head: DigestV1,
    pub sealed_head: Option<DigestV1>,
}

fn read_records(dir: &Path) -> Result<Vec<TraceRecordV1>, TraceExportErrorV1> {
    let path = dir.join(TRACE_EXPORT_RECORDS_FILE_V1);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&path)
        .map_err(|error| TraceExportErrorV1::Io(format!("read {}: {error}", path.display())))?;
    // A nonempty records file must end in a newline: the final line is
    // otherwise the partial write of an interrupted append (torn tail).
    if !content.is_empty() && !content.ends_with('\n') {
        return Err(TraceExportErrorV1::TornTail {
            seq: content.lines().count() as u64,
        });
    }
    let mut records = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if line.is_empty() {
            return Err(TraceExportErrorV1::TornTail { seq: index as u64 });
        }
        let record: TraceRecordV1 = serde_json::from_str(line).map_err(|error| {
            TraceExportErrorV1::InvalidRecord {
                seq: index as u64,
                detail: format!("cannot parse persisted record: {error}"),
            }
        })?;
        records.push(record);
    }
    Ok(records)
}

fn replay_and_verify(dir: &Path) -> Result<TraceExportSnapshotV1, TraceExportErrorV1> {
    let records = read_records(dir)?;
    let mut parent: Option<DigestV1> = None;
    for (index, record) in records.iter().enumerate() {
        if record.seq != index as u64 {
            return Err(TraceExportErrorV1::InvalidRecord {
                seq: index as u64,
                detail: format!("seq must equal the record index, got {}", record.seq),
            });
        }
        if record.parent_record_digest != parent {
            return Err(TraceExportErrorV1::InvalidRecord {
                seq: index as u64,
                detail: "record does not chain to its parent digest".to_owned(),
            });
        }
        parent = Some(record.record_digest()?);
    }
    let head = parent.unwrap_or_else(|| DigestV1::from_bytes([0; 32]));
    let sealed_path = dir.join(TRACE_EXPORT_SEALED_HEAD_FILE_V1);
    let sealed_head = if sealed_path.exists() {
        let content = fs::read_to_string(&sealed_path)
            .map_err(|error| TraceExportErrorV1::Io(format!("read sealed head: {error}")))?;
        let sealed: TraceSealedHeadV1 = serde_json::from_str(&content).map_err(|error| {
            TraceExportErrorV1::InvalidManifest(format!("sealed head not canonical: {error}"))
        })?;
        if sealed.schema_version != TRACE_EXPORT_SCHEMA_VERSION_V1 {
            return Err(TraceExportErrorV1::InvalidManifest(
                "sealed head schema version is not supported".to_owned(),
            ));
        }
        if sealed.head != head {
            return Err(TraceExportErrorV1::HeadMismatch {
                sealed: sealed.head,
                replayed: head,
            });
        }
        Some(sealed.head)
    } else {
        None
    };
    Ok(TraceExportSnapshotV1 {
        records,
        head,
        sealed_head,
    })
}

/// Open (or create) a trace export and verify its chain. Fails closed on
/// torn tails, malformed, reordered, or non-chaining records, and on
/// sealed-head mismatches.
pub fn open_trace_export_v1(dir: impl Into<PathBuf>) -> Result<TraceExportSnapshotV1, TraceExportErrorV1> {
    let dir = dir.into();
    fs::create_dir_all(&dir)
        .map_err(|error| TraceExportErrorV1::Io(format!("create {}: {error}", dir.display())))?;
    replay_and_verify(&dir)
}

/// Append one trace record, chaining it to the current head. The record is
/// persisted durably first.
pub fn append_trace_record_v1(
    dir: impl Into<PathBuf>,
    record: &TraceRecordV1,
) -> Result<TraceExportReceiptV1, TraceExportErrorV1> {
    let dir = dir.into();
    fs::create_dir_all(&dir)
        .map_err(|error| TraceExportErrorV1::Io(format!("create {}: {error}", dir.display())))?;
    let current = replay_and_verify(&dir)?;
    let expected_seq = current.records.len() as u64;
    if record.seq != expected_seq {
        return Err(TraceExportErrorV1::InvalidRecord {
            seq: record.seq,
            detail: format!("append must continue the chain at seq {expected_seq}"),
        });
    }
    let parent = if expected_seq == 0 {
        None
    } else {
        Some(current.records.last().expect("records nonempty").record_digest()?)
    };
    if record.parent_record_digest != parent {
        return Err(TraceExportErrorV1::InvalidRecord {
            seq: record.seq,
            detail: "record parent digest does not chain to the export head".to_owned(),
        });
    }
    let path = dir.join(TRACE_EXPORT_RECORDS_FILE_V1);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| TraceExportErrorV1::Io(format!("open {}: {error}", path.display())))?;
    let mut bytes = record.canonical_bytes()?;
    bytes.push(b'\n');
    file.write_all(&bytes)
        .map_err(|error| TraceExportErrorV1::Io(format!("append {}: {error}", path.display())))?;
    file.sync_all()
        .map_err(|error| TraceExportErrorV1::Io(format!("sync {}: {error}", path.display())))?;
    let snapshot = replay_and_verify(&dir)?;
    Ok(TraceExportReceiptV1 {
        schema_version: TRACE_EXPORT_SCHEMA_VERSION_V1,
        records: snapshot.records.len(),
        head: snapshot.head,
        sealed: snapshot.sealed_head.is_some(),
        export_dir: dir.display().to_string(),
        abi_version: TRACE_EXPORT_ABI_VERSION_V1.to_owned(),
    })
}

/// Seal the current export head. A later open verifies the replayed chain
/// against this head, detecting torn tails and tampering.
pub fn seal_trace_export_v1(dir: impl Into<PathBuf>) -> Result<DigestV1, TraceExportErrorV1> {
    let dir = dir.into();
    let snapshot = replay_and_verify(&dir)?;
    let sealed = TraceSealedHeadV1::new(snapshot.head);
    let content = serde_json::to_vec(&sealed).map_err(|error| {
        TraceExportErrorV1::InvalidManifest(format!("sealed head not serializable: {error}"))
    })?;
    let path = dir.join(TRACE_EXPORT_SEALED_HEAD_FILE_V1);
    fs::write(&path, content)
        .map_err(|error| TraceExportErrorV1::Io(format!("write {}: {error}", path.display())))?;
    Ok(snapshot.head)
}

/// Full export pipeline for one batch: open (creating as needed), append
/// the records in order, seal, and return the sealed receipt. Batches are
/// bounded ([`TRACE_EXPORT_MAX_BATCH_RECORDS_V1`]) -- a larger batch is a
/// loud refusal, never a silent truncation.
pub fn export_trace_pipeline_v1(
    dir: impl Into<PathBuf>,
    records: &[TraceRecordV1],
) -> Result<TraceExportReceiptV1, TraceExportErrorV1> {
    let dir = dir.into();
    if records.len() > TRACE_EXPORT_MAX_BATCH_RECORDS_V1 {
        return Err(TraceExportErrorV1::InvalidManifest(format!(
            "trace batch of {} records exceeds the bound {}",
            records.len(),
            TRACE_EXPORT_MAX_BATCH_RECORDS_V1
        )));
    }
    let current = open_trace_export_v1(&dir)?;
    let start = current.records.len() as u64;
    for (offset, record) in records.iter().enumerate() {
        if record.seq != start + offset as u64 {
            return Err(TraceExportErrorV1::InvalidRecord {
                seq: record.seq,
                detail: format!("pipeline batch must continue the chain at seq {}", start + offset as u64),
            });
        }
        append_trace_record_v1(&dir, record)?;
    }
    let head = seal_trace_export_v1(&dir)?;
    let snapshot = replay_and_verify(&dir)?;
    Ok(TraceExportReceiptV1 {
        schema_version: TRACE_EXPORT_SCHEMA_VERSION_V1,
        records: snapshot.records.len(),
        head,
        sealed: true,
        export_dir: dir.display().to_string(),
        abi_version: TRACE_EXPORT_ABI_VERSION_V1.to_owned(),
    })
}

/// Read back an export and verify its chain and seal (alias for
/// [`open_trace_export_v1`]).
pub fn read_trace_export_v1(
    dir: impl Into<PathBuf>,
) -> Result<TraceExportSnapshotV1, TraceExportErrorV1> {
    open_trace_export_v1(dir)
}

// ---------------------------------------------------------------------------
// Decision-boundary annotation artifact.
// ---------------------------------------------------------------------------

/// One annotated boundary line in a summary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionBoundaryLineV1 {
    pub seq: u64,
    pub kind: TraceEventKindV1,
    pub boundary: DecisionBoundaryKindV1,
    pub rationale: String,
    pub decision_digest: DigestV1,
    pub record_digest: DigestV1,
}

/// Sealed summary of every decision boundary annotated in an export: the
/// decision-boundary annotation artifact.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionBoundarySummaryV1 {
    pub schema_version: u16,
    pub export_dir: String,
    pub boundaries: Vec<DecisionBoundaryLineV1>,
    pub annotated_records: usize,
    pub abi_version: String,
}

impl DecisionBoundarySummaryV1 {
    pub fn digest(&self) -> DigestV1 {
        let bytes = canonical_bytes(self).expect("decision-boundary summary is JSON-serializable");
        let mut tagged = Vec::with_capacity(TRACE_EXPORT_DOMAIN_V1.len() + 128);
        tagged.extend_from_slice(TRACE_EXPORT_DOMAIN_V1);
        tagged.extend_from_slice(&bytes);
        DigestV1::from_bytes(zero_abi::sha256(&tagged))
    }
}

/// Build the decision-boundary annotation artifact for an export: every
/// cache-decision record must carry an annotation (it does -- the typed
/// constructor enforces it), and every annotated record is listed with its
/// sealed decision digest. Unannotated records are listed as
/// [`DecisionBoundaryKindV1`]-free lines only when they are not decisions;
/// a decision record without an annotation is a loud error.
pub fn summarize_decision_boundaries_v1(
    dir: impl Into<PathBuf>,
) -> Result<DecisionBoundarySummaryV1, TraceExportErrorV1> {
    let dir = dir.into();
    let snapshot = open_trace_export_v1(&dir)?;
    let mut boundaries = Vec::new();
    for record in &snapshot.records {
        if record.kind == TraceEventKindV1::CacheDecision
            && record.decision_boundary.is_none()
        {
            return Err(TraceExportErrorV1::InvalidRecord {
                seq: record.seq,
                detail: "cache-decision record lacks a decision-boundary annotation".to_owned(),
            });
        }
        if let Some(annotation) = &record.decision_boundary {
            boundaries.push(DecisionBoundaryLineV1 {
                seq: record.seq,
                kind: record.kind,
                boundary: annotation.boundary,
                rationale: annotation.rationale.clone(),
                decision_digest: annotation.decision_digest,
                record_digest: record.record_digest()?,
            });
        }
    }
    let annotated_records = boundaries.len();
    Ok(DecisionBoundarySummaryV1 {
        schema_version: TRACE_EXPORT_SCHEMA_VERSION_V1,
        export_dir: dir.display().to_string(),
        boundaries,
        annotated_records,
        abi_version: TRACE_EXPORT_ABI_VERSION_V1.to_owned(),
    })
}

// ---------------------------------------------------------------------------
// Sealed benchmark manifests.
// ---------------------------------------------------------------------------

/// Reproducibility statement of a benchmark: sealed (reproducible from the
/// manifest) or explicitly nonreproducible with a reason -- a benchmark is
/// never implicitly reproducible.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkReproducibilityV1 {
    Sealed,
    NonReproducible { reason: String },
}

/// Sealed benchmark manifest: every input digest, canonical parameters,
/// result and receipt digests, and a reproducibility statement. The
/// manifest digest (seal) is content-derived, so a result is reproducible
/// from the manifest or explicitly marked otherwise.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SealedBenchmarkManifestV1 {
    pub schema_version: u16,
    pub benchmark_id: String,
    pub workload_digest: DigestV1,
    pub engine_digest: DigestV1,
    pub worker_digests: Vec<String>,
    pub parameters: serde_json::Value,
    pub result_digest: DigestV1,
    pub receipt_digests: Vec<String>,
    pub reproducibility: BenchmarkReproducibilityV1,
    pub abi_version: String,
}

impl SealedBenchmarkManifestV1 {
    pub fn new(
        benchmark_id: impl Into<String>,
        workload_digest: DigestV1,
        engine_digest: DigestV1,
        worker_digests: Vec<String>,
        parameters: serde_json::Value,
        result_digest: DigestV1,
        receipt_digests: Vec<String>,
        reproducibility: BenchmarkReproducibilityV1,
    ) -> Result<Self, TraceExportErrorV1> {
        let manifest = Self {
            schema_version: TRACE_EXPORT_SCHEMA_VERSION_V1,
            benchmark_id: benchmark_id.into(),
            workload_digest,
            engine_digest,
            worker_digests,
            parameters,
            result_digest,
            receipt_digests,
            reproducibility,
            abi_version: TRACE_EXPORT_ABI_VERSION_V1.to_owned(),
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), TraceExportErrorV1> {
        if self.schema_version != TRACE_EXPORT_SCHEMA_VERSION_V1 {
            return Err(TraceExportErrorV1::InvalidManifest(
                "manifest schema version is not supported".to_owned(),
            ));
        }
        if self.benchmark_id.is_empty()
            || self.workload_digest == DigestV1::ZERO
            || self.engine_digest == DigestV1::ZERO
            || self.result_digest == DigestV1::ZERO
        {
            return Err(TraceExportErrorV1::InvalidManifest(
                "benchmark id and workload/engine/result digests must be nonzero".to_owned(),
            ));
        }
        if let BenchmarkReproducibilityV1::NonReproducible { reason } = &self.reproducibility {
            if reason.is_empty() {
                return Err(TraceExportErrorV1::InvalidManifest(
                    "nonreproducible benchmark must carry a reason".to_owned(),
                ));
            }
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, TraceExportErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    /// The seal: content-derived digest over the domain-tagged canonical
    /// manifest bytes. Same inputs, same seal; tampering changes the seal.
    pub fn digest(&self) -> Result<DigestV1, TraceExportErrorV1> {
        let mut tagged = Vec::with_capacity(TRACE_EXPORT_DOMAIN_V1.len() + 128);
        tagged.extend_from_slice(TRACE_EXPORT_DOMAIN_V1);
        tagged.extend_from_slice(&self.canonical_bytes()?);
        Ok(DigestV1::from_bytes(zero_abi::sha256(&tagged)))
    }
}

/// The persisted form of a sealed manifest: manifest next to its seal.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SealedManifestFileV1 {
    pub schema_version: u16,
    pub manifest: SealedBenchmarkManifestV1,
    pub seal: DigestV1,
    pub abi_version: String,
}

impl SealedManifestFileV1 {
    pub fn seal(manifest: SealedBenchmarkManifestV1) -> Result<Self, TraceExportErrorV1> {
        let seal = manifest.digest()?;
        Ok(Self {
            schema_version: TRACE_EXPORT_SCHEMA_VERSION_V1,
            manifest,
            seal,
            abi_version: TRACE_EXPORT_ABI_VERSION_V1.to_owned(),
        })
    }

    /// Verify the seal against the manifest bytes; tampering is loud.
    pub fn verify_seal(&self) -> Result<(), TraceExportErrorV1> {
        if self.schema_version != TRACE_EXPORT_SCHEMA_VERSION_V1 {
            return Err(TraceExportErrorV1::InvalidManifest(
                "sealed manifest file schema version is not supported".to_owned(),
            ));
        }
        let recomputed = self.manifest.digest()?;
        if recomputed != self.seal {
            return Err(TraceExportErrorV1::InvalidManifest(format!(
                "seal mismatch: persisted seal {} != recomputed {recomputed}",
                self.seal
            )));
        }
        Ok(())
    }
}

/// Export a sealed benchmark manifest into the export directory. The seal
/// is persisted next to the manifest; read-back re-verifies it.
pub fn export_benchmark_manifest_v1(
    dir: impl Into<PathBuf>,
    manifest: SealedBenchmarkManifestV1,
) -> Result<DigestV1, TraceExportErrorV1> {
    let dir = dir.into();
    fs::create_dir_all(&dir)
        .map_err(|error| TraceExportErrorV1::Io(format!("create {}: {error}", dir.display())))?;
    let file = SealedManifestFileV1::seal(manifest)?;
    let content = serde_json::to_vec(&file).map_err(|error| {
        TraceExportErrorV1::InvalidManifest(format!("manifest not serializable: {error}"))
    })?;
    let path = dir.join(TRACE_EXPORT_MANIFEST_FILE_V1);
    fs::write(&path, content)
        .map_err(|error| TraceExportErrorV1::Io(format!("write {}: {error}", path.display())))?;
    Ok(file.seal)
}

/// Read back and verify a sealed benchmark manifest.
pub fn read_exported_benchmark_manifest_v1(
    dir: impl Into<PathBuf>,
) -> Result<SealedBenchmarkManifestV1, TraceExportErrorV1> {
    let dir = dir.into();
    let path = dir.join(TRACE_EXPORT_MANIFEST_FILE_V1);
    if !path.exists() {
        return Err(TraceExportErrorV1::InvalidManifest(format!(
            "no benchmark manifest at {}",
            path.display()
        )));
    }
    let content = fs::read_to_string(&path)
        .map_err(|error| TraceExportErrorV1::Io(format!("read {}: {error}", path.display())))?;
    let file: SealedManifestFileV1 = serde_json::from_str(&content).map_err(|error| {
        TraceExportErrorV1::InvalidManifest(format!("cannot parse manifest: {error}"))
    })?;
    file.verify_seal()?;
    Ok(file.manifest)
}

/// The frozen contract manifest for the trace-export surface (ZS-OPS-003).
pub fn trace_export_contract_v1() -> serde_json::Value {
    serde_json::json!({
        "schema_version": TRACE_EXPORT_SCHEMA_VERSION_V1,
        "pipeline": {
            "records_file": TRACE_EXPORT_RECORDS_FILE_V1,
            "sealed_head_file": TRACE_EXPORT_SEALED_HEAD_FILE_V1,
            "manifest_file": TRACE_EXPORT_MANIFEST_FILE_V1,
            "fail_closed": "torn tails, tampered or reordered records, head mismatches",
            "max_batch_records": TRACE_EXPORT_MAX_BATCH_RECORDS_V1,
        },
        "decision_boundary": {
            "cache_decision_records": "annotation required (typed constructor)",
            "artifact": "DecisionBoundarySummaryV1 with sealed decision digests",
        },
        "benchmark_manifests": {
            "reproducibility": "sealed or explicitly nonreproducible with reason",
            "seal": "content-derived over canonical manifest bytes",
        },
        "abi_version": TRACE_EXPORT_ABI_VERSION_V1,
    })
}
