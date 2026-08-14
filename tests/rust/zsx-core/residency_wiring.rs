//! V6-R4 integration: the zero-gate Q99/residency runtime wired into the
//! session execution path (ZS-CACHE-001/003/004/010).
//!
//! Proves, over a real fixture session with real dispatches:
//! - per-tier window observations from measured (Exact) accounting,
//! - the measured demanded-object closure (verified blob byte mass),
//! - layer-validity publications for every verified CAS read,
//! - the Q99 report available on the session with labeled denominators,
//! - the gate refusal path (central change over 1% => unavailable),
//! - the live report-rejection path (missing demand weight fails closed).

#![cfg(feature = "fixture-adapters")]

use zero_abi::raw_worker::EngineIdentity;
use zsx_core::fixture::{fixture_adapters, publish_fixture_blob};
use zsx_core::ZsxSession;

fn fixture_session(session_id: &str) -> (tempfile::TempDir, ZsxSession) {
    let root = tempfile::tempdir().expect("root");
    let root_path = root.path().canonicalize().expect("canonical root");
    let (fs, graph, token) = fixture_adapters(&root_path, session_id);
    let session = ZsxSession::builder(&root_path)
        .with_session_id(session_id)
        .fszero(fs)
        .graphzero(graph)
        .tokenzero(token)
        .build()
        .expect("fixture session");
    (root, session)
}

#[test]
fn real_dispatches_produce_per_tier_q99_report_and_demand_ledger() {
    let (root, session) = fixture_session("r4-wiring");
    let fz_ref = publish_fixture_blob(root.path(), EngineIdentity::FsZero, b"fs-payload");
    let gz_ref = publish_fixture_blob(root.path(), EngineIdentity::GraphZero, b"graph-payload");
    // The token dispatch contributes its L3 window observation through
    // measured accounting; token.* argument contracts are strict, so no tz
    // ref can ride along in fixture mode (the tz demand closure is covered
    // by the real TokenZero adapter's ref emission).
    let source = format!(
        r#"
        await zero.fs.compound("read", {{__reachability_ref_fixture:"{fz_ref}"}});
        await zero.graph.query({{__reachability_ref_fixture:"{gz_ref}"}});
        await zero.token.shell("fixture", {{}});
        return "ok";
        "#
    );
    let result = session
        .execute(1, 1, source, std::time::Duration::from_secs(10))
        .expect("execution");
    assert_eq!(result.value, serde_json::json!("ok"));

    let report = session.q99_report().expect("q99 report");
    assert_eq!(report.schema, "zerostack.session_q99_report.v1");
    assert_eq!(report.window_id, "g1-r1");

    // Per-tier windows: one dispatch per engine, fixture accounting is Exact
    // with raw=8, cached=2 => demanded 8, hit 2, recompute 6/8 = 750000ppm.
    assert_eq!(report.tiers.len(), 3);
    for tier in &report.tiers {
        assert_eq!(tier.window.demanded_mass, 8);
        assert_eq!(tier.window.hit_mass, 2);
        assert_eq!(tier.window.recompute_ppm, 750_000);
        assert_eq!(tier.denominator_label, "q99_demanded_mass:8");
        // Gate refusal path observable: central change over 1% of demanded
        // mass is reported as unavailable, never averaged.
        assert!(tier.window.unavailable, "{} window must be unavailable", tier.tier.as_str());
        assert!(tier
            .window
            .reasons
            .iter()
            .any(|reason| reason.starts_with("central_change_exceeds_one_percent")));
    }
    assert!(report.unavailable);
    assert!(report
        .reasons
        .iter()
        .any(|reason| reason.starts_with("l1:central_change_exceeds_one_percent")));

    // Measured demand closure: two verified objects (fs, graph) with their
    // byte mass; the token dispatch carried no ref in fixture mode. Ledger
    // entries are digest-sorted, so compare the weight set.
    assert_eq!(report.demand_ledger.objects.len(), 2);
    let mut weights: Vec<u64> = report
        .demand_ledger
        .objects
        .iter()
        .map(|object| object.demand_weight)
        .collect();
    weights.sort_unstable();
    assert_eq!(weights, vec![10, 13]);
    assert!(report
        .demand_ledger
        .objects
        .iter()
        .all(|object| object.window_id == "g1-r1"));

    // Layer validity published for every verified CAS read.
    assert_eq!(report.layer_valid_entries, 2);
    // Masses: 3 * (8 demanded, 2 hit). Slack = ppm(6,24) - floor where the
    // floor is 99% of the integer demanded mass: 23_000_000/24 = 958333ppm.
    assert_eq!(report.resident_mass, 6);
    assert_eq!(report.demanded_mass, 24);
    assert_eq!(report.eviction_floor_mass, 23);
    assert_eq!(report.eviction_slack_ppm, Some(-708_333));
    session.shutdown().expect("shutdown");
}

#[test]
fn q99_report_is_available_immediately_after_build_with_no_vacuous_pass() {
    let (_root, session) = fixture_session("r4-prewarm");
    let report = session.q99_report().expect("prewarm report exists");
    // The prewarm window has no demand: every tier reports unavailable, never
    // a vacuous pass. W4's empty window reports BOTH the missing evidence and
    // the maximal recompute fraction (u64::MAX ppm) as reasons.
    assert!(report.unavailable);
    for reason in &report.reasons {
        assert!(
            reason.ends_with("no_demand_observations")
                || reason.starts_with("l1:central_change_exceeds_one_percent")
                || reason.starts_with("l2:central_change_exceeds_one_percent")
                || reason.starts_with("l3:central_change_exceeds_one_percent"),
            "unexpected reason: {reason}"
        );
    }
    assert!(report
        .reasons
        .iter()
        .any(|reason| reason.ends_with("no_demand_observations")));
    assert_eq!(report.demand_ledger.objects.len(), 0);
    assert_eq!(report.demanded_mass, 0);
    session.shutdown().expect("shutdown");
}

#[test]
fn report_rejection_path_live_when_demand_weight_is_missing() {
    let (root, session) = fixture_session("r4-rejection");
    // A zero-byte object is verified resident but carries no representable
    // nonzero demand weight: the report must reject (missing weights fail
    // closed) instead of silently omitting the object.
    let fz_ref = publish_fixture_blob(root.path(), EngineIdentity::FsZero, b"");
    let source = format!(
        r#"
        await zero.fs.compound("read", {{__reachability_ref_fixture:"{fz_ref}"}});
        return "ok";
        "#
    );
    let result = session
        .execute(1, 1, source, std::time::Duration::from_secs(10))
        .expect("execution with a zero-byte demand still succeeds");
    assert_eq!(result.value, serde_json::json!("ok"));
    let error = session.q99_report().expect_err("report must be rejected");
    assert!(
        error.to_string().contains("demand weight absent"),
        "unexpected rejection detail: {error}"
    );
    session.shutdown().expect("shutdown");
}
