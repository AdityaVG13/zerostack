use super::*;

fn iv(lo: u64, hi: u64) -> Interval {
    Interval::new(lo, hi).unwrap()
}

fn coord(name: &str, baseline: Interval, hit: Interval, fallback: Interval, prep: Interval, target: Interval) -> ResourceCoordinate {
    ResourceCoordinate::new(name, baseline, hit, fallback, prep, target).unwrap()
}

fn grid(den: u128) -> impl Iterator<Item = Rational> {
    (0..=den).map(move |k| Rational::new(k as i128, den).unwrap())
}

/// A typical token coordinate: fallback cheaper than baseline, hit cheapest.
fn token_coordinate() -> ResourceCoordinate {
    coord(
        "tokens",
        iv(100, 120),  // baseline
        iv(10, 20),    // hit
        iv(50, 70),    // fallback
        iv(2, 5),      // preparation
        iv(40, 45),    // target
    )
}

#[test]
fn rational_reduction_signs_and_comparisons() {
    assert_eq!(Rational::new(4, 8).unwrap(), Rational::new(1, 2).unwrap());
    assert_eq!(Rational::new(-4, 8).unwrap(), Rational::new(-1, 2).unwrap());
    assert_eq!(Rational::ZERO, Rational::new(0, 7).unwrap());
    assert_eq!(Rational::ONE, Rational::new(9, 9).unwrap());
    assert!(Rational::ZERO.is_zero());
    assert!(Rational::ONE.is_one());
    assert!(Rational::new(-1, 3).unwrap() < Rational::ZERO);
    assert!(Rational::ZERO < Rational::new(1, 3).unwrap());
    assert!(Rational::new(-2, 3).unwrap() < Rational::new(-1, 3).unwrap());
    assert!(Rational::new(1, 3).unwrap() < Rational::new(1, 2).unwrap());
    assert_eq!(
        Rational::new(1, 3).unwrap().compare(Rational::new(2, 6).unwrap()),
        std::cmp::Ordering::Equal
    );
    assert_eq!(Rational::new(3, 1).unwrap().max(Rational::new(5, 2).unwrap()), Rational::new(3, 1).unwrap());
    assert_eq!(Rational::new(3, 1).unwrap().min(Rational::new(5, 2).unwrap()), Rational::new(5, 2).unwrap());
    // Zero denominator is a loud refusal.
    assert_eq!(Rational::new(1, 0).unwrap_err(), SolverError::ZeroDenominator);
}

#[test]
fn certified_interval_matches_exhaustive_grid() {
    let coordinate = token_coordinate();
    let analytic = certified_feasible_interval(&coordinate).expect("feasible");
    for h in grid(1024) {
        assert_eq!(
            analytic.contains(h),
            certified_holds(&coordinate, h),
            "analytic and direct box check disagree at h = {h}"
        );
    }
    // The certified interval is the worst-case condition: it must be a
    // subset of the exists (best-case) interval.
    let exists = exists_feasible_interval(&coordinate).expect("exists");
    for h in grid(1024) {
        assert!(!analytic.contains(h) || exists.contains(h));
        assert_eq!(exists.contains(h), exists_holds(&coordinate, h));
    }
}

#[test]
fn solver_is_deterministic() {
    let coordinates = vec![token_coordinate(), coord(
        "cpu_ns",
        iv(1000, 1200),
        iv(100, 200),
        iv(500, 700),
        iv(20, 50),
        iv(400, 450),
    )];
    let first = feasible_intersection(&coordinates).unwrap();
    let second = feasible_intersection(&coordinates).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap()
    );
}

#[test]
fn intersection_narrows_across_coordinates() {
    let coordinates = vec![token_coordinate(), coord(
        "cpu_ns",
        iv(100, 120),
        iv(10, 20),
        iv(50, 70),
        iv(2, 5),
        iv(40, 45),
    )];
    let intersection = feasible_intersection(&coordinates).unwrap();
    let single = feasible_intersection(&[token_coordinate()]).unwrap();
    assert!(intersection.min >= single.min);
    assert!(intersection.max <= single.max);
    // The intersection contains exactly the hit rates certified by every
    // coordinate: exhaustive membership check.
    for h in grid(1024) {
        let all_hold = coordinates.iter().all(|c| certified_holds(c, h));
        assert_eq!(intersection.contains(h), all_hold, "at h = {h}");
    }
}

#[test]
fn blocker_reports_the_coordinate_that_makes_intersection_empty() {
    // tokens and cpu_ns each require h >= 1/2 (decreasing cost, target 30);
    // storage is individually infeasible at every hit rate (its preparation
    // already exceeds its target), so storage is the blocker and the
    // running intersection before it is [1/2, 1].
    let coordinates = vec![
        coord("tokens", iv(100, 100), iv(10, 10), iv(50, 50), iv(0, 0), iv(30, 30)),
        coord("cpu_ns", iv(100, 100), iv(10, 10), iv(50, 50), iv(0, 0), iv(30, 30)),
        coord(
            "storage",
            iv(1000, 1000),
            iv(100, 100),
            iv(500, 500),
            iv(600, 600), // preparation alone exceeds the target of 300
            iv(300, 300),
        ),
    ];
    let err = feasible_intersection(&coordinates).unwrap_err();
    match err {
        SolverError::EmptyIntersection { blocker } => {
            assert_eq!(blocker.coordinate, "storage");
            assert_eq!(blocker.feasible, None);
            // The running intersection before storage was [1/2, 1].
            assert_eq!(blocker.intersection_before.min, Rational::new(1, 2).unwrap());
            assert_eq!(blocker.intersection_before.max, Rational::ONE);
        }
        other => panic!("expected EmptyIntersection, got {other:?}"),
    }
}

