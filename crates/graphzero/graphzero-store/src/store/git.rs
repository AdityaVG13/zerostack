//! Git HEAD and index reader with branch repointing.
//! No network access; dirty paths come from the index vs HEAD tree.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};
use git2::{Repository, StatusOptions, StatusShow, TreeWalkMode};

use crate::ContentHash;

use super::manifest::{Manifest, SnapshotEntry};
use super::overlay;

/// Warm-daemon test shim: increments when the index is touched.
static REFRESH_COUNT: AtomicUsize = AtomicUsize::new(0);

pub fn refresh_count() -> usize {
    REFRESH_COUNT.load(Ordering::SeqCst)
}

pub fn reset_refresh_count() {
    REFRESH_COUNT.store(0, Ordering::SeqCst);
}

/// Stem hook: call when git index or HEAD changes are observed.
pub fn notify_view_refresh() {
    REFRESH_COUNT.fetch_add(1, Ordering::SeqCst);
}

/// Record that the index was modified (tests simulate daemon wake).
pub fn notify_index_touched(_repo_root: &Path) -> Result<()> {
    notify_view_refresh();
    Ok(())
}

pub struct GitReader {
    repo: Repository,
}

impl GitReader {
    pub fn open(repo_root: &Path) -> Result<Self> {
        let repo = Repository::discover(repo_root).context("git discover")?;
        Ok(Self { repo })
    }

    /// Current HEAD commit oid (hex), matching `git rev-parse HEAD`.
    pub fn head_oid(&self) -> Result<String> {
        let oid = self
            .repo
            .head()
            .context("read HEAD")?
            .peel_to_commit()
            .context("peel HEAD")?
            .id();
        Ok(oid.to_string())
    }

