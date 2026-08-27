//! Pre-index keyword matching over the worktree (graphzero-rle1).
//!
//! Graph structure queries still need a snapshot. Plain keyword locate/search
//! answers from exact text match at query time so cold clones are usable.

use std::fs;
use std::path::Path;

use super::types::{CoverageFooter, QuerySurfaceRequest, QuerySurfaceResponse, SearchHit};

/// Same skip set as `graphzero-store` indexer_walk (keep in lockstep).
const SKIP_DIRS: &[&str] = &[".git", ".graphzero", "target", "node_modules", ".venv"];
const MAX_FILE_BYTES: usize = 1_048_576;
const MAX_HITS: usize = 64;

pub fn keyword_surface(surface: &str) -> bool {
    matches!(surface, "search" | "locate" | "word")
}

pub fn worktree_keyword_response(
    repo_root: &Path,
    req: &QuerySurfaceRequest,
) -> Option<QuerySurfaceResponse> {
    let needle = req
        .query
        .as_deref()
        .or(req.name.as_deref())
        .or(req.path.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let mut files = Vec::new();
    walk_files(repo_root, &mut files);
    files.sort();
    let mut hits = Vec::new();
    if req.surface == "locate" && looks_like_path(needle) {
        path_hits(repo_root, &files, needle, &mut hits);
    }
    if hits.is_empty() {
        text_hits(repo_root, &files, needle, &mut hits);
    }
    hits.truncate(MAX_HITS);
    let truncated = hits.len() >= MAX_HITS;
    let decl_ref = hits.first().map(|h| h.evidence_ref.clone());
    Some(QuerySurfaceResponse {
        schema_version: 1,
        surface: req.surface.clone(),
        coverage: CoverageFooter {
            tier_a: 0.0,
            tier_b: 0.0,
            tier_c: 0.0,
            freshness_verified: false,
            snapshot_id: 0,
        },
        hits,
        decl_ref: if req.surface == "locate" {
            decl_ref
        } else {
            None
        },
        truncated: truncated.then_some(true),
        ..Default::default()
    })
}

fn looks_like_path(needle: &str) -> bool {
    needle.contains('/') || needle.contains('\\') || needle.contains('.')
}

fn walk_files(root: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if path.is_dir() {
            if !SKIP_DIRS.contains(&name) && !name.starts_with('.') {
                walk_files(&path, out);
            }
        } else if path.is_file() && !name.starts_with('.') {
            out.push(path);
        }
    }
}

fn rel_path(repo_root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(repo_root).unwrap_or(path);
    let text = rel.to_str()?;
    Some(text.trim_start_matches("./").replace('\\', "/"))
}

fn path_hits(
    repo_root: &Path,
    files: &[std::path::PathBuf],
    needle: &str,
    hits: &mut Vec<SearchHit>,
) {
    let needle = needle.replace('\\', "/");
    for path in files {
        let Some(rel) = rel_path(repo_root, path) else {
            continue;
        };
        if rel == needle || rel.ends_with(&needle) || rel.contains(&needle) {
            hits.push(SearchHit {
                label: rel.clone(),
                snippet: rel.clone(),
                content_sha256: String::new(),
                evidence_ref: format!("gz://path/{rel}#L1-1"),
                source: "worktree".into(),
            });
            if hits.len() >= MAX_HITS {
                return;
            }
        }
    }
}

fn text_hits(
    repo_root: &Path,
    files: &[std::path::PathBuf],
    needle: &str,
    hits: &mut Vec<SearchHit>,
) {
    for path in files {
        if hits.len() >= MAX_HITS {
            return;
        }
        let Ok(meta) = fs::metadata(path) else {
            continue;
        };
        if meta.len() as usize > MAX_FILE_BYTES {
            continue;
        }
        let Ok(bytes) = fs::read(path) else {
            continue;
        };
        if bytes.contains(&0) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let Some(rel) = rel_path(repo_root, path) else {
            continue;
        };
        for (idx, line) in text.split('\n').enumerate() {
            if !line.contains(needle) {
                continue;
            }
            let line_no = idx + 1;
            hits.push(SearchHit {
                label: format!("{rel}:{line_no}"),
                snippet: line.chars().take(200).collect(),
                content_sha256: String::new(),
                evidence_ref: format!("gz://path/{rel}#L{line_no}-{line_no}"),
                source: "worktree".into(),
            });
            if hits.len() >= MAX_HITS {
                return;
            }
        }
    }
}
