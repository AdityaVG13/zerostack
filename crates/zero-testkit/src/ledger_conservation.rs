//! Exhaustive conformance checks for the V3 causal-work ledger.

#[cfg(test)]
mod tests {
    use serde_json::json;
    use zero_abi::DigestV1;
    use zero_ledger::{
        CausalCounterUnitV1, CausalWorkChargeV1, CausalWorkClassV1, CausalWorkFailureCodeV1,
        CausalWorkOutcomeV1, CausalWorkReceiptV1, CounterCorrespondenceReceiptV1,
        CounterEvidenceModeV1, DeclaredEstimateV1, ParentCounterIdentityV1,
        ParentCounterObservationV1, ParentCounterWindowV1, ResiduePolicyV1,
    };

    fn digest(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn identity(profile: u8) -> ParentCounterIdentityV1 {
        ParentCounterIdentityV1 {
            counter_id: "parent.synthetic.integer".into(),
            unit: CausalCounterUnitV1::Calls,
            boundary_digest: digest(1),
            adapter_digest: digest(2),
            platform_profile_digest: digest(profile),
        }
    }

    fn measured(total: u64) -> ParentCounterObservationV1 {
        ParentCounterObservationV1::Measured {
            window: ParentCounterWindowV1 {
                identity: identity(3),
                start: 17,
                end: 17 + total,
            },
        }
    }

    fn enumerate_compositions(
        remaining: u64,
        index: usize,
        values: &mut [u64; 8],
        visited: &mut u64,
    ) {
        if index == values.len() - 1 {
            values[index] = remaining;
            assert_distribution(values);
            *visited += 1;
            return;
        }
        for value in 0..=remaining {
            values[index] = value;
            enumerate_compositions(remaining - value, index + 1, values, visited);
        }
    }

    fn assert_distribution(values: &[u64; 8]) {
        let charges = CausalWorkClassV1::ALL
            .into_iter()
            .zip(values.iter().copied())
            .enumerate()
            .filter_map(|(index, (class, amount))| {
                (amount != 0).then_some(CausalWorkChargeV1 {
                    work_unit_id: digest(index as u8 + 10),
                    class,
                    amount,
                })
            })
            .collect::<Vec<_>>();
        let total = values.iter().sum();
        let residue_policy = if values[7] == 0 {
            ResiduePolicyV1::RejectUnclassified
        } else {
            ResiduePolicyV1::AssignToResidue {
                policy_id: "exhaustive-residue.v1".into(),
                policy_digest: digest(18),
                residue_work_unit_id: digest(17),
            }
        };
        let outcome =
            CausalWorkReceiptV1::build(digest(99), measured(total), charges, residue_policy)
                .unwrap();
        let CausalWorkOutcomeV1::Measured { receipt } = outcome else {
            panic!("measured parent counter must produce receipt")
        };
        assert_eq!(receipt.observed_total, total);
        assert_eq!(receipt.classified_total, total);
        for (class, expected) in CausalWorkClassV1::ALL
            .into_iter()
            .zip(values.iter().copied())
        {
            assert_eq!(receipt.class_totals.class_total(class), expected);
        }
        receipt.validate().unwrap();
    }

    #[test]
    fn causal_conservation_exhausts_small_integer_distributions() {
        let mut visited = 0;
        let mut values = [0; 8];
        for total in 0..=8 {
            enumerate_compositions(total, 0, &mut values, &mut visited);
        }
        assert_eq!(visited, 12_870);
    }

    #[test]
    fn causal_mutants_fail_closed() {
        let dual = CausalWorkReceiptV1::build(
            digest(99),
            measured(2),
            vec![
                CausalWorkChargeV1 {
                    work_unit_id: digest(10),
                    class: CausalWorkClassV1::Candidate,
                    amount: 1,
                },
                CausalWorkChargeV1 {
                    work_unit_id: digest(10),
                    class: CausalWorkClassV1::Comparison,
                    amount: 1,
                },
            ],
            ResiduePolicyV1::RejectUnclassified,
        )
        .unwrap_err();
        assert_eq!(
            dual.code(),
            CausalWorkFailureCodeV1::DoubleClassifiedWorkUnit
        );

        let missing = CausalWorkReceiptV1::build(
            digest(99),
            measured(2),
            vec![CausalWorkChargeV1 {
                work_unit_id: digest(10),
                class: CausalWorkClassV1::Candidate,
                amount: 1,
            }],
            ResiduePolicyV1::RejectUnclassified,
        )
        .unwrap_err();
        assert_eq!(missing.code(), CausalWorkFailureCodeV1::UnclassifiedWork);

        let unconserved = CausalWorkReceiptV1::build(
            digest(99),
            measured(1),
            vec![CausalWorkChargeV1 {
                work_unit_id: digest(10),
                class: CausalWorkClassV1::Candidate,
                amount: 2,
            }],
            ResiduePolicyV1::RejectUnclassified,
        )
        .unwrap_err();
        assert_eq!(unconserved.code(), CausalWorkFailureCodeV1::NonConservation);

        let float_leak = json!({
            "estimator_id": "float-mutant",
            "identity": identity(3),
            "declared_value": 0.5,
            "assumptions_digest": digest(4)
        });
        assert!(serde_json::from_value::<DeclaredEstimateV1>(float_leak.clone()).is_err());
        assert!(serde_json::from_value::<ParentCounterObservationV1>(float_leak).is_err());

        let estimate_as_fact = json!({
            "availability": "measured",
            "window": {
                "identity": identity(3),
                "start": 0,
                "end": {"declared_estimate": 1}
            }
        });
        assert!(serde_json::from_value::<ParentCounterObservationV1>(estimate_as_fact).is_err());

        let overflowed_end = ParentCounterWindowV1 {
            identity: identity(3),
            start: u64::MAX,
            end: 0,
        };
        assert_eq!(
            overflowed_end.delta().unwrap_err().code(),
            CausalWorkFailureCodeV1::CounterRegressed
        );
    }

    #[test]
    fn causal_synthetic_counter_adapters_match_all_preregistered_profiles() {
        for (profile, profile_byte) in [("macos", 21), ("linux", 22), ("windows", 23)] {
            let identity = identity(profile_byte);
            let receipt = CounterCorrespondenceReceiptV1::new(
                profile.into(),
                CounterEvidenceModeV1::Synthetic,
                identity.clone(),
                ParentCounterWindowV1 {
                    identity,
                    start: 1_000,
                    end: 1_041,
                },
                41,
                digest(2),
            )
            .unwrap();
            assert!(!receipt.is_native_evidence());
            let mut forged_wire = serde_json::to_value(&receipt).unwrap();
            forged_wire["adapter_observed_delta"] = json!(40);
            assert!(serde_json::from_value::<CounterCorrespondenceReceiptV1>(forged_wire).is_err());
        }
    }
}
