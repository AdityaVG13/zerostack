//! ZS-METRIC-004: exact multi-resource feasible hit-rate intersection solver.
//!
//! Cost model (documented). For one resource coordinate the expected cost of
//! an operation served at hit rate `h` is
//!
//! ```text
//!   cost(h) = preparation + h * hit + (1 - h) * fallback
//! ```
//!
//! where `preparation`, `hit` and `fallback` are measured intervals. A cache
//! hit costs `hit` with probability `h`; a miss is served by the degraded
//! fallback path with probability `1 - h`; `preparation` is the fixed
//! per-operation overhead. The baseline interval is the status-quo no-reuse
//! cost and anchors the ordering invariants: the whole fallback interval must
//! sit at or below the whole baseline interval, and the whole hit interval at
//! or below the whole fallback interval. Baseline itself is not part of the
//! blended-cost inequality: a miss is served by the fallback path, not by a
//! full recompute.
//!
//! Certified (sufficient) condition. A hit rate `h` is certified for a
//! coordinate only if the worst case inside the uncertainty box still meets
//! the target:
//!
//! ```text
//!   preparation.hi + h * hit.hi + (1 - h) * fallback.hi <= target.lo
//! ```
//!
//! Because the coefficients `1`, `h` and `1 - h` are nonnegative on `[0, 1]`,
//! the worst case is the corner `(preparation.hi, hit.hi, fallback.hi)`. The
//! exists (necessary) condition uses the best-case corner
//! `(preparation.lo, hit.lo, fallback.lo)` against `target.hi`; it is
//! reported for diagnosis only and never certifies.
//!
//! Exactness and refusal. All arithmetic is exact rational arithmetic over
//! i128/u128 with u256 widening for comparisons: no floats, no rounding, and
//! no guessed splits. The same input always yields the same intervals. When
//! the certified intersection over every coordinate is empty, the solver
//! returns a typed blocker naming the first coordinate (in input order) whose
//! feasible interval disjoints the running intersection; it never returns a
//! compromise hit rate. Inverted intervals, coordinate values that contradict
//! the path ordering, and empty coordinate sets are loud refusals.
//!
//! The caller builds [`ResourceCoordinate`] from measured resource receipts;
//! this module is pure interval math and performs no I/O.

use std::cmp::Ordering;
use std::error::Error;
use std::fmt;

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};

/// A closed measured interval `[lo, hi]` of one resource quantity.
///
/// `lo > hi` is a typed refusal: an inverted interval is ambiguous data, never
/// silently swapped. Values are u64; the solver widens to i128/u256 for exact
/// comparisons.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Interval {
    /// Lower endpoint of the measured interval.
    pub lo: u64,
    /// Upper endpoint of the measured interval.
    pub hi: u64,
}

impl Interval {
    /// Builds an interval, refusing an inverted `lo > hi`.
    pub fn new(lo: u64, hi: u64) -> Result<Self, SolverError> {
        if lo > hi {
            return Err(SolverError::InvertedInterval {
                field: "interval".into(),
                lo,
                hi,
            });
        }
        Ok(Self { lo, hi })
    }
}

impl<'de> Deserialize<'de> for Interval {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            lo: u64,
            hi: u64,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.lo, wire.hi).map_err(de::Error::custom)
    }
}

/// One resource coordinate: the four measured intervals plus the target.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResourceCoordinate {
    /// Stable coordinate name, e.g. "tokens", "cpu_ns", "storage_bytes".
    pub name: String,
    /// Status-quo no-reuse cost interval.
    pub baseline: Interval,
    /// Cost interval of one cache hit.
    pub hit: Interval,
    /// Cost interval of one fallback-path miss.
    pub fallback: Interval,
    /// Fixed per-operation overhead interval.
    pub preparation: Interval,
    /// Target budget interval the blended cost must not exceed.
    pub target: Interval,
}

