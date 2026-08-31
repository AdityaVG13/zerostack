use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use ast_sgrep_core::{IndexOptions, Indexer, SearchOptions, Searcher};

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

#[test]
fn exact_file_scope_returns_only_that_file() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/lib.rs"), "pub struct ExactNeedle;\n").unwrap();
    std::fs::write(repo.join("src/other.rs"), "pub struct SiblingOnlyNeedle;\n").unwrap();
    let graph_store = temp.path().join("graph");
    let engine = ZeroStructuralEngine::open(&repo, &graph_store, temp.path().join("zero")).unwrap();

    let result = engine
        .query(
            &invocation(&repo),
            StructuralQuery {
                query: "ExactNeedle".into(),
                options: AsgrepOptions {
                    mode: AsgrepMode::Literal,
                    path: Some(PathBuf::from("src/lib.rs")),
                    language: Some("rust".into()),
                    source: None,
                    sink: None,
                    limit: Some(8),
                    budget_tokens: Some(256),
                },
            },
        )
        .unwrap();

    assert!(!result.hits.is_empty());
    assert!(
        result
            .hits
            .iter()
            .all(|hit| hit.path == Path::new("src/lib.rs"))
    );
    let scope = blake3::hash(b"src").to_hex();
    let scoped_index = graph_store
        .join("ast-sgrep/scopes")
        .join(format!("{scope}.db"));
    let sibling = Searcher::new(SearchOptions {
        root: repo.join("src"),
        index_path: Some(scoped_index),
        limit: 8,
        ..SearchOptions::default()
    })
    .unwrap()
    .search("literal:SiblingOnlyNeedle")
    .unwrap();
    assert!(
        sibling.hits.is_empty(),
        "an exact-file query must not index sibling files"
    );
}

#[test]
fn versioned_product_index_ignores_newer_legacy_schema() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/lib.rs"), "pub struct VersionedNeedle;\n").unwrap();
    let graph_store = temp.path().join("graph");
    let scope = blake3::hash(b"src").to_hex();
    let legacy_index = graph_store
        .join("ast-sgrep/scopes")
        .join(format!("{scope}.db"));
    std::fs::create_dir_all(legacy_index.parent().unwrap()).unwrap();
    let legacy = Indexer::new(IndexOptions {
        root: repo.join("src"),
        index_path: Some(legacy_index),
        embed_semantic: false,
        ..IndexOptions::default()
    })
    .unwrap();
    legacy
        .store()
        .connection()
        .execute_batch("PRAGMA user_version = 2147483647")
        .unwrap();
    drop(legacy);

    let engine = ZeroStructuralEngine::open(&repo, &graph_store, temp.path().join("zero")).unwrap();
    let result = engine
        .query(
            &invocation(&repo),
            StructuralQuery {
                query: "VersionedNeedle".into(),
                options: AsgrepOptions {
                    mode: AsgrepMode::Literal,
                    path: Some(PathBuf::from("src/lib.rs")),
                    language: Some("rust".into()),
                    source: None,
                    sink: None,
                    limit: Some(8),
                    budget_tokens: Some(256),
                },
            },
        )
        .expect("product index must not open an incompatible legacy database");
    assert!(!result.hits.is_empty());
}

#[test]
fn unbounded_deadline_does_not_overflow_query_budget() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("lib.rs"), "pub struct DeadlineNeedle;\n").unwrap();
    let engine =
        ZeroStructuralEngine::open(&repo, temp.path().join("graph"), temp.path().join("zero"))
            .unwrap();
    let mut call = invocation(&repo);
    call.context.deadline_unix_ms = u64::MAX;
    call.context.budget.wall_ms = u64::MAX;

    let result = engine
        .query(
            &call,
            StructuralQuery {
                query: "DeadlineNeedle".into(),
                options: AsgrepOptions {
                    mode: AsgrepMode::Literal,
                    path: Some(PathBuf::from("lib.rs")),
                    language: Some("rust".into()),
                    source: None,
                    sink: None,
                    limit: Some(8),
                    budget_tokens: Some(256),
                },
            },
        )
        .unwrap();
    assert!(!result.hits.is_empty());
}

#[cfg(unix)]
#[test]
fn exact_scope_rejects_symlinks_outside_repository() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let outside = temp.path().join("outside");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("lib.rs"), "pub struct Outside;\n").unwrap();
    symlink(&outside, repo.join("escape")).unwrap();
    symlink(outside.join("lib.rs"), repo.join("escape.rs")).unwrap();
    let engine =
        ZeroStructuralEngine::open(&repo, temp.path().join("graph"), temp.path().join("zero"))
            .unwrap();

    for path in ["escape", "escape.rs"] {
        let error = engine
            .query(
                &invocation(&repo),
                StructuralQuery {
                    query: "Outside".into(),
                    options: AsgrepOptions {
                        mode: AsgrepMode::Literal,
                        path: Some(PathBuf::from(path)),
                        language: None,
                        source: None,
                        sink: None,
                        limit: Some(8),
                        budget_tokens: Some(256),
                    },
                },
            )
            .expect_err("outside symlink scope must fail closed");
        assert_eq!(error.kind, zero_abi::EngineErrorKind::OutsideWorkspace);
    }
}
