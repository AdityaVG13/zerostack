//! Shared path helpers for CLI and MCP.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use graphzero_store::resolve_graphzero_store_root;

pub fn store_root(repo: &Path) -> PathBuf {
    resolve_graphzero_store_root(repo)
}

pub fn canonical_repo(repo: impl AsRef<Path>) -> Result<PathBuf> {
    repo.as_ref().canonicalize().context("repo path")
}

pub fn repo_pair(repo: PathBuf) -> Result<(PathBuf, PathBuf)> {
    let repo = canonical_repo(repo)?;
    Ok((store_root(&repo), repo))
}
