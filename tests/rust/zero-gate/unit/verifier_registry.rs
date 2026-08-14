//! V6-R8 targeted tests: verifier registry (ZS-VERIFY-001), obligation
//! checklist delta binding (ZS-VERIFY-002), successor-state verification
//! (ZS-VERIFY-003).

use std::collections::BTreeMap;

use super::*;
use zero_abi::ScopeObligationV1;

fn d(byte: u8) -> DigestV1 {
    DigestV1::from_bytes([byte; 32])
}

fn grades(entries: &[(ProtectedDimensionV1, CoverageGradeV1)]) -> BTreeMap<ProtectedDimensionV1, CoverageGradeV1> {
    entries.iter().copied().collect()
}

fn registered_registry() -> VerifierRegistryV1 {
    let mut registry = VerifierRegistryV1::new();
    registry.set_trusted_version("tests-verifier", "2");
    registry.set_trusted_version("successor-verifier", "1");
    registry.set_trusted_version("stale-verifier", "2");
    registry
}

fn tests_record() -> VerifierRegistryRecordV1 {
    VerifierRegistryRecordV1::new(
        "tests-verifier",
        "2",
        VerifierDomainV1::CurrentEffect,
        ProtectedDimensionV1::Tests,
        vec![d(1)],
        VerifierResultV1::Pass,
        d(7),
        12,
        grades(&[(ProtectedDimensionV1::Tests, CoverageGradeV1::Proved)]),
    )
    .unwrap()
}

fn successor_record() -> VerifierRegistryRecordV1 {
    VerifierRegistryRecordV1::new(
        "successor-verifier",
        "1",
        VerifierDomainV1::SuccessorState,
        ProtectedDimensionV1::SuccessorState,
        vec![d(1)],
        VerifierResultV1::Pass,
        d(7),
        15,
        grades(&[(
            ProtectedDimensionV1::SuccessorState,
            CoverageGradeV1::Proved,
        )]),
    )
    .unwrap()
}

/// Deterministic recompute of a successor root from predecessor + receipts.
fn honest_recompute(predecessor: DigestV1, receipts: &[DigestV1]) -> DigestV1 {
    let mut bytes = predecessor.as_bytes().to_vec();
    for receipt in receipts {
        bytes.extend_from_slice(receipt.as_bytes());
    }
    DigestV1::from_bytes(sha256(&bytes))
}

fn honest_transition() -> SuccessorStateTransitionV1 {
    let receipts = vec![d(10), d(11)];
    let claimed = honest_recompute(d(1), &receipts);
    SuccessorStateTransitionV1::new(d(1), claimed, receipts).unwrap()
}

fn honest_successor(root: DigestV1) -> SuccessorStateV1 {
    let obligations = ProtectedScopeObligationsV1::new(vec![
        ScopeObligationV1::new(
            ProtectedDimensionV1::Tests,
            true,
            CoverageGradeV1::Proved,
        )
        .unwrap(),
    ])
    .unwrap();
    let checklist = ObligationChecklistV1::new(
        root,
        vec![ObligationChecklistEntryV1 {
            dimension: ProtectedDimensionV1::Tests,
            required: true,
            evidence_refs: vec![d(20)],
        }],
    )
    .unwrap();
    SuccessorStateV1::new(root, obligations, checklist).unwrap()
}

fn future_action() -> RegisteredFutureActionV1 {
    RegisteredFutureActionV1::new(
        "future-migration",
        d(30),
        vec![ProtectedDimensionV1::Tests],
    )
    .unwrap()
}

fn successor_registry() -> VerifierRegistryV1 {
    let mut registry = registered_registry();
    registry.register(tests_record()).unwrap();
    registry.register(successor_record()).unwrap();
    registry
}

