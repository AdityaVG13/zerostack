//! Frontier Closure decomposition.

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize, de};

/// The three exhaustive Frontier Closure terms.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrontierTerm {
    /// Normalized preparation work.
    Preparation,
    /// Normalized prepared-path work.
    PreparedPath,
    /// Normalized novelty-plus-fallback work.
    NoveltyFallback,
}

impl FrontierTerm {
    /// Every term, in canonical order.
    pub const ALL: [Self; 3] = [Self::Preparation, Self::PreparedPath, Self::NoveltyFallback];

    /// Canonical lowercase term string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Preparation => "preparation",
            Self::PreparedPath => "prepared_path",
            Self::NoveltyFallback => "novelty_fallback",
        }
    }
}

impl fmt::Display for FrontierTerm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The largest limiting burden: which term limits the closure and by how
/// much.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LimitingBurden {
    /// The limiting term.
    pub term: FrontierTerm,
    /// Its reduced normalized value `term / baseline`.
    pub normalized: (u64, u64),
    /// Its absolute value.
    pub absolute: u64,
}

/// A validated Frontier Closure decomposition.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FrontierClosure {
    baseline_total: u64,
    optimized_total: u64,
    preparation: u64,
    prepared_path: u64,
    novelty_fallback: u64,
}

impl FrontierClosure {
    /// Builds a closure, refusing a zero baseline (no denominator) and a
    /// term sum that disagrees with the optimized total (closure violated).
    pub fn new(
        baseline_total: u64,
        optimized_total: u64,
        preparation: u64,
        prepared_path: u64,
        novelty_fallback: u64,
    ) -> Result<Self, FrontierError> {
        if baseline_total == 0 {
            return Err(FrontierError::ZeroBaselineTotal);
        }
        let term_sum = preparation
            .checked_add(prepared_path)
            .and_then(|sum| sum.checked_add(novelty_fallback))
            .ok_or(FrontierError::Overflow)?;
        if term_sum != optimized_total {
            return Err(FrontierError::TermSumMismatch {
                term_sum,
                optimized: optimized_total,
            });
        }
        Ok(Self {
            baseline_total,
            optimized_total,
            preparation,
            prepared_path,
            novelty_fallback,
        })
    }

    /// The baseline total every term is normalized by.
    pub fn baseline_total(&self) -> u64 {
        self.baseline_total
    }

    /// The optimized total the terms must sum to.
    pub fn optimized_total(&self) -> u64 {
        self.optimized_total
    }

    /// The absolute amount of one term.
    pub fn term(&self, term: FrontierTerm) -> u64 {
        match term {
            FrontierTerm::Preparation => self.preparation,
            FrontierTerm::PreparedPath => self.prepared_path,
            FrontierTerm::NoveltyFallback => self.novelty_fallback,
        }
    }

    /// One term normalized by the baseline, as the reduced fraction
    /// `term / baseline_total`.
    pub fn normalized_term(&self, term: FrontierTerm) -> (u64, u64) {
        reduce(self.term(term), self.baseline_total)
    }

    /// The complete optimized/baseline ratio, as the reduced fraction.
    pub fn optimized_ratio(&self) -> (u64, u64) {
        reduce(self.optimized_total, self.baseline_total)
    }

    /// Closure checker: the normalized terms sum exactly to the complete optimized/baseline ratio.
    pub fn closure_holds(&self) -> bool {
        self.preparation
            .checked_add(self.prepared_path)
            .and_then(|sum| sum.checked_add(self.novelty_fallback))
            == Some(self.optimized_total)
    }

    /// The largest limiting burden: the term with the largest normalized value; ties break
    /// in canonical term order. `None` when every term is zero: there is no burden to report.
    pub fn largest_limiting_burden(&self) -> Option<LimitingBurden> {
        let mut best: Option<(FrontierTerm, (u64, u64), u64)> = None;
        for term in FrontierTerm::ALL {
            let normalized = self.normalized_term(term);
            let absolute = self.term(term);
            // Exact rational comparison: num1 * den2 vs num2 * den1, both
            // products fit u128.
            let greater = best.as_ref().is_none_or(|(_, (num, den), _)| {
                u128::from(normalized.0) * u128::from(*den)
                    > u128::from(*num) * u128::from(normalized.1)
            });
            if greater {
                best = Some((term, normalized, absolute));
            }
        }
        let (term, normalized, absolute) = best?;
        if absolute == 0 {
            return None;
        }
        Some(LimitingBurden {
            term,
            normalized,
            absolute,
        })
    }
}

#[derive(Deserialize)]
struct FrontierClosureWire {
    baseline_total: u64,
    optimized_total: u64,
    preparation: u64,
    prepared_path: u64,
    novelty_fallback: u64,
}

impl<'de> Deserialize<'de> for FrontierClosure {
    /// Wire decoding goes through the validated constructor: a tampered
    /// closure is refused.
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = FrontierClosureWire::deserialize(deserializer)?;
        Self::new(
            wire.baseline_total,
            wire.optimized_total,
            wire.preparation,
            wire.prepared_path,
            wire.novelty_fallback,
        )
        .map_err(de::Error::custom)
    }
}

/// Typed Frontier Closure failures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrontierError {
    /// A zero baseline leaves no denominator for any normalized term.
    ZeroBaselineTotal,
    /// The terms do not sum to the optimized total: closure violated.
    TermSumMismatch {
        /// Sum of the three terms.
        term_sum: u64,
        /// Declared optimized total.
        optimized: u64,
    },
    /// The term sum overflowed u64.
    Overflow,
}

impl fmt::Display for FrontierError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroBaselineTotal => {
                f.write_str("a zero baseline total leaves no closure denominator")
            }
            Self::TermSumMismatch {
                term_sum,
                optimized,
            } => write!(
                f,
                "frontier terms sum to {term_sum} but the optimized total is {optimized}: closure violated"
            ),
            Self::Overflow => f.write_str("frontier term sum would overflow u64"),
        }
    }
}

impl Error for FrontierError {}

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
    (
        num / u64::try_from(divisor).expect("divides num"),
        den / u64::try_from(divisor).expect("divides den"),
    )
}
