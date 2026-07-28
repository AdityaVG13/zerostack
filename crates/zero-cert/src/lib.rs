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

pub type Digest = [u8; 32];
/// Object identities are portable, lowercase SHA-256 hex when rendered.
pub const OBJECT_ID_HASH_ALGORITHM: &str = "sha256";
pub const OBJECT_ID_HEX_LENGTH: usize = 64;
/// Non-hot-path portable rendering of the object identity convention.
pub fn object_identity_hex(bytes: &[u8]) -> String { zero_ref::content_hash_hex(bytes) }

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)] pub struct ObjectId(pub Digest);
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)] pub struct SymbolId(pub u64);
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)] pub struct NodeId(pub u64);
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)] pub struct CommandId(pub u64);
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)] pub struct TestId(pub u64);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpanRef {
    pub object_id: ObjectId, pub byte_start: u64, pub byte_len: u64,
    pub object_digest: Digest, pub span_digest: Digest,
}

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

#[derive(Clone, Copy, Debug)]
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
    let mut payload_offset = 0usize;
    for (index, span) in certificate.spans.iter().enumerate() {
        let object = resolver.resolve(&span.object_id).ok_or(VerificationError::MissingObject { object_id: span.object_id })?;
        let digest = zero_abi::sha256(object);
        if digest != span.object_id.0 { return Err(VerificationError::ObjectIdentityMismatch { span_index: index }); }
        if digest != span.object_digest { return Err(VerificationError::ObjectDigestMismatch { span_index: index }); }
        let start = usize::try_from(span.byte_start).map_err(|_| VerificationError::RangeOutsideObject { span_index: index })?;
        let len = usize::try_from(span.byte_len).map_err(|_| VerificationError::RangeOutsideObject { span_index: index })?;
        let end = start.checked_add(len).ok_or(VerificationError::RangeOverflow { span_index: index })?;
        let bytes = object.get(start..end).ok_or(VerificationError::RangeOutsideObject { span_index: index })?;
        if zero_abi::sha256(bytes) != span.span_digest { return Err(VerificationError::SpanDigestMismatch { span_index: index }); }
        let payload_end = payload_offset.checked_add(len).ok_or(VerificationError::PayloadLengthMismatch)?;
        if certificate.payload.get(payload_offset..payload_end).ok_or(VerificationError::PayloadLengthMismatch)? != bytes { return Err(VerificationError::PayloadMismatch { span_index: index }); }
        payload_offset = payload_end;
    }
    if payload_offset != certificate.payload.len() { return Err(VerificationError::PayloadLengthMismatch); }
    verify_completeness(certificate, resolver)?;
    Ok(VerifiedEvidence { certificate })
}

