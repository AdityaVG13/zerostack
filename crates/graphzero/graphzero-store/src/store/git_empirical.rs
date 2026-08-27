//! Tier-C git-empirical edges: co-change, churn/hot-set (P3.2).
//! Read-only git history; evidence spans cite commit-message bytes in the blob store.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use git2::{Commit, DiffOptions, Repository};

use crate::ContentHash;

use super::blob_store::BlobStore;
use super::csr::edge_kind;
use super::git::join_tree_path;
use super::indexer::{DefRecord, EdgeRecord, IndexData};

pub const SOURCE_GIT_HISTORY: &str = "git-history";
pub const MIN_COCHANGE_SUPPORT: usize = 2;
pub const DEFAULT_MAX_COMMITS: usize = 500;
pub const HOT_TOP_K: usize = 50;

type CochangePairMaps = (
    BTreeMap<(String, String), usize>,
    BTreeMap<(String, String), String>,
);

/// Scored path for hot/changes capsules.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct HotPath {
    pub path: String,
    pub churn_score: f64,
    pub content_sha256: String,
}

/// Persisted empirical state (churn + coverage metadata).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct GitEmpiricalState {
    pub churn: BTreeMap<String, f64>,
    pub commits_processed: usize,
    pub history_complete: bool,
    pub tier_c_edge_count: usize,
}

#[derive(Clone, Debug)]
pub struct TierCEdgeDraft {
    pub src_path: String,
    pub dst_path: String,
    pub support: usize,
    pub evidence_blob: ContentHash,
    pub evidence_start: u32,
    pub evidence_end: u32,
    pub confidence: u8,
}

/// Open repo read-only and walk commits reachable from HEAD (FR-001).
pub fn open_readonly(repo_root: &Path) -> Result<Repository> {
    Repository::discover(repo_root).context("git discover")
}

/// HEAD path -> content_sha256 (FR-002), same filters as indexer.
pub fn path_to_content_hash(repo_root: &Path) -> Result<BTreeMap<String, String>> {
    super::git::head_tree_content_hashes(repo_root)
}

fn path_symbol(path: &str) -> String {
    format!("path:{path}")
}

fn confidence_from_support(support: usize) -> u8 {
    let raw = 140u32.saturating_add((support as u32).saturating_mul(25));
    raw.min(255) as u8
}

fn paths_touched_root_commit(tree: &git2::Tree) -> Result<Vec<String>> {
    let mut out = Vec::new();
    tree.walk(git2::TreeWalkMode::PreOrder, |root, entry| {
        if entry.kind() == Some(git2::ObjectType::Blob) {
            let name = entry.name().unwrap_or("");
            let p = join_tree_path(root, name);
            if !p.is_empty() {
                out.push(p);
            }
        }
        git2::TreeWalkResult::Ok
    })?;
    out.sort();
    out.dedup();
    Ok(out)
}

fn paths_touched_diff_commit(
    repo: &Repository,
    parent_tree: &git2::Tree,
    tree: &git2::Tree,
) -> Result<Vec<String>> {
    let mut opts = DiffOptions::new();
    opts.include_typechange(true);
    let diff = repo.diff_tree_to_tree(Some(parent_tree), Some(tree), Some(&mut opts))?;
    let mut out = Vec::new();
    diff.foreach(
        &mut |delta, _| {
            if let Some(path) = delta.new_file().path().or_else(|| delta.old_file().path()) {
                out.push(path.to_string_lossy().replace('\\', "/"));
            }
            true
        },
        None,
        None,
        None,
    )?;
    out.sort();
    out.dedup();
    Ok(out)
}

fn paths_touched_in_commit(repo: &Repository, commit: &Commit) -> Result<Vec<String>> {
    let tree = commit.tree()?;
    if commit.parent_count() == 0 {
        return paths_touched_root_commit(&tree);
    }
    let parent = commit.parent(0)?;
    let parent_tree = parent.tree()?;
    paths_touched_diff_commit(repo, &parent_tree, &tree)
}

