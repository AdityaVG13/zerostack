use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use graphzero_engine::ZeroStructuralEngine;
use zero_abi::{
    AsgrepMode, AsgrepOptions, CancellationProbe, EngineCallContext, EngineInvocation,
    KernelBudget, StructuralEngine, StructuralQuery,
};

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
            session_id: "scoped-index".into(),
            cell_id: "cell-1".into(),
            trace_id: "scoped-index-cell-1".into(),
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

#[test]
fn scoped_query_indexes_only_requested_directory() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::create_dir_all(repo.join("noise")).unwrap();
    std::fs::write(
        repo.join("src/lib.rs"),
        "pub struct ScopedNeedle;\nimpl ScopedNeedle { pub fn run(&self) {} }\n",
    )
    .unwrap();
    for index in 0..200 {
        std::fs::write(
            repo.join("noise").join(format!("noise_{index}.rs")),
            format!("pub fn noise_{index}() {{}}\n"),
        )
        .unwrap();
    }
    let graph_store = temp.path().join("graph");
    let zero_store = temp.path().join("zero");
    let engine = ZeroStructuralEngine::open(&repo, graph_store, zero_store).unwrap();

    let result = engine
        .query(
            &invocation(&repo),
            StructuralQuery {
                query: "ScopedNeedle".into(),
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
            .any(|hit| hit.path == Path::new("src/lib.rs"))
    );
    assert!(result.coverage.as_ref().unwrap().freshness_verified);
    assert!(!result.hits.iter().any(|hit| hit.path.starts_with("noise")));
}
