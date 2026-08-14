#![forbid(unsafe_code)]

//! Pure, synchronous verification for proof-carrying RACC evidence.
//!
//! VerifiedEvidence cannot be constructed externally:
//! ~~~compile_fail
//! use zero_cert::{EvidenceCertificate, VerifiedEvidence};
//! fn forge(c: &'static EvidenceCertificate<'static>) -> VerifiedEvidence<'static, 'static> {
//!     VerifiedEvidence { certificate: c }
//! }
//! ~~~

pub mod boundary_audit;
pub mod effect_witness;
pub mod kernel_runtime;
pub mod trace_export;
pub mod worker_trust;

pub use boundary_audit::{
    AuthorityBoundaryAuditReportV1, AuthoritySurfaceV1, BOUNDARY_AUDIT_ABI_VERSION_V1,
    BOUNDARY_AUDIT_DOMAIN_V1, BOUNDARY_AUDIT_SCHEMA_VERSION_V1, BoundaryAuditErrorV1,
    ConstructionSurfaceV1, authority_boundary_audit_v1, verify_commit_authority_v1,
    verify_decision_authority_v1,
};

pub use effect_witness::{
    EFFECT_ACCEPTED_DOMAIN_V1, EFFECT_EVIDENCE_REF_DOMAIN_V1, EFFECT_WITNESS_CONTRACT_VERSION_V1,
    EFFECT_WITNESS_DOMAIN_V1, EFFECT_WITNESS_MAX_CANONICAL_BYTES_V1,
    EFFECT_WITNESS_MAX_EVIDENCE_REFS_V1, EFFECT_WITNESS_MAX_EXPANSIONS_V1, EffectAcceptedV1,
    EffectLocalizationClassV1, EffectLocalizationV1, EffectVerificationOutcomeV1,
    EffectWitnessErrorV1, EffectWitnessFailureCodeV1, EffectWitnessKindV1, EffectWitnessV1,
    accept_effect_verification_v1, effect_witness_contract_digest_v1,
    effect_witness_contract_manifest_v1, incomplete_effect_verification_v1,
    reject_effect_verification_v1,
};
pub use kernel_runtime::{
    CACHE_ADMISSION_DOMAIN_V1, CacheAdmissionGateV1, CacheAdmissionRecordV1,
    EVENT_JOURNAL_RECORDS_FILE_V1, EVENT_JOURNAL_SEALED_HEAD_FILE_V1, FileEventJournalStore,
    InMemoryJournalStore, KERNEL_RUNTIME_VERSION_V1, KernelEventJournalV1, KernelRuntimeError,
    JournalStore, ProjectRootGateV1, RootGateFaultV1, RootGateSessionV1,
};
pub use trace_export::{
    BenchmarkReproducibilityV1, DecisionBoundaryAnnotationV1, DecisionBoundaryKindV1,
    DecisionBoundaryLineV1, DecisionBoundarySummaryV1, SealedBenchmarkManifestV1,
    SealedManifestFileV1, TRACE_EXPORT_ABI_VERSION_V1, TRACE_EXPORT_DOMAIN_V1,
    TRACE_EXPORT_MANIFEST_FILE_V1, TRACE_EXPORT_MAX_BATCH_RECORDS_V1,
    TRACE_EXPORT_MAX_RECORD_BYTES_V1, TRACE_EXPORT_RECORDS_FILE_V1,
    TRACE_EXPORT_SCHEMA_VERSION_V1, TRACE_EXPORT_SEALED_HEAD_FILE_V1, TraceEventKindV1,
    TraceExportErrorV1, TraceExportReceiptV1, TraceExportSnapshotV1, TraceRecordV1,
    TraceSealedHeadV1, append_trace_record_v1, export_benchmark_manifest_v1,
    export_trace_pipeline_v1, open_trace_export_v1, read_exported_benchmark_manifest_v1,
    read_trace_export_v1, seal_trace_export_v1, summarize_decision_boundaries_v1,
    trace_export_contract_v1,
};
pub use worker_trust::{
    TrustContextV1, WORKER_TRUST_ABI_VERSION_V1, WORKER_TRUST_ADMISSION_DOMAIN_V1,
    WORKER_TRUST_ENVELOPE_DOMAIN_V1, WORKER_TRUST_REFUSAL_DOMAIN_V1,
    WORKER_TRUST_SCHEMA_VERSION_V1, WorkerAdmissionReceiptV1, WorkerEnvelopeV1,
    WorkerFrameV1, WorkerIdentityClaimV1, WorkerRefusalReasonV1, WorkerRefusalRecordV1,
    WorkerTraceV1, WorkerTrustBoundaryV1, WorkerTrustErrorV1, worker_trust_contract_v1,
};

use serde::{Deserialize, Serialize};
use std::{borrow::Cow, fmt};