#[test]
fn registry_lookup_is_typed_deterministic_and_unknown_is_loud_refusal() {
    let registry = successor_registry();

    // Deterministic typed lookup returns the exact registered record.
    let found = registry
        .lookup(VerifierDomainV1::CurrentEffect, ProtectedDimensionV1::Tests)
        .unwrap();
    assert_eq!(found.verifier_id, "tests-verifier");
    assert_eq!(found.verifier_version, "2");
    assert_eq!(found.input_roots, vec![d(1)]);
    assert_eq!(found.runtime_ms, 12);
    assert_eq!(
        found.grades.get(&ProtectedDimensionV1::Tests),
        Some(&CoverageGradeV1::Proved)
    );
    let successor = registry
        .lookup(
            VerifierDomainV1::SuccessorState,
            ProtectedDimensionV1::SuccessorState,
        )
        .unwrap();
    assert_eq!(successor.verifier_id, "successor-verifier");

    // Unknown (domain, kind) is a loud typed refusal -- never a silent skip.
    for (domain, kind) in [
        (VerifierDomainV1::CurrentEffect, ProtectedDimensionV1::Security),
        (VerifierDomainV1::SuccessorState, ProtectedDimensionV1::Tests),
        (VerifierDomainV1::CurrentEffect, ProtectedDimensionV1::SuccessorState),
    ] {
        assert_eq!(
            registry.lookup(domain, kind).unwrap_err(),
            VerifierRegistryErrorV1::UnknownVerifier { domain, kind }
        );
    }
}

#[test]
fn registration_refuses_untrusted_and_duplicate_verifiers() {
    let mut registry = registered_registry();

    let untrusted = VerifierRegistryRecordV1::new(
        "ghost-verifier",
        "1",
        VerifierDomainV1::CurrentEffect,
        ProtectedDimensionV1::Security,
        vec![d(1)],
        VerifierResultV1::Pass,
        d(7),
        5,
        grades(&[(ProtectedDimensionV1::Security, CoverageGradeV1::Proved)]),
    )
    .unwrap();
    assert_eq!(
        registry.register(untrusted).unwrap_err(),
        VerifierRegistryErrorV1::MissingTrustedVerifier {
            verifier_id: "ghost-verifier".into()
        }
    );

    registry.register(tests_record()).unwrap();
    assert_eq!(
        registry.register(tests_record()).unwrap_err(),
        VerifierRegistryErrorV1::DuplicateRegistration {
            domain: VerifierDomainV1::CurrentEffect,
            kind: ProtectedDimensionV1::Tests,
        }
    );
}

#[test]
fn freshness_accepts_current_version_and_refuses_missing_or_stale() {
    let registry = registered_registry();

    assert_eq!(registry.freshness(&tests_record()), Ok(()));

    let stale = VerifierRegistryRecordV1::new(
        "stale-verifier",
        "1",
        VerifierDomainV1::CurrentEffect,
        ProtectedDimensionV1::Security,
        vec![d(1)],
        VerifierResultV1::Pass,
        d(7),
        5,
        grades(&[(ProtectedDimensionV1::Security, CoverageGradeV1::Proved)]),
    )
    .unwrap();
    assert_eq!(
        registry.freshness(&stale).unwrap_err(),
        VerifierRegistryErrorV1::StaleVerifier {
            verifier_id: "stale-verifier".into(),
            expected: "2".into(),
            observed: "1".into(),
        }
    );

    let untrusted = VerifierRegistryRecordV1::new(
        "ghost-verifier",
        "1",
        VerifierDomainV1::CurrentEffect,
        ProtectedDimensionV1::Performance,
        vec![d(1)],
        VerifierResultV1::Pass,
        d(7),
        5,
        grades(&[(ProtectedDimensionV1::Performance, CoverageGradeV1::Proved)]),
    )
    .unwrap();
    assert_eq!(
        registry.freshness(&untrusted).unwrap_err(),
        VerifierRegistryErrorV1::MissingTrustedVerifier {
            verifier_id: "ghost-verifier".into()
        }
    );
}

