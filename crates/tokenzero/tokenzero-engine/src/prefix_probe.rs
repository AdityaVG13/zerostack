//! Provider-prefix probe types and fixture replay (ZS-BENCH-003).
//!
//! Types + fixture-driven skeleton only: no provider integration and no
//! network. The trial compares three arms -- raw-retained, retrospective
//! rewrite, and stable capsule -- by replaying recorded provider-visible
//! histories from a fixture and deriving measured prefix facts.
//!
//! Honest-telemetry law: provider eligibility and provider-reported cache
//! hits are declared facts carried through verbatim; they are never derived
//! from local history comparison. Local prefix overlap is a measured fact
//! (`lcp_tokens`, `quality_slot`) and is never called a hit. Every report
//! field is labeled measured vs declared in its doc.

use serde::{Deserialize, Serialize};

use crate::engine_common::common_prefix_len;

/// The three trial arms compared by the BENCH-003 provider-prefix probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeArm {
    /// Every provider-visible turn is retained raw; nothing is rewritten.
    RawRetained,
    /// Earlier turns are rewritten after the fact, shortening the reusable
    /// prefix across successive calls.
    RetrospectiveRewrite,
    /// A stable capsule prefix is held constant; only a volatile tail changes.
    StableCapsule,
}

/// Quality slot derived ONLY from measured local prefix overlap, never from
/// provider telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualitySlot {
    /// All reusable prefix tokens across successive calls were reused
    /// (`lcp_tokens == reusable_tokens`).
    ExactReuse,
    /// Some but not all reusable prefix tokens were reused.
    PartialReuse,
    /// No prefix tokens were reused.
    NoReuse,
}

/// One provider-visible chunk of an arm history. `tokens` is the measured
/// token cost of this chunk; identical text must carry the same token count
/// in every history so prefix comparison stays consistent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryChunk {
    pub text: String,
    pub tokens: u64,
}

/// One arm trial: successive provider-visible histories plus the measured and
/// declared facts recorded for the trial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArmTrial {
    pub arm: ProbeArm,
    /// Successive provider-visible histories, oldest first.
    pub histories: Vec<Vec<HistoryChunk>>,
    /// Measured: billed cost for the arm trial, milli-USD.
    pub cost_usd_milli: u64,
    /// Measured: wall latency of the arm trial, ms.
    pub latency_ms: u64,
    /// Measured: number of ref expansions during the arm trial.
    pub expansion_count: u64,
    /// Declared by provider telemetry: cache hit on the final call. `None`
    /// means no claim was made. Never derived from local prefix overlap.
    pub hit_declared_by_provider: Option<bool>,
    /// Declared by the harness under a named policy: whether the arm was
    /// provider-eligible for prefix caching. Never derived from histories.
    pub eligibility_declared: Option<bool>,
}

/// The fixture container for a prefix-probe replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeFixture {
    pub schema: String,
    pub arms: Vec<ArmTrial>,
}

/// Per-arm report: measured facts derived from the fixture histories plus the
/// declared facts carried through verbatim. Measured and declared fields are
/// never conflated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeReport {
    pub arm: ProbeArm,
    /// Measured: sum of longest-common-prefix token counts across all
    /// successive history pairs (derived locally via `common_prefix_len`).
    pub lcp_tokens: u64,
    /// Measured: total tokens of the arm's final provider-visible history.
    pub total_tokens: u64,
    /// Measured: billed cost for the arm trial, milli-USD.
    pub cost_usd_milli: u64,
    /// Measured: wall latency of the arm trial, ms.
    pub latency_ms: u64,
    /// Derived from measured LCP only; never a provider claim.
    pub quality_slot: QualitySlot,
    /// Measured: number of ref expansions during the arm trial.
    pub expansion_count: u64,
    /// Declared by provider telemetry on the final call; `None` = no claim.
    pub hit_declared_by_provider: Option<bool>,
    /// Declared by the harness under a named policy; never derived from
    /// histories and never conflated with a reported hit.
    pub eligibility_declared: Option<bool>,
}

fn chunk_tokens(history: &[HistoryChunk]) -> u64 {
    history.iter().map(|chunk| chunk.tokens).sum()
}

/// Replay a prefix-probe fixture and derive one [`ProbeReport`] per arm.
///
/// LCP between successive histories is computed in chunk space with the
/// shared [`common_prefix_len`] helper and converted to tokens; the report
/// carries the sum across all successive pairs. Declared fields
/// (`hit_declared_by_provider`, `eligibility_declared`) are copied verbatim
/// and never influence the derived facts.
pub fn replay_prefix_probe(fixture: &ProbeFixture) -> Vec<ProbeReport> {
    fixture
        .arms
        .iter()
        .map(|trial| {
            let mut lcp_tokens = 0u64;
            let mut reusable_tokens = 0u64;
            for pair in trial.histories.windows(2) {
                let (previous, next) = (&pair[0], &pair[1]);
                let shared = common_prefix_len(previous, next);
                lcp_tokens = lcp_tokens.saturating_add(chunk_tokens(&previous[..shared]));
                reusable_tokens = reusable_tokens.saturating_add(chunk_tokens(previous));
            }
            let total_tokens = trial
                .histories
                .last()
                .map(|history| chunk_tokens(history))
                .unwrap_or(0);
            let quality_slot = if trial.histories.len() < 2 || lcp_tokens == 0 {
                QualitySlot::NoReuse
            } else if lcp_tokens == reusable_tokens {
                QualitySlot::ExactReuse
            } else {
                QualitySlot::PartialReuse
            };
            ProbeReport {
                arm: trial.arm,
                lcp_tokens,
                total_tokens,
                cost_usd_milli: trial.cost_usd_milli,
                latency_ms: trial.latency_ms,
                quality_slot,
                expansion_count: trial.expansion_count,
                hit_declared_by_provider: trial.hit_declared_by_provider,
                eligibility_declared: trial.eligibility_declared,
            }
        })
        .collect()
}
