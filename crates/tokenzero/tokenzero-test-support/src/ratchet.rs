//! Conformal-lower ratchet: Allow | Block | Quarantine | Waiver.
//!
//! The persisted high-water mark is the truncated conformal LOWER bound.
//! A rising point estimate cannot advance or hold the bound if the lower
//! dropped. Only [`RatchetVerdict::Allow`] writes a new [`RatchetState`].

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::conformal::ParityScorecard;
use crate::parity_taxonomy::truncate_score;

pub const RATCHET_STATE_SCHEMA: &str = "gauntlet.ratchet_state.v1";
/// Skill default: exactly one category (or a global lower) may dip this far
/// before the verdict upgrades from Quarantine to Block.
pub const CATEGORY_QUARANTINE_THRESHOLD: f64 = 0.005;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum RatchetVerdict {
    Allow,
    Block,
    Quarantine,
    Waiver,
}

impl fmt::Display for RatchetVerdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Allow => "Allow",
            Self::Block => "Block",
            Self::Quarantine => "Quarantine",
            Self::Waiver => "Waiver",
        })
    }
}

/// Structured exception for an honest lower-bound downgrade. Does not
/// persist a new high-water mark and cannot treat the point as the bound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RatchetWaiver {
    pub id: String,
    pub applies_to_bound_kind: String,
    pub old_bound: f64,
    pub new_bound: f64,
    pub expired: bool,
}

impl RatchetWaiver {
    pub fn covers(&self, previous_lower: f64, current_lower: f64) -> bool {
        if self.expired {
            return false;
        }
        if self.applies_to_bound_kind != "global_lower" {
            return false;
        }
        truncate_score(self.old_bound) == truncate_score(previous_lower)
            && truncate_score(self.new_bound) == truncate_score(current_lower)
    }
}

/// Persisted high-water mark. Bounds are `truncate_score`'d at the boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RatchetState {
    pub schema_version: String,
    pub global_lower: f64,
    pub previous_bound: Option<f64>,
    pub per_category_bounds: BTreeMap<String, f64>,
}

impl Default for RatchetState {
    fn default() -> Self {
        Self {
            schema_version: RATCHET_STATE_SCHEMA.to_string(),
            global_lower: 0.0,
            previous_bound: None,
            per_category_bounds: BTreeMap::new(),
        }
    }
}

impl RatchetState {
    pub fn from_scorecard(scorecard: &ParityScorecard) -> Self {
        let mut per_category_bounds = BTreeMap::new();
        for (name, row) in &scorecard.categories {
            per_category_bounds.insert(name.clone(), truncate_score(row.lower));
        }
        Self {
            schema_version: RATCHET_STATE_SCHEMA.to_string(),
            global_lower: truncate_score(scorecard.global_lower),
            previous_bound: None,
            per_category_bounds,
        }
    }

    /// Global + per-category monotonicity. Next state is `Some` only on Allow.
    pub fn apply(&self, current: &ParityScorecard) -> (RatchetVerdict, Option<Self>) {
        self.apply_with_waiver(current, None)
    }

    pub fn apply_with_waiver(
        &self,
        current: &ParityScorecard,
        waiver: Option<&RatchetWaiver>,
    ) -> (RatchetVerdict, Option<Self>) {
        let global = apply_ratchet_with_waiver(self.global_lower, current, waiver);
        let verdict = match global {
            RatchetVerdict::Allow => self.category_gate(current),
            other => other,
        };
        let next = match verdict {
            RatchetVerdict::Allow => Some(self.advanced(current)),
            _ => None,
        };
        (verdict, next)
    }

    fn category_gate(&self, current: &ParityScorecard) -> RatchetVerdict {
        if self.per_category_bounds.is_empty() {
            return RatchetVerdict::Allow;
        }
        let band = truncate_score(CATEGORY_QUARANTINE_THRESHOLD);
        let mut small_dips = 0usize;
        for (cat, prev_lo) in &self.per_category_bounds {
            let Some(row) = current.categories.get(cat) else {
                return RatchetVerdict::Block;
            };
            let prev_lo = truncate_score(*prev_lo);
            let lo = truncate_score(row.lower);
            if lo >= prev_lo {
                continue;
            }
            if prev_lo - lo <= band {
                small_dips += 1;
            } else {
                return RatchetVerdict::Block;
            }
        }
        match small_dips {
            0 => RatchetVerdict::Allow,
            1 => RatchetVerdict::Quarantine,
            _ => RatchetVerdict::Block,
        }
    }

    fn advanced(&self, current: &ParityScorecard) -> Self {
        let mut per_category_bounds = BTreeMap::new();
        for (name, row) in &current.categories {
            let prior = self
                .per_category_bounds
                .get(name)
                .copied()
                .map(truncate_score)
                .unwrap_or(0.0);
            per_category_bounds.insert(name.clone(), truncate_score(row.lower).max(prior));
        }
        Self {
            schema_version: RATCHET_STATE_SCHEMA.to_string(),
            global_lower: truncate_score(current.global_lower)
                .max(truncate_score(self.global_lower)),
            previous_bound: Some(truncate_score(self.global_lower)),
            per_category_bounds,
        }
    }
}

/// Compare `current_scorecard.global_lower` to the persisted high-water mark.
/// The point estimate is never the bound: point > previous with lower < previous
/// is Block, even when the drop is inside the quarantine band.
#[must_use]
pub fn apply_ratchet(previous_lower: f64, current_scorecard: &ParityScorecard) -> RatchetVerdict {
    apply_ratchet_with_waiver(previous_lower, current_scorecard, None)
}

#[must_use]
pub fn apply_ratchet_with_waiver(
    previous_lower: f64,
    current_scorecard: &ParityScorecard,
    waiver: Option<&RatchetWaiver>,
) -> RatchetVerdict {
    let verdict = ratchet_unwaived(previous_lower, current_scorecard);
    match (verdict, waiver) {
        (RatchetVerdict::Allow, _) => RatchetVerdict::Allow,
        (RatchetVerdict::Block | RatchetVerdict::Quarantine, Some(w))
            if w.covers(previous_lower, current_scorecard.global_lower) =>
        {
            RatchetVerdict::Waiver
        }
        (other, _) => other,
    }
}

fn ratchet_unwaived(previous_lower: f64, current: &ParityScorecard) -> RatchetVerdict {
    if current.observation_count <= 0.0
        || !previous_lower.is_finite()
        || !current.global_lower.is_finite()
        || !current.global_point.is_finite()
        || current.global_lower > current.global_point + 1e-12
    {
        return RatchetVerdict::Block;
    }

    let prev = truncate_score(previous_lower);
    let lower = truncate_score(current.global_lower);
    let point = truncate_score(current.global_point);

    if lower >= prev {
        return RatchetVerdict::Allow;
    }
    // lower < prev. A point still above the old high-water is the lie.
    if point > prev {
        return RatchetVerdict::Block;
    }
    let drop = prev - lower;
    if drop <= truncate_score(CATEGORY_QUARANTINE_THRESHOLD) {
        RatchetVerdict::Quarantine
    } else {
        RatchetVerdict::Block
    }
}

#[cfg(test)]
#[path = "../../../../tests/tokenzero/unit/tokenzero-test-support/ratchet_tests.rs"]
mod tests;