struct ReplayEntry {
    id: String,
    unix: u64,
    paths: Vec<String>,
}

fn replay_commits_detailed(repo_root: &Path, max_commits: usize) -> Result<Vec<ReplayEntry>> {
    let repo = open_readonly(repo_root)?;
    let mut walk = repo.revwalk()?;
    walk.push_head()?;
    let mut out = Vec::new();
    for oid in walk {
        if out.len() >= max_commits {
            break;
        }
        let oid = oid?;
        let commit = repo.find_commit(oid)?;
        let unix = commit.time().seconds().max(0) as u64;
        let paths = paths_touched_in_commit(&repo, &commit)?;
        out.push(ReplayEntry {
            id: oid.to_string(),
            unix,
            paths,
        });
    }
    Ok(out)
}

/// Replay commits (newest first), cap at `max_commits` (FR-003).
pub fn replay_commits(repo_root: &Path, max_commits: usize) -> Result<Vec<(String, Vec<String>)>> {
    Ok(replay_commits_detailed(repo_root, max_commits)?
        .into_iter()
        .map(|entry| (entry.id, entry.paths))
        .collect())
}

fn last_commit_unix_map(replay: &[ReplayEntry]) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    for entry in replay {
        for path in &entry.paths {
            out.entry(path.clone()).or_insert(entry.unix);
        }
    }
    out
}

fn store_commit_evidence(
    blob_store: &BlobStore,
    commit: &Commit,
) -> Result<(ContentHash, u32, u32)> {
    let msg = commit.message().unwrap_or("").as_bytes();
    let end = msg.len().min(u32::MAX as usize) as u32;
    if end == 0 {
        let hash = blob_store.put(b"git-empirical-empty-commit")?;
        return Ok((hash, 0, 1));
    }
    let hash = blob_store.put(msg)?;
    Ok((hash, 0, end))
}

/// Mine co-change pairs with min support; emit undirected pair once (FR-004, FR-005).
fn accumulate_cochange_pairs(replay: &[(String, Vec<String>)]) -> CochangePairMaps {
    let mut pair_support: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut pair_commit: BTreeMap<(String, String), String> = BTreeMap::new();
    for (commit_id, paths) in replay {
        let unique: BTreeSet<_> = paths.iter().cloned().collect();
        let list: Vec<_> = unique.into_iter().collect();
        for i in 0..list.len() {
            for j in (i + 1)..list.len() {
                let a = list[i].clone();
                let b = list[j].clone();
                let key = if a < b { (a, b) } else { (b, a) };
                *pair_support.entry(key.clone()).or_insert(0) += 1;
                pair_commit.entry(key).or_insert_with(|| commit_id.clone());
            }
        }
    }
    (pair_support, pair_commit)
}

fn tier_c_drafts_from_pairs(
    repo: &Repository,
    blob_store: &BlobStore,
    pair_support: BTreeMap<(String, String), usize>,
    pair_commit: &BTreeMap<(String, String), String>,
    min_support: usize,
) -> Result<Vec<TierCEdgeDraft>> {
    let mut out = Vec::new();
    for ((a, b), support) in pair_support {
        if support < min_support {
            continue;
        }
        let commit_id = pair_commit
            .get(&(a.clone(), b.clone()))
            .context("co-change commit id")?;
        let oid = git2::Oid::from_str(commit_id)?;
        let commit = repo.find_commit(oid)?;
        let (blob, start, end) = store_commit_evidence(blob_store, &commit)?;
        let conf = confidence_from_support(support);
        out.push(TierCEdgeDraft {
            src_path: a,
            dst_path: b,
            support,
            evidence_blob: blob,
            evidence_start: start,
            evidence_end: end,
            confidence: conf,
        });
    }
    Ok(out)
}

pub fn mine_cochange_edges(
    replay: &[(String, Vec<String>)],
    blob_store: &BlobStore,
    repo_root: &Path,
    min_support: usize,
) -> Result<Vec<TierCEdgeDraft>> {
    let repo = open_readonly(repo_root)?;
    let (pair_support, pair_commit) = accumulate_cochange_pairs(replay);
    tier_c_drafts_from_pairs(&repo, blob_store, pair_support, &pair_commit, min_support)
}

