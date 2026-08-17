//! Phase 3 smoke: identity, both-error, artifact_id, 5-mode dispatch, preflight.

use zerostack_harness::differential_v2::ExecutionEnvelope;
use zerostack_harness::engine_identity::{
    EngineIdentity, SUBJECT_IDENTITY_LABEL, assert_subject_ne_oracle,
};
use zerostack_harness::oracle::{
    ExternalTool, OracleMode, ScenarioError, SubjectOutput, SubjectState, compare, scenario,
};
use zerostack_harness::oracle_preflight_doctor;
use zerostack_harness::repo::repo_root;
use zerostack_harness::spec_oracle::{self, all_verifiers};

fn ok_output() -> SubjectOutput {
    SubjectOutput {
        canonical: "ok".into(),
        kind: "marker",
    }
}

#[test]
fn subject_equals_subject() {
    let left = EngineIdentity::subject();
    let right = EngineIdentity::subject();
    assert_eq!(left, right);
    assert_ne!(left, EngineIdentity::oracle("spec-v1"));
    assert_eq!(left.label(), SUBJECT_IDENTITY_LABEL);
}

#[test]
fn subject_ne_oracle_for_every_mode() {
    let modes = [
        OracleMode::Spec {
            tag: "SPEC-RES-001",
        },
        OracleMode::Property {
            name: "fresh_work_sum",
        },
        OracleMode::Self_ {
            commit_sha: "abc123".into(),
        },
        OracleMode::RoundTrip { pair: "zeroref-v1" },
        OracleMode::ExternalTool(ExternalTool::Miri),
        OracleMode::ExternalTool(ExternalTool::Clippy),
    ];
    for mode in &modes {
        let oracle = mode.identity_label();
        assert_ne!(SUBJECT_IDENTITY_LABEL, oracle.as_str());
        assert_subject_ne_oracle(SUBJECT_IDENTITY_LABEL, &oracle);
    }
}

#[test]
#[should_panic(expected = "EngineIdentity collision")]
fn comparator_rejects_subject_eq_oracle() {
    assert_subject_ne_oracle("zerostack", "zerostack");
}

#[test]
fn both_error_is_agreement() {
    compare(
        "both-error",
        Err(ScenarioError::new("io", "left")),
        Err(ScenarioError::new("spec", "right")),
    );
}

#[test]
#[should_panic(expected = "one-error-one-OK")]
fn one_error_one_ok_is_hard_failure() {
    compare(
        "mixed",
        Ok(ok_output()),
        Err(ScenarioError::new("spec", "no")),
    );
}

#[test]
fn artifact_id_stable_across_run_id() {
    let oracle = EngineIdentity::oracle("spec-v1");
    let first = ExecutionEnvelope::new("phase3-smoke", &oracle).with_run_id("run-1");
    let second = ExecutionEnvelope::new("phase3-smoke", &oracle).with_run_id("run-2");
    assert_eq!(first.artifact_id(), second.artifact_id());
    assert_eq!(first.artifact_id().len(), 64);
}

#[test]
fn five_modes_dispatch() {
    let root = repo_root();
    scenario(
        || SubjectState {
            seed: 1,
            label: "spec".into(),
        },
        |_| Ok(ok_output()),
        |_| spec_oracle::run_tag("SPEC-RES-001", &root),
        OracleMode::Spec {
            tag: "SPEC-RES-001",
        },
        "spec-mode",
    );
    scenario(
        SubjectState::default,
        |_| Ok(ok_output()),
        |_| zerostack_harness::property_oracle::run_fresh_work_sum_property(),
        OracleMode::Property {
            name: "fresh_work_sum",
        },
        "property-mode",
    );
    scenario(
        SubjectState::default,
        |_| Ok(ok_output()),
        |out| match out {
            Ok(output) => zerostack_harness::self_oracle::assert_matches_blessed(output, "ok"),
            Err(error) => Err(error.clone()),
        },
        OracleMode::Self_ {
            commit_sha: "deadbeef".into(),
        },
        "self-mode",
    );
    scenario(
        SubjectState::default,
        |_| Ok(ok_output()),
        |_| zerostack_harness::roundtrip_oracle::smoke_roundtrip(),
        OracleMode::RoundTrip { pair: "zeroref-v1" },
        "roundtrip-mode",
    );
}

#[test]
fn spec_verifiers_hold_on_this_tree() {
    spec_oracle::run_all(&repo_root()).expect("wired spec verifiers must pass");
    assert!(all_verifiers().len() >= 40);
}

#[test]
fn preflight_does_not_panic_and_certifying_matches_aggregate() {
    let report = oracle_preflight_doctor::run(&repo_root());
    assert_eq!(report.schema_version, "oracle-preflight-doctor.v1");
    assert!(
        report.aggregate_outcome != "red" || report.first_failure_diagnosis.is_some(),
        "red report must name a failure"
    );
    assert_eq!(
        report.certifying,
        report.aggregate_outcome == "green",
        "certifying is true only when green: {}",
        report.to_json()
    );
}
