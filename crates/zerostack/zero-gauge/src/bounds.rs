//! ZS-CACHE-009: exact zero-failure Q99 statistical certification bounds.
//!
//! Implements Proposition 11.1 (Zero-Failure Q99 Sample Bound): with `q` the
//! certified success rate and `alpha` the one-sided error, `n` zero-failure
//! independent trials certify the bound exactly when `q^n <= alpha`
//! (equivalently `n >= ln(alpha) / ln(q)`). Known constants: `q = 99/100`,
//! `alpha = 1/20` requires 299 trials; `q = 999/1000` requires 2995.
//!
//! Exactness. The decision `q^n <= alpha` is made with fixed-point integer
//! arithmetic at 64 fractional bits, powered by binary exponentiation over
//! interval bounds widened to u256 via the shared `solver::widen_mul`:
//! no floats, no rounding, no guessed splits. When the lower and upper
//! interval bounds straddle the target, the checker refuses with
//! `AmbiguousBound` (fail closed) instead of guessing.
//!
//! Refusals. The bound applies only to zero-failure independent cold traces:
//! any observed failure, a warm/dependent trace (temporal dependence,
//! project clustering, sliding-window reuse), or a cluster design effect
//! that leaves no effective trials is a typed refusal -- the checker never
//! extrapolates.

use std::cmp::Ordering;
use std::error::Error;
use std::fmt;

use crate::solver::{Rational, widen_mul};

/// Fixed-point scale `2^64`: values in `[0, 1]` are stored as `value * SCALE`.
const SCALE: u128 = 1 << 64;

/// CACHE-009 checker input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZeroFailureBoundInput {
    /// Observed independent trials `n`.
    pub trials: u64,
    /// Observed failures (must be zero for this bound).
    pub failures: u64,
    /// Certified success rate `q`, an exact rational in `(0, 1)`.
    pub success_target: Rational,
    /// One-sided error `alpha` in `(0, 1)`; `1/20` is 95% one-sided.
    pub alpha: Rational,
    /// Premise: the trials are independent (a cold trace; no temporal
    /// dependence, project clustering, or sliding-window reuse).
    pub independent: bool,
    /// Cluster-aware design effect (`>= 1`) when trials are clustered; the
    /// effective trial count is `floor(n / design_effect)`.
    pub design_effect: Option<Rational>,
}

/// CACHE-009 certification: the zero-failure success-rate bound holds at the
/// requested confidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ZeroFailureBoundCertification {
    /// Effective (cluster-adjusted) trial count used for the decision.
    pub effective_trials: u64,
    /// Certified success rate `q`.
    pub success_target: Rational,
    /// One-sided error `alpha`.
    pub alpha: Rational,
}

/// Typed refusal of a statistical certificate. Never a weaker certificate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BoundsError {
    /// No trials were observed.
    ZeroTrials,
    /// The observed failure count exceeds the trial count.
    FailuresExceedTrials,
    /// The zero-failure bound does not apply to a trace with failures.
    NonZeroFailures { failures: u64 },
    /// A rate parameter must lie strictly inside `(0, 1)`.
    ParameterOutOfUnitInterval { parameter: &'static str },
    /// The trace is warm/dependent: universal Q99 cannot be certified from
    /// dependent trials.
    DependentTrace,
    /// The design effect must be `>= 1`.
    InvalidDesignEffect,
    /// The cluster-adjusted effective trial count is zero.
    InsufficientEffectiveSample { effective: u64 },
    /// The effective trial count is below the exact sample-size precondition
    /// `q^n <= alpha`.
    InsufficientTrials { trials: u64 },
    /// The fixed-point interval straddles the target: fail closed.
    AmbiguousBound,
    /// The required trial count exceeds `u64`.
    TrialsOverflow,
}

impl fmt::Display for BoundsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroTrials => formatter.write_str("no trials observed"),
            Self::FailuresExceedTrials => {
                formatter.write_str("failure count exceeds the trial count")
            }
            Self::NonZeroFailures { failures } => {
                write!(
                    formatter,
                    "{failures} failures: the zero-failure bound does not apply"
                )
            }
            Self::ParameterOutOfUnitInterval { parameter } => {
                write!(formatter, "{parameter} must lie strictly inside (0, 1)")
            }
            Self::DependentTrace => {
                formatter.write_str("warm/dependent trace: no universal Q99 certification")
            }
            Self::InvalidDesignEffect => formatter.write_str("design effect must be >= 1"),
            Self::InsufficientEffectiveSample { effective } => {
                write!(formatter, "effective trial count {effective} is zero")
            }
            Self::InsufficientTrials { trials } => write!(
                formatter,
                "{trials} effective zero-failure trials do not meet the exact sample-size precondition"
            ),
            Self::AmbiguousBound => {
                formatter.write_str("fixed-point bounds straddle the target: certification refused")
            }
            Self::TrialsOverflow => formatter.write_str("required trial count exceeds u64"),
        }
    }
}

