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
    let obs = obs_all_measured();
    let a = provider_usage_events("task/root", UsageArm::Zero, &obs).expect("convert");
    let b = provider_usage_events("task/root", UsageArm::Zero, &obs).expect("convert2");
    assert_eq!(a, b);
    assert_eq!(a.len(), 8);
    let expected_slugs = [
        "uncached_input",
        "cache_read",
        "cache_write",
        "reasoning",
        "visible_output",
        "billed_tokens",
        "billed_cost",
        "provider_credit",
    ];
    let expected_coords = [
        UsageCoordinate::UncachedInput,
        UsageCoordinate::CacheRead,
        UsageCoordinate::CacheWrite,
        UsageCoordinate::Reasoning,
        UsageCoordinate::VisibleOutput,
        UsageCoordinate::BilledTokens,
        UsageCoordinate::BilledCost,
        UsageCoordinate::ProviderCredit,
    ];
    for (i, ev) in a.iter().enumerate() {
        assert_eq!(
            ev.event_id,
            format!("{}:{}", obs.request_id, expected_slugs[i])
        );
        assert_eq!(ev.coordinate, expected_coords[i]);
        assert_eq!(ev.occurrence_id.as_deref(), Some(obs.request_id.as_str()));
        assert_eq!(ev.task_root, "task/root");
        assert_eq!(ev.arm, UsageArm::Zero);
    }
    for ev in &a {
        match ev.coordinate {
            UsageCoordinate::BilledCost | UsageCoordinate::ProviderCredit => {
                assert_eq!(ev.unit, "microcredits");
            }
            _ => assert_eq!(ev.unit, "tokens"),
        }
    }
    assert_eq!(a[0].observation, ObservationKind::Measured);
    assert_eq!(a[7].observation, ObservationKind::Estimated);
    assert_eq!(a[0].provenance, "p:uncached");
    assert_eq!(a[7].provenance, "p:credit");
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
