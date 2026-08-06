mod common;

use std::borrow::Cow;

use common::{Residents, fixture, object_id, provenance, span};
use zero_abi::{
    CwirVerifierClassV1, DigestV1, EffectProgramV1, EffectRollbackV1, EffectVerificationPlanV1,
    EffectVerificationStepV1, TypedEffectOperationV1, sha256,
};
use zero_cert::{
    CompletenessWitness, EffectLocalizationV1, EffectVerificationOutcomeV1,
    EffectWitnessFailureCodeV1, EffectWitnessKindV1, EvidenceCertificate, OperatorLock, Query,
    accept_effect_verification_v1, domain_snapshot_digest, effect_witness_contract_digest_v1,
    incomplete_effect_verification_v1, reject_effect_verification_v1, verify,
};

fn digest(byte: u8) -> DigestV1 {
    DigestV1::from_bytes([byte; 32])
}

fn program(snapshot: DigestV1) -> EffectProgramV1 {
    let bytes = b"verified result".to_vec();
    EffectProgramV1::new(
        snapshot,
        "return_literal",
        vec![],
        vec![],
        vec![TypedEffectOperationV1::ReturnLiteral {
            payload_digest: DigestV1::from_bytes(sha256(&bytes)),
            bytes,
        }],
        vec![],
        EffectVerificationPlanV1::new(vec![EffectVerificationStepV1 {
            verifier_digest: digest(20),
            predicate_digest: digest(21),
            environment_digest: digest(22),
            required_snapshot: snapshot,
            verifier_class: CwirVerifierClassV1::ExactChecker,
        }])
        .unwrap(),
        EffectRollbackV1::ReadOnly,
    )
    .unwrap()
}

#[test]
fn accepted_outcome_requires_verified_evidence_and_is_digest_bound() {
    let (certificate, resolver) = fixture(b"exact evidence");
    let verified = verify(&certificate, &resolver).unwrap();
    let outcome = accept_effect_verification_v1(
        digest(1),
        &program(digest(2)),
        digest(3),
        digest(21),
        digest(2),
        digest(20),
        &verified,
    )
    .unwrap();
    let EffectVerificationOutcomeV1::Accepted(accepted) = outcome else {
        panic!("expected accepted outcome");
    };
    assert_eq!(accepted.action_digest(), program(digest(2)).action_digest());
    assert_eq!(accepted.state_snapshot(), digest(2));
    assert_eq!(accepted.verifier_class(), CwirVerifierClassV1::ExactChecker);
    assert!(!accepted.canonical_bytes().unwrap().is_empty());
    assert_eq!(
        accept_effect_verification_v1(
            digest(1),
            &program(digest(2)),
            digest(3),
            digest(99),
            digest(2),
            digest(20),
            &verified,
        )
        .unwrap_err()
        .failure_code(),
        EffectWitnessFailureCodeV1::PredicateNotInPlan
    );
    assert_eq!(
        accept_effect_verification_v1(
            digest(1),
            &program(digest(2)),
            digest(3),
            digest(21),
            digest(2),
            digest(99),
            &verified,
        )
        .unwrap_err()
        .failure_code(),
        EffectWitnessFailureCodeV1::VerificationBindingMismatch
    );
}