impl ResourceCoordinate {
    /// Builds a coordinate, refusing inverted intervals, an empty name, and
    /// path-order contradictions (hit above fallback, fallback above
    /// baseline).
    pub fn new(
        name: impl Into<String>,
        baseline: Interval,
        hit: Interval,
        fallback: Interval,
        preparation: Interval,
        target: Interval,
    ) -> Result<Self, SolverError> {
        let name = name.into();
        if name.is_empty() {
            return Err(SolverError::EmptyCoordinateName);
        }
        let _ = Interval::new(baseline.lo, baseline.hi)?;
        let _ = Interval::new(hit.lo, hit.hi)?;
        let _ = Interval::new(fallback.lo, fallback.hi)?;
        let _ = Interval::new(preparation.lo, preparation.hi)?;
        let _ = Interval::new(target.lo, target.hi)?;
        if hit.hi > fallback.lo {
            return Err(SolverError::InconsistentModel {
                coordinate: name.clone(),
                relation: "hit above fallback",
            });
        }
        if fallback.hi > baseline.lo {
            return Err(SolverError::InconsistentModel {
                coordinate: name.clone(),
                relation: "fallback above baseline",
            });
        }
        Ok(Self {
            name,
            baseline,
            hit,
            fallback,
            preparation,
            target,
        })
    }
}

impl<'de> Deserialize<'de> for ResourceCoordinate {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            name: String,
            baseline: Interval,
            hit: Interval,
            fallback: Interval,
            preparation: Interval,
            target: Interval,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::new(
            wire.name,
            wire.baseline,
            wire.hit,
            wire.fallback,
            wire.preparation,
            wire.target,
        )
        .map_err(de::Error::custom)
    }
}

/// An exact reduced rational `num / den` with `den > 0`.
///
/// Values are reduced at construction, so equality is structural. Comparisons
/// widen to u256 via schoolbook multiplication: nothing is rounded.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
pub struct Rational {
    num: i128,
    den: u128,
}

impl Rational {
    /// Zero as an exact rational.
    pub const ZERO: Self = Self { num: 0, den: 1 };
    /// One as an exact rational.
    pub const ONE: Self = Self { num: 1, den: 1 };

    /// Builds a reduced rational, refusing a zero denominator.
    pub fn new(num: i128, den: u128) -> Result<Self, SolverError> {
        if den == 0 {
            return Err(SolverError::ZeroDenominator);
        }
        let divisor = gcd(num.unsigned_abs(), den);
        Ok(Self {
            num: num / i128::try_from(divisor).expect("gcd of u128 divides i128"),
            den: den / divisor,
        })
    }

    /// The signed numerator.
    pub fn num(self) -> i128 {
        self.num
    }

    /// The positive denominator.
    pub fn den(self) -> u128 {
        self.den
    }

    /// Whether this rational is exactly zero.
    pub fn is_zero(self) -> bool {
        self.num == 0
    }

    /// Whether this rational is exactly one.
    pub fn is_one(self) -> bool {
        self.num == 1 && self.den == 1
    }

    /// Exact maximum of two rationals.
    pub fn max(self, other: Self) -> Self {
        if self >= other {
            self
        } else {
            other
        }
    }

    /// Exact minimum of two rationals.
    pub fn min(self, other: Self) -> Self {
        if self <= other {
            self
        } else {
            other
        }
    }

    /// Exact three-way comparison, never rounding.
    pub fn compare(self, other: Self) -> Ordering {
        match (self.num.cmp(&0), other.num.cmp(&0)) {
            (Ordering::Less, Ordering::Greater) => Ordering::Less,
            (Ordering::Greater, Ordering::Less) => Ordering::Greater,
            // Self is zero: compare zero against the other.
            (Ordering::Equal, _) => 0_i128.cmp(&other.num),
            (_, Ordering::Equal) => self.num.cmp(&0_i128),
            (Ordering::Greater, Ordering::Greater) => u256_cmp(
                u256_unsigned_mul(self.num.unsigned_abs(), other.den),
                u256_unsigned_mul(other.num.unsigned_abs(), self.den),
            ),
            (Ordering::Less, Ordering::Less) => {
                // Both negative: self < other iff |self| > |other|.
                u256_cmp(
                    u256_unsigned_mul(other.num.unsigned_abs(), self.den),
                    u256_unsigned_mul(self.num.unsigned_abs(), other.den),
                )
            }
        }
    }
}

impl PartialOrd for Rational {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Rational {
    fn cmp(&self, other: &Self) -> Ordering {
        self.compare(*other)
    }
}

impl fmt::Display for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.num, self.den)
    }
}

impl<'de> Deserialize<'de> for Rational {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            num: i128,
            den: u128,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.num, wire.den).map_err(de::Error::custom)
    }
}

fn gcd(a: u128, b: u128) -> u128 {
    let (mut a, mut b) = (a, b);
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

/// The feasible hit-rate interval of one coordinate, endpoints inclusive.
///
/// Both endpoints lie inside `[0, 1]` and `min <= max` by construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FeasibleInterval {
    /// Smallest certified hit rate.
    pub min: Rational,
    /// Largest certified hit rate.
    pub max: Rational,
}

