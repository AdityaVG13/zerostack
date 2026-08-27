use super::search_prefilter_eval::LazyBigramIndex;
use super::subsystems::{
    IndexBuildReport, IndexRefreshReport, IndexState, SessionCaches, ViewRegistry, WorldRegistry,
};
use super::*;
use std::sync::Arc;

/// Single table for CLI letter, recovery key, and domain op id (no twin match arms).
macro_rules! define_opcodes {
    ($(
        $(#[$meta:meta])*
        $name:ident : $ch:literal / $letter:literal, $key:literal, $op:literal
    );+ $(;)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum OpCode { $( $(#[$meta])* $name, )+ }

        impl OpCode {
            pub fn from_char(c: char) -> Option<Self> {
                match c.to_ascii_uppercase() { $($ch => Some(Self::$name),)+ _ => None, } }

            /// Static single-letter form for ack prefix / budget evidence (no alloc).
            pub const fn as_letter(self) -> &'static str { match self { $(Self::$name => $letter,)+ } }

            /// Canonical uppercase CLI letter for this opcode.
            pub const fn as_char(self) -> char { self.as_letter().as_bytes()[0] as char }

            /// Default recovery-store key after a successful kernel op.
            pub const fn recovery_key(self) -> &'static str { match self { $(Self::$name => $key,)+ } }

            /// Canonical domain operation id (`fs.read`, …).
            pub const fn operation_id(self) -> &'static str { match self { $(Self::$name => $op,)+ } } } }; }

define_opcodes! {
Ls: 'L' / "L", "ls_manifest", "fs.ls"; Read: 'R' / "R", "read", "fs.read";
Search: 'S' / "S", "search", "fs.search"; Edit: 'E' / "E", "last_cert", "fs.edit";
/// Compound intent: server-side multi-op sequence, 1 visible ack for many low-level ops.
Compound: 'C' / "C", "compound", "fs.compound";
/// Expand: retrieve payload for a prior ref or view id (e.g. '17' or 'fz://...').
Expand: 'X' / "X", "expand", "fs.expand";
/// World: create/commit/drop speculative edit worlds with a 1-token ack.
World: 'W' / "W", "world", "fs.world";
/// Stat: file metadata behind a recoverable ref.
Stat: 'T' / "T", "stat", "fs.stat";
/// Resolve: intent → ranked file refs (fs.resolve).
Resolve: 'V' / "V", "resolve", "fs.resolve";
/// Write: create-or-overwrite a file with full content (fs.write). 'P' (put).
Write: 'P' / "P", "write-post", "fs.write";
/// History: queryable mutation timeline for a path (fs.history). 'H'.
History: 'H' / "H", "history", "fs.history";
/// Undo: revert a journaled mutation, preimage-guarded (fs.undo). 'U'.
Undo: 'U' / "U", "undo", "fs.undo";
/// Memory: durable mem:// volume put/get/ls/delete/rename. 'M'.
Memory: 'M' / "M", "memory", "fs.memory"; }

/// R-PAR-REC-003 / fszero-2qdw.10: compact opcode map for errors and fszero.exec pedagogy.
/// Note: **W is world, not write** — write/put is **P**.
pub const OPCODE_MAP_HINT: &str = "L=ls R=read S=search E=edit C=compound X=expand W=world(≠write) T=stat V=resolve P=write H=history U=undo M=memory";

/// Resolve an fszero.exec `code` string to a CLI letter, or a pedagogical error.
///
/// Full-word mistakes (especially `write` → W) are rejected before first-char
/// collapse so agents get an explicit W≠write correction.
pub fn parse_exec_opcode(raw: &str) -> Result<char, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(format!(
            "fszero.exec requires code; opcodes: {OPCODE_MAP_HINT}"
        ));
    }
    match s.to_ascii_lowercase().as_str() {
        "write" | "put" | "create" | "overwrite" => {
            return Err(format!(
                "W is world, not write; use opcode P (put/write) or fszero.write. Map: {OPCODE_MAP_HINT}"
            ));
        }
        "world" => return Ok('W'),
        "read" => return Ok('R'),
        "ls" | "list" => return Ok('L'),
        "search" | "grep" => return Ok('S'),
        "edit" => return Ok('E'),
        "compound" => return Ok('C'),
        "expand" => return Ok('X'),
        "stat" => return Ok('T'),
        "resolve" => return Ok('V'),
        "history" => return Ok('H'),
        "undo" => return Ok('U'),
        "memory" | "mem" => return Ok('M'),
        _ => {}
    }
    let mut chars = s.chars();
    let ch = chars.next().expect("non-empty after trim");
    // Multi-letter unknown tokens: do not silently take first char.
    if chars.next().is_some() {
        return Err(format!(
            "unknown exec code {s:?}; use a single letter. Map: {OPCODE_MAP_HINT}"
        ));
    }
    let up = ch.to_ascii_uppercase();
    if OpCode::from_char(up).is_some() {
        return Ok(up);
    }
    Err(format!("bad opcode: {ch}; map: {OPCODE_MAP_HINT}"))
}

#[derive(Debug, Clone)]
pub struct ReadCacheEntry {
    pub bytes: Arc<Vec<u8>>,
    pub mtime: SystemTime,
    pub content_ref: Arc<str>,
}

#[derive(Debug, Clone)]
pub struct ReadViewMeta {
    pub path: Arc<PathBuf>,
    pub content_ref: Arc<str>,
}

#[derive(Debug, Clone)]
pub struct IndexedLine {
    pub file_key: Arc<str>,
    pub line_no: usize,
    pub text: String,
}

pub struct FSZeroSession {
    pub root: Option<PathBuf>,
    /// Cached `canonicalize(root)` for warm path revalidate / access-log
    /// (invalidated whenever `root` is reassigned).
    pub root_canon: Option<PathBuf>,
    pub op_count: u32,
    pub last_result: Option<String>,
    pub recovery: RecoveryStore,
    pub codemode_edit_plan: bool,
    pub pending_edit_intents: Vec<i64>,
    /// Last pure-read plan plus file metadata proven by a FULL execution.
    pub codemode_relaxed_read_signature: Option<String>,
    /// MCP wire adapter may append its envelope before committing read receipts.
    pub codemode_defer_wire_receipt: bool,
    pub last_mutation_outcome: Option<crate::MutationOutcome>,
    /// Per-execution observed payload bytes for honest CodeMode accounting.
    pub codemode_materialized_bytes: u64,
    pub codemode_materialized_hashes: std::collections::HashSet<[u8; 32]>,
    pub codemode_measurement_misses: u64,
    /// True when `with_repo_store` fell back to in-memory recovery.
    pub durable_degraded: bool,
    /// Sticky store identity for this session (fszero-store-root-fragmentation-jdl).
    /// Bound once at construction; `set_workspace_root` must not rebind it.
    pub bound_store_id: Option<String>,
    /// Session store map: durable db paths this session has minted into.
    /// Expand consults these before giving up (same-process multi-root).
    pub store_map: Vec<PathBuf>,
    pub caches: SessionCaches,
    pub index: IndexState,
    pub views: ViewRegistry,
    pub worlds: WorldRegistry,
    /// Parsed from `FSZERO_TRUSTED_HOT`; reserved (listing cache always uses mtime).
    #[allow(dead_code)]
    pub trusted_hot: bool,
    pub persist_ast_index: bool,
    pub index_initialized: bool,
    /// Watch-mode channel (FSEvents/inotify or injected); None unless enabled.
    pub watch: Option<super::watch::WatchHandle>,
    /// Monotonic sequence for the durable watch change feed (fszero-lau).
    pub watch_feed_seq: u64,
    /// Counters for applied watch events (observability/telemetry).
    pub watch_stats: super::watch::WatchStats,
    /// Watch reconcile FSM (fszero-w2g.47): backlog / overflow / truncated.
    pub watch_reconcile: super::watch::WatchReconcileState,
    pub last_op_us: u128,
    pub version: u64,
    /// Groups access_log rows for co-access within one agent session.
    pub access_session_window: i64,
    /// Content hashes of payloads already served this session (novelty
    /// detector): first serve is judgment (capsule), re-serve is mechanical
    /// (ref + preview). Changed content hashes differently, so it is novel.
    pub served_content: std::collections::HashSet<u64>,
    /// Content hashes an `fs.expand` produced this execution. `expand` is the
    /// documented exact-bytes escape hatch, so the visible-wire novelty pass
    /// must return these verbatim instead of collapsing them to a preview
    /// (fszero-fs-read-content-broken-b4yg): a `read` before an `expand` of the
    /// same ref always marks the bytes served, which silently turned every such
    /// expand into `{ref, preview, seen}`.
    pub exact_served_content: std::collections::HashSet<u64>,
    /// Paths this plan wrote or mutated (pn93): a later full-file read of one
    /// returns content the session already produced, so it must inline
    /// verbatim instead of collapsing to a ref that forces a re-fetch.
    pub produced_paths: std::collections::HashSet<String>,
    /// Cooperative cancel for the in-flight MCP tools/call (fszero-l4k).
    pub request_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Absolute deadline for the in-flight MCP tools/call (fszero-l4k).
    pub request_deadline: Option<std::time::Instant>,
    /// In-memory CodeMode response (fszero-iod).
    pub last_codemode_response: Option<serde_json::Value>,
    /// Substrate runtime health (fszero-iod).
    pub runtime_health: super::runtime_health::RuntimeHealth,
    /// Last `build_index` outcome (code-aware invalidation observability).
    pub last_index_build: IndexBuildReport,
    /// Last stale-index refresh outcome (git candidate set vs full stat).
    pub last_index_refresh: IndexRefreshReport,
    /// Lazy incremental bigram bitsets for default `bigram_memmem` prefilter
    /// (fszero-kbo; escape `FSZERO_SEARCH_PREFILTER=contains`). Empty until
    /// first search ensure_files / incremental ingest upsert.
    pub lazy_bigrams: LazyBigramIndex,
}

impl Default for FSZeroSession {
    fn default() -> Self {
        Self {
            root: None,
            root_canon: None,
            op_count: 0,
            last_result: None,
            recovery: RecoveryStore::new(),
            codemode_edit_plan: false,
            pending_edit_intents: Vec::new(),
            codemode_relaxed_read_signature: None,
            codemode_defer_wire_receipt: false,
            last_mutation_outcome: None,
            codemode_materialized_bytes: 0,
            codemode_materialized_hashes: std::collections::HashSet::new(),
            codemode_measurement_misses: 0,
            durable_degraded: false,
            bound_store_id: None,
            store_map: Vec::new(),
            caches: SessionCaches::default(),
            index: IndexState::default(),
            views: ViewRegistry::default(),
            worlds: WorldRegistry::default(),
            trusted_hot: std::env::var("FSZERO_TRUSTED_HOT").ok().as_deref() == Some("1"),
            persist_ast_index: true,
            index_initialized: false,
            watch: None,
            watch_feed_seq: 0,
            watch_stats: super::watch::WatchStats::default(),
            watch_reconcile: super::watch::WatchReconcileState::default(),
            last_op_us: 0,
            version: 0,
            access_session_window: (super::unix_epoch_nanos() as i64).max(1),
            served_content: std::collections::HashSet::new(),
            exact_served_content: std::collections::HashSet::new(),
            produced_paths: std::collections::HashSet::new(),
            request_cancel: None,
            request_deadline: None,
            last_codemode_response: None,
            runtime_health: super::runtime_health::RuntimeHealth::new(),
            last_index_build: IndexBuildReport::default(),
            last_index_refresh: IndexRefreshReport::default(),
            lazy_bigrams: LazyBigramIndex::new(),
        }
    }
}

impl FSZeroSession {
    pub fn reset_codemode_measurement(&mut self) {
        self.codemode_materialized_bytes = 0;
        self.codemode_materialized_hashes.clear();
        self.codemode_measurement_misses = 0;
    }
    pub fn record_codemode_materialization(&mut self, bytes: &[u8]) -> bool {
        use sha2::Digest as _;
        let digest: [u8; 32] = sha2::Sha256::digest(bytes).into();
        if !self.codemode_materialized_hashes.insert(digest) {
            return false;
        }
        self.codemode_materialized_bytes = self
            .codemode_materialized_bytes
            .saturating_add(bytes.len() as u64);
        true
    }
    pub fn record_codemode_measurement_miss(&mut self) {
        self.codemode_measurement_misses = self.codemode_measurement_misses.saturating_add(1);
    }
    pub fn set_mutation_outcome(&mut self, outcome: crate::MutationOutcome) {
        self.last_mutation_outcome = Some(outcome);
    }
    pub fn take_mutation_outcome(&mut self) -> Option<crate::MutationOutcome> {
        self.last_mutation_outcome.take()
    }
}

fn should_build_startup_index() -> bool {
    std::env::var("FSZERO_STARTUP_INDEX").ok().as_deref() == Some("1")
}

/// Opt-in only: allow `with_repo_store` to fall back to `:memory:` when the
/// on-disk store cannot open. Default is fail-closed so agents never silently
/// lose journal / access / world durability.
fn allow_ephemeral_store() -> bool {
    std::env::var("FSZERO_ALLOW_EPHEMERAL").ok().as_deref() == Some("1")
}

/// True when the durable store cannot be opened by any amount of retrying and
/// the failure is confined to the store file itself.
///
/// Deliberately narrow. Corruption and a rejecting integrity gate mean the
/// cache is lost, which a session can survive. Anything else - a permissions
/// problem, a missing parent directory, a full disk - may indicate the caller
/// is pointed somewhere wrong, and silently running without durability would
/// hide that. Those still panic.
fn is_unrecoverable_store(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("integrity gate rejected the source")
        || lower.contains("disk image is malformed")
        || lower.contains("file is not a database")
        || lower.contains("database corruption")
}

impl FSZeroSession {
    /// Install cooperative cancel + deadline for the in-flight tools/call.
    pub fn install_request_guard(
        &mut self,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
        deadline: std::time::Instant,
    ) {
        self.request_cancel = Some(cancel);
        self.request_deadline = Some(deadline);
    }

    /// Clear the in-flight request guard after tools/call completes.
    pub fn clear_request_guard(&mut self) {
        self.request_cancel = None;
        self.request_deadline = None;
    }

    /// ABI detail when the in-flight request is dead. Client cancel wins so a
    /// timeout is never reported after `notifications/cancelled`.
    pub fn request_expiry_detail(&self) -> Option<&'static str> {
        use std::sync::atomic::Ordering;
        if self
            .request_cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::SeqCst))
        {
            return Some("request cancelled");
        }
        if self
            .request_deadline
            .is_some_and(|d| std::time::Instant::now() >= d)
        {
            return Some("request deadline exceeded");
        }
        None
    }

    /// True when the client cancelled the call or the request deadline elapsed.
    pub fn request_expired(&self) -> bool {
        self.request_expiry_detail().is_some()
    }

    /// Records that this exact content was served this session. Returns true
    /// on first encounter (novel — deliver a capsule), false on re-encounter
    /// (mechanical — ref + preview suffices).
    pub fn note_served_content(&mut self, text: &str) -> bool {
        self.served_content.insert(Self::content_hash(text))
    }

    pub fn content_hash(text: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    }

    /// Marks bytes as an exact serve (`fs.expand`), exempting them from the
    /// visible-wire novelty collapse.
    pub fn note_exact_served_content(&mut self, text: &str) {
        self.exact_served_content.insert(Self::content_hash(text));
    }

    pub fn is_exact_served_content(&self, text: &str) -> bool {
        self.exact_served_content
            .contains(&Self::content_hash(text))
    }

    /// pn93: records a path the current plan just wrote or mutated. The path
    /// is stored normalized (leading "./" stripped) since plans typically use
    /// the same relative spelling for write and read.
    pub fn note_produced_path(&mut self, path: &str) {
        self.produced_paths
            .insert(path.strip_prefix("./").unwrap_or(path).to_string());
    }

    pub fn is_produced_path(&self, path: &str) -> bool {
        self.produced_paths
            .contains(path.strip_prefix("./").unwrap_or(path))
    }

    /// Exact-serve marks are per-execution: a later plan that merely re-reads
    /// the same bytes must still get the novelty budget.
    pub fn clear_exact_served_content(&mut self) {
        self.exact_served_content.clear();
    }

    pub fn stash_codemode_response(&mut self, payload: serde_json::Value) {
        self.last_codemode_response = Some(payload);
    }

    pub fn take_codemode_response(&mut self) -> Option<serde_json::Value> {
        self.last_codemode_response.take()
    }

    pub fn force_missing_codemode_response_for_test(&mut self) {
        self.last_codemode_response = None;
        self.recovery.put_key("codemode/response", b"");
    }

    /// Inject a one-shot store fault for search/grep failure tests (fszero-szw).
    pub fn inject_store_error_for_test(&mut self, msg: impl Into<String>) {
        self.recovery.inject_store_error_for_test(msg);
    }

    pub fn new() -> Self {
        let mut s = Self::default();
        s.publish_capabilities();
        s
    }

    pub fn with_root(root: impl AsRef<Path>) -> Self {
        let mut s = Self::new();
        s.root = Some(root.as_ref().to_path_buf());
        s.refresh_root_canon();
        if should_build_startup_index() {
            let _ = s.build_index();
        }
        s
    }

    pub fn with_repo_store(root: impl AsRef<Path>) -> Self {
        let root_path = root.as_ref().to_path_buf();
        match Self::try_with_repo_store(&root_path) {
            Ok(s) => s,
            Err(e) if allow_ephemeral_store() => {
                eprintln!(
                    "fszero: durable store unavailable ({e}); using in-memory recovery (FSZERO_ALLOW_EPHEMERAL=1)"
                );
                let mut s = Self {
                    root: Some(root_path),
                    durable_degraded: true,
                    ..Self::default()
                };
                s.refresh_root_canon();
                s.publish_capabilities();
                if should_build_startup_index() {
                    let _ = s.build_index();
                }
                s
            }
            // CreateOrOpen now quarantines a destructively malformed store
            // and mints a fresh durable file (workspace is source of truth).
            // This branch is the last resort: reset itself failed (disk full,
            // permissions) so we degrade this session instead of panicking
            // the stdio worker (zerostack-byn).
            Err(e) if is_unrecoverable_store(&e) => {
                eprintln!(
                    "fszero: durable store is unusable and was NOT modified ({e}); continuing with in-memory recovery for this session. Durable features are degraded; run `fszero doctor` for the forensic and salvage paths."
                );
                let mut s = Self {
                    root: Some(root_path),
                    durable_degraded: true,
                    ..Self::default()
                };
                s.refresh_root_canon();
                s.publish_capabilities();
                if should_build_startup_index() {
                    let _ = s.build_index();
                }
                s
            }
            Err(e) => {
                panic!(
                    "fszero: durable store required ({e}); set FSZERO_ALLOW_EPHEMERAL=1 to opt into in-memory recovery"
                );
            }
        }
    }

    pub fn try_with_repo_store(root: impl AsRef<Path>) -> Result<Self, String> {
        let root_path = root.as_ref().to_path_buf();
        let mut recovery =
            prepare_repo_store(&root_path).and_then(RecoveryStore::try_with_durable)?;
        // Canonical shared CAS activation (fszero-zjt): presence of the
        // blobs/ dir under the effective store root IS the explicit opt-in;
        // FSZero never creates it implicitly.
        recovery.attach_cas_if_detected(&root_path);
        let mut s = Self {
            root: Some(root_path),
            recovery,
            ..Self::default()
        };
        s.complete_open_bootstrap();
        Ok(s)
    }

    pub fn with_durable_root(root: impl AsRef<Path>, db_path: impl AsRef<Path>) -> Self {
        let mut s = Self {
            root: Some(root.as_ref().to_path_buf()),
            recovery: RecoveryStore::with_durable(db_path),
            persist_ast_index: false,
            ..Self::default()
        };
        s.complete_open_bootstrap();
        s
    }

    /// Shared post-construct init for repo/durable open paths.
    fn complete_open_bootstrap(&mut self) {
        self.refresh_root_canon();
        if let Some(root) = self.root.clone() {
            if let Err(error) = self.recovery.reconcile_edit_intents(&root) {
                self.last_mutation_outcome = Some(crate::MutationOutcome::new(
                    crate::MutationState::Indeterminate,
                    "reopen",
                    Some(root.display().to_string()),
                ));
                eprintln!("fszero: edit-intent reconciliation indeterminate: {error}");
            }
        }
        self.bind_session_store();
        // Collapse any workspace left Partial by a kill mid multi-file publish
        // BEFORE the registry is rehydrated (fszero-k4ur.3).
        self.recover_committing_worlds();
        self.rehydrate_worlds();
        self.publish_capabilities();
        if should_build_startup_index() {
            let _ = self.build_index();
        }
    }

    /// Append `db` to `store_map` when not already present.
    fn push_store_map(&mut self, db: std::path::PathBuf) {
        if !self.store_map.iter().any(|p| p == &db) {
            self.store_map.push(db);
        }
    }

    /// Bind this session's durable store identity once (sticky for lifetime).
    fn bind_session_store(&mut self) {
        if let Some(db) = self.recovery.store_db_path() {
            let db = db.to_path_buf();
            let sid = super::zerostack_store::store_id_for_db_path(&db);
            self.bound_store_id = Some(sid);
            self.push_store_map(db);
        }
    }

    /// Rebuild in-memory WorldRegistry from the durable worlds tables.
    /// Pre/post bytes are loaded from content-addressed certs — no daemon.
    pub fn rehydrate_worlds(&mut self) {
        self.worlds.next_id = self.recovery.load_world_next_id();
        let rows = self.recovery.list_active_world_rows();
        for (wid, cert_ref, edit_rows) in rows {
            let mut edits = Vec::with_capacity(edit_rows.len());
            let mut ok = true;
            for (path_s, edit_cert) in edit_rows {
                match self.world_file_edit_from_cert(&path_s, &edit_cert) {
                    Ok(edit) => edits.push(edit),
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            }
            // Empty edits are valid (forked world awaiting stage). Corrupt
            // certs still drop the world (fszero-w2g.46 / INV-W1).
            if !ok {
                let _ = self.recovery.set_world_state(&wid, "dropped");
                continue;
            }
            let numeric = wid
                .strip_prefix('W')
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(0);
            self.worlds.next_id = self.worlds.next_id.max(numeric.saturating_add(1));
            self.worlds
                .active
                .insert(wid, super::world::WorldEdit { edits, cert_ref });
        }
    }

    fn world_file_edit_from_cert(
        &mut self,
        path_s: &str,
        cert_ref: &str,
    ) -> Result<super::world::WorldFileEdit, String> {
        let cert_bytes = self
            .recovery
            .expand(cert_ref)
            .ok_or_else(|| format!("missing world edit cert {cert_ref}"))?;
        let cert = String::from_utf8_lossy(&cert_bytes);
        let (_pre_ref, pre, _post_ref, post) = self.expand_cert_pre_post_payloads(&cert)?;
        let path = PathBuf::from(path_s);
        let (pre, post) = (
            String::from_utf8_lossy(&pre).into_owned(),
            String::from_utf8_lossy(&post).into_owned(),
        );
        let prefix = pre
            .char_indices()
            .zip(post.char_indices())
            .take_while(|((_, a), (_, b))| a == b)
            .map(|((i, ch), _)| i + ch.len_utf8())
            .last()
            .unwrap_or(0);
        let suffix = pre[prefix..]
            .chars()
            .rev()
            .zip(post[prefix..].chars().rev())
            .take_while(|(a, b)| a == b)
            .map(|(ch, _)| ch.len_utf8())
            .sum::<usize>();
        let old_end = pre.len().saturating_sub(suffix);
        let new_end = post.len().saturating_sub(suffix);
        let old = pre[prefix..old_end].to_string();
        let new = post[prefix..new_end].to_string();
        let hunk = super::world::hunk_lines(&pre, &old);
        Ok(super::world::WorldFileEdit {
            path,
            pre,
            post,
            cert_ref: cert_ref.to_string(),
            old,
            new,
            hunk,
        })
    }

    /// Change the workspace root for FS ops without rebinding the durable store.
    ///
    /// One session, one store map: refs minted before the switch still expand
    /// from the bound store (fszero-store-root-fragmentation-jdl).
    pub fn set_workspace_root(&mut self, root: impl AsRef<Path>) -> Result<(), String> {
        let root_path = root.as_ref();
        if root_path.as_os_str().is_empty() {
            return Err("empty workspace root".to_string());
        }
        let resolved = if root_path.exists() {
            fs::canonicalize(root_path)
                .map_err(|e| format!("bad workspace root {}: {e}", root_path.display()))?
        } else if root_path.is_absolute() {
            root_path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|e| format!("cwd unreadable while resolving workspace root: {e}"))?
                .join(root_path)
        };
        self.root = Some(resolved);
        self.refresh_root_canon();
        // Do NOT reopen recovery / rebind store_id — that is the fragmentation bug.
        self.index_initialized = false;
        self.caches = SessionCaches::default();
        Ok(())
    }

    /// Refresh cached `root_canon` after any root assignment.
    pub fn refresh_root_canon(&mut self) {
        self.root_canon = self.root.as_ref().and_then(|r| {
            fs::canonicalize(r)
                .ok()
                .or_else(|| r.is_absolute().then(|| r.clone()))
        });
    }

    /// Stable store identity for this session's bound durable store.
    pub fn store_id(&self) -> Option<&str> {
        self.bound_store_id.as_deref()
    }

    /// Register another durable store db into this session's store map
    /// (e.g. after discovering a minting store via ref-index metadata).
    pub fn register_store_db(&mut self, db_path: impl AsRef<Path>) {
        let db = db_path.as_ref().to_path_buf();
        if db.as_os_str().is_empty() {
            return;
        }
        self.push_store_map(db);
    }

    pub fn ensure_index_built(&mut self) -> Result<(), String> {
        if !self.index_initialized {
            self.build_index()?;
        }
        Ok(())
    }

    /// When `root` is set: ensure index, then fold stale files. Shared by search/resolve/compound.
    pub fn prepare_index_for_root(&mut self, root: Option<&Path>) -> Result<(), String> {
        if root.is_none() {
            return Ok(());
        }
        self.ensure_index_built()?;
        self.refresh_stale_index_files();
        Ok(())
    }

    /// Prepare index or return `X0 busy …` visible failure string.
    #[inline]
    pub fn prepare_index_or_busy(&mut self, root: Option<&Path>) -> Result<(), String> {
        self.prepare_index_for_root(root)
            .map_err(|e| format!("X0 busy {e}"))
    }

    pub fn record_internal_op(&mut self) {
        self.op_count += 1;
        self.version += 1;
    }

    pub fn require_root(&self) -> Result<&Path, String> {
        self.root
            .as_deref()
            .ok_or_else(|| "no root: set FSZERO_ROOT or use with_root/with_repo_store".to_string())
    }

    /// Effective workspace root for FS ops (CLI `--root` / `FSZERO_ROOT` / session ctor).
    pub fn workspace_root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Durable store SQLite path when present (may live under a shared
    /// `ZEROSTACK_STORE_ROOT`, distinct from the workspace root).
    pub fn store_db_path(&self) -> Option<&Path> {
        self.recovery.store_db_path()
    }

    /// Explicit legacy→CAS migration trigger (fszero-c6q.3): publish every
    /// verified `fz://blob` payload row of this session's store into the
    /// attached canonical CAS. Idempotent; never deletes or rewrites legacy
    /// state. Surfaced on the CLI as `fszero migrate-cas [--root PATH]`.
    pub fn migrate_blobs_to_cas(&mut self) -> Result<super::recovery::MigrationReport, String> {
        self.recovery.migrate_blobs_to_cas()
    }

    /// Parent directory of the durable store DB (or the unified store root
    /// when using `…/fszero/store.sqlite3`). `None` when in-memory.
    pub fn store_root(&self) -> Option<PathBuf> {
        super::zerostack_store::store_root_from_db_path(self.recovery.store_db_path()?)
    }

    /// Structured root report for doctor/telemetry (workspace vs store).
    pub fn root_report(&self) -> serde_json::Value {
        let workspace = self
            .root
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(none)".into());
        let store_db = self
            .recovery
            .store_db_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "memory".into());
        let store_root = self
            .store_root()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| {
                if self.durable_degraded || self.recovery.store_db_path().is_none() {
                    "memory".into()
                } else {
                    store_db.clone()
                }
            });
        let effective_root_mode = super::zerostack_store::effective_root_mode(&store_root);
        let cap = self.capability_descriptor();
        let (
            layout_version,
            store_health,
            migration_legacy,
            peer_incompatibility,
            last_integrity_error,
        ) = self
            .recovery
            .root_report_store_fragments(self.durable_degraded, &cap);
        let store_id = self.bound_store_id.clone().unwrap_or_else(|| {
            self.store_root()
                .map(|p| super::zerostack_store::store_id_for_path(&p))
                .unwrap_or_else(|| "memory".into())
        });
        let store_map: Vec<_> = self
            .store_map
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        let roots_distinct = self.root.as_ref().is_some_and(|w| {
            self.store_root().is_some_and(|s| {
                fs::canonicalize(w).ok() != fs::canonicalize(&s).ok() && !s.starts_with(w)
            })
        });
        serde_json::json!({
            "workspace_root": workspace, "store_root": store_root, "store_db": store_db, "store_id": store_id, "store_map": store_map, "durable_degraded": self.durable_degraded,
            "roots_distinct": roots_distinct, "effective_root_mode": effective_root_mode, "layout_version": layout_version, "store_health": store_health,
            "fz_runtime_health": self.runtime_health.to_json(), "migration_legacy": migration_legacy, "peer_incompatibility": peer_incompatibility,
            // ZeroRef v1 capability negotiation (fszero-c6q.5): the same
            // descriptor peers expand from the "capabilities" store key.
            "capabilities": cap,
            // Normative filesystem semantics, shared byte-for-byte by embedded,
            // CLI doctor, MCP, and CodeMode expansion surfaces.
            "filesystem_contract": super::filesystem_contract::filesystem_contract_descriptor(),
            // Canonical operation ABI + deterministic digest (fszero-ncib.1).
            "operation_abi": super::operation_abi::operation_abi_descriptor(), "fsqlite_prepared_cache": super::recovery::prepared_cache_metrics_json(), "sql_profile": super::recovery::sql_profile_json(), "last_integrity_error": last_integrity_error,
        })
    }

    pub fn expand(&self, r: &str) -> Option<Vec<u8>> {
        let sticky = match r {
            "search" => self.views.last_search_payload.as_ref(),
            "expand" => self.views.last_expand_payload.as_ref(),
            "compound" => self.views.last_compound_payload.as_ref(),
            _ => None,
        };
        sticky
            .cloned()
            .or_else(|| self.expand_read_view(r))
            .or_else(|| self.expand_batch_row_alias(r))
            .or_else(|| self.expand_via_store_map(r))
    }

    fn expand_batch_row_alias(&self, r: &str) -> Option<Vec<u8>> {
        let suffix = r.strip_prefix("codemode/batch/")?;
        let (batch_id, index) = suffix.rsplit_once('/')?;
        let index = index.parse::<usize>().ok()?;
        let batch = self.expand_via_store_map(&format!("codemode/batch/{batch_id}"))?;
        let rows: serde_json::Value = serde_json::from_slice(&batch).ok()?;
        let row = rows.as_array()?.get(index)?;
        if let Some(source_ref) = row.get("source_ref").and_then(serde_json::Value::as_str) {
            return self.expand_via_store_map(source_ref);
        }
        (!row
            .get("ok")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true))
        .then(Vec::new)
    }

    /// Expand consulting the bound store, then other session store-map entries,
    /// then the per-user ref index (via RecoveryStore::expand tiers).
    fn expand_via_store_map(&self, r: &str) -> Option<Vec<u8>> {
        self.recovery
            .expand(r)
            .or_else(|| self.with_other_mapped_stores(|remote| remote.expand(r)))
    }

    /// Open each store-map entry other than the session's current DB and try `f`.
    pub fn with_other_mapped_stores<T>(
        &self,
        mut f: impl FnMut(&RecoveryStore) -> Option<T>,
    ) -> Option<T> {
        let current = self.recovery.store_db_path();
        for db in &self.store_map {
            if current.is_some_and(|c| c == db.as_path()) {
                continue;
            }
            if let Ok(remote) = RecoveryStore::try_open_existing_durable_pub(db) {
                if let Some(v) = f(&remote) {
                    return Some(v);
                }
            }
        }
        None
    }

    pub fn expand_read_view(&self, r: &str) -> Option<Vec<u8>> {
        let rest = r.strip_prefix("view_").or_else(|| r.strip_prefix('r'))?;
        let (id, kind) = rest.split_once('/')?;
        let view_id = id.parse::<u32>().ok()?;
        let view = self.views.views.get(&view_id)?;
        match kind {
            "path" => Some(view.path.to_string_lossy().as_bytes().to_vec()),
            "ref" => Some(view.content_ref.as_bytes().to_vec()),
            "bytes" => self.recovery.expand(&view.content_ref),
            _ => None,
        }
    }

    pub fn has_recovery_payloads(&self) -> bool {
        !self.recovery.list_keys().is_empty()
    }

    pub fn list_recovery_refs(&self) -> Vec<String> {
        self.recovery.list_keys()
    }

    pub fn facts_for(&self, subject_ref: &str) -> Vec<String> {
        self.recovery.facts_for(subject_ref)
    }

    pub fn record_fact(
        &mut self,
        subject_ref: &str,
        predicate: &str,
        object_ref: &str,
        evidence_ref: &str,
        agent: &str,
    ) {
        self.recovery.put_fact(
            subject_ref,
            predicate,
            object_ref,
            evidence_ref,
            self.version,
            agent,
        );
    }

    pub fn reindex(&mut self) -> Result<(), String> {
        self.build_index()
    }

    /// Snapshot of the most recent index build (warm vs cold, dirty counts).
    pub fn last_index_build(&self) -> IndexBuildReport {
        self.last_index_build
    }

    pub fn last_index_refresh(&self) -> IndexRefreshReport {
        self.last_index_refresh
    }

    pub fn indexed_file_count(&self) -> usize {
        self.index.indexed_file_keys.len()
    }

    pub fn store_error_suffix(&mut self, prefix: &str) -> Option<String> {
        self.recovery
            .take_store_error()
            .map(|e| format!("{prefix}:0 (store failed: {e})"))
    }

    pub fn enforce_ms_budget(&mut self, start: Instant, op: &str) -> Option<String> {
        let cap_ms = super::budget::budget_ms_cap()?;
        let elapsed_us = start.elapsed().as_micros();
        if elapsed_us > cap_ms as u128 * 1000 {
            let scanned = (elapsed_us / 1000).max(1) as usize;
            self.store_budget_evidence(op, "ms", cap_ms, scanned);
            Some(format!("budget:0 ms cap={cap_ms} scanned={scanned}"))
        } else {
            None
        }
    }

    /// Restore a file during CodeMode plan transaction rollback. `mtime`,
    /// `perms`, and `xattrs` (when snapshotted) are restored too, so a
    /// rolled-back plan leaves metadata bit-perfect (fszero-md6 / 7be / l4g).
    pub fn restore_file_for_rollback(
        &mut self,
        path: &std::path::Path,
        bytes: &[u8],
        mtime: Option<std::time::SystemTime>,
        perms: Option<std::fs::Permissions>,
        xattrs: Option<String>,
    ) -> Result<(), String> {
        let root = self.require_root()?;
        let validated = validate_rollback_path(root, path)?;
        // FIFO/socket write and mtime open block; refuse from metadata first.
        crate::path::refuse_non_regular_file(&validated)?;
        std::fs::write(&validated, bytes).map_err(|e| e.to_string())?;
        if let Some(p) = perms {
            std::fs::set_permissions(&validated, p).map_err(|e| format!("mode restore: {e}"))?;
        }
        if let Some(x) = xattrs {
            restore_xattrs(&validated, &x).map_err(|e| format!("xattr restore: {e}"))?;
        }
        if let Some(t) = mtime {
            std::fs::OpenOptions::new()
                .write(true)
                .open(&validated)
                .and_then(|f| f.set_modified(t))
                .map_err(|e| format!("mtime restore: {e}"))?;
        }

        self.refresh_path_after_mutation(&validated);
        Ok(())
    }

    /// Remove a file created during a failed transactional plan.
    pub fn remove_file_for_rollback(&mut self, path: &std::path::Path) -> Result<(), String> {
        let root = self.require_root()?;
        let validated = validate_rollback_path(root, path)?;
        if validated.exists() {
            std::fs::remove_file(&validated).map_err(|e| e.to_string())?;
            self.refresh_path_after_mutation(&validated);
        }
        Ok(())
    }

    pub fn drop_active_world(&mut self, wid: &str) -> bool {
        self.worlds.active.remove(wid).is_some()
    }

    pub fn world_edit_paths(&self, wid: &str) -> Vec<std::path::PathBuf> {
        self.worlds
            .active
            .get(wid)
            .map(|world| world.edits.iter().map(|edit| edit.path.clone()).collect())
            .unwrap_or_default()
    }
}