pub use zero_ref::{
    Digest, OBJECT_ID_HASH_ALGORITHM, OBJECT_ID_HEX_LENGTH, ObjectId, SpanRef, object_identity_hex,
};
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SymbolId(pub u64);
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct NodeId(pub u64);
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct CommandId(pub u64);
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct TestId(pub u64);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(bound(deserialize = "'de: 'a"))]
pub enum Query<'a> {
    ReadSpan(SpanRef),
    ExactSearch {
        scope: ObjectId,
        #[serde(borrow)]
        pattern: Cow<'a, [u8]>,
    },
    ExactSearchDomain {
        #[serde(borrow)]
        pattern: Cow<'a, [u8]>,
        objects: Vec<ObjectId>,
        snapshot_id: Digest,
        index_id: String,
        index_version: String,
    },
    Definition {
        symbol: SymbolId,
    },
    References {
        symbol: SymbolId,
    },
    AstClosure {
        seeds: Vec<NodeId>,
        relations: u64,
        radius: u32,
    },
    CallPath {
        source: SymbolId,
        target: SymbolId,
    },
    DataflowSlice {
        sink: NodeId,
    },
    Diff {
        old: ObjectId,
        new: ObjectId,
    },
    ByteExactDiff {
        old: ObjectId,
        new: ObjectId,
        start: u64,
        before_end: u64,
        after_end: u64,
    },
    MutationOutcome {
        journal_id: Digest,
        sequence: u64,
        old: ObjectId,
        new: ObjectId,
        applied: bool,
    },
    Aggregate {
        snapshot_id: Digest,
        objects: Vec<ObjectId>,
        requested: u64,
        emitted: u64,
    },
    BuildReceipt {
        command: CommandId,
    },
    TestTrace {
        test: TestId,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Provenance {
    pub parser_id: String,
    pub parser_version: String,
    pub index_id: String,
    pub index_version: String,
    pub operator_id: String,
    pub operator_version: String,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperatorLock {
    pub operator_id: String,
    pub operator_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MutationReceipt {
    pub journal_id: Digest,
    pub sequence: u64,
    pub old: ObjectId,
    pub new: ObjectId,
    pub applied: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AggregateReceipt {
    pub snapshot_id: Digest,
    pub objects: Vec<ObjectId>,
    pub requested: u64,
    pub emitted: u64,
    pub result_digest: Digest,
}

pub fn domain_snapshot_digest(objects: &[ObjectId], index_id: &str, index_version: &str) -> Digest {
    let value = serde_json::json!({
        "index_id": index_id,
        "index_version": index_version,
        "objects": objects,
    });
    zero_abi::sha256(zero_abi::canonical_json(&value).as_bytes())
}

/// Query-bound proof shapes. Deliberately has no semantic-summary variant.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(bound(deserialize = "'de: 'a"))]
pub enum CompletenessWitness<'a> {
    ReadSpan {
        operator: OperatorLock,
    },
    ExactSearch {
        operator: OperatorLock,
        scope: ObjectId,
        #[serde(borrow)]
        pattern: Cow<'a, [u8]>,
        scope_len: u64,
        match_count: u64,
    },
    ExactSearchDomain {
        operator: OperatorLock,
        #[serde(borrow)]
        pattern: Cow<'a, [u8]>,
        objects: Vec<ObjectId>,
        snapshot_id: Digest,
        index_id: String,
        index_version: String,
        match_count: u64,
    },
    Definition {
        operator: OperatorLock,
        symbol: SymbolId,
        index_id: String,
        index_version: String,
    },
    References {
        operator: OperatorLock,
        symbol: SymbolId,
        index_id: String,
        index_version: String,
        match_count: u64,
    },
    AstClosure {
        operator: OperatorLock,
        seeds: Vec<NodeId>,
        relations: u64,
        radius: u32,
        parser_id: String,
        parser_version: String,
        visited_nodes: u64,
    },
    CallPath {
        operator: OperatorLock,
        source: SymbolId,
        target: SymbolId,
        edge_count: u64,
    },
    DataflowSlice {
        operator: OperatorLock,
        sink: NodeId,
        visited_nodes: u64,
    },
    Diff {
        operator: OperatorLock,
        old: ObjectId,
        new: ObjectId,
    },
    ByteExactDiff {
        operator: OperatorLock,
        old: ObjectId,
        new: ObjectId,
        start: u64,
        before_end: u64,
        after_end: u64,
        replacement_digest: Digest,
    },
    MutationOutcome {
        operator: OperatorLock,
        journal_id: Digest,
        sequence: u64,
        old: ObjectId,
        new: ObjectId,
        applied: bool,
        receipt_digest: Digest,
    },
    Aggregate {
        operator: OperatorLock,
        snapshot_id: Digest,
        objects: Vec<ObjectId>,
        requested: u64,
        emitted: u64,
        result_digest: Digest,
    },
    BuildReceipt {
        operator: OperatorLock,
        command: CommandId,
        exit_code: i32,
        stdout_digest: Digest,
        stderr_digest: Digest,
    },
    TestTrace {
        operator: OperatorLock,
        test: TestId,
        exit_code: i32,
        trace_digest: Digest,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(bound(deserialize = "'de: 'a"))]
pub struct EvidenceCertificate<'a> {
    #[serde(borrow)]
    pub query: Query<'a>,
    pub spans: Vec<SpanRef>,
    #[serde(borrow)]
    pub payload: Cow<'a, [u8]>,
    pub provenance: Provenance,
    #[serde(borrow)]
    pub completeness: CompletenessWitness<'a>,
    pub input_token_cost: u64,
    pub backend_work_units: u64,
}
impl EvidenceCertificate<'_> {
    pub fn canonical_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_value(self).map(|v| zero_abi::canonical_json(&v))
    }
    pub fn canonical_digest(&self) -> Result<Digest, serde_json::Error> {
        self.canonical_json()
            .map(|v| zero_abi::sha256(v.as_bytes()))
    }
}

/// Immutable resident bytes and verifier-owned trusted lock lookup. Implementations must not perform I/O.
pub trait Resolver {
    fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]>;
    fn trusted_operator_version<'a>(&'a self, operator_id: &str) -> Option<&'a str>;
    fn trusted_parser_version<'a>(&'a self, parser_id: &str) -> Option<&'a str>;
    fn trusted_index_version<'a>(&'a self, index_id: &str) -> Option<&'a str>;
    fn resolve_mutation_receipt<'a>(
        &'a self,
        _journal_id: &Digest,
        _sequence: u64,
    ) -> Option<&'a [u8]> {
        None
    }
    fn resolve_aggregate_receipt<'a>(&'a self, _snapshot_id: &Digest) -> Option<&'a [u8]> {
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerificationError {
    MissingObject { object_id: ObjectId },
    RangeOverflow { span_index: usize },
    RangeOutsideObject { span_index: usize },
    ObjectIdentityMismatch { span_index: usize },
    ObjectDigestMismatch { span_index: usize },
    SpanDigestMismatch { span_index: usize },
    PayloadLengthMismatch,
    PayloadMismatch { span_index: usize },
    WitnessQueryMismatch,
    MissingTrustedOperator,
    MissingTrustedParser,
    MissingTrustedIndex,
    MissingTrustedReceipt,
    MalformedTrustedReceipt,
    StaleOperator,
    StaleParser,
    StaleIndex,
    InvalidCompleteness,
}
impl fmt::Display for VerificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}
impl std::error::Error for VerificationError {}

