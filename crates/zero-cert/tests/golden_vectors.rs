mod common;
use std::borrow::Cow;
use common::fixture;
use zero_cert::*;

const CANONICAL_JSON: &str = r#"{"backend_work_units":1,"completeness":{"ReadSpan":{"operator":{"operator_id":"read-span","operator_version":"1"}}},"input_token_cost":3,"payload":[114,101,115,105,100,101,110,116,32,101,118,105,100,101,110,99,101],"provenance":{"index_id":"zero-index","index_version":"2","operator_id":"read-span","operator_version":"1","parser_id":"tree-sitter","parser_version":"1"},"query":{"ReadSpan":{"byte_len":17,"byte_start":0,"object_digest":[250,48,218,145,149,238,121,111,110,91,147,135,83,191,106,119,227,209,199,158,235,195,25,172,207,137,185,0,74,201,3,117],"object_id":[250,48,218,145,149,238,121,111,110,91,147,135,83,191,106,119,227,209,199,158,235,195,25,172,207,137,185,0,74,201,3,117],"span_digest":[250,48,218,145,149,238,121,111,110,91,147,135,83,191,106,119,227,209,199,158,235,195,25,172,207,137,185,0,74,201,3,117]}},"spans":[{"byte_len":17,"byte_start":0,"object_digest":[250,48,218,145,149,238,121,111,110,91,147,135,83,191,106,119,227,209,199,158,235,195,25,172,207,137,185,0,74,201,3,117],"object_id":[250,48,218,145,149,238,121,111,110,91,147,135,83,191,106,119,227,209,199,158,235,195,25,172,207,137,185,0,74,201,3,117],"span_digest":[250,48,218,145,149,238,121,111,110,91,147,135,83,191,106,119,227,209,199,158,235,195,25,172,207,137,185,0,74,201,3,117]}]}"#;
const CANONICAL_DIGEST: [u8; 32] = [192, 22, 233, 92, 240, 248, 94, 90, 108, 232, 20, 107, 61, 38, 87, 10, 246, 173, 241, 37, 138, 220, 61, 244, 50, 45, 17, 48, 64, 203, 226, 60];

#[test]
fn golden_valid_certificate_and_canonical_vectors_are_stable() {
    let (certificate, resident) = fixture(b"resident evidence");
    assert_eq!(verify(&certificate, &resident).unwrap().payload(), b"resident evidence");
    assert_eq!(certificate.canonical_json().unwrap(), CANONICAL_JSON);
    assert_eq!(certificate.canonical_digest().unwrap(), CANONICAL_DIGEST);
}

