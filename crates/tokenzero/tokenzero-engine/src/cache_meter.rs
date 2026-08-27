//! Provider cache-meter normalization and per-session cache economics.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use tokenzero_core::count_tokens;
pub use tokenzero_core::provider_cache::{
    ProviderCacheEligibility, ProviderCacheEligibilityStatus, ProviderCacheTelemetry,
};
use tokenzero_pulse::{AnytimeFailureMonitor, EProcessSnapshot};
use zero_abi::{PROVIDER_USAGE_SCHEMA, ProviderUsageObservation, UsageAmount};

pub const ANTHROPIC_CACHE_DIAGNOSIS_BETA: &str = "cache-diagnosis-2026-04-07";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheProvider {
    Anthropic,
    OpenAi,
    Gemini,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    /// True only when the provider response contained the cached-token field.
    #[serde(default)]
    pub cache_read_input_tokens_reported: bool,
    pub cache_creation_input_tokens: u64,
}

impl ProviderUsage {
    pub fn total_input_tokens(self) -> u64 {
        self.input_tokens
            .saturating_add(self.cache_read_input_tokens)
            .saturating_add(self.cache_creation_input_tokens)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CacheMeterError {
    #[error("missing provider usage field: {0}")]
    MissingField(&'static str),
    #[error("provider usage field is not an unsigned integer: {0}")]
    InvalidField(&'static str),
    #[error("invalid provider usage observation: {0}")]
    InvalidObservation(String),
    #[error("invalid cache uptime SLO configuration")]
    InvalidSloConfig,
    #[error("contradictory provider cache telemetry: {0}")]
    ContradictoryTelemetry(&'static str),
}

fn object_at<'a>(value: &'a Value, key: &str) -> &'a Value {
    value.get(key).unwrap_or(value)
}
fn required_u64(value: &Value, key: &'static str) -> Result<u64, CacheMeterError> {
    value
        .get(key)
        .ok_or(CacheMeterError::MissingField(key))?
        .as_u64()
        .ok_or(CacheMeterError::InvalidField(key))
}
fn optional_u64(value: &Value, key: &'static str) -> Result<u64, CacheMeterError> {
    value.get(key).map_or(Ok(0), |field| {
        field.as_u64().ok_or(CacheMeterError::InvalidField(key))
    })
}

fn optional_observed_u64(value: &Value, key: &'static str) -> Result<(u64, bool), CacheMeterError> {
    match value.get(key) {
        Some(field) => field
            .as_u64()
            .map(|value| (value, true))
            .ok_or(CacheMeterError::InvalidField(key)),
        None => Ok((0, false)),
    }
}

#[derive(Clone, Copy)]
enum CacheReadField {
    Flat(&'static str),
    Nested {
        object: &'static str,
        field: &'static str,
        error_name: &'static str,
    },
}

struct ProviderUsageLayout {
    usage_key: &'static str,
    input_key: &'static str,
    output_key: &'static str,
    cache_read: CacheReadField,
    cache_creation_key: Option<&'static str>,
    subtract_cached_from_input: bool,
    route: &'static str,
    model_key: Option<&'static str>,
    /// Response-root candidate keys carrying time-to-first-token in
    /// milliseconds; first present key wins, absence stays absent.
    ttft_keys: &'static [&'static str],
}

impl CacheProvider {
    fn usage_layout(self) -> ProviderUsageLayout {
        match self {
            Self::Anthropic => ProviderUsageLayout {
                usage_key: "usage",
                input_key: "input_tokens",
                output_key: "output_tokens",
                cache_read: CacheReadField::Flat("cache_read_input_tokens"),
                cache_creation_key: Some("cache_creation_input_tokens"),
                subtract_cached_from_input: false,
                route: "messages",
                model_key: Some("model"),
                ttft_keys: &["ttft", "time_to_first_token"],
            },
            Self::OpenAi => ProviderUsageLayout {
                usage_key: "usage",
                input_key: "prompt_tokens",
                output_key: "completion_tokens",
                cache_read: CacheReadField::Nested {
                    object: "prompt_tokens_details",
                    field: "cached_tokens",
                    error_name: "prompt_tokens_details.cached_tokens",
                },
                cache_creation_key: None,
                subtract_cached_from_input: true,
                route: "chat.completions",
                model_key: Some("model"),
                ttft_keys: &["response_time", "ttft", "time_to_first_token"],
            },
            Self::Gemini => ProviderUsageLayout {
                usage_key: "usageMetadata",
                input_key: "promptTokenCount",
                output_key: "candidatesTokenCount",
                cache_read: CacheReadField::Flat("cachedContentTokenCount"),
                cache_creation_key: None,
                subtract_cached_from_input: true,
                route: "generateContent",
                model_key: Some("modelVersion"),
                ttft_keys: &["ttft", "time_to_first_token"],
            },
        }
    }
}

/// Presence-sensitive time-to-first-token in milliseconds. A present field
/// is recorded verbatim; an absent field is recorded as `None`, never as 0.
fn read_response_ttft_ms(
    value: &Value,
    keys: &[&'static str],
) -> Result<Option<u64>, CacheMeterError> {
    for &key in keys {
        if let Some(field) = value.get(key) {
            return field
                .as_u64()
                .map(Some)
                .ok_or(CacheMeterError::InvalidField(key));
        }
    }
    Ok(None)
}

/// Presence-sensitive model identity from the provider response root.
fn read_response_model(
    value: &Value,
    key: &'static str,
) -> Result<Option<String>, CacheMeterError> {
    match value.get(key) {
        Some(field) => {
            let model = field
                .as_str()
                .ok_or(CacheMeterError::InvalidField(key))?
                .trim();
            Ok((!model.is_empty()).then(|| model.to_owned()))
        }
        None => Ok(None),
    }
}

fn read_cache_tokens(usage: &Value, field: CacheReadField) -> Result<(u64, bool), CacheMeterError> {
    match field {
        CacheReadField::Flat(key) => optional_observed_u64(usage, key),
        CacheReadField::Nested {
            object,
            field,
            error_name,
        } => match usage.get(object).and_then(|details| details.get(field)) {
            Some(value) => value
                .as_u64()
                .map(|tokens| (tokens, true))
                .ok_or(CacheMeterError::InvalidField(error_name)),
            None => Ok((0, false)),
        },
    }
}

pub fn parse_provider_usage(
    provider: CacheProvider,
    value: &Value,
) -> Result<ProviderUsage, CacheMeterError> {
    let layout = provider.usage_layout();
    let usage = object_at(value, layout.usage_key);
    let (cache_read_input_tokens, cache_read_input_tokens_reported) =
        read_cache_tokens(usage, layout.cache_read)?;
    let raw_input = required_u64(usage, layout.input_key)?;
    Ok(ProviderUsage {
        input_tokens: if layout.subtract_cached_from_input {
            raw_input.saturating_sub(cache_read_input_tokens)
        } else {
            raw_input
        },
        output_tokens: optional_u64(usage, layout.output_key)?,
        cache_read_input_tokens,
        cache_read_input_tokens_reported,
        cache_creation_input_tokens: match layout.cache_creation_key {
            Some(key) => optional_u64(usage, key)?,
            None => 0,
        },
    })
}

fn provider_str(provider: CacheProvider) -> &'static str {
    match provider {
        CacheProvider::Anthropic => "anthropic",
        CacheProvider::OpenAi => "openai",
        CacheProvider::Gemini => "gemini",
    }
}

fn measured_amount(amount: u64, provenance: String) -> UsageAmount {
    UsageAmount::measured(amount, provenance)
}

fn unmeasured_amount(provenance: String) -> UsageAmount {
    UsageAmount::unmeasured(provenance)
}
fn usage_amount(amount: Option<u64>, provenance: String) -> UsageAmount {
    match amount {
        Some(amount) => measured_amount(amount, provenance),
        None => unmeasured_amount(provenance),
    }
}

fn resolve_request_id(
    value: &Value,
    transport_request_id: Option<&str>,
) -> Result<String, CacheMeterError> {
    if let Some(explicit) = transport_request_id {
        let trimmed = explicit.trim();
        if trimmed.is_empty() {
            return Err(CacheMeterError::MissingField("request_id"));
        }
        return Ok(trimmed.to_owned());
    }
    for key in ["id", "responseId", "response_id", "request_id", "requestId"] {
        if let Some(field) = value.get(key).and_then(Value::as_str) {
            let trimmed = field.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_owned());
            }
        }
    }
    Err(CacheMeterError::MissingField("request_id"))
}

fn read_service_tier(value: &Value) -> Result<Option<String>, CacheMeterError> {
    for key in ["service_tier", "serviceTier"] {
        if let Some(field) = value.get(key) {
            if field.is_null() {
                continue;
            }
            let s = field
                .as_str()
                .ok_or(CacheMeterError::InvalidField("service_tier"))?;
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Ok(Some(trimmed.to_owned()));
            }
            return Ok(None);
        }
    }
    // Gemini: usageMetadata.trafficType as service_tier when top-level absent
    if let Some(usage) = value.get("usageMetadata") {
        if let Some(field) = usage.get("trafficType") {
            if !field.is_null() {
                let s = field
                    .as_str()
                    .ok_or(CacheMeterError::InvalidField("trafficType"))?;
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Ok(Some(trimmed.to_owned()));
                }
                return Ok(None);
            }
        }
    }
    Ok(None)
}

