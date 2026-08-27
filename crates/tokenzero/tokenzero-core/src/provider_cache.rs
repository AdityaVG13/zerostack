//! Provider-cache facts with explicit eligibility/telemetry separation.
//!
//! Byte-identical stable prefixes are local facts. A provider adapter may
//! separately declare eligibility under a named policy. Only provider response
//! telemetry can declare a hit, miss, expiry, or unknown result.

use crate::decision_view::StablePrefixGeometry;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use std::num::NonZeroU64;
use zero_abi::Sha256Digest;

const MAX_POLICY_ID_BYTES: usize = 1_024;
const MAX_REASON_BYTES: usize = 16_384;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderCacheError {
    EmptyPolicyId,
    PolicyIdTooLong,
    EmptyIneligibilityReason,
    IneligibilityReasonTooLong,
}

impl fmt::Display for ProviderCacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid provider-cache fact: {self:?}")
    }
}

impl Error for ProviderCacheError {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCacheEligibilityStatus {
    NotEvaluated,
    Eligible,
    Ineligible,
}

/// Provider-policy evaluation. This is never derived from prefix equality.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProviderCacheEligibility {
    status: ProviderCacheEligibilityStatus,
    policy_id: Option<String>,
    prefix_geometry_digest: Option<Sha256Digest>,
    breakpoint_after_tokens: Option<u64>,
    reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderCacheEligibilityWire {
    status: ProviderCacheEligibilityStatus,
    policy_id: Option<String>,
    prefix_geometry_digest: Option<Sha256Digest>,
    breakpoint_after_tokens: Option<u64>,
    reason: Option<String>,
}

impl<'de> Deserialize<'de> for ProviderCacheEligibility {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ProviderCacheEligibilityWire::deserialize(deserializer)?;
        match wire.status {
            ProviderCacheEligibilityStatus::NotEvaluated => {
                if wire.policy_id.is_some()
                    || wire.prefix_geometry_digest.is_some()
                    || wire.breakpoint_after_tokens.is_some()
                    || wire.reason.is_some()
                {
                    return Err(serde::de::Error::custom(
                        "not_evaluated provider eligibility carries fields",
                    ));
                }
                Ok(Self::not_evaluated())
            }
            ProviderCacheEligibilityStatus::Eligible => {
                let policy_id = wire
                    .policy_id
                    .ok_or_else(|| serde::de::Error::missing_field("policy_id"))?;
                let policy_id = validate_policy_id(policy_id)
                    .map_err(|error| serde::de::Error::custom(error.to_string()))?;
                let prefix_geometry_digest = wire
                    .prefix_geometry_digest
                    .ok_or_else(|| serde::de::Error::missing_field("prefix_geometry_digest"))?;
                let breakpoint_after_tokens = wire
                    .breakpoint_after_tokens
                    .ok_or_else(|| serde::de::Error::missing_field("breakpoint_after_tokens"))?;
                if wire.reason.is_some() {
                    return Err(serde::de::Error::custom(
                        "eligible provider eligibility carries an ineligibility reason",
                    ));
                }
                Ok(Self {
                    status: wire.status,
                    policy_id: Some(policy_id),
                    prefix_geometry_digest: Some(prefix_geometry_digest),
                    breakpoint_after_tokens: Some(breakpoint_after_tokens),
                    reason: None,
                })
            }
            ProviderCacheEligibilityStatus::Ineligible => {
                if wire.prefix_geometry_digest.is_some() || wire.breakpoint_after_tokens.is_some() {
                    return Err(serde::de::Error::custom(
                        "ineligible provider eligibility carries prefix geometry",
                    ));
                }
                Self::ineligible(
                    wire.policy_id
                        .ok_or_else(|| serde::de::Error::missing_field("policy_id"))?,
                    wire.reason
                        .ok_or_else(|| serde::de::Error::missing_field("reason"))?,
                )
                .map_err(|error| serde::de::Error::custom(error.to_string()))
            }
        }
    }
}

impl Default for ProviderCacheEligibility {
    fn default() -> Self {
        Self::not_evaluated()
    }
}

impl ProviderCacheEligibility {
    pub const fn not_evaluated() -> Self {
        Self {
            status: ProviderCacheEligibilityStatus::NotEvaluated,
            policy_id: None,
            prefix_geometry_digest: None,
            breakpoint_after_tokens: None,
            reason: None,
        }
    }

    /// Record an explicit provider-policy decision over exact prefix geometry.
    pub fn eligible(
        policy_id: impl Into<String>,
        geometry: &StablePrefixGeometry,
    ) -> Result<Self, ProviderCacheError> {
        let policy_id = validate_policy_id(policy_id.into())?;
        Ok(Self {
            status: ProviderCacheEligibilityStatus::Eligible,
            policy_id: Some(policy_id),
            prefix_geometry_digest: Some(geometry.digest()),
            breakpoint_after_tokens: Some(geometry.breakpoint_after_tokens()),
            reason: None,
        })
    }

