mod common;
use common::fixture;
use std::borrow::Cow;
use zero_cert::*;

const CANONICAL_JSON: &str = r#"{"backend_work_units":1,"completeness":{"ReadSpan":{"operator":{"operator_id":"read-span","operator_version":"1"}}},"input_token_cost":3,"payload":[114,101,115,105,100,101,110,116,32,101,118,105,100,101,110,99,101],"provenance":{"index_id":"zero-index","index_version":"2","operator_id":"read-span","operator_version":"1","parser_id":"tree-sitter","parser_version":"1"},"query":{"ReadSpan":{"byte_len":17,"byte_start":0,"object_digest":[250,48,218,145,149,238,121,111,110,91,147,135,83,191,106,119,227,209,199,158,235,195,25,172,207,137,185,0,74,201,3,117],"object_id":[250,48,218,145,149,238,121,111,110,91,147,135,83,191,106,119,227,209,199,158,235,195,25,172,207,137,185,0,74,201,3,117],"span_digest":[250,48,218,145,149,238,121,111,110,91,147,135,83,191,106,119,227,209,199,158,235,195,25,172,207,137,185,0,74,201,3,117]}},"spans":[{"byte_len":17,"byte_start":0,"object_digest":[250,48,218,145,149,238,121,111,110,91,147,135,83,191,106,119,227,209,199,158,235,195,25,172,207,137,185,0,74,201,3,117],"object_id":[250,48,218,145,149,238,121,111,110,91,147,135,83,191,106,119,227,209,199,158,235,195,25,172,207,137,185,0,74,201,3,117],"span_digest":[250,48,218,145,149,238,121,111,110,91,147,135,83,191,106,119,227,209,199,158,235,195,25,172,207,137,185,0,74,201,3,117]}]}"#;
const CANONICAL_DIGEST: [u8; 32] = [
    192, 22, 233, 92, 240, 248, 94, 90, 108, 232, 20, 107, 61, 38, 87, 10, 246, 173, 241, 37, 138,
    220, 61, 244, 50, 45, 17, 48, 64, 203, 226, 60,
];

#[test]
fn golden_valid_certificate_and_canonical_vectors_are_stable() {
    let (certificate, resident) = fixture(b"resident evidence");
    assert_eq!(
        verify(&certificate, &resident).unwrap().payload(),
        b"resident evidence"
    );
    assert_eq!(certificate.canonical_json().unwrap(), CANONICAL_JSON);
    assert_eq!(certificate.canonical_digest().unwrap(), CANONICAL_DIGEST);
}

#[test]
fn zero_ref_object_identity_convention_is_portable_sha256() {
    assert_eq!(OBJECT_ID_HASH_ALGORITHM, "sha256");
    assert_eq!(OBJECT_ID_HEX_LENGTH, 64);
    assert_eq!(
        object_identity_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        ObjectId(zero_abi::sha256(b"abc")).0,
        zero_abi::sha256(b"abc")
    );
}

#[test]
fn rejects_payload_range_digest_object_and_witness_tampering() {
    let (base, resident) = fixture(b"resident evidence");
    let mut payload = base.clone();
    payload.payload = Cow::Owned(b"Resident evidence".to_vec());
    assert!(matches!(
        verify(&payload, &resident),
        Err(VerificationError::PayloadMismatch { .. })
    ));
    let mut range = base.clone();
    range.spans[0].byte_len = u64::MAX;
    assert!(matches!(
        verify(&range, &resident),
        Err(VerificationError::RangeOutsideObject { .. })
            | Err(VerificationError::RangeOverflow { .. })
    ));
    let mut digest = base.clone();
    digest.spans[0].span_digest[0] ^= 1;
    assert!(matches!(
        verify(&digest, &resident),
        Err(VerificationError::SpanDigestMismatch { .. })
    ));
    let mut object = base.clone();
    object.spans[0].object_digest[0] ^= 1;
    assert!(matches!(
        verify(&object, &resident),
        Err(VerificationError::ObjectDigestMismatch { .. })
    ));
    let mut witness = base.clone();
    witness.completeness = CompletenessWitness::Diff {
        operator: lock(),
        old: ObjectId([0; 32]),
        new: ObjectId([1; 32]),
    };
    assert_eq!(
        verify(&witness, &resident).unwrap_err(),
        VerificationError::WitnessQueryMismatch
    );
}

