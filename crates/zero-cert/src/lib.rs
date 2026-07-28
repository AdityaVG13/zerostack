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

use std::{borrow::Cow, fmt};
use serde::{Deserialize, Serialize};

pub use zero_ref::{
    object_identity_hex, Digest, ObjectId, SpanRef, OBJECT_ID_HASH_ALGORITHM,
    OBJECT_ID_HEX_LENGTH,
};
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)] pub struct SymbolId(pub u64);
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)] pub struct NodeId(pub u64);
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)] pub struct CommandId(pub u64);
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)] pub struct TestId(pub u64);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(bound(deserialize = "'de: 'a"))]
pub enum Query<'a> {
    ReadSpan(SpanRef),
    ExactSearch { scope: ObjectId, #[serde(borrow)] pattern: Cow<'a, [u8]> },
    Definition { symbol: SymbolId }, References { symbol: SymbolId },
    AstClosure { seeds: Vec<NodeId>, relations: u64, radius: u32 },
    CallPath { source: SymbolId, target: SymbolId }, DataflowSlice { sink: NodeId },
    Diff { old: ObjectId, new: ObjectId }, BuildReceipt { command: CommandId },
    TestTrace { test: TestId },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Provenance {
    pub parser_id: String, pub parser_version: String,
    pub index_id: String, pub index_version: String,
    pub operator_id: String, pub operator_version: String,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperatorLock { pub operator_id: String, pub operator_version: String }

/// Query-bound proof shapes. Deliberately has no semantic-summary variant.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(bound(deserialize = "'de: 'a"))]
pub enum CompletenessWitness<'a> {
    ReadSpan { operator: OperatorLock },
    ExactSearch { operator: OperatorLock, scope: ObjectId, #[serde(borrow)] pattern: Cow<'a, [u8]>, scope_len: u64, match_count: u64 },
    Definition { operator: OperatorLock, symbol: SymbolId, index_id: String, index_version: String },
    References { operator: OperatorLock, symbol: SymbolId, index_id: String, index_version: String, match_count: u64 },
    AstClosure { operator: OperatorLock, seeds: Vec<NodeId>, relations: u64, radius: u32, parser_id: String, parser_version: String, visited_nodes: u64 },
    CallPath { operator: OperatorLock, source: SymbolId, target: SymbolId, edge_count: u64 },
    DataflowSlice { operator: OperatorLock, sink: NodeId, visited_nodes: u64 },
    Diff { operator: OperatorLock, old: ObjectId, new: ObjectId },
    BuildReceipt { operator: OperatorLock, command: CommandId, exit_code: i32, stdout_digest: Digest, stderr_digest: Digest },
    TestTrace { operator: OperatorLock, test: TestId, exit_code: i32, trace_digest: Digest },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(bound(deserialize = "'de: 'a"))]
pub struct EvidenceCertificate<'a> {
    #[serde(borrow)] pub query: Query<'a>, pub spans: Vec<SpanRef>,
    #[serde(borrow)] pub payload: Cow<'a, [u8]>, pub provenance: Provenance,
    #[serde(borrow)] pub completeness: CompletenessWitness<'a>,
    pub input_token_cost: u64, pub backend_work_units: u64,
}
impl EvidenceCertificate<'_> {
    pub fn canonical_json(&self) -> Result<String, serde_json::Error> { serde_json::to_value(self).map(|v| zero_abi::canonical_json(&v)) }
    pub fn canonical_digest(&self) -> Result<Digest, serde_json::Error> { self.canonical_json().map(|v| zero_abi::sha256(v.as_bytes())) }
}