    pub fn ineligible(
        policy_id: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<Self, ProviderCacheError> {
        let policy_id = validate_policy_id(policy_id.into())?;
        let reason = reason.into();
        if reason.is_empty() {
            return Err(ProviderCacheError::EmptyIneligibilityReason);
        }
        if reason.len() > MAX_REASON_BYTES {
            return Err(ProviderCacheError::IneligibilityReasonTooLong);
        }
        Ok(Self {
            status: ProviderCacheEligibilityStatus::Ineligible,
            policy_id: Some(policy_id),
            prefix_geometry_digest: None,
            breakpoint_after_tokens: None,
            reason: Some(reason),
        })
    }

    pub const fn status(&self) -> ProviderCacheEligibilityStatus {
        self.status
    }

    pub fn policy_id(&self) -> Option<&str> {
        self.policy_id.as_deref()
    }

    pub const fn prefix_geometry_digest(&self) -> Option<Sha256Digest> {
        self.prefix_geometry_digest
    }

    pub const fn breakpoint_after_tokens(&self) -> Option<u64> {
        self.breakpoint_after_tokens
    }

    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    pub const fn is_evaluated(&self) -> bool {
        !matches!(self.status, ProviderCacheEligibilityStatus::NotEvaluated)
    }

    pub const fn is_eligible(&self) -> bool {
        matches!(self.status, ProviderCacheEligibilityStatus::Eligible)
    }
}

/// Closed provider-reported cache result. `Unavailable` is not a miss.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum ProviderCacheTelemetry {
    #[default]
    Unavailable,
    ReportedHit {
        cached_input_tokens: NonZeroU64,
    },
    ReportedMiss,
    ReportedUnknown,
    Expired,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProviderCacheTelemetryStatusWire {
    Unavailable,
    ReportedHit,
    ReportedMiss,
    ReportedUnknown,
    Expired,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderCacheTelemetryWire {
    status: ProviderCacheTelemetryStatusWire,
    cached_input_tokens: Option<NonZeroU64>,
}

impl<'de> Deserialize<'de> for ProviderCacheTelemetry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ProviderCacheTelemetryWire::deserialize(deserializer)?;
        match (wire.status, wire.cached_input_tokens) {
            (ProviderCacheTelemetryStatusWire::Unavailable, None) => Ok(Self::Unavailable),
            (ProviderCacheTelemetryStatusWire::ReportedHit, Some(cached_input_tokens)) => {
                Ok(Self::ReportedHit {
                    cached_input_tokens,
                })
            }
            (ProviderCacheTelemetryStatusWire::ReportedMiss, None) => Ok(Self::ReportedMiss),
            (ProviderCacheTelemetryStatusWire::ReportedUnknown, None) => Ok(Self::ReportedUnknown),
            (ProviderCacheTelemetryStatusWire::Expired, None) => Ok(Self::Expired),
            (ProviderCacheTelemetryStatusWire::ReportedHit, None) => {
                Err(serde::de::Error::missing_field("cached_input_tokens"))
            }
            (
                ProviderCacheTelemetryStatusWire::Unavailable
                | ProviderCacheTelemetryStatusWire::ReportedMiss
                | ProviderCacheTelemetryStatusWire::ReportedUnknown
                | ProviderCacheTelemetryStatusWire::Expired,
                Some(_),
            ) => Err(serde::de::Error::custom(
                "non-hit provider telemetry carries cached input tokens",
            )),
        }
    }
}

impl ProviderCacheTelemetry {
    /// Convert presence-sensitive provider token telemetry without inference.
    pub const fn from_reported_cached_tokens(cached_input_tokens: Option<u64>) -> Self {
        match cached_input_tokens {
            None => Self::Unavailable,
            Some(0) => Self::ReportedMiss,
            Some(cached_input_tokens) => match NonZeroU64::new(cached_input_tokens) {
                Some(cached_input_tokens) => Self::ReportedHit {
                    cached_input_tokens,
                },
                None => Self::ReportedMiss,
            },
        }
    }

    pub const fn is_reported_hit(&self) -> bool {
        matches!(self, Self::ReportedHit { .. })
    }

    pub const fn is_reported_miss(&self) -> bool {
        matches!(self, Self::ReportedMiss)
    }

    /// True only for hit/miss telemetry suitable for a hit-rate denominator.
    pub const fn is_hit_rate_observation(&self) -> bool {
        matches!(self, Self::ReportedHit { .. } | Self::ReportedMiss)
    }

    pub const fn cached_input_tokens(&self) -> Option<u64> {
        match self {
            Self::ReportedHit {
                cached_input_tokens,
            } => Some(cached_input_tokens.get()),
            Self::ReportedMiss => Some(0),
            Self::Unavailable | Self::ReportedUnknown | Self::Expired => None,
        }
    }
}

fn validate_policy_id(policy_id: String) -> Result<String, ProviderCacheError> {
    if policy_id.is_empty() {
        return Err(ProviderCacheError::EmptyPolicyId);
    }
    if policy_id.len() > MAX_POLICY_ID_BYTES {
        return Err(ProviderCacheError::PolicyIdTooLong);
    }
    Ok(policy_id)
}