#[test]
fn trusted_locks_reject_missing_and_stale_operator_parser_and_index() {
    let (certificate, mut resident) = fixture(b"resident evidence");
    resident.operator = None;
    assert_eq!(
        verify(&certificate, &resident).unwrap_err(),
        VerificationError::MissingTrustedOperator
    );
    resident.operator = Some("0");
    assert_eq!(
        verify(&certificate, &resident).unwrap_err(),
        VerificationError::StaleOperator
    );
    resident.operator = Some("1");
    resident.parser = None;
    assert_eq!(
        verify(&certificate, &resident).unwrap_err(),
        VerificationError::MissingTrustedParser
    );
    resident.parser = Some("0");
    assert_eq!(
        verify(&certificate, &resident).unwrap_err(),
        VerificationError::StaleParser
    );
    resident.parser = Some("1");
    resident.index = None;
    assert_eq!(
        verify(&certificate, &resident).unwrap_err(),
        VerificationError::MissingTrustedIndex
    );
    resident.index = Some("0");
    assert_eq!(
        verify(&certificate, &resident).unwrap_err(),
        VerificationError::StaleIndex
    );
    resident.index = Some("2");
    let mut stale_witness = certificate.clone();
    stale_witness.completeness = CompletenessWitness::ReadSpan {
        operator: OperatorLock {
            operator_id: "read-span".into(),
            operator_version: "0".into(),
        },
    };
    assert_eq!(
        verify(&stale_witness, &resident).unwrap_err(),
        VerificationError::StaleOperator
    );
    let mut stale_parser = certificate.clone();
    stale_parser.query = Query::AstClosure {
        seeds: vec![NodeId(1)],
        relations: 1,
        radius: 1,
    };
    stale_parser.completeness = CompletenessWitness::AstClosure {
        operator: lock(),
        seeds: vec![NodeId(1)],
        relations: 1,
        radius: 1,
        parser_id: "tree-sitter".into(),
        parser_version: "0".into(),
        visited_nodes: 1,
    };
    assert_eq!(
        verify(&stale_parser, &resident).unwrap_err(),
        VerificationError::StaleParser
    );
    let mut stale_index = certificate.clone();
    stale_index.query = Query::Definition {
        symbol: SymbolId(1),
    };
    stale_index.completeness = CompletenessWitness::Definition {
        operator: lock(),
        symbol: SymbolId(1),
        index_id: "zero-index".into(),
        index_version: "0".into(),
    };
    assert_eq!(
        verify(&stale_index, &resident).unwrap_err(),
        VerificationError::StaleIndex
    );
}

fn lock() -> OperatorLock {
    OperatorLock {
        operator_id: "read-span".into(),
        operator_version: "1".into(),
    }
}
fn rejects_bound_substitution(query: Query<'static>, witness: CompletenessWitness<'static>) {
    let (mut certificate, resident) = fixture(b"resident evidence");
    certificate.query = query;
    certificate.completeness = witness;
    assert!(matches!(
        verify(&certificate, &resident),
        Err(VerificationError::InvalidCompleteness) | Err(VerificationError::WitnessQueryMismatch)
    ));
}