#[derive(Debug)]
pub struct VerifiedEvidence<'certificate, 'payload> {
    certificate: &'certificate EvidenceCertificate<'payload>,
}
impl<'certificate, 'payload> VerifiedEvidence<'certificate, 'payload> {
    pub fn certificate(&self) -> &'certificate EvidenceCertificate<'payload> {
        self.certificate
    }
    pub fn query(&self) -> &'certificate Query<'payload> {
        &self.certificate.query
    }
    pub fn spans(&self) -> &'certificate [SpanRef] {
        &self.certificate.spans
    }
    pub fn payload(&self) -> &'certificate [u8] {
        self.certificate.payload.as_ref()
    }
    pub fn provenance(&self) -> &'certificate Provenance {
        &self.certificate.provenance
    }
    pub fn input_token_cost(&self) -> u64 {
        self.certificate.input_token_cost
    }
    pub fn backend_work_units(&self) -> u64 {
        self.certificate.backend_work_units
    }
}

pub fn verify<'certificate, 'payload, R: Resolver + ?Sized>(
    certificate: &'certificate EvidenceCertificate<'payload>,
    resolver: &R,
) -> Result<VerifiedEvidence<'certificate, 'payload>, VerificationError> {
    check_trusted_provenance(&certificate.provenance, resolver)?;
    verify_spans(certificate, resolver)?;
    verify_completeness(certificate, resolver)?;
    Ok(VerifiedEvidence { certificate })
}

fn verify_spans<R: Resolver + ?Sized>(
    c: &EvidenceCertificate<'_>,
    resolver: &R,
) -> Result<(), VerificationError> {
    if matches!(
        &c.query,
        Query::MutationOutcome { .. } | Query::Aggregate { .. }
    ) {
        return invalid_if(c.spans.is_empty());
    }
    let mut payload_offset = 0usize;
    for (index, span) in c.spans.iter().enumerate() {
        payload_offset = verify_span(index, span, c.payload.as_ref(), payload_offset, resolver)?;
    }
    if payload_offset != c.payload.len() {
        return Err(VerificationError::PayloadLengthMismatch);
    }
    Ok(())
}

