use super::*;

#[test]
fn terms_sum_to_optimized_and_closure_holds() {
    // baseline 200; optimized 100 = 20 (prep) + 30 (path) + 50 (novelty).
    let closure = FrontierClosure::new(200, 100, 20, 30, 50).unwrap();
    assert!(closure.closure_holds());
    assert_eq!(closure.optimized_ratio(), (1, 2));
    // Normalized terms: 1/10, 3/20, 1/4.
    assert_eq!(closure.normalized_term(FrontierTerm::Preparation), (1, 10));
    assert_eq!(closure.normalized_term(FrontierTerm::PreparedPath), (3, 20));
    assert_eq!(closure.normalized_term(FrontierTerm::NoveltyFallback), (1, 4));
    // 1/10 + 3/20 + 1/4 = 2/20 + 3/20 + 5/20 = 10/20 = 1/2: complete ratio.
    assert_eq!(closure.largest_limiting_burden().unwrap().term, FrontierTerm::NoveltyFallback);
}

#[test]
fn closure_mismatch_and_zero_baseline_are_refused() {
    // Terms do not sum to the optimized total.
    assert_eq!(
        FrontierClosure::new(200, 100, 20, 30, 40).unwrap_err(),
        FrontierError::TermSumMismatch {
            term_sum: 90,
            optimized: 100,
        }
    );
    // Zero baseline leaves no denominator.
    assert_eq!(
        FrontierClosure::new(0, 0, 0, 0, 0).unwrap_err(),
        FrontierError::ZeroBaselineTotal
    );
    // A tampered wire closure is refused on decode.
    let wire = r#"{"baseline_total":200,"optimized_total":100,"preparation":20,"prepared_path":30,"novelty_fallback":40}"#;
    assert!(serde_json::from_str::<FrontierClosure>(wire).is_err());
}

#[test]
fn terms_are_nonnegative_and_closure_survives_reduction() {
    // Shared divisors: reduction must not break the closure identity.
    let closure = FrontierClosure::new(100, 60, 10, 20, 30).unwrap();
    assert!(closure.closure_holds());
    assert_eq!(closure.normalized_term(FrontierTerm::Preparation), (1, 10));
    assert_eq!(closure.normalized_term(FrontierTerm::PreparedPath), (1, 5));
    assert_eq!(closure.normalized_term(FrontierTerm::NoveltyFallback), (3, 10));
    // 1/10 + 1/5 + 3/10 = 6/10 = 3/5 = optimized/baseline.
    assert_eq!(closure.optimized_ratio(), (3, 5));
    // u64 terms cannot be negative: nonnegativity is structural.
    let _ = FrontierClosure::new(u64::MAX, u64::MAX, u64::MAX, 0, 0).unwrap();
}

#[test]
fn largest_limiting_burden_reports_and_breaks_ties_canonically() {
    // PreparedPath is the largest normalized term.
    let closure = FrontierClosure::new(100, 80, 20, 40, 20).unwrap();
    let burden = closure.largest_limiting_burden().unwrap();
    assert_eq!(burden.term, FrontierTerm::PreparedPath);
    assert_eq!(burden.normalized, (2, 5));
    assert_eq!(burden.absolute, 40);

    // Ties break in canonical order (preparation < prepared-path <
    // novelty-fallback).
    let closure = FrontierClosure::new(100, 60, 20, 20, 20).unwrap();
    assert_eq!(closure.largest_limiting_burden().unwrap().term, FrontierTerm::Preparation);

    // All-zero terms: no burden to report.
    let closure = FrontierClosure::new(100, 0, 0, 0, 0).unwrap();
    assert_eq!(closure.largest_limiting_burden(), None);
}

#[test]
fn frontier_roundtrip_and_canonical_order() {
    let closure = FrontierClosure::new(200, 100, 20, 30, 50).unwrap();
    let json = serde_json::to_string(&closure).unwrap();
    assert_eq!(serde_json::from_str::<FrontierClosure>(&json).unwrap(), closure);
    for term in FrontierTerm::ALL {
        let json = serde_json::to_string(&term).unwrap();
        assert_eq!(serde_json::from_str::<FrontierTerm>(&json).unwrap(), term);
        assert_eq!(json, format!("\"{}\"", term.as_str()));
    }
}
