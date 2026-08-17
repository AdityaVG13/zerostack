//! Trace-export pipeline, sealed benchmark manifests, and decision-boundary
//! annotations (ZS-OPS-003 / V6-R14).
//!
//! Every benchmark result must be reproducible from sealed manifests or
//! explicitly marked nonreproducible; decisions must carry boundary
//! annotations; cache events and invalidation reasons must be observable.
//! This module is the observability surface:
//!
//! - [`TraceExportJournal`] is a durable, append-only, chain-sealed trace
//!   export (JSONL under `<dir>/trace_records.jsonl` with a sealed head at
//!   `<dir>/trace_sealed_head`), reusing the exact fail-closed pattern of
//!   R6's `KernelEventJournal`: replay on open, torn-tail and tamper
//!   refusal, sealed-head verification. Every record chains to its parent
//!   record digest.
//! - [`TraceRecord`] carries a typed [`TraceEventKind`] (cache
//!   decisions, invalidations, verification outcomes, commits, executions,
//!   resource charges) and an optional [`DecisionBoundaryAnnotation`].
//!   Cache-decision records MUST carry the annotation (enforced by the
//!   typed constructor [`TraceRecord::cache_decision`]); the annotation
//!   names the boundary kind, the rationale, and the sealed decision
//!   digest -- this is the decision-boundary annotation artifact.
//! - [`DecisionBoundarySummary`] summarizes every annotated boundary in
//!   an export into one sealed artifact.
//! - [`SealedBenchmarkManifest`] seals workload/engine/worker digests,
//!   canonical parameters, result and receipt digests, and a
//!   reproducibility statement (`Sealed` or explicit `NonReproducible`).
//!   [`export_benchmark_manifest`] persists the manifest next to its
//!   seal; [`read_exported_benchmark_manifest`] verifies the seal on
//!   read-back (tampering is loud).
//!
//! The pipeline entry point is [`export_trace_pipeline`]: append a
//! bounded batch of records, seal, and return a sealed receipt.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use zero_abi::{Sha256Digest, canonical_json};

/// Schema version of trace-export artifacts.
pub const TRACE_EXPORT_SCHEMA_VERSION: u16 = 1;
/// Domain tag bound into trace record digests.
pub const TRACE_EXPORT_DOMAIN: &[u8] = b"zerostack.trace-export.v1\0";
/// Records file name inside an export directory.
pub const TRACE_EXPORT_RECORDS_FILE: &str = "trace_records.jsonl";
/// Sealed-head file name inside an export directory.
pub const TRACE_EXPORT_SEALED_HEAD_FILE: &str = "trace_sealed_head";
/// Benchmark manifest file name inside an export directory.
pub const TRACE_EXPORT_MANIFEST_FILE: &str = "benchmark_manifest.json";
/// Maximum records exported in one pipeline call (bounded batches).
pub const TRACE_EXPORT_MAX_BATCH_RECORDS: usize = 10_000;
/// Maximum canonical bytes of one trace record.
pub const TRACE_EXPORT_MAX_RECORD_BYTES: usize = 64 * 1024;
/// ABI tag carried by trace-export artifacts.
pub const TRACE_EXPORT_ABI_VERSION: &str = "v6-r14";

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, TraceExportError> {
    let json = serde_json::to_value(value)
        .map_err(|error| TraceExportError::InvalidRecord {
            seq: 0,
            detail: format!("artifact is not JSON-serializable: {error}"),
        })?;
    Ok(canonical_json(&json).into_bytes())
}

/// Typed trace event kinds. These are observability classes, finer-grained
/// than the authoritative `EventClass` (which remains the journal's
/// vocabulary); the export is the human- and tool-readable lens over the
/// same facts.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceEventKind {
    CacheDecision,
    Invalidation,
    VerificationOutcome,
    Commit,
    Execution,
    ResourceCharge,
}

impl TraceEventKind {
    pub const ALL: [TraceEventKind; 6] = [
        TraceEventKind::CacheDecision,
        TraceEventKind::Invalidation,
        TraceEventKind::VerificationOutcome,
        TraceEventKind::Commit,
        TraceEventKind::Execution,
        TraceEventKind::ResourceCharge,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TraceEventKind::CacheDecision => "cache_decision",
            TraceEventKind::Invalidation => "invalidation",
            TraceEventKind::VerificationOutcome => "verification_outcome",
            TraceEventKind::Commit => "commit",
            TraceEventKind::Execution => "execution",
            TraceEventKind::ResourceCharge => "resource_charge",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|kind| kind.as_str() == value)
    }
}

