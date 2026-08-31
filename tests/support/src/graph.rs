use std::fs;
use std::path::{Path, PathBuf};

use graphzero_store::store::indexer;
use tempfile::TempDir;

use crate::git_commit_all;

pub const FILE_A: &str =
    "fn alpha(x: u64) -> u64 {\n    beta(x) + 1\n}\n\nfn beta(v: u64) -> u64 {\n    v * 2\n}\n";
pub const FILE_B: &str =
    "struct Widget;\n\nfn gamma() {\n    alpha(7);\n}\n\nfn function_foo() {\n    gamma();\n}\n";

pub struct BasicGraphFixture {
    pub dir: TempDir,
    pub repo_root: PathBuf,
    pub store_root: PathBuf,
}

pub fn write_alpha_beta_repo(repo_root: &Path) {
    fs::create_dir_all(repo_root.join("src")).expect("create graph fixture src directory");
    fs::write(repo_root.join("src/a.rs"), FILE_A).expect("write graph fixture src/a.rs");
    fs::write(repo_root.join("src/b.rs"), FILE_B).expect("write graph fixture src/b.rs");
}

pub fn basic_indexed_repo() -> BasicGraphFixture {
    let dir = tempfile::tempdir().expect("create graph fixture directory");
    let repo_root = dir.path().join("repo");
    write_alpha_beta_repo(&repo_root);
    fs::write(repo_root.join(".gitignore"), ".graphzero/\n")
        .expect("write graph fixture .gitignore");
    git_commit_all(&repo_root);
    let store_root = repo_root.join(".graphzero");
    indexer::index_repo(&repo_root, &store_root).expect("index graph fixture");
    BasicGraphFixture {
        dir,
        repo_root,
        store_root,
    }
}

pub struct ReserveFixture {
    pub dir: TempDir,
    pub repo_root: PathBuf,
    pub store_root: PathBuf,
}

pub fn reserve_indexed_fixture() -> ReserveFixture {
    let dir = tempfile::tempdir().expect("create reserve fixture directory");
    let repo_root = dir.path().join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("create reserve fixture src directory");
    fs::write(
        repo_root.join("src/parse_ref.rs"),
        "pub fn parse_ref(input: &str) -> usize {\n    helper(input)\n}\n\nfn helper(s: &str) -> usize {\n    s.len()\n}\n",
    )
    .expect("write reserve parse_ref fixture");
    fs::write(
        repo_root.join("src/caller_a.rs"),
        "use crate::parse_ref::parse_ref;\n\npub fn use_parse_ref(x: &str) -> usize {\n    parse_ref(x)\n}\n",
    )
    .expect("write reserve caller fixture");
    fs::write(
        repo_root.join("src/config_loader.rs"),
        "pub fn load_config() -> usize {\n    0\n}\n",
    )
    .expect("write reserve config fixture");
    fs::write(
        repo_root.join("src/lib.rs"),
        "pub mod parse_ref;\npub mod caller_a;\npub mod config_loader;\n",
    )
    .expect("write reserve lib fixture");
    let store_root = repo_root.join(".graphzero");
    indexer::index_repo(&repo_root, &store_root).expect("index reserve fixture");
    ReserveFixture {
        dir,
        repo_root,
        store_root,
    }
}
