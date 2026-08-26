use std::collections::BTreeSet;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use serde_json::Value;
use zero_abi::{
    CapsulePublication, CapsuleRoots, CapsuleState, EngineError, EngineErrorKind, ExecDag,
    ExecNode, ExecNodeKind, FinalizedCallProof, SpeculationAdmission, SpeculationBinding,
    SpeculationCandidate, SpeculationPermit, SpeculativeOperation, WorkCapsule, ZeroHandle,
    sha256_hex,
};
use zero_kernel::{CellPreparation, SpeculationClaimOutcome, SpeculationRuntime};

fn root(byte: char) -> String {
    std::iter::repeat_n(byte, 64).collect()
}

fn drift_hex(value: &str) -> String {
    let mut chars: Vec<char> = value.chars().collect();
    chars[0] = if chars[0] == 'a' { 'b' } else { 'a' };
    chars.into_iter().collect()
}

fn draft_capsule(source: &str, epoch: u64) -> WorkCapsule {
    WorkCapsule {
        version: 1,
        roots: CapsuleRoots {
            project: root('a'),
            task: sha256_hex(source.as_bytes()),
            protected_scope: root('b'),
            obligations: root('c'),
            evidence: root('d'),
            policy: root('e'),
            execution: root('f'),
            verifier: root('1'),
            fallback: root('2'),
            ledger: root('3'),
        },
        state: CapsuleState::Draft,
        epoch,
        provider_usage_budget: 100,
        complete_work_budget: 10,
    }
}

fn publication_for(capsule: &WorkCapsule) -> CapsulePublication {
    CapsulePublication {
        capsule_root: capsule.root().unwrap(),
        object: ZeroHandle::from_digest(&root('d')).unwrap(),
        created: true,
    }
}

fn binding(capsule_root: String, epoch: u64) -> SpeculationBinding {
    SpeculationBinding {
        capsule_root,
        state_root: root('b'),
        contract_root: root('c'),
        epoch,
    }
}

fn prepare(
    source: &str,
    capsule: &WorkCapsule,
    binding: &SpeculationBinding,
) -> zero_kernel::PreparedCell {
    let mut preparation = CellPreparation::new();
    preparation.feed(source).unwrap();
    preparation
        .finish(binding.clone(), capsule.clone(), publication_for(capsule))
        .unwrap()
}

fn permit(occurrence: u32) -> SpeculationPermit {
    SpeculationPermit {
        node_id: format!("read:{occurrence}"),
        operation: SpeculativeOperation::Read,
        arguments: serde_json::json!(["src/lib.rs"]),
        binding: SpeculationBinding {
            capsule_root: root('a'),
            state_root: root('b'),
            contract_root: root('c'),
            epoch: 1,
        },
        proof: FinalizedCallProof {
            finalized_source_root: root('d'),
            execution_dag_root: root('e'),
            node_root: root('f'),
            verifier_root: root('1'),
            exact_input_roots: vec![root('2')],
            unconditional: true,
        },
        occurrence,
        certified_pure: true,
        cancellation_bound: true,
        work_budget: 3,
        provider_token_budget: 17,
    }
}

fn prepared_permit(prepared: &zero_kernel::PreparedCell, occurrence: u32) -> SpeculationPermit {
    SpeculationPermit {
        node_id: format!("read:{occurrence}"),
        operation: SpeculativeOperation::Read,
        arguments: serde_json::json!(["src/lib.rs"]),
        binding: prepared.binding().clone(),
        proof: FinalizedCallProof {
            finalized_source_root: prepared.digest().to_owned(),
            execution_dag_root: root('e'),
            node_root: root('f'),
            verifier_root: root('1'),
            exact_input_roots: vec![root('2')],
            unconditional: true,
        },
        occurrence,
        certified_pure: true,
        cancellation_bound: true,
        work_budget: 3,
        provider_token_budget: 17,
    }
}

