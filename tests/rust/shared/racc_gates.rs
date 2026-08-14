use zerostack_shared_tests::checks::CheckStatus;
use zerostack_shared_tests::fake_substrate::RaccFakeSubstrate;
use zerostack_shared_tests::racc::{
    BudgetMutation, CertificateMutation, Charges, ReceiptMutation, RegressionEvidence,
    ReleaseEvidence, ResidencyMutation, TaskAcceptanceReceiptDocument, TaskReleaseEvidence,
    TaskTransactionMutation, check_budget, check_cert, check_inline, check_irreversible_gate,
    check_receipt, check_release_aggregate, check_residency, check_task_transaction, digest_hex,
    run_racc_suite,
};

fn assert_fails(result: zerostack_shared_tests::racc::RaccCheckResult) {
    assert_eq!(
        result.status,
        CheckStatus::Fail,
        "mutation unexpectedly passed: {}",
        result.detail
    );
}

#[test]
fn prior_six_and_task_transaction_gate_pass_on_the_honest_fake() {
    let report = run_racc_suite(&mut RaccFakeSubstrate::default());
    assert!(report.all_pass(), "{report:#?}");
    assert_eq!(
        serde_json::to_value(&report.checks).unwrap()[0]["id"],
        "RACC-CERT"
    );
    assert_eq!(report.checks.len(), 7);
}

#[test]
fn certificate_gate_rejects_every_required_mutation_class() {
    for mutation in [
        CertificateMutation::OmitPayload,
        CertificateMutation::ExtraPayload,
        CertificateMutation::StaleIndex,
        CertificateMutation::StaleParser,
        CertificateMutation::StaleOperator,
        CertificateMutation::WrongDomain,
        CertificateMutation::WrongQueryParameters,
        CertificateMutation::WrongWitnessKind,
    ] {
        let mut fake = RaccFakeSubstrate {
            certificate_mutation: mutation,
            ..Default::default()
        };
        assert_fails(check_cert(&mut fake));
    }
}

#[test]
fn receipt_gate_rejects_identity_arithmetic_and_every_hidden_charge() {
    for mutation in [
        ReceiptMutation::ReplayIdentity,
        ReceiptMutation::PhaseArithmetic,
        ReceiptMutation::OmitFailedTrials,
        ReceiptMutation::OmitRetries,
        ReceiptMutation::OmitVerificationCalls,
        ReceiptMutation::OmitRecoveryCalls,
        ReceiptMutation::OmitExpansions,
        ReceiptMutation::OmitFailedExpansions,
        ReceiptMutation::OmitFallbackCharges,
    ] {
        let mut fake = RaccFakeSubstrate {
            receipt_mutation: mutation,
            ..Default::default()
        };
        assert_fails(check_receipt(&mut fake));
    }
}

#[test]
fn irreversible_gate_rejects_a_committed_unproven_effect() {
    let mut fake = RaccFakeSubstrate {
        skip_irreversible_gate: true,
        ..Default::default()
    };
    assert_fails(check_irreversible_gate(&mut fake));
}

#[test]
fn budget_gate_rejects_nonnested_and_underreported_expansion_cost() {
    for mutation in [BudgetMutation::Nonnested, BudgetMutation::UnderreportedCost] {
        let mut fake = RaccFakeSubstrate {
            budget_mutation: mutation,
            ..Default::default()
        };
        assert_fails(check_budget(&mut fake));
    }
}

#[test]
fn inline_gate_rejects_a_second_certificate_fetch() {
    let mut fake = RaccFakeSubstrate {
        second_certificate_fetch: true,
        ..Default::default()
    };
    assert_fails(check_inline(&mut fake));
}

#[test]
fn residency_gate_rejects_corruption_and_silent_miss() {
    for mutation in [ResidencyMutation::Corruption, ResidencyMutation::SilentMiss] {
        let mut fake = RaccFakeSubstrate {
            residency_mutation: mutation,
            ..Default::default()
        };
        assert_fails(check_residency(&mut fake));
    }
}

fn release_fixture(fake: bool) -> ReleaseEvidence {
    let accounting = Charges {
        successful_trials: 3,
        failed_trials: 2,
        retries: 2,
        verification_calls: 5,
        recovery_calls: 2,
        expansions: 3,
        failed_expansions: 2,
        fallback_charges: 3,
    };
    ReleaseEvidence {
        target_identity: "release-target-v1".into(),
        target_digest: digest_hex(b"release-target-v1"),
        preregistered_before_evaluation: true,
        tasks: vec![
            TaskReleaseEvidence {
                task_id: "task-paired".into(),
                raw_cost: 100,
                compressed_cost: 75,
                evidence: RegressionEvidence::PoweredPaired {
                    powered: true,
                    no_regression: true,
                },
            },
            TaskReleaseEvidence {
                task_id: "task-t13".into(),
                raw_cost: 80,
                compressed_cost: 80,
                evidence: RegressionEvidence::T13NoRegret {
                    receipt: TaskAcceptanceReceiptDocument {
                        schema_version: 1,
                        task_id: "task-t13".into(),
                        verifier_command_id: 41,
                        verifier_environment_digest: digest_hex(b"release-env"),
                        outcome: "passed".into(),
                        exit_code: 0,
                        expected_artifact_digests: vec![digest_hex(b"artifact")],
                        observed_artifact_digests: vec![digest_hex(b"artifact")],
                        journal_id: digest_hex(b"journal"),
                        attempt_cost: 5,
                    },
                },
            },
        ],
        expected_accounting: accounting.clone(),
        reported_accounting: accounting,
        hub_fake_substrate: fake,
    }
}

