    use super::*;
    fn input(effect_class: EffectClass, required_budget: u128) -> GateInput<'static, 'static> {
        GateInput {
            effect_class,
            required_budget,
            verified_evidence: None,
            task_receipt: None,
        }
    }
    #[test]
    fn fixed_transition_table() {
        let s = GateState::new(4).unwrap();
        let (terminal, gate) = decide(s, input(EffectClass::ReadOnly, 4)).unwrap();
        assert!(matches!(gate, DecisionGate::RawFallback));
        assert_eq!(
            decide(terminal, input(EffectClass::ReadOnly, 5)).unwrap_err(),
            GateError::TerminalState
        );
        let (expanded, gate) = decide(s, input(EffectClass::ReversibleMutation, 5)).unwrap();
        assert!(matches!(
            gate,
            DecisionGate::Expand(NextBudget { budget: 8, .. })
        ));
        assert_eq!(expanded.cumulative_visible_cost(), 12);
        let (_, gate) = decide(s, input(EffectClass::Irreversible, u128::MAX)).unwrap();
        assert!(matches!(gate, DecisionGate::RawFallback));
    }
    struct Accept;
    impl TaskAcceptanceVerifier for Accept {
        fn verify_run(&self, _: &TaskRunEvidence) -> Result<(), TaskVerifierError> {
            Ok(())
        }
    }
    struct Reject;
    impl TaskAcceptanceVerifier for Reject {
        fn verify_run(&self, _: &TaskRunEvidence) -> Result<(), TaskVerifierError> {
            Err(TaskVerifierError::UntrustedRunEvidence)
        }
    }
    fn run(exit_code: i32, observed: Vec<[u8; 32]>, cost: u64) -> TaskRunEvidence {
        TaskRunEvidence::new(
            7,
            CommandId(11),
            [2; 32],
            exit_code,
            vec![[3; 32]],
            observed,
            [4; 32],
            cost,
        )
    }
    fn attempt(evidence: TaskRunEvidence) -> SandboxAttempt {
        begin_task_attempt(EffectClass::ReversibleMutation, evidence).unwrap()
    }

    #[test]
    fn objective_verifier_mints_complete_passing_receipt_and_commit() {
        let verified = verify_task_acceptance(&Accept, attempt(run(0, vec![[3; 32]], 9))).unwrap();
        let receipt = verified.receipt();
        assert_eq!(receipt.task_id(), 7);
        assert_eq!(receipt.verifier(), CommandId(11));
        assert_eq!(receipt.verifier_environment_digest(), &[2; 32]);
        assert_eq!(receipt.outcome(), TaskOutcome::Passed);
        assert_eq!(receipt.exit_code(), 0);
        assert_eq!(receipt.expected_artifact_digests(), &[[3; 32]]);
        assert_eq!(receipt.observed_artifact_digests(), &[[3; 32]]);
        assert_eq!(receipt.journal_id(), &[4; 32]);
        assert_eq!(receipt.attempt_cost(), 9);
        let gate_input = GateInput {
            effect_class: EffectClass::ReversibleMutation,
            required_budget: 4,
            verified_evidence: None,
            task_receipt: Some(verified.into_receipt()),
        };
        let (_, gate) = decide(GateState::new(4).unwrap(), gate_input).unwrap();
        let DecisionGate::TaskVerified(receipt) = gate else {
            panic!("expected task receipt")
        };
        assert_eq!(receipt.task_id(), 7);
    }

    #[test]
    fn verifier_failures_and_missing_receipts_rollback_with_cost() {
        let rejected =
            verify_task_acceptance(&Reject, attempt(run(0, vec![[3; 32]], 7))).unwrap_err();
        assert_eq!(
            rejected.reason(),
            TaskAcceptanceError::VerifierRejected(TaskVerifierError::UntrustedRunEvidence)
        );
        assert_eq!(rejected.rollback().attempt_cost(), 7);
        let nonzero =
            verify_task_acceptance(&Accept, attempt(run(2, vec![[3; 32]], 8))).unwrap_err();
        assert_eq!(
            nonzero.reason(),
            TaskAcceptanceError::NonZeroOutcome { exit_code: 2 }
        );
        assert_eq!(
            nonzero.rollback().reason(),
            RollbackReason::VerificationFailed(TaskAcceptanceError::NonZeroOutcome {
                exit_code: 2
            })
        );
        let mismatch =
            verify_task_acceptance(&Accept, attempt(run(0, vec![[9; 32]], 10))).unwrap_err();
        assert!(matches!(
            mismatch.reason(),
            TaskAcceptanceError::ArtifactMismatch { index: 0, .. }
        ));
        assert_eq!(mismatch.rollback().attempt_cost(), 10);
        let missing = attempt(run(0, vec![[3; 32]], 11)).rollback_missing_receipt();
        assert_eq!(missing.reason(), RollbackReason::MissingReceipt);
        assert_eq!(missing.attempt_cost(), 11);
    }

    #[test]
    fn irreversible_speculation_is_typed_rejection_even_with_receipt() {
        assert_eq!(
            begin_task_attempt(EffectClass::Irreversible, run(0, vec![[3; 32]], 1)).unwrap_err(),
            SpeculationError::IrreversibleEffect
        );
        let receipt = verify_task_acceptance(&Accept, attempt(run(0, vec![[3; 32]], 1)))
            .unwrap()
            .into_receipt();
        let gate_input = GateInput {
            effect_class: EffectClass::Irreversible,
            required_budget: u128::MAX,
            verified_evidence: None,
            task_receipt: Some(receipt),
        };
        assert_eq!(
            decide(GateState::new(4).unwrap(), gate_input).unwrap_err(),
            GateError::IrreversibleSpeculation
        );
    }

    #[test]
    fn attempts_are_nonzero_cost_and_artifacts_are_bounded() {
        assert_eq!(
            begin_task_attempt(EffectClass::ReadOnly, run(0, vec![[3; 32]], 0)).unwrap_err(),
            SpeculationError::ZeroAttemptCost
        );
        let too_many = vec![[3; 32]; MAX_TASK_ARTIFACTS + 1];
        assert_eq!(
            begin_task_attempt(
                EffectClass::ReadOnly,
                TaskRunEvidence::new(
                    7,
                    CommandId(1),
                    [2; 32],
                    0,
                    too_many.clone(),
                    too_many,
                    [4; 32],
                    1
                )
            )
            .unwrap_err(),
            SpeculationError::TooManyArtifacts {
                count: MAX_TASK_ARTIFACTS + 1,
                maximum: MAX_TASK_ARTIFACTS
            }
        );
    }

    #[test]
    fn irreversible_without_proof_is_immediate_terminal_fallback() {
        for required_budget in [1, u128::MAX] {
            let (state, gate) = decide(
                GateState::new(4).unwrap(),
                input(EffectClass::Irreversible, required_budget),
            )
            .unwrap();
            assert_eq!(state.phase(), GatePhase::Terminal);
            assert!(matches!(gate, DecisionGate::RawFallback));
        }
    }

    #[test]
    fn geometric_and_nonmonotone_demands_obey_bound() {
        for demands in [
            [2, 3, 5, 9, 17],
            [33, 3, 65, 2, 129],
            [1025, 17, 2049, 1, 4097],
        ] {
            let mut state = GateState::new(2).unwrap();
            let mut high = 2;
            for demand in demands {
                high = high.max(demand);
                while demand > state.current_budget() {
                    (state, _) = decide(state, input(EffectClass::ReadOnly, demand)).unwrap();
                }
            }
            assert!(check_t10_bound(state, high, 7).unwrap().holds);
        }
    }
    #[test]
    fn edge_errors_are_typed() {
        assert_eq!(GateState::new(0), Err(GateError::ZeroInitialBudget));
        let state = GateState {
            initial_budget: 1,
            current_budget: u128::MAX - 1,
            cumulative_visible_cost: 1,
            rounds: 1,
            phase: GatePhase::Active,
        };
        assert_eq!(
            decide(state, input(EffectClass::ReadOnly, u128::MAX)).unwrap_err(),
            GateError::BudgetOverflow
        );
        assert_eq!(ceil_log2_ratio(1, 0), Err(GateError::ZeroInitialBudget));
        assert_eq!(
            check_t10_bound(GateState::new(1).unwrap(), 0, 0),
            Err(GateError::ZeroHindsightBudget)
        );
    }