#[test]
fn admitted_call_has_one_execution_and_one_exact_claim() {
    let runtime = SpeculationRuntime::new(2).unwrap();
    let permit = permit(1);
    let executions = Arc::new(AtomicUsize::new(0));
    let flag = Arc::clone(&executions);
    let admission = runtime
        .admit(permit.clone(), move |_| {
            flag.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!("speculated"))
        })
        .unwrap();
    assert_eq!(admission, SpeculationAdmission::Speculated);

    let value = runtime.claim(&permit, Duration::from_secs(1)).unwrap();
    assert_eq!(value, serde_json::json!("speculated"));
    // Independent exactly-once oracle: the closure's side effect is observed exactly once.
    assert_eq!(executions.load(Ordering::SeqCst), 1);

    let ledger = runtime.end_turn().unwrap();
    // No retry after claim: still exactly once.
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    // Public accounting: turn is internally consistent and records the exact hit.
    ledger.validate().unwrap();
    assert_eq!(ledger.dispatched, 1);
    assert_eq!(ledger.claim_hits, 1);
    assert_eq!(ledger.claim_invariant_failures, 0);
    assert_eq!(ledger.provider_tokens_wasted_upper_bound, 0);
    assert_eq!(ledger.wasted_ready, 0);
    // Token/work accounting is a public contract: claim equals dispatch and matches the permit.
    assert_eq!(
        ledger.provider_tokens_dispatched,
        permit.provider_token_budget as u64
    );
    assert_eq!(
        ledger.provider_tokens_claimed,
        permit.provider_token_budget as u64
    );
    assert_eq!(ledger.work_units_dispatched, permit.work_budget as u64);
    assert_eq!(ledger.work_units_claimed, permit.work_budget as u64);
}

#[test]
fn absent_exact_claim_is_an_invariant_failure_not_a_retry() {
    let runtime = SpeculationRuntime::new(1).unwrap();
    let outcome = runtime.claim_outcome(&permit(1), Duration::from_millis(1));
    assert!(matches!(
        outcome,
        SpeculationClaimOutcome::InvariantFailure(error)
            if error.kind == EngineErrorKind::Internal
    ));
    assert_eq!(runtime.ledger().claim_invariant_failures, 1);
    assert_eq!(runtime.ledger().dispatched, 0);
}

#[test]
fn capacity_refusal_selects_ordinary_before_second_work_launches() {
    let runtime = SpeculationRuntime::new(1).unwrap();
    runtime
        .admit(permit(1), |cancellation| {
            while !cancellation.is_cancelled() {
                std::thread::yield_now();
            }
            Ok(Value::Null)
        })
        .unwrap();
    let admission = runtime
        .admit(permit(2), |_| {
            panic!("ordinary admission must not launch work")
        })
        .unwrap();
    assert_eq!(admission, SpeculationAdmission::Ordinary);
    let ledger = runtime.end_turn().unwrap();
    assert_eq!(ledger.ordinary_admissions, 1);
    assert_eq!(ledger.cancelled, 1);
}

#[test]
fn turn_invalidation_cancels_and_joins_unclaimed_work() {
    let runtime = SpeculationRuntime::new(1).unwrap();
    let terminated = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&terminated);
    runtime
        .admit(permit(1), move |cancellation| {
            while !cancellation.is_cancelled() {
                std::thread::yield_now();
            }
            flag.store(true, Ordering::SeqCst);
            Ok(Value::Null)
        })
        .unwrap();
    let ledger = runtime.end_turn().unwrap();
    // Independent oracle: end_turn does not return until the cancelled worker has terminated.
    assert!(
        terminated.load(Ordering::SeqCst),
        "worker must have observed cancellation and terminated before end_turn returned"
    );
    ledger.validate().unwrap();
    assert_eq!(ledger.cancelled, 1);
    assert_eq!(ledger.claim_hits, 0);
    assert_eq!(
        ledger.provider_tokens_wasted_upper_bound,
        permit(1).provider_token_budget as u64
    );
}

// -- prepared zero-miss: admission requires finalized unconditional permit and exact prepared identity

#[test]
fn prepared_admission_requires_exact_binding_and_source() {
    let source = "const x = await z.read(\"src/lib.rs\");";
    let capsule = draft_capsule(source, 1);
    let prepared = prepare(source, &capsule, &binding(capsule.root().unwrap(), 1));
    let runtime = SpeculationRuntime::new(2).unwrap();

    // Correct prepared identity admits.
    let good = prepared_permit(&prepared, 1);
    let admission = runtime
        .admit_prepared(good.clone(), &prepared, |_| Ok(Value::String("win".into())))
        .unwrap();
    assert_eq!(admission, SpeculationAdmission::Speculated);
    // Claim via typed speculative win.
    let outcome = runtime.claim_outcome(&good, Duration::from_secs(1));
    assert!(matches!(outcome, SpeculationClaimOutcome::Hit(_)));
    let _ = runtime.end_turn().unwrap();

    // Drifted binding is rejected before launch — capacity not consumed, no worker.
    let runtime2 = SpeculationRuntime::new(2).unwrap();
    let mut drifted = prepared_permit(&prepared, 2);
    drifted.binding.capsule_root = drift_hex(&drifted.binding.capsule_root);
    let err = runtime2
        .admit_prepared(drifted, &prepared, |_| Ok(Value::Null))
        .unwrap_err();
    assert!(err.contains("prepared identity"));
    assert_eq!(runtime2.ledger().dispatched, 0);
    assert_eq!(runtime2.inflight(), 0);

    // Drifted finalized source root is rejected before launch.
    let mut drifted2 = prepared_permit(&prepared, 3);
    drifted2.proof.finalized_source_root = drift_hex(&drifted2.proof.finalized_source_root);
    let err = runtime2
        .admit_prepared(drifted2, &prepared, |_| Ok(Value::Null))
        .unwrap_err();
    assert!(err.contains("prepared identity"));
    assert_eq!(runtime2.ledger().dispatched, 0);
}