fn read_reasoning_amount(
    provider: CacheProvider,
    usage: &Value,
) -> Result<UsageAmount, CacheMeterError> {
    match provider {
        CacheProvider::OpenAi => {
            let prov = "openai:usage.completion_tokens_details.reasoning_tokens".to_owned();
            if let Some(details) = usage.get("completion_tokens_details") {
                if let Some(field) = details.get("reasoning_tokens") {
                    let amount = field.as_u64().ok_or(CacheMeterError::InvalidField(
                        "completion_tokens_details.reasoning_tokens",
                    ))?;
                    return Ok(measured_amount(amount, prov));
                }
            }
            Ok(unmeasured_amount(prov))
        }
        CacheProvider::Gemini => {
            let prov = "gemini:usageMetadata.thoughtsTokenCount".to_owned();
            if let Some(field) = usage.get("thoughtsTokenCount") {
                let amount = field
                    .as_u64()
                    .ok_or(CacheMeterError::InvalidField("thoughtsTokenCount"))?;
                return Ok(measured_amount(amount, prov));
            }
            Ok(unmeasured_amount(prov))
        }
        CacheProvider::Anthropic => {
            let prov = "anthropic:usage.reasoning_tokens".to_owned();
            if let Some(field) = usage.get("reasoning_tokens") {
                let amount = field
                    .as_u64()
                    .ok_or(CacheMeterError::InvalidField("reasoning_tokens"))?;
                return Ok(measured_amount(amount, prov));
            }
            Ok(unmeasured_amount(prov))
        }
    }
}

