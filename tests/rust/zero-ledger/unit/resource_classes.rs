use super::*;

fn row(class: ResourceClass, amount: u64, source: MeasurementSource) -> ResourceRow {
    ResourceRow::new(class, amount, source)
}

fn make_ledger(rows: Vec<ResourceRow>) -> ResourceLedger {
    let mut ledger = ResourceLedger::new();
    for row in rows {
        ledger.record(row);
    }
    ledger
}

#[test]
fn class_roundtrip_and_unknown_class_refusal() {
    for class in ResourceClass::ALL {
        let text = class.as_str();
        assert_eq!(text.parse::<ResourceClass>().unwrap(), class);
        assert_eq!(format!("{class}"), text);
        let json = serde_json::to_string(&class).unwrap();
        assert_eq!(serde_json::from_str::<ResourceClass>(&json).unwrap(), class);
        assert_eq!(json, format!("\"{text}\""));
    }
    // Unknown class: loud refusal on parse and on the wire.
    assert_eq!(
        "cpu_nanoseconds_bogus".parse::<ResourceClass>().unwrap_err(),
        ResourceClassParseError {
            unknown_class: "cpu_nanoseconds_bogus".into()
        }
    );
    assert!(serde_json::from_str::<ResourceClass>("\"bogus\"").is_err());
    // Round trip of the full row shape.
    let ledger = make_ledger(vec![row(ResourceClass::ToolArgsBytes, 10, MeasurementSource::Exact)]);
    let json = serde_json::to_string(&ledger).unwrap();
    assert_eq!(serde_json::from_str::<ResourceLedger>(&json).unwrap(), ledger);
}

#[test]
fn derived_totals_respect_the_exactness_labeling_law() {
    // All exact inputs -> Exact derived total.
    let ledger = make_ledger(vec![
        row(ResourceClass::CpuNanoseconds, 10, MeasurementSource::Exact),
        row(ResourceClass::CpuNanoseconds, 5, MeasurementSource::Exact),
    ]);
    let total = ledger.total_for(ResourceClass::CpuNanoseconds).unwrap();
    assert_eq!(total.amount(), 15);
    assert_eq!(total.source(), MeasurementSource::Exact);

    // Any bounded input demotes the derived total to Bounded, never Exact.
    let ledger = make_ledger(vec![
        row(ResourceClass::CpuNanoseconds, 10, MeasurementSource::Exact),
        row(ResourceClass::CpuNanoseconds, 5, MeasurementSource::Bounded),
    ]);
    assert_eq!(
        ledger.total_for(ResourceClass::CpuNanoseconds).unwrap().source(),
        MeasurementSource::Bounded
    );

    // Any estimate input demotes to Estimate.
    let ledger = make_ledger(vec![
        row(ResourceClass::CpuNanoseconds, 10, MeasurementSource::Exact),
        row(ResourceClass::CpuNanoseconds, 5, MeasurementSource::Estimate),
    ]);
    assert_eq!(
        ledger.total_for(ResourceClass::CpuNanoseconds).unwrap().source(),
        MeasurementSource::Estimate
    );

    // A class with no rows is absent (None), not silently zero.
    assert_eq!(ledger.total_for(ResourceClass::GpuNanoseconds), None);
    // Grand total covers every class with checked u128 widening.
    let ledger = make_ledger(vec![
        row(ResourceClass::CpuNanoseconds, u64::MAX, MeasurementSource::Exact),
        row(ResourceClass::GpuNanoseconds, u64::MAX, MeasurementSource::Exact),
    ]);
    assert_eq!(
        ledger.grand_total().amount(),
        u128::from(u64::MAX) + u128::from(u64::MAX)
    );
    assert_eq!(ledger.grand_total().source(), MeasurementSource::Exact);
}

#[test]
fn bill_line_validation_refuses_empty_provider_and_bad_tolerance() {
    assert_eq!(
        ProviderBillLine::new("", ResourceClass::CpuNanoseconds, 10, 0).unwrap_err(),
        LedgerError::EmptyBillProvider
    );
    assert_eq!(
        ProviderBillLine::new("acme", ResourceClass::CpuNanoseconds, 10, PPM_ONE + 1).unwrap_err(),
        LedgerError::PpmOutOfRange {
            ppm: PPM_ONE + 1
        }
    );
    // Wire decode goes through the validated constructor.
    let wire = r#"{"provider":"","class":"cpu_nanoseconds","billed_amount":10,"tolerance_ppm":0}"#;
    assert!(serde_json::from_str::<ProviderBillLine>(wire).is_err());
    let wire = r#"{"provider":"acme","class":"bogus","billed_amount":10,"tolerance_ppm":0}"#;
    assert!(serde_json::from_str::<ProviderBillLine>(wire).is_err());
}

