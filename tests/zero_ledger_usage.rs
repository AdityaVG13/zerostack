use zero_ledger::usage::*;

#[test]
fn attribution_is_exactly_once() {
    let baseline = vec!["a".into(), "b".into()];
    let rows = vec![
        SavingsOccurrence {
            occurrence_id: "a".into(),
            baseline_event_id: "ea".into(),
            optimized_event_id: Some("za".into()),
            disposition: SavingsDisposition::Retained,
        },
        SavingsOccurrence {
            occurrence_id: "b".into(),
            baseline_event_id: "eb".into(),
            optimized_event_id: None,
            disposition: SavingsDisposition::Eliminated {
                mechanism: "decision_view".into(),
            },
        },
    ];
    assert!(validate_disjoint_attribution(&baseline, &rows).is_ok());
    assert!(validate_disjoint_attribution(&baseline, &rows[..1]).is_err());
}

#[test]
fn hidden_model_calls_are_detected() {
    let reconciliation = ModelCallReconciliation {
        declared: 1,
        observed: 2,
    };
    assert_eq!(reconciliation.hidden_calls(), 1);
    assert!(!reconciliation.reconciled());
}
