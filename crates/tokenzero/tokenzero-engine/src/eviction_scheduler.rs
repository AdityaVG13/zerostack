//! Cache-aware prompt eviction scheduling and replay accounting.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::CacheProvider;

pub const OPENAI_MAX_RETENTION_SECONDS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrefixTier {
    System,
    Tools,
    Messages,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CacheBreakpoint {
    pub ttl_seconds: u64,
    pub minimum_requests: u64,
    pub write_multiplier: f64,
}

pub fn provider_breakpoints(provider: CacheProvider) -> &'static [CacheBreakpoint] {
    const ANTHROPIC: [CacheBreakpoint; 2] = [
        CacheBreakpoint {
            ttl_seconds: 300,
            minimum_requests: 2,
            write_multiplier: 1.25,
        },
        CacheBreakpoint {
            ttl_seconds: 3_600,
            minimum_requests: 3,
            write_multiplier: 2.0,
        },
    ];
    const OPENAI: [CacheBreakpoint; 1] = [CacheBreakpoint {
        ttl_seconds: OPENAI_MAX_RETENTION_SECONDS,
        minimum_requests: 2,
        write_multiplier: 1.0,
    }];
    const GEMINI: [CacheBreakpoint; 1] = [CacheBreakpoint {
        ttl_seconds: 3_600,
        minimum_requests: 2,
        write_multiplier: 1.0,
    }];
    match provider {
        CacheProvider::Anthropic => &ANTHROPIC,
        CacheProvider::OpenAi => &OPENAI,
        CacheProvider::Gemini => &GEMINI,
    }
}

