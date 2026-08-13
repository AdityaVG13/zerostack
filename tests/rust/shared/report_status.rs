use zerostack_shared_tests::{
    CHECK_IDS, CheckResult, CompletionStatus, ConformanceReport, Ns, RAW_CHECK_IDS, Surface,
};
fn plan_checks() -> Vec<CheckResult> {
    CHECK_IDS
        .iter()
        .map(|id| CheckResult::pass(id, id))
        .collect()
}
fn raw_checks() -> Vec<CheckResult> {
    RAW_CHECK_IDS
        .iter()
        .map(|id| CheckResult::pass(id, id))
        .collect()
}

#[test]
fn complete_codemode_raw_pass() {
    // Codemode surface requires the raw-worker RW1-RW10 set, NOT G1-G10.
    let r = ConformanceReport::new(Ns::Fz, "fake", Surface::Codemode, raw_checks());
    assert_eq!(r.completion_status, CompletionStatus::Complete);
    assert!(r.passed);
    assert_eq!(r.contract_version, "raw-worker-v2");
}

#[test]
fn complete_planner_pass() {
    // Planner surface requires the plan-level G1-G10 set.
    let r = ConformanceReport::new(Ns::Fz, "fake", Surface::Planner, plan_checks());
    assert_eq!(r.completion_status, CompletionStatus::Complete);
    assert!(r.passed);
    assert_eq!(r.contract_version, "1.0");
}

#[test]
fn codemode_plan_checks_cannot_false_green() {
    // A codemode report carrying G1-G10 (no RW gates) must NOT be complete:
    // the surface-specific scope is RW1-RW10, which is missing.
    let r = ConformanceReport::new(Ns::Fz, "fake", Surface::Codemode, plan_checks());
    assert_eq!(r.completion_status, CompletionStatus::Partial);
    assert!(!r.passed);
}

#[test]
fn failed_required_gate() {
    let mut c = raw_checks();
    c[1] = CheckResult::fail("RW2", "RW2", "bad ref");
    let r = ConformanceReport::new(Ns::Fz, "fake", Surface::Codemode, c);
    assert_eq!(r.completion_status, CompletionStatus::Failed);
    assert!(!r.passed);
}

#[test]
fn skipped_required_gate() {
    let mut c = raw_checks();
    c[1] = CheckResult::skip("RW2", "RW2", "worker did not initialize");
    let r = ConformanceReport::new(Ns::Fz, "fake", Surface::Codemode, c);
    assert_eq!(r.completion_status, CompletionStatus::Partial);
    assert!(!r.passed);
}

#[test]
fn non_required_skip_does_not_cap_mcp_exposure() {
    let c = vec![
        CheckResult::pass("G1", "exposure"),
        CheckResult::skip(
            "G2",
            "refs",
            "not applicable to MCP surface; requires planner execution",
        ),
    ];
    let r = ConformanceReport::new(Ns::Fz, "fake", Surface::Mcp, c);
    assert_eq!(r.completion_status, CompletionStatus::Complete);
    assert!(r.passed);
    assert_eq!(r.contract_version, "mcp-exposure-v1");
}

#[test]
fn mcp_g1_pass_with_explicit_plan_skips_is_complete() {
    let mut c = vec![CheckResult::pass("G1", "exposure")];
    c.extend(CHECK_IDS[1..].iter().map(|id| {
        CheckResult::skip(
            id,
            id,
            "not applicable to MCP surface; requires planner execution",
        )
    }));
    let r = ConformanceReport::new(Ns::Fz, "fake", Surface::Mcp, c);
    assert_eq!(r.completion_status, CompletionStatus::Complete);
    assert!(r.passed);
}

#[test]
fn mcp_missing_or_failed_exposure_is_not_complete() {
    let missing = ConformanceReport::new(Ns::Fz, "fake", Surface::Mcp, vec![]);
    assert_eq!(missing.completion_status, CompletionStatus::Partial);
    assert!(!missing.passed);

    let failed = ConformanceReport::new(
        Ns::Fz,
        "fake",
        Surface::Mcp,
        vec![CheckResult::fail("G1", "exposure", "missing")],
    );
    assert_eq!(failed.completion_status, CompletionStatus::Failed);
    assert!(!failed.passed);
}

#[test]
fn serde_exposes_statuses_and_legacy_passed() {
    let s = CheckResult::skip("G2", "refs", "stable reason");
    let r = ConformanceReport::new(
        Ns::Fz,
        "fake",
        Surface::Mcp,
        vec![CheckResult::pass("G1", "exposure"), s],
    );
    let v = serde_json::to_value(r).unwrap();
    assert_eq!(v["passed"], true);
    assert_eq!(v["completion_status"], "complete");
    assert_eq!(v["checks"][0]["status"], "pass");
    assert_eq!(v["checks"][1]["status"], "skipped");
    assert_eq!(v["checks"][1]["skip_reason"], "stable reason");
}
