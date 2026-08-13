    use super::*;

    fn d(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn identity() -> ParentCounterIdentityV1 {
        ParentCounterIdentityV1 {
            counter_id: "parent.cpu_ns".into(),
            unit: CausalCounterUnitV1::CpuNanoseconds,
            boundary_digest: d(1),
            adapter_digest: d(2),
            platform_profile_digest: d(3),
        }
    }

    fn measured(total: u64) -> ParentCounterObservationV1 {
        ParentCounterObservationV1::Measured {
            window: ParentCounterWindowV1 {
                identity: identity(),
                start: 100,
                end: 100 + total,
            },
        }
    }

    fn charge(byte: u8, class: CausalWorkClassV1, amount: u64) -> CausalWorkChargeV1 {
        CausalWorkChargeV1 {
            work_unit_id: d(byte),
            class,
            amount,
        }
    }

    #[test]
    fn causal_classes_are_exactly_eight_and_contract_is_stable() {
        assert_eq!(CausalWorkClassV1::ALL.len(), 8);
        assert_eq!(
            causal_work_contract_digest_v1().to_hex(),
            "094be0570d982ab1817b8296e403db516fd43cfa5162014a9532e645b4a2eb82"
        );
    }

    #[test]
    fn causal_classes_conserve_and_preregistered_residue_closes() {
        let outcome = CausalWorkReceiptV1::build(
            d(9),
            measured(10),
            vec![
                charge(1, CausalWorkClassV1::Candidate, 3),
                charge(2, CausalWorkClassV1::Verification, 2),
            ],
            ResiduePolicyV1::AssignToResidue {
                policy_id: "unattributed-parent-delta.v1".into(),
                policy_digest: d(4),
                residue_work_unit_id: d(8),
            },
        )
        .unwrap();
        let CausalWorkOutcomeV1::Measured { receipt } = outcome else {
            panic!("measurement must produce receipt")
        };
        assert_eq!(receipt.class_totals.residue, 5);
        assert_eq!(receipt.classified_total, 10);
        receipt.validate().unwrap();
        assert_eq!(
            receipt.receipt_digest.to_hex(),
            "33763c6ab2d3c9374d3238ed54d2930e014cd49b61befb42eca8ab623fef72bf"
        );

        let valid_wire = serde_json::to_value(&receipt).unwrap();
        let mut bad_version = valid_wire.clone();
        bad_version["schema_version"] = json!(2);
        assert!(serde_json::from_value::<CausalWorkReceiptV1>(bad_version).is_err());
        let mut bad_total = valid_wire.clone();
        bad_total["observed_total"] = json!(9);
        assert!(serde_json::from_value::<CausalWorkReceiptV1>(bad_total).is_err());
        let mut bad_order = valid_wire.clone();
        bad_order["charges"].as_array_mut().unwrap().reverse();
        assert!(serde_json::from_value::<CausalWorkReceiptV1>(bad_order).is_err());
        let mut bad_digest = valid_wire;
        bad_digest["receipt_digest"] = json!(d(0));
        assert!(serde_json::from_value::<CausalWorkReceiptV1>(bad_digest).is_err());
    }

    #[test]
    fn causal_classes_reject_dual_missing_overflow_and_estimate_alias() {
        assert_eq!(
            CausalWorkReceiptV1::build(
                d(9),
                measured(2),
                vec![
                    charge(1, CausalWorkClassV1::Candidate, 1),
                    charge(1, CausalWorkClassV1::Fallback, 1),
                ],
                ResiduePolicyV1::RejectUnclassified,
            )
            .unwrap_err()
            .code(),
            CausalWorkFailureCodeV1::DoubleClassifiedWorkUnit
        );
        assert_eq!(
            CausalWorkReceiptV1::build(
                d(9),
                measured(2),
                vec![charge(1, CausalWorkClassV1::Candidate, 1)],
                ResiduePolicyV1::RejectUnclassified,
            )
            .unwrap_err()
            .code(),
            CausalWorkFailureCodeV1::UnclassifiedWork
        );
        assert_eq!(
            CausalClassTotalsV1 {
                candidate: u64::MAX,
                verification: 1,
                ..Default::default()
            }
            .checked_total()
            .unwrap_err()
            .code(),
            CausalWorkFailureCodeV1::CounterOverflow
        );
        let estimate = json!({
            "estimator_id": "declared",
            "identity": identity(),
            "declared_value": 1.5,
            "assumptions_digest": d(5)
        });
        assert!(serde_json::from_value::<DeclaredEstimateV1>(estimate.clone()).is_err());
        assert!(serde_json::from_value::<ParentCounterObservationV1>(estimate).is_err());
    }

    #[test]
    fn causal_classes_unavailable_is_unmeasured_not_zero() {
        let outcome = CausalWorkReceiptV1::build(
            d(9),
            ParentCounterObservationV1::Unmeasured {
                identity: identity(),
                reason: "counter unavailable".into(),
            },
            Vec::new(),
            ResiduePolicyV1::RejectUnclassified,
        )
        .unwrap();
        assert!(matches!(outcome, CausalWorkOutcomeV1::Unmeasured { .. }));
    }

    #[test]
    fn causal_classes_archived_v2_fixture_stays_readable_without_rewrite() {
        const ARCHIVE: &[u8] = include_bytes!("../fixtures/token-ledger-v2-archive.json");
        let preserved = ARCHIVE.to_vec();
        let ledger: crate::TokenLedger = serde_json::from_slice(ARCHIVE).unwrap();
        assert_eq!(ledger.billed_tokens, 6);
        assert_eq!(ledger.failed_trial_tokens, 3);
        assert_eq!(ledger.retry_tokens, 2);
        assert_eq!(ledger.recovery_tokens, 4);
        assert_eq!(ledger.reexpansion_tokens, 1);
        assert_eq!(ledger.fallback_tokens, 5);
        assert_eq!(ledger.check_accounting_complete().unwrap(), 21);
        assert_eq!(ARCHIVE, preserved.as_slice());
        assert_eq!(
            DigestV1::from_bytes(sha256(ARCHIVE)).to_hex(),
            "650b5e225689e57a142d815b4b6e709b02b58f2c5ed81b8d30405ede8cbd331d"
        );
    }

    #[test]
    fn causal_classes_legacy_mapping_never_becomes_fact() {
        for legacy in [
            LegacyChargeClassV2::Billed,
            LegacyChargeClassV2::FailedTrial,
            LegacyChargeClassV2::Retry,
            LegacyChargeClassV2::Recovery,
            LegacyChargeClassV2::Reexpansion,
            LegacyChargeClassV2::Fallback,
        ] {
            let mapping = map_legacy_class_v2(legacy);
            assert!(mapping.requires_remeasurement);
            assert!(!mapping.measured_fact);
        }
    }

    fn digest_from_u64(value: u64) -> DigestV1 {
        let mut bytes = [0_u8; 32];
        bytes[..8].copy_from_slice(&value.to_le_bytes());
        DigestV1::from_bytes(bytes)
    }

    fn eight_class_receipt() -> CausalWorkReceiptV1 {
        // One charge per causal class; the residue class is closed by the
        // preregistered AssignToResidue policy, not by a caller-supplied class.
        let outcome = CausalWorkReceiptV1::build(
            d(9),
            measured(36),
            vec![
                charge(1, CausalWorkClassV1::Candidate, 1),
                charge(2, CausalWorkClassV1::Verification, 2),
                charge(3, CausalWorkClassV1::Comparison, 3),
                charge(4, CausalWorkClassV1::Baseline, 4),
                charge(5, CausalWorkClassV1::Fallback, 5),
                charge(6, CausalWorkClassV1::Restoration, 6),
                charge(7, CausalWorkClassV1::Prewarm, 7),
            ],
            ResiduePolicyV1::AssignToResidue {
                policy_id: "unattributed-parent-delta.v1".into(),
                policy_digest: d(4),
                residue_work_unit_id: d(8),
            },
        )
        .unwrap();
        let CausalWorkOutcomeV1::Measured { receipt } = outcome else {
            panic!("measurement must produce receipt")
        };
        receipt
    }

    #[test]
    fn causal_receipt_round_trips_truthfully_through_canonical_json() {
        let receipt = eight_class_receipt();
        assert_eq!(receipt.charges.len(), 8);
        assert_eq!(receipt.class_totals.candidate, 1);
        assert_eq!(receipt.class_totals.verification, 2);
        assert_eq!(receipt.class_totals.comparison, 3);
        assert_eq!(receipt.class_totals.baseline, 4);
        assert_eq!(receipt.class_totals.fallback, 5);
        assert_eq!(receipt.class_totals.restoration, 6);
        assert_eq!(receipt.class_totals.prewarm, 7);
        assert_eq!(receipt.class_totals.residue, 8);
        assert_eq!(receipt.classified_total, 36);
        assert_eq!(receipt.observed_total, 36);

        let wire = serde_json::to_value(&receipt).unwrap();
        let decoded: CausalWorkReceiptV1 = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(decoded, receipt);
        decoded.validate().unwrap();
        assert_eq!(decoded.compute_digest().unwrap(), receipt.receipt_digest);

        // Canonical JSON is stable across the round trip and never carries a
        // float: every counter serializes as an integer. A float leak (1.5
        // masquerading as a measured fact) must be structurally impossible.
        let canonical = canonical_json(&wire);
        assert_eq!(
            canonical,
            canonical_json(&serde_json::to_value(&decoded).unwrap())
        );
        fn assert_integer_only(value: &Value) {
            match value {
                Value::Number(number) => {
                    assert!(
                        number.as_u64().is_some() || number.as_i64().is_some(),
                        "wire carried a float: {number}"
                    );
                }
                Value::Array(items) => items.iter().for_each(assert_integer_only),
                Value::Object(map) => map.values().for_each(assert_integer_only),
                _ => {}
            }
        }
        assert_integer_only(&wire);
    }

    #[test]
    fn all_eight_causal_classes_conserve_with_residue_closure() {
        let receipt = eight_class_receipt();
        assert_eq!(receipt.class_totals.checked_total().unwrap(), 36);
        // The residue charge carries exactly the preregistered work-unit id.
        let residue = receipt
            .charges
            .iter()
            .find(|c| c.class == CausalWorkClassV1::Residue)
            .expect("residue charge must exist");
        assert_eq!(residue.work_unit_id, d(8));
        assert_eq!(residue.amount, 8);
        // A wire mutant that labels a non-preregistered work unit as residue
        // (with totals patched to match) breaks the preregistered policy and
        // must fail closed before the digest check.
        let mut mutant = receipt.clone();
        mutant.charges[0].class = CausalWorkClassV1::Residue;
        mutant.class_totals.candidate = 0;
        mutant.class_totals.residue = 9;
        assert_eq!(
            mutant.validate().unwrap_err().code(),
            CausalWorkFailureCodeV1::ResidueWithoutPolicy
        );
    }

    #[test]
    fn counter_correspondence_receipt_round_trips_and_rejects_wire_mutation() {
        let identity = identity();
        let window = ParentCounterWindowV1 {
            identity: identity.clone(),
            start: 10,
            end: 25,
        };
        let receipt = CounterCorrespondenceReceiptV1::new(
            "macos-arm64".into(),
            CounterEvidenceModeV1::Synthetic,
            identity.clone(),
            window.clone(),
            15,
            identity.adapter_digest,
        )
        .unwrap();
        assert!(!receipt.is_native_evidence());

        let wire = serde_json::to_value(&receipt).unwrap();
        let decoded: CounterCorrespondenceReceiptV1 = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(decoded, receipt);
        assert!(!decoded.is_native_evidence());
        assert_eq!(
            decoded.parent_window.identity.counter_id, "parent.cpu_ns",
            "round trip must preserve the bound counter identity"
        );

        // Native evidence mode round-trips and is distinguishable on the wire.
        let native = CounterCorrespondenceReceiptV1::new(
            "windows-x64".into(),
            CounterEvidenceModeV1::Native,
            identity.clone(),
            window,
            15,
            identity.adapter_digest,
        )
        .unwrap();
        assert!(native.is_native_evidence());
        let native_wire = serde_json::to_value(&native).unwrap();
        assert_eq!(native_wire["evidence_mode"], "native");
        let native_decoded: CounterCorrespondenceReceiptV1 =
            serde_json::from_value(native_wire).unwrap();
        assert_eq!(native_decoded, native);

        // Every bound field rejects a wire mutation; nothing can drift silently.
        let mut delta = wire.clone();
        delta["adapter_observed_delta"] = json!(14);
        assert!(serde_json::from_value::<CounterCorrespondenceReceiptV1>(delta).is_err());

        let mut window_mut = wire.clone();
        window_mut["parent_window"]["end"] = json!(26);
        assert!(serde_json::from_value::<CounterCorrespondenceReceiptV1>(window_mut).is_err());

        let mut identity_mut = wire.clone();
        identity_mut["identity"]["counter_id"] = json!("other.counter");
        assert!(serde_json::from_value::<CounterCorrespondenceReceiptV1>(identity_mut).is_err());

        let mut adapter_mut = wire.clone();
        adapter_mut["adapter_binary_digest"] = json!(d(9));
        assert!(serde_json::from_value::<CounterCorrespondenceReceiptV1>(adapter_mut).is_err());

        let mut profile_mut = wire.clone();
        profile_mut["platform_profile"] = json!("");
        assert!(serde_json::from_value::<CounterCorrespondenceReceiptV1>(profile_mut).is_err());
    }

    #[test]
    fn unmeasured_counter_serializes_as_unmeasured_not_zero() {
        let outcome = CausalWorkReceiptV1::build(
            d(9),
            ParentCounterObservationV1::Unmeasured {
                identity: identity(),
                reason: "counter unavailable".into(),
            },
            Vec::new(),
            ResiduePolicyV1::RejectUnclassified,
        )
        .unwrap();
        let wire = serde_json::to_value(&outcome).unwrap();
        assert_eq!(wire["outcome"], "unmeasured");
        assert_eq!(wire["identity"]["counter_id"], "parent.cpu_ns");
        assert_eq!(wire["reason"], "counter unavailable");
        assert!(
            wire.get("receipt").is_none(),
            "unmeasured must never carry a zero receipt"
        );
        assert!(
            wire.get("window").is_none(),
            "unmeasured must not fabricate a counter window"
        );
        let decoded: CausalWorkOutcomeV1 = serde_json::from_value(wire).unwrap();
        assert_eq!(decoded, outcome);

        // A truly measured zero-delta window is a different wire shape: the
        // unmeasured case is never an alias for zero.
        let zero_measured = CausalWorkReceiptV1::build(
            d(9),
            measured(0),
            Vec::new(),
            ResiduePolicyV1::RejectUnclassified,
        )
        .unwrap();
        let zero_wire = serde_json::to_value(&zero_measured).unwrap();
        assert_eq!(zero_wire["outcome"], "measured");
        assert_eq!(zero_wire["receipt"]["observed_total"], 0);
        let zero_decoded: CausalWorkOutcomeV1 = serde_json::from_value(zero_wire).unwrap();
        assert_eq!(zero_decoded, zero_measured);
        assert_ne!(zero_decoded, outcome);
    }

    #[test]
    fn per_class_overflow_is_typed_for_every_class() {
        for class in CausalWorkClassV1::ALL {
            let mut totals = CausalClassTotalsV1::default();
            totals.add(class, u64::MAX).unwrap();
            assert_eq!(
                totals.add(class, 1).unwrap_err().code(),
                CausalWorkFailureCodeV1::CounterOverflow,
                "class {class:?} must overflow as a typed failure"
            );
        }
    }

    #[test]
    fn wire_receipt_rejects_version_totals_policy_and_regression_mutations() {
        let receipt = eight_class_receipt();
        let valid_wire = serde_json::to_value(&receipt).unwrap();

        let mut bad_taxonomy = valid_wire.clone();
        bad_taxonomy["taxonomy_version"] = json!(2);
        assert!(serde_json::from_value::<CausalWorkReceiptV1>(bad_taxonomy).is_err());

        let mut bad_totals = valid_wire.clone();
        bad_totals["class_totals"]["candidate"] = json!(999);
        assert!(serde_json::from_value::<CausalWorkReceiptV1>(bad_totals).is_err());

        let mut bad_policy = valid_wire.clone();
        bad_policy["residue_policy"] = json!({"policy": "reject_unclassified"});
        assert!(serde_json::from_value::<CausalWorkReceiptV1>(bad_policy).is_err());

        let mut bad_window = valid_wire.clone();
        bad_window["measurement"]["end"] = json!(99);
        assert!(serde_json::from_value::<CausalWorkReceiptV1>(bad_window).is_err());

        let mut bad_duplicate = valid_wire.clone();
        bad_duplicate["charges"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "work_unit_id": d(1),
                "class": "fallback",
                "amount": 1
            }));
        assert!(serde_json::from_value::<CausalWorkReceiptV1>(bad_duplicate).is_err());

        let mut bad_zero_amount = valid_wire.clone();
        bad_zero_amount["charges"][0]["amount"] = json!(0);
        assert!(serde_json::from_value::<CausalWorkReceiptV1>(bad_zero_amount).is_err());
    }

    #[test]
    fn estimate_json_cannot_decode_into_any_measurement_namespace() {
        let estimate = json!({
            "estimator_id": "declared",
            "identity": identity(),
            "declared_value": 10,
            "assumptions_digest": d(5)
        });
        assert!(serde_json::from_value::<DeclaredEstimateV1>(estimate.clone()).is_ok());
        assert!(serde_json::from_value::<ParentCounterObservationV1>(estimate.clone()).is_err());
        assert!(serde_json::from_value::<CausalWorkOutcomeV1>(estimate.clone()).is_err());
        assert!(serde_json::from_value::<CausalWorkReceiptV1>(estimate).is_err());
    }

    #[test]
    fn too_many_charges_is_a_typed_failure_at_the_boundary() {
        let over = (0..=CAUSAL_WORK_MAX_CHARGES as u64)
            .map(|index| CausalWorkChargeV1 {
                work_unit_id: digest_from_u64(index),
                class: CausalWorkClassV1::Candidate,
                amount: 1,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            CausalWorkReceiptV1::build(
                d(9),
                measured(CAUSAL_WORK_MAX_CHARGES as u64 + 1),
                over,
                ResiduePolicyV1::RejectUnclassified,
            )
            .unwrap_err()
            .code(),
            CausalWorkFailureCodeV1::TooManyCharges
        );

        // Exactly the maximum is legal and conserves.
        let at_limit = (0..CAUSAL_WORK_MAX_CHARGES as u64)
            .map(|index| CausalWorkChargeV1 {
                work_unit_id: digest_from_u64(index),
                class: CausalWorkClassV1::Candidate,
                amount: 1,
            })
            .collect::<Vec<_>>();
        let outcome = CausalWorkReceiptV1::build(
            d(9),
            measured(CAUSAL_WORK_MAX_CHARGES as u64),
            at_limit,
            ResiduePolicyV1::RejectUnclassified,
        )
        .unwrap();
        let CausalWorkOutcomeV1::Measured { receipt } = outcome else {
            panic!("measurement must produce receipt")
        };
        assert_eq!(receipt.classified_total, CAUSAL_WORK_MAX_CHARGES as u64);
    }
