//! Provider-neutral usage observations and exactly-once savings attribution.
//!
//! Measured, estimated, and unmeasured coordinates remain distinct. Totals
//! include measured events only, so missing provider data cannot become zero.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageCoordinate {
    VisibleInput,
    ToolResult,
    BilledInput,
    CacheRead,
    CacheWrite,
    Reasoning,
    VisibleOutput,
    ProviderCredit,
    BackendCpu,
    BackendIo,
    Latency,
    Storage,
    Verification,
    UncachedInput,
    BilledTokens,
    BilledCost,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationKind {
    Measured,
    Estimated,
    Unmeasured,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageArm {
    Baseline,
    Zero,
    Competitor,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UsageEvent {
    pub event_id: String,
    pub task_root: String,
    pub arm: UsageArm,
    pub coordinate: UsageCoordinate,
    pub amount: u64,
    pub unit: String,
    pub provenance: String,
    pub observation: ObservationKind,
    pub window_id: Option<String>,
    pub occurrence_id: Option<String>,
}

impl UsageEvent {
    pub fn validate(&self) -> Result<(), String> {
        if self.event_id.is_empty()
            || self.task_root.is_empty()
            || self.unit.is_empty()
            || self.provenance.is_empty()
        {
            return Err("usage event identity, task, unit, and provenance are required".into());
        }
        if self.observation == ObservationKind::Unmeasured && self.amount != 0 {
            return Err("unmeasured usage cannot carry a numeric amount".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PlanWindow {
    pub id: String,
    pub provider: String,
    pub plan_tier: String,
    pub model_contract_root: String,
    pub harness_contract_root: String,
    pub started_unix_ms: u64,
    pub ended_unix_ms: Option<u64>,
    pub reset_observed_unix_ms: Option<u64>,
    pub throttle_observed_unix_ms: Option<u64>,
}

impl PlanWindow {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() || self.provider.is_empty() || self.plan_tier.is_empty() {
            return Err("plan window identity, provider, and tier are required".into());
        }
        if self
            .ended_unix_ms
            .is_some_and(|end| end < self.started_unix_ms)
        {
            return Err("plan window end precedes start".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavingsDisposition {
    Retained,
    Eliminated { mechanism: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SavingsOccurrence {
    pub occurrence_id: String,
    pub baseline_event_id: String,
    pub optimized_event_id: Option<String>,
    pub disposition: SavingsDisposition,
}

pub fn validate_disjoint_attribution(
    baseline_occurrences: &[String],
    attributions: &[SavingsOccurrence],
) -> Result<(), String> {
    let expected: BTreeSet<&str> = baseline_occurrences.iter().map(String::as_str).collect();
    if expected.len() != baseline_occurrences.len() {
        return Err("baseline occurrence ids must be unique".into());
    }
    let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
    for attribution in attributions {
        if attribution.occurrence_id.is_empty() || attribution.baseline_event_id.is_empty() {
            return Err("savings attribution ids must not be empty".into());
        }
        if !expected.contains(attribution.occurrence_id.as_str()) {
            return Err(format!(
                "unexpected savings occurrence {}",
                attribution.occurrence_id
            ));
        }
        *counts.entry(&attribution.occurrence_id).or_default() += 1;
    }
    for occurrence in expected {
        if counts.get(occurrence).copied() != Some(1) {
            return Err(format!(
                "occurrence {occurrence} is not classified exactly once"
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelCallReconciliation {
    pub declared: u64,
    pub observed: u64,
}

impl ModelCallReconciliation {
    pub const fn hidden_calls(self) -> u64 {
        self.observed.saturating_sub(self.declared)
    }
    pub const fn reconciled(self) -> bool {
        self.declared == self.observed
    }
}

pub fn coordinate_totals(events: &[UsageEvent]) -> Result<BTreeMap<UsageCoordinate, u128>, String> {
    let mut totals = BTreeMap::new();
    for event in events {
        event.validate()?;
        if event.observation == ObservationKind::Measured {
            let current = totals.entry(event.coordinate).or_insert(0_u128);
            *current = current
                .checked_add(u128::from(event.amount))
                .ok_or("usage coordinate overflow")?;
        }
    }
    Ok(totals)
}
/// Convert a provider-neutral observation into deterministic [`UsageEvent`]s.
///
/// Every coordinate in the observation produces one event, even when
/// unmeasured (amount 0, observation Unmeasured). This preserves
/// missing-data honesty: absent provider fields remain `Unmeasured` and
/// never become measured zero.
pub fn provider_usage_events(
    task_root: &str,
    arm: UsageArm,
    observation: &zero_abi::zero_kernel::ProviderUsageObservation,
) -> Result<Vec<UsageEvent>, String> {
    if task_root.is_empty() {
        return Err("task_root must not be empty".into());
    }
    observation.validate()?;

    let entries: [(
        UsageCoordinate,
        &zero_abi::zero_kernel::UsageAmount,
        &str,
        &str,
    ); 8] = [
        (
            UsageCoordinate::UncachedInput,
            &observation.uncached_input_tokens,
            "uncached_input",
            "tokens",
        ),
        (
            UsageCoordinate::CacheRead,
            &observation.cached_read_input_tokens,
            "cache_read",
            "tokens",
        ),
        (
            UsageCoordinate::CacheWrite,
            &observation.cached_write_input_tokens,
            "cache_write",
            "tokens",
        ),
        (
            UsageCoordinate::Reasoning,
            &observation.reasoning_tokens,
            "reasoning",
            "tokens",
        ),
        (
            UsageCoordinate::VisibleOutput,
            &observation.output_tokens,
            "visible_output",
            "tokens",
        ),
        (
            UsageCoordinate::BilledTokens,
            &observation.billed_tokens,
            "billed_tokens",
            "tokens",
        ),
        (
            UsageCoordinate::BilledCost,
            &observation.billed_microcredits,
            "billed_cost",
            "microcredits",
        ),
        (
            UsageCoordinate::ProviderCredit,
            &observation.credit_microcredits,
            "provider_credit",
            "microcredits",
        ),
    ];

    let mut out = Vec::with_capacity(8);
    for (coordinate, amount, slug, unit) in entries {
        let observation_kind = match amount.measurement {
            zero_abi::zero_kernel::UsageMeasurement::Measured => ObservationKind::Measured,
            zero_abi::zero_kernel::UsageMeasurement::Estimated => ObservationKind::Estimated,
            zero_abi::zero_kernel::UsageMeasurement::Unmeasured => ObservationKind::Unmeasured,
        };
        let numeric = amount.amount.unwrap_or(0);
        let event = UsageEvent {
            event_id: format!("{}:{slug}", observation.request_id),
            task_root: task_root.to_string(),
            arm,
            coordinate,
            amount: numeric,
            unit: unit.to_string(),
            provenance: amount.provenance.clone(),
            observation: observation_kind,
            window_id: None,
            occurrence_id: Some(observation.request_id.clone()),
        };
        event.validate()?;
        out.push(event);
    }
    Ok(out)
}
