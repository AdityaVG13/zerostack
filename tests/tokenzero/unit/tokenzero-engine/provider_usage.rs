use serde_json::json;
use tokenzero_engine::{CacheProvider, parse_provider_usage, parse_provider_usage_observation};
use zero_abi::{PROVIDER_USAGE_SCHEMA, UsageMeasurement};

#[test]
fn openai_full_observation() {
    let value = json!({
        "id": "chatcmpl-openai-123",
        "model": "gpt-4o-mini",
        "service_tier": "flex",
        "usage": {
            "prompt_tokens": 100,
            "completion_tokens": 50,
            "total_tokens": 150,
            "prompt_tokens_details": { "cached_tokens": 30 },
            "completion_tokens_details": { "reasoning_tokens": 20 }
        }
    });
    let obs = parse_provider_usage_observation(CacheProvider::OpenAi, &value, None)
        .expect("openai parse");
    assert_eq!(obs.schema, PROVIDER_USAGE_SCHEMA);
    assert_eq!(obs.provider, "openai");
    assert_eq!(obs.model.as_deref(), Some("gpt-4o-mini"));
    assert_eq!(obs.request_id, "chatcmpl-openai-123");
    assert_eq!(obs.route.as_deref(), Some("chat.completions"));
    assert_eq!(obs.service_tier.as_deref(), Some("flex"));
    // uncached = prompt - cached = 70
    assert_eq!(
        obs.uncached_input_tokens.measurement,
        UsageMeasurement::Measured
    );
    assert_eq!(obs.uncached_input_tokens.amount, Some(70));
    assert_eq!(
        obs.cached_read_input_tokens.measurement,
        UsageMeasurement::Measured
    );
    assert_eq!(obs.cached_read_input_tokens.amount, Some(30));
    assert_eq!(
        obs.cached_write_input_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
    assert_eq!(obs.cached_write_input_tokens.amount, None);
    assert_eq!(obs.reasoning_tokens.measurement, UsageMeasurement::Measured);
    assert_eq!(obs.reasoning_tokens.amount, Some(20));
    assert_eq!(obs.output_tokens.measurement, UsageMeasurement::Measured);
    assert_eq!(obs.output_tokens.amount, Some(50));
    assert_eq!(obs.billed_tokens.measurement, UsageMeasurement::Measured);
    assert_eq!(obs.billed_tokens.amount, Some(150));
    assert_eq!(
        obs.billed_microcredits.measurement,
        UsageMeasurement::Unmeasured
    );
    assert_eq!(
        obs.credit_microcredits.measurement,
        UsageMeasurement::Unmeasured
    );
    obs.validate().expect("validate");
    // reuse parse_provider_usage unchanged
    let base = parse_provider_usage(CacheProvider::OpenAi, &value).unwrap();
    assert_eq!(base.input_tokens, 70);
    assert_eq!(base.cache_read_input_tokens, 30);
    assert!(base.cache_read_input_tokens_reported);
}

#[test]
fn gemini_full_observation() {
    let value = json!({
        "responseId": "resp-gemini-999",
        "modelVersion": "gemini-2.0-flash",
        "usageMetadata": {
            "promptTokenCount": 200,
            "cachedContentTokenCount": 40,
            "candidatesTokenCount": 60,
            "thoughtsTokenCount": 15,
            "totalTokenCount": 260
        }
    });
    let obs = parse_provider_usage_observation(CacheProvider::Gemini, &value, None)
        .expect("gemini parse");
    assert_eq!(obs.provider, "gemini");
    assert_eq!(obs.model.as_deref(), Some("gemini-2.0-flash"));
    assert_eq!(obs.request_id, "resp-gemini-999");
    assert_eq!(obs.route.as_deref(), Some("generateContent"));
    assert_eq!(obs.uncached_input_tokens.amount, Some(160)); // 200-40
    assert_eq!(obs.cached_read_input_tokens.amount, Some(40));
    assert_eq!(
        obs.cached_read_input_tokens.measurement,
        UsageMeasurement::Measured
    );
    assert_eq!(
        obs.cached_write_input_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
    assert_eq!(obs.reasoning_tokens.amount, Some(15));
    assert_eq!(obs.output_tokens.amount, Some(60));
    assert_eq!(obs.billed_tokens.amount, Some(260));
    assert_eq!(obs.billed_tokens.measurement, UsageMeasurement::Measured);
}

#[test]
fn anthropic_full_observation() {
    let value = json!({
        "id": "msg_anthropic_123",
        "model": "claude-3-5-sonnet-20241022",
        "usage": {
            "input_tokens": 80,
            "output_tokens": 40,
            "cache_read_input_tokens": 10,
            "cache_creation_input_tokens": 5
        }
    });
    let obs = parse_provider_usage_observation(CacheProvider::Anthropic, &value, None)
        .expect("anthropic parse");
    assert_eq!(obs.provider, "anthropic");
    assert_eq!(obs.request_id, "msg_anthropic_123");
    assert_eq!(obs.model.as_deref(), Some("claude-3-5-sonnet-20241022"));
    assert_eq!(obs.uncached_input_tokens.amount, Some(80));
    assert_eq!(obs.cached_read_input_tokens.amount, Some(10));
    assert_eq!(
        obs.cached_read_input_tokens.measurement,
        UsageMeasurement::Measured
    );
    assert_eq!(obs.cached_write_input_tokens.amount, Some(5));
    assert_eq!(
        obs.cached_write_input_tokens.measurement,
        UsageMeasurement::Measured
    );
    assert_eq!(
        obs.reasoning_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
    assert_eq!(obs.output_tokens.amount, Some(40));
    assert_eq!(obs.billed_tokens.measurement, UsageMeasurement::Unmeasured);
    assert_eq!(
        obs.billed_microcredits.measurement,
        UsageMeasurement::Unmeasured
    );
}

#[test]
fn cache_read_zero_is_measured() {
    let value = json!({
        "id": "chatcmpl-zero",
        "usage": {
            "prompt_tokens": 50,
            "completion_tokens": 10,
            "prompt_tokens_details": { "cached_tokens": 0 }
        }
    });
    let obs = parse_provider_usage_observation(CacheProvider::OpenAi, &value, None).unwrap();
    assert_eq!(
        obs.cached_read_input_tokens.measurement,
        UsageMeasurement::Measured
    );
    assert_eq!(obs.cached_read_input_tokens.amount, Some(0));
    assert_eq!(obs.uncached_input_tokens.amount, Some(50));
}

#[test]
fn missing_optional_fields_are_unmeasured_not_zero() {
    let value = json!({
        "id": "chatcmpl-missing",
        "usage": {
            "prompt_tokens": 10
            // no completion_tokens, no cached_tokens, no reasoning, no total
        }
    });
    let obs = parse_provider_usage_observation(CacheProvider::OpenAi, &value, None).unwrap();
    // uncached is Unmeasured when cached detail absent (cannot split)
    assert_eq!(
        obs.uncached_input_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
    assert_eq!(obs.uncached_input_tokens.amount, None);
    assert_eq!(obs.output_tokens.measurement, UsageMeasurement::Unmeasured);
    assert_eq!(obs.output_tokens.amount, None);
    assert_eq!(
        obs.cached_read_input_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
    assert_eq!(
        obs.reasoning_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
    assert_eq!(obs.billed_tokens.measurement, UsageMeasurement::Unmeasured);
    assert_eq!(
        obs.billed_microcredits.measurement,
        UsageMeasurement::Unmeasured
    );
    assert_eq!(
        obs.credit_microcredits.measurement,
        UsageMeasurement::Unmeasured
    );
    // existing parse_provider_usage should return 0 for missing optional fields
    let base = parse_provider_usage(CacheProvider::OpenAi, &value).unwrap();
    assert_eq!(base.output_tokens, 0);
    assert_eq!(base.cache_read_input_tokens, 0);
    assert!(!base.cache_read_input_tokens_reported);
}

#[test]
fn anthropic_cache_write_absent_is_unmeasured() {
    let value = json!({
        "id": "msg_no_write",
        "usage": {
            "input_tokens": 20,
            "output_tokens": 5,
            "cache_read_input_tokens": 2
            // no cache_creation
        }
    });
    let obs = parse_provider_usage_observation(CacheProvider::Anthropic, &value, None).unwrap();
    assert_eq!(
        obs.cached_write_input_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
    assert_eq!(obs.cached_write_input_tokens.amount, None);
    assert_eq!(
        obs.cached_read_input_tokens.measurement,
        UsageMeasurement::Measured
    );
}

#[test]
fn gemini_missing_thoughts_is_unmeasured() {
    let value = json!({
        "responseId": "resp-no-thoughts",
        "usageMetadata": {
            "promptTokenCount": 30,
            "candidatesTokenCount": 10
        }
    });
    let obs = parse_provider_usage_observation(CacheProvider::Gemini, &value, None).unwrap();
    assert_eq!(
        obs.reasoning_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
}

#[test]
fn request_id_precedence_transport_over_provider() {
    let value = json!({
        "id": "chatcmpl-provider-id",
        "responseId": "resp-should-not-win",
        "request_id": "req-should-not-win",
        "usage": {
            "prompt_tokens": 5,
            "completion_tokens": 5
        }
    });
    // transport wins
    let obs =
        parse_provider_usage_observation(CacheProvider::OpenAi, &value, Some("transport-xyz"))
            .unwrap();
    assert_eq!(obs.request_id, "transport-xyz");
    // without transport, id wins
    let obs2 = parse_provider_usage_observation(CacheProvider::OpenAi, &value, None).unwrap();
    assert_eq!(obs2.request_id, "chatcmpl-provider-id");
    // Gemini: responseId path
    let gem = json!({
        "responseId": "gem-resp-1",
        "request_id": "req-1",
        "usageMetadata": { "promptTokenCount": 1, "candidatesTokenCount": 1 }
    });
    let obs3 = parse_provider_usage_observation(CacheProvider::Gemini, &gem, None).unwrap();
    assert_eq!(obs3.request_id, "gem-resp-1");
    // Anthropic request_id field
    let ant = json!({
        "request_id": "anth-req-123",
        "usage": { "input_tokens": 1, "output_tokens": 1 }
    });
    let obs4 = parse_provider_usage_observation(CacheProvider::Anthropic, &ant, None).unwrap();
    assert_eq!(obs4.request_id, "anth-req-123");
}

#[test]
fn missing_request_id_is_error() {
    let value = json!({
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 5
        }
    });
    let err = parse_provider_usage_observation(CacheProvider::OpenAi, &value, None).unwrap_err();
    assert_eq!(
        err,
        tokenzero_engine::CacheMeterError::MissingField("request_id")
    );
    // whitespace transport id is immediately MissingField, does not fall back to provider id
    let err2 =
        parse_provider_usage_observation(CacheProvider::OpenAi, &value, Some("   ")).unwrap_err();
    assert_eq!(
        err2,
        tokenzero_engine::CacheMeterError::MissingField("request_id")
    );
    let with_id = json!({
        "id": "chatcmpl-should-not-fallback",
        "usage": { "prompt_tokens": 10, "completion_tokens": 5 }
    });
    let err3 =
        parse_provider_usage_observation(CacheProvider::OpenAi, &with_id, Some("  ")).unwrap_err();
    assert_eq!(
        err3,
        tokenzero_engine::CacheMeterError::MissingField("request_id")
    );
}

#[test]
fn microcredits_always_unmeasured_unless_explicit() {
    let value = json!({
        "id": "chatcmpl-micro",
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 10
        }
    });
    let obs = parse_provider_usage_observation(CacheProvider::OpenAi, &value, None).unwrap();
    assert_eq!(
        obs.billed_microcredits.measurement,
        UsageMeasurement::Unmeasured
    );
    assert_eq!(
        obs.credit_microcredits.measurement,
        UsageMeasurement::Unmeasured
    );
}

#[test]
fn openai_billed_unmeasured_when_total_absent() {
    let value = json!({
        "id": "chatcmpl-no-total",
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 5,
            "prompt_tokens_details": { "cached_tokens": 2 }
        }
    });
    let obs = parse_provider_usage_observation(CacheProvider::OpenAi, &value, None).unwrap();
    assert_eq!(obs.billed_tokens.measurement, UsageMeasurement::Unmeasured);
}

#[test]
fn observation_missing_usage_is_unmeasured_not_error() {
    // No usage object at all
    let value = json!({ "id": "chatcmpl-no-usage" });
    let obs = parse_provider_usage_observation(CacheProvider::OpenAi, &value, None).unwrap();
    assert_eq!(
        obs.uncached_input_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
    assert_eq!(
        obs.cached_read_input_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
    assert_eq!(
        obs.cached_write_input_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
    assert_eq!(obs.output_tokens.measurement, UsageMeasurement::Unmeasured);
    assert_eq!(
        obs.reasoning_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
    assert_eq!(obs.billed_tokens.measurement, UsageMeasurement::Unmeasured);
    // malformed present field still errors
    let bad = json!({ "id": "chatcmpl-bad", "usage": { "prompt_tokens": "not-a-number" } });
    let err = parse_provider_usage_observation(CacheProvider::OpenAi, &bad, None).unwrap_err();
    assert_eq!(
        err,
        tokenzero_engine::CacheMeterError::InvalidField("prompt_tokens")
    );
    // parse_provider_usage still errors on missing input (unchanged)
    assert!(parse_provider_usage(CacheProvider::OpenAi, &value).is_err());
}

#[test]
fn observation_missing_input_is_unmeasured() {
    let value = json!({
        "id": "chatcmpl-no-input",
        "usage": { "completion_tokens": 5 }
    });
    let obs = parse_provider_usage_observation(CacheProvider::OpenAi, &value, None).unwrap();
    assert_eq!(
        obs.uncached_input_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
    assert_eq!(
        obs.cached_read_input_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
    // Gemini missing input
    let gem = json!({
        "responseId": "resp-no-input",
        "usageMetadata": { "candidatesTokenCount": 5 }
    });
    let obs2 = parse_provider_usage_observation(CacheProvider::Gemini, &gem, None).unwrap();
    assert_eq!(
        obs2.uncached_input_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
    // Anthropic missing input
    let ant = json!({
        "id": "msg_no_input",
        "usage": { "output_tokens": 5 }
    });
    let obs3 = parse_provider_usage_observation(CacheProvider::Anthropic, &ant, None).unwrap();
    assert_eq!(
        obs3.uncached_input_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
}

#[test]
fn openai_input_without_cache_detail_is_unmeasured() {
    let value = json!({
        "id": "chatcmpl-raw-only",
        "usage": { "prompt_tokens": 100, "completion_tokens": 10 }
    });
    let obs = parse_provider_usage_observation(CacheProvider::OpenAi, &value, None).unwrap();
    // cached detail absent => cannot split, so uncached Unmeasured
    assert_eq!(
        obs.uncached_input_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
    assert_eq!(
        obs.cached_read_input_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
    // but output still Measured
    assert_eq!(obs.output_tokens.measurement, UsageMeasurement::Measured);
}

#[test]
fn gemini_input_without_cache_detail_is_unmeasured() {
    let gem = json!({
        "responseId": "resp-raw-only",
        "usageMetadata": { "promptTokenCount": 200, "candidatesTokenCount": 10 }
    });
    let obs = parse_provider_usage_observation(CacheProvider::Gemini, &gem, None).unwrap();
    assert_eq!(
        obs.uncached_input_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
    assert_eq!(
        obs.cached_read_input_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
}

#[test]
fn gemini_traffic_type_as_service_tier() {
    let gem = json!({
        "responseId": "resp-gem-traffic",
        "usageMetadata": {
            "promptTokenCount": 10,
            "candidatesTokenCount": 5,
            "trafficType": "ON_DEMAND"
        }
    });
    let obs = parse_provider_usage_observation(CacheProvider::Gemini, &gem, None).unwrap();
    assert_eq!(obs.service_tier.as_deref(), Some("ON_DEMAND"));
    // top-level still wins
    let gem2 = json!({
        "responseId": "resp-gem-traffic2",
        "service_tier": "batch",
        "usageMetadata": {
            "promptTokenCount": 10,
            "candidatesTokenCount": 5,
            "trafficType": "ON_DEMAND"
        }
    });
    let obs2 = parse_provider_usage_observation(CacheProvider::Gemini, &gem2, None).unwrap();
    assert_eq!(obs2.service_tier.as_deref(), Some("batch"));
}

#[test]
fn blank_model_is_absent_and_anthropic_foreign_reasoning_shape_is_unmeasured() {
    let openai = json!({
        "id": "chatcmpl-blank-model",
        "model": "   ",
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 5,
            "prompt_tokens_details": { "cached_tokens": 0 }
        }
    });
    let observation =
        parse_provider_usage_observation(CacheProvider::OpenAi, &openai, None).unwrap();
    assert_eq!(observation.model, None);

    let anthropic = json!({
        "id": "msg-foreign-reasoning",
        "usage": {
            "input_tokens": 10,
            "output_tokens": 5,
            "completion_tokens_details": { "reasoning_tokens": 3 }
        }
    });
    let observation =
        parse_provider_usage_observation(CacheProvider::Anthropic, &anthropic, None).unwrap();
    assert_eq!(
        observation.reasoning_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
}

#[test]
fn microcredit_invalid_field_names_kind() {
    let bad_billed = json!({
        "id": "chatcmpl-bad-billed",
        "usage": { "prompt_tokens": 10, "completion_tokens": 5, "billed_microcredits": "not-a-number" }
    });
    let err =
        parse_provider_usage_observation(CacheProvider::OpenAi, &bad_billed, None).unwrap_err();
    assert_eq!(
        err,
        tokenzero_engine::CacheMeterError::InvalidField("billed_microcredits")
    );
    let bad_credit = json!({
        "id": "chatcmpl-bad-credit",
        "usage": { "prompt_tokens": 10, "completion_tokens": 5, "credit_microcredits": "oops" }
    });
    let err2 =
        parse_provider_usage_observation(CacheProvider::OpenAi, &bad_credit, None).unwrap_err();
    assert_eq!(
        err2,
        tokenzero_engine::CacheMeterError::InvalidField("credit_microcredits")
    );
}

#[test]
fn legacy_parser_keeps_required_input_contract() {
    let value = json!({
        "id": "chatcmpl-bad",
        "usage": {
            "completion_tokens": 5
        }
    });
    assert!(parse_provider_usage(CacheProvider::OpenAi, &value).is_err());
    let observation =
        parse_provider_usage_observation(CacheProvider::OpenAi, &value, None).unwrap();
    assert_eq!(
        observation.uncached_input_tokens.measurement,
        UsageMeasurement::Unmeasured
    );
}
