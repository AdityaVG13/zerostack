//! ZS-METRIC-005: cost allocation over reuse campaigns and break-even
//! horizons.
//!
//! Indexing/object-formation cost must never vanish from a warm-run story:
//! every per-use figure produced here includes the cold build. A reuse
//! campaign is described by
//!
//! - `cold_build_cost` C: the indexing/object-formation cost paid once,
//! - `per_use_cost` W: the operating cost of one warm use,
//! - `alternative_per_use_cost` A: the cost of one use without the campaign,
//! - `reuse_count` n: actual uses in the campaign.
//!
//! Per-use savings are `s = A - W`. Exact arithmetic only: allocated shares
//! are reduced rational pairs, never floats.
//!
//! Refusals ([`CampaignError::Impossible`]): a campaign with no uses
//! (denominator nonpositive), an alternative that is not strictly more
//! expensive than the campaign (no savings), an empty per-use savings sample
//! for the Q99 horizon, and a Q99 savings of zero. A claimed campaign total
//! below the cold-build cost is refused as a warm-run claim that omits the
//! cold build -- "no warm-run claim omits cold build".

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize, de};

/// One actual reuse campaign with its measured cold build and per-use costs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReuseCampaign {
    cold_build_cost: u64,
    per_use_cost: u64,
    alternative_per_use_cost: u64,
    reuse_count: u64,
}

impl ReuseCampaign {
    /// Builds a campaign, refusing a zero reuse count (denominator
    /// nonpositive) and an alternative that is not strictly more expensive
    /// than the campaign (nonpositive savings).
    pub fn new(
        cold_build_cost: u64,
        per_use_cost: u64,
        alternative_per_use_cost: u64,
        reuse_count: u64,
    ) -> Result<Self, CampaignError> {
        if reuse_count == 0 {
            return Err(CampaignError::Impossible(
                "reuse campaign has no uses: denominator nonpositive",
            ));
        }
        if alternative_per_use_cost <= per_use_cost {
            return Err(CampaignError::Impossible(
                "per-use savings nonpositive: the alternative is not strictly more expensive than the campaign",
            ));
        }
        Ok(Self {
            cold_build_cost,
            per_use_cost,
            alternative_per_use_cost,
            reuse_count,
        })
    }

    /// The one-time indexing/object-formation cost.
    pub fn cold_build_cost(&self) -> u64 {
        self.cold_build_cost
    }

    /// Operating cost of one warm use, excluding the cold build.
    pub fn per_use_cost(&self) -> u64 {
        self.per_use_cost
    }

    /// Cost of one use without the campaign.
    pub fn alternative_per_use_cost(&self) -> u64 {
        self.alternative_per_use_cost
    }

    /// Actual reuse count in the campaign.
    pub fn reuse_count(&self) -> u64 {
        self.reuse_count
    }

    /// Exact per-use savings `s = A - W`, strictly positive by construction.
    pub fn per_use_savings(&self) -> u64 {
        self.alternative_per_use_cost - self.per_use_cost
    }

    /// Total campaign cost `C + n * W`, always including the cold build.
    pub fn total_campaign_cost(&self) -> u128 {
        u128::from(self.cold_build_cost) + u128::from(self.per_use_cost) * u128::from(self.reuse_count)
    }

    /// Total cost of the alternative over the same uses: `n * A`.
    pub fn alternative_total_cost(&self) -> u128 {
        u128::from(self.alternative_per_use_cost) * u128::from(self.reuse_count)
    }

    /// Campaign surplus over the alternative at the actual reuse count:
    /// `n * A - (C + n * W)`. Negative means the campaign has not broken even.
    pub fn campaign_surplus(&self) -> i128 {
        i128::try_from(self.alternative_total_cost())
            .expect("fits i128")
            - i128::try_from(self.total_campaign_cost()).expect("fits i128")
    }

    /// Amortized cold-build share per use as the reduced fraction `C / n`.
    ///
    /// This is the allocation of the indexing/object-formation cost over the
    /// actual reuse campaign.
    pub fn amortized_cold_per_use(&self) -> (u64, u64) {
        reduce(self.cold_build_cost, self.reuse_count)
    }

