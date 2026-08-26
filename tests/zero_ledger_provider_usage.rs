use zero_abi::zero_kernel::{PROVIDER_USAGE_SCHEMA, ProviderUsageObservation, UsageAmount};
use zero_ledger::usage::{
    ObservationKind, UsageArm, UsageCoordinate, coordinate_totals, provider_usage_events,
};

fn obs_all_measured() -> ProviderUsageObservation {
    ProviderUsageObservation {
        schema: PROVIDER_USAGE_SCHEMA.to_string(),
        provider: "openai".into(),
        model: Some("gpt-4o-mini".into()),
        request_id: "req_deterministic_1".into(),
        route: None,
        service_tier: Some("default".into()),
        uncached_input_tokens: UsageAmount::measured(11, "p:uncached"),
        cached_read_input_tokens: UsageAmount::measured(2, "p:read"),
        cached_write_input_tokens: UsageAmount::measured(3, "p:write"),
        reasoning_tokens: UsageAmount::measured(7, "p:reason"),
        output_tokens: UsageAmount::measured(13, "p:output"),
        billed_tokens: UsageAmount::measured(33, "p:billed_tokens"),
        billed_microcredits: UsageAmount::measured(5000, "p:billed_cost"),
        credit_microcredits: UsageAmount::estimated(100, "p:credit"),
    }
}

fn obs_with_unmeasured() -> ProviderUsageObservation {
    let mut o = obs_all_measured();
    o.cached_write_input_tokens = UsageAmount::unmeasured("p:missing_write");
    o.credit_microcredits = UsageAmount::unmeasured("p:missing_credit");
    o
}

#[test]
fn provider_usage_events_deterministic_and_units() {
    use std::collections::{BTreeMap, BTreeSet};
    let obs = obs_all_measured();
    let a = provider_usage_events("task/root", UsageArm::Zero, &obs).expect("convert");
    let b = provider_usage_events("task/root", UsageArm::Zero, &obs).expect("convert2");
    assert_eq!(a, b, "conversion must be deterministic");

    // Index by coordinate to avoid positional coupling; also assert uniqueness.
    let mut by_coord: BTreeMap<UsageCoordinate, &zero_ledger::usage::UsageEvent> = BTreeMap::new();
    for ev in &a {
        assert!(
            by_coord.insert(ev.coordinate, ev).is_none(),
            "duplicate coordinate {:?}",
            ev.coordinate
        );
        ev.validate().expect("event validates");
        assert_eq!(ev.task_root, "task/root");
        assert_eq!(ev.arm, UsageArm::Zero);
        assert_eq!(ev.occurrence_id.as_deref(), Some(obs.request_id.as_str()));
        assert!(!ev.event_id.is_empty());
        assert!(!ev.provenance.is_empty());
        assert!(!ev.unit.is_empty());
    }
    let expected_coords: BTreeSet<UsageCoordinate> = [
        UsageCoordinate::UncachedInput,
        UsageCoordinate::CacheRead,
        UsageCoordinate::CacheWrite,
        UsageCoordinate::Reasoning,
        UsageCoordinate::VisibleOutput,
        UsageCoordinate::BilledTokens,
        UsageCoordinate::BilledCost,
        UsageCoordinate::ProviderCredit,
    ]
    .into_iter()
    .collect();
    assert_eq!(
        by_coord.keys().cloned().collect::<BTreeSet<_>>(),
        expected_coords,
        "complete coordinate set"
    );

    // Slug identity per coordinate (coordinate -> slug is stable).
    let slug_for = |c: UsageCoordinate| match c {
        UsageCoordinate::UncachedInput => "uncached_input",
        UsageCoordinate::CacheRead => "cache_read",
        UsageCoordinate::CacheWrite => "cache_write",
        UsageCoordinate::Reasoning => "reasoning",
        UsageCoordinate::VisibleOutput => "visible_output",
        UsageCoordinate::BilledTokens => "billed_tokens",
        UsageCoordinate::BilledCost => "billed_cost",
        UsageCoordinate::ProviderCredit => "provider_credit",
        _ => panic!("unexpected coordinate {c:?}"),
    };
    for (coord, ev) in &by_coord {
        let slug = slug_for(*coord);
        assert_eq!(
            ev.event_id,
            format!("{}:{}", obs.request_id, slug),
            "event_id for {coord:?}"
        );
        match coord {
            UsageCoordinate::BilledCost | UsageCoordinate::ProviderCredit => {
                assert_eq!(ev.unit, "microcredits", "unit for {coord:?}");
            }
            _ => assert_eq!(ev.unit, "tokens", "unit for {coord:?}"),
        }
    }

    // Observation and provenance per coordinate — honest, not aliased.
    assert_eq!(
        by_coord[&UsageCoordinate::UncachedInput].observation,
        ObservationKind::Measured
    );
    assert_eq!(
        by_coord[&UsageCoordinate::UncachedInput].provenance,
        "p:uncached"
    );
    assert_eq!(
        by_coord[&UsageCoordinate::CacheRead].observation,
        ObservationKind::Measured
    );
    assert_eq!(by_coord[&UsageCoordinate::CacheRead].provenance, "p:read");
    assert_eq!(
        by_coord[&UsageCoordinate::CacheWrite].observation,
        ObservationKind::Measured
    );
    assert_eq!(by_coord[&UsageCoordinate::CacheWrite].provenance, "p:write");
    assert_eq!(
        by_coord[&UsageCoordinate::Reasoning].observation,
        ObservationKind::Measured
    );
    assert_eq!(by_coord[&UsageCoordinate::Reasoning].provenance, "p:reason");
    assert_eq!(
        by_coord[&UsageCoordinate::VisibleOutput].observation,
        ObservationKind::Measured
    );
    assert_eq!(
        by_coord[&UsageCoordinate::VisibleOutput].provenance,
        "p:output"
    );
    assert_eq!(
        by_coord[&UsageCoordinate::BilledTokens].observation,
        ObservationKind::Measured
    );
    assert_eq!(
        by_coord[&UsageCoordinate::BilledTokens].provenance,
        "p:billed_tokens"
    );
    assert_eq!(
        by_coord[&UsageCoordinate::BilledCost].observation,
        ObservationKind::Measured
    );
    assert_eq!(
        by_coord[&UsageCoordinate::BilledCost].provenance,
        "p:billed_cost"
    );
    assert_eq!(
        by_coord[&UsageCoordinate::ProviderCredit].observation,
        ObservationKind::Estimated
    );
    assert_eq!(
        by_coord[&UsageCoordinate::ProviderCredit].provenance,
        "p:credit"
    );
}