#[test]
fn rejects_cross_query_parameter_substitution_for_every_witness() {
    let z = ObjectId([0; 32]);
    let o = ObjectId([1; 32]);
    let mut other_span = fixture(b"resident evidence").0.spans[0].clone();
    other_span.byte_len -= 1;
    rejects_bound_substitution(
        Query::ReadSpan(other_span),
        CompletenessWitness::ReadSpan { operator: lock() },
    );
    rejects_bound_substitution(
        Query::ExactSearch {
            scope: z,
            pattern: Cow::Borrowed(b"a"),
        },
        CompletenessWitness::ExactSearch {
            operator: lock(),
            scope: o,
            pattern: Cow::Borrowed(b"a"),
            scope_len: 1,
            match_count: 0,
        },
    );
    rejects_bound_substitution(
        Query::Definition {
            symbol: SymbolId(1),
        },
        CompletenessWitness::Definition {
            operator: lock(),
            symbol: SymbolId(2),
            index_id: "zero-index".into(),
            index_version: "2".into(),
        },
    );
    rejects_bound_substitution(
        Query::References {
            symbol: SymbolId(1),
        },
        CompletenessWitness::References {
            operator: lock(),
            symbol: SymbolId(2),
            index_id: "zero-index".into(),
            index_version: "2".into(),
            match_count: 1,
        },
    );
    rejects_bound_substitution(
        Query::AstClosure {
            seeds: vec![NodeId(1)],
            relations: 3,
            radius: 2,
        },
        CompletenessWitness::AstClosure {
            operator: lock(),
            seeds: vec![NodeId(2)],
            relations: 3,
            radius: 2,
            parser_id: "tree-sitter".into(),
            parser_version: "1".into(),
            visited_nodes: 1,
        },
    );
    rejects_bound_substitution(
        Query::CallPath {
            source: SymbolId(1),
            target: SymbolId(2),
        },
        CompletenessWitness::CallPath {
            operator: lock(),
            source: SymbolId(9),
            target: SymbolId(2),
            edge_count: 0,
        },
    );
    rejects_bound_substitution(
        Query::DataflowSlice { sink: NodeId(1) },
        CompletenessWitness::DataflowSlice {
            operator: lock(),
            sink: NodeId(2),
            visited_nodes: 1,
        },
    );
    rejects_bound_substitution(
        Query::Diff { old: z, new: o },
        CompletenessWitness::Diff {
            operator: lock(),
            old: o,
            new: z,
        },
    );
    rejects_bound_substitution(
        Query::BuildReceipt {
            command: CommandId(1),
        },
        CompletenessWitness::BuildReceipt {
            operator: lock(),
            command: CommandId(2),
            exit_code: 0,
            stdout_digest: z.0,
            stderr_digest: o.0,
        },
    );
    rejects_bound_substitution(
        Query::TestTrace { test: TestId(1) },
        CompletenessWitness::TestTrace {
            operator: lock(),
            test: TestId(2),
            exit_code: 0,
            trace_digest: z.0,
        },
    );
}

#[test]
fn success_path_allocates_nothing() {
    let (certificate, resident) = fixture(b"resident evidence");
    let allocation =
        allocation_counter::measure(|| assert!(verify(&certificate, &resident).is_ok()));
    assert_eq!(
        allocation.count_total, 0,
        "verification allocated: {allocation:?}"
    );
}

fn frontier_certificate(
    query: Query<'static>,
    completeness: CompletenessWitness<'static>,
    spans: Vec<SpanRef>,
    payload: Vec<u8>,
) -> EvidenceCertificate<'static> {
    EvidenceCertificate {
        query,
        spans,
        payload: Cow::Owned(payload),
        provenance: common::provenance(),
        completeness,
        input_token_cost: 0,
        backend_work_units: 0,
    }
}

fn canonical_receipt<T: serde::Serialize>(receipt: &T) -> Vec<u8> {
    zero_abi::canonical_json(&serde_json::to_value(receipt).unwrap()).into_bytes()
}

#[test]
fn exact_search_domain_zero_hit_is_complete_only_for_the_bound_domain() {
    let first = b"alpha".as_slice();
    let second = b"beta".as_slice();
    let objects = vec![common::object_id(first), common::object_id(second)];
    let snapshot = domain_snapshot_digest(&objects, "zero-index", "2");
    let query = Query::ExactSearchDomain {
        pattern: Cow::Borrowed(b"z"),
        objects: objects.clone(),
        snapshot_id: snapshot,
        index_id: "zero-index".into(),
        index_version: "2".into(),
    };
    let witness = CompletenessWitness::ExactSearchDomain {
        operator: lock(),
        pattern: Cow::Borrowed(b"z"),
        objects: objects.clone(),
        snapshot_id: snapshot,
        index_id: "zero-index".into(),
        index_version: "2".into(),
        match_count: 0,
    };
    let certificate = frontier_certificate(query, witness, vec![], vec![]);
    let residents = common::Residents {
        objects: vec![first, second],
        mutation_receipts: vec![],
        aggregate_receipts: vec![],
    };
    assert!(verify(&certificate, &residents).is_ok());

    let mut omitted = certificate.clone();
    if let CompletenessWitness::ExactSearchDomain { objects, .. } = &mut omitted.completeness {
        objects.pop();
    }
    assert!(matches!(
        verify(&omitted, &residents),
        Err(VerificationError::WitnessQueryMismatch)
    ));
    let mut reordered = certificate.clone();
    if let CompletenessWitness::ExactSearchDomain { objects, .. } = &mut reordered.completeness {
        objects.reverse();
    }
    assert!(matches!(
        verify(&reordered, &residents),
        Err(VerificationError::WitnessQueryMismatch)
    ));
    let mut stale_snapshot = certificate.clone();
    if let Query::ExactSearchDomain { snapshot_id, .. } = &mut stale_snapshot.query {
        snapshot_id[0] ^= 1;
    }
    if let CompletenessWitness::ExactSearchDomain { snapshot_id, .. } =
        &mut stale_snapshot.completeness
    {
        snapshot_id[0] ^= 1;
    }
    assert!(matches!(
        verify(&stale_snapshot, &residents),
        Err(VerificationError::InvalidCompleteness)
    ));
    let mut stale_index = certificate.clone();
    if let Query::ExactSearchDomain { index_version, .. } = &mut stale_index.query {
        *index_version = "1".into();
    }
    if let CompletenessWitness::ExactSearchDomain { index_version, .. } =
        &mut stale_index.completeness
    {
        *index_version = "1".into();
    }
    assert!(matches!(
        verify(&stale_index, &residents),
        Err(VerificationError::StaleIndex)
    ));
    let incomplete_residents = common::Residents {
        objects: vec![first],
        mutation_receipts: vec![],
        aggregate_receipts: vec![],
    };
    assert!(matches!(
        verify(&certificate, &incomplete_residents),
        Err(VerificationError::MissingObject { .. })
    ));
}

