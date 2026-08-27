//! Frontier resident-set planner (ZS-CACHE-007).
//!
//! PROPOSES ONLY. Verification authority is the hub
//! `ResidencyThresholdChecker` (ZeroStack `zero-gate/src/residency.rs`,
//! W4): the optimizer proposes, the checker authorizes. This module never
//! authorizes residency and never re-implements the checker; it emits a
//! proposal in the hub `causal_residency_plan` vocabulary (planned objects
//! with demand weights, capacity, optimizer name) so the checker can verify
//! it unchanged.
//!
//! The planner selects resident objects under capacity / latency /
//! invalidation budgets with a deterministic density-ordered greedy (demand
//! weight per byte, latency and invalidation as hard per-object constraints
//! while building the set). Budgets the demand cannot fit inside are
//! reported as `budget_violations` so downstream policy can see the shortfall
//! without the planner pretending to authorize.

use serde::{Deserialize, Serialize};

pub const FRONTIER_PLAN_SCHEMA: &str = "tokenzero.frontier-plan/v1";
/// Optimizer name carried in proposals so the hub checker can attribute
/// the plan to this proposer.
pub const FRONTIER_OPTIMIZER_NAME: &str = "tokenzero-frontier-density-v1";

/// One demanded object in a proposal. Field names mirror the hub
/// `causal_residency_plan` object vocabulary (`object_root`, `size_bytes`,
/// `demand_weight`, `valid`, `resident`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrontierPlanObject {
    pub object_root: String,
    pub size_bytes: u64,
    /// Demanded weight in ppm of the window demand.
    pub demand_weight: u64,
    pub valid: bool,
    /// Planner proposal: whether the object should stay resident. The hub
    /// checker holds the final verdict.
    pub resident: bool,
    /// Estimated read latency when resident, milliseconds.
    pub estimated_latency_ms: u64,
    /// Expected invalidations per demand window.
    pub expected_invalidations: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrontierBudgets {
    pub capacity_bytes: u64,
    pub latency_budget_ms: u64,
    pub invalidation_budget: u64,
}

/// Proposed resident set. PROPOSAL ONLY: `budget_violations` names budgets
/// the total demand cannot fit inside; the hub
/// `ResidencyThresholdChecker` is the verifying authority.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FrontierPlan {
    pub schema: &'static str,
    pub tier: String,
    pub capacity_bytes: u64,
    /// Retained-valid-mass fraction the planner targets; the checker
    /// enforces it (this module never does).
    pub threshold: f64,
    pub objects: Vec<FrontierPlanObject>,
    pub optimizer: &'static str,
    pub resident_bytes: u64,
    pub resident_valid_weight: u64,
    pub total_demand_weight: u64,
    pub budget_violations: Vec<String>,
}

/// Deterministic density-ordered greedy resident-set proposal.
///
/// Candidates are the valid, demanded objects sorted by demand weight per
/// byte (descending), with a stable tie-break on `object_root` so proposals
/// are order-independent. An object joins the resident set only when adding
/// it keeps every budget inside its bound. Budgets the total demand cannot
/// satisfy are listed in `budget_violations` (empty means the demand fits
/// and the proposal claims nothing outside the budgets).
pub fn plan_frontier_resident_set(
    tier: &str,
    threshold: f64,
    objects: &[FrontierPlanObject],
    budgets: &FrontierBudgets,
) -> FrontierPlan {
    let threshold = threshold.clamp(0.0, 1.0);
    let mut plan_objects = objects.to_vec();
    let total_size: u64 = objects.iter().map(|object| object.size_bytes).sum();
    let total_latency: u64 = objects
        .iter()
        .map(|object| object.estimated_latency_ms)
        .sum();
    let total_invalidations: u64 = objects
        .iter()
        .map(|object| object.expected_invalidations)
        .sum();

    let mut violations = Vec::new();
    if total_size > budgets.capacity_bytes {
        violations.push("capacity".to_string());
    }
    if total_latency > budgets.latency_budget_ms {
        violations.push("latency".to_string());
    }
    if total_invalidations > budgets.invalidation_budget {
        violations.push("invalidations".to_string());
    }

    // Density-ordered candidates, stable tie-break on the object root.
    let mut candidates = plan_objects
        .iter()
        .enumerate()
        .filter(|(_, object)| object.valid && object.demand_weight > 0)
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        density(right.1)
            .cmp(&density(left.1))
            .then_with(|| left.1.object_root.cmp(&right.1.object_root))
    });

    let mut resident_indices = Vec::new();
    let mut resident_bytes = 0_u64;
    let mut latency_used = 0_u64;
    let mut invalidations_used = 0_u64;
    for (index, object) in candidates {
        let candidate_bytes = resident_bytes.saturating_add(object.size_bytes);
        let candidate_latency = latency_used.saturating_add(object.estimated_latency_ms);
        let candidate_invalidations =
            invalidations_used.saturating_add(object.expected_invalidations);
        if candidate_bytes <= budgets.capacity_bytes
            && candidate_latency <= budgets.latency_budget_ms
            && candidate_invalidations <= budgets.invalidation_budget
        {
            resident_indices.push(index);
            resident_bytes = candidate_bytes;
            latency_used = candidate_latency;
            invalidations_used = candidate_invalidations;
        }
    }
    for index in resident_indices {
        plan_objects[index].resident = true;
    }

    let resident_valid_weight = plan_objects
        .iter()
        .filter(|object| object.resident && object.valid)
        .map(|object| object.demand_weight)
        .sum();
    let total_demand_weight = plan_objects.iter().map(|object| object.demand_weight).sum();

    FrontierPlan {
        schema: FRONTIER_PLAN_SCHEMA,
        tier: tier.to_string(),
        capacity_bytes: budgets.capacity_bytes,
        threshold,
        objects: plan_objects,
        optimizer: FRONTIER_OPTIMIZER_NAME,
        resident_bytes,
        resident_valid_weight,
        total_demand_weight,
        budget_violations: violations,
    }
}

/// Demand weight per byte; zero-byte objects sort highest so zero-cost
/// closures are always eligible first.
const fn density(object: &FrontierPlanObject) -> u64 {
    if object.size_bytes == 0 {
        u64::MAX
    } else {
        object.demand_weight / object.size_bytes
    }
}