fn verify_span<R: Resolver + ?Sized>(
    index: usize,
    span: &SpanRef,
    payload: &[u8],
    payload_offset: usize,
    resolver: &R,
) -> Result<usize, VerificationError> {
    let object = resolver
        .resolve(&span.object_id)
        .ok_or(VerificationError::MissingObject {
            object_id: span.object_id,
        })?;
    let digest = zero_abi::sha256(object);
    if digest != span.object_id.0 {
        return Err(VerificationError::ObjectIdentityMismatch { span_index: index });
    }
    if digest != span.object_digest {
        return Err(VerificationError::ObjectDigestMismatch { span_index: index });
    }
    let start = usize::try_from(span.byte_start)
        .map_err(|_| VerificationError::RangeOutsideObject { span_index: index })?;
    let len = usize::try_from(span.byte_len)
        .map_err(|_| VerificationError::RangeOutsideObject { span_index: index })?;
    let end = start
        .checked_add(len)
        .ok_or(VerificationError::RangeOverflow { span_index: index })?;
    let bytes = object
        .get(start..end)
        .ok_or(VerificationError::RangeOutsideObject { span_index: index })?;
    if zero_abi::sha256(bytes) != span.span_digest {
        return Err(VerificationError::SpanDigestMismatch { span_index: index });
    }
    verify_payload_span(index, payload, payload_offset, bytes, len)
}

fn verify_payload_span(
    index: usize,
    payload: &[u8],
    payload_offset: usize,
    bytes: &[u8],
    len: usize,
) -> Result<usize, VerificationError> {
    let payload_end = payload_offset
        .checked_add(len)
        .ok_or(VerificationError::PayloadLengthMismatch)?;
    if payload
        .get(payload_offset..payload_end)
        .ok_or(VerificationError::PayloadLengthMismatch)?
        != bytes
    {
        return Err(VerificationError::PayloadMismatch { span_index: index });
    }
    Ok(payload_end)
}

fn check_trusted_provenance<R: Resolver + ?Sized>(
    p: &Provenance,
    r: &R,
) -> Result<(), VerificationError> {
    check_trusted_operator(p, r)?;
    check_trusted_parser(p, r)?;
    check_trusted_index(p, r)
}

fn check_trusted_operator<R: Resolver + ?Sized>(
    p: &Provenance,
    r: &R,
) -> Result<(), VerificationError> {
    let version = r
        .trusted_operator_version(&p.operator_id)
        .ok_or(VerificationError::MissingTrustedOperator)?;
    if version != p.operator_version {
        return Err(VerificationError::StaleOperator);
    }
    Ok(())
}

fn check_trusted_parser<R: Resolver + ?Sized>(
    p: &Provenance,
    r: &R,
) -> Result<(), VerificationError> {
    let version = r
        .trusted_parser_version(&p.parser_id)
        .ok_or(VerificationError::MissingTrustedParser)?;
    if version != p.parser_version {
        return Err(VerificationError::StaleParser);
    }
    Ok(())
}

fn check_trusted_index<R: Resolver + ?Sized>(
    p: &Provenance,
    r: &R,
) -> Result<(), VerificationError> {
    let version = r
        .trusted_index_version(&p.index_id)
        .ok_or(VerificationError::MissingTrustedIndex)?;
    if version != p.index_version {
        return Err(VerificationError::StaleIndex);
    }
    Ok(())
}

fn check_operator(lock: &OperatorLock, p: &Provenance) -> Result<(), VerificationError> {
    if lock.operator_id == p.operator_id && lock.operator_version == p.operator_version {
        Ok(())
    } else {
        Err(VerificationError::StaleOperator)
    }
}
fn check_parser(id: &str, version: &str, p: &Provenance) -> Result<(), VerificationError> {
    if id == p.parser_id && version == p.parser_version {
        Ok(())
    } else {
        Err(VerificationError::StaleParser)
    }
}
fn check_index(id: &str, version: &str, p: &Provenance) -> Result<(), VerificationError> {
    if id == p.index_id && version == p.index_version {
        Ok(())
    } else {
        Err(VerificationError::StaleIndex)
    }
}

fn verify_completeness<R: Resolver + ?Sized>(
    c: &EvidenceCertificate<'_>,
    resolver: &R,
) -> Result<(), VerificationError> {
    use Query as Q;
    match &c.query {
        Q::ReadSpan(_) => verify_read_span(c),
        Q::ExactSearch { .. } => verify_search_binding(c, resolver),
        Q::ExactSearchDomain { .. } => verify_search_domain(c, resolver),
        Q::Definition { .. } => verify_definition(c),
        Q::References { .. } => verify_references(c),
        Q::AstClosure { .. } => verify_ast_closure(c),
        _ => verify_graph_or_execution(c, resolver),
    }
}

fn verify_graph_or_execution<R: Resolver + ?Sized>(
    c: &EvidenceCertificate<'_>,
    resolver: &R,
) -> Result<(), VerificationError> {
    match &c.query {
        Query::CallPath { .. } => verify_call_path(c),
        Query::DataflowSlice { .. } => verify_dataflow_slice(c),
        Query::Diff { .. } => verify_diff(c),
        Query::ByteExactDiff { .. } => verify_byte_exact_diff(c, resolver),
        Query::MutationOutcome { .. } => verify_mutation_outcome(c, resolver),
        Query::Aggregate { .. } => verify_aggregate(c, resolver),
        Query::BuildReceipt { .. } => verify_build_receipt(c),
        Query::TestTrace { .. } => verify_test_trace(c),
        _ => Err(witness_mismatch()),
    }
}

