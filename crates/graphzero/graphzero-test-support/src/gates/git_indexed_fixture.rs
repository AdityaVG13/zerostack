#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;

use git2::{IndexAddOption, Repository, Signature};
use tempfile::TempDir;

pub struct GitIndexedFixture {
    pub _dir: TempDir,
    pub repo_root: PathBuf,
    pub store_root: PathBuf,
}

fn git_commit(repo_root: &std::path::Path, message: &str) {
    let repo = Repository::discover(repo_root)
        .expect("failed to discover git repository for fixture commit");
    let mut index = repo
        .index()
        .expect("failed to open git index for fixture commit");
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .expect("failed to add fixture files to git index");
    index
        .write()
        .expect("failed to write git index for fixture commit");
    let tree_id = index
        .write_tree()
        .expect("failed to write git tree for fixture commit");
    let tree = repo
        .find_tree(tree_id)
        .expect("failed to load git tree for fixture commit");
    let sig = Signature::now("test", "test@example.com")
        .expect("failed to create git signature for fixture commit");
    let head = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<_> = head.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .expect("failed to create fixture git commit");
}

fn git_history_index_command(
    mut command: std::process::Command,
    repo_root: &std::path::Path,
) -> std::process::Command {
    command
        .arg("index")
        .arg(repo_root)
        .env("GRAPHZERO_INCLUDE_GIT_HISTORY", "1");
    command
}

fn index_with_git_history(repo_root: &std::path::Path, store_root: &std::path::Path) {
    assert_eq!(
        store_root,
        repo_root.join(".graphzero"),
        "git fixture CLI indexing requires the conventional store path"
    );
    let output = git_history_index_command(super::mcp_session::graphzero(), repo_root)
        .output()
        .expect("failed to execute GraphZero index for git fixture");
    assert!(
        output.status.success(),
        "GraphZero git fixture index failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Repo with repeated co-changes on src/a.rs + src/b.rs for tier-C hot/changes surfaces.
pub fn cochange_git_indexed_fixture() -> GitIndexedFixture {
    let dir =
        tempfile::tempdir().expect("failed to create temporary directory for cochange fixture");
    let repo_root = dir.path().join("repo");
    fs::create_dir_all(repo_root.join("src"))
        .expect("failed to create src directory for cochange fixture");
    fs::write(repo_root.join(".gitignore"), ".graphzero/\n")
        .expect("failed to write .gitignore for cochange fixture");
    fs::write(repo_root.join("src/a.rs"), "fn a() {}\n")
        .expect("failed to write initial src/a.rs in cochange fixture");
    fs::write(repo_root.join("src/b.rs"), "fn b() {}\n")
        .expect("failed to write initial src/b.rs in cochange fixture");
    fs::write(repo_root.join("src/orphan.rs"), "fn orphan() {}\n")
        .expect("failed to write src/orphan.rs in cochange fixture");
    fs::write(repo_root.join("src/unrelated.rs"), "fn unrelated() {}\n")
        .expect("failed to write src/unrelated.rs in cochange fixture");

    Repository::init(&repo_root).expect("failed to initialize cochange fixture git repository");
    git_commit(&repo_root, "initial");
    for i in 0..3 {
        fs::write(
            repo_root.join("src/a.rs"),
            format!("fn a() {{ /* {i} */ }}\n"),
        )
        .expect("failed to update cochanged fixture source file");
        fs::write(
            repo_root.join("src/b.rs"),
            format!("fn b() {{ /* {i} */ }}\n"),
        )
        .expect("failed to update cochanged fixture source file");
        git_commit(&repo_root, &format!("co-touch {i}"));
    }
    git_commit(&repo_root, "orphan only");

    let store_root = repo_root.join(".graphzero");
    index_with_git_history(&repo_root, &store_root);
    GitIndexedFixture {
        _dir: dir,
        repo_root,
        store_root,
    }
}
