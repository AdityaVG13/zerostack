//! Compact command core.

use std::path::Path;

use anyhow::Result;
use graphzero_store::store::compaction;

use super::paths::{canonical_repo, store_root};

pub fn run(repo: &Path) -> Result<String> {
    let repo = canonical_repo(repo)?;
    let root = store_root(&repo);
    let id = compaction::compact(&root)?;
    Ok(format!("{{\"compacted_snapshot\":{id}}}"))
}
