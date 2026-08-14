use super::*;

fn rational(num: i128, den: u128) -> Rational {
    Rational::new(num, den).unwrap()
}

fn bound_input(
    trials: u64,
    failures: u64,
    success_target: Rational,
    alpha: Rational,
    independent: bool,
) -> ZeroFailureBoundInput {
    ZeroFailureBoundInput {
        trials,
        failures,
        success_target,
        alpha,
        independent,
        design_effect: None,
    }
}

fn q99() -> Rational {
    rational(99, 100)
}

fn alpha95() -> Rational {
    rational(1, 20)
}

#[test]
fn cache009_certifies_299_zero_failure_trials() {
    let input = bound_input(299, 0, q99(), alpha95(), true);
    let certification = zero_failure_bound_certifies(&input).unwrap();
    assert_eq!(certification.effective_trials, 299);
    assert_eq!(certification.success_target, q99());
    assert_eq!(certification.alpha, alpha95());
}

#[test]
fn cache009_refuses_298_trials() {
    // 299 is the exact sample-size precondition for 99% @ 95% one-sided:
    // one trial short must refuse, never approximate.
    let input = bound_input(298, 0, q99(), alpha95(), true);
    assert_eq!(
        zero_failure_bound_certifies(&input),
        Err(BoundsError::InsufficientTrials { trials: 298 })
    );
}

#[test]
fn cache009_refuses_any_failure() {
    let input = bound_input(299, 1, q99(), alpha95(), true);
    assert_eq!(
        zero_failure_bound_certifies(&input),
        Err(BoundsError::NonZeroFailures { failures: 1 })
    );
}

#[test]
fn cache009_refuses_dependent_warm_trace() {
    // The warm-trace caveat: never certify universal Q99 from dependent
    // trials (temporal dependence, clustering, sliding-window reuse).
    let input = bound_input(299, 0, q99(), alpha95(), false);
    assert_eq!(
        zero_failure_bound_certifies(&input),
        Err(BoundsError::DependentTrace)
    );
}

#[test]
fn cache009_refuses_zero_trials() {
    let input = bound_input(0, 0, q99(), alpha95(), true);
    assert_eq!(
        zero_failure_bound_certifies(&input),
        Err(BoundsError::ZeroTrials)
    );
}

#[test]
fn cache009_refuses_failures_exceeding_trials() {
    let input = bound_input(10, 11, q99(), alpha95(), true);
    assert_eq!(
        zero_failure_bound_certifies(&input),
        Err(BoundsError::FailuresExceedTrials)
    );
}

#[test]
fn cache009_refuses_out_of_unit_parameters() {
    let input = bound_input(299, 0, rational(100, 100), alpha95(), true);
    assert_eq!(
        zero_failure_bound_certifies(&input),
        Err(BoundsError::ParameterOutOfUnitInterval {
            parameter: "success_target"
        })
    );
    let input = bound_input(299, 0, q99(), rational(0, 1), true);
    assert_eq!(
        zero_failure_bound_certifies(&input),
        Err(BoundsError::ParameterOutOfUnitInterval { parameter: "alpha" })
    );
}

#[test]
fn cache009_cluster_design_effect_stiffens_effective_sample() {
    // 299 clustered trials with design effect 2 yield 149 effective trials,
    // which cannot meet the exact precondition.
    let mut input = bound_input(299, 0, q99(), alpha95(), true);
    input.design_effect = Some(rational(2, 1));
    assert_eq!(
        zero_failure_bound_certifies(&input),
        Err(BoundsError::InsufficientTrials { trials: 149 })
    );
    // A fractional design effect of 3/2 yields floor(299*2/3) = 199.
    input.design_effect = Some(rational(3, 2));
    assert_eq!(
        zero_failure_bound_certifies(&input),
        Err(BoundsError::InsufficientTrials { trials: 199 })
    );
    // A design effect of exactly 1 leaves the effective sample untouched.
    input.design_effect = Some(rational(1, 1));
    assert_eq!(zero_failure_bound_certifies(&input).unwrap().effective_trials, 299);
}

#[test]
fn cache009_refuses_invalid_design_effect() {
    let mut input = bound_input(299, 0, q99(), alpha95(), true);
    input.design_effect = Some(rational(1, 2));
    assert_eq!(
        zero_failure_bound_certifies(&input),
        Err(BoundsError::InvalidDesignEffect)
    );
}

#[test]
fn cache009_refuses_vanishing_effective_sample() {
    let mut input = bound_input(1, 0, q99(), alpha95(), true);
    input.design_effect = Some(rational(2, 1));
    assert_eq!(
        zero_failure_bound_certifies(&input),
        Err(BoundsError::InsufficientEffectiveSample { effective: 0 })
    );
}

#[test]
fn cache009_min_trials_matches_proposition_11_1_constants() {
    // Proposition 11.1: q = 0.99, alpha = 0.05 => 299 trials;
    // q = 0.999 => 2995 trials.
    assert_eq!(min_zero_failure_trials(q99(), alpha95()).unwrap(), 299);
    assert_eq!(min_zero_failure_trials(rational(999, 1000), alpha95()).unwrap(), 2995);
}

#[test]
fn cache009_certifies_999_success_rate_at_2995_trials() {
    let input = bound_input(2995, 0, rational(999, 1000), alpha95(), true);
    assert!(zero_failure_bound_certifies(&input).is_ok());
    let input = bound_input(2994, 0, rational(999, 1000), alpha95(), true);
    assert_eq!(
        zero_failure_bound_certifies(&input),
        Err(BoundsError::InsufficientTrials { trials: 2994 })
    );
}

#[test]
fn cache009_deterministic() {
    let input = bound_input(299, 0, q99(), alpha95(), true);
    let first = zero_failure_bound_certifies(&input);
    let second = zero_failure_bound_certifies(&input);
    assert_eq!(first, second);
    assert!(first.is_ok());

    let failing = bound_input(298, 0, q99(), alpha95(), true);
    let first = zero_failure_bound_certifies(&failing);
    let second = zero_failure_bound_certifies(&failing);
    assert_eq!(first, second);
    assert!(first.is_err());
}
