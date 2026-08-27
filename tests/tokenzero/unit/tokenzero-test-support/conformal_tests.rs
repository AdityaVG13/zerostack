use super::*;
use crate::parity_taxonomy::{FeatureUniverse, truncate_score};

fn tiny_all_passing(n: usize) -> FeatureUniverse {
    let mut toml = String::from(
        r#"
schema_version = "gauntlet.supported_surface_matrix.v1"
[categories.only]
weight = 1.0
"#,
    );
    let w = 1.0 / n as f64;
    for i in 0..n {
        toml.push_str(&format!(
            r#"
[[features]]
id = "F-TZ-P{i:02}"
title = "pass {i}"
category = "only"
weight = {w}
status = "supported"
"#
        ));
    }
    FeatureUniverse::load_from_str(&toml, "perfect-small-n").expect("tiny universe")
}

#[test]
fn beta_uniform_mean_and_closed_form_quantiles() {
    let uni = BetaParams::UNIFORM_PRIOR;
    assert_eq!(uni.mean(), 0.5);
    assert!((uni.variance() - 1.0 / 12.0).abs() < 1e-12);
    assert!((uni.quantile(0.25) - 0.25).abs() < 1e-12);
    assert!((uni.quantile(0.975) - 0.975).abs() < 1e-12);

    let two_one = BetaParams {
        alpha: 2.0,
        beta: 1.0,
    };
    assert!((two_one.mean() - 2.0 / 3.0).abs() < 1e-12);
    let q025 = two_one.quantile(0.025);
    assert!((q025 - 0.025_f64.sqrt()).abs() < 1e-12);
    assert!((two_one.cdf(q025) - 0.025).abs() < 1e-9);
}

#[test]
fn one_of_one_point_is_not_the_lower_bound() {
    let iv = score_passes_trials(1.0, 1.0, DEFAULT_CONFIDENCE);
    assert_eq!(iv.point, truncate_score(2.0 / 3.0));
    assert_eq!(iv.lower, truncate_score(0.025_f64.sqrt()));
    assert_eq!(iv.upper, truncate_score(0.975_f64.sqrt()));
    assert!(iv.lower < iv.point);
    assert!(iv.point < 1.0);
    assert_ne!(iv.lower, 1.0);
}

#[test]
fn hundred_of_hundred_still_cannot_certify_at_one() {
    let iv = score_passes_trials(100.0, 100.0, DEFAULT_CONFIDENCE);
    assert!(iv.point < 1.0);
    assert!(iv.lower < iv.point);
    assert!(iv.lower < 0.99);
}

#[test]
fn point_estimate_as_bound_is_fail_closed() {
    assert!(!release_pass_on_point_estimate(1.0, 1.0));
    assert!(!release_pass_on_point_estimate(0.999999, 0.5));
    assert!(!release_pass_on_point_estimate(0.0, 0.0));
}

#[test]
fn tiny_perfect_universe_cannot_certify_on_point_or_raw() {
    let u = tiny_all_passing(1);
    assert!(u.strict_100_certifiable());
    assert_eq!(u.effective_coverage(), 1.0);
    let sc = u.conformal_scorecard();
    assert_eq!(sc.release_uses, "conformal_lower");
    assert_eq!(sc.point_estimate_as_bound, "fail_closed");
    assert_eq!(sc.global_raw, 1.0);
    assert_eq!(sc.global_point, truncate_score(2.0 / 3.0));
    assert_eq!(sc.global_lower, truncate_score(0.025_f64.sqrt()));
    assert_eq!(
        sc.release_decision(sc.global_point, 0.0),
        ReleaseVerdict::Block(ReleaseBlock::PointEstimateUsedAsBound)
    );
    assert_eq!(
        sc.release_decision(sc.global_raw, 0.0),
        ReleaseVerdict::Block(ReleaseBlock::PointEstimateUsedAsBound)
    );
    assert_eq!(
        sc.release_decision(u.effective_coverage(), 0.0),
        ReleaseVerdict::Block(ReleaseBlock::PointEstimateUsedAsBound)
    );
    assert!(!u.conformal_release_eligible(1.0));
    assert!(!u.conformal_release_eligible(0.99));
    assert!(!sc.release_on_point_estimate(0.0));
    assert_eq!(
        sc.release_decision(sc.global_lower, sc.global_lower),
        ReleaseVerdict::Allow
    );
}

#[test]
fn three_of_three_still_below_release_one() {
    let u = tiny_all_passing(3);
    assert!(u.strict_100_certifiable());
    let sc = u.conformal_scorecard();
    assert_eq!(sc.global_raw, 1.0);
    assert_eq!(sc.global_point, truncate_score(4.0 / 5.0));
    assert!(sc.global_lower < sc.global_point);
    assert!(!u.conformal_release_eligible(1.0));
    assert_eq!(
        sc.release_decision(sc.global_point, 0.5),
        ReleaseVerdict::Block(ReleaseBlock::PointEstimateUsedAsBound)
    );
}