fn check_trusted_provenance<R: Resolver + ?Sized>(p: &Provenance, r: &R) -> Result<(), VerificationError> {
    let operator = r.trusted_operator_version(&p.operator_id).ok_or(VerificationError::MissingTrustedOperator)?;
    if operator != p.operator_version { return Err(VerificationError::StaleOperator); }
    let parser = r.trusted_parser_version(&p.parser_id).ok_or(VerificationError::MissingTrustedParser)?;
    if parser != p.parser_version { return Err(VerificationError::StaleParser); }
    let index = r.trusted_index_version(&p.index_id).ok_or(VerificationError::MissingTrustedIndex)?;
    if index != p.index_version { return Err(VerificationError::StaleIndex); }
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
    use {CompletenessWitness as W, Query as Q};
    let (lock, valid) = match (&c.query, &c.completeness) {
        (Q::ReadSpan(wanted), W::ReadSpan { operator }) => (operator, c.spans.as_slice() == std::slice::from_ref(wanted)),
        (Q::ExactSearch { scope, pattern }, W::ExactSearch { operator, scope: bound_scope, pattern: bound_pattern, scope_len, match_count }) => {
            check_operator(operator, &c.provenance)?;
            if scope != bound_scope || pattern.as_ref() != bound_pattern.as_ref() { return Err(VerificationError::WitnessQueryMismatch); }
            return verify_search(*scope, pattern, *scope_len, *match_count, c, resolver);
        }
        (Q::Definition { symbol }, W::Definition { operator, symbol: bound, index_id, index_version }) => { check_index(index_id, index_version, &c.provenance)?; if symbol != bound { return Err(VerificationError::WitnessQueryMismatch); } (operator, c.spans.len() == 1) }
        (Q::References { symbol }, W::References { operator, symbol: bound, index_id, index_version, match_count }) => { check_index(index_id, index_version, &c.provenance)?; if symbol != bound { return Err(VerificationError::WitnessQueryMismatch); } (operator, usize::try_from(*match_count).ok() == Some(c.spans.len())) }
        (Q::AstClosure { seeds, relations, radius }, W::AstClosure { operator, seeds: bound_seeds, relations: bound_relations, radius: bound_radius, parser_id, parser_version, visited_nodes }) => { check_parser(parser_id, parser_version, &c.provenance)?; if seeds != bound_seeds || relations != bound_relations || radius != bound_radius { return Err(VerificationError::WitnessQueryMismatch); } (operator, *visited_nodes > 0 && !c.spans.is_empty()) }
        (Q::CallPath { source, target }, W::CallPath { operator, source: bound_source, target: bound_target, edge_count }) => { if source != bound_source || target != bound_target { return Err(VerificationError::WitnessQueryMismatch); } (operator, usize::try_from(*edge_count).ok().and_then(|n| n.checked_add(1)) == Some(c.spans.len())) },
        (Q::DataflowSlice { sink }, W::DataflowSlice { operator, sink: bound, visited_nodes }) => { if sink != bound { return Err(VerificationError::WitnessQueryMismatch); } (operator, *visited_nodes > 0 && !c.spans.is_empty()) },
        (Q::Diff { old, new }, W::Diff { operator, old: bound_old, new: bound_new }) => { if old != bound_old || new != bound_new { return Err(VerificationError::WitnessQueryMismatch); } (operator, c.spans.iter().any(|s| s.object_id == *old) && c.spans.iter().any(|s| s.object_id == *new)) },
        (Q::BuildReceipt { command }, W::BuildReceipt { operator, command: bound, stdout_digest, stderr_digest, .. }) => { if command != bound { return Err(VerificationError::WitnessQueryMismatch); } (operator, c.spans.len() == 2 && c.spans[0].span_digest == *stdout_digest && c.spans[1].span_digest == *stderr_digest) },
        (Q::TestTrace { test }, W::TestTrace { operator, test: bound, trace_digest, .. }) => { if test != bound { return Err(VerificationError::WitnessQueryMismatch); } (operator, c.spans.len() == 1 && c.spans[0].span_digest == *trace_digest) },
        _ => return Err(VerificationError::WitnessQueryMismatch),
    };
    check_operator(lock, &c.provenance)?;
    if valid { Ok(()) } else { Err(VerificationError::InvalidCompleteness) }
}
fn verify_search<R: Resolver + ?Sized>(scope: ObjectId, pattern: &[u8], scope_len: u64, match_count: u64, c: &EvidenceCertificate<'_>, resolver: &R) -> Result<(), VerificationError> {
    let object = resolver.resolve(&scope).ok_or(VerificationError::MissingObject { object_id: scope })?;
    if zero_abi::sha256(object) != scope.0 { return Err(VerificationError::ObjectIdentityMismatch { span_index: 0 }); }
    if pattern.is_empty() || usize::try_from(scope_len).ok() != Some(object.len()) || usize::try_from(match_count).ok() != Some(c.spans.len()) { return Err(VerificationError::InvalidCompleteness); }
    let mut found = 0usize;
    for (offset, window) in object.windows(pattern.len()).enumerate() {
        if window == pattern {
            let span = c.spans.get(found).ok_or(VerificationError::InvalidCompleteness)?;
            if span.object_id != scope || usize::try_from(span.byte_start).ok() != Some(offset) || usize::try_from(span.byte_len).ok() != Some(pattern.len()) { return Err(VerificationError::InvalidCompleteness); }
            found = found.checked_add(1).ok_or(VerificationError::InvalidCompleteness)?;
        }
    }
    if found == c.spans.len() { Ok(()) } else { Err(VerificationError::InvalidCompleteness) }
}
