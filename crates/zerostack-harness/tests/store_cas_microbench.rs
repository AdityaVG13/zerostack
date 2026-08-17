//! Focused store CAS / atomic_write bench-shaped test.
//!
//! Setup and teardown stay outside the timed window. Emits JSON v3-shaped
//! fields. Not a keep: this runs under the test profile, not release-perf.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use zero_store::{SharedCas, atomic_write_file};
use zerostack_harness::hot_path_profile_snapshot::HotPathProfileSnapshot;
use zerostack_harness::measure::{Measurement, measure_with_teardown};
use zerostack_harness::repo::repo_root;

fn scratch_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("zerostack-{label}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&path).expect("scratch dir");
    path
}

fn report(
    scenario_id: &str,
    category: &str,
    measurement: &Measurement,
    counters: &HotPathProfileSnapshot,
) -> Value {
    json!({
        "schema_version": "zerostack.comprehensive-bench-report.v3",
        "detected_environment": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "cpu_count": std::thread::available_parallelism().map(|n| n.get()).ok(),
            "cargo_profile": if cfg!(debug_assertions) { "debug" } else { "release-or-custom" },
            "keep_eligible": false,
            "note": "Bench-shaped test, not a keep. Do not compare debug numbers to release-perf."
        },
        "summary": {
            "total_scenarios": 1,
            "primary_score": measurement.median_ms,
            "primary_score_name": "median_ms",
            "primary_score_direction": "lower_is_better",
            "geomean_ratio": null,
            "p90_ratio": null,
            "throughput": null,
            "per_category_weighted": {
                "score": measurement.median_ms,
                "weights": { category: 1.0 }
            }
        },
        "ci_regression_gate": {
            "schema_version": "zerostack.comprehensive-bench-ci-regression-gate.v2",
            "primary_score_max_regression_pct": 0.03,
            "geomean_max_regression_pct": 0.05,
            "category_geomean_max_regression_pct": 0.10,
            "p90_max_regression_pct": 0.15,
            "throughput_max_regression_pct": 0.05
        },
        "cv_pct": measurement.cv_pct,
        "sections": [{
            "section_id": scenario_id,
            "title": measurement.label,
            "rows": [{
                "scenario_id": scenario_id,
                "scenario": measurement.label,
                "category": category,
                "subject": {
                    "median_ms": measurement.median_ms,
                    "p95_ms": measurement.p95_ms,
                    "p99_ms": measurement.p99_ms,
                    "cv_pct": measurement.cv_pct,
                    "iterations": measurement.iterations
                }
            }]
        }],
        "hot_path_counters": counters,
    })
}

fn assert_v3_shape(value: &Value) {
    assert_eq!(
        value["schema_version"],
        "zerostack.comprehensive-bench-report.v3"
    );
    assert!(value["detected_environment"].is_object());
    assert!(value["summary"]["per_category_weighted"]["score"].is_number());
    assert!(value["ci_regression_gate"].is_object());
    assert!(value["sections"].as_array().is_some_and(|s| !s.is_empty()));
}

#[test]
fn cas_put_get_emits_json_v3() {
    HotPathProfileSnapshot::reset_for_test();
    let root = scratch_dir("cas-bench");
    let payload = b"zerostack-cas-microbench-v1";
    let before = HotPathProfileSnapshot::snapshot();
    let measurement = measure_with_teardown(
        "cas_put_get",
        || {
            let cas = SharedCas::open(&root);
            HotPathProfileSnapshot::record_cas_write();
            let hash = cas.put(payload).expect("cas put");
            HotPathProfileSnapshot::record_cas_read();
            let got = cas.get_verified(&hash).expect("cas get");
            assert_eq!(got, payload);
        },
        || {
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).expect("recreate store");
        },
    );
    let counters = before.diff(&HotPathProfileSnapshot::snapshot());
    let value = report("cas_put_get", "WriteSingle", &measurement, &counters);
    assert_v3_shape(&value);
    assert!(counters.cas_write >= 3);
    assert!(counters.cas_read >= 3);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn atomic_write_emits_json_v3() {
    HotPathProfileSnapshot::reset_for_test();
    let root = scratch_dir("atomic-write-bench");
    let dest = root.join("artifact.bin");
    let payload = b"zerostack-atomic-write-microbench-v1";
    let before = HotPathProfileSnapshot::snapshot();
    let measurement = measure_with_teardown(
        "atomic_write_file",
        || {
            HotPathProfileSnapshot::record_atomic_write_fsync();
            HotPathProfileSnapshot::record_atomic_write_rename();
            atomic_write_file(&dest, payload).expect("atomic_write_file");
            let got = fs::read(&dest).expect("read dest");
            assert_eq!(got, payload);
        },
        || {
            let _ = fs::remove_file(&dest);
        },
    );
    let counters = before.diff(&HotPathProfileSnapshot::snapshot());
    let value = report("atomic_write_file", "WriteSingle", &measurement, &counters);
    assert_v3_shape(&value);
    assert!(counters.atomic_write_fsync >= 3);
    assert!(counters.atomic_write_rename >= 3);
    assert!(
        repo_root()
            .join(".bench-history/savings-bench.latest.json")
            .is_file(),
        "committed savings-bench ratchet missing"
    );
    let _ = fs::remove_dir_all(&root);
}