    /// Tracked paths whose index entry differs from HEAD tree (stage-0 only).
    pub fn dirty_paths(&self) -> Result<Vec<String>> {
        let mut opts = StatusOptions::new();
        opts.include_untracked(false)
            .recurse_untracked_dirs(false)
            .show(StatusShow::Index);
        let statuses = self.repo.statuses(Some(&mut opts))?;
        let mut out = Vec::new();
        for entry in statuses.iter() {
            if let Ok(path) = entry.path() {
                out.push(path.replace('\\', "/"));
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }
}

pub(crate) fn join_tree_path(root: &str, name: &str) -> String {
    let root = root.trim_end_matches('/');
    if root.is_empty() {
        name.to_string()
    } else {
        format!("{root}/{name}")
    }
    .replace('\\', "/")
}

/// Map repo-relative path -> git blob oid hex for the current HEAD tree.
pub fn head_tree_blob_hashes(repo_root: &Path) -> Result<BTreeMap<String, String>> {
    let repo = Repository::discover(repo_root).context("git discover")?;
    let head = repo.head()?.peel_to_tree()?;
    let mut out = BTreeMap::new();
    head.walk(TreeWalkMode::PreOrder, |root, entry| {
        if entry.kind() == Some(git2::ObjectType::Blob) {
            let name = entry.name().unwrap_or("");
            let path = join_tree_path(root, name);
            if !path.is_empty() {
                out.insert(path, entry.id().to_string());
            }
        }
        git2::TreeWalkResult::Ok
    })?;
    Ok(out)
}

fn looks_binary(content: &[u8]) -> bool {
    content[..content.len().min(8000)].contains(&0)
}

const MAX_INDEXABLE_BLOB_BYTES: usize = 4 * 1024 * 1024;

fn blob_content_indexable(content: &[u8]) -> bool {
    !content.is_empty() && !looks_binary(content) && content.len() <= MAX_INDEXABLE_BLOB_BYTES
}

fn insert_head_blob_hash(
    repo: &Repository,
    root: &str,
    entry: &git2::TreeEntry<'_>,
    out: &mut BTreeMap<String, String>,
) {
    if entry.kind() != Some(git2::ObjectType::Blob) {
        return;
    }
    let name = entry.name().unwrap_or("");
    let path = join_tree_path(root, name);
    if path.is_empty() {
        return;
    }
    let Ok(blob) = repo.find_blob(entry.id()) else {
        return;
    };
    let content = blob.content();
    if !blob_content_indexable(content) {
        return;
    }
    out.insert(path, ContentHash::of(content).to_hex());
}

/// Path -> content-hash hex for blobs in the current HEAD tree, using the
/// same skip rules as `indexer::collect` (empty/binary/oversized omitted).
pub fn head_tree_content_hashes(repo_root: &Path) -> Result<BTreeMap<String, String>> {
    let repo = Repository::discover(repo_root).context("git discover")?;
    let head = repo.head()?.peel_to_tree()?;
    let mut out = BTreeMap::new();
    head.walk(TreeWalkMode::PreOrder, |root, entry| {
        insert_head_blob_hash(&repo, root, entry, &mut out);
        git2::TreeWalkResult::Ok
    })?;
    Ok(out)
}

pub fn record_head_snapshot(store_root: &Path, repo_root: &Path, snapshot_id: u64) -> Result<()> {
    let Ok(reader) = GitReader::open(repo_root) else {
        return Ok(());
    };
    let head = reader.head_oid()?;
    let dir = store_root.join("branches");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(head), snapshot_id.to_string())?;
    Ok(())
}

/// Snapshot ids referenced by branch pointers. Branch mappings are durable
/// navigation roots, so snapshot rotation must treat every valid id as pinned.
pub fn branch_snapshot_ids(store_root: &Path) -> Result<BTreeSet<u64>> {
    let branches = store_root.join("branches");
    if !branches.is_dir() {
        return Ok(BTreeSet::new());
    }

    let mut ids = BTreeSet::new();
    for entry in std::fs::read_dir(&branches).context("read branch pointers")? {
        let entry = entry.context("read branch pointer entry")?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let value = std::fs::read_to_string(entry.path())
            .with_context(|| format!("read branch pointer {}", entry.path().display()))?;
        let snapshot_id = value
            .trim()
            .parse::<u64>()
            .with_context(|| format!("parse branch pointer {}", entry.path().display()))?;
        ids.insert(snapshot_id);
    }
    Ok(ids)
}

/// Branch pointers whose snapshot is absent from the manifest or whose
/// published snapshot artifacts are incomplete. Intended for doctor/fsck
/// surfaces as well as focused integrity tests.
pub fn dangling_branch_pointers(
    store_root: &Path,
    manifest: &Manifest,
) -> Result<Vec<(String, u64)>> {
    let branches = store_root.join("branches");
    if !branches.is_dir() {
        return Ok(Vec::new());
    }

    let mut dangling = Vec::new();
    for entry in std::fs::read_dir(&branches).context("read branch pointers")? {
        let entry = entry.context("read branch pointer entry")?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let branch = entry.file_name().to_string_lossy().into_owned();
        let value = std::fs::read_to_string(entry.path())
            .with_context(|| format!("read branch pointer {}", entry.path().display()))?;
        let snapshot_id = value
            .trim()
            .parse::<u64>()
            .with_context(|| format!("parse branch pointer {}", entry.path().display()))?;
        let complete = manifest
            .snapshots
            .iter()
            .find(|snapshot| snapshot.snapshot_id == snapshot_id)
            .is_some_and(|snapshot| {
                let shards = store_root.join("shards");
                shards
                    .join(super::indexer::global_file_name(snapshot_id))
                    .is_file()
                    && shards
                        .join(super::indexer::paths_file_name(snapshot_id))
                        .is_file()
                    && (0..snapshot.shard_hashes.len()).all(|index| {
                        shards
                            .join(super::indexer::shard_file_name(snapshot_id, index))
                            .is_file()
                    })
            });
        if !complete {
            dangling.push((branch, snapshot_id));
        }
    }
    dangling.sort();
    Ok(dangling)
}

fn snapshot_id_for_head(store_root: &Path, repo_root: &Path) -> Result<Option<u64>> {
    let head = GitReader::open(repo_root)?.head_oid()?;
    let path = store_root.join("branches").join(head);
    let Ok(txt) = std::fs::read_to_string(path) else {
        return Ok(None);
    };
    Ok(txt.trim().parse().ok())
}

fn paths_sidecar_hashes(store_root: &Path, snapshot_id: u64) -> Result<BTreeMap<String, String>> {
    let path = store_root
        .join("shards")
        .join(super::indexer::paths_file_name(snapshot_id));
    let txt = std::fs::read_to_string(path).context("read paths sidecar")?;
    let mut out = BTreeMap::new();
    for line in txt.lines() {
        let mut parts = line.splitn(5, ' ');
        let (Some(hash_hex), _, _, _, Some(rel)) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            continue;
        };
        out.insert(rel.to_string(), hash_hex.to_string());
    }
    Ok(out)
}

