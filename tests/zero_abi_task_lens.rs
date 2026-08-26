//! Root law tests for the Wave16 Task Lens internal engine contract.
//!
//! Lattice law under test: `Unsafe` dominates `Unknown` dominates `Safe`.
//! A `Safe` verdict must satisfy every Safe law; anything missing, stale, or
//! incomplete must degrade to `Unknown`; an explicit semantic choice or
//! conflict must be `Unsafe` with reasons; and `reasons` is always the
//! canonical sorted-and-deduplicated list. Coverage tiers A/B/C are
//! independent: completeness is `tier_a_pct >= 99.0` with
//! `freshness_verified`, while B/C may be anywhere in 0..100.

use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use zero_abi::{
    AsgrepMode, AsgrepOptions, CancellationProbe, EngineCallContext, EngineError, EngineErrorKind,
    EngineInvocation, KernelBudget, SafetyVerdict, StructuralCoverage, StructuralEngine,
    StructuralHit, StructuralQuery, StructuralResult, TaskLensCompilerImpact, TaskLensError,
    TaskLensRequest, TaskLensResult, ZeroHandle,
};

fn handle(fill: char) -> ZeroHandle {
    let digest = Sha256::digest([fill as u8]);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("write to String");
    }
    ZeroHandle::from_digest(&encoded).expect("valid handle")
}
fn malformed_handle() -> ZeroHandle {
    // `ZeroHandle` is serde-transparent: malformed values deserialize
    // without the runtime parser, so validation must re-parse them.
    serde_json::from_value(serde_json::json!("z://blob/NOTHEX"))
        .expect("transparent handle deserializes unchecked")
}

fn options() -> AsgrepOptions {
    AsgrepOptions {
        mode: AsgrepMode::Literal,
        path: None,
        language: None,
        source: None,
        sink: None,
        limit: Some(16),
        budget_tokens: None,
    }
}

fn request() -> TaskLensRequest {
    TaskLensRequest {
        query: "pub fn task_lens".into(),
        options: options(),
        capsule_root: Some(handle('c')),
        required_snapshot: Some(handle('s')),
    }
}

fn coverage() -> StructuralCoverage {
    StructuralCoverage {
        tier_a_pct: 100.0,
        tier_b_pct: 35.0,
        tier_c_pct: 25.0,
        freshness_verified: true,
        snapshot_id: 7,
    }
}

fn locus() -> StructuralHit {
    StructuralHit {
        path: PathBuf::from("src/lib.rs"),
        symbol: Some("task_lens".into()),
        line_start: Some(1),
        line_end: Some(1),
        preview: Some("pub fn task_lens() {}".into()),
        evidence: Some(handle('e')),
        source: Some(handle('f')),
        score: 1.0,
    }
}

/// A result satisfying every Safe law against [`request`].
fn complete_result() -> TaskLensResult {
    TaskLensResult {
        verdict: SafetyVerdict::Safe,
        locus: Some(locus()),
        impact: TaskLensCompilerImpact {
            complete: true,
            edge_roots: vec![handle('g')],
            reverse_roots: vec![handle('h')],
        },
        proof_support: vec![handle('i'), handle('j')],
        evidence_roots: vec![handle('c'), handle('s'), handle('k')],
        coverage: Some(coverage()),
        index_digest: "a".repeat(64),
        reasons: vec![],
    }
}

#[test]
fn safe_complete_validates() {
    request().validate().expect("request is well formed");
    complete_result()
        .validate(&request())
        .expect("complete safe result satisfies every Safe law");
    assert!(complete_result().verdict.grants_authority());
}

#[test]
fn incomplete_impact_rejects_safe_verdict() {
    let mut result = complete_result();
    result.impact.complete = false;
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::IncompleteImpact)
    );
}
#[test]
fn safe_requires_rooted_compiler_reverse_impact() {
    let mut result = complete_result();
    result.impact.edge_roots = vec![];
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::IncompleteImpact)
    );

    let mut result = complete_result();
    result.impact.reverse_roots = vec![];
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::IncompleteImpact)
    );
}
#[test]
fn safe_rejects_malformed_locus_root() {
    let mut result = complete_result();
    let mut hit = locus();
    hit.evidence = Some(malformed_handle());
    result.locus = Some(hit);
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::MalformedLocusRoot(malformed_handle()))
    );
}

#[test]
fn safe_rejects_malformed_impact_root() {
    let mut result = complete_result();
    result.impact.edge_roots = vec![handle('g'), malformed_handle()];
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::MalformedImpactRoot(malformed_handle()))
    );

    let mut result = complete_result();
    result.impact.reverse_roots = vec![malformed_handle()];
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::MalformedImpactRoot(malformed_handle()))
    );
}

