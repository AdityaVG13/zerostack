//! Watch-mode incremental index: FSEvents (macOS) / inotify (Linux) events
//! applied as per-file index updates. Watch is **acceleration only**
//! (fszero-w2g.47): it never claims sole source of truth under event loss.
//!
//! A `notify` watcher thread forwards coalesced events over a std mpsc
//! channel; the single-threaded session drains the channel at op boundaries
//! (start of `execute()` and of CodeMode plan execution). Renames/moves
//! arrive as remove+create pairs and are handled in either order by statting
//! the path at apply time. Watcher overflow or errors trigger a TARGETED
//! rescan -- a sig-diff walk of the affected subtree -- never a full rebuild.
//!
//! Reconcile FSM (fszero-w2g.1 / .2 / .3):
//! - Rescan events are drained with priority over Path storms.
//! - Drain backlog / truncated walks mark the index **untrusted** until a
//!   complete rescan or metadata refresh clears the flag.
//! - While untrusted, `refresh_stale_index_files` does not short-circuit.
//!
//! Opt-in via `FSZERO_WATCH=1`; long-lived server modes call
//! `start_watcher_if_enabled()` after session construction. One-shot CLI
//! invocations never spawn watchers.

use super::ast::walk::{
    is_ignored_dir_name, is_supported_source_entry, relative_file_key_fast, walk_max_files,
    walk_rs_files,
};
use super::session::FSZeroSession;
#[cfg(feature = "watch")]
use notify::{RecursiveMode, Watcher};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

/// Bound on events applied per drain; leftovers apply on the next boundary.
const MAX_DRAIN_EVENTS: usize = 4096;

/// One coalesced filesystem notification.
#[derive(Debug, Clone)]
pub enum WatchEvent {
    Path(PathBuf),
    Rescan(Option<PathBuf>),
}

/// Observability counters for the watch surface.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WatchStats {
    /// Events pulled off the channel and processed.
    pub events_seen: u64,
    /// Files (re)indexed by watch drains.
    pub files_updated: u64,
    /// Files dropped from the index by watch drains.
    pub files_removed: u64,
    /// Targeted subtree rescans (overflow/error/directory events).
    pub rescans: u64,
    /// Drains that processed at least one event.
    pub drains: u64,
    /// Times a Path storm forced rescan priority / backlog marking.
    pub rescan_priority_drains: u64,
    /// Times a truncated walk left removal detection skipped.
    pub truncated_rescans: u64,
    /// Events drained before the cold index completed and deferred to
    /// the full build_index disk read (fszero-rotation-i1-gqgt.21).
    pub pre_index_deferred_events: u64,
}

/// Reconcile state: when any flag is set the watch-accelerated index is not
/// sole authority and metadata refresh must run (fszero-w2g.47).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WatchReconcileState {
    /// Channel still held Path events after a bounded drain.
    pub drain_backlog: bool,
    /// Overflow/error Rescan not yet completed with a full trusted walk.
    pub overflow_pending: bool,
    /// Last rescan hit walk_max_files; removals may be stale.
    pub untrusted_removals: bool,
    /// Events were drained before index_initialized and deferred to the
    /// next full build_index (fszero-rotation-i1-gqgt.21). Visible so the
    /// deferral is not silently lost; cleared when the cold index completes.
    pub pre_index_deferred: bool,
    /// Monotonic dirty generation for observability.
    pub dirty_generation: u64,
}

impl WatchReconcileState {
    /// True when watch acceleration may be treated as caught-up.
    pub fn index_trusted(&self) -> bool {
        !self.drain_backlog
            && !self.overflow_pending
            && !self.untrusted_removals
            && !self.pre_index_deferred
    }

    fn mark_dirty(&mut self) {
        self.dirty_generation = self.dirty_generation.saturating_add(1);
    }

    fn clear_after_trusted_rescan(&mut self) {
        self.drain_backlog = false;
        self.overflow_pending = false;
        self.untrusted_removals = false;
    }

    pub fn clear_pre_index_deferred(&mut self) {
        self.pre_index_deferred = false;
    }
}

/// Live watch channel; keeps the OS watcher alive for the session lifetime.
pub struct WatchHandle {
    rx: Receiver<WatchEvent>,
    /// `None` when events are injected through `attach_watch_channel` (tests)
    /// or when built without feature `watch`.
    #[cfg(feature = "watch")]
    _watcher: Option<notify::RecommendedWatcher>,
    #[cfg(not(feature = "watch"))]
    _watcher: Option<()>,
}