#[test]
fn individually_empty_coordinate_is_a_loud_blocker() {
    // Preparation alone (10) already exceeds the target (5): no hit rate can
    // meet the target, so the solver refuses instead of returning a
    // compromise.
    let coordinate = coord(
        "tokens",
        iv(100, 100),
        iv(10, 10),
        iv(50, 50),
        iv(10, 10),
        iv(5, 5),
    );
    assert!(certified_feasible_interval(&coordinate).is_none());
    let err = feasible_intersection(&[coordinate]).unwrap_err();
    match err {
        SolverError::EmptyIntersection { blocker } => {
            assert_eq!(blocker.coordinate, "tokens");
        }
        other => panic!("expected EmptyIntersection, got {other:?}"),
    }
}

#[test]
fn no_guessed_split_when_infeasible() {
    // The certified intersection is empty; the solver must refuse, never
    // return a midpoint or averaged hit rate.
    let coordinates = vec![
        coord("tokens", iv(100, 100), iv(10, 10), iv(50, 50), iv(0, 0), iv(30, 30)),
        coord(
            "cpu_ns",
            iv(100, 100),
            iv(10, 10),
            iv(50, 50),
            iv(40, 40), // preparation alone exceeds the target of 30
            iv(30, 30),
        ),
    ];
    assert!(matches!(
        feasible_intersection(&coordinates),
        Err(SolverError::EmptyIntersection { .. })
    ));
}

#[test]
fn inverted_interval_is_refused() {
    assert_eq!(
        Interval::new(10, 5).unwrap_err(),
        SolverError::InvertedInterval {
            field: "interval".into(),
            lo: 10,
            hi: 5,
        }
    );
    // Wire decode of an inverted interval is refused too.
    let wire = r#"{"lo":10,"hi":5}"#;
    assert!(serde_json::from_str::<Interval>(wire).is_err());
}

#[test]
fn path_order_violations_are_refused() {
    // Hit above fallback.
    let err = ResourceCoordinate::new(
        "tokens",
        iv(100, 120),
        iv(60, 80),
        iv(50, 70),
        iv(2, 5),
        iv(40, 45),
    )
    .unwrap_err();
    assert_eq!(
        err,
        SolverError::InconsistentModel {
            coordinate: "tokens".into(),
            relation: "hit above fallback",
        }
    );
    // Fallback above baseline.
    let err = ResourceCoordinate::new(
        "tokens",
        iv(40, 60),
        iv(10, 20),
        iv(50, 70),
        iv(2, 5),
        iv(40, 45),
    )
    .unwrap_err();
    assert_eq!(
        err,
        SolverError::InconsistentModel {
            coordinate: "tokens".into(),
            relation: "fallback above baseline",
        }
    );
    // Empty name and empty coordinate set.
    assert_eq!(
        ResourceCoordinate::new("", iv(1, 2), iv(1, 2), iv(1, 2), iv(1, 2), iv(1, 2)).unwrap_err(),
        SolverError::EmptyCoordinateName
    );
    assert_eq!(
        feasible_intersection(&[]).unwrap_err(),
        SolverError::NoCoordinates
    );
}

#[test]
fn slope_sign_boundaries_are_exact() {
    // Constant cost (hit == fallback): feasibility depends only on
    // preparation + fallback vs target.
    let coordinate = coord("tokens", iv(100, 100), iv(50, 50), iv(50, 50), iv(10, 10), iv(60, 60));
    let interval = certified_feasible_interval(&coordinate).unwrap();
    assert_eq!(interval, FeasibleInterval { min: Rational::ZERO, max: Rational::ONE });
    let coordinate = coord("tokens", iv(100, 100), iv(50, 50), iv(50, 50), iv(10, 10), iv(59, 59));
    assert!(certified_feasible_interval(&coordinate).is_none());

    // Decreasing cost (hit < fallback): feasible only above a threshold.
    let coordinate = coord("tokens", iv(100, 100), iv(10, 10), iv(50, 50), iv(0, 0), iv(30, 30));
    let interval = certified_feasible_interval(&coordinate).unwrap();
    assert_eq!(interval.min, Rational::new(1, 2).unwrap());
    assert_eq!(interval.max, Rational::ONE);
    // Boundary: h = 1/2 exactly meets the target (cost 30 <= 30).
    assert!(certified_holds(&coordinate, Rational::new(1, 2).unwrap()));
    assert!(!certified_holds(&coordinate, Rational::new(499, 1000).unwrap()));
}

#[test]
fn coordinate_wire_decode_refuses_inconsistent_data() {
    let wire = r#"{"name":"tokens","baseline":{"lo":100,"hi":120},"hit":{"lo":60,"hi":80},"fallback":{"lo":50,"hi":70},"preparation":{"lo":2,"hi":5},"target":{"lo":40,"hi":45}}"#;
    assert!(serde_json::from_str::<ResourceCoordinate>(wire).is_err());
    let wire = r#"{"name":"tokens","baseline":{"lo":120,"hi":100},"hit":{"lo":10,"hi":20},"fallback":{"lo":50,"hi":70},"preparation":{"lo":2,"hi":5},"target":{"lo":40,"hi":45}}"#;
    assert!(serde_json::from_str::<ResourceCoordinate>(wire).is_err());
    let wire = r#"{"name":"tokens","baseline":{"lo":100,"hi":120},"hit":{"lo":10,"hi":20},"fallback":{"lo":50,"hi":70},"preparation":{"lo":2,"hi":5},"target":{"lo":40,"hi":45}}"#;
    let decoded: ResourceCoordinate = serde_json::from_str(wire).unwrap();
    assert_eq!(decoded, token_coordinate());
}