fn read_billed_tokens_amount(
    provider: CacheProvider,
    usage: &Value,
) -> Result<UsageAmount, CacheMeterError> {
    match provider {
        CacheProvider::OpenAi => {
            let prov = "openai:usage.total_tokens".to_owned();
            if let Some(field) = usage.get("total_tokens") {
                let amount = field
                    .as_u64()
                    .ok_or(CacheMeterError::InvalidField("total_tokens"))?;
                return Ok(measured_amount(amount, prov));
            }
            if let Some(field) = usage.get("totalTokens") {
                let amount = field
                    .as_u64()
                    .ok_or(CacheMeterError::InvalidField("totalTokens"))?;
                return Ok(measured_amount(amount, prov));
            }
            Ok(unmeasured_amount(prov))
        }
        CacheProvider::Gemini => {
            let prov = "gemini:usageMetadata.totalTokenCount".to_owned();
            if let Some(field) = usage.get("totalTokenCount") {
                let amount = field
                    .as_u64()
                    .ok_or(CacheMeterError::InvalidField("totalTokenCount"))?;
                return Ok(measured_amount(amount, prov));
            }
            Ok(unmeasured_amount(prov))
        }
        CacheProvider::Anthropic => {
            let prov = "anthropic:usage.total_tokens".to_owned();
            if let Some(field) = usage.get("total_tokens") {
                let amount = field
                    .as_u64()
                    .ok_or(CacheMeterError::InvalidField("total_tokens"))?;
                return Ok(measured_amount(amount, prov));
            }
            Ok(unmeasured_amount(prov))
        }
    }
}