fn watch_env_enabled() -> bool {
    std::env::var("FSZERO_WATCH").ok().as_deref() == Some("1")
}

/// Root-relative key for an absolute event path; `None` when outside root.
fn rel_key_of(root: &Path, root_canon: &Path, path: &Path) -> Option<String> {
    let rel = path
        .strip_prefix(root_canon)
        .or_else(|_| path.strip_prefix(root))
        .ok()?;
    Some(rel.display().to_string())
}

/// Mirrors the walk filters: any ignored directory name in the relative path
/// (.fszero, .asgrep, .zerostack, .git, target, node_modules, ...) skips it.
fn rel_has_ignored_component(rel: &str) -> bool {
    rel.split('/').any(is_ignored_dir_name)
}

/// Watcher-thread event mapping: filter store/VCS noise early, forward the
/// rest. Errors and rescan-flagged events map to targeted rescans.
#[cfg(feature = "watch")]
fn forward_event(
    tx: &Sender<WatchEvent>,
    root: &Path,
    root_canon: &Path,
    res: notify::Result<notify::Event>,
) {
    let ev = match res {
        Ok(ev) => ev,
        Err(_) => {
            let _ = tx.send(WatchEvent::Rescan(None));
            return;
        }
    };
    if ev.need_rescan() {
        let _ = tx.send(WatchEvent::Rescan(ev.paths.into_iter().next()));
        return;
    }
    if matches!(ev.kind, notify::EventKind::Access(_)) {
        return;
    }
    for path in ev.paths {
        // Drop events under ignored dirs on the watcher thread so the
        // session channel never fills with .fszero/.asgrep store churn.
        match rel_key_of(root, root_canon, &path) {
            Some(rel) if rel.is_empty() || rel_has_ignored_component(&rel) => continue,
            _ => {}
        }
        let _ = tx.send(WatchEvent::Path(path));
    }
}

impl FSZeroSession {
    /// Env-gated startup for long-lived modes (`--mode=codemode` / `mcp`).
    pub fn start_watcher_if_enabled(&mut self) {
        if watch_env_enabled() && self.root.is_some() && self.watch.is_none() {
            if let Err(e) = self.start_watcher() {
                eprintln!("fszero: watch mode unavailable ({e}); using per-op staleness checks");
            }
        }
    }

    /// Subscribe to FSEvents/inotify for the session root.
    pub fn start_watcher(&mut self) -> Result<(), String> {
        #[cfg(not(feature = "watch"))]
        {
            return Err(
                "watch feature not compiled into this artifact (rebuild with --features watch)"
                    .into(),
            );
        }
        #[cfg(feature = "watch")]
        {
            let root = self.require_root()?.to_path_buf();
            let root_canon = fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
            let (tx, rx) = std::sync::mpsc::channel();
            let (froot, fcanon) = (root.clone(), root_canon.clone());
            let mut watcher = notify::recommended_watcher(move |res| {
                forward_event(&tx, &froot, &fcanon, res);
            })
            .map_err(|e| format!("watcher init failed: {e}"))?;
            watcher
                .watch(&root_canon, RecursiveMode::Recursive)
                .map_err(|e| format!("watch {} failed: {e}", root_canon.display()))?;
            self.watch = Some(WatchHandle {
                rx,
                _watcher: Some(watcher),
            });
            Ok(())
        }
    }

    /// Test/injection surface: drive drains from a plain channel, no OS watcher.
    pub fn attach_watch_channel(&mut self, rx: Receiver<WatchEvent>) {
        self.watch = Some(WatchHandle { rx, _watcher: None });
    }

    pub fn watch_active(&self) -> bool {
        self.watch.is_some()
    }

    pub fn watch_stats(&self) -> WatchStats {
        self.watch_stats
    }

    /// Reconcile FSM snapshot (fszero-w2g.47).
    pub fn watch_reconcile_state(&self) -> WatchReconcileState {
        self.watch_reconcile
    }

    /// Whether the watch-accelerated index is currently trusted as caught-up.
    pub fn watch_index_trusted(&self) -> bool {
        !self.watch_active() || self.watch_reconcile.index_trusted()
    }

    /// Pull pending events off the channel and apply them. Called at op
    /// boundaries; no-op when nothing changed. Machine-wide admission is
    /// hub-owned, so the drain runs directly against the domain channel.
    pub fn drain_watch_events(&mut self) {
        if self.watch.is_none() {
            return;
        }
        self.drain_watch_events_unlocked();
    }

