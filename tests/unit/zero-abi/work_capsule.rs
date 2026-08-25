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

#[test]
fn capsule_transitions_fail_closed() {
    let mut capsule = WorkCapsule {
        version: 1,
        roots: roots(),
        state: CapsuleState::Draft,
        epoch: 0,
        provider_usage_budget: 10,
        complete_work_budget: 20,
    };
    assert!(capsule.root().is_ok());
    assert!(capsule.transition(CapsuleState::EvidenceComplete).is_ok());
    assert!(capsule.transition(CapsuleState::Committed).is_err());
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
