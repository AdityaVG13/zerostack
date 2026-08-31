//! Path-confinement regressions for missing-file rollback.
//! Embedded `..` after a normal component must remain outside the accepted root.

use fszero_store::path::{canonical_path_within_root, lexical_normalize, validate_rollback_path};
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn rollback_rejects_embedded_dotdot_escape() {
    let root = tempfile::tempdir().unwrap();
    let outside = root.path().parent().unwrap().join("OUTSIDE_OF_ROOT");
    let attack = PathBuf::from("nested/../../").join(outside.file_name().unwrap());
    let err = validate_rollback_path(root.path(), &attack).expect_err("must reject escape");
    assert!(
        err.contains("outside root") || err.contains("parent"),
        "unexpected err: {err}"
    );
}

#[test]
fn rollback_rejects_sub_dotdot_dotdot_secret() {
    let root = tempfile::tempdir().unwrap();
    let attack = Path::new("sub/../../secret");
    let err = validate_rollback_path(root.path(), attack).expect_err("must reject");
    assert!(err.contains("outside root"), "unexpected err: {err}");
}

#[test]
fn rollback_accepts_missing_nested_path_under_root() {
    let root = tempfile::tempdir().unwrap();
    let missing = Path::new("a/b/c.txt");
    let got = validate_rollback_path(root.path(), missing).expect("in-root missing path ok");
    let expect = fs::canonicalize(root.path()).unwrap().join("a/b/c.txt");
    assert!(got.ends_with("a/b/c.txt") || got == expect);
    assert!(canonical_path_within_root(
        &fs::canonicalize(root.path()).unwrap(),
        &lexical_normalize(&got)
    ));
}

#[test]
fn lexical_normalize_collapses_dotdot() {
    let p = Path::new("/proj/nested/../../secret");
    let n = lexical_normalize(p);
    assert_eq!(n, Path::new("/secret"));
}

#[test]
fn within_root_rejects_parent_dir_in_rest() {
    let root = Path::new("/workspace/root");
    let bad = Path::new("/workspace/root/sub/../../secret");
    assert!(!canonical_path_within_root(root, bad));
    let good = Path::new("/workspace/root/a/b");
    assert!(canonical_path_within_root(root, good));
}

#[cfg(unix)]
#[test]
fn gitignore_update_refuses_fifo_without_opening_it() {
    use std::os::unix::fs::FileTypeExt;
    use std::time::Duration;
    use zerostack_test_support::{TempWorkspace, assert_completes_within, make_fifo};

    let workspace = TempWorkspace::new("fszero-gitignore-fifo-").unwrap();
    let gitignore = workspace.path(".gitignore");
    make_fifo(&gitignore);
    let root = workspace.root().to_path_buf();

    let error = assert_completes_within(
        "gitignore-fifo-rejection",
        Duration::from_millis(1_500),
        move || {
            fszero_store::zerostack_store::ensure_repo_gitignore(&root, true)
                .expect_err("FIFO .gitignore must fail closed")
        },
    );

    assert!(error.contains("unsupported file kind") && error.contains("fifo"));
    assert!(
        std::fs::symlink_metadata(gitignore)
            .unwrap()
            .file_type()
            .is_fifo()
    );
}

#[test]
fn gitignore_update_writes_regular_file() {
    let workspace = zerostack_test_support::TempWorkspace::new("fszero-gitignore-").unwrap();
    fszero_store::zerostack_store::ensure_repo_gitignore(workspace.root(), true).unwrap();
    let text = std::fs::read_to_string(workspace.path(".gitignore")).unwrap();
    assert!(text.lines().any(|line| line.trim() == ".zerostack/"));
}

#[cfg(unix)]
#[test]
fn edit_intent_reconciliation_refuses_fifo_without_opening_it() {
    use std::os::unix::fs::FileTypeExt;
    use std::time::Duration;
    use zerostack_test_support::{TempWorkspace, assert_completes_within, make_fifo};

    let workspace = TempWorkspace::new("fszero-edit-intent-fifo-").unwrap();
    let root = workspace.create_dir_all("workspace").unwrap();
    let fifo = root.join("pipe.fifo");
    make_fifo(&fifo);
    let db = workspace.path("store.sqlite3");
    let root_text = root.to_str().expect("UTF-8 workspace path").to_owned();

    let error = assert_completes_within(
        "edit-intent-fifo-rejection",
        Duration::from_millis(1_500),
        move || {
            let store = fszero_store::recovery::RecoveryStore::with_durable(&db);
            store
                .create_edit_intent(&root_text, "pipe.fifo", b"pre", b"post", "", "", 0, 0, "{}")
                .expect("insert edit intent");
            store
                .reconcile_edit_intents(std::path::Path::new(&root_text))
                .expect_err("FIFO reconcile must fail closed")
        },
    );

    assert!(error.contains("unsupported file kind") && error.contains("fifo"));
    assert!(
        std::fs::symlink_metadata(fifo)
            .unwrap()
            .file_type()
            .is_fifo()
    );
}