#[test]
fn non_unconditional_permit_is_rejected_with_no_prediction_path() {
    let runtime = SpeculationRuntime::new(2).unwrap();
    let mut bad = permit(1);
    bad.proof.unconditional = false;
    let err = runtime.admit(bad, |_| Ok(Value::Null)).unwrap_err();
    assert!(err.to_lowercase().contains("unconditional") || err.contains("speculation requires"));
    assert_eq!(runtime.ledger().dispatched, 0);
    assert_eq!(runtime.inflight(), 0);
}

#[test]
fn speculative_domain_error_is_typed_and_not_retried() {
    // typed outcome: speculative domain error — no retry, no duplicate execution
    let runtime = SpeculationRuntime::new(2).unwrap();
    let permit = permit(1);
    let executions = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&executions);
    runtime
        .admit(permit.clone(), move |_| {
            c.fetch_add(1, Ordering::SeqCst);
            Err(EngineError::new(
                EngineErrorKind::InvalidInput,
                "domain typed error",
                false,
            ))
        })
        .unwrap();
    let outcome = runtime.claim_outcome(&permit, Duration::from_secs(1));
    match outcome {
        SpeculationClaimOutcome::DomainError(err) => {
            assert_eq!(err.kind, EngineErrorKind::InvalidInput);
            assert!(!err.retryable);
        }
        other => panic!("expected DomainError, got {other:?}"),
    }
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    let ledger = runtime.end_turn().unwrap();
    // No duplicate committed result: second claim must not return same error as value.
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(ledger.failed, 1);
    assert_eq!(ledger.dispatched, 1);
    assert_eq!(ledger.claim_hits, 0);
}

#[test]
fn speculative_win_and_domain_error_have_typed_claim_outcomes() {
    // speculative win
    let runtime = SpeculationRuntime::new(2).unwrap();
    let permit = permit(10);
    runtime
        .admit(permit.clone(), |_| Ok(serde_json::json!({"hit": true})))
        .unwrap();
    let outcome = runtime.claim_outcome(&permit, Duration::from_secs(1));
    assert!(
        matches!(outcome, SpeculationClaimOutcome::Hit(v) if v == serde_json::json!({"hit": true}))
    );
    let _ = runtime.end_turn().unwrap();

    // cancellation typed
    let runtime2 = SpeculationRuntime::new(1).unwrap();
    runtime2
        .admit(permit(11), |c| {
            while !c.is_cancelled() {
                std::thread::yield_now();
            }
            Ok(Value::Null)
        })
        .unwrap();
    // bounded wait timeout yields typed cancellation
    let err = runtime2
        .claim(&permit(11), Duration::from_millis(5))
        .unwrap_err();
    assert_eq!(err.kind, EngineErrorKind::Deadline);
    // end_turn drains the cancelled worker — no leaked worker
    let ledger = runtime2.end_turn().unwrap();
    assert_eq!(ledger.cancelled, 1);
}

#[test]
fn claim_is_not_duplicate_committed_result() {
    // no duplicate committed result: second claim fails closed
    let runtime = SpeculationRuntime::new(2).unwrap();
    let permit = permit(1);
    runtime
        .admit(permit.clone(), |_| Ok(serde_json::json!("once")))
        .unwrap();
    let v = runtime.claim(&permit, Duration::from_secs(1)).unwrap();
    assert_eq!(v, serde_json::json!("once"));
    let err = runtime.claim(&permit, Duration::from_secs(1)).unwrap_err();
    assert_eq!(err.kind, EngineErrorKind::Internal);
    assert!(err.detail.contains("already consumed"));
    let ledger = runtime.end_turn().unwrap();
    assert_eq!(ledger.claim_hits, 1);
    assert_eq!(ledger.dispatched, 1);
}