/// Churn scores from replay (FR-006).
pub fn compute_churn(replay: &[(String, Vec<String>)]) -> BTreeMap<String, f64> {
    let mut scores: BTreeMap<String, f64> = BTreeMap::new();
    for (_, paths) in replay {
        for p in paths {
            *scores.entry(p.clone()).or_insert(0.0) += 1.0;
        }
    }
    scores
}

pub fn hot_top_with_hashes(
    churn: &BTreeMap<String, f64>,
    path_hashes: &BTreeMap<String, String>,
    k: usize,
) -> Vec<HotPath> {
    let mut items: Vec<_> = churn
        .iter()
        .map(|(path, score)| HotPath {
            path: path.clone(),
            churn_score: *score,
            content_sha256: path_hashes.get(path).cloned().unwrap_or_default(),
        })
        .collect();
    items.sort_by(|a, b| {
        b.churn_score
            .partial_cmp(&a.churn_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    items.truncate(k);
    items
}

pub fn state_path(store_root: &Path) -> PathBuf {
    store_root.join("git_empirical").join("state.json")
}

pub fn save_state(store_root: &Path, state: &GitEmpiricalState) -> Result<()> {
    let path = state_path(store_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_string_pretty(state)?)?;
    Ok(())
}

pub fn load_state(store_root: &Path) -> Result<Option<GitEmpiricalState>> {
    let path = state_path(store_root);
    let Ok(bytes) = fs::read(&path) else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_slice(&bytes)?))
}

fn ensure_evidence_blob(data: &mut IndexData, hash: ContentHash) {
    if data.blobs.contains_key(&hash) {
        return;
    }
    data.blobs.insert(
        hash,
        super::indexer::BlobMeta {
            path: format!("git-evidence:{}", hash.to_hex()),
            mtime_nanos: 0,
            size: 0,
            tier_bits: 0b100,
            content_len: 0,
        },
    );
    data.blob_order.push(hash);
}

fn mark_path_tier_c(
    data: &mut IndexData,
    path: &str,
    path_to_blob: &BTreeMap<String, ContentHash>,
) {
    if let Some(hash) = path_to_blob.get(path)
        && let Some(meta) = data.blobs.get_mut(hash)
    {
        meta.tier_bits |= 0b100;
    }
}

fn index_path_to_blob(data: &IndexData) -> BTreeMap<String, ContentHash> {
    data.blob_order
        .iter()
        .filter_map(|h| {
            let meta = data.blobs.get(h)?;
            Some((meta.path.clone(), *h))
        })
        .collect()
}

fn push_cochange_edges_into_index(
    data: &mut IndexData,
    drafts: &[TierCEdgeDraft],
    path_to_blob: &BTreeMap<String, ContentHash>,
) {
    for d in drafts {
        let src_sym = path_symbol(&d.src_path);
        let dst_sym = path_symbol(&d.dst_path);
        ensure_evidence_blob(data, d.evidence_blob);
        data.edges.push(EdgeRecord {
            src: src_sym,
            dst: dst_sym,
            kind: edge_kind::CO_CHANGED,
            confidence: d.confidence,
            blob: d.evidence_blob,
            start: d.evidence_start,
            end: d.evidence_end,
        });
        mark_path_tier_c(data, &d.src_path, path_to_blob);
        mark_path_tier_c(data, &d.dst_path, path_to_blob);
    }
}

fn history_complete_after_replay(
    repo_root: &Path,
    commits_processed: usize,
    max_commits: usize,
) -> Result<bool> {
    let repo = open_readonly(repo_root)?;
    let total_commits = {
        let mut w = repo.revwalk()?;
        w.push_head()?;
        w.count()
    };
    Ok(commits_processed >= total_commits.min(max_commits))
}

/// Append tier-C edges into `IndexData` and mark tier C coverage on touched blobs (FR-012).
pub fn append_tier_c_to_index(
    data: &mut IndexData,
    store_root: &Path,
    repo_root: &Path,
    max_commits: usize,
) -> Result<GitEmpiricalState> {
    let blob_store = BlobStore::open(store_root)?;
    let detailed = replay_commits_detailed(repo_root, max_commits)?;
    let last_commits = last_commit_unix_map(&detailed);
    let replay: Vec<(String, Vec<String>)> = detailed
        .into_iter()
        .map(|entry| (entry.id, entry.paths))
        .collect();
    let commits_processed = replay.len();
    let churn = compute_churn(&replay);
    let drafts = mine_cochange_edges(&replay, &blob_store, repo_root, MIN_COCHANGE_SUPPORT)?;

    let path_to_blob = index_path_to_blob(data);
    push_cochange_edges_into_index(data, &drafts, &path_to_blob);
    for p in churn.keys() {
        mark_path_tier_c(data, p, &path_to_blob);
    }

    let history_complete =
        history_complete_after_replay(repo_root, commits_processed, max_commits)?;

    let state = GitEmpiricalState {
        churn,
        commits_processed,
        history_complete,
        tier_c_edge_count: drafts.len(),
    };
    save_state(store_root, &state)?;
    let _ = super::frecency::merge_last_commits(store_root, &last_commits);
    Ok(state)
}

/// Incremental: apply only HEAD commit churn delta (FR-009).
pub fn apply_latest_commit(
    store_root: &Path,
    repo_root: &Path,
) -> Result<(GitEmpiricalState, std::time::Duration)> {
    let start = Instant::now();
    let mut state = load_state(store_root)?.unwrap_or_default();
    let repo = open_readonly(repo_root)?;
    let head = repo.head()?.peel_to_commit()?;
    let paths = paths_touched_in_commit(&repo, &head)?;
    for p in &paths {
        *state.churn.entry(p.clone()).or_insert(0.0) += 1.0;
    }
    state.commits_processed = state.commits_processed.saturating_add(1);
    save_state(store_root, &state)?;
    let unix = head.time().seconds().max(0) as u64;
    let mut last_commits = BTreeMap::new();
    for path in &paths {
        last_commits.insert(path.clone(), unix);
    }
    let _ = super::frecency::merge_last_commits(store_root, &last_commits);
    Ok((state, start.elapsed()))
}

/// Blame-linked edge when blame hits the def's start line (FR-008).
pub fn blame_link_for_def(
    repo_root: &Path,
    store_root: &Path,
    rel_path: &str,
    def: &DefRecord,
) -> Result<Option<EdgeRecord>> {
    let repo = open_readonly(repo_root)?;
    let blame = repo.blame_file(Path::new(rel_path), None)?;
    let line_no = content_line_index(repo_root, rel_path, def.start as usize)?;
    let hunk = blame.get_line(line_no).context("blame line")?;
    let commit = repo.find_commit(hunk.final_commit_id())?;
    let blob_store = BlobStore::open(store_root)?;
    let (blob, s, e) = store_commit_evidence(&blob_store, &commit)?;
    Ok(Some(EdgeRecord {
        src: format!("blame:{}", hunk.final_commit_id()),
        dst: def.name.clone(),
        kind: edge_kind::CO_CHANGED,
        confidence: 200,
        blob,
        start: s,
        end: e,
    }))
}

fn content_line_index(repo_root: &Path, rel_path: &str, byte_offset: usize) -> Result<usize> {
    let full = repo_root.join(rel_path);
    let content = fs::read(&full)?;
    Ok(content[..byte_offset.min(content.len())]
        .iter()
        .filter(|&&b| b == b'\n')
        .count())
}

pub fn tier_c_coverage_fraction(state: &GitEmpiricalState) -> f64 {
    if state.history_complete {
        1.0
    } else if state.commits_processed == 0 {
        0.0
    } else {
        (state.commits_processed as f64 / (state.commits_processed as f64 + 10.0)).min(0.99)
    }
}