/// Pick the shortest provider cache window whose empirical inter-request gaps
/// retain enough requests to amortize its write. The initial request counts
/// toward the provider's documented break-even.
pub fn ttl_from_gaps(provider: CacheProvider, gaps_seconds: &[u64]) -> Option<CacheBreakpoint> {
    provider_breakpoints(provider)
        .iter()
        .copied()
        .find(|breakpoint| {
            let retained_requests = 1_u64.saturating_add(
                gaps_seconds
                    .iter()
                    .scan(0_u64, |elapsed, gap| {
                        *elapsed = elapsed.saturating_add(*gap);
                        Some(*elapsed)
                    })
                    .take_while(|elapsed| *elapsed <= breakpoint.ttl_seconds)
                    .count() as u64,
            );
            retained_requests >= breakpoint.minimum_requests
        })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvictionCandidate {
    pub id: String,
    pub tier: PrefixTier,
    pub prefix_tokens: u64,
    pub prefix_rewrite_cost_tokens: u64,
    pub expected_remaining_requests: u64,
    pub read_savings_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvictionDecisionKind {
    Evict,
    PreserveProtectedTier,
    PreserveNegativeExpectedValue,
    PreserveNoViableTtl,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvictionDecision {
    pub candidate: EvictionCandidate,
    pub kind: EvictionDecisionKind,
    pub ttl_seconds: Option<u64>,
    pub cache_breakpoint_at_seconds: Option<u64>,
    pub expected_read_savings_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvictionBatch {
    pub cache_breakpoint_at_seconds: u64,
    pub ttl_seconds: u64,
    pub candidate_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvictionSchedule {
    pub decisions: Vec<EvictionDecision>,
    pub batches: Vec<EvictionBatch>,
}

/// System and tool tiers are immutable cache roots. Only message-tier spans can
/// be replaced by TZ-EVICT refs, and only under strict positive expected value.
pub fn schedule_evictions(
    provider: CacheProvider,
    now_seconds: u64,
    measured_gaps_seconds: &[u64],
    candidates: &[EvictionCandidate],
) -> EvictionSchedule {
    let breakpoint = ttl_from_gaps(provider, measured_gaps_seconds);
    let decisions = candidates
        .iter()
        .cloned()
        .map(|candidate| {
            let expected_read_savings_tokens = candidate
                .expected_remaining_requests
                .saturating_mul(candidate.read_savings_tokens);
            let kind = if candidate.tier != PrefixTier::Messages {
                EvictionDecisionKind::PreserveProtectedTier
            } else if breakpoint.is_none() {
                EvictionDecisionKind::PreserveNoViableTtl
            } else if expected_read_savings_tokens <= candidate.prefix_rewrite_cost_tokens {
                EvictionDecisionKind::PreserveNegativeExpectedValue
            } else {
                EvictionDecisionKind::Evict
            };
            let ttl_seconds = (kind == EvictionDecisionKind::Evict)
                .then(|| breakpoint.expect("viable TTL for eviction").ttl_seconds);
            EvictionDecision {
                candidate,
                kind,
                ttl_seconds,
                cache_breakpoint_at_seconds: ttl_seconds.map(|ttl| now_seconds.saturating_add(ttl)),
                expected_read_savings_tokens,
            }
        })
        .collect::<Vec<_>>();

    let mut grouped = BTreeMap::<(u64, u64), Vec<String>>::new();
    for decision in &decisions {
        if let (Some(at), Some(ttl)) = (decision.cache_breakpoint_at_seconds, decision.ttl_seconds)
        {
            grouped
                .entry((at, ttl))
                .or_default()
                .push(decision.candidate.id.clone());
        }
    }
    let batches = grouped
        .into_iter()
        .map(
            |((cache_breakpoint_at_seconds, ttl_seconds), mut candidate_ids)| {
                candidate_ids.sort();
                EvictionBatch {
                    cache_breakpoint_at_seconds,
                    ttl_seconds,
                    candidate_ids,
                }
            },
        )
        .collect();
    EvictionSchedule { decisions, batches }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvictionReplayItem {
    pub candidate: EvictionCandidate,
    pub observed_remaining_requests: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvictionReplayReport {
    pub naive_billed_tokens: u64,
    pub scheduled_billed_tokens: u64,
    pub saved_billed_tokens: u64,
    pub schedule: EvictionSchedule,
}

pub fn simulate_eviction_replay(
    provider: CacheProvider,
    now_seconds: u64,
    measured_gaps_seconds: &[u64],
    replay: &[EvictionReplayItem],
) -> EvictionReplayReport {
    let candidates = replay
        .iter()
        .map(|item| item.candidate.clone())
        .collect::<Vec<_>>();
    let schedule = schedule_evictions(provider, now_seconds, measured_gaps_seconds, &candidates);
    let mut naive_billed_tokens = 0_u64;
    let mut scheduled_billed_tokens = 0_u64;
    for (item, decision) in replay.iter().zip(&schedule.decisions) {
        let uncached = item
            .observed_remaining_requests
            .saturating_mul(item.candidate.prefix_tokens);
        naive_billed_tokens = naive_billed_tokens.saturating_add(uncached);
        scheduled_billed_tokens = scheduled_billed_tokens.saturating_add(
            if decision.kind == EvictionDecisionKind::Evict {
                item.candidate.prefix_rewrite_cost_tokens.saturating_add(
                    item.observed_remaining_requests.saturating_mul(
                        item.candidate
                            .prefix_tokens
                            .saturating_sub(item.candidate.read_savings_tokens),
                    ),
                )
            } else {
                uncached
            },
        );
    }
    EvictionReplayReport {
        naive_billed_tokens,
        scheduled_billed_tokens,
        saved_billed_tokens: naive_billed_tokens.saturating_sub(scheduled_billed_tokens),
        schedule,
    }
}

/// Session-local bridge to tokenzero.ledger.v1 eviction_amortization. Event IDs
/// make retries idempotent, so cache savings cannot be double-booked.
#[derive(Debug, Default)]
pub struct EvictionSavingsLedger {
    recorded_event_ids: BTreeSet<String>,
    saved_billed_tokens: u64,
}

impl EvictionSavingsLedger {
    pub fn record_once(&mut self, event_id: impl Into<String>, saved_billed_tokens: u64) -> bool {
        if !self.recorded_event_ids.insert(event_id.into()) {
            return false;
        }
        self.saved_billed_tokens = self.saved_billed_tokens.saturating_add(saved_billed_tokens);
        true
    }

    pub fn saved_billed_tokens(&self) -> u64 {
        self.saved_billed_tokens
    }

    pub fn eviction_amortization(&self) -> Value {
        json!({
            "schema": "tokenzero.eviction-amortization.v1",
            "saved_billed_tokens": self.saved_billed_tokens,
            "unique_events": self.recorded_event_ids.len(),
        })
    }
}
