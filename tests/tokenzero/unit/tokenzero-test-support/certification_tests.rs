use super::*;
use crate::invariant_catalog::InvariantCatalog;
use crate::parity_taxonomy::FeatureUniverse;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("TokenZero repo root")
}

#[test]
fn conformal_lower_one_is_unreachable_on_finite_samples() {
    assert!(CONFORMAL_LOWER_ONE_UNREACHABLE);
    let u = FeatureUniverse::load_embedded().expect("embedded matrix");
    let sc = u.conformal_scorecard();
    assert!(sc.global_lower < 1.0);
    assert!(!u.conformal_release_eligible(1.0));
}

#[test]
fn catalog_verification_reaches_one_hundred_when_pattern_65_is_satisfied() {
    let root = repo_root();
    let mut catalog = InvariantCatalog::tokenzero_phase7();
    crate::invariant_catalog::seal_satisfied_hashes(&mut catalog, &root);
    assert_eq!(
        catalog_verification_pct(&catalog),
        CERTIFICATION_MIN_VERIFICATION_PCT
    );
}

#[test]
fn certification_ready_when_catalog_and_suite_are_complete() {
    let root = repo_root();
    let mut catalog = InvariantCatalog::tokenzero_phase7();
    let universe = FeatureUniverse::load_embedded().expect("embedded");
    let assessment = assess_certification(
        &mut catalog,
        &universe,
        &root,
        CERTIFICATION_REQUIRED_SUITE_PASS_RATE_PCT,
        0,
    );
    assert_eq!(assessment.conformal_used_as, "ratchet_high_water");
    assert!(assessment.conformal_lower < 1.0);
    assert!(
        assessment.is_ready(),
        "catalog Pass + 100% verification must Ready even when conformal lower < 1.0: {:?}",
        assessment.verdict
    );
}

#[test]
fn certification_holds_when_suite_is_not_one_hundred() {
    let root = repo_root();
    let mut catalog = InvariantCatalog::tokenzero_phase7();
    let universe = FeatureUniverse::load_embedded().expect("embedded");
    let assessment = assess_certification(&mut catalog, &universe, &root, 99.0, 0);
    assert!(!assessment.is_ready());
}