#[test]
fn release_aggregate_preserves_paper_12_2_and_labels_fake_green() {
    let fake = check_release_aggregate(&release_fixture(true));
    assert_eq!(fake.status, CheckStatus::Pass);
    assert!(!fake.production_release_pass);
    assert!(fake.detail.contains("not a production release pass"));
    assert_eq!(
        fake.task_ratios,
        vec![("task-paired".into(), 75, 100), ("task-t13".into(), 80, 80)]
    );

    let production = check_release_aggregate(&release_fixture(false));
    assert!(production.production_release_pass);
}

#[test]
fn release_aggregate_rejects_unfixed_target_regression_and_hidden_accounting() {
    let mut target = release_fixture(false);
    target.preregistered_before_evaluation = false;
    assert_eq!(check_release_aggregate(&target).status, CheckStatus::Fail);

    let mut regression = release_fixture(false);
    regression.tasks[0].evidence = RegressionEvidence::PoweredPaired {
        powered: false,
        no_regression: true,
    };
    assert_eq!(
        check_release_aggregate(&regression).status,
        CheckStatus::Fail
    );

    let mut hidden = release_fixture(false);
    hidden.reported_accounting.failed_trials = 0;
    assert_eq!(check_release_aggregate(&hidden).status, CheckStatus::Fail);
}

#[test]
fn task_transaction_gate_accepts_objective_commit_and_charged_rollback() {
    assert_eq!(
        check_task_transaction(&mut RaccFakeSubstrate::default()).status,
        CheckStatus::Pass
    );
}

#[test]
fn task_transaction_gate_rejects_missing_charge_receipt_and_irreversible_speculation() {
    for mutation in [
        TaskTransactionMutation::MissingCharge,
        TaskTransactionMutation::MissingReceiptCommit,
        TaskTransactionMutation::AllowIrreversible,
    ] {
        let mut fake = RaccFakeSubstrate {
            task_transaction_mutation: mutation,
            ..Default::default()
        };
        assert_fails(check_task_transaction(&mut fake));
    }
}

#[test]
fn z4_native_counter_receipts_cover_every_supported_profile() {
    let model: serde_json::Value =
        serde_json::from_str(include_str!("../../../conformance/models/causal-work-v3.json")).unwrap();
    let supported = model["native_counter_profiles"]["supported"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    let receipts = [
        (
            "macos",
            include_str!("../../../conformance/models/causal-work-v3-native-macos-receipt.json"),
        ),
        (
            "linux",
            include_str!("../../../conformance/models/causal-work-v3-native-linux-receipt.json"),
        ),
        (
            "windows",
            include_str!("../../../conformance/models/causal-work-v3-native-windows-receipt.json"),
        ),
    ];
    assert_eq!(
        supported,
        receipts
            .iter()
            .map(|(profile, _)| *profile)
            .collect::<std::collections::BTreeSet<_>>()
    );
    for (profile, json) in receipts {
        let receipt: serde_json::Value = serde_json::from_str(json).unwrap();
        let correspondence = &receipt["correspondence"];
        let collection = &receipt["dsr_collection_receipt"];
        assert_eq!(receipt["schema_version"], 1);
        assert_eq!(
            receipt["model_or_spec_version"],
            "zerostack.causal_work.native_counter.v1"
        );
        assert_eq!(
            receipt["source_repository_heads"]["ZeroStack"],
            "788821b82465835daef9c3ad6341f34da70a86be"
        );
        assert_eq!(
            receipt["execution_authority"]["source_tree_digest"],
            "0620283f80efbc3cef015fbf4ded90f492ad9bd2310eb188d72fa4c17ca0b0ce"
        );
        assert_eq!(receipt["platform_profile"]["os"], profile);
        assert_eq!(receipt["platform_profile"]["native_evidence"], true);
        assert_eq!(receipt["result"]["status"], "passed_native");
        assert_eq!(receipt["result"]["parent_delta"], 4_096);
        assert_eq!(receipt["result"]["adapter_delta"], 4_096);
        assert_eq!(correspondence["evidence_mode"], "native");
        assert_eq!(correspondence["platform_profile"], profile);
        assert_eq!(correspondence["parent_window"]["start"], 0);
        assert_eq!(correspondence["parent_window"]["end"], 4_096);
        assert_eq!(correspondence["adapter_observed_delta"], 4_096);
        assert_eq!(
            correspondence["identity"],
            correspondence["parent_window"]["identity"]
        );
        assert_eq!(
            correspondence["adapter_binary_digest"],
            receipt["output_artifact_hashes"]["adapter_binary"]
        );
        assert_eq!(
            collection["artifact_sha256"],
            correspondence["adapter_binary_digest"]
        );
        assert_eq!(collection["status"], "success");
        assert_eq!(collection["exit_code"], 0);
        assert_eq!(collection["method"], "native");
    }
}
