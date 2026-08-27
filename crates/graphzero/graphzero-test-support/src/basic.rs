use std::fs;
use std::path::{Path, PathBuf};

use graphzero_store::store::indexer;
use tempfile::TempDir;

pub const FILE_A: &str =
    "fn alpha(x: u64) -> u64 {\n    beta(x) + 1\n}\n\nfn beta(v: u64) -> u64 {\n    v * 2\n}\n";
pub const FILE_B: &str =
    "struct Widget;\n\nfn gamma() {\n    alpha(7);\n}\n\nfn function_foo() {\n    gamma();\n}\n";

pub struct BasicFixture {
    pub dir: TempDir,
    pub repo_root: PathBuf,
    pub store_root: PathBuf,
}

pub fn write_alpha_beta_repo(repo_root: &Path) {
    fs::create_dir_all(repo_root.join("src"))
        .expect("failed to create src directory for alpha/beta fixture");
    fs::write(repo_root.join("src/a.rs"), FILE_A)
        .expect("failed to write src/a.rs for alpha/beta fixture");
    fs::write(repo_root.join("src/b.rs"), FILE_B)
        .expect("failed to write src/b.rs for alpha/beta fixture");
}

pub fn make_repo() -> BasicFixture {
    let dir = tempfile::tempdir().expect("failed to create temporary directory for basic fixture");
    let repo_root = dir.path().join("repo");
    write_alpha_beta_repo(&repo_root);
    fs::write(repo_root.join(".gitignore"), ".graphzero/\n")
        .expect("failed to write .gitignore for basic fixture repository");
    let store_root = repo_root.join(".graphzero");
    BasicFixture {
        dir,
        repo_root,
        store_root,
    }
}

pub fn indexed_fixture() -> BasicFixture {
    let fx = make_repo();
    indexer::index_repo(&fx.repo_root, &fx.store_root)
        .expect("failed to index basic fixture repository");
    fx
}

pub fn indexed_repo() -> BasicFixture {
    let fx = make_repo();
    super::git::git_commit_all(&fx.repo_root);
    indexer::index_repo(&fx.repo_root, &fx.store_root)
        .expect("failed to index basic fixture repository");
    fx
}

pub fn indexed_repo_no_git() -> BasicFixture {
    indexed_fixture()
}