#[test]
fn record_validation_is_fail_closed() {
    // A passing record must grade its own kind as covered.
    let lying = VerifierRegistryRecordV1::new(
        "tests-verifier",
        "2",
        VerifierDomainV1::CurrentEffect,
        ProtectedDimensionV1::Tests,
        vec![d(1)],
        VerifierResultV1::Pass,
        d(7),
        12,
        grades(&[(ProtectedDimensionV1::Tests, CoverageGradeV1::Unknown)]),
    );
    assert!(matches!(
        lying,
        Err(VerifierRegistryErrorV1::InvalidRecord(_))
    ));

    // Zero-cost and rootless records are refused.
    let no_inputs = VerifierRegistryRecordV1::new(
        "tests-verifier",
        "2",
        VerifierDomainV1::CurrentEffect,
        ProtectedDimensionV1::Tests,
        vec![],
        VerifierResultV1::Pass,
        d(7),
        12,
        grades(&[(ProtectedDimensionV1::Tests, CoverageGradeV1::Proved)]),
    );
    assert!(matches!(
        no_inputs,
        Err(VerifierRegistryErrorV1::InvalidRecord(_))
    ));
    let zero_runtime = VerifierRegistryRecordV1::new(
        "tests-verifier",
        "2",
        VerifierDomainV1::CurrentEffect,
        ProtectedDimensionV1::Tests,
        vec![d(1)],
        VerifierResultV1::Pass,
        d(7),
        0,
        grades(&[(ProtectedDimensionV1::Tests, CoverageGradeV1::Proved)]),
    );
    assert!(matches!(
        zero_runtime,
        Err(VerifierRegistryErrorV1::InvalidRecord(_))
    ));
}

#[test]
fn substitute_delta_after_verification_fails() {
    let registry = successor_registry();
    let checklist = ObligationChecklistV1::new(
        d(1),
        vec![ObligationChecklistEntryV1 {
            dimension: ProtectedDimensionV1::Tests,
            required: true,
            evidence_refs: vec![d(20)],
        }],
    )
    .unwrap();

    // Authority for the exact verified delta holds.
    assert!(assert_current_effect_authority_v1(
        &registry,
        ProtectedDimensionV1::Tests,
        d(1),
        &checklist
    )
    .is_ok());

    // A substituted delta is a loud refusal, both via the checklist binding
    // and via the registered record's verified input roots.
    assert_eq!(
        assert_current_effect_authority_v1(
            &registry,
            ProtectedDimensionV1::Tests,
            d(2),
            &checklist
        )
        .unwrap_err(),
        VerifierRegistryErrorV1::DeltaSubstitutedAfterVerification {
            expected: d(1),
            observed: d(2),
        }
    );
    let checklist_for_other = ObligationChecklistV1::new(
        d(2),
        vec![ObligationChecklistEntryV1 {
            dimension: ProtectedDimensionV1::Tests,
            required: true,
            evidence_refs: vec![d(20)],
        }],
    )
    .unwrap();
    assert_eq!(
        assert_current_effect_authority_v1(
            &registry,
            ProtectedDimensionV1::Tests,
            d(2),
            &checklist_for_other
        )
        .unwrap_err(),
        VerifierRegistryErrorV1::UnverifiedDelta { delta_root: d(2) }
    );
}

#[test]
fn non_passing_registered_result_never_grants_authority() {
    let mut registry = registered_registry();
    let failed = VerifierRegistryRecordV1::new(
        "tests-verifier",
        "2",
        VerifierDomainV1::CurrentEffect,
        ProtectedDimensionV1::Security,
        vec![d(1)],
        VerifierResultV1::Fail,
        d(7),
        12,
        grades(&[(ProtectedDimensionV1::Security, CoverageGradeV1::Unknown)]),
    )
    .unwrap();
    registry.register(failed).unwrap();
    let checklist = ObligationChecklistV1::new(
        d(1),
        vec![ObligationChecklistEntryV1 {
            dimension: ProtectedDimensionV1::Security,
            required: true,
            evidence_refs: vec![d(20)],
        }],
    )
    .unwrap();
    assert_eq!(
        assert_current_effect_authority_v1(
            &registry,
            ProtectedDimensionV1::Security,
            d(1),
            &checklist
        )
        .unwrap_err(),
        VerifierRegistryErrorV1::NonPassingVerifierResult {
            result: VerifierResultV1::Fail
        }
    );
}

