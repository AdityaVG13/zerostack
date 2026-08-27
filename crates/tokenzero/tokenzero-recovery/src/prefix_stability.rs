//! Cache-prefix stability invariants shared by renderers and golden tests.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use thiserror::Error;
use tokenzero_core::{count_tokens, sha256_hex};

pub const MAX_CACHE_BLOCKS_PER_TURN: usize = 15;
/// The public-runtime estimator and provider tokenizers differ near cache
/// floors. Requiring 5 estimator tokens for every 4 provider tokens is a
/// conservative 25% boundary tolerance: warn early rather than claim caching.
pub const ESTIMATOR_FLOOR_SAFETY_NUMERATOR: usize = 5;
pub const ESTIMATOR_FLOOR_SAFETY_DENOMINATOR: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheModelTier {
    Opus,
    FableOrSonnet46,
    OlderSonnet,
}

impl CacheModelTier {
    pub const fn min_cacheable_tokens(self) -> usize {
        match self {
            Self::Opus => 4_096,
            Self::FableOrSonnet46 => 2_048,
            Self::OlderSonnet => 1_024,
        }
    }

    pub const fn min_cacheable_estimator_tokens(self) -> usize {
        self.min_cacheable_tokens()
            .saturating_mul(ESTIMATOR_FLOOR_SAFETY_NUMERATOR)
            .div_ceil(ESTIMATOR_FLOOR_SAFETY_DENOMINATOR)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderObservation<'a> {
    pub content: &'a str,
    pub rendered: &'a str,
    pub level: &'a str,
    /// A real tokenizer identifier, or an explicitly labelled estimator id.
    pub tokenizer_id: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheablePrefix {
    pub bytes: String,
    pub cache_breakpoint: bool,
    /// Number of provider cache-control blocks attributed to each turn.
    pub blocks_per_turn: BTreeMap<u64, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixStabilityAlert {
    BelowCacheableFloor {
        observed_tokens: usize,
        required_tokens: usize,
        model_tier: CacheModelTier,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PrefixStabilityViolation {
    #[error("cacheable prefix changed between provider cache breakpoints")]
    NonMonotonePrefix,
    #[error("render changed for identical content, level, and tokenizer ({tokenizer_id})")]
    NonDeterministicRender { tokenizer_id: String },
    #[error("turn {turn} has {blocks} cache blocks; maximum is {maximum}")]
    BlockBudgetExceeded {
        turn: u64,
        blocks: usize,
        maximum: usize,
    },
}

#[derive(Debug, Default)]
pub struct PrefixStabilityGuard {
    last_prefix: Option<String>,
    renders: HashMap<(String, String, String), String>,
    prefix_observations: usize,
}

impl PrefixStabilityGuard {
    pub fn observe_prefix(
        &mut self,
        prefix: &CacheablePrefix,
        model_tier: CacheModelTier,
    ) -> Result<Option<PrefixStabilityAlert>, PrefixStabilityViolation> {
        if !prefix.cache_breakpoint
            && self
                .last_prefix
                .as_ref()
                .is_some_and(|previous| !prefix.bytes.starts_with(previous))
        {
            return Err(PrefixStabilityViolation::NonMonotonePrefix);
        }
        for (&turn, &blocks) in &prefix.blocks_per_turn {
            if blocks > MAX_CACHE_BLOCKS_PER_TURN {
                return Err(PrefixStabilityViolation::BlockBudgetExceeded {
                    turn,
                    blocks,
                    maximum: MAX_CACHE_BLOCKS_PER_TURN,
                });
            }
        }
        self.last_prefix = Some(prefix.bytes.clone());
        self.prefix_observations = self.prefix_observations.saturating_add(1);
        let observed_tokens = count_tokens(&prefix.bytes);
        let required_tokens = model_tier.min_cacheable_estimator_tokens();
        Ok((observed_tokens < required_tokens).then_some(
            PrefixStabilityAlert::BelowCacheableFloor {
                observed_tokens,
                required_tokens,
                model_tier,
            },
        ))
    }

    pub fn observe_render(
        &mut self,
        observation: RenderObservation<'_>,
    ) -> Result<String, PrefixStabilityViolation> {
        let key = (
            sha256_hex(observation.content),
            observation.level.to_owned(),
            observation.tokenizer_id.to_owned(),
        );
        if self
            .renders
            .get(&key)
            .is_some_and(|prior| prior.as_bytes() != observation.rendered.as_bytes())
        {
            return Err(PrefixStabilityViolation::NonDeterministicRender {
                tokenizer_id: observation.tokenizer_id.to_owned(),
            });
        }
        self.renders
            .entry(key)
            .or_insert_with(|| observation.rendered.to_owned());
        Ok(sha256_hex(observation.rendered))
    }

    pub fn observation_counts(&self) -> (usize, usize) {
        (self.prefix_observations, self.renders.len())
    }
}
