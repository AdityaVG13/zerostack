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

#[test]
fn adaptive_sampling_concentrates_on_frontier_tasks() {
    use zero_gauge::adaptive_eval::{AdaptiveEvalConfig, TaskHistory, select_tasks};

    let histories = vec![
        TaskHistory {
            id: "easy".into(),
            successes: 10,
            trials: 10,
        },
        TaskHistory {
            id: "frontier".into(),
            successes: 5,
            trials: 10,
        },
        TaskHistory {
            id: "hard".into(),
            successes: 0,
            trials: 10,
        },
    ];
    let selection = select_tasks(&histories, 1, 7, AdaptiveEvalConfig::default()).unwrap();
    assert!(selection.inclusion_probabilities[1] > selection.inclusion_probabilities[0]);
    assert!(selection.inclusion_probabilities[1] > selection.inclusion_probabilities[2]);
}

#[test]
fn adaptive_estimators_preserve_constant_outcomes() {
    use zero_gauge::adaptive_eval::{anchored_difference_estimate, hajek_estimate};

    assert!((hajek_estimate(&[(0.5, 0.2), (0.5, 0.8)]).unwrap() - 0.5).abs() < 1e-9);
    let estimate = anchored_difference_estimate(&[0.5, 0.5], &[(0, 0.5, 0.5)]).unwrap();
    assert!((estimate - 0.5).abs() < 1e-9);
}

#[test]
fn savings_report_from_pair_is_self_verifying_and_tamper_evident() {
    use zero_gauge::observation::{
        MachineFingerprint, MeasuredUsage, Observation, ObservationKind, TaskIdentity,
    };
    use zero_gauge::pair::PairedObservations;
    use zero_gauge::report::{ReportError, SavingsReport};

    let task = TaskIdentity {
        task_id: "report-validation".into(),
        corpus_sha: None,
    };
    let machine = MachineFingerprint {
        os: "test-os".into(),
        arch: "test-arch".into(),
        cpu_model: "test-cpu".into(),
        kernel: "test-kernel".into(),
        rustc_version: "test-rustc".into(),
        git_sha: "a".repeat(40),
        cargo_profile: "test".into(),
    };
    let pair = PairedObservations::new(
        Observation {
            task: task.clone(),
            machine: machine.clone(),
            kind: ObservationKind::NativeBaseline,
            usage: MeasuredUsage::new(100, 200, 4),
        },
        Observation {
            task,
            machine,
            kind: ObservationKind::ZeroDirect,
            usage: MeasuredUsage::new(60, 150, 3),
        },
    )
    .unwrap();
    let report = SavingsReport::from_pair(&pair).unwrap();
    report.validate().unwrap();

    let mut forged = report;
    forged.tokens.numerator += 1;
    assert!(matches!(
        forged.validate(),
        Err(ReportError::InconsistentUnit { .. }) | Err(ReportError::ProvenanceMismatch { .. })
    ));
}
