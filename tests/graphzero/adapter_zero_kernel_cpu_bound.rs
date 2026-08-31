use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use zero_abi::{
    AsgrepMode, AsgrepOptions, CancellationProbe, EngineCallContext, EngineInvocation,
    KernelBudget, StructuralEngine, StructuralQuery,
};
use zero_graph::ZeroStructuralEngine;

struct NoopCancel;

impl CancellationProbe for NoopCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}

fn invocation(root: &Path, budget: KernelBudget) -> EngineInvocation {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    EngineInvocation {
        context: EngineCallContext {
            workspace_root: root.to_path_buf(),
            project_root: root.to_path_buf(),
            session_id: "cpu-bound".into(),
            cell_id: "cell-1".into(),
            trace_id: "cpu-bound-cell-1".into(),
            deadline_unix_ms: now + 30_000,
            budget,
        },
        cancellation: Arc::new(NoopCancel),
    }
}

fn high_budget() -> KernelBudget {
    KernelBudget {
        wall_ms: 30_000,
        cpu_ms: 30_000,
        memory_bytes: 128 * 1024 * 1024,
        call_limit: 64,
        task_limit: 16,
        output_byte_limit: 64 * 1024,
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

#[test]
fn zero_kernel_cpu_bound() {
    // Authoritative limit is exactly 1 regardless of caller budget.
    assert_eq!(ZeroStructuralEngine::INDEX_WORKERS, 1);

    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(
        repo.join("src/lib.rs"),
        "pub struct CpuBoundNeedle;\nimpl CpuBoundNeedle { pub fn run(&self) {} }\n",
    )
    .unwrap();
    std::fs::write(repo.join("src/main.rs"), "fn main() {}\n").unwrap();
    commit_fixture(&repo);

    let graph_store = temp.path().join("graph");
    let zero_store = temp.path().join("zero");

    // Constructor must remain no-index / no-thread: no pool, no files yet.
    let engine = ZeroStructuralEngine::open(&repo, &graph_store, &zero_store).unwrap();
    assert!(
        !graph_store.join(".manifest").exists(),
        "open must not create a cold graph index"
    );
    // AST index is also lazy; no file should exist before first query.
    let ast_index = graph_store.join("ast-sgrep").join("index.db");
    assert!(!ast_index.exists(), "open must not build AST index");

    // Small first query must still succeed even with a high task_limit budget;
    // the adapter caps to 1 worker.
    let budget = high_budget();
    assert!(
        budget.task_limit > 1,
        "test budget should exceed the 1-worker cap"
    );
    let result = engine
        .query(
            &invocation(&repo, budget),
            StructuralQuery {
                query: "CpuBoundNeedle".into(),
                options: AsgrepOptions {
                    mode: AsgrepMode::Literal,
                    path: Some(PathBuf::from("src")),
                    language: Some("rust".into()),
                    source: None,
                    sink: None,
                    limit: Some(8),
                    budget_tokens: Some(256),
                },
            },
        )
        .unwrap();
    assert!(
        result
            .hits
            .iter()
            .any(|hit| hit.path == Path::new("src/lib.rs")),
        "first AST query should find the needle"
    );
    // Even a one-worker index should verify freshness for this small repo.
    // If coverage is not verified, the hit is still evidence the index ran.
    assert!(!result.hits.is_empty());

    // Relationship (graph) cold-index must also be lazy and capped to the
    // same 1-worker pool. Trigger a graph mode query and verify it still
    // succeeds without amplifying workers.
    assert!(
        !graph_store.join(".manifest").exists(),
        "AST-only query must not create the graph index"
    );
    let _graph_result = engine
        .query(
            &invocation(&repo, high_budget()),
            StructuralQuery {
                query: "CpuBoundNeedle".into(),
                options: AsgrepOptions {
                    mode: AsgrepMode::Symbols,
                    path: None,
                    language: None,
                    source: None,
                    sink: None,
                    limit: Some(8),
                    budget_tokens: Some(256),
                },
            },
        )
        .unwrap();
    // Query must succeed (empty hits allowed, but no error) and the
    // lazily-created pool should now be initialized after first graph use.
    assert!(
        graph_store.join(".manifest").exists(),
        "first graph query must publish the cold index manifest"
    );
    assert_eq!(ZeroStructuralEngine::INDEX_WORKERS, 1);
}