impl FeasibleInterval {
    /// Exact membership: whether `h` lies inside this interval.
    pub fn contains(self, h: Rational) -> bool {
        self.min <= h && h <= self.max
    }
}

impl fmt::Display for FeasibleInterval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}, {}]", self.min, self.max)
    }
}

/// Per-coordinate solver report: certified and diagnostic intervals.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CoordinateReport {
    /// Coordinate name.
    pub coordinate: String,
    /// Certified (worst-case) feasible interval; `None` when empty.
    pub certified: Option<FeasibleInterval>,
    /// Exists (best-case) feasible interval; `None` when certainly empty.
    pub exists: Option<FeasibleInterval>,
}

/// The certified feasible hit-rate intersection over every coordinate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FeasibleIntersection {
    /// Certified intersection lower bound (max of coordinate minima).
    pub min: Rational,
    /// Certified intersection upper bound (min of coordinate maxima).
    pub max: Rational,
    /// Per-coordinate reports, in input order.
    pub coordinates: Vec<CoordinateReport>,
}

impl FeasibleIntersection {
    /// Exact membership of the certified intersection.
    pub fn contains(&self, h: Rational) -> bool {
        self.min <= h && h <= self.max
    }
}

/// Blocker report: which coordinate made the certified intersection empty.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Blocker {
    /// The coordinate whose feasible interval disjoints the intersection.
    pub coordinate: String,
    /// That coordinate's certified feasible interval; `None` when the
    /// coordinate is itself infeasible at every hit rate.
    pub feasible: Option<FeasibleInterval>,
    /// The running intersection immediately before the blocker was applied.
    pub intersection_before: FeasibleInterval,
}

/// Typed solver failures. None of these are recoverable by averaging inputs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SolverError {
    /// A rational was constructed with a zero denominator.
    ZeroDenominator,
    /// An interval was constructed with `lo > hi`.
    InvertedInterval {
        /// Which interval field was inverted.
        field: String,
        /// The lower endpoint.
        lo: u64,
        /// The upper endpoint.
        hi: u64,
    },
    /// A coordinate carried no name, so no blocker could be reported.
    EmptyCoordinateName,
    /// Measured intervals contradict the path ordering of the cost model.
    InconsistentModel {
        /// Coordinate name.
        coordinate: String,
        /// Which ordering invariant was violated.
        relation: &'static str,
    },
    /// No coordinates were provided, so no intersection exists.
    NoCoordinates,
    /// The certified intersection is empty: feasibility is refused, not
    /// guessed.
    EmptyIntersection {
        /// The coordinate that made the intersection empty.
        blocker: Box<Blocker>,
    },
}

impl fmt::Display for SolverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroDenominator => f.write_str("a rational denominator was zero"),
            Self::InvertedInterval { field, lo, hi } => {
                write!(f, "inverted interval {field}: lo {lo} > hi {hi}")
            }
            Self::EmptyCoordinateName => f.write_str("a coordinate carried an empty name"),
            Self::InconsistentModel {
                coordinate,
                relation,
            } => write!(f, "coordinate {coordinate} is inconsistent: {relation}"),
            Self::NoCoordinates => f.write_str("no coordinates were provided"),
            Self::EmptyIntersection { blocker } => match blocker.feasible {
                Some(feasible) => write!(
                    f,
                    "no feasible hit rate: coordinate {} requires {} but the running intersection is {}",
                    blocker.coordinate, feasible, blocker.intersection_before
                ),
                None => write!(
                    f,
                    "no feasible hit rate: coordinate {} is infeasible at every hit rate; running intersection was {}",
                    blocker.coordinate, blocker.intersection_before
                ),
            },
        }
    }
}

impl Error for SolverError {}