/// Immutable resident bytes and verifier-owned trusted lock lookup. Implementations must not perform I/O.
pub trait Resolver {
    fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]>;
    fn trusted_operator_version<'a>(&'a self, operator_id: &str) -> Option<&'a str>;
    fn trusted_parser_version<'a>(&'a self, parser_id: &str) -> Option<&'a str>;
    fn trusted_index_version<'a>(&'a self, index_id: &str) -> Option<&'a str>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerificationError {
    MissingObject { object_id: ObjectId }, RangeOverflow { span_index: usize },
    RangeOutsideObject { span_index: usize }, ObjectIdentityMismatch { span_index: usize },
    ObjectDigestMismatch { span_index: usize }, SpanDigestMismatch { span_index: usize },
    PayloadLengthMismatch, PayloadMismatch { span_index: usize }, WitnessQueryMismatch,
    MissingTrustedOperator, MissingTrustedParser, MissingTrustedIndex,
    StaleOperator, StaleParser, StaleIndex, InvalidCompleteness,
}
impl fmt::Display for VerificationError { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{:?}", self) } }
impl std::error::Error for VerificationError {}

#[derive(Debug)]
pub struct VerifiedEvidence<'certificate, 'payload> { certificate: &'certificate EvidenceCertificate<'payload> }
impl<'certificate, 'payload> VerifiedEvidence<'certificate, 'payload> {
    pub fn certificate(&self) -> &'certificate EvidenceCertificate<'payload> { self.certificate }
    pub fn query(&self) -> &'certificate Query<'payload> { &self.certificate.query }
    pub fn spans(&self) -> &'certificate [SpanRef] { &self.certificate.spans }
    pub fn payload(&self) -> &'certificate [u8] { self.certificate.payload.as_ref() }
    pub fn provenance(&self) -> &'certificate Provenance { &self.certificate.provenance }
    pub fn input_token_cost(&self) -> u64 { self.certificate.input_token_cost }
    pub fn backend_work_units(&self) -> u64 { self.certificate.backend_work_units }
}

pub fn verify<'certificate, 'payload, R: Resolver + ?Sized>(certificate: &'certificate EvidenceCertificate<'payload>, resolver: &R) -> Result<VerifiedEvidence<'certificate, 'payload>, VerificationError> {
    check_trusted_provenance(&certificate.provenance, resolver)?;
    verify_spans(certificate, resolver)?;
    verify_completeness(certificate, resolver)?;
    Ok(VerifiedEvidence { certificate })
}

fn verify_spans<R: Resolver + ?Sized>(c: &EvidenceCertificate<'_>, resolver: &R) -> Result<(), VerificationError> {
    let mut payload_offset = 0usize;
    for (index, span) in c.spans.iter().enumerate() {
        payload_offset = verify_span(index, span, c.payload.as_ref(), payload_offset, resolver)?;
    }
    if payload_offset != c.payload.len() { return Err(VerificationError::PayloadLengthMismatch); }
    Ok(())
}

fn verify_span<R: Resolver + ?Sized>(index: usize, span: &SpanRef, payload: &[u8], payload_offset: usize, resolver: &R) -> Result<usize, VerificationError> {
    let object = resolver.resolve(&span.object_id).ok_or(VerificationError::MissingObject { object_id: span.object_id })?;
    let digest = zero_abi::sha256(object);
    if digest != span.object_id.0 { return Err(VerificationError::ObjectIdentityMismatch { span_index: index }); }
    if digest != span.object_digest { return Err(VerificationError::ObjectDigestMismatch { span_index: index }); }
    let start = usize::try_from(span.byte_start).map_err(|_| VerificationError::RangeOutsideObject { span_index: index })?;
    let len = usize::try_from(span.byte_len).map_err(|_| VerificationError::RangeOutsideObject { span_index: index })?;
    let end = start.checked_add(len).ok_or(VerificationError::RangeOverflow { span_index: index })?;
    let bytes = object.get(start..end).ok_or(VerificationError::RangeOutsideObject { span_index: index })?;
    if zero_abi::sha256(bytes) != span.span_digest { return Err(VerificationError::SpanDigestMismatch { span_index: index }); }
    verify_payload_span(index, payload, payload_offset, bytes, len)
}

fn verify_payload_span(index: usize, payload: &[u8], payload_offset: usize, bytes: &[u8], len: usize) -> Result<usize, VerificationError> {
    let payload_end = payload_offset.checked_add(len).ok_or(VerificationError::PayloadLengthMismatch)?;
    if payload.get(payload_offset..payload_end).ok_or(VerificationError::PayloadLengthMismatch)? != bytes { return Err(VerificationError::PayloadMismatch { span_index: index }); }
    Ok(payload_end)
}

fn check_trusted_provenance<R: Resolver + ?Sized>(p: &Provenance, r: &R) -> Result<(), VerificationError> {
    check_trusted_operator(p, r)?;
    check_trusted_parser(p, r)?;
    check_trusted_index(p, r)
}

fn check_trusted_operator<R: Resolver + ?Sized>(p: &Provenance, r: &R) -> Result<(), VerificationError> {
    let version = r.trusted_operator_version(&p.operator_id).ok_or(VerificationError::MissingTrustedOperator)?;
    if version != p.operator_version { return Err(VerificationError::StaleOperator); }
    Ok(())
}

fn check_trusted_parser<R: Resolver + ?Sized>(p: &Provenance, r: &R) -> Result<(), VerificationError> {
    let version = r.trusted_parser_version(&p.parser_id).ok_or(VerificationError::MissingTrustedParser)?;
    if version != p.parser_version { return Err(VerificationError::StaleParser); }
    Ok(())
}

fn check_trusted_index<R: Resolver + ?Sized>(p: &Provenance, r: &R) -> Result<(), VerificationError> {
    let version = r.trusted_index_version(&p.index_id).ok_or(VerificationError::MissingTrustedIndex)?;
    if version != p.index_version { return Err(VerificationError::StaleIndex); }
    Ok(())
}

fn check_operator(lock: &OperatorLock, p: &Provenance) -> Result<(), VerificationError> {
    if lock.operator_id == p.operator_id && lock.operator_version == p.operator_version { Ok(()) } else { Err(VerificationError::StaleOperator) }
}
fn check_parser(id: &str, version: &str, p: &Provenance) -> Result<(), VerificationError> {
    if id == p.parser_id && version == p.parser_version { Ok(()) } else { Err(VerificationError::StaleParser) }
}
fn check_index(id: &str, version: &str, p: &Provenance) -> Result<(), VerificationError> {
    if id == p.index_id && version == p.index_version { Ok(()) } else { Err(VerificationError::StaleIndex) }
}

fn verify_completeness<R: Resolver + ?Sized>(c: &EvidenceCertificate<'_>, resolver: &R) -> Result<(), VerificationError> {
    use Query as Q;
    match &c.query {
        Q::ReadSpan(_) => verify_read_span(c),
        Q::ExactSearch { .. } => verify_search_binding(c, resolver),
        Q::Definition { .. } => verify_definition(c),
        Q::References { .. } => verify_references(c),
        Q::AstClosure { .. } => verify_ast_closure(c),
        _ => verify_graph_or_execution(c),
    }
}

fn verify_graph_or_execution(c: &EvidenceCertificate<'_>) -> Result<(), VerificationError> {
    match &c.query {
        Query::CallPath { .. } => verify_call_path(c),
        Query::DataflowSlice { .. } => verify_dataflow_slice(c),
        Query::Diff { .. } => verify_diff(c),
        Query::BuildReceipt { .. } => verify_build_receipt(c),
        Query::TestTrace { .. } => verify_test_trace(c),
        _ => Err(witness_mismatch()),
    }
}

fn witness_mismatch() -> VerificationError { VerificationError::WitnessQueryMismatch }
fn invalid_if(valid: bool) -> Result<(), VerificationError> {
    if valid { Ok(()) } else { Err(VerificationError::InvalidCompleteness) }
}

fn verify_read_span(c: &EvidenceCertificate<'_>) -> Result<(), VerificationError> {
    let (Query::ReadSpan(wanted), CompletenessWitness::ReadSpan { operator }) = (&c.query, &c.completeness) else { return Err(witness_mismatch()); };
    check_operator(operator, &c.provenance)?;
    invalid_if(c.spans.as_slice() == std::slice::from_ref(wanted))
}

fn verify_search_binding<R: Resolver + ?Sized>(c: &EvidenceCertificate<'_>, resolver: &R) -> Result<(), VerificationError> {
    let (Query::ExactSearch { scope, pattern }, CompletenessWitness::ExactSearch { operator, scope: bound_scope, pattern: bound_pattern, scope_len, match_count }) = (&c.query, &c.completeness) else { return Err(witness_mismatch()); };
    check_operator(operator, &c.provenance)?;
    if scope != bound_scope || pattern.as_ref() != bound_pattern.as_ref() { return Err(witness_mismatch()); }
    verify_search(*scope, pattern, *scope_len, *match_count, c, resolver)
}

fn verify_definition(c: &EvidenceCertificate<'_>) -> Result<(), VerificationError> {
    let (Query::Definition { symbol }, CompletenessWitness::Definition { operator, symbol: bound, index_id, index_version }) = (&c.query, &c.completeness) else { return Err(witness_mismatch()); };
    check_index(index_id, index_version, &c.provenance)?;
    if symbol != bound { return Err(witness_mismatch()); }
    check_operator(operator, &c.provenance)?;
    invalid_if(c.spans.len() == 1)
}

fn verify_references(c: &EvidenceCertificate<'_>) -> Result<(), VerificationError> {
    let (Query::References { symbol }, CompletenessWitness::References { operator, symbol: bound, index_id, index_version, match_count }) = (&c.query, &c.completeness) else { return Err(witness_mismatch()); };
    check_index(index_id, index_version, &c.provenance)?;
    if symbol != bound { return Err(witness_mismatch()); }
    check_operator(operator, &c.provenance)?;
    invalid_if(usize::try_from(*match_count).ok() == Some(c.spans.len()))
}

fn verify_ast_closure(c: &EvidenceCertificate<'_>) -> Result<(), VerificationError> {
    let (Query::AstClosure { seeds, relations, radius }, CompletenessWitness::AstClosure { operator, seeds: bound_seeds, relations: bound_relations, radius: bound_radius, parser_id, parser_version, visited_nodes }) = (&c.query, &c.completeness) else { return Err(witness_mismatch()); };
    check_parser(parser_id, parser_version, &c.provenance)?;
    if seeds != bound_seeds || relations != bound_relations || radius != bound_radius { return Err(witness_mismatch()); }
    check_operator(operator, &c.provenance)?;
    invalid_if(*visited_nodes > 0 && !c.spans.is_empty())
}

fn verify_call_path(c: &EvidenceCertificate<'_>) -> Result<(), VerificationError> {
    let (Query::CallPath { source, target }, CompletenessWitness::CallPath { operator, source: bound_source, target: bound_target, edge_count }) = (&c.query, &c.completeness) else { return Err(witness_mismatch()); };
    if source != bound_source || target != bound_target { return Err(witness_mismatch()); }
    check_operator(operator, &c.provenance)?;
    invalid_if(usize::try_from(*edge_count).ok().and_then(|n| n.checked_add(1)) == Some(c.spans.len()))
}

fn verify_dataflow_slice(c: &EvidenceCertificate<'_>) -> Result<(), VerificationError> {
    let (Query::DataflowSlice { sink }, CompletenessWitness::DataflowSlice { operator, sink: bound, visited_nodes }) = (&c.query, &c.completeness) else { return Err(witness_mismatch()); };
    if sink != bound { return Err(witness_mismatch()); }
    check_operator(operator, &c.provenance)?;
    invalid_if(*visited_nodes > 0 && !c.spans.is_empty())
}

fn verify_diff(c: &EvidenceCertificate<'_>) -> Result<(), VerificationError> {
    let (Query::Diff { old, new }, CompletenessWitness::Diff { operator, old: bound_old, new: bound_new }) = (&c.query, &c.completeness) else { return Err(witness_mismatch()); };
    if old != bound_old || new != bound_new { return Err(witness_mismatch()); }
    check_operator(operator, &c.provenance)?;
    invalid_if(c.spans.iter().any(|s| s.object_id == *old) && c.spans.iter().any(|s| s.object_id == *new))
}

fn verify_build_receipt(c: &EvidenceCertificate<'_>) -> Result<(), VerificationError> {
    let (Query::BuildReceipt { command }, CompletenessWitness::BuildReceipt { operator, command: bound, stdout_digest, stderr_digest, .. }) = (&c.query, &c.completeness) else { return Err(witness_mismatch()); };
    if command != bound { return Err(witness_mismatch()); }
    check_operator(operator, &c.provenance)?;
    invalid_if(c.spans.len() == 2 && c.spans[0].span_digest == *stdout_digest && c.spans[1].span_digest == *stderr_digest)
}

fn verify_test_trace(c: &EvidenceCertificate<'_>) -> Result<(), VerificationError> {
    let (Query::TestTrace { test }, CompletenessWitness::TestTrace { operator, test: bound, trace_digest, .. }) = (&c.query, &c.completeness) else { return Err(witness_mismatch()); };
    if test != bound { return Err(witness_mismatch()); }
    check_operator(operator, &c.provenance)?;
    invalid_if(c.spans.len() == 1 && c.spans[0].span_digest == *trace_digest)
}

fn verify_search<R: Resolver + ?Sized>(scope: ObjectId, pattern: &[u8], scope_len: u64, match_count: u64, c: &EvidenceCertificate<'_>, resolver: &R) -> Result<(), VerificationError> {
    let object = resolver.resolve(&scope).ok_or(VerificationError::MissingObject { object_id: scope })?;
    if zero_abi::sha256(object) != scope.0 { return Err(VerificationError::ObjectIdentityMismatch { span_index: 0 }); }
    validate_search_domain(pattern, scope_len, match_count, object, c)?;
    validate_search_witness(scope, pattern, object, &c.spans)
}

fn validate_search_domain(pattern: &[u8], scope_len: u64, match_count: u64, object: &[u8], c: &EvidenceCertificate<'_>) -> Result<(), VerificationError> {
    if pattern.is_empty() || usize::try_from(scope_len).ok() != Some(object.len()) || usize::try_from(match_count).ok() != Some(c.spans.len()) { return Err(VerificationError::InvalidCompleteness); }
    Ok(())
}

fn validate_search_witness(scope: ObjectId, pattern: &[u8], object: &[u8], spans: &[SpanRef]) -> Result<(), VerificationError> {
    let mut found = 0usize;
    for (offset, window) in object.windows(pattern.len()).enumerate() {
        if window == pattern { found = validate_search_match(scope, pattern.len(), offset, found, spans)?; }
    }
    invalid_if(found == spans.len())
}

fn validate_search_match(scope: ObjectId, pattern_len: usize, offset: usize, found: usize, spans: &[SpanRef]) -> Result<usize, VerificationError> {
    let span = spans.get(found).ok_or(VerificationError::InvalidCompleteness)?;
    if span.object_id != scope || usize::try_from(span.byte_start).ok() != Some(offset) || usize::try_from(span.byte_len).ok() != Some(pattern_len) { return Err(VerificationError::InvalidCompleteness); }
    found.checked_add(1).ok_or(VerificationError::InvalidCompleteness)
}
