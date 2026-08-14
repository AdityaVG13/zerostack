    use super::*;

    use AssetStateV1::*;

    fn digest(seed: u64) -> String {
        format!("{seed:064x}")
    }

    fn scope(operation_class: &str, seed: u64) -> ScopeV1 {
        ScopeV1::new(operation_class, digest(seed)).unwrap()
    }

    fn asset_captured() -> CapabilityAssetV1 {
        CapabilityAssetV1::captured(
            "asset:extract-facts",
            "project:alpha",
            scope("extract_facts", 1),
            vec![RootedPreconditionV1::new("root", digest(2)).unwrap()],
            vec!["read:sources".into()],
            vec!["write:cache".into()],
            vec!["effect:local".into()],
            "postcondition: facts extracted",
            "verifier:fixture",
            "successor: next_extract",
            "rollback: baseline_restore",
            vec![DependencyPinV1::new("sources", digest(3)).unwrap()],
            FreshnessPolicyV1::new(100, 7).unwrap(),
            CostV1 {
                capture_units: 5,
                maintenance_units_per_epoch: 30,
            },
        )
        .unwrap()
    }

    fn asset_in_state(state: AssetStateV1) -> CapabilityAssetV1 {
        let mut asset = asset_captured();
        match state {
            Captured => {}
            Shadow => {
                asset.enter_shadow().unwrap();
            }
            Promoted => {
                asset.enter_shadow().unwrap();
                asset.promote().unwrap();
            }
            Demoted => {
                asset.enter_shadow().unwrap();
                asset.demote().unwrap();
            }
            Revoked => {
                asset
                    .revoke_for(&RevocationTriggerV1::ContractChange, "test")
                    .unwrap();
            }
            Expired => {
                asset.enter_shadow().unwrap();
                asset.promote().unwrap();
                asset.expire().unwrap();
            }
        }
        asset
    }

    fn all_outcomes() -> Vec<MatchOutcomeV1> {
        vec![
            MatchOutcomeV1::Matched,
            MatchOutcomeV1::OutOfScope,
            MatchOutcomeV1::CrossProject,
            MatchOutcomeV1::StaleDependency {
                name: "sources".into(),
            },
            MatchOutcomeV1::ExpiredFreshness,
        ]
    }

    fn current_digests() -> Vec<(String, String)> {
        vec![("sources".to_string(), digest(3))]
    }

    /// CAP-001 acceptance: a captured asset is non-authoritative until
    /// separately proved. Exhaustive: every non-Promoted state x every match
    /// outcome is `BaselineRequired`; only Promoted AND Matched executes.
    #[test]
    fn captured_asset_is_never_authoritative() {
        for state in [Captured, Shadow, Demoted, Revoked, Expired] {
            let asset = asset_in_state(state);
            for outcome in all_outcomes() {
                match asset.use_asset(&outcome) {
                    UseOutcomeV1::BaselineRequired { added_cost_units } => {
                        assert!(added_cost_units > 0, "standing cost must be added");
                    }
                    UseOutcomeV1::Executable => {
                        panic!("{state:?} asset must never be executable for {outcome:?}")
                    }
                }
            }
        }
        // Promoted is executable ONLY when the task Matched.
        let asset = asset_in_state(Promoted);
        for outcome in all_outcomes() {
            let executable = matches!(asset.use_asset(&outcome), UseOutcomeV1::Executable);
            assert_eq!(
                executable,
                outcome == MatchOutcomeV1::Matched,
                "Promoted asset mis-executed for {outcome:?}"
            );
        }
    }

    /// CAP-002 acceptance: out-of-scope task does not match; changed
    /// dependency invalidates and revokes.
    #[test]
    fn out_of_scope_task_does_not_match_and_changed_dependency_invalidates() {
        let asset = asset_captured();
        // Wrong input shape digest -> OutOfScope (exact scope only).
        assert_eq!(
            match_task(&asset, &scope("extract_facts", 99), "project:alpha", 10, &current_digests()),
            MatchOutcomeV1::OutOfScope
        );
        // Wrong operation class -> OutOfScope.
        assert_eq!(
            match_task(&asset, &scope("summarize", 1), "project:alpha", 10, &current_digests()),
            MatchOutcomeV1::OutOfScope
        );
        // Cross-project -> CrossProject, never Matched (leakage kill).
        assert_eq!(
            match_task(&asset, &asset.scope, "project:other", 10, &current_digests()),
            MatchOutcomeV1::CrossProject
        );
        // Changed dependency digest -> StaleDependency.
        let stale = vec![("sources".to_string(), digest(99))];
        assert_eq!(
            match_task(&asset, &asset.scope, "project:alpha", 10, &stale),
            MatchOutcomeV1::StaleDependency {
                name: "sources".into()
            }
        );
        // Missing pinned dependency -> StaleDependency.
        assert_eq!(
            match_task(&asset, &asset.scope, "project:alpha", 10, &[]),
            MatchOutcomeV1::StaleDependency {
                name: "sources".into()
            }
        );
        // Revocation on dependency change records the trigger and reason.
        let mut asset = asset_captured();
        asset
            .revoke_for(
                &RevocationTriggerV1::DependencyChange {
                    name: "sources".into(),
                },
                "dependency digest changed",
            )
            .unwrap();
        assert_eq!(asset.state, Revoked);
        let reason = asset.revocation_reason.as_deref().unwrap();
        assert!(reason.contains("dependency_change:sources"));
        assert!(reason.contains("dependency digest changed"));
        // Revoked asset never matches into execution.
        let outcome = match_task(&asset, &asset.scope, "project:alpha", 10, &current_digests());
        assert!(matches!(
            asset.use_asset(&outcome),
            UseOutcomeV1::BaselineRequired { .. }
        ));
    }

    /// CAP-003 acceptance: the promotion report includes misses, regressions,
    /// strict rescues, and complete cost; any regression denies and demands
    /// demotion; insufficient trials deny; all-clean promotes.
    #[test]
    fn promotion_report_includes_misses_regressions_rescues_and_complete_cost() {
        let asset = asset_captured();
        let trial = |id: &str, miss: bool, regression: bool, rescue: bool, cost: u64| {
            ShadowTrialV1::new(
                id,
                &asset.asset_id,
                digest(10),
                if miss { digest(11) } else { digest(10) },
                regression,
                rescue,
                cost,
            )
            .unwrap()
        };

        let mixed = vec![
            trial("t1", false, false, true, 7),
            trial("t2", false, false, false, 7),
            trial("t3", true, false, false, 7),
            trial("t4", false, true, false, 7),
            trial("t5", false, false, false, 7),
        ];
        let decision = evaluate_promotion(&asset, &mixed, 5);
        let report = match &decision {
            PromotionDecisionV1::Promote { report } => {
                panic!("regression must deny promotion: {report:?}")
            }
            PromotionDecisionV1::Deny { report, reasons } => {
                assert!(
                    reasons
                        .iter()
                        .any(|reason| reason.starts_with("protected_regression")),
                    "negative transfer must be surfaced: {reasons:?}"
                );
                assert!(reasons.iter().any(|reason| reason.starts_with("shadow_miss")));
                report.clone()
            }
        };
        assert_eq!(report.trials_observed, 5);
        assert_eq!(report.misses, 1);
        assert_eq!(report.regressions, 1);
        assert_eq!(report.strict_rescues, 1);
        // Complete cost: capture 5 + maintenance 30*5 + trials 7*5 = 190.
        assert_eq!(report.complete_cost_units, 190);

        // Insufficient trials -> Deny.
        let decision = evaluate_promotion(&asset, &mixed[..3], 5);
        assert!(matches!(decision, PromotionDecisionV1::Deny { .. }));

        // All clean and >= min_trials -> Promote, complete cost accounted.
        let clean = vec![
            trial("c1", false, false, true, 7),
            trial("c2", false, false, true, 7),
        ];
        let decision = evaluate_promotion(&asset, &clean, 2);
        let PromotionDecisionV1::Promote { report } = decision else {
            panic!("clean trials must promote")
        };
        assert_eq!(report.trials_observed, 2);
        assert_eq!(report.misses, 0);
        assert_eq!(report.regressions, 0);
        assert_eq!(report.strict_rescues, 2);
        assert_eq!(report.complete_cost_units, 5 + 30 * 2 + 7 * 2);

        // Empty trials -> Deny even when min_trials is zero.
        let decision = evaluate_promotion(&asset, &[], 0);
        assert!(matches!(decision, PromotionDecisionV1::Deny { .. }));

        // Trials for other assets are ignored (never laundered in).
        let other = ShadowTrialV1::new(
            "x1",
            "asset:unrelated",
            digest(10),
            digest(10),
            false,
            false,
            1,
        )
        .unwrap();
        let decision = evaluate_promotion(&asset, &[other], 1);
        assert!(matches!(decision, PromotionDecisionV1::Deny { .. }));
    }

    /// CAP-004 acceptance: recording a syndrome for a Promoted asset forces
    /// demotion, and the store is append-only.
    #[test]
    fn syndrome_recording_demotes_promoted_asset_and_store_is_append_only() {
        let mut asset = asset_in_state(Promoted);
        let mut store = SyndromeStoreV1::new();
        store
            .record_for(
                SyndromeV1::new(
                    "synd:1",
                    &asset.asset_id,
                    "unexpected_failure",
                    12,
                    "shadow diverged at runtime",
                )
                .unwrap(),
                &mut asset,
            )
            .unwrap();
        assert_eq!(asset.state, Demoted);
        assert_eq!(store.syndromes_for(&asset.asset_id).len(), 1);

        // A syndrome on a Shadow asset also demotes.
        let mut shadowed = asset_in_state(Shadow);
        store
            .record_for(
                SyndromeV1::new("synd:2", &shadowed.asset_id, "trial_failure", 12, "d").unwrap(),
                &mut shadowed,
            )
            .unwrap();
        assert_eq!(shadowed.state, Demoted);

        // Append-only: records accumulate, nothing is deleted.
        store
            .record(SyndromeV1::new("synd:3", "asset:other", "x", 12, "d").unwrap())
            .unwrap();
        assert_eq!(store.syndromes.len(), 3);
        assert_eq!(store.syndromes_for("asset:other").len(), 1);

        // Duplicate syndrome ids are rejected.
        let duplicate = SyndromeV1::new("synd:1", "asset:other", "x", 12, "d").unwrap();
        assert!(store.record(duplicate).is_err());
    }

    /// CAP-005 acceptance: an expired or revoked asset adds cost but can
    /// never execute, even when the task itself matches exactly.
    #[test]
    fn expired_or_revoked_asset_adds_cost_but_cannot_execute() {
        for state in [Demoted, Revoked, Expired] {
            let asset = asset_in_state(state);
            let outcome = match_task(&asset, &asset.scope, "project:alpha", 10, &current_digests());
            assert_eq!(outcome, MatchOutcomeV1::Matched, "{state:?} scope still matches");
            match asset.use_asset(&outcome) {
                UseOutcomeV1::BaselineRequired { added_cost_units } => {
                    // Standing cost: capture 5 + one maintenance epoch 30.
                    assert_eq!(added_cost_units, 35, "{state:?}");
                }
                UseOutcomeV1::Executable => panic!("{state:?} asset must never execute"),
            }
        }
    }

    /// METRIC-009 acceptance: negative lifetime-value assets are retired
    /// automatically; positive-value assets are untouched.
    #[test]
    fn negative_lifetime_value_assets_demote_automatically() {
        let mut ledger = AssetValueLedgerV1::new();
        let mut bad = asset_in_state(Promoted);
        bad.asset_id = "asset:bad".into();
        let mut good = asset_in_state(Promoted);
        good.asset_id = "asset:good".into();
        let certified = CertifiedLowerBoundV1::new(scope("extract_facts", 1), 20, digest(9)).unwrap();

        ledger.record_capture(&bad).unwrap();
        ledger.record_capture(&good).unwrap();
        for epoch in 1..=3 {
            // bad: benefit 10, maintenance 30 -> negative per epoch.
            ledger.record_benefit(&bad, epoch, 10, 100, &certified).unwrap();
            // good: benefit 100, maintenance 30 -> positive per epoch.
            ledger.record_benefit(&good, epoch, 100, 200, &certified).unwrap();
        }
        assert!(ledger.lifetime_value("asset:bad") < 0);
        assert!(ledger.lifetime_value("asset:good") > 0);

        // threshold_epochs = 3: only the negative asset is retired.
        assert_eq!(ledger.retire_negative(3), vec!["asset:bad".to_string()]);
        // Below the observation threshold nothing is retired. The bad asset
        // spans 4 distinct epochs (capture at epoch 7 + benefits 1..=3).
        assert!(ledger.retire_negative(5).is_empty());

        let mut assets = vec![bad, good];
        let applied = ledger.apply_retirement(&mut assets, 3);
        assert_eq!(applied, vec!["asset:bad".to_string()]);
        assert_eq!(assets[0].state, Demoted);
        assert_eq!(assets[1].state, Promoted);
    }

    /// METRIC-010 acceptance: benefit claims are clamped to the certified
    /// unavoidable-work lower bound and fail closed when exceeded.
    #[test]
    fn benefit_claims_are_clamped_to_certified_lower_bound() {
        let mut ledger = AssetValueLedgerV1::new();
        let asset = asset_captured();
        let certified = CertifiedLowerBoundV1::new(asset.scope.clone(), 20, digest(9)).unwrap();

        // allowed = 100 - 20 = 80; claiming 90 fails closed and records 80.
        let err = ledger
            .record_benefit(&asset, 1, 90, 100, &certified)
            .unwrap_err();
        assert_eq!(
            err,
            AssetErrorV1::ClampedBenefit {
                claimed: 90,
                allowed: 80
            }
        );
        assert_eq!(ledger.entries.last().unwrap().benefit_units, 80);

        // Claiming within the bound is fine and recorded exactly.
        ledger.record_benefit(&asset, 2, 50, 100, &certified).unwrap();
        assert_eq!(ledger.entries.last().unwrap().benefit_units, 50);

        // A certified bound for a different scope is rejected.
        let other = CertifiedLowerBoundV1::new(scope("other", 7), 20, digest(8)).unwrap();
        assert!(matches!(
            ledger.record_benefit(&asset, 3, 10, 100, &other),
            Err(AssetErrorV1::ScopeMismatch(_))
        ));
    }

    /// CAP-001/003 acceptance: the state machine has NO path to Promoted
    /// except via Shadow, and terminal states have no edge toward Shadow or
    /// Promoted.
    #[test]
    fn asset_state_machine_has_no_path_to_promoted_except_via_shadow() {
        let states = [Captured, Shadow, Promoted, Demoted, Revoked, Expired];
        for from in states {
            for to in states {
                let allowed = allowed_asset_transition(from, to);
                let expected = matches!(
                    (from, to),
                    (Captured, Shadow)
                        | (Shadow, Promoted)
                        | (Shadow, Demoted)
                        | (Promoted, Demoted)
                        | (Promoted, Revoked)
                        | (Promoted, Expired)
                        | (_, Revoked)
                );
                assert_eq!(allowed, expected, "edge {from:?} -> {to:?}");
            }
        }
        // The only one-step edge toward Promoted is Shadow -> Promoted.
        for from in states {
            assert_eq!(
                allowed_asset_transition(from, Promoted),
                from == Shadow,
                "from {from:?}"
            );
        }
        // Terminal states have no edge toward Shadow or Promoted.
        for from in [Demoted, Revoked, Expired] {
            assert!(!allowed_asset_transition(from, Shadow), "{from:?}");
            assert!(!allowed_asset_transition(from, Promoted), "{from:?}");
        }
        // Direct construction/promotion from Captured is impossible.
        let mut asset = asset_captured();
        assert!(asset.promote().is_err());
        assert_eq!(asset.state, Captured);
        asset.enter_shadow().unwrap();
        assert!(asset.promote().is_ok());
        // Promoted cannot be re-promoted or re-entered.
        assert!(asset.promote().is_err());
        assert!(asset.enter_shadow().is_err());
    }

    // -- ZS-SEC-005: capability gate wired into the CAS read path ---------

    use tempfile::tempdir;
    use zero_store::CasReadGate as _;
    use zero_store::SharedCas;

    /// Cross-project guessed-root: the caller guesses another project's root
    /// content hash; the gate refuses before any object lookup.
    #[test]
    fn cas_gate_refuses_cross_project_guessed_root() {
        let asset = asset_in_state(Promoted);
        let gate = CasCapabilityGateV1::new(asset, "project:beta", 7).unwrap();
        let err = gate
            .authorize_read(&digest(1))
            .expect_err("cross-project read must be refused");
        assert!(format!("{err:?}").contains("cross-project"));
        assert_eq!(err.class(), "policy_denied");
    }

    /// A permit names the exact content hash it authorizes; mismatched
    /// content is refused fail-loud.
    #[test]
    fn cas_gate_refuses_content_the_asset_does_not_authorize() {
        let asset = asset_in_state(Promoted);
        let gate = CasCapabilityGateV1::new(asset, "project:alpha", 7).unwrap();
        let err = gate
            .authorize_read(&digest(99))
            .expect_err("content outside the authorized scope must be refused");
        assert!(format!("{err:?}").contains("not authorized"));
    }

    /// A stale capability (freshness lapsed even while state still says
    /// Promoted) is refused.
    #[test]
    fn cas_gate_refuses_stale_capability() {
        // asset_captured: captured_epoch 7, valid_epochs 100 -> expires at 107.
        let asset = asset_in_state(Promoted);
        let gate = CasCapabilityGateV1::new(asset, "project:alpha", 108).unwrap();
        let err = gate
            .authorize_read(&digest(1))
            .expect_err("stale capability must be refused");
        assert!(format!("{err:?}").contains("stale"));
    }

    /// A non-promoted asset never authorizes reads (CAP-001).
    #[test]
    fn cas_gate_refuses_non_promoted_asset() {
        for state in [Captured, Shadow, Demoted, Revoked, Expired] {
            let asset = asset_in_state(state);
            let gate = CasCapabilityGateV1::new(asset, "project:alpha", 7).unwrap();
            assert!(gate.authorize_read(&digest(1)).is_err(), "{state:?} must not authorize reads");
        }
    }

    /// Live permit end-to-end through a real CAS: Promoted + matching
    /// project + exact authorized content hash + fresh epoch reads the
    /// bytes; every other combination refuses with no bytes.
    #[test]
    fn cas_gate_live_permit_passes_through_the_store() {
        let dir = tempdir().unwrap();
        let cas = SharedCas::open(dir.path());
        let bytes = b"authorized project content";
        let hash = cas.put(bytes).unwrap();

        let mut asset = asset_captured();
        asset.scope = ScopeV1::new("extract_facts", hash.clone()).unwrap();
        asset.enter_shadow().unwrap();
        asset.promote().unwrap();

        let gate = CasCapabilityGateV1::new(asset, "project:alpha", 7).unwrap();
        assert_eq!(cas.get_verified_gated(&hash, &gate).unwrap(), bytes);

        // The same gate refuses a foreign object in the same store.
        let other_hash = cas.put(b"other project content").unwrap();
        assert!(matches!(
            cas.get_verified_gated(&other_hash, &gate),
            Err(zero_store::CasError::PolicyDenied(_))
        ));
    }