    /// Priority drain (fszero-w2g.2): Rescan events are never starved by a
    /// Path FIFO flood. Path events are capped at MAX_DRAIN_EVENTS; if more
    /// Paths remain, mark backlog and force a root Rescan so catch-up is
    /// fair rather than fairness-dependent.
    fn drain_watch_events_unlocked(&mut self) {
        let Some(handle) = self.watch.as_ref() else {
            return;
        };
        let mut rescans: Vec<WatchEvent> = Vec::new();
        let mut paths: Vec<WatchEvent> = Vec::new();
        let mut path_capped = false;
        while let Ok(ev) = handle.rx.try_recv() {
            match ev {
                WatchEvent::Rescan(_) => rescans.push(ev),
                WatchEvent::Path(_) if paths.len() < MAX_DRAIN_EVENTS => paths.push(ev),
                WatchEvent::Path(_) => {
                    path_capped = true;
                    // Leave remaining Path events on the channel by not
                    // consuming further Paths — but we already consumed this
                    // one; convert storm into overflow rescan instead of drop.
                    if !rescans
                        .iter()
                        .any(|e| matches!(e, WatchEvent::Rescan(None)))
                    {
                        rescans.push(WatchEvent::Rescan(None));
                    }
                    // Drain any further Rescans still queued (priority).
                    while let Ok(more) = handle.rx.try_recv() {
                        match more {
                            WatchEvent::Rescan(_) => rescans.push(more),
                            WatchEvent::Path(_) => {
                                // Skip excess paths; root rescan covers them.
                            }
                        }
                    }
                    break;
                }
            }
        }
        if path_capped {
            self.watch_stats.rescan_priority_drains += 1;
            self.watch_reconcile.drain_backlog = true;
            self.watch_reconcile.overflow_pending = true;
            self.watch_reconcile.mark_dirty();
        }
        // Rescans first, then paths (coalesce order in apply).
        let mut events = rescans;
        events.extend(paths);
        if !events.is_empty() {
            // Pre-index drains are deferred to the full build_index disk
            // read, but must be visible (not silently dropped). Record a
            // dirty flag and counter so callers can observe the deferral.
            if !self.index_initialized {
                self.watch_stats.events_seen += events.len() as u64;
                self.watch_stats.pre_index_deferred_events += events.len() as u64;
                self.watch_stats.drains += 1;
                self.watch_reconcile.pre_index_deferred = true;
                self.watch_reconcile.mark_dirty();
                return;
            }
            self.apply_watch_events(events);
        }
    }

    /// Apply a batch of watch events as incremental index updates.
    /// Returns the number of files updated or removed.
    pub fn apply_watch_events(&mut self, events: Vec<WatchEvent>) -> usize {
        if events.is_empty() {
            return 0;
        }
        let Some(root) = self.root.clone() else {
            return 0;
        };
        // Before the cold index has run there is nothing to patch: the
        // deferred build_index observes current disk state anyway. Record
        // the deferral visibly rather than silently dropping.
        if !self.index_initialized {
            self.watch_stats.events_seen += events.len() as u64;
            self.watch_stats.pre_index_deferred_events += events.len() as u64;
            self.watch_stats.drains += 1;
            self.watch_reconcile.pre_index_deferred = true;
            self.watch_reconcile.mark_dirty();
            return 0;
        }
        self.watch_stats.drains += 1;
        self.watch_stats.events_seen += events.len() as u64;
        let root_canon = fs::canonicalize(&root).unwrap_or_else(|_| root.clone());

        // Coalesce: dedupe path events, collect rescan targets.
        let mut rescan_all = false;
        let mut rescan_subtrees: Vec<String> = Vec::new();
        let mut rel_paths: Vec<String> = Vec::new();
        let mut seen_rel: HashSet<String> = HashSet::new();
        for ev in events {
            match ev {
                WatchEvent::Rescan(None) => rescan_all = true,
                WatchEvent::Rescan(Some(p)) => match rel_key_of(&root, &root_canon, &p) {
                    Some(rel) if !rel_has_ignored_component(&rel) => rescan_subtrees.push(rel),
                    Some(_) => {}
                    None => rescan_all = true,
                },
                WatchEvent::Path(p) => {
                    if let Some(rel) = rel_key_of(&root, &root_canon, &p) {
                        if !rel.is_empty()
                            && !rel_has_ignored_component(&rel)
                            && seen_rel.insert(rel.clone())
                        {
                            rel_paths.push(rel);
                        }
                    }
                }
            }
        }

        let mut changed: Vec<String> = Vec::new();
        let mut removed: Vec<String> = Vec::new();
        // Any Rescan means catch-up work; mark overflow until a complete walk.
        if rescan_all || !rescan_subtrees.is_empty() {
            self.watch_reconcile.overflow_pending = true;
            self.watch_reconcile.mark_dirty();
        }
        let mut all_rescans_complete = true;
        if rescan_all {
            self.watch_stats.rescans += 1;
            if !self.rescan_subtree(&root, "", &mut changed, &mut removed) {
                all_rescans_complete = false;
            }
        } else {
            // Rescans before Path events (fszero-w2g.2 liveness).
            for rel in &rescan_subtrees {
                self.watch_stats.rescans += 1;
                if !self.rescan_subtree(&root, rel, &mut changed, &mut removed) {
                    all_rescans_complete = false;
                }
            }
            for rel in rel_paths {
                self.apply_one_path(&root, &rel, &mut changed, &mut removed);
            }
        }

        if rescan_all && all_rescans_complete {
            // Full root rescan that completed: index is trusted again.
            self.watch_reconcile.clear_after_trusted_rescan();
        } else if !rescan_subtrees.is_empty() && all_rescans_complete && !rescan_all {
            // Scoped rescans completed: clear overflow only if no backlog.
            if !self.watch_reconcile.drain_backlog && !self.watch_reconcile.untrusted_removals {
                self.watch_reconcile.overflow_pending = false;
            }
        }

        changed.sort();
        changed.dedup();
        removed.sort();
        removed.dedup();
        // A path both removed and re-created within one drain counts as changed.
        removed.retain(|k| !changed.contains(k));
        if changed.is_empty() && removed.is_empty() {
            return 0;
        }
        self.watch_stats.files_updated += changed.len() as u64;
        self.watch_stats.files_removed += removed.len() as u64;
        self.sync_asgrep_files(&root, &changed, &removed);
        self.publish_watch_feed(&changed, &removed);
        changed.len() + removed.len()
    }

