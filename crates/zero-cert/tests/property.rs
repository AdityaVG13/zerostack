mod common;
use common::fixture;
use std::borrow::Cow;
use zero_cert::{verify, OperatorLock, VerificationError};

#[test]
fn fixed_seed_valid_and_single_bit_tampering_property() {
    let mut state = 0x4d59_5df4_d0f3_3173_u64;
    for len in 1..=128usize {
        let mut bytes = vec![0; len];
        for byte in &mut bytes {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *byte = state as u8;
        }
        let (certificate, resident) = fixture(&bytes);
        assert!(verify(&certificate, &resident).is_ok());
        let mut payload = certificate.clone();
        let mut changed = bytes.clone();
        changed[len / 2] ^= 1;
        payload.payload = Cow::Owned(changed);
        assert!(matches!(
            verify(&payload, &resident),
            Err(VerificationError::PayloadMismatch { .. })
        ));
        let mut digest = certificate.clone();
        digest.spans[0].span_digest[(state as usize) & 31] ^= 1;
        assert!(matches!(
            verify(&digest, &resident),
            Err(VerificationError::SpanDigestMismatch { .. })
        ));
    }
}

#[test]
fn ordered_domain_digest_and_overlapping_matches_are_deterministic() {
    use zero_cert::{domain_snapshot_digest, CompletenessWitness, EvidenceCertificate, Query};
    let first = b"aaa".as_slice();
    let second = b"ba".as_slice();
    let objects = vec![common::object_id(first), common::object_id(second)];
    let snapshot = domain_snapshot_digest(&objects, "zero-index", "2");
    let mut reversed = objects.clone();
    reversed.reverse();
    assert_ne!(
        snapshot,
        domain_snapshot_digest(&reversed, "zero-index", "2")
    );
    assert_ne!(
        snapshot,
        domain_snapshot_digest(&objects, "zero-index", "3")
    );
    let certificate = EvidenceCertificate {
        query: Query::ExactSearchDomain {
            pattern: Cow::Borrowed(b"aa"),
            objects: objects.clone(),
            snapshot_id: snapshot,
            index_id: "zero-index".into(),
            index_version: "2".into(),
        },
        spans: vec![common::span(first, 0, 2), common::span(first, 1, 2)],
        payload: Cow::Borrowed(b"aaaa"),
        provenance: common::provenance(),
        completeness: CompletenessWitness::ExactSearchDomain {
            operator: OperatorLock {
                operator_id: "read-span".into(),
                operator_version: "1".into(),
            },
            pattern: Cow::Borrowed(b"aa"),
            objects,
            snapshot_id: snapshot,
            index_id: "zero-index".into(),
            index_version: "2".into(),
            match_count: 2,
        },
        input_token_cost: 0,
        backend_work_units: 0,
    };
    let residents = common::Residents {
        objects: vec![first, second],
        mutation_receipts: vec![],
        aggregate_receipts: vec![],
    };
    assert!(verify(&certificate, &residents).is_ok());
    let mut omitted = certificate.clone();
    omitted.spans.pop();
    omitted.payload.to_mut().truncate(2);
    assert!(matches!(
        verify(&omitted, &residents),
        Err(VerificationError::InvalidCompleteness)
    ));
}