fn witness_mismatch() -> VerificationError {
    VerificationError::WitnessQueryMismatch
}
fn invalid_if(valid: bool) -> Result<(), VerificationError> {
    if valid {
        Ok(())
    } else {
        Err(VerificationError::InvalidCompleteness)
    }
}

fn verify_read_span(c: &EvidenceCertificate<'_>) -> Result<(), VerificationError> {
    let (Query::ReadSpan(wanted), CompletenessWitness::ReadSpan { operator }) =
        (&c.query, &c.completeness)
    else {
        return Err(witness_mismatch());
    };
    check_operator(operator, &c.provenance)?;
    invalid_if(c.spans.as_slice() == std::slice::from_ref(wanted))
}

fn verify_search_binding<R: Resolver + ?Sized>(
    c: &EvidenceCertificate<'_>,
    resolver: &R,
) -> Result<(), VerificationError> {
    let (
        Query::ExactSearch { scope, pattern },
        CompletenessWitness::ExactSearch {
            operator,
            scope: bound_scope,
            pattern: bound_pattern,
            scope_len,
            match_count,
        },
    ) = (&c.query, &c.completeness)
    else {
        return Err(witness_mismatch());
    };
    check_operator(operator, &c.provenance)?;
    if scope != bound_scope || pattern.as_ref() != bound_pattern.as_ref() {
        return Err(witness_mismatch());
    }
    verify_search(*scope, pattern, *scope_len, *match_count, c, resolver)
}

fn verify_definition(c: &EvidenceCertificate<'_>) -> Result<(), VerificationError> {
    let (
        Query::Definition { symbol },
        CompletenessWitness::Definition {
            operator,
            symbol: bound,
            index_id,
            index_version,
        },
    ) = (&c.query, &c.completeness)
    else {
        return Err(witness_mismatch());
    };
    check_index(index_id, index_version, &c.provenance)?;
    if symbol != bound {
        return Err(witness_mismatch());
    }
    check_operator(operator, &c.provenance)?;
    invalid_if(c.spans.len() == 1)
}

fn verify_references(c: &EvidenceCertificate<'_>) -> Result<(), VerificationError> {
    let (
        Query::References { symbol },
        CompletenessWitness::References {
            operator,
            symbol: bound,
            index_id,
            index_version,
            match_count,
        },
    ) = (&c.query, &c.completeness)
    else {
        return Err(witness_mismatch());
    };
    check_index(index_id, index_version, &c.provenance)?;
    if symbol != bound {
        return Err(witness_mismatch());
    }
    check_operator(operator, &c.provenance)?;
    invalid_if(usize::try_from(*match_count).ok() == Some(c.spans.len()))
}

fn verify_ast_closure(c: &EvidenceCertificate<'_>) -> Result<(), VerificationError> {
    let (
        Query::AstClosure {
            seeds,
            relations,
            radius,
        },
        CompletenessWitness::AstClosure {
            operator,
            seeds: bound_seeds,
            relations: bound_relations,
            radius: bound_radius,
            parser_id,
            parser_version,
            visited_nodes,
        },
    ) = (&c.query, &c.completeness)
    else {
        return Err(witness_mismatch());
    };
    check_parser(parser_id, parser_version, &c.provenance)?;
    if seeds != bound_seeds || relations != bound_relations || radius != bound_radius {
        return Err(witness_mismatch());
    }
    check_operator(operator, &c.provenance)?;
    invalid_if(*visited_nodes > 0 && !c.spans.is_empty())
}

fn verify_call_path(c: &EvidenceCertificate<'_>) -> Result<(), VerificationError> {
    let (
        Query::CallPath { source, target },
        CompletenessWitness::CallPath {
            operator,
            source: bound_source,
            target: bound_target,
            edge_count,
        },
    ) = (&c.query, &c.completeness)
    else {
        return Err(witness_mismatch());
    };
    if source != bound_source || target != bound_target {
        return Err(witness_mismatch());
    }
    check_operator(operator, &c.provenance)?;
    invalid_if(
        usize::try_from(*edge_count)
            .ok()
            .and_then(|n| n.checked_add(1))
            == Some(c.spans.len()),
    )
}

fn verify_dataflow_slice(c: &EvidenceCertificate<'_>) -> Result<(), VerificationError> {
    let (
        Query::DataflowSlice { sink },
        CompletenessWitness::DataflowSlice {
            operator,
            sink: bound,
            visited_nodes,
        },
    ) = (&c.query, &c.completeness)
    else {
        return Err(witness_mismatch());
    };
    if sink != bound {
        return Err(witness_mismatch());
    }
    check_operator(operator, &c.provenance)?;
    invalid_if(*visited_nodes > 0 && !c.spans.is_empty())
}