    /// Durable change feed for sibling engines (fszero-lau): ordered events
    /// with monotonic seq + index generation, persisted under `watch/feed`
    /// in the (unified) store so graphzero/cachezero can poll and replay.
    /// Ring of the newest FEED_CAP events; a consumer whose cursor is older
    /// than the ring's head must full-resync (documented contract in
    /// docs/design/watch-feed.md).
    fn publish_watch_feed(&mut self, changed: &[String], removed: &[String]) {
        const FEED_CAP: usize = 1024;
        let prior = self
            .recovery
            .payload("watch/feed")
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
        // Seq is monotonic ACROSS sessions: resume from the persisted feed.
        if let Some(last) = prior
            .as_ref()
            .and_then(|v| v.get("last_seq"))
            .and_then(|v| v.as_u64())
        {
            self.watch_feed_seq = self.watch_feed_seq.max(last);
        }
        let mut events: Vec<serde_json::Value> = prior
            .and_then(|v| v.get("events").and_then(|e| e.as_array()).cloned())
            .unwrap_or_default();
        let generation = self.index.ast_generation;
        for (kind, keys) in [("changed", changed), ("removed", removed)] {
            for rel in keys {
                self.watch_feed_seq += 1;
                events.push(serde_json::json!( { "seq": self.watch_feed_seq, "kind": kind, "file": rel, "generation": generation, }));
            }
        }
        if events.len() > FEED_CAP {
            let drop = events.len() - FEED_CAP;
            events.drain(..drop);
        }
        let payload =
            serde_json::json!({ "version": 1, "last_seq": self.watch_feed_seq, "events": events, })
                .to_string();
        self.recovery.put_key("watch/feed", payload.as_bytes());
    }

    /// Drop one indexed key and record it in `removed` (no-op if absent).
    fn drop_indexed_if_present(&mut self, root: &Path, rel: &str, removed: &mut Vec<String>) {
        if self.index.indexed_file_keys.contains(rel) {
            self.remove_indexed_file(root, rel);
            removed.push(rel.to_string());
        }
    }

    /// Drop an already-owned indexed key and record it.
    fn drop_indexed_owned(&mut self, root: &Path, key: String, removed: &mut Vec<String>) {
        self.remove_indexed_file(root, &key);
        removed.push(key);
    }

