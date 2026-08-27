//! Focused worktree walking and path helpers for the repo indexer.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::path_safety::file_name_to_str;

const SKIP_DIRS: &[&str] = &[".git", ".graphzero", "target", "node_modules", ".venv"];

pub(super) fn looks_binary(content: &[u8]) -> bool {
    content[..content.len().min(8000)].contains(&0)
}

pub(super) fn walk_files(root: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(root)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let file_name = entry.file_name();
        let name = file_name_to_str(&file_name, "indexer walk")?;
        // file_type() uses the dirent type when the FS provides it (APFS does),
        // avoiding one lstat per entry on the warm-index walk.
        let is_dir = match entry.file_type() {
            Ok(ft) => ft.is_dir(),
            Err(_) => path.is_dir(),
        };
        if is_dir {
            if !SKIP_DIRS.contains(&name) && !name.starts_with('.') {
                walk_files(&path, out)?;
            }
        } else if !name.starts_with('.') && entry.file_type().map(|ft| ft.is_file()).unwrap_or(true)
        {
            out.push(path);
        }
    }
    Ok(())
}

pub(super) fn rel_path_string(repo_root: &Path, path: &Path) -> Result<String> {
    let rel = path.strip_prefix(repo_root).unwrap_or(path);
    let text = rel
        .to_str()
        .with_context(|| format!("indexer path is not UTF-8: {}", rel.display()))?;
    Ok(text.trim_start_matches("./").to_string())
}