fn resolve_hashed<'a, R: Resolver + ?Sized>(
    object_id: &ObjectId,
    resolver: &'a R,
) -> Result<&'a [u8], VerificationError> {
    let bytes = resolver
        .resolve(object_id)
        .ok_or(VerificationError::MissingObject {
            object_id: *object_id,
        })?;
    if zero_abi::sha256(bytes) != object_id.0 {
        return Err(VerificationError::InvalidCompleteness);
    }
    Ok(bytes)
}

fn verify_search_domain<R: Resolver + ?Sized>(
    c: &EvidenceCertificate<'_>,
    resolver: &R,
) -> Result<(), VerificationError> {
    let (
        Query::ExactSearchDomain {
            pattern,
            objects,
            snapshot_id,
            index_id,
            index_version,
        },
        CompletenessWitness::ExactSearchDomain {
            operator,
            pattern: bound_pattern,
            objects: bound_objects,
            snapshot_id: bound_snapshot,
            index_id: bound_index,
            index_version: bound_version,
            match_count,
        },
    ) = (&c.query, &c.completeness)
    else {
        return Err(witness_mismatch());
    };
    if pattern != bound_pattern
        || objects != bound_objects
        || snapshot_id != bound_snapshot
        || index_id != bound_index
        || index_version != bound_version
    {
        return Err(witness_mismatch());
    }
    check_operator(operator, &c.provenance)?;
    check_index(index_id, index_version, &c.provenance)?;
    if pattern.is_empty()
        || objects.is_empty()
        || *snapshot_id != domain_snapshot_digest(objects, index_id, index_version)
    {
        return Err(VerificationError::InvalidCompleteness);
    }
    let mut span_index = 0usize;
    let mut matches = 0u64;
    for object_id in objects {
        let bytes = resolve_hashed(object_id, resolver)?;
        verify_domain_matches(
            c,
            object_id,
            bytes,
            pattern.as_ref(),
            &mut span_index,
            &mut matches,
        )?;
    }
    invalid_if(span_index == c.spans.len() && matches == *match_count)
}

fn verify_domain_matches(
    c: &EvidenceCertificate<'_>,
    object_id: &ObjectId,
    bytes: &[u8],
    pattern: &[u8],
    span_index: &mut usize,
    matches: &mut u64,
) -> Result<(), VerificationError> {
    for start in 0..=bytes.len().saturating_sub(pattern.len()) {
        if bytes.get(start..start + pattern.len()) == Some(pattern) {
            let span = c
                .spans
                .get(*span_index)
                .ok_or(VerificationError::InvalidCompleteness)?;
            if !domain_span_matches(span, object_id, start, pattern) {
                return Err(VerificationError::InvalidCompleteness);
            }
            *span_index += 1;
            *matches = matches
                .checked_add(1)
                .ok_or(VerificationError::InvalidCompleteness)?;
        }
    }
    Ok(())
}

fn domain_span_matches(span: &SpanRef, object_id: &ObjectId, start: usize, pattern: &[u8]) -> bool {
    span.object_id == *object_id
        && span.object_digest == object_id.0
        && span.byte_start == start as u64
        && span.byte_len == pattern.len() as u64
        && span.span_digest == zero_abi::sha256(pattern)
}

fn verify_byte_exact_diff<R: Resolver + ?Sized>(
    c: &EvidenceCertificate<'_>,
    resolver: &R,
) -> Result<(), VerificationError> {
    let (
        Query::ByteExactDiff {
            old,
            new,
            start,
            before_end,
            after_end,
        },
        CompletenessWitness::ByteExactDiff {
            operator,
            old: bound_old,
            new: bound_new,
            start: bound_start,
            before_end: bound_before,
            after_end: bound_after,
            replacement_digest,
        },
    ) = (&c.query, &c.completeness)
    else {
        return Err(witness_mismatch());
    };
    if (old, new, start, before_end, after_end)
        != (bound_old, bound_new, bound_start, bound_before, bound_after)
    {
        return Err(witness_mismatch());
    }
    check_operator(operator, &c.provenance)?;
    if old == new {
        return Err(VerificationError::InvalidCompleteness);
    }
    let old_bytes = resolve_hashed(old, resolver)?;
    let new_bytes = resolve_hashed(new, resolver)?;
    let start = usize::try_from(*start).map_err(|_| VerificationError::InvalidCompleteness)?;
    let before_end =
        usize::try_from(*before_end).map_err(|_| VerificationError::InvalidCompleteness)?;
    let after_end =
        usize::try_from(*after_end).map_err(|_| VerificationError::InvalidCompleteness)?;
    verify_diff_ranges(
        c,
        old_bytes,
        new_bytes,
        start,
        before_end,
        after_end,
        replacement_digest,
        new,
    )
}