impl Error for BoundsError {}

/// Certifies the zero-failure bound for the measured trial count.
///
/// Refuses (never extrapolates) when: `n == 0`, failures were observed,
/// `q` or `alpha` lies outside `(0, 1)`, the trace is dependent (warm), the
/// design effect is `< 1`, the effective trial count is zero, or
/// `q^effective_n > alpha`.
pub fn zero_failure_bound_certifies(
    input: &ZeroFailureBoundInput,
) -> Result<ZeroFailureBoundCertification, BoundsError> {
    validate_unit_open(input.success_target, "success_target")?;
    validate_unit_open(input.alpha, "alpha")?;
    if input.trials == 0 {
        return Err(BoundsError::ZeroTrials);
    }
    if input.failures > input.trials {
        return Err(BoundsError::FailuresExceedTrials);
    }
    if input.failures != 0 {
        return Err(BoundsError::NonZeroFailures {
            failures: input.failures,
        });
    }
    if !input.independent {
        return Err(BoundsError::DependentTrace);
    }
    let effective = match input.design_effect {
        None => input.trials,
        Some(design_effect) => {
            if design_effect.num() <= 0 || (design_effect.num() as u128) < design_effect.den() {
                return Err(BoundsError::InvalidDesignEffect);
            }
            effective_trials(input.trials, design_effect)
        }
    };
    if effective == 0 {
        return Err(BoundsError::InsufficientEffectiveSample { effective });
    }
    if !power_le(input.success_target, effective, input.alpha)? {
        return Err(BoundsError::InsufficientTrials { trials: effective });
    }
    Ok(ZeroFailureBoundCertification {
        effective_trials: effective,
        success_target: input.success_target,
        alpha: input.alpha,
    })
}