#[test]
fn reconciliation_exact_within_tolerance_and_out_of_tolerance() {
    let ledger = make_ledger(vec![
        row(ResourceClass::CpuNanoseconds, 100, MeasurementSource::Exact),
        row(ResourceClass::CpuNanoseconds, 50, MeasurementSource::Exact),
    ]);
    let bills = vec![
        ProviderBillLine::new("acme", ResourceClass::CpuNanoseconds, 150, 0).unwrap(),
        ProviderBillLine::new("acme", ResourceClass::WireBytes, 0, 0).unwrap(),
    ];
    let report = ledger.reconcile(&bills).unwrap();
    assert_eq!(report.overall, ReconciliationState::Exact);
    assert_eq!(report.lines[0].status, BillLineStatus::ReconcilesExactly);
    assert_eq!(report.lines[0].ledger_total, 150);
    assert_eq!(report.lines[0].deviation, 0);

    // Within tolerance (exactly at the boundary: deviation * 1e6 == billed * ppm).
    let bills = vec![
        ProviderBillLine::new("acme", ResourceClass::CpuNanoseconds, 200, 250_000).unwrap(),
    ];
    let report = ledger.reconcile(&bills).unwrap();
    assert_eq!(report.overall, ReconciliationState::WithinTolerance);
    assert_eq!(
        report.lines[0].status,
        BillLineStatus::ReconcilesWithinTolerance
    );
    assert_eq!(report.lines[0].deviation, 50); // 50 <= 200 * 250000 / 1e6 = 50

    // One ppm past the boundary is refused.
    let bills = vec![
        ProviderBillLine::new("acme", ResourceClass::CpuNanoseconds, 200, 249_999).unwrap(),
    ];
    assert_eq!(
        ledger.reconcile(&bills).unwrap_err(),
        LedgerError::OutOfTolerance {
            provider: "acme".into(),
            class: "cpu_nanoseconds",
            billed: 200,
            ledger: 150,
            tolerance_ppm: 249_999,
        }
    );
}

#[test]
fn exact_state_requires_exact_rows_and_hidden_work_is_refused() {
    // Amounts match exactly but a contributing row was Bounded: the overall
    // state must not be labeled Exact.
    let ledger = make_ledger(vec![row(
        ResourceClass::CpuNanoseconds,
        150,
        MeasurementSource::Bounded,
    )]);
    let bills = vec![ProviderBillLine::new("acme", ResourceClass::CpuNanoseconds, 150, 0).unwrap()];
    let report = ledger.reconcile(&bills).unwrap();
    assert_eq!(report.overall, ReconciliationState::WithinTolerance);
    assert_eq!(report.lines[0].status, BillLineStatus::ReconcilesExactly);

    // A billed coordinate with no ledger rows is hidden uncharged work.
    let ledger = make_ledger(vec![row(
        ResourceClass::CpuNanoseconds,
        150,
        MeasurementSource::Exact,
    )]);
    let bills = vec![ProviderBillLine::new("acme", ResourceClass::GpuNanoseconds, 42, 0).unwrap()];
    assert_eq!(
        ledger.reconcile(&bills).unwrap_err(),
        LedgerError::HiddenUnchargedWork {
            provider: "acme".into(),
            class: "gpu_nanoseconds",
            billed: 42,
        }
    );
}

#[test]
fn uncached_input_and_storage_classes_are_first_class() {
    let ledger = make_ledger(vec![
        row(ResourceClass::UncachedInputTokens, 77, MeasurementSource::Exact),
        row(ResourceClass::StorageBytes, 1000, MeasurementSource::Exact),
        row(ResourceClass::Maintenance, 3, MeasurementSource::Bounded),
    ]);
    assert_eq!(
        ledger.total_for(ResourceClass::UncachedInputTokens).unwrap().amount(),
        77
    );
    assert_eq!(
        ledger.total_for(ResourceClass::StorageBytes).unwrap().amount(),
        1000
    );
    assert_eq!(
        ledger.total_for(ResourceClass::Maintenance).unwrap().source(),
        MeasurementSource::Bounded
    );
}