pub fn prepare_repo_store(root: &Path) -> Result<PathBuf, String> {
    let root_canon = canonicalize_root(root)?;
    // zerostack-pi1: the precedence fix can strand a DB at the location the
    // old pin-first, ungated resolver chose. Adopt it before opening, so the
    // fix never looks like data loss. Never overwrites an existing DB.
    let _ = super::store_migration::adopt_superseded_store(&root_canon);
    let unified = super::zerostack_store::zerostack_store_or_detect(&root_canon).is_some();
    let db_path = super::zerostack_store::fszero_store_sqlite_path(&root_canon);
    if unified {
        if let Some(store) = super::zerostack_store::zerostack_store_or_detect(&root_canon) {
            if super::zerostack_store::store_is_global_host(&store, &root_canon) {
                // Per-repo shard under the global host (kflx isolation).
                super::zerostack_store::ensure_repo_metadata_layout(&store, &root_canon)?;
                // Crash-safe, non-guessing migration of legacy global metadata DB.
                let _ = super::store_migration::migrate_legacy_global_store(
                    &store,
                    &[root_canon.as_path()],
                );
            } else {
                super::zerostack_store::ensure_unified_store_layout(&store)?;
            }
        }
    } else if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create .fszero failed: {e}"))?;
        // Self-ignoring store dir (fszero-4wt): a `*` gitignore INSIDE
        // .fszero keeps the store out of `git status` without touching the
        // repo's own .gitignore — no tool can tell FSZero keeps state here.
        let marker = parent.join(".gitignore");
        if !marker.exists() {
            let _ = fs::write(&marker, "*\n");
        }
    }
    if std::env::var("FSZERO_WRITE_GITIGNORE").ok().as_deref() == Some("1") {
        let _ = super::zerostack_store::ensure_repo_gitignore(&root_canon, unified);
    }
    Ok(db_path)
}

pub fn estimate_visible_tokens(s: &str) -> usize {
    if s.is_empty() {
        return 0;
    }
    const ONES: &[&str] = &[
        "L", "R", "S", "E", "C", "X", "W", "L1", "R1", "R17", "R42", "S1", "S5", "E1", "Eok",
        "X17", "E0", "X0", "W1", "W17", "!", "OK",
    ];
    if ONES.contains(&s) {
        return 1;
    }
    if s.len() <= 3
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "!?-".contains(c))
    {
        return 1;
    }
    s.len().div_ceil(4)
}