/// Computes the certified (worst-case) feasible interval for one corner box.
///
/// Solves `p + h * hit + (1 - h) * fallback <= t` for `h in [0, 1]` with exact
/// rational bounds. `None` means the interval is empty: even the best hit rate
/// cannot meet the target inside this box.
fn corner_interval(p: u64, hit: u64, fallback: u64, target: u64) -> Option<FeasibleInterval> {
    // k = target - p - fallback; slope = hit - fallback. Both fit i128.
    let k = i128::from(target) - i128::from(p) - i128::from(fallback);
    let slope = i128::from(hit) - i128::from(fallback);
    match slope.cmp(&0) {
        Ordering::Equal => {
            // Cost is constant p + fallback regardless of the hit rate.
            if k >= 0 {
                Some(FeasibleInterval {
                    min: Rational::ZERO,
                    max: Rational::ONE,
                })
            } else {
                None
            }
        }
        Ordering::Greater => {
            // h <= k / slope; the constraint is vacuous below h = 0.
            if k < 0 {
                return None;
            }
            let bound = Rational::new(k, slope as u128)
                .expect("slope > 0 and k >= 0, so the rational is valid");
            Some(FeasibleInterval {
                min: Rational::ZERO,
                max: bound.min(Rational::ONE),
            })
        }
        Ordering::Less => {
            // h >= k / slope (division flips the inequality).
            if k >= 0 {
                return Some(FeasibleInterval {
                    min: Rational::ZERO,
                    max: Rational::ONE,
                });
            }
            let bound = Rational::new(-k, (-slope) as u128)
                .expect("slope < 0 and k < 0, so both parts are positive");
            if bound > Rational::ONE {
                None
            } else {
                Some(FeasibleInterval {
                    min: bound,
                    max: Rational::ONE,
                })
            }
        }
    }
}

/// The certified (worst-case) feasible hit-rate interval of one coordinate.
pub fn certified_feasible_interval(coordinate: &ResourceCoordinate) -> Option<FeasibleInterval> {
    corner_interval(
        coordinate.preparation.hi,
        coordinate.hit.hi,
        coordinate.fallback.hi,
        coordinate.target.lo,
    )
}

/// The exists (best-case) feasible hit-rate interval of one coordinate.
///
/// Diagnostic only: an exists interval never certifies feasibility, because
/// the true instantiation inside the box is unknown.
pub fn exists_feasible_interval(coordinate: &ResourceCoordinate) -> Option<FeasibleInterval> {
    corner_interval(
        coordinate.preparation.lo,
        coordinate.hit.lo,
        coordinate.fallback.lo,
        coordinate.target.hi,
    )
}

/// Exact direct evaluation of the certified condition at `h`:
/// `preparation.hi + h * hit.hi + (1 - h) * fallback.hi <= target.lo`.
///
/// This is the independent box check used to validate the analytic interval:
/// for any hit rate, the direct evaluation and the analytic interval must
/// agree exactly.
pub fn certified_holds(coordinate: &ResourceCoordinate, h: Rational) -> bool {
    let p = i128::from(coordinate.preparation.hi);
    let hit = i128::from(coordinate.hit.hi);
    let fallback = i128::from(coordinate.fallback.hi);
    let target = i128::from(coordinate.target.lo);
    let den = i128::try_from(h.den()).expect("denominator fits i128");
    let lhs = u256_add(
        u256_signed_mul(p + fallback, den),
        u256_signed_mul(h.num(), hit - fallback),
    );
    let rhs = u256_signed_mul(target, den);
    u256_cmp(lhs, rhs) != Ordering::Greater
}

/// Exact direct evaluation of the exists condition at `h`:
/// `preparation.lo + h * hit.lo + (1 - h) * fallback.lo <= target.hi`.
pub fn exists_holds(coordinate: &ResourceCoordinate, h: Rational) -> bool {
    let p = i128::from(coordinate.preparation.lo);
    let hit = i128::from(coordinate.hit.lo);
    let fallback = i128::from(coordinate.fallback.lo);
    let target = i128::from(coordinate.target.hi);
    let den = i128::try_from(h.den()).expect("denominator fits i128");
    let lhs = u256_add(
        u256_signed_mul(p + fallback, den),
        u256_signed_mul(h.num(), hit - fallback),
    );
    let rhs = u256_signed_mul(target, den);
    u256_cmp(lhs, rhs) != Ordering::Greater
}