#[test]
fn rejected_and_incomplete_witnesses_bind_localization_and_exact_refs() {
    let (certificate, resolver) = fixture(b"exact evidence");
    let verified = verify(&certificate, &resolver).unwrap();
    let effect = program(digest(2));
    let outcome = reject_effect_verification_v1(
        digest(1),
        &effect,
        digest(3),
        EffectWitnessKindV1::PredicateMismatch,
        digest(21),
        digest(2),
        EffectLocalizationV1::operation(0),
        vec![digest(9), digest(8)],
        digest(20),
        &verified,
    )
    .unwrap();
    let EffectVerificationOutcomeV1::Rejected(witness) = outcome else {
        panic!("expected rejected outcome");
    };
    assert_eq!(witness.exact_evidence_refs().len(), 1);
    assert_eq!(witness.expansion_handles(), &[digest(8), digest(9)]);
    assert_eq!(witness.localization().operation_index(), Some(0));
    assert_eq!(
        reject_effect_verification_v1(
            digest(1),
            &effect,
            digest(3),
            EffectWitnessKindV1::PredicateMismatch,
            digest(21),
            digest(2),
            EffectLocalizationV1::operation(1),
            vec![],
            digest(20),
            &verified,
        )
        .unwrap_err()
        .failure_code(),
        EffectWitnessFailureCodeV1::InvalidLocalization
    );
    let bytes = witness.canonical_bytes().unwrap();
    assert_eq!(
        zero_cert::EffectWitnessV1::from_canonical_bytes(&bytes).unwrap(),
        witness
    );
    let mut tampered = serde_json::to_value(&witness).unwrap();
    tampered["witness_digest"] = serde_json::Value::String(digest(99).to_hex());
    let tampered = zero_abi::canonical_json(&tampered).into_bytes();
    assert_eq!(
        zero_cert::EffectWitnessV1::from_canonical_bytes(&tampered)
            .unwrap_err()
            .failure_code(),
        EffectWitnessFailureCodeV1::WitnessDigestMismatch
    );

    assert!(matches!(
        incomplete_effect_verification_v1(
            digest(1),
            &effect,
            digest(3),
            EffectWitnessKindV1::IncompleteCoverage,
            digest(21),
            digest(2),
            EffectLocalizationV1::predicate(),
            vec![],
            digest(20),
            &verified,
        )
        .unwrap(),
        EffectVerificationOutcomeV1::Incomplete(_)
    ));
}

#[test]
fn evidence_snapshot_scope_mismatch_is_typed() {
    let bytes = b"alpha beta";
    let object = object_id(bytes);
    let objects = vec![object];
    let observed_snapshot = domain_snapshot_digest(&objects, "zero-index", "2");
    let certificate = EvidenceCertificate {
        query: Query::ExactSearchDomain {
            pattern: Cow::Borrowed(b"alpha"),
            objects: objects.clone(),
            snapshot_id: observed_snapshot,
            index_id: "zero-index".into(),
            index_version: "2".into(),
        },
        spans: vec![span(bytes, 0, 5)],
        payload: Cow::Borrowed(b"alpha"),
        provenance: provenance(),
        completeness: CompletenessWitness::ExactSearchDomain {
            operator: OperatorLock {
                operator_id: "read-span".into(),
                operator_version: "1".into(),
            },
            pattern: Cow::Borrowed(b"alpha"),
            objects: objects.clone(),
            snapshot_id: observed_snapshot,
            index_id: "zero-index".into(),
            index_version: "2".into(),
            match_count: 1,
        },
        input_token_cost: 1,
        backend_work_units: 1,
    };
    let resolver = Residents {
        objects: vec![bytes],
        mutation_receipts: vec![],
        aggregate_receipts: vec![],
    };
    let verified = verify(&certificate, &resolver).unwrap();
    let expected_snapshot = digest(99);
    let error = accept_effect_verification_v1(
        digest(1),
        &program(expected_snapshot),
        digest(3),
        digest(21),
        expected_snapshot,
        digest(20),
        &verified,
    )
    .unwrap_err();
    assert_eq!(
        error.failure_code(),
        EffectWitnessFailureCodeV1::StaleEvidence
    );
}

#[test]
fn localization_bounds_and_contract_digest_are_stable() {
    assert_eq!(
        EffectLocalizationV1::byte_range(digest(1), u64::MAX, 1)
            .unwrap_err()
            .failure_code(),
        EffectWitnessFailureCodeV1::RangeOverflow
    );
    assert_eq!(
        effect_witness_contract_digest_v1().to_hex(),
        "9fc748d0722b0a31ea39974abb4e4005a44a5a27bafcb9f3b406d10d7ecf630a"
    );
}