#[allow(clippy::too_many_arguments)]
fn verify_diff_ranges(
    c: &EvidenceCertificate<'_>,
    old: &[u8],
    new: &[u8],
    start: usize,
    before_end: usize,
    after_end: usize,
    replacement_digest: &Digest,
    new_id: &ObjectId,
) -> Result<(), VerificationError> {
    let selected = old
        .get(start..before_end)
        .ok_or(VerificationError::InvalidCompleteness)?;
    let replacement = new
        .get(start..after_end)
        .ok_or(VerificationError::InvalidCompleteness)?;
    if selected == replacement
        || old.get(..start) != new.get(..start)
        || old.get(before_end..) != new.get(after_end..)
        || zero_abi::sha256(replacement) != *replacement_digest
    {
        return Err(VerificationError::InvalidCompleteness);
    }
    if c.payload.as_ref() != replacement {
        return Err(VerificationError::InvalidCompleteness);
    }
    let span = c
        .spans
        .first()
        .ok_or(VerificationError::InvalidCompleteness)?;
    invalid_if(c.spans.len() == 1 && domain_span_matches(span, new_id, start, replacement))
}

fn verify_mutation_outcome<R: Resolver + ?Sized>(
    c: &EvidenceCertificate<'_>,
    resolver: &R,
) -> Result<(), VerificationError> {
    let (
        Query::MutationOutcome {
            journal_id,
            sequence,
            old,
            new,
            applied,
        },
        CompletenessWitness::MutationOutcome {
            operator,
            journal_id: bound_journal,
            sequence: bound_sequence,
            old: bound_old,
            new: bound_new,
            applied: bound_applied,
            receipt_digest,
        },
    ) = (&c.query, &c.completeness)
    else {
        return Err(witness_mismatch());
    };
    if (journal_id, sequence, old, new, applied)
        != (
            bound_journal,
            bound_sequence,
            bound_old,
            bound_new,
            bound_applied,
        )
    {
        return Err(witness_mismatch());
    }
    check_operator(operator, &c.provenance)?;
    let _ = resolve_hashed(old, resolver)?;
    let _ = resolve_hashed(new, resolver)?;
    let trusted = resolver
        .resolve_mutation_receipt(journal_id, *sequence)
        .ok_or(VerificationError::MissingTrustedReceipt)?;
    if zero_abi::sha256(trusted) != *receipt_digest || c.payload.as_ref() != trusted {
        return Err(VerificationError::InvalidCompleteness);
    }
    let receipt: MutationReceipt =
        serde_json::from_slice(trusted).map_err(|_| VerificationError::MalformedTrustedReceipt)?;
    if !mutation_receipt_matches(&receipt, journal_id, *sequence, old, new, *applied) {
        return Err(witness_mismatch());
    }
    invalid_if(*sequence != 0 && !(*applied && old == new))
}

fn mutation_receipt_matches(
    receipt: &MutationReceipt,
    journal_id: &Digest,
    sequence: u64,
    old: &ObjectId,
    new: &ObjectId,
    applied: bool,
) -> bool {
    receipt.journal_id == *journal_id
        && receipt.sequence == sequence
        && receipt.old == *old
        && receipt.new == *new
        && receipt.applied == applied
}

fn verify_aggregate<R: Resolver + ?Sized>(
    c: &EvidenceCertificate<'_>,
    resolver: &R,
) -> Result<(), VerificationError> {
    let (
        Query::Aggregate {
            snapshot_id,
            objects,
            requested,
            emitted,
        },
        CompletenessWitness::Aggregate {
            operator,
            snapshot_id: bound_snapshot,
            objects: bound_objects,
            requested: bound_requested,
            emitted: bound_emitted,
            result_digest,
        },
    ) = (&c.query, &c.completeness)
    else {
        return Err(witness_mismatch());
    };
    if (snapshot_id, objects, requested, emitted)
        != (
            bound_snapshot,
            bound_objects,
            bound_requested,
            bound_emitted,
        )
    {
        return Err(witness_mismatch());
    }
    check_operator(operator, &c.provenance)?;
    let object_count = verify_aggregate_domain(snapshot_id, objects, &c.provenance, resolver)?;
    invalid_if(*requested == object_count && *emitted <= *requested)?;
    let trusted = resolver
        .resolve_aggregate_receipt(snapshot_id)
        .ok_or(VerificationError::MissingTrustedReceipt)?;
    let receipt: AggregateReceipt =
        serde_json::from_slice(trusted).map_err(|_| VerificationError::MalformedTrustedReceipt)?;
    if !aggregate_receipt_matches(
        &receipt,
        snapshot_id,
        objects,
        *requested,
        *emitted,
        result_digest,
    ) {
        return Err(witness_mismatch());
    }
    invalid_if(zero_abi::sha256(c.payload.as_ref()) == *result_digest)
}

