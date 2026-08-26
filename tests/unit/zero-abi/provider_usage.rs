use zero_abi::{PROVIDER_USAGE_SCHEMA, ProviderUsageObservation, UsageAmount, UsageMeasurement};

fn measured(amount: u64) -> UsageAmount {
    UsageAmount::measured(amount, "openai:usage.prompt_tokens")
}

fn unmeasured() -> UsageAmount {
    UsageAmount::unmeasured("openai:missing")
}

fn sample_observation() -> ProviderUsageObservation {
    ProviderUsageObservation {
        schema: PROVIDER_USAGE_SCHEMA.to_string(),
        provider: "openai".into(),
        model: Some("gpt-4o".into()),
        request_id: "req_123".into(),
        route: Some("/v1/chat/completions".into()),
        service_tier: None,
        uncached_input_tokens: measured(10),
        cached_read_input_tokens: measured(2),
        cached_write_input_tokens: unmeasured(),
        reasoning_tokens: measured(5),
        output_tokens: measured(20),
        billed_tokens: measured(37),
        billed_microcredits: UsageAmount::measured(1000, "openai:billed"),
        credit_microcredits: unmeasured(),
    }
}

#[test]
fn usage_amount_measured_requires_amount() {
    let mut amt = measured(1);
    amt.validate().expect("measured with amount ok");
    amt.amount = None;
    assert!(amt.validate().is_err());
}

#[test]
fn usage_amount_estimated_requires_amount() {
    let mut amt = UsageAmount::estimated(5, "prov");
    amt.validate().expect("estimated ok");
    amt.amount = None;
    assert!(amt.validate().is_err());
}

#[test]
fn usage_amount_unmeasured_forbids_amount() {
    let mut amt = unmeasured();
    amt.validate().expect("unmeasured none ok");
    amt.amount = Some(0);
    assert!(amt.validate().is_err());
    // measured zero is allowed (amount Some(0) is still Some)
    let zero = measured(0);
    zero.validate().expect("measured zero is valid");
}

#[test]
fn usage_amount_provenance_nonempty() {
    let mut amt = measured(1);
    amt.provenance = "".into();
    assert!(amt.validate().is_err());
    amt.provenance = "x".into();
    amt.measurement = UsageMeasurement::Unmeasured;
    amt.amount = None;
    amt.validate().expect("provenance nonempty passes");
}

#[test]
fn provider_observation_validates_schema_and_ids() {
    let mut obs = sample_observation();
    obs.validate().expect("valid");
    obs.schema = "wrong".into();
    assert!(obs.validate().is_err());
    obs.schema = PROVIDER_USAGE_SCHEMA.into();
    obs.provider = "".into();
    assert!(obs.validate().is_err());
    obs.provider = "openai".into();
    obs.request_id = "".into();
    assert!(obs.validate().is_err());
}

#[test]
fn provider_observation_serde_roundtrip() {
    let obs = sample_observation();
    let json = serde_json::to_string(&obs).expect("serialize");
    let decoded: ProviderUsageObservation = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(obs, decoded);
    decoded.validate().expect("roundtrip valid");
    // unmeasured amount is absent, not zero
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let cached_write = &v["cachedWriteInputTokens"];
    assert!(
        cached_write.get("amount").is_none() || cached_write["amount"].is_null(),
        "unmeasured should have no amount"
    );
    // measured zero roundtrips as Some(0)
    let mut with_zero = sample_observation();
    with_zero.uncached_input_tokens = measured(0);
    let j2 = serde_json::to_string(&with_zero).unwrap();
    let d2: ProviderUsageObservation = serde_json::from_str(&j2).unwrap();
    assert_eq!(d2.uncached_input_tokens.amount, Some(0));
    assert_eq!(
        d2.uncached_input_tokens.measurement,
        UsageMeasurement::Measured
    );
}

#[test]
fn usage_measurement_serde_snake_case() {
    let m = UsageMeasurement::Measured;
    let s = serde_json::to_string(&m).unwrap();
    assert_eq!(s, "\"measured\"");
    let e: UsageMeasurement = serde_json::from_str("\"estimated\"").unwrap();
    assert_eq!(e, UsageMeasurement::Estimated);
    let u: UsageMeasurement = serde_json::from_str("\"unmeasured\"").unwrap();
    assert_eq!(u, UsageMeasurement::Unmeasured);
}

#[test]
fn unknown_fields_rejected() {
    let json = serde_json::json!({
        "schema": PROVIDER_USAGE_SCHEMA,
        "provider": "openai",
        "requestId": "req_1",
        "uncachedInputTokens": {"measurement": "measured", "amount": 1, "provenance": "p"},
        "cachedReadInputTokens": {"measurement": "measured", "amount": 1, "provenance": "p"},
        "cachedWriteInputTokens": {"measurement": "unmeasured", "provenance": "p"},
        "reasoningTokens": {"measurement": "measured", "amount": 1, "provenance": "p"},
        "outputTokens": {"measurement": "measured", "amount": 1, "provenance": "p"},
        "billedTokens": {"measurement": "measured", "amount": 1, "provenance": "p"},
        "billedMicrocredits": {"measurement": "measured", "amount": 1, "provenance": "p"},
        "creditMicrocredits": {"measurement": "unmeasured", "provenance": "p"},
        "extra": 1
    });
    let res: Result<ProviderUsageObservation, _> = serde_json::from_value(json);
    assert!(res.is_err(), "unknown fields should be rejected");
}
