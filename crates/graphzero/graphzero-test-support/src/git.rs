use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static UNIQUE_SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Isolated session id for tests touching the process-global DEFAULT session ledger.
pub fn unique_session_id(label: &str) -> String {
    format!(
        "{label}-{}-{}",
        std::process::id(),
        UNIQUE_SESSION_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// `git init` + commit everything so tracked blobs resolve via git fallback.
pub fn git_commit_all(repo_root: &Path) {
    let repo = git2::Repository::init(repo_root).expect("git init");
    let mut index = repo
        .index()
        .expect("failed to open git index for fixture repository");
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .expect("failed to update git fixture repository");
    index
        .write()
        .expect("failed to write git index for fixture repository");
    let tree_id = index
        .write_tree()
        .expect("failed to write git tree for fixture commit");
    let tree = repo
        .find_tree(tree_id)
        .expect("failed to load git tree for fixture commit");
    let sig = git2::Signature::now("test", "test@example.com")
        .expect("failed to create git signature for fixture commit");
    repo.commit(Some("HEAD"), &sig, &sig, "fixture", &tree, &[])
        .expect("failed to update git fixture repository");
}
