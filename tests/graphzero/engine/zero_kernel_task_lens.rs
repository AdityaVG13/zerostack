//! Wave16 Task Lens root tests over a real indexed fixture
//! (graphzero-engine `StructuralEngine::task_lens`).
//!
//! Covers the fail-closed lens laws end to end:
//!   - known definition is `Safe` when every requested root exists and the
//!     required snapshot equals the live index digest;
//!   - a stale/mismatched required snapshot (and an absent capsule root) is
//!     `Unknown`, never `Safe`;
//!   - an ambiguous query (multiple candidate definitions) is `Unknown`;
//!   - AST-only evidence (literal mode) is `Unknown` with
//!     `compiler_semantics_required`.

use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use graphzero_engine::ZeroStructuralEngine;
use zero_abi::{
    AsgrepMode, AsgrepOptions, CancellationProbe, EngineCallContext, EngineInvocation,
    KernelBudget, SafetyVerdict, StructuralEngine, TaskLensRequest, ZeroHandle,
};
use zero_store::ZeroCas;

struct NoopCancel;

impl CancellationProbe for NoopCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}

fn invocation(root: &Path) -> EngineInvocation {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    EngineInvocation {
        context: EngineCallContext {
            workspace_root: root.to_path_buf(),
            project_root: root.to_path_buf(),
            session_id: "task-lens".into(),
            cell_id: "cell-1".into(),
            trace_id: "task-lens-cell-1".into(),
            deadline_unix_ms: now + 30_000,
            budget: KernelBudget {
                wall_ms: 30_000,
                cpu_ms: 30_000,
                memory_bytes: 128 * 1024 * 1024,
                call_limit: 64,
                task_limit: 4,
                output_byte_limit: 64 * 1024,
            },
        },
        cancellation: Arc::new(NoopCancel),
    }
}

fn commit_fixture(root: &Path) {
    let repository = git2::Repository::init(root).unwrap();
    let mut index = repository.index().unwrap();
    index
        .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repository.find_tree(tree_id).unwrap();
    let signature = git2::Signature::now("ZeroKernel test", "zero@example.invalid").unwrap();
    repository
        .commit(Some("HEAD"), &signature, &signature, "fixture", &tree, &[])
        .unwrap();
}

/// Fixture layout:
///   - `lens_build` is called by `lens_consume` (one reverse calls edge);
///   - `lens_ambig` is defined twice (ambiguous query);
///   - `LensTarget` is a plain struct with no callers.
fn fixture_repo(repo: &Path) {
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(
        repo.join("src/lib.rs"),
        "pub struct LensTarget { pub value: u32 }\n\
         pub fn lens_build() -> LensTarget { LensTarget { value: 7 } }\n\
         pub fn lens_consume() -> u32 { lens_build().value }\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("src/other.rs"),
        "pub fn lens_ambig() -> u32 { 1 }\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("src/lib2.rs"),
        "pub fn lens_ambig() -> u32 { 2 }\n",
    )
    .unwrap();
    commit_fixture(repo);
}

fn lens_options(mode: AsgrepMode) -> AsgrepOptions {
    AsgrepOptions {
        mode,
        path: None,
        language: None,
        source: None,
        sink: None,
        limit: None,
        budget_tokens: Some(512),
    }
}

#[test]
fn task_lens_safe_definition_with_all_roots() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    fixture_repo(&repo);
    let graph_store = temp.path().join("graph");
    let zero_store = temp.path().join("zero");
    let engine = ZeroStructuralEngine::open(&repo, &graph_store, &zero_store).unwrap();

    // Probe: no roots requested, Symbols mode.
    let probe = engine
        .task_lens(
            &invocation(&repo),
            TaskLensRequest {
                query: "lens_build".into(),
                options: lens_options(AsgrepMode::Symbols),
                capsule_root: None,
                required_snapshot: None,
            },
        )
        .unwrap();
    assert_eq!(
        probe.verdict,
        SafetyVerdict::Safe,
        "known definition should be Safe, got reasons {:?}",
        probe.reasons
    );
    assert!(
        !probe.index_digest.is_empty(),
        "index digest must be present"
    );
    let locus = probe.locus.as_ref().expect("Safe result carries a locus");
    assert_eq!(locus.symbol.as_deref(), Some("lens_build"));
    assert_eq!(locus.path, Path::new("src/lib.rs"));
    assert!(
        locus.evidence.is_some() || locus.source.is_some(),
        "locus must be anchored to a content handle"
    );
    assert!(probe.impact.complete, "reverse impact must be complete");
    assert!(
        !probe.proof_support.is_empty(),
        "proof support must be non-empty"
    );
    assert!(
        probe
            .coverage
            .as_ref()
            .is_some_and(|coverage| coverage.freshness_verified && coverage.tier_a_pct >= 99.0),
        "coverage must be fresh and complete"
    );

    // All roots exist: capsule root present in CAS and required snapshot
    // equal to the live index digest; Definition mode routes the same path.
    let cas = ZeroCas::open(&zero_store);
    let capsule_root = cas.put(b"{\"capsule\":\"fixture\"}").unwrap();
    let required_snapshot = ZeroHandle::from_digest(&probe.index_digest).unwrap();
    let result = engine
        .task_lens(
            &invocation(&repo),
            TaskLensRequest {
                query: "lens_build".into(),
                options: lens_options(AsgrepMode::Definition),
                capsule_root: Some(capsule_root.clone()),
                required_snapshot: Some(required_snapshot.clone()),
            },
        )
        .unwrap();
    assert_eq!(
        result.verdict,
        SafetyVerdict::Safe,
        "Safe with all roots honored, got reasons {:?}",
        result.reasons
    );
    assert!(
        result.evidence_roots.contains(&capsule_root),
        "evidence must cover the requested capsule root"
    );
    assert!(
        result.evidence_roots.contains(&required_snapshot),
        "evidence must cover the requested snapshot root"
    );
    assert_eq!(result.index_digest, probe.index_digest);
}

