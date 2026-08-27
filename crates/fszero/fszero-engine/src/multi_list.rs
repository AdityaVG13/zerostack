//! Batched directory listings backed by a session-local directory trie.

use super::FSZeroSession;
use super::list_ops::is_zerostack_store_dir_name;
use super::path::sanitize_relative_arg;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListManyItem {
    pub path: String,
    #[serde(default = "default_depth")]
    pub depth: usize,
    #[serde(default)]
    pub include_hidden: bool,
}

const fn default_depth() -> usize {
    1
}

impl ListManyItem {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            depth: default_depth(),
            include_hidden: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListCacheStatus {
    Cold,
    Warm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirentRow {
    /// Name relative to the requested directory. Nested names contain slashes.
    pub name: String,
    /// file, directory, symlink, or other.
    pub kind: String,
    pub size: u64,
    pub mtime: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListManyResult {
    pub path: String,
    pub cache_status: ListCacheStatus,
    pub entries: Vec<DirentRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
struct IndexedDirent {
    name: String,
    kind: String,
    size: u64,
    mtime: u64,
    is_directory: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DirectoryIndex {
    directories: BTreeMap<String, Vec<IndexedDirent>>,
}

impl DirectoryIndex {
    fn build(root: &Path) -> Result<Self, String> {
        let mut index = Self::default();
        index.walk(root, ".")?;
        Ok(index)
    }

    fn walk(&mut self, directory: &Path, key: &str) -> Result<(), String> {
        let read = fs::read_dir(directory)
            .map_err(|error| format!("list {} failed: {error}", directory.display()))?;
        let mut entries = Vec::new();
        let mut children = Vec::<(PathBuf, String)>::new();
        for entry in read {
            let entry =
                entry.map_err(|error| format!("list {} failed: {error}", directory.display()))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("metadata {} failed: {error}", path.display()))?;
            let file_type = metadata.file_type();
            let is_directory = file_type.is_dir();
            if is_directory && is_zerostack_store_dir_name(&name) {
                continue;
            }
            let kind = if is_directory {
                "directory"
            } else if file_type.is_file() {
                "file"
            } else if file_type.is_symlink() {
                "symlink"
            } else {
                "other"
            };
            let mtime = metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_nanos().min(u64::MAX as u128) as u64)
                .unwrap_or(0);
            entries.push(IndexedDirent {
                name: name.clone(),
                kind: kind.to_string(),
                size: metadata.len(),
                mtime,
                is_directory,
            });
            if is_directory {
                let child_key = join_key(key, &name);
                children.push((path, child_key));
            }
        }
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        self.directories.insert(key.to_string(), entries);
        children.sort_by(|left, right| left.1.cmp(&right.1));
        for (path, child_key) in children {
            self.walk(&path, &child_key)?;
        }
        Ok(())
    }

    fn list(
        &self,
        path: &str,
        depth: usize,
        include_hidden: bool,
    ) -> Result<Vec<DirentRow>, String> {
        if !self.directories.contains_key(path) {
            return Err(format!("not a directory: {path}"));
        }
        let mut rows = Vec::new();
        self.collect(path, "", depth, include_hidden, &mut rows);
        Ok(rows)
    }

    fn collect(
        &self,
        key: &str,
        prefix: &str,
        depth: usize,
        include_hidden: bool,
        rows: &mut Vec<DirentRow>,
    ) {
        if depth == 0 {
            return;
        }
        let Some(entries) = self.directories.get(key) else {
            return;
        };
        for entry in entries {
            if !include_hidden && entry.name.starts_with('.') {
                continue;
            }
            let name = if prefix.is_empty() {
                entry.name.clone()
            } else {
                format!("{prefix}/{}", entry.name)
            };
            rows.push(DirentRow {
                name: name.clone(),
                kind: entry.kind.clone(),
                size: entry.size,
                mtime: entry.mtime,
            });
            if entry.is_directory && depth > 1 {
                self.collect(
                    &join_key(key, &entry.name),
                    &name,
                    depth - 1,
                    include_hidden,
                    rows,
                );
            }
        }
    }
}

fn join_key(parent: &str, name: &str) -> String {
    if parent == "." {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

fn normalize_path(path: &str) -> Result<String, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "." {
        return Ok(".".to_string());
    }
    let clean = sanitize_relative_arg(trimmed)?;
    let value = clean.to_string_lossy().replace('\\', "/");
    Ok(value.trim_end_matches('/').to_string())
}

impl FSZeroSession {
    pub fn multi_list(&mut self, items: &[ListManyItem]) -> Vec<ListManyResult> {
        self.drain_watch_events();
        let Some(root) = self.root.clone() else {
            return items
                .iter()
                .map(|item| ListManyResult {
                    path: item.path.clone(),
                    cache_status: ListCacheStatus::Cold,
                    entries: Vec::new(),
                    error: Some("workspace root is not set".to_string()),
                })
                .collect();
        };
        let mut output = Vec::with_capacity(items.len());
        for item in items {
            let path = match normalize_path(&item.path) {
                Ok(path) => path,
                Err(error) => {
                    output.push(ListManyResult {
                        path: item.path.clone(),
                        cache_status: if self.caches.directory_index.is_some() {
                            ListCacheStatus::Warm
                        } else {
                            ListCacheStatus::Cold
                        },
                        entries: Vec::new(),
                        error: Some(error),
                    });
                    continue;
                }
            };
            let cache_status = if self.caches.directory_index.is_some() {
                ListCacheStatus::Warm
            } else {
                match DirectoryIndex::build(&root) {
                    Ok(index) => self.caches.directory_index = Some(index),
                    Err(error) => {
                        output.push(ListManyResult {
                            path: item.path.clone(),
                            cache_status: ListCacheStatus::Cold,
                            entries: Vec::new(),
                            error: Some(error),
                        });
                        continue;
                    }
                }
                ListCacheStatus::Cold
            };
            let (entries, error) = match self
                .caches
                .directory_index
                .as_ref()
                .expect("index built")
                .list(&path, item.depth, item.include_hidden)
            {
                Ok(entries) => (entries, None),
                Err(error) => (Vec::new(), Some(error)),
            };
            output.push(ListManyResult {
                path: item.path.clone(),
                cache_status,
                entries,
                error,
            });
        }
        output
    }
}