#[test]
fn successor_accepted_on_honest_transition_and_mints_sealed_receipt() {
    let registry = successor_registry();
    let transition = honest_transition();
    let successor = honest_successor(transition.claimed_successor_root);
    let action = future_action();

    let receipt = verify_successor_state_v1(
        &registry,
        &transition,
        &successor,
        &action,
        honest_recompute,
    )
    .unwrap();

    // Verification produced a sealed, digest-bound receipt.
    assert_eq!(receipt.receipt_version, VERIFIER_REGISTRY_CONTRACT_VERSION_V1);
    assert_eq!(receipt.verifier_id, "successor-verifier");
    assert_eq!(receipt.verifier_version, "1");
    assert_eq!(receipt.domain, VerifierDomainV1::SuccessorState);
    assert_eq!(receipt.predecessor_root, d(1));
    assert_eq!(receipt.successor_root, transition.claimed_successor_root);
    assert_eq!(receipt.action_id, "future-migration");
    assert_eq!(receipt.runtime_ms, 15);
    assert_ne!(receipt.evidence_digest, DigestV1::ZERO);
    let root = receipt.receipt_root().unwrap();
    assert_ne!(root, DigestV1::ZERO);
    assert_eq!(root, receipt.receipt_root().unwrap()); // deterministic
}

#[test]
fn successor_refused_on_tampered_state_and_fault_carries_receipts() {
    let registry = successor_registry();
    let action = future_action();

    // Tampered claimed successor root: loud SuccessorMismatch WITH receipts.
    let honest = honest_transition();
    let tampered_transition = SuccessorStateTransitionV1::new(
        d(1),
        d(99),
        honest.receipts.clone(),
    )
    .unwrap();
    let successor = honest_successor(honest.claimed_successor_root);
    let fault = verify_successor_state_v1(
        &registry,
        &tampered_transition,
        &successor,
        &action,
        honest_recompute,
    )
    .unwrap_err();
    assert_eq!(
        fault,
        VerifierRegistryErrorV1::SuccessorMismatch {
            verifier_id: "successor-verifier".into(),
            claimed: d(99),
            recomputed: honest.claimed_successor_root,
            receipts: honest.receipts.clone(),
        }
    );

    // Tampered receipts: recompute disagrees, fault carries the tampered set.
    let swapped = SuccessorStateTransitionV1::new(
        d(1),
        honest.claimed_successor_root,
        vec![d(10), d(12)],
    )
    .unwrap();
    let fault = verify_successor_state_v1(
        &registry,
        &swapped,
        &successor,
        &action,
        honest_recompute,
    )
    .unwrap_err();
    let VerifierRegistryErrorV1::SuccessorMismatch {
        verifier_id,
        claimed,
        recomputed,
        receipts,
    } = fault
    else {
        panic!("expected SuccessorMismatch");
    };
    assert_eq!(verifier_id, "successor-verifier");
    assert_eq!(claimed, honest.claimed_successor_root);
    assert_ne!(recomputed, claimed);
    assert_eq!(receipts, vec![d(10), d(12)]);

    // Unknown registered verifier for the successor domain: loud refusal.
    let mut empty = registered_registry();
    empty.register(tests_record()).unwrap();
    assert_eq!(
        verify_successor_state_v1(
            &empty,
            &honest,
            &successor,
            &action,
            honest_recompute
        )
        .unwrap_err(),
        VerifierRegistryErrorV1::UnknownVerifier {
            domain: VerifierDomainV1::SuccessorState,
            kind: ProtectedDimensionV1::SuccessorState,
        }
    );
}

