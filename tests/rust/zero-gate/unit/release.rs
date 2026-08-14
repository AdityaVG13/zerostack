    use super::*;

    fn gates_all_true() -> std::collections::BTreeMap<String, bool> {
        PUBLIC_CLAIM_GATES
            .iter()
            .map(|gate| (gate.to_string(), true))
            .collect()
    }

    fn claim(formulation: &str) -> PublicClaimV1 {
        PublicClaimV1::new(
            "claim:q99-v6",
            formulation,
            gates_all_true(),
            vec!["fz://blob/benchmark-evidence".into(), "fz://blob/checksums".into()],
            Some(1_800_000_000_000),
            "zerostack.racc.v6",
        )
        .unwrap()
    }

    fn current_table() -> SupersessionTableV1 {
        SupersessionTableV1::new(vec![
            SupersessionRecordV1::new(
                "draft4_rewrite_formula",
                Some("draft5_rewrite_formula".into()),
                "rewrite formula corrected in Draft 5",
            )
            .unwrap(),
            SupersessionRecordV1::new(
                "model_internal_one_token_framing",
                Some("model_visible_token_framing".into()),
                "one-token framing must be model-visible",
            )
            .unwrap(),
            SupersessionRecordV1::new(
                "ambiguous_q99_percentage",
                Some("q99_demand_weighted_percentage".into()),
                "Q99 percentage must be demand-weighted with declared denominator",
            )
            .unwrap(),
        ])
        .unwrap()
    }

    /// BENCH-009 acceptance: a claim passing every gate with a current
    /// formulation and complete artifacts/scope/date is approved.
    #[test]
    fn complete_claim_with_current_formulation_passes() {
        let verdict = ReleaseCheckerV1::check(&claim("draft6_current"), &current_table()).unwrap();
        assert!(verdict.approved, "{:?}", verdict.reasons);
        assert!(verdict.reasons.is_empty());
    }

    /// BENCH-009 acceptance: release fails when ANY required artifact, claim
    /// scope, or provider-fact date is absent -- nothing is inferred.
    #[test]
    fn missing_artifact_scope_or_date_fails_release() {
        let table = current_table();
        let mut no_artifacts = claim("draft6_current");
        no_artifacts.required_artifacts = vec![];
        let verdict = ReleaseCheckerV1::check(&no_artifacts, &table).unwrap();
        assert!(!verdict.approved);
        assert!(verdict.reasons.contains(&"required_artifacts_missing".into()));

        let mut no_date = claim("draft6_current");
        no_date.provider_fact_date_unix_ms = None;
        let verdict = ReleaseCheckerV1::check(&no_date, &table).unwrap();
        assert!(!verdict.approved);
        assert!(verdict.reasons.contains(&"provider_fact_date_missing".into()));

        // An absent claim scope is structurally invalid: the checker rejects
        // the claim before any verdict (release fails closed either way).
        let no_scope = PublicClaimV1 {
            claim_scope: String::new(),
            ..claim("draft6_current")
        };
        assert_eq!(
            ReleaseCheckerV1::check(&no_scope, &table),
            Err(ReleaseErrorV1::InvalidClaim(
                "claim_scope must be nonempty".into()
            ))
        );
    }

    /// BENCH-009 acceptance: any unsatisfied gate fails the release.
    #[test]
    fn any_unsatisfied_gate_fails_release() {
        let table = current_table();
        for gate in PUBLIC_CLAIM_GATES {
            let mut claim = claim("draft6_current");
            claim.gates.insert(gate.to_string(), false);
            let verdict = ReleaseCheckerV1::check(&claim, &table).unwrap();
            assert!(!verdict.approved, "gate {gate} must fail the release");
            assert!(
                verdict
                    .reasons
                    .contains(&format!("gate_not_satisfied:{gate}")),
                "{:?}",
                verdict.reasons
            );
        }
    }

    /// ZS-RELEASE-001 acceptance: the release checker flags the Draft 4
    /// rewrite formula, model-internal one-token framing, and ambiguous Q99
    /// percentage when used as current authority -- even with every gate
    /// satisfied.
    #[test]
    fn superseded_formulations_fail_even_with_all_gates() {
        let table = current_table();
        for formulation in SUPERSEDED_FORMULATIONS {
            let verdict = ReleaseCheckerV1::check(&claim(formulation), &table).unwrap();
            assert!(!verdict.approved, "{formulation} must fail the release");
            assert!(
                verdict
                    .reasons
                    .iter()
                    .any(|reason| reason.starts_with("superseded_formulation:")),
                "{:?}",
                verdict.reasons
            );
        }
        // An unknown formulation is NOT in the table: current by default.
        let verdict = ReleaseCheckerV1::check(&claim("unlisted_formulation"), &table).unwrap();
        assert!(verdict.approved);
    }

    /// Claim and table validation fail closed on structure.
    #[test]
    fn claim_and_table_validation_fail_closed() {
        // Unknown gate names rejected.
        let mut bad_gate = claim("draft6_current");
        bad_gate.gates.insert("invented_gate".into(), true);
        assert!(ReleaseCheckerV1::check(&bad_gate, &current_table()).is_err());

        // Missing gate declaration rejected.
        let mut missing = claim("draft6_current");
        missing.gates.remove(PUBLIC_CLAIM_GATES[0]);
        assert!(ReleaseCheckerV1::check(&missing, &current_table()).is_err());

        // Duplicate formulation ids rejected in the table.
        let mut table = current_table();
        table.records.push(
            SupersessionRecordV1::new("draft4_rewrite_formula", Some("x".into()), "dup").unwrap(),
        );
        assert!(table.validate().is_err());

        // Empty formulation id rejected.
        assert!(SupersessionRecordV1::new("", Some("x".into()), "reason").is_err());
        assert!(SupersessionRecordV1::new("f", Some("x".into()), "").is_err());
    }