#[test]
fn provider_usage_events_missing_data_honesty() {
    let obs = obs_with_unmeasured();
    let events = provider_usage_events("task/root", UsageArm::Baseline, &obs).expect("convert");
    let write = events
        .iter()
        .find(|e| e.coordinate == UsageCoordinate::CacheWrite)
        .unwrap();
    assert_eq!(write.observation, ObservationKind::Unmeasured);
    assert_eq!(write.amount, 0);
    let credit = events
        .iter()
        .find(|e| e.coordinate == UsageCoordinate::ProviderCredit)
        .unwrap();
    assert_eq!(credit.observation, ObservationKind::Unmeasured);
    assert_eq!(credit.amount, 0);
    let totals = coordinate_totals(&events).expect("totals");
    assert!(!totals.contains_key(&UsageCoordinate::CacheWrite));
    assert!(!totals.contains_key(&UsageCoordinate::ProviderCredit));
    assert_eq!(totals[&UsageCoordinate::UncachedInput], 11);
    assert_eq!(totals[&UsageCoordinate::BilledCost], 5000);
}

#[test]
fn provider_usage_new_coordinates_exist_and_roundtrip() {
    for coord in [
        UsageCoordinate::UncachedInput,
        UsageCoordinate::BilledTokens,
        UsageCoordinate::BilledCost,
    ] {
        let s = serde_json::to_string(&coord).unwrap();
        let back: UsageCoordinate = serde_json::from_str(&s).unwrap();
        assert_eq!(coord, back);
    }
    let c: UsageCoordinate = serde_json::from_str("\"uncached_input\"").unwrap();
    assert_eq!(c, UsageCoordinate::UncachedInput);
    assert_eq!(
        serde_json::from_str::<UsageCoordinate>("\"billed_tokens\"").unwrap(),
        UsageCoordinate::BilledTokens
    );
    assert_eq!(
        serde_json::from_str::<UsageCoordinate>("\"billed_cost\"").unwrap(),
        UsageCoordinate::BilledCost
    );
}

#[test]
fn provider_usage_events_validate_and_task_root() {
    let mut obs = obs_all_measured();
    obs.request_id = "".into();
    assert!(provider_usage_events("task/root", UsageArm::Zero, &obs).is_err());
    let obs2 = obs_all_measured();
    assert!(provider_usage_events("", UsageArm::Zero, &obs2).is_err());
}
