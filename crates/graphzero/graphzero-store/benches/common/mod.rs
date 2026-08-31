//! Shared benchmark fixture: generates a synthetic repo, indexes it once,
//! and hands out the store/repo roots.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use zerostack_test_support::git_commit_all;

pub struct Fixture {
    pub repo_root: PathBuf,
    pub store_root: PathBuf,
    pub _dir: tempfile::TempDir,
}

/// Multi-size sweep for scaling curves.
/// Criterion groups use `BenchmarkId::new("files", n)`.
pub const BENCH_FILE_SWEEP: &[usize] = &[1, 10, 50, 100, 500, 1000];

/// Number of generated source files; override with GRAPHZERO_BENCH_FILES.
/// Each file is ~50 lines, so 2000 files ~ 100k LOC.
fn file_count() -> usize {
    std::env::var("GRAPHZERO_BENCH_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000)
}

fn build_fixture(n: usize) -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo_root = dir.path().join("repo");
    fs::create_dir_all(repo_root.join("src")).unwrap();
    for i in 0..n {
        let mut content = String::new();
        for j in 0..10 {
            content.push_str(&format!(
                "fn func_{i}_{j}(x: u64) -> u64 {{\n    let y = x + {j};\n    helper_{}(y)\n}}\n\n",
                (i + j) % 97
            ));
        }
        for j in 0..2 {
            content.push_str(&format!(
                "fn helper_{}(v: u64) -> u64 {{\n    v * 2\n}}\n\n",
                (i * 2 + j) % 97
            ));
        }
        fs::write(repo_root.join(format!("src/file_{i:05}.rs")), content).unwrap();
    }
    git_commit_all(&repo_root);
    let store_root = repo_root.join(".graphzero");
    graphzero_store::store::indexer::index_repo(&repo_root, &store_root).expect("index");
    Fixture {
        repo_root,
        store_root,
        _dir: dir,
    }
}

/// Default single-N fixture (`GRAPHZERO_BENCH_FILES`, default 2000).
pub fn fixture() -> &'static Fixture {
    static FIXTURE: OnceLock<Fixture> = OnceLock::new();
    FIXTURE.get_or_init(|| build_fixture(file_count()))
}

/// Cached fixture for a specific file count (multi-size criterion sweeps).
pub fn fixture_n(n: usize) -> &'static Fixture {
    static BY_N: OnceLock<Mutex<HashMap<usize, &'static Fixture>>> = OnceLock::new();
    let map = BY_N.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().expect("bench fixture map");
    if let Some(fx) = guard.get(&n) {
        return *fx;
    }
    let leaked: &'static Fixture = Box::leak(Box::new(build_fixture(n)));
    guard.insert(n, leaked);
    leaked
}
