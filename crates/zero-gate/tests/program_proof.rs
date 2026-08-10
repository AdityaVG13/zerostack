//! Fixture-backed Program assembly proof tests.
//!
//! The frozen fixture `fixtures/program-evidence-v1.json` carries five
//! separated reports (planner, worker, MCP, lifecycle, GC) for a positive
//! program plus eleven negative aggregation cases. The positive case must
//! assemble into an authoritative `ProgramProof` without fallback closure and
//! without synthetic receipts; every negative case must fail closed with the
//! exact error kind the fixture declares.

use serde::Deserialize;
use zero_gate::{
    GcReport, LifecycleReport, McpReport, PROGRAM_ASSEMBLY_SCHEMA_VERSION, PlannerReport,
    ProgramDigest, ProgramReports, WorkerClosureKind, WorkerReport, assemble,
};

const FIXTURE: &str = include_str!("fixtures/program-evidence-v1.json");

#[derive(Debug, Deserialize)]
struct Fixture {
    schema_version: u16,
    positive: PositiveCase,
    negative: Vec<NegativeCase>,
}

#[derive(Debug, Deserialize)]
struct PositiveCase {
    reports: ReportsShape,
    expected_program_digest: String,
}

#[derive(Debug, Deserialize)]
struct NegativeCase {
    name: String,
    reports: ReportsShape,
    expected_error: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ReportsShape {
    planner: Option<PlannerReport>,
    worker: Option<WorkerReport>,
    mcp: Option<McpReport>,
    lifecycle: Option<LifecycleReport>,
    gc: Option<GcReport>,
}

fn to_reports(shape: ReportsShape) -> ProgramReports {
    let mut reports = ProgramReports::new();
    if let Some(report) = shape.planner {
        reports = reports.planner(report);
    }
    if let Some(report) = shape.worker {
        reports = reports.worker(report);
    }
    if let Some(report) = shape.mcp {
        reports = reports.mcp(report);
    }
    if let Some(report) = shape.lifecycle {
        reports = reports.lifecycle(report);
    }
    if let Some(report) = shape.gc {
        reports = reports.gc(report);
    }
    reports
}

fn parse_digest(hex: &str) -> ProgramDigest {
    assert_eq!(hex.len(), 64, "digest must be 32 bytes of hex");
    let mut digest = [0u8; 32];
    for (index, chunk) in hex.as_bytes().chunks_exact(2).enumerate() {
        let pair = std::str::from_utf8(chunk).unwrap();
        digest[index] = u8::from_str_radix(pair, 16).expect("hex digest");
    }
    digest
}

fn fixture() -> Fixture {
    serde_json::from_str(FIXTURE).expect("frozen fixture must parse")
}

#[test]
fn fixture_schema_version_is_current() {
    let fixture = fixture();
    assert_eq!(fixture.schema_version, PROGRAM_ASSEMBLY_SCHEMA_VERSION);
}

#[test]
fn positive_program_proof_passes_without_fallback_or_synthetic_receipts() {
    let fixture = fixture();
    let reports = to_reports(fixture.positive.reports);
    let proof = assemble(reports).expect("fixture-backed Program proof must assemble");

    // Authoritative proof binds the program and its truthful aggregates.
    assert_eq!(
        proof.program_digest(),
        parse_digest(&fixture.positive.expected_program_digest),
        "program digest must match the frozen known answer"
    );
    assert_eq!(proof.step_count(), 3);
    assert_eq!(proof.tool_count(), 2);
    assert_eq!(proof.call_count(), 5);
    assert_eq!(proof.collected_objects(), 7);
    assert_eq!(proof.freed_bytes(), 4096);
    proof.verify().expect("proof must re-verify");

    // The proof is commit-closed: no fallback was aggregated.
    let worker = proof.worker_digest();
    assert_ne!(worker, [0u8; 32], "worker evidence must be real");
}

#[test]
fn every_report_in_the_positive_fixture_is_real_evidence() {
    // None of the five separated reports may carry a zero binding: a synthetic
    // receipt must never be aggregated into a proof.
    let fixture = fixture();
    let shape = fixture.positive.reports;
    let planner = shape.planner.expect("planner report present");
    let worker = shape.worker.expect("worker report present");
    let mcp = shape.mcp.expect("mcp report present");
    let lifecycle = shape.lifecycle.expect("lifecycle report present");
    let _gc = shape.gc.expect("gc report present");

    assert_ne!(planner.plan_digest(), [0u8; 32]);
    assert!(planner.step_count() >= 1);
    assert_ne!(worker.worker_id(), [0u8; 32]);
    assert_ne!(worker.effects_digest(), [0u8; 32]);
    assert_ne!(worker.output_digest(), [0u8; 32]);
    assert_ne!(worker.mcp_evidence_digest(), [0u8; 32]);
    assert!(worker.executed_steps() >= 1);
    assert_ne!(mcp.tools_digest(), [0u8; 32]);
    assert!(lifecycle.executed_step_count() >= 1);

    // The worker committed; the fixture must not rely on a fallback.
    assert_eq!(worker.closure_kind(), WorkerClosureKind::Commit);
}

#[test]
fn negative_cases_fail_closed_with_exact_error_kinds() {
    let fixture = fixture();
    assert!(
        !fixture.negative.is_empty(),
        "fixture must carry negative cases"
    );
    for case in &fixture.negative {
        let reports = to_reports(case.reports.clone());
        let error = assemble(reports).expect_err(&format!("case '{}' must fail", case.name));
        assert_eq!(
            error.kind(),
            case.expected_error,
            "case '{}' must fail with the exact declared error",
            case.name
        );
    }
}