/// The smallest `n >= 1` with `q^n <= alpha`: the exact sample-size
/// precondition of Proposition 11.1 (`299` for `q = 99/100`, `alpha = 1/20`;
/// `2995` for `q = 999/1000`).
///
/// Deterministic binary search over the monotone power sequence. Refuses
/// with [`BoundsError::AmbiguousBound`] if any step cannot be decided.
pub fn min_zero_failure_trials(q: Rational, alpha: Rational) -> Result<u64, BoundsError> {
    validate_unit_open(q, "success_target")?;
    validate_unit_open(alpha, "alpha")?;
    if power_le(q, 1, alpha)? {
        return Ok(1);
    }
    // Double `hi` until `q^hi <= alpha`; the sequence decreases to 0.
    let mut hi = 1u64;
    loop {
        hi = hi.checked_mul(2).ok_or(BoundsError::TrialsOverflow)?;
        if power_le(q, hi, alpha)? {
            break;
        }
    }
    let mut lo = hi / 2;
    while lo + 1 < hi {
        let mid = lo + (hi - lo) / 2;
        if power_le(q, mid, alpha)? {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Ok(hi)
}

/// Exact fixed-point decision of `q^n <= alpha` for rationals `q, alpha` in
/// `(0, 1)` and `n >= 1`.
///
/// Sound interval arithmetic: `[lo, hi]` always brackets `q^n * SCALE`.
/// `Ok(true)` when the upper bound is still at or below the target, `Ok(false)`
/// when the lower bound is already above the target, and
/// `Err(AmbiguousBound)` when the bounds straddle the target.
fn power_le(q: Rational, n: u64, alpha: Rational) -> Result<bool, BoundsError> {
    if n == 0 {
        // q^0 = 1 > alpha (alpha < 1): the claim is certainly false.
        return Ok(false);
    }
    let target = scale_rational(alpha);
    let base = scale_rational_interval(q);
    let mut power = Interval {
        lo: SCALE,
        hi: SCALE,
    };
    let mut factor = base;
    let mut remaining = n;
    while remaining > 0 {
        if remaining & 1 == 1 {
            power = interval_mul(&power, &factor);
        }
        remaining >>= 1;
        if remaining > 0 {
            factor = interval_mul(&factor, &factor);
        }
    }
    let upper_le =
        cmp_u256(widen_mul(power.hi, target.1), widen_mul(target.0, SCALE)) != Ordering::Greater;
    if upper_le {
        return Ok(true);
    }
    let lower_gt =
        cmp_u256(widen_mul(power.lo, target.1), widen_mul(target.0, SCALE)) == Ordering::Greater;
    if lower_gt {
        return Ok(false);
    }
    Err(BoundsError::AmbiguousBound)
}

/// `alpha * SCALE` as an exact pair `(num, den)` so the comparison
/// `power <= alpha * SCALE` becomes `power * den <= num * SCALE` without
/// rounding.
fn scale_rational(alpha: Rational) -> (u128, u128) {
    (alpha.num() as u128, alpha.den())
}

/// The fixed-point interval `[floor(q * SCALE), ceil(q * SCALE)]`.
fn scale_rational_interval(q: Rational) -> Interval {
    let num = q.num() as u128;
    let den = q.den();
    let (hi, lo) = widen_mul(num, SCALE);
    Interval {
        lo: u256_div_u128_floor(hi, lo, den),
        hi: u256_div_u128_ceil(hi, lo, den),
    }
}

/// One fixed-point interval `[lo, hi]` in units of `SCALE`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Interval {
    lo: u128,
    hi: u128,
}

/// Exact interval product: `floor(lo1 * lo2 / SCALE)` and
/// `ceil(hi1 * hi2 / SCALE)`. Both operands are `<= SCALE`, so the products
/// fit u256.
fn interval_mul(a: &Interval, b: &Interval) -> Interval {
    Interval {
        lo: mul_floor(a.lo, b.lo),
        hi: mul_ceil(a.hi, b.hi),
    }
}

/// `floor(x * y / SCALE)` for `x, y <= SCALE`, exact via u256 widening.
fn mul_floor(x: u128, y: u128) -> u128 {
    let (hi, lo) = widen_mul(x, y);
    (hi << 64) | (lo >> 64)
}

/// `ceil(x * y / SCALE)` for `x, y <= SCALE`, exact via u256 widening.
fn mul_ceil(x: u128, y: u128) -> u128 {
    let (hi, lo) = widen_mul(x, y);
    let (sum, carry) = lo.overflowing_add(SCALE - 1);
    let hi = hi.wrapping_add(u128::from(carry));
    (hi << 64) | (sum >> 64)
}

/// `floor(n / design_effect)` with a rational design effect `>= 1`, exact via
/// u256 widening and bitwise long division. The quotient is `<= n`, so it
/// fits u64.
fn effective_trials(trials: u64, design_effect: Rational) -> u64 {
    let (hi, lo) = widen_mul(trials as u128, design_effect.den());
    u256_div_u128_floor(hi, lo, design_effect.num() as u128) as u64
}

/// Exact `floor((hi * 2^128 + lo) / divisor)` for `divisor > 0`; the quotient
/// must fit u128. Bitwise long division over both words, subtract-once per
/// step (the remainder is always `< divisor` before the shift).
fn u256_div_u128_floor(hi: u128, lo: u128, divisor: u128) -> u128 {
    debug_assert!(divisor > 0);
    let mut quotient = 0u128;
    let mut remainder = 0u128;
    for bit in (0..128).rev() {
        let (mut shifted, over) = remainder.overflowing_shl(1);
        shifted |= (hi >> bit) & 1;
        if over || shifted >= divisor {
            shifted = shifted.wrapping_sub(divisor);
            quotient |= 1 << bit;
        }
        remainder = shifted;
    }
    for bit in (0..128).rev() {
        let (mut shifted, over) = remainder.overflowing_shl(1);
        shifted |= (lo >> bit) & 1;
        if over || shifted >= divisor {
            shifted = shifted.wrapping_sub(divisor);
            quotient |= 1 << bit;
        }
        remainder = shifted;
    }
    quotient
}

/// `ceil((hi * 2^128 + lo) / divisor) = floor((value + divisor - 1) / divisor)`,
/// exact via the floor helper (the addend fits: the numerator is `< 2^256`).
fn u256_div_u128_ceil(hi: u128, lo: u128, divisor: u128) -> u128 {
    let (lo, carry) = lo.overflowing_add(divisor - 1);
    let hi = hi.wrapping_add(u128::from(carry));
    u256_div_u128_floor(hi, lo, divisor)
}

/// Exact three-way comparison of two u256 values `(hi * 2^128 + lo)`.
fn cmp_u256(a: (u128, u128), b: (u128, u128)) -> Ordering {
    a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1))
}

/// Refuses a rate parameter outside the open unit interval `(0, 1)`.
fn validate_unit_open(rate: Rational, parameter: &'static str) -> Result<(), BoundsError> {
    if rate.num() <= 0 || (rate.num() as u128) >= rate.den() {
        return Err(BoundsError::ParameterOutOfUnitInterval { parameter });
    }
    Ok(())
}
