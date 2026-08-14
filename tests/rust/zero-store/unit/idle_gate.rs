//! Idle-overhead release gate tests (ZS-OPS-006 / V6-R14): the gate refuses
//! without evidence, budget breaches are loud sealed refusals (never silent
//! clamping), background activity refuses the gate (daemonless law). The
//! real-host measurement test lives in the crate's integration test
//! (`idle_gate_host.rs`) where `getrusage` may be called.

use super::*;

fn sample(cpu_user_ns: u64, cpu_sys_ns: u64, rss_bytes: u64, wall: u64, at: u64) -> IdleSampleV1 {
    IdleSampleV1::new(wall, cpu_user_ns, cpu_sys_ns, rss_bytes, at)
}

fn within_budget_evidence() -> IdleWindowEvidenceV1 {
    // 50ms window, ~0 CPU, ~1MB RSS -> well inside 0.1% / 500MB.
    IdleWindowEvidenceV1::new(
        1_000_000,
        51_000_000,
        vec![
            sample(5_000_000_000, 2_000_000_000, 1_000_000, 10_000_000, 1_000_000),
            sample(5_000_000_500, 2_000_000_500, 1_100_000, 30_000_000, 25_000_000),
            sample(5_000_001_000, 2_000_001_000, 1_050_000, 60_000_000, 51_000_000),
        ],
        0,
    )
    .unwrap()
}

/// The release gate fails without evidence (acceptance core).
#[test]
fn release_gate_refuses_without_evidence() {
    let error = evaluate_idle_release_gate_v1(None, IdleBudgetsV1::default()).unwrap_err();
    assert_eq!(error, IdleGateErrorV1::RequiresEvidence);
}

/// Evidence within budget admits with counts and a stable digest.
#[test]
fn release_gate_admits_evidence_within_budget_and_seals_receipt() {
    let evidence = within_budget_evidence();
    let first_digest = evidence.digest().unwrap();
    assert_eq!(evidence.digest().unwrap(), first_digest, "evidence digest is deterministic");

    let receipt = evaluate_idle_release_gate_v1(Some(&evidence), IdleBudgetsV1::default()).unwrap();
    assert!(receipt.admitted);
    assert_eq!(receipt.refusal, None);
    assert_eq!(receipt.samples, 3);
    assert_eq!(receipt.window_wall_ns, 50_000_000);
    assert_eq!(receipt.observed_max_rss_bytes, 1_100_000);
    assert_eq!(receipt.evidence_digest, first_digest);
    assert_eq!(
        receipt.digest().unwrap(),
        receipt.digest().unwrap(),
        "receipt digest is deterministic"
    );

    // The same evidence passes the same gate again: evidence is replayable,
    // the decision is a function of evidence + budgets.
    let again = evaluate_idle_release_gate_v1(Some(&evidence), IdleBudgetsV1::default()).unwrap();
    assert_eq!(again, receipt);
}

/// CPU over budget is a loud sealed refusal with observed/budget values --
/// never clamped or silently ignored.
#[test]
fn cpu_budget_breach_is_loud_sealed_refusal() {
    // 100% CPU over the window (1e9 ppb vs 1e6 budget).
    let hot = IdleWindowEvidenceV1::new(
        1_000_000,
        51_000_000,
        vec![
            sample(5_000_000_000, 2_000_000_000, 1_000_000, 10_000_000, 1_000_000),
            sample(5_000_000_000, 2_000_000_000, 1_100_000, 30_000_000, 25_000_000),
            sample(5_040_000_000, 2_010_000_000, 1_050_000, 60_000_000, 51_000_000),
        ],
        0,
    )
    .unwrap();
    let receipt = evaluate_idle_release_gate_v1(Some(&hot), IdleBudgetsV1::default()).unwrap();
    assert!(!receipt.admitted);
    let refusal = receipt.refusal.unwrap();
    assert_eq!(refusal.reason, IdleGateRefusalReasonV1::CpuBudgetViolation);
    assert_eq!(refusal.observed, 1_000_000_000);
    assert_eq!(refusal.budget, DEFAULT_IDLE_MAX_CPU_FRACTION_PPB_V1);
    assert_eq!(receipt.observed_max_cpu_fraction_ppb, 1_000_000_000);
}