    /// Stat one root-relative path and apply the matching update.
    fn apply_one_path(
        &mut self,
        root: &Path,
        rel: &str,
        changed: &mut Vec<String>,
        removed: &mut Vec<String>,
    ) {
        let abs = root.join(rel);
        match fs::symlink_metadata(&abs) {
            Ok(meta) if meta.file_type().is_symlink() => {
                self.drop_indexed_if_present(root, rel, removed);
            }
            Ok(meta) if meta.is_dir() => {
                self.watch_stats.rescans += 1;
                let _ = self.rescan_subtree(root, rel, changed, removed);
            }
            Ok(meta) => {
                if !is_supported_source_entry(&abs, &meta) {
                    self.drop_indexed_if_present(root, rel, removed);
                    return;
                }
                let unchanged = meta.modified().ok().is_some_and(|mtime| {
                    self.index.file_sig.get(rel) == Some(&(mtime, meta.len()))
                });
                if unchanged {
                    return;
                }
                self.reindex_path(&abs);
                changed.push(rel.to_string());
            }
            Err(_) => {
                // Gone: single file, or a directory removed/moved away.
                if self.index.indexed_file_keys.contains(rel) {
                    self.drop_indexed_if_present(root, rel, removed);
                } else {
                    let prefix = format!("{rel}/");
                    let subkeys: Vec<String> = self
                        .index
                        .indexed_file_keys
                        .iter()
                        .filter(|k| k.starts_with(&prefix))
                        .cloned()
                        .collect();
                    for key in subkeys {
                        self.drop_indexed_owned(root, key, removed);
                    }
                }
            }
        }
    }

    /// Targeted rescan: walk one subtree with the standard filters, reindex
    /// only sig diffs, drop indexed keys the walk no longer sees. This is the
    /// overflow recovery path -- bounded by the subtree, never a rebuild.
    ///
    /// Returns `true` when the walk completed (removal detection ran).
    /// Truncated walks mark `untrusted_removals` (fszero-w2g.3).
    fn rescan_subtree(
        &mut self,
        root: &Path,
        rel: &str,
        changed: &mut Vec<String>,
        removed: &mut Vec<String>,
    ) -> bool {
        let subtree = if rel.is_empty() {
            root.to_path_buf()
        } else {
            root.join(rel)
        };
        let entries = walk_rs_files(&subtree);
        let walk_complete = entries.len() < walk_max_files();
        let mut seen: HashSet<String> = HashSet::with_capacity(entries.len());
        for (path, meta) in entries {
            let key = relative_file_key_fast(root, &path);
            let unchanged = meta
                .modified()
                .ok()
                .is_some_and(|mtime| self.index.file_sig.get(&key) == Some(&(mtime, meta.len())));
            seen.insert(key.clone());
            if !unchanged {
                self.reindex_path(&path);
                changed.push(key);
            }
        }
        // Removal detection is only sound when the walk was not truncated by
        // the file cap; a truncated walk must never drop unseen keys — and
        // must mark the index untrusted so readers do not treat it as sole truth.
        if !walk_complete {
            self.watch_stats.truncated_rescans += 1;
            self.watch_reconcile.untrusted_removals = true;
            self.watch_reconcile.mark_dirty();
            return false;
        }
        let prefix = if rel.is_empty() {
            String::new()
        } else {
            format!("{rel}/")
        };
        let stale: Vec<String> = self
            .index
            .indexed_file_keys
            .iter()
            .filter(|k| k.starts_with(&prefix) && !seen.contains(*k))
            .cloned()
            .collect();
        for key in stale {
            self.drop_indexed_owned(root, key, removed);
        }
        true
    }

    /// Drop every trace of one file: in-memory index maps, caches, and the
    /// persisted AST rows. The asgrep row is removed in sync_asgrep_files.
    fn remove_indexed_file(&mut self, root: &Path, file_key: &str) {
        self.index.symbols.retain(|(_, file)| file != file_key);
        self.index.indexed_file_keys.remove(file_key);
        self.index.file_sig.remove(file_key);
        self.lazy_bigrams.remove(file_key);
        self.clear_query_caches();
        let abs = root.join(file_key);
        self.caches.content.remove(&abs);
        self.caches
            .paths
            .remove(&format!("{}\0{}", root.display(), file_key));
        if let Some(parent) = abs.parent() {
            self.caches.ls.remove(&parent.to_path_buf());
        }
        if self.persist_ast_index {
            self.recovery.ast.clear_for_file(file_key);
        }
    }

    /// Mirror watch updates into the ast-sgrep store and refresh the searcher.
    fn sync_asgrep_files(&mut self, _root: &Path, _changed: &[String], _removed: &[String]) {
        // Direct literal scan reads current files; no SQLite index to sync.
    }
}
