use super::*;
use crate::causal_work::{
    CausalWorkChargeV1, CausalWorkClassV1, CausalWorkOutcomeV1, CausalWorkReceiptV1,
    ParentCounterIdentityV1, ParentCounterObservationV1, ParentCounterWindowV1, ResiduePolicyV1,
};
use zero_abi::DigestV1;

fn d(byte: u8) -> DigestV1 {
    DigestV1::from_bytes([byte; 32])
}

fn identity() -> ParentCounterIdentityV1 {
    ParentCounterIdentityV1 {
        counter_id: "parent.cpu_ns".into(),
        unit: crate::CausalCounterUnitV1::CpuNanoseconds,
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

fn receipt(byte: u8, charges: Vec<(u8, CausalWorkClassV1, u64)>) -> CausalWorkReceiptV1 {
    let charges: Vec<CausalWorkChargeV1> = charges
        .into_iter()
        .map(|(id, class, amount)| CausalWorkChargeV1 {
            work_unit_id: d(id),
            class,
            amount,
        })
        .collect();
    let total: u64 = charges.iter().map(|charge| charge.amount).sum();
    let outcome = CausalWorkReceiptV1::build(
        d(byte),
        measured(total),
        charges,
        ResiduePolicyV1::RejectUnclassified,
    )
    .unwrap();
    let CausalWorkOutcomeV1::Measured { receipt } = outcome else {
        panic!("measured receipt expected");
    };
    receipt
}

fn entry(id: &str, amount: u64) -> ChargingEntry {
    ChargingEntry {
        work_unit_id: id.into(),
        amount,
        source: MeasurementSource::Exact,
    }
}

fn map(phase: ChargingPhase, entries: Vec<ChargingEntry>) -> ChargingMap {
    ChargingMap::build(phase, entries).unwrap()
}

fn empty_set() -> ChargingMapSet {
    ChargingMapSet::new(
        ChargingPhase::ALL
            .iter()
            .map(|phase| map(*phase, vec![]))
            .collect(),
    )
    .unwrap()
}

fn policy() -> PhasePolicy {
    PhasePolicy::new(&[
        (CausalWorkClassV1::Candidate, ChargingPhase::Reasoning),
        (CausalWorkClassV1::Verification, ChargingPhase::Verification),
        (CausalWorkClassV1::Comparison, ChargingPhase::Decisions),
        (CausalWorkClassV1::Baseline, ChargingPhase::RequestInfo),
        (CausalWorkClassV1::Fallback, ChargingPhase::Output),
        (CausalWorkClassV1::Restoration, ChargingPhase::Reasoning),
        (CausalWorkClassV1::Prewarm, ChargingPhase::Effects),
        (CausalWorkClassV1::Residue, ChargingPhase::Effects),
    ])
    .unwrap()
}

#[test]
fn map_total_is_the_exact_entry_sum_and_entries_are_canonical() {
    let charging_map = map(
        ChargingPhase::Verification,
        vec![entry("b", 3), entry("a", 4), entry("c", 2)],
    );
    assert_eq!(charging_map.total(), 9);
    assert_eq!(charging_map.source(), MeasurementSource::Exact);
    let ids: Vec<&str> = charging_map.entries().iter().map(|e| e.work_unit_id.as_str()).collect();
    assert_eq!(ids, vec!["a", "b", "c"]);
}

#[test]
fn map_refusals_empty_id_zero_amount_duplicate_overflow() {
    assert_eq!(
        ChargingMap::build(ChargingPhase::Output, vec![entry("", 1)]).unwrap_err(),
        ChargingMapError::EmptyWorkUnitId
    );
    assert_eq!(
        ChargingMap::build(ChargingPhase::Output, vec![entry("a", 0)]).unwrap_err(),
        ChargingMapError::ZeroAmount {
            work_unit_id: "a".into()
        }
    );
    assert_eq!(
        ChargingMap::build(ChargingPhase::Output, vec![entry("a", 1), entry("a", 2)]).unwrap_err(),
        ChargingMapError::DuplicateWorkUnit {
            work_unit_id: "a".into()
        }
    );
    let long_id = "x".repeat(CAUSAL_WORK_MAX_ID_BYTES + 1);
    assert!(matches!(
        ChargingMap::build(ChargingPhase::Output, vec![entry(&long_id, 1)]).unwrap_err(),
        ChargingMapError::WorkUnitIdTooLong { .. }
    ));
    let big = ChargingMap::build(
        ChargingPhase::Output,
        vec![
            ChargingEntry {
                work_unit_id: "a".into(),
                amount: u64::MAX,
                source: MeasurementSource::Exact,
            },
            ChargingEntry {
                work_unit_id: "b".into(),
                amount: 1,
                source: MeasurementSource::Exact,
            },
        ],
    )
    .unwrap_err();
    assert_eq!(big, ChargingMapError::CounterOverflow);
}

#[test]
fn map_source_label_honors_exactness_law() {
    let charging_map = ChargingMap::build(
        ChargingPhase::Verification,
        vec![
            ChargingEntry {
                work_unit_id: "a".into(),
                amount: 1,
                source: MeasurementSource::Exact,
            },
            ChargingEntry {
                work_unit_id: "b".into(),
                amount: 2,
                source: MeasurementSource::Bounded,
            },
        ],
    )
    .unwrap();
    // Derived total must not be labeled Exact unless every input was Exact.
    assert_eq!(charging_map.source(), MeasurementSource::Bounded);
}

#[test]
fn closure_is_full_when_attributed_equals_measured() {
    let set = ChargingMapSet::new(vec![
        map(ChargingPhase::RequestInfo, vec![entry("r", 10)]),
        map(ChargingPhase::Verification, vec![entry("v", 30)]),
        map(ChargingPhase::Output, vec![entry("o", 60)]),
        map(ChargingPhase::Decisions, vec![]),
        map(ChargingPhase::Reasoning, vec![]),
        map(ChargingPhase::Effects, vec![]),
    ])
    .unwrap();
    let report = set.check_closure(100).unwrap();
    assert_eq!(report.attributed, 100);
    assert_eq!(report.unclaimed, 0);
    assert_eq!(report.gamma, (1, 1));
    assert!(report.full);
}

#[test]
fn closure_partial_reports_unclaimed_and_never_guesses() {
    let set = ChargingMapSet::new(vec![
        map(ChargingPhase::RequestInfo, vec![entry("r", 10)]),
        map(ChargingPhase::Verification, vec![entry("v", 30)]),
        map(ChargingPhase::Output, vec![entry("o", 50)]),
        map(ChargingPhase::Decisions, vec![]),
        map(ChargingPhase::Reasoning, vec![]),
        map(ChargingPhase::Effects, vec![]),
    ])
    .unwrap();
    let report = set.check_closure(100).unwrap();
    assert_eq!(report.attributed, 90);
    assert_eq!(report.unclaimed, 10);
    assert_eq!(report.gamma, (9, 10));
    assert!(!report.full);
    // The unclaimed 10 must not appear in any map: no guessed split.
    assert_eq!(set.total_attributed().amount(), 90);
}

#[test]
fn conservation_off_by_one_is_refused() {
    let set = ChargingMapSet::new(vec![
        map(ChargingPhase::RequestInfo, vec![entry("r", 10)]),
        map(ChargingPhase::Verification, vec![entry("v", 30)]),
        map(ChargingPhase::Output, vec![entry("o", 61)]),
        map(ChargingPhase::Decisions, vec![]),
        map(ChargingPhase::Reasoning, vec![]),
        map(ChargingPhase::Effects, vec![]),
    ])
    .unwrap();
    assert_eq!(
        set.check_closure(100).unwrap_err(),
        ChargingMapError::NonConservation {
            attributed: 101,
            measured: 100,
        }
    );
    // Zero measured total has no closure denominator.
    assert_eq!(
        empty_set().check_closure(0).unwrap_err(),
        ChargingMapError::ZeroMeasuredTotal
    );
}

#[test]
fn overlap_checker_rejects_double_counting() {
    let set = ChargingMapSet::new(vec![
        map(ChargingPhase::RequestInfo, vec![entry("shared", 10)]),
        map(ChargingPhase::Verification, vec![entry("shared", 30)]),
        map(ChargingPhase::Output, vec![entry("o", 60)]),
        map(ChargingPhase::Decisions, vec![]),
        map(ChargingPhase::Reasoning, vec![]),
        map(ChargingPhase::Effects, vec![]),
    ])
    .unwrap();
    assert_eq!(
        set.check_overlap().unwrap_err(),
        ChargingMapError::OverlappingCharge {
            work_unit_id: "shared".into(),
            first: ChargingPhase::RequestInfo,
            second: ChargingPhase::Verification,
        }
    );
}

#[test]
fn set_requires_exactly_one_map_per_phase() {
    let err = ChargingMapSet::new(vec![
        map(ChargingPhase::Output, vec![entry("a", 1)]),
        map(ChargingPhase::Output, vec![entry("b", 1)]),
    ])
    .unwrap_err();
    assert_eq!(err, ChargingMapError::DuplicatePhase(ChargingPhase::Output));
    let err = ChargingMapSet::new(vec![map(ChargingPhase::Output, vec![entry("a", 1)])]).unwrap_err();
    assert!(matches!(err, ChargingMapError::MissingPhases(missing) if missing.len() == 5));
}

#[test]
fn policy_must_be_total_and_conflict_free() {
    let err = PhasePolicy::new(&[
        (CausalWorkClassV1::Candidate, ChargingPhase::Reasoning),
        (CausalWorkClassV1::Candidate, ChargingPhase::Output),
    ])
    .unwrap_err();
    assert_eq!(
        err,
        ChargingMapError::PolicyConflict {
            class: CausalWorkClassV1::Candidate
        }
    );
    let err = PhasePolicy::new(&[(CausalWorkClassV1::Candidate, ChargingPhase::Reasoning)])
        .unwrap_err();
    assert_eq!(
        err,
        ChargingMapError::IncompletePolicy(CausalWorkClassV1::Verification)
    );
}

#[test]
fn solve_is_deterministic_and_groups_by_policy() {
    let receipts = vec![
        receipt(
            9,
            vec![
                (1, CausalWorkClassV1::Candidate, 3),
                (2, CausalWorkClassV1::Verification, 2),
                (3, CausalWorkClassV1::Baseline, 4),
            ],
        ),
        receipt(
            10,
            vec![
                (4, CausalWorkClassV1::Candidate, 5),
                (5, CausalWorkClassV1::Fallback, 1),
            ],
        ),
    ];
    let first = ChargingMapSet::solve(&policy(), &receipts).unwrap();
    let second = ChargingMapSet::solve(&policy(), &receipts).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap()
    );
    // Candidate -> Reasoning: 3 + 5 = 8; Verification: 2; RequestInfo: 4;
    // Output (fallback): 1.
    assert_eq!(first.map(ChargingPhase::Reasoning).total(), 8);
    assert_eq!(first.map(ChargingPhase::Verification).total(), 2);
    assert_eq!(first.map(ChargingPhase::RequestInfo).total(), 4);
    assert_eq!(first.map(ChargingPhase::Output).total(), 1);
    assert_eq!(first.total_attributed().amount(), 15);
    // Sources are Exact: every charge came from a measured receipt.
    assert_eq!(first.total_attributed().source(), MeasurementSource::Exact);
    // The solved set is disjoint by construction (receipts are exactly-one).
    first.check_overlap().unwrap();
}

#[test]
fn solve_refuses_empty_and_tampered_input() {
    assert_eq!(
        ChargingMapSet::solve(&policy(), &[]).unwrap_err(),
        ChargingMapError::EmptyReceiptSet
    );
    // A receipt whose charges do not conserve the measured window fails
    // validation and is refused: the solver never guesses a split.
    let outcome = CausalWorkReceiptV1::build(
        d(9),
        measured(3),
        vec![CausalWorkChargeV1 {
            work_unit_id: d(1),
            class: CausalWorkClassV1::Candidate,
            amount: 3,
        }],
        ResiduePolicyV1::RejectUnclassified,
    )
    .unwrap();
    let CausalWorkOutcomeV1::Measured { mut receipt } = outcome else {
        panic!("measured receipt expected");
    };
    receipt.observed_total = 11; // corrupt the conservation invariant
    let err = ChargingMapSet::solve(&policy(), &[receipt]).unwrap_err();
    assert!(matches!(err, ChargingMapError::InvalidReceipt(_)));
}

#[test]
fn duplicate_work_unit_across_receipts_is_refused() {
    // The same work unit id charged in two receipts lands in the same phase
    // map twice: double classification, refused rather than merged.
    let receipts = vec![
        receipt(9, vec![(1, CausalWorkClassV1::Candidate, 3)]),
        receipt(10, vec![(1, CausalWorkClassV1::Candidate, 4)]),
    ];
    let err = ChargingMapSet::solve(&policy(), &receipts).unwrap_err();
    assert_eq!(
        err,
        ChargingMapError::DuplicateWorkUnit {
            work_unit_id: d(1).to_hex()
        }
    );
}

#[test]
fn phase_roundtrip_and_wire_tamper_refusal() {
    for phase in ChargingPhase::ALL {
        let json = serde_json::to_string(&phase).unwrap();
        assert_eq!(serde_json::from_str::<ChargingPhase>(&json).unwrap(), phase);
        assert_eq!(json, format!("\"{}\"", phase.as_str()));
    }
    // A wire map whose totals disagree with its entries is refused.
    let charging_map = map(ChargingPhase::Output, vec![entry("a", 5)]);
    let mut value = serde_json::to_value(&charging_map).unwrap();
    value["total"] = serde_json::json!(99);
    assert!(serde_json::from_value::<ChargingMap>(value).is_err());
    // A wire set missing a phase is refused.
    let set = empty_set();
    let mut value = serde_json::to_value(&set).unwrap();
    value["maps"].as_object_mut().unwrap().remove("output");
    assert!(serde_json::from_value::<ChargingMapSet>(value).is_err());
}

#[test]
fn gamma_never_exceeds_one_across_many_attributions() {
    for measured in [1u64, 7, 100, 1000] {
        for attributed in 1..=measured {
            let set = ChargingMapSet::new(vec![
                map(ChargingPhase::RequestInfo, vec![entry("r", attributed)]),
                map(ChargingPhase::Verification, vec![]),
                map(ChargingPhase::Output, vec![]),
                map(ChargingPhase::Decisions, vec![]),
                map(ChargingPhase::Reasoning, vec![]),
                map(ChargingPhase::Effects, vec![]),
            ])
            .unwrap();
            let report = set.check_closure(measured).unwrap();
            assert!(report.gamma.0 <= report.gamma.1);
            assert_eq!(report.attributed, u128::from(attributed));
        }
    }
}