#[test]
fn locally_passing_edit_that_breaks_registered_future_action_is_rejected() {
    let registry = successor_registry();

    // The edit itself is locally passing: current-effect authority holds.
    let edit_delta = d(1);
    let edit_checklist = ObligationChecklistV1::new(
        edit_delta,
        vec![ObligationChecklistEntryV1 {
            dimension: ProtectedDimensionV1::Tests,
            required: true,
            evidence_refs: vec![d(20)],
        }],
    )
    .unwrap();
    assert!(assert_current_effect_authority_v1(
        &registry,
        ProtectedDimensionV1::Tests,
        edit_delta,
        &edit_checklist
    )
    .is_ok());

    // But the successor state drops the evidence-backed Tests coverage the
    // registered future action depends on: grade becomes Unknown.
    let transition = honest_transition();
    let mut degraded = honest_successor(transition.claimed_successor_root);
    degraded.obligations = ProtectedScopeObligationsV1::new(vec![ScopeObligationV1::new(
        ProtectedDimensionV1::Tests,
        true,
        CoverageGradeV1::Unknown,
    )
    .unwrap()])
    .unwrap();

    let fault = verify_successor_state_v1(
        &registry,
        &transition,
        &degraded,
        &future_action(),
        honest_recompute,
    )
    .unwrap_err();
    assert_eq!(
        fault,
        VerifierRegistryErrorV1::RegisteredFutureActionNotPreserved {
            action_id: "future-migration".into(),
            dimension: ProtectedDimensionV1::Tests,
            grade: CoverageGradeV1::Unknown,
        }
    );

    // Variant: grade looks covered but the checklist carries no evidence
    // refs for the required dimension.
    let mut evidence_less = honest_successor(transition.claimed_successor_root);
    evidence_less.checklist = ObligationChecklistV1::new(
        transition.claimed_successor_root,
        vec![ObligationChecklistEntryV1 {
            dimension: ProtectedDimensionV1::Security,
            required: true,
            evidence_refs: vec![d(21)],
        }],
    )
    .unwrap();
    let fault = verify_successor_state_v1(
        &registry,
        &transition,
        &evidence_less,
        &future_action(),
        honest_recompute,
    )
    .unwrap_err();
    let VerifierRegistryErrorV1::RegisteredFutureActionNotPreserved {
        action_id,
        dimension,
        grade,
    } = fault
    else {
        panic!("expected RegisteredFutureActionNotPreserved");
    };
    assert_eq!(action_id, "future-migration");
    assert_eq!(dimension, ProtectedDimensionV1::Tests);
    assert_eq!(grade, CoverageGradeV1::Proved);
}

#[test]
fn successor_state_must_bind_its_own_root_and_preserve_required_dimensions() {
    let registry = successor_registry();
    let transition = honest_transition();
    let action = future_action();

    // Checklist bound to a different root than the successor state is invalid.
    let mut misbound = honest_successor(transition.claimed_successor_root);
    misbound.checklist = ObligationChecklistV1::new(
        d(200),
        vec![ObligationChecklistEntryV1 {
            dimension: ProtectedDimensionV1::Tests,
            required: true,
            evidence_refs: vec![d(20)],
        }],
    )
    .unwrap();
    assert!(matches!(
        verify_successor_state_v1(&registry, &transition, &misbound, &action, honest_recompute)
            .unwrap_err(),
        VerifierRegistryErrorV1::InvalidSuccessorState(_)
    ));

    // A transition that does not change the root is invalid.
    assert_eq!(
        SuccessorStateTransitionV1::new(d(1), d(1), vec![d(10)]).unwrap_err(),
        VerifierRegistryErrorV1::InvalidTransition(
            "a transition must change the state root".into()
        )
    );

    // A transition with zero receipts cannot substantiate anything.
    assert!(matches!(
        SuccessorStateTransitionV1::new(d(1), d(2), vec![]).unwrap_err(),
        VerifierRegistryErrorV1::InvalidTransition(_)
    ));
}
