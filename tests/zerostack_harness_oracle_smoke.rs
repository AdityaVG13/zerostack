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
use zerostack_harness::spec_oracle::{self};
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
    // Independent oracle: artifact_id is SHA-256 of canonical JSON excluding run_id.
    // Must be stable across run_id but sensitive to every artifact-defining input.
    let oracle = EngineIdentity::oracle("spec-v1");
    let base = ExecutionEnvelope::new("phase3-smoke", &oracle);
    let with_run_a = base.clone().with_run_id("run-1");
    let with_run_b = base.clone().with_run_id("run-2");
    let id_base = base.artifact_id();
    let id_a = with_run_a.artifact_id();
    let id_b = with_run_b.artifact_id();
    // Stability across run_id.
    assert_eq!(id_base, id_a, "run_id must not affect artifact_id");
    assert_eq!(id_a, id_b, "different run_ids must yield same artifact_id");
    // Not vacuous: different defining inputs must change the id.
    let other_scenario = ExecutionEnvelope::new("different-scenario", &oracle).artifact_id();
    assert_ne!(
        id_base, other_scenario,
        "scenario_id must affect artifact_id"
    );
    let other_oracle = EngineIdentity::oracle("property-suite-v1").clone();
    let other_engine = ExecutionEnvelope::new("phase3-smoke", &other_oracle).artifact_id();
    assert_ne!(
        id_base, other_engine,
        "oracle identity must affect artifact_id"
    );
    let mut with_seed = base.clone();
    with_seed.seed = 42;
    assert_ne!(
        id_base,
        with_seed.artifact_id(),
        "seed must affect artifact_id"
    );
    // Shape is SHA-256 hex, not an arbitrary literal length snapshot.
    assert_eq!(id_base.len(), 64, "artifact_id is SHA-256 hex");
    assert!(
        id_base
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()),
        "artifact_id must be lowercase hex: {id_base}"
    );
}

#[test]
fn five_modes_dispatch() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    fn assert_propagates(mode: OracleMode, label: &str) {
        let sentinel = format!("sentinel-{label}");
        let sentinel_clone = sentinel.clone();
        let invoked = Arc::new(AtomicUsize::new(0));
        let invoked_clone = Arc::clone(&invoked);
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            scenario(
                SubjectState::default,
                |_| Ok(ok_output()),
                move |_| {
                    invoked_clone.fetch_add(1, Ordering::SeqCst);
                    Err(ScenarioError::new("oracle", sentinel_clone.clone()))
                },
                mode.clone(),
                label,
            );
        }));
        assert!(
            outcome.is_err(),
            "{label}: scenario must propagate oracle failure"
        );
        let payload = outcome.unwrap_err();
        let msg = if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else if let Some(s) = payload.downcast_ref::<&str>() {
            s.to_string()
        } else {
            format!("{payload:?}")
        };
        assert!(
            msg.contains(&sentinel),
            "{label}: panic must carry sentinel error, got: {msg}"
        );
        assert!(
            msg.contains("one-error-one-OK"),
            "{label}: panic must be via one-error-one-OK path, got: {msg}"
        );
        assert_eq!(
            invoked.load(Ordering::SeqCst),
            1,
            "{label}: oracle must be invoked exactly once"
        );

        // Success path: same mode with Ok must not panic and must invoke once.
        let ok_invoked = Arc::new(AtomicUsize::new(0));
        let ok_clone = Arc::clone(&ok_invoked);
        let ok_outcome = catch_unwind(AssertUnwindSafe(|| {
            scenario(
                SubjectState::default,
                |_| Ok(ok_output()),
                move |_| {
                    ok_clone.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
                mode,
                label,
            );
        }));
        assert!(
            ok_outcome.is_ok(),
            "{label}: scenario must succeed when oracle succeeds"
        );
        assert_eq!(
            ok_invoked.load(Ordering::SeqCst),
            1,
            "{label}: successful oracle must be invoked exactly once"
        );
    }

    // Every public OracleMode variant, including both ExternalTool identities.
    assert_propagates(
        OracleMode::Spec {
            tag: "SPEC-RES-001",
        },
        "spec-mode",
    );
    assert_propagates(
        OracleMode::Property {
            name: "fresh_work_sum",
        },
        "property-mode",
    );
    assert_propagates(
        OracleMode::Self_ {
            commit_sha: "deadbeef".into(),
        },
        "self-mode",
    );
    assert_propagates(
        OracleMode::RoundTrip { pair: "zeroref-v1" },
        "roundtrip-mode",
    );
    assert_propagates(
        OracleMode::ExternalTool(ExternalTool::Clippy),
        "clippy-mode",
    );
    assert_propagates(OracleMode::ExternalTool(ExternalTool::Miri), "miri-mode");
}

#[test]
fn spec_verifiers_hold_on_this_tree() {
    let root = repo_root();
    // Unverified tags must be honestly surfaced, not painted as passes.
    let unverified = spec_oracle::unverified_tags();
    let tags: Vec<&str> = unverified.iter().map(|row| row.tag).collect();
    for required in ["SPEC-SURF-002", "SPEC-HUB-005"] {
        assert!(
            tags.contains(&required),
            "{required} must remain UNVERIFIED in unverified_tags"
        );
        let err = spec_oracle::run_tag(required, &root).expect_err("unverified tag must fail");
        assert_eq!(err.class, "UNVERIFIED", "{required} must be UNVERIFIED");
        assert!(
            err.message.contains(required),
            "{required} diagnostic must name the tag"
        );
    }
    // Wired verifiers must succeed on this tree when run individually, without
    // asserting a global count or requiring the whole suite to pass in one call.
    for tag in ["SPEC-RES-001", "SPEC-RES-002"] {
        spec_oracle::run_tag(tag, &root)
            .unwrap_or_else(|e| panic!("{tag} should pass on this tree: {e}"));
    }
    // Unknown tags must not be silently accepted.
    let unknown = spec_oracle::run_tag("SPEC-DOES-NOT-EXIST", &root).expect_err("unknown tag");
    assert!(
        unknown.message.contains("no verifier wired"),
        "unknown tag must report missing verifier, got: {unknown}"
    );
}

#[test]
fn preflight_does_not_panic_and_certifying_matches_aggregate() {
    let report = oracle_preflight_doctor::run(&repo_root());
    assert_eq!(report.schema_version, "oracle-preflight-doctor.v1");
    assert_eq!(report.subject_kind, "in-process-zero-kernel");
    assert!(!report.require_installed_zsx_binary);
    assert!(!report.harness_is_workspace_member);
    assert!(
        report.aggregate_outcome != "red" || report.first_failure_diagnosis.is_some(),
        "red report must name a failure: {}",
        report.to_json()
    );
    assert_eq!(
        report.certifying,
        report.aggregate_outcome == "green",
        "certifying is true only when green: {}",
        report.to_json()
    );
    assert!(
        !report
            .checks
            .iter()
            .any(|check| check.name == "subject_binary_zsx" && check.outcome == "red"),
        "locate_zsx must not be a certifying red check: {}",
        report.to_json()
    );
}