#[test]
fn safe_rejects_malformed_proof_root() {
    let mut result = complete_result();
    result.proof_support = vec![malformed_handle()];
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::MalformedProofRoot(malformed_handle()))
    );
}

#[test]
fn safe_rejects_malformed_evidence_root() {
    let mut result = complete_result();
    result.evidence_roots = vec![handle('c'), handle('s'), malformed_handle()];
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::MalformedEvidenceRoot(malformed_handle()))
    );
}

#[test]
fn safe_requires_live_index_digest() {
    let mut result = complete_result();
    result.index_digest = "short".into();
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::MalformedIndexDigest)
    );

    let mut result = complete_result();
    result.index_digest = "z".repeat(64); // 64 chars, not hex
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::MalformedIndexDigest)
    );

    let mut result = complete_result();
    result.index_digest = "A".repeat(64); // uppercase hex is not canonical
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::MalformedIndexDigest)
    );
}

#[test]
fn unknown_may_carry_malformed_data_but_never_authority() {
    let mut result = complete_result();
    result.verdict = SafetyVerdict::Unknown {
        reasons: vec!["stale_index".into()],
    };
    result.reasons = vec!["stale_index".into()];
    result.proof_support = vec![malformed_handle()];
    result.index_digest = "not-a-digest".into();
    result
        .validate(&request())
        .expect("unknown tolerates partial malformed data");
    assert!(!result.verdict.grants_authority());
}

#[test]
fn incomplete_degrades_to_unknown_with_reason() {
    let mut result = complete_result();
    result.verdict = SafetyVerdict::Unknown {
        reasons: vec!["incomplete_impact".into()],
    };
    result.impact.complete = false;
    result.reasons = vec!["incomplete_impact".into()];
    result
        .validate(&request())
        .expect("degraded unknown with reason validates");
    assert!(!result.verdict.grants_authority());
}

#[test]
fn unsafe_requires_reasons() {
    let mut result = complete_result();
    result.verdict = SafetyVerdict::Unsafe { reasons: vec![] };
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::UnsafeWithoutReasons)
    );

    let mut result = complete_result();
    result.verdict = SafetyVerdict::Unsafe {
        reasons: vec!["semantic_conflict".into()],
    };
    result.reasons = vec!["semantic_conflict".into()];
    result
        .validate(&request())
        .expect("explicit unsafe with reasons validates");
    assert!(!result.verdict.grants_authority());
}

#[test]
fn reason_normalization_sorts_and_dedups() {
    let mut result = complete_result();
    result.verdict = SafetyVerdict::Unknown {
        reasons: vec![
            "stale_index".into(),
            "missing_coverage".into(),
            "stale_index".into(),
        ],
    };
    result.reasons = vec![
        "stale_index".into(),
        "missing_coverage".into(),
        "stale_index".into(),
    ];
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::UnnormalizedReasons)
    );

    let normalized = result.normalize();
    assert_eq!(normalized.reasons, vec!["missing_coverage", "stale_index"]);
    assert_eq!(
        normalized.verdict.reasons(),
        vec!["missing_coverage", "stale_index"]
    );
    normalized
        .validate(&request())
        .expect("normalized result validates");
}

#[test]
fn reason_mismatch_between_verdict_and_result_is_rejected() {
    let mut result = complete_result();
    result.verdict = SafetyVerdict::Unsafe {
        reasons: vec!["semantic_conflict".into()],
    };
    result.reasons = vec!["extra".into(), "semantic_conflict".into()];
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::ReasonMismatch)
    );
}

#[test]
fn safe_requires_exactly_one_rooted_locus() {
    let mut result = complete_result();
    result.locus = None;
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::MissingLocus)
    );

    let mut result = complete_result();
    result.locus = Some(StructuralHit {
        path: PathBuf::from("src/lib.rs"),
        symbol: None,
        line_start: None,
        line_end: None,
        preview: None,
        evidence: None,
        source: None,
        score: 0.0,
    });
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::UnrootedLocus)
    );
}

#[test]
fn safe_requires_requested_snapshot_and_capsule_roots() {
    let mut result = complete_result();
    result.evidence_roots = vec![handle('k')];
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::MissingEvidenceRoot(handle('c')))
    );

    let mut result = complete_result();
    result.evidence_roots = vec![handle('c'), handle('k')];
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::MissingEvidenceRoot(handle('s')))
    );
}

#[test]
fn safe_without_semantic_choice_gap() {
    let mut result = complete_result();
    result.reasons = vec!["semantic_choice".into()];
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::SafeWithReasons)
    );
}

#[test]
fn safe_requires_non_empty_fresh_proof_support() {
    let mut result = complete_result();
    result.proof_support = vec![];
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::MissingProofSupport)
    );

    let mut result = complete_result();
    result.coverage = None;
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::MissingCoverage)
    );

    let mut result = complete_result();
    let mut stale = coverage();
    stale.freshness_verified = false;
    result.coverage = Some(stale);
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::StaleCoverage)
    );
}