/// The decision-boundary kinds a trace record can annotate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionBoundaryKind {
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

impl DecisionBoundaryKind {
    pub const ALL: [DecisionBoundaryKind; 10] = [
        DecisionBoundaryKind::CacheAdmission,
        DecisionBoundaryKind::CacheRefusal,
        DecisionBoundaryKind::Invalidation,
        DecisionBoundaryKind::VerificationAccepted,
        DecisionBoundaryKind::VerificationRejected,
        DecisionBoundaryKind::CommitAuthorized,
        DecisionBoundaryKind::CommitRefused,
        DecisionBoundaryKind::ExecutionAccepted,
        DecisionBoundaryKind::ExecutionRefused,
        DecisionBoundaryKind::ResourceCharge,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            DecisionBoundaryKind::CacheAdmission => "cache_admission",
            DecisionBoundaryKind::CacheRefusal => "cache_refusal",
            DecisionBoundaryKind::Invalidation => "invalidation",
            DecisionBoundaryKind::VerificationAccepted => "verification_accepted",
            DecisionBoundaryKind::VerificationRejected => "verification_rejected",
            DecisionBoundaryKind::CommitAuthorized => "commit_authorized",
            DecisionBoundaryKind::CommitRefused => "commit_refused",
            DecisionBoundaryKind::ExecutionAccepted => "execution_accepted",
            DecisionBoundaryKind::ExecutionRefused => "execution_refused",
            DecisionBoundaryKind::ResourceCharge => "resource_charge",
        }
    }
}

/// One decision-boundary annotation: which boundary was crossed, why, and
/// the sealed digest of the decision artifact it refers to.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionBoundaryAnnotation {
    pub boundary: DecisionBoundaryKind,
    pub rationale: String,
    pub decision_digest: Sha256Digest,
}

