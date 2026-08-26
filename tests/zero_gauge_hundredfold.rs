use zero_gauge::hundredfold::*;

#[test]
fn hundredfold_boundary_and_coverage_are_exact() {
    assert_eq!(multiplier_ppm(1_000, 10).unwrap(), 100_000_000);
    assert_eq!(
        required_prepared_coverage_ppm(1_000, 0, 1, 1_000, 100).unwrap(),
        990_991
    );
}

#[test]
fn sliding_window_exposes_local_collapse() {
    let baseline = [1_000, 1_000, 1_000];
    let optimized = [10, 20, 10];
    assert_eq!(
        minimum_window_multiplier_ppm(&baseline, &optimized, 1).unwrap(),
        50_000_000
    );
}