fn verify_aggregate_domain<R: Resolver + ?Sized>(
    snapshot_id: &Digest,
    objects: &[ObjectId],
    provenance: &Provenance,
    resolver: &R,
) -> Result<u64, VerificationError> {
    if objects.is_empty()
        || *snapshot_id
            != domain_snapshot_digest(objects, &provenance.index_id, &provenance.index_version)
    {
        return Err(VerificationError::InvalidCompleteness);
    }
    for object_id in objects {
        let _ = resolve_hashed(object_id, resolver)?;
    }
    u64::try_from(objects.len()).map_err(|_| VerificationError::InvalidCompleteness)
}

fn aggregate_receipt_matches(
    receipt: &AggregateReceipt,
    snapshot_id: &Digest,
    objects: &[ObjectId],
    requested: u64,
    emitted: u64,
    result_digest: &Digest,
) -> bool {
    receipt.snapshot_id == *snapshot_id
        && receipt.objects.as_slice() == objects
        && receipt.requested == requested
        && receipt.emitted == emitted
        && receipt.result_digest == *result_digest
}

fn verify_diff(c: &EvidenceCertificate<'_>) -> Result<(), VerificationError> {
    let (
        Query::Diff { old, new },
        CompletenessWitness::Diff {
            operator,
            old: bound_old,
            new: bound_new,
        },
    ) = (&c.query, &c.completeness)
    else {
        return Err(witness_mismatch());
    };
    if old != bound_old || new != bound_new {
        return Err(witness_mismatch());
    }
    check_operator(operator, &c.provenance)?;
    invalid_if(
        c.spans.iter().any(|s| s.object_id == *old) && c.spans.iter().any(|s| s.object_id == *new),
    )
}

fn verify_build_receipt(c: &EvidenceCertificate<'_>) -> Result<(), VerificationError> {
    let (
        Query::BuildReceipt { command },
        CompletenessWitness::BuildReceipt {
            operator,
            command: bound,
            stdout_digest,
            stderr_digest,
            ..
        },
    ) = (&c.query, &c.completeness)
    else {
        return Err(witness_mismatch());
    };
    if command != bound {
        return Err(witness_mismatch());
    }
    check_operator(operator, &c.provenance)?;
    invalid_if(
        c.spans.len() == 2
            && c.spans[0].span_digest == *stdout_digest
            && c.spans[1].span_digest == *stderr_digest,
    )
}

fn verify_test_trace(c: &EvidenceCertificate<'_>) -> Result<(), VerificationError> {
    let (
        Query::TestTrace { test },
        CompletenessWitness::TestTrace {
            operator,
            test: bound,
            trace_digest,
            ..
        },
    ) = (&c.query, &c.completeness)
    else {
        return Err(witness_mismatch());
    };
    if test != bound {
        return Err(witness_mismatch());
    }
    check_operator(operator, &c.provenance)?;
    invalid_if(c.spans.len() == 1 && c.spans[0].span_digest == *trace_digest)
}

fn verify_search<R: Resolver + ?Sized>(
    scope: ObjectId,
    pattern: &[u8],
    scope_len: u64,
    match_count: u64,
    c: &EvidenceCertificate<'_>,
    resolver: &R,
) -> Result<(), VerificationError> {
    let object = resolver
        .resolve(&scope)
        .ok_or(VerificationError::MissingObject { object_id: scope })?;
    if zero_abi::sha256(object) != scope.0 {
        return Err(VerificationError::ObjectIdentityMismatch { span_index: 0 });
    }
    validate_search_domain(pattern, scope_len, match_count, object, c)?;
    validate_search_witness(scope, pattern, object, &c.spans)
}

fn validate_search_domain(
    pattern: &[u8],
    scope_len: u64,
    match_count: u64,
    object: &[u8],
    c: &EvidenceCertificate<'_>,
) -> Result<(), VerificationError> {
    if pattern.is_empty()
        || usize::try_from(scope_len).ok() != Some(object.len())
        || usize::try_from(match_count).ok() != Some(c.spans.len())
    {
        return Err(VerificationError::InvalidCompleteness);
    }
    Ok(())
}

fn validate_search_witness(
    scope: ObjectId,
    pattern: &[u8],
    object: &[u8],
    spans: &[SpanRef],
) -> Result<(), VerificationError> {
    let mut found = 0usize;
    for (offset, window) in object.windows(pattern.len()).enumerate() {
        if window == pattern {
            found = validate_search_match(scope, pattern.len(), offset, found, spans)?;
        }
    }
    invalid_if(found == spans.len())
}

fn validate_search_match(
    scope: ObjectId,
    pattern_len: usize,
    offset: usize,
    found: usize,
    spans: &[SpanRef],
) -> Result<usize, VerificationError> {
    let span = spans
        .get(found)
        .ok_or(VerificationError::InvalidCompleteness)?;
    if span.object_id != scope
        || usize::try_from(span.byte_start).ok() != Some(offset)
        || usize::try_from(span.byte_len).ok() != Some(pattern_len)
    {
        return Err(VerificationError::InvalidCompleteness);
    }
    found
        .checked_add(1)
        .ok_or(VerificationError::InvalidCompleteness)
}
