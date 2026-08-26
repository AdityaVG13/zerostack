use std::collections::BTreeMap;

use zero_abi::*;

fn root(byte: char) -> String {
    std::iter::repeat_n(byte, 64).collect()
}

fn roots() -> CapsuleRoots {
    CapsuleRoots {
        project: root('a'),
        task: root('b'),
        protected_scope: root('c'),
        obligations: root('d'),
        evidence: root('e'),
        policy: root('f'),
        execution: root('1'),
        verifier: root('2'),
        fallback: root('3'),
        ledger: root('4'),
    }
}

const ALL_STATES: [CapsuleState; 9] = [
    CapsuleState::Draft,
    CapsuleState::EvidenceComplete,
    CapsuleState::PolicyComplete,
    CapsuleState::InterruptRequired,
    CapsuleState::Executable,
    CapsuleState::ExecutedInSandbox,
    CapsuleState::Verified,
    CapsuleState::BudgetAccepted,
    CapsuleState::Committed,
];

#[test]
fn draft_genesis_is_deterministic_and_rooted() {
    let first = WorkCapsule::draft(roots(), 1, 0, 0).expect("valid draft");
    let second = WorkCapsule::draft(roots(), 1, 0, 0).expect("valid draft");
    assert_eq!(first, second);
    assert_eq!(first.root().unwrap(), second.root().unwrap());
    assert_eq!(first.version, 1);
    assert_eq!(first.epoch, 1);
    assert_eq!(first.state, CapsuleState::Draft);
    assert_eq!(first.provider_usage_budget, 0);
    assert_eq!(first.complete_work_budget, 0);

    // Any root difference changes the canonical capsule root.
    let mut other = roots();
    other.ledger = root('9');
    let changed = WorkCapsule::draft(other, 1, 0, 0).expect("valid draft");
    assert_ne!(first.root().unwrap(), changed.root().unwrap());

    // Any epoch or budget difference changes the canonical capsule root.
    let changed_epoch = WorkCapsule::draft(roots(), 2, 0, 0).expect("valid draft");
    assert_ne!(first.root().unwrap(), changed_epoch.root().unwrap());
    let changed_provider = WorkCapsule::draft(roots(), 1, 5, 0).expect("valid draft");
    assert_ne!(first.root().unwrap(), changed_provider.root().unwrap());
    let changed_complete = WorkCapsule::draft(roots(), 1, 0, 7).expect("valid draft");
    assert_ne!(first.root().unwrap(), changed_complete.root().unwrap());

    // Budgets are retained exactly as supplied, zero included.
    assert_eq!(changed_provider.provider_usage_budget, 5);
    assert_eq!(changed_provider.complete_work_budget, 0);
    assert_eq!(changed_complete.provider_usage_budget, 0);
    assert_eq!(changed_complete.complete_work_budget, 7);

    // Zero is never manufactured: epoch zero is rejected at genesis, and
    // zero budgets only appear because the draft above supplied them.
    assert!(WorkCapsule::draft(roots(), 0, 0, 0).is_err());

    // Invalid roots are rejected at genesis.
    let mut invalid = roots();
    invalid.project = "not-a-root".into();
    assert!(WorkCapsule::draft(invalid, 1, 0, 0).is_err());
}

#[test]
fn capsule_validate_requires_positive_version_and_epoch() {
    let mut versionless = WorkCapsule::draft(roots(), 1, 0, 0).unwrap();
    versionless.version = 0;
    assert!(versionless.validate().is_err());
    assert!(versionless.root().is_err());

    let mut epoched_zero = WorkCapsule::draft(roots(), 1, 0, 0).unwrap();
    epoched_zero.epoch = 0;
    assert!(epoched_zero.validate().is_err());
    assert!(epoched_zero.root().is_err());
}

#[test]
fn every_state_edge_is_legal_only_when_allowed() {
    for from in ALL_STATES {
        for to in ALL_STATES {
            let mut capsule = WorkCapsule::draft(roots(), 1, 0, 0).unwrap();
            capsule.state = from;
            capsule.epoch = 7;
            let mut successor = capsule.clone();
            successor.state = to;
            successor.epoch += 1;
            assert_eq!(
                capsule.validate_successor(&successor).is_ok(),
                from.allows(to),
                "{from:?} -> {to:?} violates the transition law"
            );
        }
    }
}

#[test]
fn advance_follows_the_law_and_bumps_epoch() {
    let mut capsule = WorkCapsule::draft(roots(), 1, 0, 0).unwrap();
    assert_eq!(capsule.epoch, 1);
    capsule.advance(CapsuleState::EvidenceComplete).unwrap();
    assert_eq!(capsule.state, CapsuleState::EvidenceComplete);
    assert_eq!(capsule.epoch, 2);

    // Illegal jumps fail closed and leave the capsule untouched.
    let before = capsule.clone();
    assert!(capsule.advance(CapsuleState::Committed).is_err());
    assert_eq!(capsule, before);

    capsule.advance(CapsuleState::PolicyComplete).unwrap();
    assert_eq!(capsule.state, CapsuleState::PolicyComplete);
    assert_eq!(capsule.epoch, 3);
}