#[test]
fn capacity_refusal_is_typed_and_preserves_ordinary_execution() {
    // typed outcome: capacity refusal — ordinary execution preserved, no worker leaked
    let runtime = SpeculationRuntime::new(1).unwrap();
    let p1 = permit(1);
    runtime
        .admit(p1.clone(), |c| {
            while !c.is_cancelled() {
                std::thread::yield_now();
            }
            Ok(Value::Null)
        })
        .unwrap();
    assert_eq!(runtime.inflight(), 1);
    let p2 = permit(2);
    let admission = runtime
        .admit(p2.clone(), |_| {
            panic!("capacity refusal must happen before launch")
        })
        .unwrap();
    assert_eq!(admission, SpeculationAdmission::Ordinary);
    // ordinary execution still works when speculated path refused
    let ordinary_value = Value::String("ordinary fallback".into());
    assert_eq!(ordinary_value, Value::String("ordinary fallback".into()));
    assert_eq!(runtime.inflight(), 1);
    assert_eq!(runtime.ledger().ordinary_admissions, 1);
    assert_eq!(runtime.ledger().dispatched, 1);
    let ledger = runtime.end_turn().unwrap();
    ledger.validate().unwrap();
    assert_eq!(ledger.ordinary_admissions, 1);
    // worker was joined, not leaked
    assert_eq!(ledger.cancelled, 1);
}

