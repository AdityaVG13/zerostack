use std::fs;
use std::path::{Path, PathBuf};

/// Default walk file cap. The EFFECTIVE cap (env-overridable) is
/// walk_max_files() — watch-mode rescans use it to tell a truncated walk
/// from a complete one (removal detection is only sound when complete).
///
/// 100k is the product northstar: a large monorepo must still snap-to-file
/// and list instantly. The previous 10k default silently truncated those
/// trees, so incremental index + stale-row GC never saw the rest.
pub const WALK_MAX_FILES: usize = 100_000;

/// FSZERO_INDEX_MAX_FILES overrides the default walk cap.
pub fn walk_max_files() -> usize {
    crate::env_usize("FSZERO_INDEX_MAX_FILES").unwrap_or(WALK_MAX_FILES)
}

#[derive(Debug)]
pub struct WalkReport {
    pub files: Vec<(PathBuf, fs::Metadata)>,
    pub truncated: bool,
}

pub fn walk_rs_files(root: &Path) -> Vec<(PathBuf, fs::Metadata)> {
    walk_rs_files_with_report(root).files
}

pub fn walk_rs_files_with_report(root: &Path) -> WalkReport {
    let skip_gitignore = matches!(
        std::env::var("FSZERO_SKIP_GITIGNORE").ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE")
    );
    walk_rs_files_with_options(root, walk_max_files(), skip_gitignore)
}

fn walk_rs_files_with_options(root: &Path, max_files: usize, skip_gitignore: bool) -> WalkReport {
    const MAX_DEPTH: usize = 64;
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .max_depth(Some(MAX_DEPTH))
        .follow_links(false)
        .hidden(false)
        .require_git(false)
        .git_ignore(!skip_gitignore)
        .git_exclude(!skip_gitignore)
        .git_global(!skip_gitignore)
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_some_and(|kind| kind.is_dir())
                || !entry.file_name().to_str().is_some_and(is_ignored_dir_name)
        });

    let mut files = Vec::with_capacity(max_files.min(4096));
    let mut truncated = false;
    for entry in builder.build().flatten() {
        if entry.depth() == 0 {
            continue;
        }
        let path = entry.into_path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink()
            || !meta.is_file()
            || !is_supported_source_entry(&path, &meta)
        {
            continue;
        }
        if files.len() == max_files {
            truncated = true;
            break;
        }
        files.push((path, meta));
    }
    WalkReport { files, truncated }
}

pub fn is_supported_source_file(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    is_supported_source_entry(path, &meta)
}

pub fn is_supported_source_entry(path: &Path, meta: &fs::Metadata) -> bool {
    const MAX_INDEX_FILE_BYTES: u64 = 2 * 1024 * 1024;
    const SKIP_EXTS: &[&str] = &[
        "a", "bin", "bmp", "class", "dylib", "exe", "gif", "ico", "jar", "jpeg", "jpg", "lockb",
        "mp3", "mp4", "o", "pdf", "png", "rlib", "rmeta", "so", "sqlite", "sqlite3", "wasm",
        "webp", "zip",
    ];
    if meta.len() > MAX_INDEX_FILE_BYTES {
        return false;
    }
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return true;
    };
    !SKIP_EXTS.contains(&ext.to_ascii_lowercase().as_str())
}

pub fn looks_binary_bytes(content: &[u8]) -> bool {
    content[..content.len().min(8000)].contains(&0)
}

pub use super::adapter::{is_structural_file_key, is_structural_path as is_structural_source_file};

/// Directory names the index walk never descends into (shared with the
/// watch-mode event filter).
pub fn is_ignored_dir_name(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".hg"
            | ".svn"
            | ".asgrep"
            | ".beads"
            | ".fszero"
            | ".greplm"
            | ".graphzero"
            | ".mypy_cache"
            | ".pytest_cache"
            | ".ruff_cache"
            | ".tokenzero"
            | ".venv"
            | ".zerostack"
            | "__pycache__"
            | "target"
            | "node_modules"
            | "build"
            | "dist"
            | "venv"
    )
}

/// Fast path for walk entries, which always live under the original `root`
/// prefix: skips the two `fs::canonicalize` syscalls per file.
pub fn relative_file_key_fast(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) => rel.display().to_string(),
        Err(_) => relative_file_key(root, path),
    }
}

pub fn relative_file_key(root: &Path, path: &Path) -> String {
    let root_canon = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let path_canon = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    path_canon
        .strip_prefix(&root_canon)
        .unwrap_or(&path_canon)
        .display()
        .to_string()
}