#[test]
fn safe_requires_complete_tier_a_coverage() {
    let mut result = complete_result();
    let mut partial = coverage();
    partial.tier_a_pct = 98.5; // below the 99% complete law
    result.coverage = Some(partial);
    assert_eq!(
        result.validate(&request()),
        Err(TaskLensError::IncompleteCoverage)
    );
}

#[test]
fn coverage_tiers_are_independent() {
    let mut result = complete_result();
    let mut independent = coverage();
    independent.tier_b_pct = 0.0;
    independent.tier_c_pct = 100.0;
    result.coverage = Some(independent);
    result
        .validate(&request())
        .expect("tier B/C are independent; tier A >= 99% suffices");
}

#[test]
fn empty_query_is_rejected() {
    let mut request = request();
    request.query = "   ".into();
    assert_eq!(request.validate(), Err(TaskLensError::EmptyQuery));
}
#[test]
fn invalid_requested_root_is_rejected() {
    // `ZeroHandle` is serde-transparent, so a malformed root can enter a
    // request without the runtime parser; `validate` must reject it.
    let mut capsule_request = request();
    capsule_request.capsule_root = Some(
        serde_json::from_value(serde_json::json!("z://blob/NOTHEX"))
            .expect("transparent handle deserializes unchecked"),
    );
    assert_eq!(
        capsule_request.validate(),
        Err(TaskLensError::InvalidRequestedRoot(
            capsule_request.capsule_root.clone().unwrap()
        ))
    );

    let mut snapshot_request = request();
    snapshot_request.required_snapshot = Some(
        serde_json::from_value(serde_json::json!("garbage"))
            .expect("transparent handle deserializes unchecked"),
    );
    assert_eq!(
        snapshot_request.validate(),
        Err(TaskLensError::InvalidRequestedRoot(
            snapshot_request.required_snapshot.clone().unwrap()
        ))
    );
}

#[test]
fn wire_shape_is_camel_case_and_denies_unknown_fields() {
    let value = serde_json::to_value(&request()).expect("request serializes");
    assert_eq!(
        value["capsuleRoot"],
        serde_json::json!(handle('c').as_str())
    );
    assert_eq!(
        value["requiredSnapshot"],
        serde_json::json!(handle('s').as_str())
    );
    let mut with_unknown = value;
    with_unknown["bogus"] = serde_json::json!(1);
    assert!(
        serde_json::from_value::<TaskLensRequest>(with_unknown).is_err(),
        "unknown request fields are rejected"
    );

    let value = serde_json::to_value(&complete_result()).expect("result serializes");
    assert_eq!(value["indexDigest"], "a".repeat(64));
    assert_eq!(value["impact"]["complete"], serde_json::json!(true));
    let mut with_unknown = value;
    with_unknown["bogus"] = serde_json::json!(1);
    assert!(
        serde_json::from_value::<TaskLensResult>(with_unknown).is_err(),
        "unknown result fields are rejected"
    );
}

struct NoopCancel;
impl CancellationProbe for NoopCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}

fn invocation() -> EngineInvocation {
    EngineInvocation {
        context: EngineCallContext {
            workspace_root: PathBuf::from("/workspace"),
            project_root: PathBuf::from("/workspace/project"),
            session_id: "session".into(),
            cell_id: "cell".into(),
            trace_id: "trace".into(),
            deadline_unix_ms: 1_000,
            budget: KernelBudget {
                wall_ms: 1_000,
                cpu_ms: 500,
                memory_bytes: 64 * 1024 * 1024,
                call_limit: 32,
                task_limit: 8,
                output_byte_limit: 64 * 1024,
            },
        },
        cancellation: Arc::new(NoopCancel),
    }
}

/// A mock that implements only `query` — it must stay source-compatible when
/// the trait gains the `task_lens` default, and the default must be
/// fail-closed (never a `Safe`-looking result).
struct QueryOnlyEngine;
impl StructuralEngine for QueryOnlyEngine {
    fn query(
        &self,
        _invocation: &EngineInvocation,
        _query: StructuralQuery,
    ) -> Result<StructuralResult, EngineError> {
        Err(EngineError::new(
            EngineErrorKind::Unsupported,
            "query is not exercised by this test",
            false,
        ))
    }
}

#[test]
fn default_task_lens_is_fail_closed_unsupported() {
    let engine = QueryOnlyEngine;
    let error = engine
        .task_lens(&invocation(), request())
        .expect_err("default task_lens must not succeed");
    assert_eq!(error.kind, EngineErrorKind::Unsupported);
    assert!(!error.retryable);
    assert!(error.detail.contains("task_lens"));
}
