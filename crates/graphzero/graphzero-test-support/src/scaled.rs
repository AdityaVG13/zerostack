use std::fs;
use std::path::{Path, PathBuf};

use graphzero_store::store::indexer;
use tempfile::TempDir;

pub struct ScaledFixture {
    pub dir: TempDir,
    pub repo_root: PathBuf,
    pub store_root: PathBuf,
    pub target_symbol: String,
    pub file_count: usize,
}

/// Chain-call fixture: `sym_k` calls `sym_{k-1}` when k > 0.
pub fn write_scaled_repo(repo_root: &Path, file_count: usize) -> String {
    fs::create_dir_all(repo_root.join("src"))
        .expect("failed to create src directory for scaled fixture");
    let mid = file_count / 2;
    for i in 0..file_count {
        let body = if i == 0 {
            "pub fn sym_0() -> u64 { 0 }\n".to_string()
        } else {
            format!("pub fn sym_{i}() -> u64 {{ sym_{p}() + 1 }}\n", p = i - 1)
        };
        fs::write(repo_root.join(format!("src/m_{i:04}.rs")), body)
            .unwrap_or_else(|err| panic!("failed to write scaled fixture src/m_{i:04}.rs: {err}"));
    }
    format!("sym_{mid}")
}

pub fn indexed_scaled_repo(file_count: usize) -> ScaledFixture {
    let dir = tempfile::tempdir().expect("failed to create temporary directory for scaled fixture");
    let repo_root = dir.path().join("repo");
    let target_symbol = write_scaled_repo(&repo_root, file_count);
    let store_root = repo_root.join(".graphzero");
    indexer::index_repo(&repo_root, &store_root)
        .expect("failed to index scaled fixture repository");
    ScaledFixture {
        dir,
        repo_root,
        store_root,
        target_symbol,
        file_count,
    }
}