#[test]
fn successor_rejects_immutable_root_mutation_but_allows_evolving_roots() {
    let capsule = WorkCapsule::draft(roots(), 1, 0, 0).unwrap();
    let mut successor = capsule.clone();
    successor.state = CapsuleState::EvidenceComplete;
    successor.epoch += 1;
    assert!(capsule.validate_successor(&successor).is_ok());

    // Immutable across the capsule life: project, task, protected_scope, fallback.
    let mut mutated = successor.clone();
    mutated.roots.project = root('9');
    assert!(capsule.validate_successor(&mutated).is_err());
    let mut mutated = successor.clone();
    mutated.roots.task = root('9');
    assert!(capsule.validate_successor(&mutated).is_err());
    let mut mutated = successor.clone();
    mutated.roots.protected_scope = root('9');
    assert!(capsule.validate_successor(&mutated).is_err());
    let mut mutated = successor.clone();
    mutated.roots.fallback = root('9');
    assert!(capsule.validate_successor(&mutated).is_err());

    // Evolving roots may change freely between transitions.
    let mut mutated = successor.clone();
    mutated.roots.obligations = root('9');
    assert!(capsule.validate_successor(&mutated).is_ok());
    let mut mutated = successor.clone();
    mutated.roots.evidence = root('9');
    assert!(capsule.validate_successor(&mutated).is_ok());
    let mut mutated = successor.clone();
    mutated.roots.policy = root('9');
    assert!(capsule.validate_successor(&mutated).is_ok());
    let mut mutated = successor.clone();
    mutated.roots.execution = root('9');
    assert!(capsule.validate_successor(&mutated).is_ok());
    let mut mutated = successor.clone();
    mutated.roots.verifier = root('9');
    assert!(capsule.validate_successor(&mutated).is_ok());
    let mut mutated = successor.clone();
    mutated.roots.ledger = root('9');
    assert!(capsule.validate_successor(&mutated).is_ok());
}

#[test]
fn successor_epoch_must_stay_positive_and_nondecreasing() {
    let mut capsule = WorkCapsule::draft(roots(), 1, 0, 0).unwrap();
    capsule.epoch = 5;
    let mut successor = capsule.clone();
    successor.state = CapsuleState::EvidenceComplete;

    successor.epoch = 4;
    assert!(capsule.validate_successor(&successor).is_err());

    successor.epoch = 5;
    assert!(capsule.validate_successor(&successor).is_ok());

    successor.epoch = 6;
    assert!(capsule.validate_successor(&successor).is_ok());
}

#[test]
fn interrupt_scheduler_never_spends_native_reserve() {
    let interrupt = SemanticInterrupt {
        id: "choice".into(),
        kind: SemanticInterruptKind::ArchitectureChoice,
        capsule_root: root('a'),
        obligation_root: root('b'),
        decision_frontier_root: root('c'),
        evidence_view_root: root('d'),
        reasoning_contract_root: root('e'),
        continuation_root: root('f'),
        exact_handles: vec![],
        budget_impact: 11,
    };
    let schedule =
        schedule_next(MechanicalVerdict::Unknown, &[interrupt], 20, 10, true, None).unwrap();
    assert_eq!(schedule.action, ScheduleAction::NativeEscape);
    assert_eq!(schedule.reserved_native_budget, 10);
}

#[test]
fn proven_dominance_elides_native_fallback() {
    let output = root('a');
    let dominance = ZeroDominanceProof {
        capsule_root: root('b'),
        ledger_root: root('c'),
        baseline_output_root: output.clone(),
        zero_output_root: output,
        protected_regressions: 0,
        correctness_complete: true,
        baseline_visible_tokens: 100,
        zero_visible_tokens: 10,
        baseline_complete_work: 100,
        zero_complete_work: 20,
    };
    let schedule = schedule_next(
        MechanicalVerdict::Safe,
        &[],
        20,
        10,
        false,
        Some(&dominance),
    )
    .unwrap();
    assert_eq!(schedule.action, ScheduleAction::ContinueMechanical);
    assert_eq!(schedule.reserved_native_budget, 0);

    let mut insufficient = dominance.clone();
    insufficient.zero_visible_tokens = insufficient.baseline_visible_tokens;
    assert!(
        schedule_next(
            MechanicalVerdict::Safe,
            &[],
            20,
            10,
            false,
            Some(&insufficient),
        )
        .is_err()
    );

    let decision = choose_regime(&GovernorInput {
        reuse_valid: false,
        mechanical: MechanicalVerdict::Safe,
        dialect_verified: false,
        semantic_choice_required: false,
        saved_budget_available: true,
        baseline_available: true,
        zero_dominance: Some(dominance),
    });
    assert!(decision.fallback_elided);
    assert!(!decision.baseline_reserved);
}

#[test]
fn mechanical_and_promotion_gates_require_every_premise() {
    let evidence = MechanicalEvidence {
        deterministic: true,
        effects_verified: true,
        bounded: true,
        cancellable: true,
        transactional: true,
        proof_complete: true,
        native_fallback_available: false,
        has_unresolved_choice: false,
    };
    assert_eq!(evidence.verdict(), MechanicalVerdict::Safe);
    let promotion = PromotionEvidence {
        exact_parity: true,
        protected_regressions: 0,
        complete_resource_reconciled: true,
        hidden_model_calls: 0,
        model_calls_reconciled: true,
        fault_injection_passed: true,
        rollback_passed: true,
    };
    assert!(promotion.permits_promotion());
}

#[test]
fn promotion_inputs_are_checked_not_asserted() {
    let output = root('a');
    let rollback = root('b');
    let inputs = PromotionInputs {
        baseline_output_root: output.clone(),
        candidate_output_root: output,
        protected_regressions: 0,
        declared_resources: BTreeMap::from([("provider_tokens".into(), 10)]),
        observed_resources: BTreeMap::from([("provider_tokens".into(), 10)]),
        declared_model_calls: 1,
        observed_model_calls: 1,
        injected_faults: 2,
        contained_faults: 2,
        rollback_root_before: rollback.clone(),
        rollback_root_after: rollback,
    };
    assert!(inputs.evaluate().unwrap().permits_promotion());
}