impl DecisionBoundaryAnnotation {
    pub fn new(
        boundary: DecisionBoundaryKind,
        rationale: impl Into<String>,
        decision_digest: Sha256Digest,
    ) -> Result<Self, TraceExportError> {
        let annotation = Self {
            boundary,
            rationale: rationale.into(),
            decision_digest,
        };
        if annotation.rationale.is_empty() {
            return Err(TraceExportError::InvalidRecord {
                seq: 0,
                detail: "decision-boundary rationale must be nonempty".to_owned(),
            });
        }
        if annotation.decision_digest == Sha256Digest::ZERO {
            return Err(TraceExportError::InvalidRecord {
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
pub struct TraceRecord {
    pub schema_version: u16,
    pub seq: u64,
    pub kind: TraceEventKind,
    pub decision_boundary: Option<DecisionBoundaryAnnotation>,
    pub payload_root: String,
    pub authority: String,
    pub parent_record_digest: Option<Sha256Digest>,
    pub abi_version: String,
}

impl TraceRecord {
    pub fn new(
        seq: u64,
        kind: TraceEventKind,
        decision_boundary: Option<DecisionBoundaryAnnotation>,
        payload_root: impl Into<String>,
        authority: impl Into<String>,
        parent_record_digest: Option<Sha256Digest>,
    ) -> Result<Self, TraceExportError> {
        let record = Self {
            schema_version: TRACE_EXPORT_SCHEMA_VERSION,
            seq,
            kind,
            decision_boundary,
            payload_root: payload_root.into(),
            authority: authority.into(),
            parent_record_digest,
            abi_version: TRACE_EXPORT_ABI_VERSION.to_owned(),
        };
        if record.payload_root.is_empty() || record.authority.is_empty() {
            return Err(TraceExportError::InvalidRecord {
                seq,
                detail: "trace payload root and authority must be nonempty".to_owned(),
            });
        }
        if record.kind == TraceEventKind::CacheDecision && record.decision_boundary.is_none() {
            return Err(TraceExportError::InvalidRecord {
                seq,
                detail: "cache-decision records require a decision-boundary annotation".to_owned(),
            });
        }
        if record.canonical_bytes()?.len() > TRACE_EXPORT_MAX_RECORD_BYTES {
            return Err(TraceExportError::InvalidRecord {
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
        annotation: DecisionBoundaryAnnotation,
        payload_root: impl Into<String>,
        authority: impl Into<String>,
        parent_record_digest: Option<Sha256Digest>,
    ) -> Result<Self, TraceExportError> {
        Self::new(
            seq,
            TraceEventKind::CacheDecision,
            Some(annotation),
            payload_root,
            authority,
            parent_record_digest,
        )
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, TraceExportError> {
        canonical_bytes(self)
    }

    /// Content-derived record digest over the domain-tagged canonical bytes.
    pub fn record_digest(&self) -> Result<Sha256Digest, TraceExportError> {
        let mut tagged = Vec::with_capacity(TRACE_EXPORT_DOMAIN.len() + 128);
        tagged.extend_from_slice(TRACE_EXPORT_DOMAIN);
        tagged.extend_from_slice(&self.canonical_bytes()?);
        Ok(Sha256Digest::from_bytes(zero_abi::sha256(&tagged)))
    }
}

/// Loud, fail-closed errors for the trace export surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TraceExportError {
    Io(String),
    InvalidRecord { seq: u64, detail: String },
    TornTail { seq: u64 },
    HeadMismatch { sealed: Sha256Digest, replayed: Sha256Digest },
    InvalidManifest(String),
}

impl std::fmt::Display for TraceExportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TraceExportError::Io(detail) => write!(formatter, "trace export I/O failure: {detail}"),
            TraceExportError::InvalidRecord { seq, detail } => {
                write!(formatter, "invalid trace record at seq {seq}: {detail}")
            }
            TraceExportError::TornTail { seq } => {
                write!(formatter, "torn trace tail at seq {seq}")
            }
            TraceExportError::HeadMismatch { sealed, replayed } => {
                write!(formatter, "trace head mismatch: sealed {sealed} != replayed {replayed}")
            }
            TraceExportError::InvalidManifest(detail) => {
                write!(formatter, "invalid benchmark manifest: {detail}")
            }
        }
    }
}

impl std::error::Error for TraceExportError {}

/// The sealed-head marker persisted after a pipeline run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TraceSealedHead {
    pub schema_version: u16,
    pub head: Sha256Digest,
    pub abi_version: String,
}

impl TraceSealedHead {
    pub fn new(head: Sha256Digest) -> Self {
        Self {
            schema_version: TRACE_EXPORT_SCHEMA_VERSION,
            head,
            abi_version: TRACE_EXPORT_ABI_VERSION.to_owned(),
        }
    }
}

/// Sealed receipt of one trace-export pipeline run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TraceExportReceipt {
    pub schema_version: u16,
    pub records: usize,
    pub head: Sha256Digest,
    pub sealed: bool,
    pub export_dir: String,
    pub abi_version: String,
}

impl TraceExportReceipt {
    pub fn digest(&self) -> Sha256Digest {
        let bytes = canonical_bytes(self).expect("trace export receipt is JSON-serializable");
        let mut tagged = Vec::with_capacity(TRACE_EXPORT_DOMAIN.len() + 128);
        tagged.extend_from_slice(TRACE_EXPORT_DOMAIN);
        tagged.extend_from_slice(&bytes);
        Sha256Digest::from_bytes(zero_abi::sha256(&tagged))
    }
}

/// A replayed trace export: the verified record chain plus its head.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceExportSnapshot {
    pub records: Vec<TraceRecord>,
    pub head: Sha256Digest,
    pub sealed_head: Option<Sha256Digest>,
}

fn read_records(dir: &Path) -> Result<Vec<TraceRecord>, TraceExportError> {
    let path = dir.join(TRACE_EXPORT_RECORDS_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&path)
        .map_err(|error| TraceExportError::Io(format!("read {}: {error}", path.display())))?;
    // A nonempty records file must end in a newline: the final line is
    // otherwise the partial write of an interrupted append (torn tail).
    if !content.is_empty() && !content.ends_with('\n') {
        return Err(TraceExportError::TornTail {
            seq: content.lines().count() as u64,
        });
    }
    let mut records = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if line.is_empty() {
            return Err(TraceExportError::TornTail { seq: index as u64 });
        }
        let record: TraceRecord = serde_json::from_str(line).map_err(|error| {
            TraceExportError::InvalidRecord {
                seq: index as u64,
                detail: format!("cannot parse persisted record: {error}"),
            }
        })?;
        records.push(record);
    }
    Ok(records)
}

fn replay_and_verify(dir: &Path) -> Result<TraceExportSnapshot, TraceExportError> {
    let records = read_records(dir)?;
    let mut parent: Option<Sha256Digest> = None;
    for (index, record) in records.iter().enumerate() {
        if record.seq != index as u64 {
            return Err(TraceExportError::InvalidRecord {
                seq: index as u64,
                detail: format!("seq must equal the record index, got {}", record.seq),
            });
        }
        if record.parent_record_digest != parent {
            return Err(TraceExportError::InvalidRecord {
                seq: index as u64,
                detail: "record does not chain to its parent digest".to_owned(),
            });
        }
        parent = Some(record.record_digest()?);
    }
    let head = parent.unwrap_or_else(|| Sha256Digest::from_bytes([0; 32]));
    let sealed_path = dir.join(TRACE_EXPORT_SEALED_HEAD_FILE);
    let sealed_head = if sealed_path.exists() {
        let content = fs::read_to_string(&sealed_path)
            .map_err(|error| TraceExportError::Io(format!("read sealed head: {error}")))?;
        let sealed: TraceSealedHead = serde_json::from_str(&content).map_err(|error| {
            TraceExportError::InvalidManifest(format!("sealed head not canonical: {error}"))
        })?;
        if sealed.schema_version != TRACE_EXPORT_SCHEMA_VERSION {
            return Err(TraceExportError::InvalidManifest(
                "sealed head schema version is not supported".to_owned(),
            ));
        }
        if sealed.head != head {
            return Err(TraceExportError::HeadMismatch {
                sealed: sealed.head,
                replayed: head,
            });
        }
        Some(sealed.head)
    } else {
        None
    };
    Ok(TraceExportSnapshot {
        records,
        head,
        sealed_head,
    })
}

/// Open (or create) a trace export and verify its chain. Fails closed on
/// torn tails, malformed, reordered, or non-chaining records, and on
/// sealed-head mismatches.
pub fn open_trace_export(dir: impl Into<PathBuf>) -> Result<TraceExportSnapshot, TraceExportError> {
    let dir = dir.into();
    fs::create_dir_all(&dir)
        .map_err(|error| TraceExportError::Io(format!("create {}: {error}", dir.display())))?;
    replay_and_verify(&dir)
}

/// Append one trace record, chaining it to the current head. The record is
/// persisted durably first.
pub fn append_trace_record(
    dir: impl Into<PathBuf>,
    record: &TraceRecord,
) -> Result<TraceExportReceipt, TraceExportError> {
    let dir = dir.into();
    fs::create_dir_all(&dir)
        .map_err(|error| TraceExportError::Io(format!("create {}: {error}", dir.display())))?;
    let current = replay_and_verify(&dir)?;
    let expected_seq = current.records.len() as u64;
    if record.seq != expected_seq {
        return Err(TraceExportError::InvalidRecord {
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
        return Err(TraceExportError::InvalidRecord {
            seq: record.seq,
            detail: "record parent digest does not chain to the export head".to_owned(),
        });
    }
    let path = dir.join(TRACE_EXPORT_RECORDS_FILE);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| TraceExportError::Io(format!("open {}: {error}", path.display())))?;
    let mut bytes = record.canonical_bytes()?;
    bytes.push(b'\n');
    file.write_all(&bytes)
        .map_err(|error| TraceExportError::Io(format!("append {}: {error}", path.display())))?;
    file.sync_all()
        .map_err(|error| TraceExportError::Io(format!("sync {}: {error}", path.display())))?;
    let snapshot = replay_and_verify(&dir)?;
    Ok(TraceExportReceipt {
        schema_version: TRACE_EXPORT_SCHEMA_VERSION,
        records: snapshot.records.len(),
        head: snapshot.head,
        sealed: snapshot.sealed_head.is_some(),
        export_dir: dir.display().to_string(),
        abi_version: TRACE_EXPORT_ABI_VERSION.to_owned(),
    })
}

/// Seal the current export head. A later open verifies the replayed chain
/// against this head, detecting torn tails and tampering.
pub fn seal_trace_export(dir: impl Into<PathBuf>) -> Result<Sha256Digest, TraceExportError> {
    let dir = dir.into();
    let snapshot = replay_and_verify(&dir)?;
    let sealed = TraceSealedHead::new(snapshot.head);
    let content = serde_json::to_vec(&sealed).map_err(|error| {
        TraceExportError::InvalidManifest(format!("sealed head not serializable: {error}"))
    })?;
    let path = dir.join(TRACE_EXPORT_SEALED_HEAD_FILE);
    fs::write(&path, content)
        .map_err(|error| TraceExportError::Io(format!("write {}: {error}", path.display())))?;
    Ok(snapshot.head)
}

/// Full export pipeline for one batch: open (creating as needed), append
/// the records in order, seal, and return the sealed receipt. Batches are
/// bounded ([`TRACE_EXPORT_MAX_BATCH_RECORDS`]) -- a larger batch is a
/// loud refusal, never a silent truncation.
pub fn export_trace_pipeline(
    dir: impl Into<PathBuf>,
    records: &[TraceRecord],
) -> Result<TraceExportReceipt, TraceExportError> {
    let dir = dir.into();
    if records.len() > TRACE_EXPORT_MAX_BATCH_RECORDS {
        return Err(TraceExportError::InvalidManifest(format!(
            "trace batch of {} records exceeds the bound {}",
            records.len(),
            TRACE_EXPORT_MAX_BATCH_RECORDS
        )));
    }
    let current = open_trace_export(&dir)?;
    let start = current.records.len() as u64;
    for (offset, record) in records.iter().enumerate() {
        if record.seq != start + offset as u64 {
            return Err(TraceExportError::InvalidRecord {
                seq: record.seq,
                detail: format!("pipeline batch must continue the chain at seq {}", start + offset as u64),
            });
        }
        append_trace_record(&dir, record)?;
    }
    let head = seal_trace_export(&dir)?;
    let snapshot = replay_and_verify(&dir)?;
    Ok(TraceExportReceipt {
        schema_version: TRACE_EXPORT_SCHEMA_VERSION,
        records: snapshot.records.len(),
        head,
        sealed: true,
        export_dir: dir.display().to_string(),
        abi_version: TRACE_EXPORT_ABI_VERSION.to_owned(),
    })
}

/// Read back an export and verify its chain and seal (alias for
/// [`open_trace_export`]).
pub fn read_trace_export(
    dir: impl Into<PathBuf>,
) -> Result<TraceExportSnapshot, TraceExportError> {
    open_trace_export(dir)
}

// ---------------------------------------------------------------------------
// Decision-boundary annotation artifact.
// ---------------------------------------------------------------------------

/// One annotated boundary line in a summary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionBoundaryLine {
    pub seq: u64,
    pub kind: TraceEventKind,
    pub boundary: DecisionBoundaryKind,
    pub rationale: String,
    pub decision_digest: Sha256Digest,
    pub record_digest: Sha256Digest,
}

/// Sealed summary of every decision boundary annotated in an export: the
/// decision-boundary annotation artifact.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionBoundarySummary {
    pub schema_version: u16,
    pub export_dir: String,
    pub boundaries: Vec<DecisionBoundaryLine>,
    pub annotated_records: usize,
    pub abi_version: String,
}