#[test]
fn task_lens_stale_or_mismatched_snapshot_is_unknown() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    fixture_repo(&repo);
    let engine =
        ZeroStructuralEngine::open(&repo, temp.path().join("graph"), temp.path().join("zero"))
            .unwrap();

    // Mismatched required snapshot: a valid digest that is not the live index.
    let wrong_snapshot = ZeroHandle::from_digest(&"0".repeat(64)).unwrap();
    let result = engine
        .task_lens(
            &invocation(&repo),
            TaskLensRequest {
                query: "lens_build".into(),
                options: lens_options(AsgrepMode::Symbols),
                capsule_root: None,
                required_snapshot: Some(wrong_snapshot),
            },
        )
        .unwrap();
    assert!(matches!(result.verdict, SafetyVerdict::Unknown { .. }));
    assert!(
        result
            .reasons
            .iter()
            .any(|reason| reason.contains("snapshot")),
        "reason must name the snapshot mismatch, got {:?}",
        result.reasons
    );
    assert!(result.locus.is_none());
    assert!(!result.impact.complete);

    // Requested capsule root absent from the store is also Unknown.
    let absent_capsule = ZeroHandle::from_digest(&"1".repeat(64)).unwrap();
    let result = engine
        .task_lens(
            &invocation(&repo),
            TaskLensRequest {
                query: "lens_build".into(),
                options: lens_options(AsgrepMode::Symbols),
                capsule_root: Some(absent_capsule),
                required_snapshot: None,
            },
        )
        .unwrap();
    assert!(matches!(result.verdict, SafetyVerdict::Unknown { .. }));
    assert!(
        result
            .reasons
            .iter()
            .any(|reason| reason.contains("capsule root")),
        "reason must name the missing capsule root, got {:?}",
        result.reasons
    );
}

#[test]
fn task_lens_ambiguous_query_is_unknown() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    fixture_repo(&repo);
    let engine =
        ZeroStructuralEngine::open(&repo, temp.path().join("graph"), temp.path().join("zero"))
            .unwrap();
    let result = engine
        .task_lens(
            &invocation(&repo),
            TaskLensRequest {
                query: "lens_ambig".into(),
                options: lens_options(AsgrepMode::Symbols),
                capsule_root: None,
                required_snapshot: None,
            },
        )
        .unwrap();
    assert!(matches!(result.verdict, SafetyVerdict::Unknown { .. }));
    assert!(
        result
            .reasons
            .iter()
            .any(|reason| reason.contains("multiple candidate definitions")),
        "reason must name the ambiguity, got {:?}",
        result.reasons
    );
    assert!(result.locus.is_none());
}

#[test]
fn task_lens_ast_literal_is_unknown() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    fixture_repo(&repo);
    let engine =
        ZeroStructuralEngine::open(&repo, temp.path().join("graph"), temp.path().join("zero"))
            .unwrap();
    let result = engine
        .task_lens(
            &invocation(&repo),
            TaskLensRequest {
                query: "lens_build".into(),
                options: lens_options(AsgrepMode::Literal),
                capsule_root: None,
                required_snapshot: None,
            },
        )
        .unwrap();
    assert!(matches!(result.verdict, SafetyVerdict::Unknown { .. }));
    assert!(
        result
            .reasons
            .iter()
            .any(|reason| reason.contains("compiler_semantics_required")),
        "AST evidence alone cannot claim compiler semantics, got {:?}",
        result.reasons
    );
    assert!(result.locus.is_none());
    assert!(!result.impact.complete);
}
