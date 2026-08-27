use std::fs;
use std::path::PathBuf;

use graphzero_reserve::IntentOperation;
use graphzero_store::store::indexer;
use tempfile::TempDir;

pub struct ReserveFixture {
    pub dir: TempDir,
    pub repo_root: PathBuf,
    pub store_root: PathBuf,
}

pub fn reserve_indexed_fixture() -> ReserveFixture {
    let dir =
        tempfile::tempdir().expect("failed to create temporary directory for reserve fixture");
    let repo_root = dir.path().join("repo");
    fs::create_dir_all(repo_root.join("src"))
        .expect("failed to create src directory for reserve fixture");
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
    .expect("failed to write source file for reserve fixture");
    fs::write(
        repo_root.join("src/caller_a.rs"),
        r#"use crate::parse_ref::parse_ref;

pub fn use_parse_ref(x: &str) -> usize {
    parse_ref(x)
}
"#,
    )
    .expect("failed to write source file for reserve fixture");
    fs::write(
        repo_root.join("src/config_loader.rs"),
        r#"pub fn load_config() -> usize {
    0
}
"#,
    )
    .expect("failed to write source file for reserve fixture");
    fs::write(
        repo_root.join("src/lib.rs"),
        r#"pub mod parse_ref;
pub mod caller_a;
pub mod config_loader;
"#,
    )
    .expect("failed to write source file for reserve fixture");
    let store_root = repo_root.join(".graphzero");
    indexer::index_repo(&repo_root, &store_root)
        .expect("failed to index reserve fixture repository");
    ReserveFixture {
        dir,
        repo_root,
        store_root,
    }
}

pub fn parse_ref_ops() -> Vec<IntentOperation> {
    vec![IntentOperation {
        kind: "change_signature".into(),
        target_symbol: Some("parse_ref".into()),
        intent_text: Some("change signature of parse_ref".into()),
    }]
}

pub fn use_parse_ref_ops() -> Vec<IntentOperation> {
    vec![IntentOperation {
        kind: "change_signature".into(),
        target_symbol: Some("use_parse_ref".into()),
        intent_text: Some("change signature of use_parse_ref".into()),
    }]
}

pub fn load_config_ops() -> Vec<IntentOperation> {
    vec![IntentOperation {
        kind: "change_signature".into(),
        target_symbol: Some("load_config".into()),
        intent_text: Some("change signature of load_config".into()),
    }]
}