#[test]
fn byte_exact_diff_binds_ranges_replacement_and_span() {
    let old = b"abcXYZtail".as_slice();
    let new = b"abcQtail".as_slice();
    let old_id = common::object_id(old);
    let new_id = common::object_id(new);
    let query = Query::ByteExactDiff {
        old: old_id,
        new: new_id,
        start: 3,
        before_end: 6,
        after_end: 4,
    };
    let witness = CompletenessWitness::ByteExactDiff {
        operator: lock(),
        old: old_id,
        new: new_id,
        start: 3,
        before_end: 6,
        after_end: 4,
        replacement_digest: zero_abi::sha256(b"Q"),
    };
    let certificate =
        frontier_certificate(query, witness, vec![common::span(new, 3, 1)], b"Q".to_vec());
    let residents = common::Residents {
        objects: vec![old, new],
        mutation_receipts: vec![],
        aggregate_receipts: vec![],
    };
    assert!(verify(&certificate, &residents).is_ok());
    let deleted_old = b"abcXtail".as_slice();
    let deleted_new = b"abctail".as_slice();
    let deletion = frontier_certificate(
        Query::ByteExactDiff {
            old: common::object_id(deleted_old),
            new: common::object_id(deleted_new),
            start: 3,
            before_end: 4,
            after_end: 3,
        },
        CompletenessWitness::ByteExactDiff {
            operator: lock(),
            old: common::object_id(deleted_old),
            new: common::object_id(deleted_new),
            start: 3,
            before_end: 4,
            after_end: 3,
            replacement_digest: zero_abi::sha256(b""),
        },
        vec![common::span(deleted_new, 3, 0)],
        vec![],
    );
    let deletion_residents = common::Residents {
        objects: vec![deleted_old, deleted_new],
        mutation_receipts: vec![],
        aggregate_receipts: vec![],
    };
    assert!(verify(&deletion, &deletion_residents).is_ok());
    let mut tampered = certificate.clone();
    tampered.payload = Cow::Borrowed(b"R");
    assert!(matches!(
        verify(&tampered, &residents),
        Err(VerificationError::PayloadMismatch { .. })
    ));
    let mut bad_range = certificate.clone();
    if let Query::ByteExactDiff { before_end, .. } = &mut bad_range.query {
        *before_end = 99;
    }
    if let CompletenessWitness::ByteExactDiff { before_end, .. } = &mut bad_range.completeness {
        *before_end = 99;
    }
    assert!(matches!(
        verify(&bad_range, &residents),
        Err(VerificationError::InvalidCompleteness)
    ));
    let noop = frontier_certificate(
        Query::ByteExactDiff {
            old: old_id,
            new: old_id,
            start: 3,
            before_end: 6,
            after_end: 6,
        },
        CompletenessWitness::ByteExactDiff {
            operator: lock(),
            old: old_id,
            new: old_id,
            start: 3,
            before_end: 6,
            after_end: 6,
            replacement_digest: zero_abi::sha256(b"XYZ"),
        },
        vec![common::span(old, 3, 3)],
        b"XYZ".to_vec(),
    );
    let noop_residents = common::Residents {
        objects: vec![old],
        mutation_receipts: vec![],
        aggregate_receipts: vec![],
    };
    assert!(matches!(
        verify(&noop, &noop_residents),
        Err(VerificationError::InvalidCompleteness)
    ));
}

