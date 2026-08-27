use std::fs;
use std::path::{Path, PathBuf};

use graphzero_store::store::indexer;
use tempfile::TempDir;

use super::git::git_commit_all;

pub struct BlastFixture {
    pub dir: TempDir,
    pub repo_root: PathBuf,
    pub store_root: PathBuf,
}

pub fn blast_indexed_fixture() -> BlastFixture {
    let dir = tempfile::tempdir().expect("failed to create temporary directory for blast fixture");
    let repo_root = dir.path().join("repo");
    write_blast_repo(repo_root.as_path());
    index_blast_repo(repo_root.as_path());
    let store_root = repo_root.join(".graphzero");
    BlastFixture {
        dir,
        repo_root,
        store_root,
    }
}

/// Blast fixture with git history (store tests that resolve tracked blobs).
pub fn blast_git_indexed_fixture() -> BlastFixture {
    let dir = tempfile::tempdir().expect("failed to create temporary directory for blast fixture");
    let repo_root = dir.path().join("repo");
    write_blast_repo(repo_root.as_path());
    git_commit_all(repo_root.as_path());
    index_blast_repo(repo_root.as_path());
    let store_root = repo_root.join(".graphzero");
    BlastFixture {
        dir,
        repo_root,
        store_root,
    }
}

pub fn blast_indexed_fixture_from(repo_root: &Path) {
    write_blast_repo(repo_root);
    index_blast_repo(repo_root);
}

pub fn write_blast_repo(repo_root: &Path) {
    fs::create_dir_all(repo_root.join("src"))
        .expect("failed to create src directory for blast fixture");
    fs::create_dir_all(repo_root.join("tests"))
        .expect("failed to create tests directory for blast fixture");
    fs::write(
        repo_root.join("src/parse_ref.rs"),
        r#"pub fn parse_ref(input: &str) -> usize {
    helper(input)
}

fn helper(s: &str) -> usize {
    s.len()
}
"#,
    )
    .expect("failed to write source file for blast fixture");
    fs::write(
        repo_root.join("src/config_loader.rs"),
        r#"use std::collections::HashMap;

pub fn load_config() -> HashMap<String, String> {
    let m = HashMap::new();
    let _ = m.get("api_key");
    m
}
"#,
    )
    .expect("failed to write source file for blast fixture");
    fs::write(
        repo_root.join("src/caller_a.rs"),
        r#"use crate::parse_ref::parse_ref;

pub fn use_parse_ref(x: &str) -> usize {
    parse_ref(x)
}
"#,
    )
    .expect("failed to write source file for blast fixture");
    fs::write(
        repo_root.join("src/caller_b.rs"),
        r#"use crate::parse_ref::parse_ref;

pub fn another_caller(v: &str) -> usize {
    parse_ref(v) + 1
}
"#,
    )
    .expect("failed to write source file for blast fixture");
    fs::write(
        repo_root.join("src/caller_c.rs"),
        r#"use crate::caller_a::use_parse_ref;

pub fn transitive_caller(s: &str) -> usize {
    use_parse_ref(s)
}
"#,
    )
    .expect("failed to write source file for blast fixture");
    fs::write(
        repo_root.join("src/lib.rs"),
        r#"pub mod parse_ref;
pub mod config_loader;
pub mod caller_a;
pub mod caller_b;
pub mod caller_c;
"#,
    )
    .expect("failed to write source file for blast fixture");
    fs::write(
        repo_root.join("tests/cli.rs"),
        r#"#[test]
fn covers_parse_ref() {
    assert_eq!(graphzero_fixture::parse_ref::parse_ref("ab"), 2);
}
"#,
    )
    .expect("failed to write source file for blast fixture");
    fs::write(
        repo_root.join("Cargo.toml"),
        r#"[package]
name = "graphzero_fixture"
version = "0.0.0"
edition = "2021"
"#,
    )
    .expect("failed to write source file for blast fixture");
}

fn index_blast_repo(repo_root: &Path) {
    indexer::index_repo(repo_root, &repo_root.join(".graphzero"))
        .expect("failed to index blast fixture repository");
}