#[test]
fn no_evidence_fails_closed() {
    let sc = score_categories(&[], BetaParams::UNIFORM_PRIOR, DEFAULT_CONFIDENCE, &[]);
    assert_eq!(
        sc.release_decision(sc.global_lower, 0.0),
        ReleaseVerdict::Block(ReleaseBlock::NoEvidence)
    );
    assert!(!sc.conformal_certifiable(0.0));
}

#[test]
fn conformal_residuals_widen_below_bayesian_lower() {
    let bayes = score_passes_trials(20.0, 20.0, DEFAULT_CONFIDENCE);
    let residuals = [0.40, 0.41, 0.42, 0.39, 0.43, 0.40, 0.41, 0.42];
    let (band, status, q) = apply_conformal_residuals(bayes, &residuals, DEFAULT_CONFIDENCE);
    assert_eq!(status, ConformalStatus::Calibrated);
    assert!(q.is_some());
    assert!(band.lower < bayes.lower);
    assert_eq!(
        band.lower,
        truncate_score((bayes.point - q.unwrap()).max(0.0))
    );
}

#[test]
fn fewer_than_two_residuals_stays_bootstrap_bayesian() {
    let bayes = score_passes_trials(5.0, 5.0, DEFAULT_CONFIDENCE);
    let (band, status, q) = apply_conformal_residuals(bayes, &[0.2], DEFAULT_CONFIDENCE);
    assert_eq!(status, ConformalStatus::BootstrapBayesian);
    assert!(q.is_none());
    assert_eq!(band.lower, bayes.lower);
}

#[test]
fn embedded_universe_lower_bound_blocks_release_at_one() {
    let u = FeatureUniverse::load_embedded().expect("embedded matrix");
    let sc = u.conformal_scorecard();
    assert_eq!(sc.schema_version, SCORECARD_SCHEMA);
    assert_eq!(sc.conformal_status, ConformalStatus::BootstrapBayesian);
    assert_eq!(sc.categories.len(), 12);
    assert!(sc.observation_count > 0.0);
    assert!(sc.global_lower < sc.global_point);
    assert!(sc.global_point <= sc.global_raw || sc.global_raw == 0.0);
    assert_ne!(sc.global_lower, u.effective_coverage());
    assert!(!u.conformal_release_eligible(1.0));
    assert!(!u.strict_100_certifiable());
    assert_eq!(
        sc.release_decision(sc.global_point, 0.0),
        ReleaseVerdict::Block(ReleaseBlock::PointEstimateUsedAsBound)
    );
    assert_eq!(
        sc.release_decision(u.effective_coverage(), 0.0),
        ReleaseVerdict::Block(ReleaseBlock::PointEstimateUsedAsBound)
    );
    let mcp = sc
        .categories
        .get("mcp-cli-honesty")
        .expect("mcp-cli-honesty");
    assert_eq!(mcp.trials, 3.0);
    assert!(mcp.raw_rate < 1.0);
    assert!(mcp.lower < mcp.point);
    let json = serde_json::to_string(&sc).expect("scorecard json");
    assert!(json.contains("conformal_lower"));
    assert!(json.contains("fail_closed"));
    if std::env::var("TOKENZERO_DUMP_SCORECARD").as_deref() == Ok("1") {
        eprintln!("{}", serde_json::to_string_pretty(&sc).expect("pretty"));
    }
}

#[test]
fn truncate_score_applied_to_all_leaf_outputs() {
    let iv = score_passes_trials(7.0, 10.0, DEFAULT_CONFIDENCE);
    for x in [iv.point, iv.lower, iv.upper] {
        assert_eq!(x, truncate_score(x));
    }
}

#[test]
fn category_partial_splits_alpha_and_beta() {
    let ev = CategoryEvidence {
        category: "only".into(),
        weight: 1.0,
        successes: 0.5,
        failures: 0.5,
    };
    let sc = score_categories(
        std::slice::from_ref(&ev),
        BetaParams::UNIFORM_PRIOR,
        DEFAULT_CONFIDENCE,
        &[],
    );
    let row = sc.categories.get("only").unwrap();
    assert_eq!(row.alpha, 1.5);
    assert_eq!(row.beta, 1.5);
    assert_eq!(row.raw_rate, 0.5);
    assert_eq!(row.point, truncate_score(0.5));
    assert!(row.lower < 0.5);
    assert!(row.upper > 0.5);
}