impl DecisionBoundarySummary {
    pub fn digest(&self) -> Sha256Digest {
        let bytes = canonical_bytes(self).expect("decision-boundary summary is JSON-serializable");
        let mut tagged = Vec::with_capacity(TRACE_EXPORT_DOMAIN.len() + 128);
        tagged.extend_from_slice(TRACE_EXPORT_DOMAIN);
        tagged.extend_from_slice(&bytes);
        Sha256Digest::from_bytes(zero_abi::sha256(&tagged))
    }
}

/// Build the decision-boundary annotation artifact for an export: every
/// cache-decision record must carry an annotation (it does -- the typed
/// constructor enforces it), and every annotated record is listed with its
/// sealed decision digest. Unannotated records are listed as
/// [`DecisionBoundaryKind`]-free lines only when they are not decisions;
/// a decision record without an annotation is a loud error.
pub fn summarize_decision_boundaries(
    dir: impl Into<PathBuf>,
) -> Result<DecisionBoundarySummary, TraceExportError> {
    let dir = dir.into();
    let snapshot = open_trace_export(&dir)?;
    let mut boundaries = Vec::new();
    for record in &snapshot.records {
        if record.kind == TraceEventKind::CacheDecision
            && record.decision_boundary.is_none()
        {
            return Err(TraceExportError::InvalidRecord {
                seq: record.seq,
                detail: "cache-decision record lacks a decision-boundary annotation".to_owned(),
            });
        }
        if let Some(annotation) = &record.decision_boundary {
            boundaries.push(DecisionBoundaryLine {
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
    Ok(DecisionBoundarySummary {
        schema_version: TRACE_EXPORT_SCHEMA_VERSION,
        export_dir: dir.display().to_string(),
        boundaries,
        annotated_records,
        abi_version: TRACE_EXPORT_ABI_VERSION.to_owned(),
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
pub enum BenchmarkReproducibility {
    Sealed,
    NonReproducible { reason: String },
}

/// Sealed benchmark manifest: every input digest, canonical parameters,
/// result and receipt digests, and a reproducibility statement. The
/// manifest digest (seal) is content-derived, so a result is reproducible
/// from the manifest or explicitly marked otherwise.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SealedBenchmarkManifest {
    pub schema_version: u16,
    pub benchmark_id: String,
    pub workload_digest: Sha256Digest,
    pub engine_digest: Sha256Digest,
    pub worker_digests: Vec<String>,
    pub parameters: serde_json::Value,
    pub result_digest: Sha256Digest,
    pub receipt_digests: Vec<String>,
    pub reproducibility: BenchmarkReproducibility,
    pub abi_version: String,
}

impl SealedBenchmarkManifest {
    pub fn new(
        benchmark_id: impl Into<String>,
        workload_digest: Sha256Digest,
        engine_digest: Sha256Digest,
        worker_digests: Vec<String>,
        parameters: serde_json::Value,
        result_digest: Sha256Digest,
        receipt_digests: Vec<String>,
        reproducibility: BenchmarkReproducibility,
    ) -> Result<Self, TraceExportError> {
        let manifest = Self {
            schema_version: TRACE_EXPORT_SCHEMA_VERSION,
            benchmark_id: benchmark_id.into(),
            workload_digest,
            engine_digest,
            worker_digests,
            parameters,
            result_digest,
            receipt_digests,
            reproducibility,
            abi_version: TRACE_EXPORT_ABI_VERSION.to_owned(),
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), TraceExportError> {
        if self.schema_version != TRACE_EXPORT_SCHEMA_VERSION {
            return Err(TraceExportError::InvalidManifest(
                "manifest schema version is not supported".to_owned(),
            ));
        }
        if self.benchmark_id.is_empty()
            || self.workload_digest == Sha256Digest::ZERO
            || self.engine_digest == Sha256Digest::ZERO
            || self.result_digest == Sha256Digest::ZERO
        {
            return Err(TraceExportError::InvalidManifest(
                "benchmark id and workload/engine/result digests must be nonzero".to_owned(),
            ));
        }
        if let BenchmarkReproducibility::NonReproducible { reason } = &self.reproducibility {
            if reason.is_empty() {
                return Err(TraceExportError::InvalidManifest(
                    "nonreproducible benchmark must carry a reason".to_owned(),
                ));
            }
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, TraceExportError> {
        self.validate()?;
        canonical_bytes(self)
    }

    /// The seal: content-derived digest over the domain-tagged canonical
    /// manifest bytes. Same inputs, same seal; tampering changes the seal.
    pub fn digest(&self) -> Result<Sha256Digest, TraceExportError> {
        let mut tagged = Vec::with_capacity(TRACE_EXPORT_DOMAIN.len() + 128);
        tagged.extend_from_slice(TRACE_EXPORT_DOMAIN);
        tagged.extend_from_slice(&self.canonical_bytes()?);
        Ok(Sha256Digest::from_bytes(zero_abi::sha256(&tagged)))
    }
}

/// The persisted form of a sealed manifest: manifest next to its seal.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SealedManifestFile {
    pub schema_version: u16,
    pub manifest: SealedBenchmarkManifest,
    pub seal: Sha256Digest,
    pub abi_version: String,
}

impl SealedManifestFile {
    pub fn seal(manifest: SealedBenchmarkManifest) -> Result<Self, TraceExportError> {
        let seal = manifest.digest()?;
        Ok(Self {
            schema_version: TRACE_EXPORT_SCHEMA_VERSION,
            manifest,
            seal,
            abi_version: TRACE_EXPORT_ABI_VERSION.to_owned(),
        })
    }

    /// Verify the seal against the manifest bytes; tampering is loud.
    pub fn verify_seal(&self) -> Result<(), TraceExportError> {
        if self.schema_version != TRACE_EXPORT_SCHEMA_VERSION {
            return Err(TraceExportError::InvalidManifest(
                "sealed manifest file schema version is not supported".to_owned(),
            ));
        }
        let recomputed = self.manifest.digest()?;
        if recomputed != self.seal {
            return Err(TraceExportError::InvalidManifest(format!(
                "seal mismatch: persisted seal {} != recomputed {recomputed}",
                self.seal
            )));
        }
        Ok(())
    }
}

/// Export a sealed benchmark manifest into the export directory. The seal
/// is persisted next to the manifest; read-back re-verifies it.
pub fn export_benchmark_manifest(
    dir: impl Into<PathBuf>,
    manifest: SealedBenchmarkManifest,
) -> Result<Sha256Digest, TraceExportError> {
    let dir = dir.into();
    fs::create_dir_all(&dir)
        .map_err(|error| TraceExportError::Io(format!("create {}: {error}", dir.display())))?;
    let file = SealedManifestFile::seal(manifest)?;
    let content = serde_json::to_vec(&file).map_err(|error| {
        TraceExportError::InvalidManifest(format!("manifest not serializable: {error}"))
    })?;
    let path = dir.join(TRACE_EXPORT_MANIFEST_FILE);
    fs::write(&path, content)
        .map_err(|error| TraceExportError::Io(format!("write {}: {error}", path.display())))?;
    Ok(file.seal)
}

/// Read back and verify a sealed benchmark manifest.
pub fn read_exported_benchmark_manifest(
    dir: impl Into<PathBuf>,
) -> Result<SealedBenchmarkManifest, TraceExportError> {
    let dir = dir.into();
    let path = dir.join(TRACE_EXPORT_MANIFEST_FILE);
    if !path.exists() {
        return Err(TraceExportError::InvalidManifest(format!(
            "no benchmark manifest at {}",
            path.display()
        )));
    }
    let content = fs::read_to_string(&path)
        .map_err(|error| TraceExportError::Io(format!("read {}: {error}", path.display())))?;
    let file: SealedManifestFile = serde_json::from_str(&content).map_err(|error| {
        TraceExportError::InvalidManifest(format!("cannot parse manifest: {error}"))
    })?;
    file.verify_seal()?;
    Ok(file.manifest)
}

/// The frozen contract manifest for the trace-export surface (ZS-OPS-003).
pub fn trace_export_contract() -> serde_json::Value {
    serde_json::json!({
        "schema_version": TRACE_EXPORT_SCHEMA_VERSION,
        "pipeline": {
            "records_file": TRACE_EXPORT_RECORDS_FILE,
            "sealed_head_file": TRACE_EXPORT_SEALED_HEAD_FILE,
            "manifest_file": TRACE_EXPORT_MANIFEST_FILE,
            "fail_closed": "torn tails, tampered or reordered records, head mismatches",
            "max_batch_records": TRACE_EXPORT_MAX_BATCH_RECORDS,
        },
        "decision_boundary": {
            "cache_decision_records": "annotation required (typed constructor)",
            "artifact": "DecisionBoundarySummary with sealed decision digests",
        },
        "benchmark_manifests": {
            "reproducibility": "sealed or explicitly nonreproducible with reason",
            "seal": "content-derived over canonical manifest bytes",
        },
        "abi_version": TRACE_EXPORT_ABI_VERSION,
    })
}
