//! SPEC-TZ-PFX-001: estimator cache-floor is 5/4, not a hit claim.

use tokenzero_recovery::prefix_stability::{
    CacheModelTier, CacheablePrefix, PrefixStabilityAlert, PrefixStabilityGuard,
    ESTIMATOR_FLOOR_SAFETY_DENOMINATOR, ESTIMATOR_FLOOR_SAFETY_NUMERATOR,
};

#[test]
fn estimator_floor_is_five_over_four() {
    assert_eq!(ESTIMATOR_FLOOR_SAFETY_NUMERATOR, 5);
    assert_eq!(ESTIMATOR_FLOOR_SAFETY_DENOMINATOR, 4);
    assert_eq!(CacheModelTier::Opus.min_cacheable_tokens(), 4_096);
    assert_eq!(
        CacheModelTier::Opus.min_cacheable_estimator_tokens(),
        4_096usize.saturating_mul(5).div_ceil(4)
    );
}

#[test]
fn below_floor_alerts_and_does_not_claim_a_provider_hit() {
    let mut guard = PrefixStabilityGuard::default();
    let prefix = CacheablePrefix {
        bytes: "short".to_string(),
        cache_breakpoint: true,
        blocks_per_turn: Default::default(),
    };
    let alert = guard
        .observe_prefix(&prefix, CacheModelTier::Opus)
        .expect("short prefix is legal, just below floor");
    match alert {
        Some(PrefixStabilityAlert::BelowCacheableFloor {
            observed_tokens,
            required_tokens,
            model_tier,
        }) => {
            assert!(observed_tokens < required_tokens);
            assert_eq!(model_tier, CacheModelTier::Opus);
        }
        other => panic!("expected below-floor alert, got {other:?}"),
    }
}