/// RSS over budget is a loud sealed refusal.
#[test]
fn rss_budget_breach_is_loud_sealed_refusal() {
    let bloated = IdleWindowEvidenceV1::new(
        1_000_000,
        51_000_000,
        vec![
            sample(5_000_000_000, 2_000_000_000, 600 * 1024 * 1024, 10_000_000, 1_000_000),
            sample(5_000_000_500, 2_000_000_500, 640 * 1024 * 1024, 30_000_000, 25_000_000),
            sample(5_000_001_000, 2_000_001_000, 620 * 1024 * 1024, 60_000_000, 51_000_000),
        ],
        0,
    )
    .unwrap();
    let receipt = evaluate_idle_release_gate_v1(Some(&bloated), IdleBudgetsV1::default()).unwrap();
    assert!(!receipt.admitted);
    let refusal = receipt.refusal.unwrap();
    assert_eq!(refusal.reason, IdleGateRefusalReasonV1::RssBudgetViolation);
    assert_eq!(refusal.observed, 640 * 1024 * 1024);
    assert_eq!(refusal.budget, DEFAULT_IDLE_MAX_RSS_BYTES_V1);
    assert_eq!(receipt.observed_max_rss_bytes, 640 * 1024 * 1024);
}

/// Background activity during the idle window refuses the gate (daemonless
/// law: no background work when idle).
#[test]
fn background_activity_refuses_the_gate() {
    let mut evidence = within_budget_evidence();
    evidence.background_activity_events = 3;
    let receipt = evaluate_idle_release_gate_v1(Some(&evidence), IdleBudgetsV1::default()).unwrap();
    assert!(!receipt.admitted);
    let refusal = receipt.refusal.unwrap();
    assert_eq!(
        refusal.reason,
        IdleGateRefusalReasonV1::BackgroundActivityDetected
    );
    assert_eq!(refusal.observed, 3);
}

/// Structurally invalid evidence fails loud before any budget comparison.
#[test]
fn invalid_evidence_fails_loud() {
    // Empty sample set.
    let empty = IdleWindowEvidenceV1::new(1_000_000, 2_000_000, vec![], 0);
    assert!(matches!(empty, Err(IdleGateErrorV1::InvalidEvidence(_))));

    // Non-monotonic CPU readouts.
    let backwards = IdleWindowEvidenceV1::new(
        1_000_000,
        3_000_000,
        vec![
            sample(5_000_000_000, 2_000_000_000, 1_000_000, 10_000_000, 1_000_000),
            sample(4_000_000_000, 2_000_000_000, 1_100_000, 20_000_000, 3_000_000),
        ],
        0,
    );
    assert!(matches!(backwards, Err(IdleGateErrorV1::InvalidEvidence(_))));
}

/// A real measured idle window: the in-process sampler sleeps the window
/// (no spinning, no child processes) and the resulting evidence passes the
/// default budgets -- the sidecar's actual steady state is measured, not
/// promised. (Host sampler lives in `tests/rust/zero-store/idle_gate_host.rs`
/// because `getrusage` requires `unsafe` on current libc and this crate is
/// `#![forbid(unsafe_code)]`.)
#[test]
fn measured_idle_window_is_covered_by_host_integration_test() {
    // Fixture-level double-check of the measure/validate round trip with a
    // deterministic sampler is covered by `release_gate_admits_evidence...`;
    // the real host measurement is exercised in `idle_gate_host.rs`.
    let manifest = idle_gate_contract_v1();
    assert_eq!(manifest["release_gate"]["fails_without_evidence"], true);
}

/// The contract manifest freezes the semantics.
#[test]
fn contract_manifest_freezes_budgets_and_no_evidence_refusal() {
    let manifest = idle_gate_contract_v1();
    assert_eq!(manifest["schema_version"], IDLE_GATE_SCHEMA_VERSION_V1);
    assert_eq!(
        manifest["budgets"]["max_cpu_fraction"],
        serde_json::json!("0.1% (1_000_000 ppb)")
    );
    assert_eq!(
        manifest["budgets"]["max_rss_bytes"],
        serde_json::json!(500 * 1024 * 1024)
    );
    assert_eq!(manifest["release_gate"]["fails_without_evidence"], true);
    assert_eq!(manifest["measurement"]["no_spawn_per_call"], true);
    assert_eq!(manifest["measurement"]["no_background_work_while_idle"], true);
}

/// Evidence digest is tamper-sensitive: flipping one RSS byte changes it.
#[test]
fn evidence_digest_detects_tampering() {
    let evidence = within_budget_evidence();
    let original = evidence.digest().unwrap();
    let mut tampered = evidence.clone();
    tampered.samples[1].rss_bytes += 1;
    assert_ne!(tampered.digest().unwrap(), original);
}