/// Computes the certified feasible hit-rate intersection over every
/// coordinate, or reports the coordinate that makes it empty.
///
/// Deterministic: the same coordinates in the same order always yield the
/// same intersection, and the blocker is always the first coordinate whose
/// interval disjoints the running intersection. Infeasibility is a typed
/// error, never a compromise hit rate.
pub fn feasible_intersection(
    coordinates: &[ResourceCoordinate],
) -> Result<FeasibleIntersection, SolverError> {
    if coordinates.is_empty() {
        return Err(SolverError::NoCoordinates);
    }
    let mut min = Rational::ZERO;
    let mut max = Rational::ONE;
    let mut reports = Vec::with_capacity(coordinates.len());
    for coordinate in coordinates {
        let certified = certified_feasible_interval(coordinate);
        let exists = exists_feasible_interval(coordinate);
        let report = CoordinateReport {
            coordinate: coordinate.name.clone(),
            certified,
            exists,
        };
        if let Some(interval) = certified {
            let intersection_before = FeasibleInterval { min, max };
            let next_min = min.max(interval.min);
            let next_max = max.min(interval.max);
            if next_min > next_max {
                return Err(SolverError::EmptyIntersection {
                    blocker: Box::new(Blocker {
                        coordinate: coordinate.name.clone(),
                        feasible: Some(interval),
                        intersection_before,
                    }),
                });
            }
            min = next_min;
            max = next_max;
        } else {
            return Err(SolverError::EmptyIntersection {
                blocker: Box::new(Blocker {
                    coordinate: coordinate.name.clone(),
                    feasible: None,
                    intersection_before: FeasibleInterval { min, max },
                }),
            });
        }
        reports.push(report);
    }
    Ok(FeasibleIntersection {
        min,
        max,
        coordinates: reports,
    })
}

// --- Exact u256 arithmetic (schoolbook), used only for comparisons. ---

/// A signed 256-bit value in two's complement, `hi * 2^128 + lo` with signed
/// `hi`. Every intermediate magnitude here is below `2^132`, so `hi` never
/// grows beyond a few bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct U256 {
    hi: i128,
    lo: u128,
}

fn u256_add(a: U256, b: U256) -> U256 {
    let (lo, carry) = a.lo.overflowing_add(b.lo);
    U256 {
        hi: a.hi + b.hi + i128::from(carry),
        lo,
    }
}

fn u256_neg(a: U256) -> U256 {
    let (lo, carry) = (!a.lo).overflowing_add(1);
    U256 {
        hi: !a.hi + i128::from(carry),
        lo,
    }
}

fn u256_cmp(a: U256, b: U256) -> Ordering {
    a.hi.cmp(&b.hi).then_with(|| a.lo.cmp(&b.lo))
}

/// Exact sign-aware product of two signed i128 values widened to u256.
fn u256_signed_mul(a: i128, b: i128) -> U256 {
    let negative = (a < 0) != (b < 0);
    let mut value = u256_unsigned_mul(a.unsigned_abs(), b.unsigned_abs());
    if negative {
        value = u256_neg(value);
    }
    value
}

/// Exact unsigned product of two nonnegative values widened to u256.
fn u256_unsigned_mul(a: u128, b: u128) -> U256 {
    let (hi, lo) = widen_mul(a, b);
    U256 {
        hi: i128::try_from(hi).expect("hi fits i128"),
        lo,
    }
}

/// Schoolbook u128 x u128 -> u256 multiplication, `(hi, lo)`.
///
/// No widening primitive is assumed on the toolchain; this is exact and
/// carries correctly at every boundary. `pub(crate)` so the theorem-checker
/// and statistical-bound modules reuse the same widening instead of
/// reimplementing it.
pub(crate) fn widen_mul(a: u128, b: u128) -> (u128, u128) {
    const MASK: u128 = u128::MAX >> 64;
    let a_lo = a & MASK;
    let a_hi = a >> 64;
    let b_lo = b & MASK;
    let b_hi = b >> 64;
    let ll = a_lo * b_lo;
    let lh = a_hi * b_lo;
    let hl = a_lo * b_hi;
    let hh = a_hi * b_hi;
    // m = lh + hl = m_hi * 2^64 + m_lo, with m < 2^129.
    let m_lo = (lh & MASK) + (hl & MASK);
    let m_hi = (lh >> 64) + (hl >> 64) + (m_lo >> 64);
    // lo = (m_lo * 2^64 + ll) mod 2^128 = ((m_lo + ll_hi) * 2^64 + ll_lo) mod 2^128.
    let ll_hi = ll >> 64;
    let ll_lo = ll & MASK;
    let mid = m_lo + ll_hi;
    let lo = ((mid & MASK) << 64) | ll_lo;
    let hi = hh + m_hi + (mid >> 64);
    (hi, lo)
}

#[cfg(test)]
#[path = "../../../tests/rust/zero-gauge/unit/solver.rs"]
mod tests;
