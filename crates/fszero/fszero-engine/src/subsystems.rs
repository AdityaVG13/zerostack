use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use super::session::{ReadCacheEntry, ReadViewMeta};
use super::world::WorldEdit;

/// Hot FS caches (ls, content, path resolution, search/compound memo).
#[derive(Debug, Default)]
pub struct SessionCaches {
    pub ls: HashMap<PathBuf, (Vec<String>, SystemTime)>,
    pub content: HashMap<PathBuf, ReadCacheEntry>,
    /// Resolved absolute paths; Arc so warm sticky re-reads share the PathBuf.
    pub paths: HashMap<String, Arc<PathBuf>>,
    /// Sticky last successful resolve for consecutive identical path args
    /// (typical AI re-read of the same file — avoids key alloc + HashMap get).
    pub last_path_arg: Option<String>,
    pub last_path: Option<Arc<PathBuf>>,
    /// Repo-relative path for last warm read (skip rebuild on sticky re-read).
    /// Arc so sticky re-reads avoid String clone into access_log path.
    pub last_access_rel: Option<Arc<str>>,
    /// Content-hash suffix for last warm read access_log row.
    pub last_access_hash: Option<Arc<str>>,
    /// Last warm-read visible response (`read:N bytes ref=…`) for sticky reuse.
    pub last_warm_response: Option<String>,
    pub last_warm_len: Option<usize>,
    /// Search memo: (query, ast_generation) → (payload body, content_ref).
    /// Warm hits reuse the mint ref and skip re-hash / CAS / ref-index work.
    /// content_ref is Arc so warm format! paths avoid re-allocating the blob id.
    pub search: HashMap<(String, u64), (String, Arc<str>)>,
    pub compound: HashMap<(String, u64), String>,
    /// Certified empty search answers with scope roots (fszero-ojnv).
    pub negative_cache: super::negative_cache::NegativeCache,
    /// Opaque search page cursors (fszero-enuj).
    pub search_cursors: super::search_cursor::SearchCursorStore,
    /// Directory trie for multi_list batch listings (warm when built, cold on first call).
    pub directory_index: Option<super::multi_list::DirectoryIndex>,
    /// Syntax forest for multi_ast_search: unchanged files are never reparsed
    /// across calls within one session.
    #[cfg(feature = "fszero-ast-sgrep")]
    pub ast_forest: super::multi_ast_search::AstForest,
}

/// In-memory symbol/grep index + AST generation counter.
#[derive(Debug, Clone, Default)]
pub struct IndexState {
    pub symbols: Vec<(String, String)>,
    pub indexed_file_keys: HashSet<String>,
    pub ast_generation: u64,
    pub file_sig: HashMap<String, (SystemTime, u64)>,
}

/// Observability snapshot from the most recent [`FSZeroSession::build_index`].
/// Used by tests to prove code-aware invalidation both directions (fszero-1it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IndexBuildReport {
    /// Stable build mode for telemetry: cold, incremental, or warm_skip.
    pub mode: &'static str,
    /// True when the durable manifest producer matched and file-sig diff ran.
    pub incremental: bool,
    /// True when stored producer fingerprint matched the running engine.
    pub producer_matched: bool,
    pub dirty: usize,
    pub removed: usize,
    /// Files that were read+parsed this build (0 on warm unchanged).
    pub ingested: usize,
    pub files_walked: usize,
    /// True when the configured walk cap prevented a complete traversal.
    pub truncated: bool,
}

/// Observability snapshot from the most recent
/// [`FSZeroSession::refresh_stale_index_files`].
///
/// `stated` is the number of filesystem metadata calls. A git worktree can
/// keep this at the dirty-set size instead of the whole tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IndexRefreshReport {
    /// `watch_skip`, `full_stat`, or `git_candidates`.
    pub mode: &'static str,
    /// Paths considered this refresh (0 on watch skip).
    pub candidates: usize,
    /// `stat` calls issued.
    pub stated: usize,
    /// Paths handed to `reindex_path`.
    pub reindexed: usize,
}

/// Ephemeral read-view registry and last-op payload shortcuts.
#[derive(Debug, Default)]
pub struct ViewRegistry {
    pub views: HashMap<u32, ReadViewMeta>,
    pub last_view_id: u32,
    pub last_read_ref: Option<std::sync::Arc<str>>,
    pub last_search_payload: Option<Vec<u8>>,
    pub last_expand_payload: Option<Vec<u8>>,
    pub last_compound_payload: Option<Vec<u8>>,
}

/// Staged speculative edit worlds.
#[derive(Debug)]
pub struct WorldRegistry {
    pub active: HashMap<String, WorldEdit>,
    pub next_id: u32,
    /// Hunks published by successful `commit_world` calls this session.
    /// A later commit whose base moved rejects if its hunks overlap these
    /// (filesystem-v1 world-overlap): never last-write-wins.
    pub committed_hunks: Vec<(PathBuf, (u32, u32))>,
}

impl WorldRegistry {
    pub fn new() -> Self {
        Self {
            active: HashMap::new(),
            next_id: 1,
            committed_hunks: Vec::new(),
        }
    }
}

impl Default for WorldRegistry {
    fn default() -> Self {
        Self::new()
    }
}