#[test]
fn zero_ref_object_identity_convention_is_portable_sha256() {
    assert_eq!(OBJECT_ID_HASH_ALGORITHM, "sha256");
    assert_eq!(OBJECT_ID_HEX_LENGTH, 64);
    assert_eq!(object_identity_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(ObjectId(zero_abi::sha256(b"abc")).0, zero_abi::sha256(b"abc"));
}

#[test]
fn rejects_payload_range_digest_object_and_witness_tampering() {
    let (base, resident) = fixture(b"resident evidence");
    let mut payload = base.clone(); payload.payload = Cow::Owned(b"Resident evidence".to_vec());
    assert!(matches!(verify(&payload, &resident), Err(VerificationError::PayloadMismatch { .. })));
    let mut range = base.clone(); range.spans[0].byte_len = u64::MAX;
    assert!(matches!(verify(&range, &resident), Err(VerificationError::RangeOutsideObject { .. }) | Err(VerificationError::RangeOverflow { .. })));
    let mut digest = base.clone(); digest.spans[0].span_digest[0] ^= 1;
    assert!(matches!(verify(&digest, &resident), Err(VerificationError::SpanDigestMismatch { .. })));
    let mut object = base.clone(); object.spans[0].object_digest[0] ^= 1;
    assert!(matches!(verify(&object, &resident), Err(VerificationError::ObjectDigestMismatch { .. })));
    let mut witness = base.clone(); witness.completeness = CompletenessWitness::Diff { operator: lock(), old: ObjectId([0; 32]), new: ObjectId([1; 32]) };
    assert_eq!(verify(&witness, &resident).unwrap_err(), VerificationError::WitnessQueryMismatch);
}

#[test]
fn trusted_locks_reject_missing_and_stale_operator_parser_and_index() {
    let (certificate, mut resident) = fixture(b"resident evidence");
    resident.operator = None; assert_eq!(verify(&certificate, &resident).unwrap_err(), VerificationError::MissingTrustedOperator);
    resident.operator = Some("0"); assert_eq!(verify(&certificate, &resident).unwrap_err(), VerificationError::StaleOperator);
    resident.operator = Some("1"); resident.parser = None; assert_eq!(verify(&certificate, &resident).unwrap_err(), VerificationError::MissingTrustedParser);
    resident.parser = Some("0"); assert_eq!(verify(&certificate, &resident).unwrap_err(), VerificationError::StaleParser);
    resident.parser = Some("1"); resident.index = None; assert_eq!(verify(&certificate, &resident).unwrap_err(), VerificationError::MissingTrustedIndex);
    resident.index = Some("0"); assert_eq!(verify(&certificate, &resident).unwrap_err(), VerificationError::StaleIndex);
    resident.index = Some("2");
    let mut stale_witness = certificate.clone(); stale_witness.completeness = CompletenessWitness::ReadSpan { operator: OperatorLock { operator_id: "read-span".into(), operator_version: "0".into() } };
    assert_eq!(verify(&stale_witness, &resident).unwrap_err(), VerificationError::StaleOperator);
    let mut stale_parser = certificate.clone();
    stale_parser.query = Query::AstClosure { seeds: vec![NodeId(1)], relations: 1, radius: 1 };
    stale_parser.completeness = CompletenessWitness::AstClosure { operator: lock(), seeds: vec![NodeId(1)], relations: 1, radius: 1, parser_id: "tree-sitter".into(), parser_version: "0".into(), visited_nodes: 1 };
    assert_eq!(verify(&stale_parser, &resident).unwrap_err(), VerificationError::StaleParser);
    let mut stale_index = certificate.clone();
    stale_index.query = Query::Definition { symbol: SymbolId(1) };
    stale_index.completeness = CompletenessWitness::Definition { operator: lock(), symbol: SymbolId(1), index_id: "zero-index".into(), index_version: "0".into() };
    assert_eq!(verify(&stale_index, &resident).unwrap_err(), VerificationError::StaleIndex);
}

fn lock() -> OperatorLock { OperatorLock { operator_id: "read-span".into(), operator_version: "1".into() } }
fn rejects_bound_substitution(query: Query<'static>, witness: CompletenessWitness<'static>) {
    let (mut certificate, resident) = fixture(b"resident evidence");
    certificate.query = query; certificate.completeness = witness;
    assert!(matches!(verify(&certificate, &resident), Err(VerificationError::InvalidCompleteness) | Err(VerificationError::WitnessQueryMismatch)));
}

#[test]
fn rejects_cross_query_parameter_substitution_for_every_witness() {
    let z = ObjectId([0; 32]); let o = ObjectId([1; 32]);
    let mut other_span = fixture(b"resident evidence").0.spans[0].clone(); other_span.byte_len -= 1;
    rejects_bound_substitution(Query::ReadSpan(other_span), CompletenessWitness::ReadSpan { operator: lock() });
    rejects_bound_substitution(Query::ExactSearch { scope: z, pattern: Cow::Borrowed(b"a") }, CompletenessWitness::ExactSearch { operator: lock(), scope: o, pattern: Cow::Borrowed(b"a"), scope_len: 1, match_count: 0 });
    rejects_bound_substitution(Query::Definition { symbol: SymbolId(1) }, CompletenessWitness::Definition { operator: lock(), symbol: SymbolId(2), index_id: "zero-index".into(), index_version: "2".into() });
    rejects_bound_substitution(Query::References { symbol: SymbolId(1) }, CompletenessWitness::References { operator: lock(), symbol: SymbolId(2), index_id: "zero-index".into(), index_version: "2".into(), match_count: 1 });
    rejects_bound_substitution(Query::AstClosure { seeds: vec![NodeId(1)], relations: 3, radius: 2 }, CompletenessWitness::AstClosure { operator: lock(), seeds: vec![NodeId(2)], relations: 3, radius: 2, parser_id: "tree-sitter".into(), parser_version: "1".into(), visited_nodes: 1 });
    rejects_bound_substitution(Query::CallPath { source: SymbolId(1), target: SymbolId(2) }, CompletenessWitness::CallPath { operator: lock(), source: SymbolId(9), target: SymbolId(2), edge_count: 0 });
    rejects_bound_substitution(Query::DataflowSlice { sink: NodeId(1) }, CompletenessWitness::DataflowSlice { operator: lock(), sink: NodeId(2), visited_nodes: 1 });
    rejects_bound_substitution(Query::Diff { old: z, new: o }, CompletenessWitness::Diff { operator: lock(), old: o, new: z });
    rejects_bound_substitution(Query::BuildReceipt { command: CommandId(1) }, CompletenessWitness::BuildReceipt { operator: lock(), command: CommandId(2), exit_code: 0, stdout_digest: z.0, stderr_digest: o.0 });
    rejects_bound_substitution(Query::TestTrace { test: TestId(1) }, CompletenessWitness::TestTrace { operator: lock(), test: TestId(2), exit_code: 0, trace_digest: z.0 });
}

#[test]
fn success_path_allocates_nothing() {
    let (certificate, resident) = fixture(b"resident evidence");
    let allocation = allocation_counter::measure(|| assert!(verify(&certificate, &resident).is_ok()));
    assert_eq!(allocation.count_total, 0, "verification allocated: {allocation:?}");
}
