use zerostack_codemode_conformance::{
    CheckResult, CompletionStatus, ConformanceReport, GateStatus, Ns, Surface, CHECK_IDS,
};
fn complete_checks() -> Vec<CheckResult> {
    CHECK_IDS
        .iter()
        .map(|id| CheckResult::pass(id, id))
        .collect()
}
#[test]
fn complete_codemode_pass() {
    let r = ConformanceReport::new(Ns::Fz, "fake", Surface::Codemode, complete_checks());
    assert_eq!(r.completion_status, CompletionStatus::Complete);
    assert!(r.passed);
}
#[test]
fn failed_required_gate() {
    let mut c = complete_checks();
    c[1] = CheckResult::fail("G2", "G2", "bad ref");
    let r = ConformanceReport::new(Ns::Fz, "fake", Surface::Codemode, c);
    assert_eq!(r.completion_status, CompletionStatus::Failed);
    assert!(!r.passed);
}
#[test]
fn skipped_required_gate() {
    let mut c = complete_checks();
    c[1] = CheckResult::skip("G2", "G2", "CodeMode server did not initialize");
    let r = ConformanceReport::new(Ns::Fz, "fake", Surface::Codemode, c);
    assert_eq!(r.completion_status, CompletionStatus::Partial);
    assert!(!r.passed);
}
#[test]
fn non_required_skip_is_partial_not_failed() {
    let c = vec![
        CheckResult::pass("G1", "exposure"),
        CheckResult::skip(
            "G2",
            "refs",
            "not applicable to MCP surface; requires CodeMode execution",
        ),
    ];
    let r = ConformanceReport::new(Ns::Fz, "fake", Surface::Mcp, c);
    assert_eq!(r.completion_status, CompletionStatus::Partial);
    assert!(!r.passed);
}
#[test]
fn mcp_g1_only_cannot_false_green() {
    let mut c = vec![CheckResult::pass("G1", "exposure")];
    c.extend(CHECK_IDS[1..].iter().map(|id| {
        CheckResult::skip(
            id,
            id,
            "not applicable to MCP surface; requires CodeMode execution",
        )
    }));
    let r = ConformanceReport::new(Ns::Fz, "fake", Surface::Mcp, c);
    assert_eq!(r.completion_status, CompletionStatus::Partial);
    assert!(!r.passed);
}
#[test]
fn serde_exposes_statuses_and_legacy_passed() {
    let s = CheckResult::skip("G2", "refs", "stable reason");
    assert_eq!(s.status, GateStatus::Skipped);
    let r = ConformanceReport::new(
        Ns::Fz,
        "fake",
        Surface::Mcp,
        vec![CheckResult::pass("G1", "exposure"), s],
    );
    let v = serde_json::to_value(r).unwrap();
    assert_eq!(v["passed"], false);
    assert_eq!(v["completion_status"], "partial");
    assert_eq!(v["checks"][0]["status"], "pass");
    assert_eq!(v["checks"][1]["status"], "skipped");
    assert_eq!(v["checks"][1]["skip_reason"], "stable reason");
}