#[test]
fn mutation_outcome_requires_an_independently_trusted_receipt() {
    let old = b"old".as_slice();
    let new = b"new".as_slice();
    let old_id = common::object_id(old);
    let new_id = common::object_id(new);
    let receipt = MutationReceipt {
        journal_id: [7; 32],
        sequence: 1,
        old: old_id,
        new: new_id,
        applied: true,
    };
    let trusted = canonical_receipt(&receipt);
    let digest = zero_abi::sha256(&trusted);
    let query = Query::MutationOutcome {
        journal_id: [7; 32],
        sequence: 1,
        old: old_id,
        new: new_id,
        applied: true,
    };
    let witness = CompletenessWitness::MutationOutcome {
        operator: lock(),
        journal_id: [7; 32],
        sequence: 1,
        old: old_id,
        new: new_id,
        applied: true,
        receipt_digest: digest,
    };
    let certificate = frontier_certificate(query, witness, vec![], trusted.clone());
    let residents = common::Residents {
        objects: vec![old, new],
        mutation_receipts: vec![([7; 32], 1, &trusted)],
        aggregate_receipts: vec![],
    };
    assert!(verify(&certificate, &residents).is_ok());

    let missing = common::Residents {
        objects: vec![old, new],
        mutation_receipts: vec![],
        aggregate_receipts: vec![],
    };
    assert!(matches!(
        verify(&certificate, &missing),
        Err(VerificationError::MissingTrustedReceipt)
    ));
    let mut malformed_bytes = trusted.clone();
    assert_eq!(malformed_bytes.pop(), Some(b'}'));
    malformed_bytes.extend_from_slice(br#","unknown":true}"#);
    let malformed = common::Residents {
        objects: vec![old, new],
        mutation_receipts: vec![([7; 32], 1, &malformed_bytes)],
        aggregate_receipts: vec![],
    };
    let mut malformed_certificate = certificate.clone();
    malformed_certificate.payload = Cow::Owned(malformed_bytes.clone());
    if let CompletenessWitness::MutationOutcome { receipt_digest, .. } =
        &mut malformed_certificate.completeness
    {
        *receipt_digest = zero_abi::sha256(&malformed_bytes);
    }
    assert!(matches!(
        verify(&malformed_certificate, &malformed),
        Err(VerificationError::MalformedTrustedReceipt)
    ));
    let mismatched_bytes = canonical_receipt(&MutationReceipt {
        sequence: 2,
        ..receipt.clone()
    });
    let mismatched = common::Residents {
        objects: vec![old, new],
        mutation_receipts: vec![([7; 32], 1, &mismatched_bytes)],
        aggregate_receipts: vec![],
    };
    let mut mismatched_certificate = certificate.clone();
    mismatched_certificate.payload = Cow::Owned(mismatched_bytes.clone());
    if let CompletenessWitness::MutationOutcome { receipt_digest, .. } =
        &mut mismatched_certificate.completeness
    {
        *receipt_digest = zero_abi::sha256(&mismatched_bytes);
    }
    assert!(matches!(
        verify(&mismatched_certificate, &mismatched),
        Err(VerificationError::WitnessQueryMismatch)
    ));
    let mut tampered = certificate.clone();
    tampered.payload.to_mut()[0] ^= 1;
    assert!(matches!(
        verify(&tampered, &residents),
        Err(VerificationError::InvalidCompleteness)
    ));

    let noop_receipt = canonical_receipt(&MutationReceipt {
        journal_id: [7; 32],
        sequence: 1,
        old: old_id,
        new: old_id,
        applied: true,
    });
    let noop = frontier_certificate(
        Query::MutationOutcome {
            journal_id: [7; 32],
            sequence: 1,
            old: old_id,
            new: old_id,
            applied: true,
        },
        CompletenessWitness::MutationOutcome {
            operator: lock(),
            journal_id: [7; 32],
            sequence: 1,
            old: old_id,
            new: old_id,
            applied: true,
            receipt_digest: zero_abi::sha256(&noop_receipt),
        },
        vec![],
        noop_receipt.clone(),
    );
    let noop_residents = common::Residents {
        objects: vec![old],
        mutation_receipts: vec![([7; 32], 1, &noop_receipt)],
        aggregate_receipts: vec![],
    };
    assert!(matches!(
        verify(&noop, &noop_residents),
        Err(VerificationError::InvalidCompleteness)
    ));
}

#[test]
fn aggregate_requires_an_independently_trusted_completion_receipt() {
    let first = b"one".as_slice();
    let second = b"two".as_slice();
    let objects = vec![common::object_id(first), common::object_id(second)];
    let snapshot = domain_snapshot_digest(&objects, "zero-index", "2");
    let result = b"sum=3".to_vec();
    let digest = zero_abi::sha256(&result);
    let receipt = AggregateReceipt {
        snapshot_id: snapshot,
        objects: objects.clone(),
        requested: 2,
        emitted: 1,
        result_digest: digest,
    };
    let trusted = canonical_receipt(&receipt);
    let query = Query::Aggregate {
        snapshot_id: snapshot,
        objects: objects.clone(),
        requested: 2,
        emitted: 1,
    };
    let witness = CompletenessWitness::Aggregate {
        operator: lock(),
        snapshot_id: snapshot,
        objects: objects.clone(),
        requested: 2,
        emitted: 1,
        result_digest: digest,
    };
    let certificate = frontier_certificate(query, witness, vec![], result);
    let residents = common::Residents {
        objects: vec![first, second],
        mutation_receipts: vec![],
        aggregate_receipts: vec![(snapshot, &trusted)],
    };
    assert!(verify(&certificate, &residents).is_ok());

    let missing = common::Residents {
        objects: vec![first, second],
        mutation_receipts: vec![],
        aggregate_receipts: vec![],
    };
    assert!(matches!(
        verify(&certificate, &missing),
        Err(VerificationError::MissingTrustedReceipt)
    ));
    let mut malformed_bytes = trusted.clone();
    assert_eq!(malformed_bytes.pop(), Some(b'}'));
    malformed_bytes.extend_from_slice(br#","unknown":true}"#);
    let malformed = common::Residents {
        objects: vec![first, second],
        mutation_receipts: vec![],
        aggregate_receipts: vec![(snapshot, &malformed_bytes)],
    };
    assert!(matches!(
        verify(&certificate, &malformed),
        Err(VerificationError::MalformedTrustedReceipt)
    ));
    let mismatched_bytes = canonical_receipt(&AggregateReceipt {
        emitted: 2,
        ..receipt.clone()
    });
    let mismatched = common::Residents {
        objects: vec![first, second],
        mutation_receipts: vec![],
        aggregate_receipts: vec![(snapshot, &mismatched_bytes)],
    };
    assert!(matches!(
        verify(&certificate, &mismatched),
        Err(VerificationError::WitnessQueryMismatch)
    ));

    let mut omitted = certificate.clone();
    if let CompletenessWitness::Aggregate { objects, .. } = &mut omitted.completeness {
        objects.pop();
    }
    assert!(matches!(
        verify(&omitted, &residents),
        Err(VerificationError::WitnessQueryMismatch)
    ));
    let mut reordered = certificate.clone();
    if let CompletenessWitness::Aggregate { objects, .. } = &mut reordered.completeness {
        objects.reverse();
    }
    assert!(matches!(
        verify(&reordered, &residents),
        Err(VerificationError::WitnessQueryMismatch)
    ));
    let mut bad_count = certificate.clone();
    if let Query::Aggregate { requested, .. } = &mut bad_count.query {
        *requested = 1;
    }
    if let CompletenessWitness::Aggregate { requested, .. } = &mut bad_count.completeness {
        *requested = 1;
    }
    assert!(matches!(
        verify(&bad_count, &residents),
        Err(VerificationError::InvalidCompleteness)
    ));
    let mut bad_digest = certificate.clone();
    if let CompletenessWitness::Aggregate { result_digest, .. } = &mut bad_digest.completeness {
        result_digest[0] ^= 1;
    }
    assert!(matches!(
        verify(&bad_digest, &residents),
        Err(VerificationError::WitnessQueryMismatch)
    ));
}
