//! Horizon-cost capsule admission estimator (ZS-VIEW-006).
//!
//! The legacy admission rule is a fixed byte threshold: in Auto mode,
//! local payloads larger than `capsule_exact_ref_threshold_bytes` are
//! admitted as exact refs (`render::local_payload_policy`). This module
//! adds an expected-value estimator trading payload size, estimated
//! expansion probability, the expected reuse horizon and the ref handling
//! cost.
//!
//! The default policy (`AdmissionPolicy::ByteThreshold`) reproduces the
//! legacy rule exactly; `AdmissionPolicy::HorizonCost` is opt-in via
//! `EngineConfig`. The estimator PROPOSES admission only -- capsule
//! formation and recovery-ref authority stay in `engine_read.rs` /
//! `tokenzero_core::make_capsule_with_recovery_ref`.

use serde::{Deserialize, Serialize};

pub const ADMISSION_SCHEMA: &str = "tokenzero.admission/v1";

/// Rough lexical token estimate for payload bytes (4 bytes/token). Used only
/// for horizon-cost comparisons, never for billing, telemetry, or Q99 claims.
pub const ADMISSION_BYTES_PER_TOKEN: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionPolicy {
    /// Fixed byte threshold: exact legacy behavior, estimator not consulted.
    #[default]
    ByteThreshold,
    /// Horizon-cost estimator (opt-in via `EngineConfig`).
    HorizonCost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionReason {
    AboveByteThreshold,
    BelowByteThreshold,
    RefAdmittedByHorizon,
    InlineCheaperThanRef,
    NoExpectedReuse,
    ExpansionAlways,
    EstimatesMissing,
}

/// One admission decision for one payload. Serialized form is the
/// proposal-side receipt; the engine remains free to refuse it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdmissionDecision {
    pub schema: &'static str,
    pub policy: AdmissionPolicy,
    pub admit_exact_ref: bool,
    pub reason: AdmissionReason,
    pub payload_bytes: usize,
    /// Per-mille estimate of the probability the payload is later expanded.
    pub expansion_probability_milli: u32,
    /// Expected remaining reads including this one.
    pub horizon: u64,
    /// Estimated handling cost of carrying the ref, in tokens.
    pub handling_cost_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionEstimator {
    /// Legacy byte threshold; reproduced exactly by the `ByteThreshold`
    /// policy and used as the baseline inside the horizon-cost model.
    pub exact_ref_threshold_bytes: usize,
    /// Default expansion probability per-mille when the caller has no
    /// replay-derived prediction (fixtures: `token-amplification-replay.json`).
    pub default_expansion_probability_milli: u32,
    /// Default expected remaining reads when the caller has no horizon.
    pub default_horizon: u64,
}

impl Default for AdmissionEstimator {
    fn default() -> Self {
        Self {
            exact_ref_threshold_bytes: crate::config::DEFAULT_CAPSULE_EXACT_REF_THRESHOLD_BYTES,
            default_expansion_probability_milli: 100,
            default_horizon: 1,
        }
    }
}

impl AdmissionEstimator {
    /// Legacy rule: admit the exact ref iff payload bytes exceed the
    /// threshold. This is the exact behavior `engine_read` shipped before
    /// V6-T6 and the `ByteThreshold` policy path.
    pub fn decide_threshold(&self, payload_bytes: usize) -> AdmissionDecision {
        let admit = payload_bytes > self.exact_ref_threshold_bytes;
        AdmissionDecision {
            schema: ADMISSION_SCHEMA,
            policy: AdmissionPolicy::ByteThreshold,
            admit_exact_ref: admit,
            reason: if admit {
                AdmissionReason::AboveByteThreshold
            } else {
                AdmissionReason::BelowByteThreshold
            },
            payload_bytes,
            expansion_probability_milli: 0,
            horizon: 1,
            handling_cost_tokens: 0,
        }
    }

    /// Horizon-cost expected-value model.
    ///
    /// ```text
    /// inline_cost = horizon * payload_tokens
    /// ref_cost    = handling_cost + p * horizon * payload_tokens
    /// ```
    ///
    /// Admit iff `ref_cost < inline_cost`, i.e.
    ///
    /// ```text
    /// handling_cost * 1000 < (1000 - p_milli) * horizon * payload_tokens
    /// ```
    ///
    /// A payload that always expands (`p_milli >= 1000`) or is never
    /// reused (`horizon == 0`) is never admitted; the ref must cover its
    /// own handling cost out of expected reuse value.
    pub fn decide_horizon_cost(
        &self,
        payload_bytes: usize,
        expansion_probability_milli: Option<u32>,
        horizon: Option<u64>,
        handling_cost_tokens: u64,
    ) -> AdmissionDecision {
        let (p_milli, horizon) = match (expansion_probability_milli, horizon) {
            (Some(p), Some(h)) => (p.min(1000), h),
            _ => {
                return AdmissionDecision {
                    schema: ADMISSION_SCHEMA,
                    policy: AdmissionPolicy::HorizonCost,
                    admit_exact_ref: false,
                    reason: AdmissionReason::EstimatesMissing,
                    payload_bytes,
                    expansion_probability_milli: expansion_probability_milli.unwrap_or(0),
                    horizon: horizon.unwrap_or(0),
                    handling_cost_tokens,
                };
            }
        };
        let payload_tokens = u128::from((payload_bytes / ADMISSION_BYTES_PER_TOKEN) as u64);
        let (admit, reason) = if horizon == 0 {
            (false, AdmissionReason::NoExpectedReuse)
        } else if p_milli >= 1000 {
            (false, AdmissionReason::ExpansionAlways)
        } else {
            let expected_savings =
                u128::from(1000 - p_milli) * u128::from(horizon) * payload_tokens;
            if u128::from(handling_cost_tokens) * 1000 < expected_savings {
                (true, AdmissionReason::RefAdmittedByHorizon)
            } else {
                (false, AdmissionReason::InlineCheaperThanRef)
            }
        };
        AdmissionDecision {
            schema: ADMISSION_SCHEMA,
            policy: AdmissionPolicy::HorizonCost,
            admit_exact_ref: admit,
            reason,
            payload_bytes,
            expansion_probability_milli: p_milli,
            horizon,
            handling_cost_tokens,
        }
    }
}