fn read_microcredit_amount(
    provider: CacheProvider,
    usage: &Value,
    kind: &str,
) -> Result<UsageAmount, CacheMeterError> {
    let prov = format!("{}:usage.{}", provider_str(provider), kind);
    if let Some(field) = usage.get(kind) {
        let error_name: &'static str = match kind {
            "billed_microcredits" => "billed_microcredits",
            "credit_microcredits" => "credit_microcredits",
            _ => "billed_microcredits",
        };
        let amount = field
            .as_u64()
            .ok_or(CacheMeterError::InvalidField(error_name))?;
        return Ok(measured_amount(amount, prov));
    }
    Ok(unmeasured_amount(prov))
}

pub fn parse_provider_usage_observation(
    provider: CacheProvider,
    value: &Value,
    transport_request_id: Option<&str>,
) -> Result<ProviderUsageObservation, CacheMeterError> {
    let layout = provider.usage_layout();
    let request_id = resolve_request_id(value, transport_request_id)?;
    let route = Some(layout.route.to_owned());
    let model = match layout.model_key {
        Some(key) => read_response_model(value, key)?,
        None => None,
    };
    let service_tier = read_service_tier(value)?;
    let pstr = provider_str(provider);

    // Usage object: absent => all token fields Unmeasured; present but not object => error if expected
    let usage = match value.get(layout.usage_key) {
        Some(v) if v.is_object() => v,
        Some(v) if v.is_null() => v,
        Some(_) => {
            // Non-object usage value is malformed; treat as InvalidField for the usage key
            return Err(CacheMeterError::InvalidField(layout.usage_key));
        }
        None => &Value::Null,
    };

    let raw_input = match usage.get(layout.input_key) {
        Some(field) => Some(
            field
                .as_u64()
                .ok_or(CacheMeterError::InvalidField(layout.input_key))?,
        ),
        None => None,
    };
    let cached_read = {
        let (amount, present) = read_cache_tokens(usage, layout.cache_read)?;
        present.then_some(amount)
    };

    // For providers whose input total includes cached tokens, the uncached
    // coordinate is knowable only when the provider also reports the cached
    // component. Missing detail remains Unmeasured, never measured zero.
    let (uncached_input_tokens, cached_read_input_tokens) = match provider {
        CacheProvider::Anthropic => (
            usage_amount(raw_input, format!("{pstr}:usage.{}", layout.input_key)),
            usage_amount(
                cached_read,
                "anthropic:usage.cache_read_input_tokens".to_owned(),
            ),
        ),
        CacheProvider::OpenAi => (
            usage_amount(
                raw_input
                    .zip(cached_read)
                    .map(|(raw, cached)| raw.saturating_sub(cached)),
                format!("{pstr}:usage.{}", layout.input_key),
            ),
            usage_amount(
                cached_read,
                "openai:usage.prompt_tokens_details.cached_tokens".to_owned(),
            ),
        ),
        CacheProvider::Gemini => (
            usage_amount(
                raw_input
                    .zip(cached_read)
                    .map(|(raw, cached)| raw.saturating_sub(cached)),
                format!("{pstr}:usage.{}", layout.input_key),
            ),
            usage_amount(
                cached_read,
                "gemini:usageMetadata.cachedContentTokenCount".to_owned(),
            ),
        ),
    };

    let cached_write_input_tokens = match provider {
        CacheProvider::Anthropic => {
            let prov = "anthropic:usage.cache_creation_input_tokens".to_owned();
            if usage.is_null() {
                unmeasured_amount(prov)
            } else if let Some(field) = usage.get("cache_creation_input_tokens") {
                let amount = field
                    .as_u64()
                    .ok_or(CacheMeterError::InvalidField("cache_creation_input_tokens"))?;
                measured_amount(amount, prov)
            } else {
                unmeasured_amount(prov)
            }
        }
        _ => {
            let prov = format!("{pstr}:usage.cache_creation_input_tokens");
            unmeasured_amount(prov)
        }
    };

    let output_tokens = {
        let prov = format!("{pstr}:usage.{}", layout.output_key);
        if usage.is_null() {
            unmeasured_amount(prov)
        } else {
            match optional_observed_u64(usage, layout.output_key) {
                Ok((amount, true)) => measured_amount(amount, prov),
                Ok((_, false)) => unmeasured_amount(prov),
                Err(e) => return Err(e),
            }
        }
    };

    let reasoning_tokens = if usage.is_null() {
        // For absent usage, reasoning is unmeasured with provider-specific provenance
        let prov = match provider {
            CacheProvider::OpenAi => {
                "openai:usage.completion_tokens_details.reasoning_tokens".to_owned()
            }
            CacheProvider::Gemini => "gemini:usageMetadata.thoughtsTokenCount".to_owned(),
            CacheProvider::Anthropic => "anthropic:usage.reasoning_tokens".to_owned(),
        };
        unmeasured_amount(prov)
    } else {
        read_reasoning_amount(provider, usage)?
    };

    let billed_tokens = if usage.is_null() {
        let prov = match provider {
            CacheProvider::OpenAi => "openai:usage.total_tokens".to_owned(),
            CacheProvider::Gemini => "gemini:usageMetadata.totalTokenCount".to_owned(),
            CacheProvider::Anthropic => "anthropic:usage.total_tokens".to_owned(),
        };
        unmeasured_amount(prov)
    } else {
        read_billed_tokens_amount(provider, usage)?
    };

    let billed_microcredits = if usage.is_null() {
        let prov = format!("{pstr}:usage.billed_microcredits");
        unmeasured_amount(prov)
    } else {
        read_microcredit_amount(provider, usage, "billed_microcredits")?
    };
    let credit_microcredits = if usage.is_null() {
        let prov = format!("{pstr}:usage.credit_microcredits");
        unmeasured_amount(prov)
    } else {
        read_microcredit_amount(provider, usage, "credit_microcredits")?
    };

    let observation = ProviderUsageObservation {
        schema: PROVIDER_USAGE_SCHEMA.to_owned(),
        provider: pstr.to_owned(),
        model,
        request_id,
        route,
        service_tier,
        uncached_input_tokens,
        cached_read_input_tokens,
        cached_write_input_tokens,
        reasoning_tokens,
        output_tokens,
        billed_tokens,
        billed_microcredits,
        credit_microcredits,
    };
    observation
        .validate()
        .map_err(CacheMeterError::InvalidObservation)?;
    Ok(observation)
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CachePricing {
    pub input_per_million: f64,
    pub cache_read_per_million: f64,
    pub cache_creation_per_million: f64,
    pub output_per_million: f64,
}
impl CachePricing {
    pub fn realized_dollars(self, usage: ProviderUsage) -> f64 {
        (usage.input_tokens as f64 * self.input_per_million
            + usage.cache_read_input_tokens as f64 * self.cache_read_per_million
            + usage.cache_creation_input_tokens as f64 * self.cache_creation_per_million
            + usage.output_tokens as f64 * self.output_per_million)
            / 1_000_000.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnthropicCacheDiagnosisRequest {
    pub previous_message_id: String,
}
impl AnthropicCacheDiagnosisRequest {
    pub fn headers(&self) -> [(&'static str, &'static str); 1] {
        [("anthropic-beta", ANTHROPIC_CACHE_DIAGNOSIS_BETA)]
    }
    pub fn body(&self) -> Value {
        json!({ "previous_message_id": self.previous_message_id })
    }
}
pub fn cache_miss_attribution(value: &Value) -> Option<String> {
    let diagnosis = value
        .get("cache_diagnosis")
        .or_else(|| value.get("diagnosis"))
        .unwrap_or(value);
    ["cache_miss_reason", "miss_reason", "reason"]
        .into_iter()
        .find_map(|key| {
            diagnosis
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
}

fn provider_cache_telemetry(
    usage: ProviderUsage,
    diagnosis: Option<&Value>,
) -> Result<ProviderCacheTelemetry, CacheMeterError> {
    let reported_tokens = usage
        .cache_read_input_tokens_reported
        .then_some(usage.cache_read_input_tokens);
    match diagnosis.and_then(cache_miss_attribution).as_deref() {
        Some("expired" | "cache_expired" | "ttl_expired") => {
            if reported_tokens.is_some_and(|tokens| tokens > 0) {
                return Err(CacheMeterError::ContradictoryTelemetry(
                    "positive cached tokens reported with expiry",
                ));
            }
            Ok(ProviderCacheTelemetry::Expired)
        }
        Some("unknown" | "provider_unknown") => {
            if reported_tokens.is_some() {
                return Err(CacheMeterError::ContradictoryTelemetry(
                    "known cached-token count reported with unknown status",
                ));
            }
            Ok(ProviderCacheTelemetry::ReportedUnknown)
        }
        _ => Ok(ProviderCacheTelemetry::from_reported_cached_tokens(
            reported_tokens,
        )),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheObservation {
    pub provider: CacheProvider,
    /// Provider API route used for the request (e.g. `messages`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    /// Model identity reported by the provider response, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Presence-sensitive time to first token in milliseconds. Absence is
    /// recorded as `None`; it is never defaulted to 0.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<u64>,
    /// Per-observation cache key supplied by the caller, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_key: Option<String>,
    pub request_tokens: u64,
    pub stable_prefix_tokens: u64,
    pub churn_depth_tokens: u64,
    pub usage: ProviderUsage,
    /// Explicit provider-policy result; prefix identity alone never sets this.
    #[serde(default)]
    pub eligibility: ProviderCacheEligibility,
    /// Presence-sensitive provider response telemetry.
    #[serde(default)]
    pub provider_telemetry: ProviderCacheTelemetry,
    pub realized_dollars: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub miss_attribution: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CacheSloConfig {
    pub target_hit_rate: f64,
    pub regression_hit_rate: f64,
    pub alpha: f64,
    pub novelty_budget_tokens: u64,
}

impl Default for CacheSloConfig {
    fn default() -> Self {
        Self {
            target_hit_rate: 0.8,
            regression_hit_rate: 0.5,
            alpha: 0.05,
            novelty_budget_tokens: 1_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CacheSloDashboard {
    pub target_hit_rate: f64,
    /// Legacy numeric value; inspect `provider_measurement_available` first.
    pub measured_hit_rate: f64,
    pub provider_measurement_available: bool,
    pub error_budget_tokens: u64,
    pub error_budget_consumed_tokens: u64,
    pub error_budget_remaining_tokens: u64,
    pub novelty_budget_tokens: u64,
    pub novel_tokens: u64,
    pub novelty_budget_remaining_tokens: u64,
    pub burn_alert: bool,
    pub burn_monitor: EProcessSnapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CacheSessionReport {
    pub requests: u64,
    pub prefix_stability_ratio: f64,
    pub average_churn_depth_tokens: f64,
    /// Legacy ratio; unavailable cached-token telemetry contributes zero.
    pub hit_rate: f64,
    pub eligibility_evaluated_requests: u64,
    pub eligible_requests: u64,
    pub prefix_eligibility_rate: Option<f64>,
    pub provider_telemetry_requests: u64,
    pub provider_reported_hit_requests: u64,
    pub provider_reported_miss_requests: u64,
    pub provider_unavailable_requests: u64,
    pub provider_reported_unknown_requests: u64,
    pub provider_expired_requests: u64,
    pub provider_reported_hit_rate: Option<f64>,
    pub provider_reported_cached_token_ratio: Option<f64>,
    pub realized_dollars_per_request: f64,
    pub exact_miss_attributions: Vec<String>,
    pub token_amplification_milli: Option<u64>,
    pub effective_rate_limit_multiplier: f64,
    pub rate_limit_multiplier_certified: bool,
    pub cache_uptime: CacheSloDashboard,
}
#[derive(Debug, Default)]
pub struct CacheMeter {
    previous_request: Option<String>,
    observations: Vec<CacheObservation>,
}

impl CacheMeter {
    pub fn observe(
        &mut self,
        provider: CacheProvider,
        request: &str,
        usage_value: &Value,
        pricing: CachePricing,
        diagnosis: Option<&Value>,
    ) -> Result<&CacheObservation, CacheMeterError> {
        self.observe_with_eligibility(
            provider,
            request,
            ProviderCacheEligibility::not_evaluated(),
            usage_value,
            pricing,
            diagnosis,
            None,
        )
    }

    /// Observe one request with an explicit provider-policy evaluation.
    /// `cache_key` is the caller-known per-observation cache key, if any.
    pub fn observe_with_eligibility(
        &mut self,
        provider: CacheProvider,
        request: &str,
        eligibility: ProviderCacheEligibility,
        usage_value: &Value,
        pricing: CachePricing,
        diagnosis: Option<&Value>,
        cache_key: Option<&str>,
    ) -> Result<&CacheObservation, CacheMeterError> {
        let usage = parse_provider_usage(provider, usage_value)?;
        let provider_telemetry = provider_cache_telemetry(usage, diagnosis)?;
        let layout = provider.usage_layout();
        let ttft_ms = read_response_ttft_ms(usage_value, layout.ttft_keys)?;
        let model = match layout.model_key {
            Some(key) => read_response_model(usage_value, key)?,
            None => None,
        };
        let request_tokens = count_tokens(request) as u64;
        let stable_prefix_tokens = self.previous_request.as_deref().map_or(0, |previous| {
            count_tokens(common_prefix(previous, request)) as u64
        });
        self.observations.push(CacheObservation {
            provider,
            route: Some(layout.route.to_owned()),
            model,
            ttft_ms,
            cache_key: cache_key.map(str::to_owned),
            request_tokens,
            stable_prefix_tokens,
            churn_depth_tokens: stable_prefix_tokens,
            usage,
            eligibility,
            provider_telemetry,
            realized_dollars: pricing.realized_dollars(usage),
            miss_attribution: diagnosis.and_then(cache_miss_attribution),
        });
        self.previous_request = Some(request.to_owned());
        Ok(self
            .observations
            .last()
            .expect("observation was just pushed"))
    }
    pub fn observations(&self) -> &[CacheObservation] {
        &self.observations
    }
    pub fn report(&self) -> CacheSessionReport {
        self.report_with_slo(CacheSloConfig::default(), None)
            .expect("default cache SLO is valid")
    }
    pub fn report_with_slo(
        &self,
        config: CacheSloConfig,
        token_amplification_milli: Option<u64>,
    ) -> Result<CacheSessionReport, CacheMeterError> {
        let requests = self.observations.len() as u64;
        let stable = self
            .observations
            .iter()
            .map(|item| item.stable_prefix_tokens)
            .sum::<u64>();
        let request_mass = self
            .observations
            .iter()
            .map(|item| item.request_tokens)
            .sum::<u64>();
        let eligibility_evaluated_requests = self
            .observations
            .iter()
            .filter(|item| item.eligibility.is_evaluated())
            .count() as u64;
        let eligible_requests = self
            .observations
            .iter()
            .filter(|item| item.eligibility.is_eligible())
            .count() as u64;
        let provider_reported_hit_requests = self
            .observations
            .iter()
            .filter(|item| item.provider_telemetry.is_reported_hit())
            .count() as u64;
        let provider_reported_miss_requests = self
            .observations
            .iter()
            .filter(|item| item.provider_telemetry.is_reported_miss())
            .count() as u64;
        let provider_unavailable_requests = self
            .observations
            .iter()
            .filter(|item| matches!(item.provider_telemetry, ProviderCacheTelemetry::Unavailable))
            .count() as u64;
        let provider_reported_unknown_requests = self
            .observations
            .iter()
            .filter(|item| {
                matches!(
                    item.provider_telemetry,
                    ProviderCacheTelemetry::ReportedUnknown
                )
            })
            .count() as u64;
        let provider_expired_requests = self
            .observations
            .iter()
            .filter(|item| matches!(item.provider_telemetry, ProviderCacheTelemetry::Expired))
            .count() as u64;
        let provider_telemetry_requests =
            provider_reported_hit_requests.saturating_add(provider_reported_miss_requests);
        let legacy_cached = self.observations.iter().fold(0u64, |total, item| {
            total.saturating_add(item.usage.cache_read_input_tokens)
        });
        let legacy_input = self.observations.iter().fold(0u64, |total, item| {
            total.saturating_add(item.usage.total_input_tokens())
        });
        let reported = self
            .observations
            .iter()
            .filter(|item| item.provider_telemetry.is_hit_rate_observation())
            .collect::<Vec<_>>();
        let cached = reported.iter().fold(0u64, |total, item| {
            total.saturating_add(
                item.provider_telemetry
                    .cached_input_tokens()
                    .expect("hit-rate observations carry cached-token telemetry"),
            )
        });
        let input = reported.iter().fold(0u64, |total, item| {
            total.saturating_add(item.usage.total_input_tokens())
        });
        let churn = self
            .observations
            .iter()
            .skip(1)
            .map(|item| item.churn_depth_tokens)
            .sum::<u64>();
        let transitions = requests.saturating_sub(1);
        let dollars = self
            .observations
            .iter()
            .map(|item| item.realized_dollars)
            .sum::<f64>();
        let provider_reported_cached_token_ratio = optional_ratio(cached, input);
        let provider_reported_hit_rate =
            optional_ratio(provider_reported_hit_requests, provider_telemetry_requests);
        let prefix_eligibility_rate =
            optional_ratio(eligible_requests, eligibility_evaluated_requests);
        let legacy_hit_rate = ratio(legacy_cached, legacy_input);
        let measured_provider_hit_rate = provider_reported_cached_token_ratio.unwrap_or(0.0);
        let novel_tokens = input.saturating_sub(cached);
        let null_failure_rate = 1.0 - config.target_hit_rate;
        let alternative_failure_rate = 1.0 - config.regression_hit_rate;
        let mut burn_monitor =
            AnytimeFailureMonitor::new(config.alpha, null_failure_rate, alternative_failure_rate)
                .map_err(|_| CacheMeterError::InvalidSloConfig)?;
        let burn_snapshot = burn_monitor.observe_counts(novel_tokens, cached);
        let error_budget_tokens = (input as f64 * null_failure_rate).floor() as u64;
        let effective_rate_limit_multiplier = if novel_tokens == 0 {
            if input == 0 { 1.0 } else { input as f64 }
        } else {
            input as f64 / novel_tokens as f64
        };
        Ok(CacheSessionReport {
            requests,
            prefix_stability_ratio: ratio(stable, request_mass),
            average_churn_depth_tokens: if transitions == 0 {
                0.0
            } else {
                churn as f64 / transitions as f64
            },
            hit_rate: legacy_hit_rate,
            eligibility_evaluated_requests,
            eligible_requests,
            prefix_eligibility_rate,
            provider_telemetry_requests,
            provider_reported_hit_requests,
            provider_reported_miss_requests,
            provider_unavailable_requests,
            provider_reported_unknown_requests,
            provider_expired_requests,
            provider_reported_hit_rate,
            provider_reported_cached_token_ratio,
            realized_dollars_per_request: if requests == 0 {
                0.0
            } else {
                dollars / requests as f64
            },
            exact_miss_attributions: self
                .observations
                .iter()
                .filter_map(|item| item.miss_attribution.clone())
                .collect(),
            token_amplification_milli,
            effective_rate_limit_multiplier,
            rate_limit_multiplier_certified: input > 0,
            cache_uptime: CacheSloDashboard {
                target_hit_rate: config.target_hit_rate,
                measured_hit_rate: measured_provider_hit_rate,
                provider_measurement_available: provider_reported_cached_token_ratio.is_some(),
                error_budget_tokens,
                error_budget_consumed_tokens: novel_tokens,
                error_budget_remaining_tokens: error_budget_tokens.saturating_sub(novel_tokens),
                novelty_budget_tokens: config.novelty_budget_tokens,
                novel_tokens,
                novelty_budget_remaining_tokens: config
                    .novelty_budget_tokens
                    .saturating_sub(novel_tokens),
                burn_alert: burn_snapshot.tripped,
                burn_monitor: burn_snapshot,
            },
        })
    }
}
fn common_prefix<'a>(left: &'a str, right: &str) -> &'a str {
    let mut end = 0;
    for ((left_index, left_char), (_, right_char)) in left.char_indices().zip(right.char_indices())
    {
        if left_char != right_char {
            break;
        }
        end = left_index + left_char.len_utf8();
    }
    &left[..end]
}
fn ratio(numerator: u64, denominator: u64) -> f64 {
    optional_ratio(numerator, denominator).unwrap_or(0.0)
}

fn optional_ratio(numerator: u64, denominator: u64) -> Option<f64> {
    (denominator != 0).then(|| numerator as f64 / denominator as f64)
}
