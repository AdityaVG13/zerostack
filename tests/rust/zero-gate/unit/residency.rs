    use super::*;

    use zero_abi::DigestV1;

    fn digest(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn objects(count: u64, weight: u64) -> Vec<DemandWeightedObjectV1> {
        (0..count)
            .map(|index| {
                DemandWeightedObjectV1::new(
                    digest((index % 250) as u8 + 1),
                    weight,
                    "w1",
                    CacheLayerTierV1::L2,
                )
                .unwrap()
            })
            .collect()
    }

    /// ZS-CACHE-010 acceptance: a Q99 report with absent or inconsistent
    /// demanded weights is rejected (declared coordinate/window/tier).
    #[test]
    fn demand_ledger_rejects_missing_weights_and_duplicates() {
        assert!(DemandWeightedObjectV1::new(digest(1), 0, "w1", CacheLayerTierV1::L2).is_err());
        assert!(DemandWeightedObjectV1::new(digest(1), 1, "", CacheLayerTierV1::L2).is_err());

        let mut ledger = DemandWeightLedgerV1::new(objects(3, 10)).unwrap();
        assert_eq!(ledger.window_mass("w1"), 30);
        assert_eq!(ledger.tier_mass("w1", CacheLayerTierV1::L2), 30);
        // Duplicate declaration for the same object+window fails closed.
        ledger
            .objects
            .push(DemandWeightedObjectV1::new(digest(1), 10, "w1", CacheLayerTierV1::L2).unwrap());
        assert!(ledger.validate().is_err());
    }

    /// ZS-CACHE-004 acceptance: central change >1% of demanded mass reports
    /// Q99 unavailable; impossibility is reported, never averaged away.
    #[test]
    fn central_change_over_one_percent_reports_unavailable_not_average() {
        let mut window = Q99WindowV1::new("w1", 1000);
        // 100 demanded mass: 99 hit, 1 recomputed = 1% exactly (available).
        window.observe(DemandObservationV1::new(99, true).unwrap());
        window.observe(DemandObservationV1::new(1, false).unwrap());
        let report = window.report(1000);
        assert!(!report.unavailable, "{report:?}");
        assert_eq!(report.recompute_ppm, 10_000);

        // 2 recomputed of 100 = 2% > 1%: unavailable, reason recorded.
        let mut window = Q99WindowV1::new("w1", 1000);
        window.observe(DemandObservationV1::new(98, true).unwrap());
        window.observe(DemandObservationV1::new(2, false).unwrap());
        let report = window.report(1000);
        assert!(report.unavailable, "{report:?}");
        assert!(report.reasons.iter().any(|reason| reason.contains("central_change")));

        // Empty window: unavailable (no evidence), never vacuously passing.
        let report = Q99WindowV1::new("w1", 1000).report(1000);
        assert!(report.unavailable);
        assert!(report.reasons.iter().any(|reason| reason == "no_demand_observations"));
    }

    /// ZS-CACHE-005 acceptance: the restoration threshold is
    /// `max(0, I0 - 0.01W)`; valid mass below it forces Q99 unavailable.
    #[test]
    fn restoration_threshold_is_initial_minus_one_percent_of_demanded() {
        let mut window = Q99WindowV1::new("w1", 1000);
        window.observe(DemandObservationV1::new(100, true).unwrap());
        // W = 100, I0 = 1000: threshold = 1000 - 1 = 999.
        assert_eq!(window.restoration_threshold(), 999);
        assert!(window.q99_unavailable(999).is_none());
        assert!(window.q99_unavailable(998).is_some());

        // Erosion floors at zero: huge W cannot push the threshold negative.
        let mut window = Q99WindowV1::new("w1", 10);
        window.observe(DemandObservationV1::new(2000, true).unwrap());
        assert_eq!(window.restoration_threshold(), 0);
        assert!(window.q99_unavailable(0).is_none());
    }

    /// ZS-CACHE-011 acceptance: optimizer proposes, independent checker
    /// authorizes; a plan just below the threshold fails, just above passes.
    #[test]
    fn threshold_checker_rejects_just_below_and_accepts_just_above() {
        fn plan(weight: u64) -> ResidencyPlanV1 {
            ResidencyPlanV1 {
                tier: "l2".into(),
                capacity_bytes: 1_000_000,
                threshold: 0.99,
                demand_window_root: "fz://root/demand-w1".into(),
                objects: vec![
                    ResidencyPlanObjectV1::new("fz://blob/a", 10, weight, true, true).unwrap(),
                    ResidencyPlanObjectV1::new("fz://blob/b", 10, 1000 - weight, true, false)
                        .unwrap(),
                ],
                optimizer: Some("optimizer:seed-1".into()),
                proposal_root: None,
            }
        }

        // 98.9% coverage: just below 99% -> rejected.
        let below = plan(989);
        assert!(matches!(
            ResidencyThresholdCheckerV1::authorize(&below),
            Err(ResidencyErrorV1::PlanRejected(_))
        ));
        // 99.1% coverage: just above -> accepted.
        let above = plan(991);
        ResidencyThresholdCheckerV1::authorize(&above).expect("just above threshold passes");

        // Capacity breach fails closed even at full coverage.
        let mut oversized = plan(1000);
        oversized.capacity_bytes = 5;
        assert!(ResidencyThresholdCheckerV1::authorize(&oversized).is_err());

        // No demanded weight fails closed.
        let empty = ResidencyPlanV1 {
            tier: "l2".into(),
            capacity_bytes: 1_000_000,
            threshold: 0.99,
            demand_window_root: "fz://root/demand-w1".into(),
            objects: vec![],
            optimizer: None,
            proposal_root: None,
        };
        assert!(ResidencyThresholdCheckerV1::authorize(&empty).is_err());

        // A claimed proposal_root that does not match content fails closed.
        let mut forged = plan(991);
        forged.proposal_root = Some("deadbeef".into());
        assert!(ResidencyThresholdCheckerV1::authorize(&forged).is_err());
    }

    /// ZS-CACHE-012 acceptance: eviction slack sigma = W_R - 0.99W; an
    /// eviction that pushes resident mass below the 99% floor is rejected.
    #[test]
    fn eviction_slack_guard_rejects_below_the_ninety_nine_percent_floor() {
        // Demanded 1000; resident 1000 -> slack = 1000 - 990 = 10 (1%).
        let slack = EvictionSlackV1::new(1000, 1000).unwrap();
        assert_eq!(slack.slack_ppm(), 10_000);
        // Evicting 10 keeps resident at exactly 990 (99%) -> allowed.
        slack.guard_eviction(10).expect("at the floor is allowed");
        // Evicting 11 drops below the floor -> rejected.
        assert!(matches!(
            slack.guard_eviction(11),
            Err(ResidencyErrorV1::SlackExceeded { .. })
        ));

        // Resident at 99.5% of demanded: slack is 0.5% of demanded.
        let slack = EvictionSlackV1::new(995, 1000).unwrap();
        assert!(slack.guard_eviction(4).is_ok());
        assert!(slack.guard_eviction(6).is_err());

        // Zero demanded mass fails closed at construction.
        assert!(EvictionSlackV1::new(0, 0).is_err());
    }

    /// ZS-CACHE-013 acceptance: provider (L3) loss preserves L2 validity;
    /// recovery is fetch/rematerialize, never rediscovery; tombstones never
    /// delete the L2 validity record.
    #[test]
    fn l3_loss_never_becomes_project_amnesia() {
        let mut ledger = LayerValidityLedgerV1::new();
        let object = digest(7);

        // L3 loss on an entry with no L2 validity record fails closed
        // (undiscovered loss).
        assert!(matches!(
            ledger.mark_l3_loss(object),
            Err(ResidencyErrorV1::L3LossUndiscovered(_))
        ));

        ledger.publish_l2(object).unwrap();
        ledger.mark_l3_loss(object).unwrap();
        let entry = ledger.entry(object).unwrap();
        // L2 validity preserved; L3 invalid; refetch required; identity kept.
        assert!(entry.l2_valid);
        assert!(!entry.l3_valid);
        assert!(entry.l2_needs_refetch);
        assert_eq!(entry.object_root, object);

        // Refetch completes with the SAME object root (no rediscovery).
        ledger.complete_refetch(object).unwrap();
        let entry = ledger.entry(object).unwrap();
        assert!(!entry.l2_needs_refetch);
        assert!(entry.l3_valid);
        assert_eq!(entry.object_root, object);

        // Tombstone clears liveness but the entry (and its causal record)
        // remains: never deleted.
        ledger.tombstone(object).unwrap();
        let entry = ledger.entry(object).unwrap();
        assert!(!entry.l1_valid);
        assert!(!entry.l2_valid);
        assert!(ledger.entry(object).is_some());
    }

    /// Plan wire shape matches the canonical V6 schema field set.
    #[test]
    fn residency_plan_serializes_to_the_schema_field_set() {
        let plan = ResidencyPlanV1 {
            tier: "l2".into(),
            capacity_bytes: 4096,
            threshold: 0.99,
            demand_window_root: "fz://root/demand-w1".into(),
            objects: vec![ResidencyPlanObjectV1::new(
                "fz://blob/a",
                10,
                100,
                true,
                true,
            )
            .unwrap()],
            optimizer: Some("optimizer:seed-1".into()),
            proposal_root: None,
        };
        let value: Value = serde_json::to_value(&plan).unwrap();
        let object = value.as_object().unwrap();
        for key in ["tier", "capacity_bytes", "threshold", "demand_window_root", "objects"] {
            assert!(object.contains_key(key), "missing {key}");
        }
        assert!(!object.contains_key("extra"));
        // Threshold out of range fails closed.
        let mut bad = plan;
        bad.threshold = 1.5;
        assert!(bad.validate().is_err());
    }
