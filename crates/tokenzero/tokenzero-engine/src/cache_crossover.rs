//! Integer-only compress-vs-cache crossover policy.
//!
//! The receipt compares complete projected work. `common_overhead_tokens` is
//! the identical fixed work H (including already-compressed churn) included in
//! every candidate. Costs use labelled token-cost ppm and are not provider
//! telemetry, billing truth, or Q99 evidence.

use crate::CacheProvider;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const CACHE_CROSSOVER_SCHEMA: &str = "tokenzero.cache-crossover/v1";
pub const TOKEN_COST_PPM_SCALE: u64 = 1_000_000;
const MAX_ID_BYTES: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheContentClass {
    Stable,
    Churn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheCrossoverAction {
    CacheStable,
    Compress,
    KeepInline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheCrossoverReason {
    CachedStableCheaperOrEqual,
    CompressionStrictlyBeatsCache,
    BelowCacheableFloor,
    ChurnIsNotCacheable,
    CompressionNotAdmitted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheCrossoverInput {
    pub provider: CacheProvider,
    /// Names the provider/model/revision pricing policy behind the multiplier.
    pub policy_id: String,
    /// Identifies provider-locked counts or the exact labelled estimator.
    pub token_unit_id: String,
    pub content_class: CacheContentClass,
    pub original_tokens: u64,
    pub compressed_tokens: u64,
    /// Receipt/certificate id for independent candidate quality admission.
    /// Without one, the optimizer never selects compression.
    pub compression_admission_id: Option<String>,
    /// Complete identical work H, expressed in the same labelled token units.
    pub common_overhead_tokens: u64,
    /// Cached-read cost d in token-cost ppm. 100_000 means d=0.1.
    pub cached_read_multiplier_ppm: u64,
    /// Provider floor in the same token units as `original_tokens`.
    pub min_cacheable_tokens: u64,
    /// Tokens in the mutable suffix that cannot be cached; paid at full cost
    /// on every read. 0 reproduces the legacy model.
    #[serde(default)]
    pub suffix_size_tokens: u64,
    /// One-time token cost of performing the compaction/rewrite; amortized
    /// over `remaining_reuse_horizon` reads. 0 reproduces the legacy model.
    #[serde(default)]
    pub compaction_cost_tokens: u64,
    /// Expected total reads over the cache TTL, including this one. 1 = this
    /// read only (legacy single-read model); higher horizons amortize the
    /// one-time rewrite cost and re-evaluate the cached form per read.
    #[serde(default = "one")]
    pub remaining_reuse_horizon: u64,
}

const fn one() -> u64 {
    1
}

/// Engine-side knobs for the emission-path crossover call site
/// (ZS-CACHE-006). Defaults (d = 1.0, horizon = 1, floor 1000 tokens)
/// reproduce the historical `pick_cheaper` emission byte-for-byte: the
/// cached read costs exactly the inline read, so the decision reduces to
/// the plain token-count comparison between the flat and compact forms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmissionCrossoverConfig {
    /// Cached-read cost d in token-cost ppm. `TOKEN_COST_PPM_SCALE` means
    /// d = 1.0 (legacy-neutral).
    pub cached_read_multiplier_ppm: u64,
    /// Expected total reads over the TTL, including this one.
    pub remaining_reuse_horizon: u64,
    /// Provider floor in tokens; flat forms below it are never cached.
    pub min_cacheable_tokens: u64,
}

impl Default for EmissionCrossoverConfig {
    fn default() -> Self {
        Self {
            cached_read_multiplier_ppm: TOKEN_COST_PPM_SCALE,
            remaining_reuse_horizon: 1,
            min_cacheable_tokens: 1_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheCrossoverReceipt {
    pub schema: &'static str,
    pub provider: CacheProvider,
    pub policy_id: String,
    /// Identifies provider-locked counts or the exact labelled estimator.
    pub token_unit_id: String,
    pub content_class: CacheContentClass,
    pub original_tokens: u64,
    pub compressed_tokens: u64,
    pub compression_admission_id: Option<String>,
    pub common_overhead_tokens: u64,
    pub cached_read_multiplier_ppm: u64,
    pub min_cacheable_tokens: u64,
    pub action: CacheCrossoverAction,
    pub reason: CacheCrossoverReason,
    pub cache_eligible: bool,
    pub suffix_size_tokens: u64,
    pub compaction_cost_tokens: u64,
    pub remaining_reuse_horizon: u64,
    /// Complete H + original cost, scaled by `TOKEN_COST_PPM_SCALE`.
    pub inline_total_token_cost_ppm: u128,
    /// Complete H + (compaction + compressed) cost, scaled by
    /// `TOKEN_COST_PPM_SCALE`.
    pub compressed_total_token_cost_ppm: u128,
    /// Complete H + suffix + d*prefix cost. Hypothetical when cache is
    /// ineligible.
    pub cached_total_token_cost_ppm: u128,
    /// Horizon-weighted inline cost: H + h*original.
    pub inline_projected_token_cost_ppm: u128,
    /// Horizon-weighted compress cost: H + compaction + h*compressed.
    pub compressed_projected_token_cost_ppm: u128,
    /// Horizon-weighted cache cost:
    /// H + h*(suffix + d*(original - suffix)).
    pub cached_projected_token_cost_ppm: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CacheCrossoverError {
    #[error("cache crossover policy_id must be non-empty and at most 1024 bytes")]
    InvalidPolicyId,
    #[error("cache crossover token_unit_id must be non-empty and at most 1024 bytes")]
    InvalidTokenUnitId,
    #[error("compression admission id must be non-empty and at most 1024 bytes")]
    InvalidCompressionAdmissionId,
    #[error("cache crossover requires original_tokens > 0")]
    EmptyContent,
    #[error("cached-read multiplier must be in 1..=1000000 ppm")]
    InvalidCachedReadMultiplier,
    #[error("cache crossover requires a nonzero cacheability floor")]
    InvalidCacheableFloor,
    #[error("suffix_size_tokens must not exceed original_tokens")]
    InvalidSuffixSize,
    #[error("remaining_reuse_horizon must be at least 1")]
    InvalidReuseHorizon,
}

pub fn decide_cache_crossover(
    input: &CacheCrossoverInput,
) -> Result<CacheCrossoverReceipt, CacheCrossoverError> {
    validate_input(input)?;

    let overhead = u128::from(input.common_overhead_tokens) * u128::from(TOKEN_COST_PPM_SCALE);
    let scale = u128::from(TOKEN_COST_PPM_SCALE);
    let horizon = u128::from(input.remaining_reuse_horizon);
    let suffix = u128::from(input.suffix_size_tokens);
    let compaction = u128::from(input.compaction_cost_tokens);
    let original = u128::from(input.original_tokens);
    let compressed = u128::from(input.compressed_tokens);
    let multiplier = u128::from(input.cached_read_multiplier_ppm);
    let cacheable_prefix = original - suffix;

    // Single-read totals: the legacy model when the new inputs are at their
    // defaults (suffix = 0, compaction = 0, horizon = 1).
    let inline_total = overhead + original * scale;
    let compressed_total = overhead + (compaction + compressed) * scale;
    let cached_total = overhead + suffix * scale + cacheable_prefix * multiplier;

    // Horizon-weighted projections. The one-time compaction cost is paid
    // once and amortized over the remaining reuse horizon; every read pays
    // the mutable suffix at full cost and the cached prefix at the
    // multiplier. With horizon = 1 each projection equals its single-read
    // total, so the legacy decision is reproduced exactly.
    let inline_projected = overhead + horizon * original * scale;
    let compressed_projected = overhead + compaction * scale + horizon * compressed * scale;
    let cached_projected = overhead + horizon * (suffix * scale + cacheable_prefix * multiplier);

    let cache_eligible = input.content_class == CacheContentClass::Stable
        && cacheable_prefix >= u128::from(input.min_cacheable_tokens);

    let compression_admitted = input.compression_admission_id.is_some();
    let (action, reason) = match input.content_class {
        CacheContentClass::Stable if !compression_admitted && cache_eligible => (
            CacheCrossoverAction::CacheStable,
            CacheCrossoverReason::CompressionNotAdmitted,
        ),
        _ if !compression_admitted => (
            CacheCrossoverAction::KeepInline,
            CacheCrossoverReason::CompressionNotAdmitted,
        ),
        CacheContentClass::Churn => (
            smaller_inline_action(inline_projected, compressed_projected),
            CacheCrossoverReason::ChurnIsNotCacheable,
        ),
        CacheContentClass::Stable if !cache_eligible => (
            smaller_inline_action(inline_projected, compressed_projected),
            CacheCrossoverReason::BelowCacheableFloor,
        ),
        CacheContentClass::Stable if compressed_projected < cached_projected => (
            CacheCrossoverAction::Compress,
            CacheCrossoverReason::CompressionStrictlyBeatsCache,
        ),
        CacheContentClass::Stable => (
            CacheCrossoverAction::CacheStable,
            CacheCrossoverReason::CachedStableCheaperOrEqual,
        ),
    };

    Ok(CacheCrossoverReceipt {
        schema: CACHE_CROSSOVER_SCHEMA,
        provider: input.provider,
        policy_id: input.policy_id.clone(),
        token_unit_id: input.token_unit_id.clone(),
        content_class: input.content_class,
        original_tokens: input.original_tokens,
        compressed_tokens: input.compressed_tokens,
        compression_admission_id: input.compression_admission_id.clone(),
        common_overhead_tokens: input.common_overhead_tokens,
        cached_read_multiplier_ppm: input.cached_read_multiplier_ppm,
        min_cacheable_tokens: input.min_cacheable_tokens,
        action,
        reason,
        cache_eligible,
        suffix_size_tokens: input.suffix_size_tokens,
        compaction_cost_tokens: input.compaction_cost_tokens,
        remaining_reuse_horizon: input.remaining_reuse_horizon,
        inline_total_token_cost_ppm: inline_total,
        compressed_total_token_cost_ppm: compressed_total,
        cached_total_token_cost_ppm: cached_total,
        inline_projected_token_cost_ppm: inline_projected,
        compressed_projected_token_cost_ppm: compressed_projected,
        cached_projected_token_cost_ppm: cached_projected,
    })
}

const fn smaller_inline_action(inline_total: u128, compressed_total: u128) -> CacheCrossoverAction {
    if compressed_total < inline_total {
        CacheCrossoverAction::Compress
    } else {
        CacheCrossoverAction::KeepInline
    }
}

fn validate_input(input: &CacheCrossoverInput) -> Result<(), CacheCrossoverError> {
    if input.policy_id.is_empty() || input.policy_id.len() > MAX_ID_BYTES {
        return Err(CacheCrossoverError::InvalidPolicyId);
    }
    if input.token_unit_id.is_empty() || input.token_unit_id.len() > MAX_ID_BYTES {
        return Err(CacheCrossoverError::InvalidTokenUnitId);
    }
    if input
        .compression_admission_id
        .as_ref()
        .is_some_and(|id| id.is_empty() || id.len() > MAX_ID_BYTES)
    {
        return Err(CacheCrossoverError::InvalidCompressionAdmissionId);
    }
    if input.original_tokens == 0 {
        return Err(CacheCrossoverError::EmptyContent);
    }
    if !(1..=TOKEN_COST_PPM_SCALE).contains(&input.cached_read_multiplier_ppm) {
        return Err(CacheCrossoverError::InvalidCachedReadMultiplier);
    }
    if input.min_cacheable_tokens == 0 {
        return Err(CacheCrossoverError::InvalidCacheableFloor);
    }
    if input.suffix_size_tokens > input.original_tokens {
        return Err(CacheCrossoverError::InvalidSuffixSize);
    }
    if input.remaining_reuse_horizon == 0 {
        return Err(CacheCrossoverError::InvalidReuseHorizon);
    }
    Ok(())
}