    /// All-in per-use cost as the reduced fraction `(C + n * W) / n`.
    ///
    /// Every per-use figure the campaign reports includes the cold build.
    pub fn allocated_per_use(&self) -> (u128, u128) {
        let numerator = u128::from(self.cold_build_cost) + u128::from(self.per_use_cost) * u128::from(self.reuse_count);
        let denominator = u128::from(self.reuse_count);
        let divisor = gcd(numerator, denominator);
        (numerator / divisor, denominator / divisor)
    }

    /// Strict break-even horizon: the smallest number of uses with
    /// `C + n * W <= n * A`, i.e. `ceil(C / s)`, exact.
    ///
    /// Zero uses when the cold build is zero (break-even at once).
    pub fn strict_break_even_uses(&self) -> u64 {
        let cold = u128::from(self.cold_build_cost);
        if cold == 0 {
            return 0;
        }
        let savings = u128::from(self.per_use_savings());
        let horizon = cold.div_ceil(savings);
        u64::try_from(horizon).expect("horizon <= cold build, which fits u64")
    }

    /// Q99 break-even horizon over the *actual* campaign's per-use savings
    /// sample.
    ///
    /// The sample's 99th percentile is the exact rank
    /// `ceil(0.99 * len)` in ascending order (1-based); the horizon is
    /// `ceil(C / q99)`. Refusals: an empty sample (no observed savings) and a
    /// zero q99 savings (no finite horizon).
    pub fn q99_break_even_uses(&self, observed_per_use_savings: &[u64]) -> Result<u64, CampaignError> {
        if observed_per_use_savings.is_empty() {
            return Err(CampaignError::Impossible(
                "no per-use savings observations: Q99 break-even is underdetermined",
            ));
        }
        let mut sorted = observed_per_use_savings.to_vec();
        sorted.sort_unstable();
        let len = sorted.len();
        let rank = (99 * len).div_ceil(100); // ceil(0.99 * len), at least 1
        let q99 = sorted[rank - 1];
        if q99 == 0 {
            return Err(CampaignError::Impossible(
                "zero Q99 savings: no finite break-even horizon",
            ));
        }
        let cold = u128::from(self.cold_build_cost);
        if cold == 0 {
            return Ok(0);
        }
        let horizon = cold.div_ceil(u128::from(q99));
        Ok(u64::try_from(horizon).expect("horizon <= cold build, which fits u64"))
    }

    /// Verifies that a claimed campaign total includes the cold build.
    ///
    /// Any claim below the cold-build cost omits it and is refused: a
    /// warm-run figure can never be presented as the campaign cost.
    pub fn check_claim_includes_cold_build(&self, claimed_total: u128) -> Result<(), CampaignError> {
        if claimed_total < u128::from(self.cold_build_cost) {
            return Err(CampaignError::WarmRunClaimOmitsColdBuild {
                claimed: claimed_total,
                cold_build: self.cold_build_cost,
            });
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct ReuseCampaignWire {
    cold_build_cost: u64,
    per_use_cost: u64,
    alternative_per_use_cost: u64,
    reuse_count: u64,
}

impl<'de> Deserialize<'de> for ReuseCampaign {
    /// Wire decoding goes through the validated constructor.
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ReuseCampaignWire::deserialize(deserializer)?;
        Self::new(
            wire.cold_build_cost,
            wire.per_use_cost,
            wire.alternative_per_use_cost,
            wire.reuse_count,
        )
        .map_err(de::Error::custom)
    }
}

/// Typed campaign failures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CampaignError {
    /// The requested computation has no honest denominator.
    Impossible(&'static str),
    /// A claimed campaign total omits the cold build.
    WarmRunClaimOmitsColdBuild {
        /// The claimed total.
        claimed: u128,
        /// The cold-build cost a valid claim must cover.
        cold_build: u64,
    },
}

impl fmt::Display for CampaignError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Impossible(reason) => write!(f, "impossible: {reason}"),
            Self::WarmRunClaimOmitsColdBuild { claimed, cold_build } => write!(
                f,
                "claimed campaign total {claimed} omits the cold build {cold_build}"
            ),
        }
    }
}

impl Error for CampaignError {}

fn gcd(a: u128, b: u128) -> u128 {
    let (mut a, mut b) = (a, b);
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

fn reduce(num: u64, den: u64) -> (u64, u64) {
    let divisor = gcd(u128::from(num), u128::from(den));
    (num / u64::try_from(divisor).expect("divides num"), den / u64::try_from(divisor).expect("divides den"))
}

#[cfg(test)]
#[path = "../../../tests/rust/zero-ledger/unit/campaign.rs"]
mod tests;
