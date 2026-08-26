use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::Value;
use zero_abi::{
    CapsulePublication, CapsuleRoots, CapsuleState, EngineErrorKind, ExecDag, ExecNode,
    ExecNodeKind, FinalizedCallProof, SpeculationAdmission, SpeculationBinding,
    SpeculationCandidate, SpeculationPermit, SpeculativeOperation, WorkCapsule, ZeroHandle,
    sha256_hex,
};
use zero_kernel::{CellPreparation, SpeculationRuntime};

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

#[test]
fn admitted_call_has_one_execution_and_one_exact_claim() {
    let runtime = SpeculationRuntime::new(2).unwrap();
    let permit = permit(1);
    let admission = runtime
        .admit(permit.clone(), |_| Ok(serde_json::json!("speculated")))
        .unwrap();
    assert_eq!(admission, SpeculationAdmission::Speculated);

    let value = runtime.claim(&permit, Duration::from_secs(1)).unwrap();
    assert_eq!(value, serde_json::json!("speculated"));
    let ledger = runtime.end_turn().unwrap();
    assert_eq!(ledger.dispatched, 1);
    assert_eq!(ledger.claim_hits, 1);
    assert_eq!(ledger.claim_invariant_failures, 0);
    assert_eq!(ledger.provider_tokens_dispatched, 17);
    assert_eq!(ledger.provider_tokens_claimed, 17);
    assert_eq!(ledger.provider_tokens_wasted_upper_bound, 0);
    assert_eq!(ledger.work_units_dispatched, ledger.work_units_claimed);
}

#[test]
fn absent_exact_claim_is_an_invariant_failure_not_a_retry() {
    let runtime = SpeculationRuntime::new(1).unwrap();
    let error = runtime
        .claim(&permit(1), Duration::from_millis(1))
        .unwrap_err();
    assert_eq!(error.kind, EngineErrorKind::Internal);
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
    runtime
        .admit(permit(1), |cancellation| {
            while !cancellation.is_cancelled() {
                std::thread::yield_now();
            }
            Ok(Value::Null)
        })
        .unwrap();
    let ledger = runtime.end_turn().unwrap();
    assert_eq!(ledger.cancelled, 1);
    assert_eq!(ledger.claim_hits, 0);
    assert_eq!(ledger.provider_tokens_wasted_upper_bound, 17);
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

#[test]
fn compiled_plan_cannot_be_swayed_by_a_divergent_caller_binding() {
    let source = "const x = await z.read(\"src/lib.rs\");";
    let capsule = draft_capsule(source, 1);
    let sealed = binding(capsule.root().unwrap(), 1);
    let prepared = prepare(source, &capsule, &sealed);

    let divergent = SpeculationBinding {
        capsule_root: root('a'),
        state_root: root('b'),
        contract_root: root('c'),
        epoch: 9,
    };
    assert!(prepared.validate_binding(&divergent).is_err());

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
    assert_eq!(plan.binding, sealed);
    assert_eq!(plan.finalized_source_root, prepared.digest());
    plan.validate().unwrap();
}
