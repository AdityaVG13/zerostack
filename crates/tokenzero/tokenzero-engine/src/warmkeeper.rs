//! Deterministic, provider-cache-aware re-warm scheduling.

use serde::{Deserialize, Serialize};

use crate::{CachePricing, CacheProvider};

/// Provider lane ordering. Self-hosted lanes remain visible for compatibility,
/// but never issue paid cache touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarmLaneTier {
    PaidFrontier,
    SelfHosted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WarmLane {
    pub provider: CacheProvider,
    pub model: String,
    pub tier: WarmLaneTier,
    pub ttl_seconds: u64,
    pub prefix_tokens: u64,
    pub expected_reads_per_ttl: f64,
    pub pricing: CachePricing,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_touch_at_seconds: Option<u64>,
}

impl WarmLane {
    pub fn read_savings_dollars(&self) -> f64 {
        self.prefix_tokens as f64
            * (self.pricing.input_per_million - self.pricing.cache_read_per_million).max(0.0)
            / 1_000_000.0
    }
    pub fn write_premium_dollars(&self) -> f64 {
        self.prefix_tokens as f64 * self.pricing.cache_creation_per_million / 1_000_000.0
    }
    pub fn next_touch_at_seconds(&self, now_seconds: u64) -> u64 {
        self.last_touch_at_seconds
            .map_or(now_seconds, |last| last.saturating_add(self.ttl_seconds))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZeroOutputTouch {
    pub provider: CacheProvider,
    pub model: String,
    pub max_output_tokens: u64,
}

/// Output cap for a background cache rewarm ping. One token is enough to land
/// a write on the provider cache; it is never counted as a provider hit.
pub const WARM_PING_OUTPUT_TOKENS: u64 = 1;

/// Keepalive-vs-rewrite choice on session resume after a gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeRewarmKind {
    /// Gap is inside TTL: a 1-token touch refreshes the prefix without a rewrite.
    KeepaliveTouch,
    /// Gap met or passed TTL: reconstruct the byte-identical prefix and rewrite.
    FullRewrite,
}

/// Keepalive vs rewrite crossover for session resume.
///
/// Short gaps refresh TTL by touch. Long gaps take one clean rewrite.
/// `ttl_seconds == 0` is not a keepalive window, so it rewrites.
pub fn resume_rewarm_kind(gap_seconds: u64, ttl_seconds: u64) -> ResumeRewarmKind {
    if ttl_seconds == 0 || gap_seconds >= ttl_seconds {
        ResumeRewarmKind::FullRewrite
    } else {
        ResumeRewarmKind::KeepaliveTouch
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarmDecisionKind {
    Touch,
    NotDue,
    NegativeExpectedValue,
    CompatibilityOnly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WarmDecision {
    pub provider: CacheProvider,
    pub model: String,
    pub tier: WarmLaneTier,
    pub kind: WarmDecisionKind,
    pub next_touch_at_seconds: u64,
    pub expected_savings_dollars: f64,
    pub write_premium_dollars: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub touch: Option<ZeroOutputTouch>,
}

/// Evaluate lanes independently, then render paid-frontier lanes first and the
/// self-hosted compatibility column last. A 24-hour TTL schedules daily touches.
pub fn schedule_rewarms(now_seconds: u64, lanes: &[WarmLane]) -> Vec<WarmDecision> {
    let mut decisions = lanes
        .iter()
        .map(|lane| {
            let next_touch = lane.next_touch_at_seconds(now_seconds);
            let expected_savings = lane.expected_reads_per_ttl * lane.read_savings_dollars();
            let write_premium = lane.write_premium_dollars();
            let kind = if lane.tier == WarmLaneTier::SelfHosted {
                WarmDecisionKind::CompatibilityOnly
            } else if now_seconds < next_touch {
                WarmDecisionKind::NotDue
            } else if expected_savings <= write_premium {
                WarmDecisionKind::NegativeExpectedValue
            } else {
                WarmDecisionKind::Touch
            };
            WarmDecision {
                provider: lane.provider,
                model: lane.model.clone(),
                tier: lane.tier,
                kind,
                next_touch_at_seconds: next_touch,
                expected_savings_dollars: expected_savings,
                write_premium_dollars: write_premium,
                touch: (kind == WarmDecisionKind::Touch).then(|| ZeroOutputTouch {
                    provider: lane.provider,
                    model: lane.model.clone(),
                    max_output_tokens: WARM_PING_OUTPUT_TOKENS,
                }),
            }
        })
        .collect::<Vec<_>>();
    decisions.sort_by(|left, right| {
        tier_rank(left.tier)
            .cmp(&tier_rank(right.tier))
            .then_with(|| provider_rank(left.provider).cmp(&provider_rank(right.provider)))
            .then_with(|| left.model.cmp(&right.model))
    });
    decisions
}

/// Hot placement tier for a prefetched closure (ZS-CACHE-008). PROPOSES a
/// prefetch target; warm touches remain under `schedule_rewarms` authority
/// and serve graduation stays shadow-gated in `tokenzero-recovery`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotPlacement {
    Standard,
    Hot,
}

/// One prefetch proposal: a closure worth warming before its first read
/// because its demand share is high (and, once hazard prediction lands, its
/// invalidation hazard is low).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrefetchTarget {
    pub provider: CacheProvider,
    pub model: String,
    pub prefix_tokens: u64,
    pub placement: HotPlacement,
    /// Demand share of the window in per-mille (1000 = whole window).
    pub demand_milli: u64,
}

/// Select prefetch candidates from warm lanes (ZS-CACHE-008 hook). Lanes
/// whose demand share crosses `threshold_milli` become prefetch targets;
/// the top `hot_quota` of the selected set (density-ordered, stable) get
/// Hot placement. Mismatched lane/demand lengths fail loud.
pub fn select_prefetch_targets(
    lanes: &[WarmLane],
    demand_milli: &[u64],
    threshold_milli: u64,
    hot_quota: usize,
) -> Vec<PrefetchTarget> {
    assert_eq!(
        lanes.len(),
        demand_milli.len(),
        "prefetch demand scores must cover every lane"
    );
    let mut selected = lanes
        .iter()
        .zip(demand_milli.iter())
        .filter(|(_, demand)| **demand >= threshold_milli)
        .map(|(lane, demand)| PrefetchTarget {
            provider: lane.provider,
            model: lane.model.clone(),
            prefix_tokens: lane.prefix_tokens,
            placement: HotPlacement::Standard,
            demand_milli: *demand,
        })
        .collect::<Vec<_>>();
    selected.sort_by(|left, right| {
        right
            .demand_milli
            .cmp(&left.demand_milli)
            .then_with(|| left.model.cmp(&right.model))
    });
    for target in selected.iter_mut().take(hot_quota) {
        target.placement = HotPlacement::Hot;
    }
    selected
}

fn tier_rank(tier: WarmLaneTier) -> u8 {
    match tier {
        WarmLaneTier::PaidFrontier => 0,
        WarmLaneTier::SelfHosted => 1,
    }
}
fn provider_rank(provider: CacheProvider) -> u8 {
    match provider {
        CacheProvider::Anthropic => 0,
        CacheProvider::OpenAi => 1,
        CacheProvider::Gemini => 2,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WarmReplayLane {
    #[serde(flatten)]
    pub lane: WarmLane,
    pub observed_reads: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WarmSimulationReport {
    pub no_warm_billed_dollars: f64,
    pub always_warm_billed_dollars: f64,
    pub ev_gated_billed_dollars: f64,
    pub decisions: Vec<WarmDecision>,
}

/// Simulate one TTL window without network access, using CacheMeter pricing.
pub fn simulate_warmkeeper(lanes: &[WarmReplayLane]) -> WarmSimulationReport {
    let lane_values = lanes
        .iter()
        .map(|item| item.lane.clone())
        .collect::<Vec<_>>();
    let decisions = schedule_rewarms(0, &lane_values);
    let mut no_warm = 0.0;
    let mut always_warm = 0.0;
    let mut ev_gated = 0.0;
    for item in lanes {
        let reads = item.observed_reads as f64;
        let input_cost =
            item.lane.prefix_tokens as f64 * item.lane.pricing.input_per_million / 1_000_000.0;
        let cache_read_cost =
            item.lane.prefix_tokens as f64 * item.lane.pricing.cache_read_per_million / 1_000_000.0;
        let write_cost = item.lane.write_premium_dollars();
        no_warm += reads * input_cost;
        always_warm += if item.lane.tier == WarmLaneTier::PaidFrontier {
            write_cost + reads * cache_read_cost
        } else {
            reads * input_cost
        };
        let decision = decisions
            .iter()
            .find(|decision| {
                decision.provider == item.lane.provider && decision.model == item.lane.model
            })
            .expect("every replay lane has one decision");
        ev_gated += if decision.kind == WarmDecisionKind::Touch {
            write_cost + reads * cache_read_cost
        } else {
            reads * input_cost
        };
    }
    WarmSimulationReport {
        no_warm_billed_dollars: no_warm,
        always_warm_billed_dollars: always_warm,
        ev_gated_billed_dollars: ev_gated,
        decisions,
    }
}
