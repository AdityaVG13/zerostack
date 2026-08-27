//! FileEngine::lookup fails closed when a budget would hide remaining entries.
//!
//! The ABI returns `Vec<PathBuf>` with no truncated flag (`LookupOptions` is
//! only `{ filter, limit }`). A prefix that looks complete is a lie, so both
//! the requested result cap and the engine visited cap (`LOOKUP_ENTRY_LIMIT`)
//! must surface `EngineErrorKind::Budget`.

use fszero_engine::ZeroFileEngine;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use zero_abi::{
    CancellationProbe, EngineCallContext, EngineErrorKind, EngineInvocation, FileEngine,
    KernelBudget, LookupOptions,
};

/// Must match `LOOKUP_ENTRY_LIMIT` in `crates/fszero-engine/src/zero_kernel.rs`.
const LOOKUP_ENTRY_LIMIT: usize = 10_000;

struct NoopCancel;
impl CancellationProbe for NoopCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}

fn invocation(root: &Path) -> EngineInvocation {
    EngineInvocation {
        context: EngineCallContext {
            workspace_root: root.to_path_buf(),
            project_root: root.to_path_buf(),
            session_id: "lookup-budget".into(),
            cell_id: "cell-1".into(),
            trace_id: "lookup-budget-cell-1".into(),
            deadline_unix_ms: u64::MAX,
            budget: KernelBudget {
                wall_ms: 5_000,
                cpu_ms: 5_000,
                memory_bytes: 64 * 1024 * 1024,
                call_limit: 1_024,
                task_limit: 8,
                output_byte_limit: 64 * 1024,
            },
        },
        cancellation: Arc::new(NoopCancel),
    }
}

fn workspace() -> (tempfile::TempDir, ZeroFileEngine) {
    let dir = tempfile::tempdir().unwrap();
    let engine = ZeroFileEngine::open(
        dir.path(),
        dir.path().join(".zerostack"),
        "lookup-budget-contract",
    )
    .unwrap();
    (dir, engine)
}

fn populate_files(dir: &Path, count: usize) {
    fs::create_dir_all(dir).unwrap();
    for i in 0..count {
        fs::write(dir.join(format!("f{i:05}.txt")), b"x").unwrap();
    }
}

fn lookup(
    engine: &ZeroFileEngine,
    root: &Path,
    dir: &str,
    options: LookupOptions,
) -> Result<Vec<PathBuf>, zero_abi::EngineError> {
    engine.lookup(&invocation(root), PathBuf::from(dir), options)
}

fn assert_budget(err: &zero_abi::EngineError) {
    assert_eq!(err.kind, EngineErrorKind::Budget);
    assert!(
        err.detail.contains("lookup budget exceeded"),
        "Budget detail must name lookup budget, got {}",
        err.detail
    );
}

#[test]
fn requested_limit_fails_closed_when_more_matching_entries_remain() {
    let (dir, engine) = workspace();
    populate_files(&dir.path().join("many"), 8);

    let err = lookup(
        &engine,
        dir.path(),
        "many",
        LookupOptions {
            filter: None,
            limit: Some(3),
            recursive: false,
        },
    )
    .expect_err("a result cap with remaining matches must not look complete");
    assert_budget(&err);
}

#[test]
fn requested_limit_returns_complete_when_match_count_equals_cap() {
    let (dir, engine) = workspace();
    populate_files(&dir.path().join("many"), 3);

    let paths = lookup(
        &engine,
        dir.path(),
        "many",
        LookupOptions {
            filter: None,
            limit: Some(3),
            recursive: false,
        },
    )
    .expect("exactly `limit` matches is a complete listing");
    assert_eq!(paths.len(), 3);
}

#[test]
fn requested_limit_returns_all_when_under_cap() {
    let (dir, engine) = workspace();
    populate_files(&dir.path().join("many"), 3);

    let paths = lookup(
        &engine,
        dir.path(),
        "many",
        LookupOptions {
            filter: None,
            limit: Some(10),
            recursive: false,
        },
    )
    .expect("under-cap listings are complete");
    assert_eq!(paths.len(), 3);
}

#[test]
fn visited_entry_cap_fails_closed_even_with_a_high_result_limit() {
    let (dir, engine) = workspace();
    populate_files(&dir.path().join("many"), LOOKUP_ENTRY_LIMIT + 8);

    let err = lookup(
        &engine,
        dir.path(),
        "many",
        LookupOptions {
            filter: None,
            limit: Some(100_000),
            recursive: false,
        },
    )
    .expect_err("LOOKUP_ENTRY_LIMIT must not return a silent prefix");
    assert_budget(&err);
    assert!(
        err.detail.contains("entry_cap=10000"),
        "visited-cap Budget must name the entry cap, got {}",
        err.detail
    );
}