fn snapshot_matches_tree(
    store_root: &Path,
    entry: &SnapshotEntry,
    live: &BTreeMap<String, String>,
) -> Result<bool> {
    let indexed = paths_sidecar_hashes(store_root, entry.snapshot_id)?;
    Ok(indexed == *live)
}

fn write_active_head_marker(store_root: &Path, repo_root: &Path) -> Result<()> {
    let head = GitReader::open(repo_root)?.head_oid()?;
    std::fs::write(store_root.join("active_head"), head)?;
    Ok(())
}

fn promote_snapshot_in_manifest(
    manifest: &mut Manifest,
    id: u64,
    missing_msg: &'static str,
) -> Result<()> {
    let entry = manifest
        .snapshots
        .iter()
        .find(|s| s.snapshot_id == id)
        .cloned()
        .with_context(|| missing_msg)?;
    manifest.snapshots.retain(|s| s.snapshot_id != id);
    manifest.snapshots.push(entry);
    manifest.snapshots.sort_by_key(|s| s.snapshot_id);
    while manifest.snapshots.len() > 2 {
        manifest.snapshots.remove(0);
    }
    Ok(())
}

/// Essential: when `id` is already latest, only refresh `active_head` (no manifest rewrite).
fn finalize_repoint(
    store_root: &Path,
    repo_root: &Path,
    manifest: &mut Manifest,
    id: u64,
    missing_msg: &'static str,
) -> Result<Option<u64>> {
    if manifest.latest().map(|s| s.snapshot_id) == Some(id) {
        write_active_head_marker(store_root, repo_root)?;
        return Ok(Some(id));
    }
    promote_snapshot_in_manifest(manifest, id, missing_msg)?;
    manifest.atomic_publish(store_root)?;
    write_active_head_marker(store_root, repo_root)?;
    record_head_snapshot(store_root, repo_root, id)?;
    Ok(Some(id))
}

fn snapshot_id_matching_head_tree(
    store_root: &Path,
    manifest: &Manifest,
    tree_paths: &BTreeMap<String, String>,
) -> Result<Option<u64>> {
    for entry in manifest.snapshots.iter().rev() {
        if snapshot_matches_tree(store_root, entry, tree_paths)? {
            return Ok(Some(entry.snapshot_id));
        }
    }
    Ok(None)
}

fn try_repoint_from_head_branch_map(store_root: &Path, repo_root: &Path) -> Result<Option<u64>> {
    let Some(id) = snapshot_id_for_head(store_root, repo_root)? else {
        return Ok(None);
    };
    let manifest = Manifest::load(store_root)?;
    if !manifest.snapshots.iter().any(|s| s.snapshot_id == id) {
        return Ok(None);
    }
    finalize_repoint(
        store_root,
        repo_root,
        &mut Manifest::load(store_root)?,
        id,
        "snapshot missing for head mapping",
    )
}

/// Re-point the active manifest entry when HEAD tree paths match an existing
/// snapshot without running `collect` / full extraction.
pub fn repoint_active_snapshot(store_root: &Path, repo_root: &Path) -> Result<Option<u64>> {
    if let Some(id) = try_repoint_from_head_branch_map(store_root, repo_root)? {
        return Ok(Some(id));
    }

    // Match only HEAD. Worktree walks include untracked files from later checkouts
    // that cannot match an older snapshot.
    let tree_paths = head_tree_content_hashes(repo_root)?;
    let mut manifest = Manifest::load(store_root)?;
    if manifest.snapshots.is_empty() {
        return Ok(None);
    }
    let Some(id) = snapshot_id_matching_head_tree(store_root, &manifest, &tree_paths)? else {
        return Ok(None);
    };
    finalize_repoint(
        store_root,
        repo_root,
        &mut manifest,
        id,
        "matched snapshot missing",
    )
}

/// Index dirty tracked paths into the worktree overlay.
pub fn sync_dirty_overlay(
    store_root: &Path,
    worktree_id: &str,
    repo_root: &Path,
) -> Result<Vec<String>> {
    let reader = GitReader::open(repo_root)?;
    let dirty = reader.dirty_paths()?;
    if dirty.is_empty() {
        return Ok(dirty);
    }
    let refs: Vec<&str> = dirty.iter().map(|s| s.as_str()).collect();
    overlay::index_worktree_files(store_root, worktree_id, repo_root, &refs)?;
    Ok(dirty)
}