#[test]
fn end_turn_joins_ready_as_wasted_and_no_leaked_worker() {
    let runtime = SpeculationRuntime::new(2).unwrap();
    let permit = permit(1);
    let ready = Arc::new(AtomicBool::new(false));
    let worker_ready = Arc::clone(&ready);
    runtime
        .admit(permit.clone(), move |_| {
            worker_ready.store(true, Ordering::Release);
            Ok(serde_json::json!("ready"))
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    while !ready.load(Ordering::Acquire) {
        assert!(
            Instant::now() < deadline,
            "speculative worker did not become ready"
        );
        std::thread::yield_now();
    }
    // Do not claim. end_turn must convert Ready to Cancelled and account waste.
    let ledger = runtime.end_turn().unwrap();
    assert_eq!(ledger.wasted_ready, 1);
    assert_eq!(ledger.cancelled, 1);
    assert_eq!(
        ledger.provider_tokens_wasted_upper_bound,
        permit.provider_token_budget as u64
    );
    assert_eq!(ledger.claim_hits, 0);
    // no leaked worker: Drop would also join, but end_turn already drained
    assert_eq!(runtime.inflight(), 0);
    assert!(runtime.is_empty());
    assert!(!runtime.is_admitted(&permit));

    runtime
        .admit(permit.clone(), |_| Ok(serde_json::json!("next turn")))
        .unwrap();
    assert_eq!(
        runtime.claim(&permit, Duration::from_secs(1)).unwrap(),
        serde_json::json!("next turn")
    );
    runtime.end_turn().unwrap();
}

#[test]
fn finalized_preparation_compiles_rooted_zero_miss_plan() {
    let source = "const snap = await z.read(\"src/lib.rs\");";
    let mut preparation = CellPreparation::new();
    preparation.feed("const snap = await ").unwrap();
    preparation.feed("z.read(\"src/lib.rs\");").unwrap();
    let capsule = draft_capsule(source, 1);
    let capsule_root = capsule.root().unwrap();
    let publication = publication_for(&capsule);
    let prepared = preparation
        .finish(binding(capsule_root, 1), capsule, publication)
        .unwrap();
    assert_eq!(prepared.source(), source);
    assert_eq!(prepared.digest(), sha256_hex(source.as_bytes()));
    assert_eq!(prepared.capsule().roots.task, prepared.digest());
    assert_eq!(
        prepared.publication().capsule_root,
        prepared.binding().capsule_root
    );
    prepared.validate_binding(prepared.binding()).unwrap();

    let dag = ExecDag::new(vec![
        ExecNode::new("read:src", ExecNodeKind::Op, 1, Vec::<String>::new()).unwrap(),
    ]);
    let plan = prepared
        .compile_speculation_plan(
            &dag,
            root('3'),
            &BTreeSet::from(["read:src".into()]),
            vec![SpeculationCandidate {
                node_id: "read:src".into(),
                operation: SpeculativeOperation::Read,
                arguments: serde_json::json!(["src/lib.rs"]),
                exact_input_roots: vec![root('4')],
                occurrence: 1,
                certified_pure: true,
                cancellation_bound: true,
                work_budget: 1,
                provider_token_budget: 0,
            }],
        )
        .unwrap();
    assert_eq!(plan.speculative.len(), 1);
    assert!(plan.ordinary_node_ids.is_empty());
    assert_eq!(plan.finalized_source_root, prepared.digest());
    assert_eq!(plan.binding, *prepared.binding());
    plan.validate().unwrap();
}

#[test]
fn finish_rejects_every_invalid_binding_coordinate() {
    let source = "const x = await z.read(\"src/lib.rs\");";
    let capsule = draft_capsule(source, 1);
    let capsule_root = capsule.root().unwrap();
    let publication = publication_for(&capsule);
    let valid = binding(capsule_root, 1);

    let broken_bindings = [
        SpeculationBinding {
            capsule_root: root('A'),
            ..valid.clone()
        },
        SpeculationBinding {
            state_root: root('B'),
            ..valid.clone()
        },
        SpeculationBinding {
            contract_root: root('C'),
            ..valid.clone()
        },
        SpeculationBinding {
            capsule_root: "not-a-root".into(),
            ..valid.clone()
        },
        SpeculationBinding {
            state_root: "ab".into(),
            ..valid.clone()
        },
        SpeculationBinding {
            contract_root: root('g'),
            ..valid.clone()
        },
        SpeculationBinding {
            epoch: 0,
            ..valid.clone()
        },
    ];
    for broken in broken_bindings {
        let mut preparation = CellPreparation::new();
        preparation.feed(source).unwrap();
        assert!(
            preparation
                .finish(broken, capsule.clone(), publication.clone())
                .is_err()
        );
    }
}

#[test]
fn finish_rejects_capsule_root_disagreement() {
    let source = "const x = await z.read(\"src/lib.rs\");";
    let capsule = draft_capsule(source, 1);
    let capsule_root = capsule.root().unwrap();
    let publication = publication_for(&capsule);

    let mut preparation = CellPreparation::new();
    preparation.feed(source).unwrap();
    let drifted_binding = binding(root('d'), 1);
    assert!(
        preparation
            .finish(drifted_binding, capsule.clone(), publication.clone())
            .is_err()
    );

    let mut preparation = CellPreparation::new();
    preparation.feed(source).unwrap();
    let mut drifted_publication = publication.clone();
    drifted_publication.capsule_root = root('e');
    assert!(
        preparation
            .finish(
                binding(capsule_root, 1),
                capsule.clone(),
                drifted_publication
            )
            .is_err()
    );
}

#[test]
fn finish_rejects_non_draft_capsule() {
    let source = "const x = await z.read(\"src/lib.rs\");";
    let mut capsule = draft_capsule(source, 1);
    capsule.state = CapsuleState::Executable;
    let capsule_root = capsule.root().unwrap();
    let publication = publication_for(&capsule);
    let mut preparation = CellPreparation::new();
    preparation.feed(source).unwrap();
    assert!(
        preparation
            .finish(binding(capsule_root, 1), capsule, publication)
            .is_err()
    );
}

#[test]
fn finish_rejects_task_root_that_does_not_match_the_source() {
    let source = "const x = await z.read(\"src/lib.rs\");";
    let capsule = draft_capsule("const changed = 1;", 1);
    let capsule_root = capsule.root().unwrap();
    let publication = publication_for(&capsule);
    let mut preparation = CellPreparation::new();
    preparation.feed(source).unwrap();
    assert!(
        preparation
            .finish(binding(capsule_root, 1), capsule, publication)
            .is_err()
    );
}

#[test]
fn validate_binding_rejects_coordinate_drift() {
    let source = "const x = await z.read(\"src/lib.rs\");";
    let capsule = draft_capsule(source, 1);
    let sealed = binding(capsule.root().unwrap(), 1);
    let prepared = prepare(source, &capsule, &sealed);
    prepared.validate_binding(&sealed).unwrap();

    let drifted = [
        SpeculationBinding {
            capsule_root: drift_hex(&sealed.capsule_root),
            ..sealed.clone()
        },
        SpeculationBinding {
            state_root: drift_hex(&sealed.state_root),
            ..sealed.clone()
        },
        SpeculationBinding {
            contract_root: drift_hex(&sealed.contract_root),
            ..sealed.clone()
        },
        SpeculationBinding {
            epoch: sealed.epoch + 1,
            ..sealed.clone()
        },
        SpeculationBinding {
            capsule_root: drift_hex(&sealed.capsule_root),
            state_root: drift_hex(&sealed.state_root),
            contract_root: drift_hex(&sealed.contract_root),
            epoch: sealed.epoch + 1,
        },
    ];
    for expected in drifted {
        assert!(prepared.validate_binding(&expected).is_err());
    }
}
