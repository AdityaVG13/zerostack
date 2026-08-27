use super::super::session::{FSZeroSession, ReadCacheEntry};
use super::adapter::{ExtractedFile, extract_structural};
use super::types::{IndexedFile, IndexedFn, IndexedImport};
use super::walk::{
    is_ignored_dir_name, is_structural_file_key, is_structural_source_file,
    is_supported_source_file, looks_binary_bytes, relative_file_key, relative_file_key_fast,
    walk_rs_files_with_report,
};
use crate::subsystems::{IndexBuildReport, IndexRefreshReport};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::mem;
use std::path::Path;
use std::process::Command;

/// Durable key holding the AST generation plus per-file (mtime, len)
/// signatures from the last persisted index build.
const INDEX_MANIFEST_KEY: &str = "ast/index_manifest";
const INDEX_ROOT_FINGERPRINT_KEY: &str = "ast/index_root_fingerprint";
/// Paths that were dirty vs HEAD when last indexed. Next one-shot process
/// must re-stat these even if `git status` is now clean (revert-to-HEAD).
const INDEX_DIRTY_KEYS_KEY: &str = "ast/index_dirty_keys";

/// Schema of persisted AST rows / extraction contract. Bump when derived row
/// shape or extraction semantics change so stores invalidate cleanly without
/// wiping unrelated durable content (fszero-1it / cocoindex-style memo).
const INDEX_SCHEMA_VERSION: u32 = 2;

/// Maximum number of files ingested in one batch. Cold 100k-file builds stay
/// memory-bounded while still amortizing the per-batch merge and persist.
const INGEST_BATCH_FILES: usize = 131072;

/// Per-phase wall attribution for build_index, emitted as one JSON line on
/// stderr when FSZERO_INDEX_PHASES is set (fszero-xez scaling analysis).
/// Zero-cost when the env var is absent.
struct PhaseTimer {
    enabled: bool,
    started: std::time::Instant,
    last: std::time::Instant,
    entries: Vec<(&'static str, f64)>,
}

impl PhaseTimer {
    fn new() -> Self {
        let now = std::time::Instant::now();
        Self {
            enabled: std::env::var_os("FSZERO_INDEX_PHASES").is_some(),
            started: now,
            last: now,
            entries: Vec::new(),
        }
    }

    fn mark(&mut self, name: &'static str) {
        if !self.enabled {
            return;
        }
        let now = std::time::Instant::now();
        self.entries
            .push((name, (now - self.last).as_secs_f64() * 1e3));
        self.last = now;
    }

    fn accumulate(&mut self, name: &'static str, dur: std::time::Duration) {
        if !self.enabled {
            return;
        }
        self.entries.push((name, dur.as_secs_f64() * 1e3));
        self.last = std::time::Instant::now();
    }

    fn finish(self, counts: serde_json::Value) {
        if !self.enabled {
            return;
        }
        let phases: serde_json::Map<String, serde_json::Value> = self
            .entries
            .into_iter()
            .map(|(name, ms)| (name.to_string(), serde_json::json!(ms)))
            .collect();
        eprintln!(
            "{}",
            serde_json::json!({
                "index_phases_ms": phases, "total_ms": (std::time::Instant::now() - self.started).as_secs_f64() * 1e3,
                "counts": counts,
            })
        );
    }
}

type FileSig = (u128, u64);

/// Memo identity for the AST index producer: engine commit + schema + the
/// config knobs that change which files are walked or how they are extracted.
/// Override with `FSZERO_INDEX_PRODUCER` (tests / forced invalidation).
fn index_producer_fingerprint() -> String {
    if let Ok(over) = std::env::var("FSZERO_INDEX_PRODUCER") {
        let trimmed = over.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let commit = option_env!("FSZERO_COMMIT").unwrap_or("unknown");
    let ast_sgrep = cfg!(feature = "fszero-ast-sgrep") as u8;
    let semantic_local = cfg!(feature = "fszero-semantic-local") as u8;
    // When the opt-in tier is compiled in, pin the local embedder id so a
    // backend swap invalidates derived semantic CAS rows (fszero-1it / 9wn).
    #[cfg(feature = "fszero-semantic-local")]
    let semantic_embedder = crate::semantic_local::EMBEDDER_ID;
    #[cfg(not(feature = "fszero-semantic-local"))]
    let semantic_embedder = "off";
    let skip_gitignore = match std::env::var("FSZERO_SKIP_GITIGNORE").ok().as_deref() {
        Some("1") | Some("true") | Some("TRUE") => 1u8,
        _ => 0u8,
    };
    let material = format!(
        "commit={commit}\nschema={INDEX_SCHEMA_VERSION}\nast_sgrep={ast_sgrep}\nsemantic_local={semantic_local}\nsemantic_embedder={semantic_embedder}\nskip_gitignore={skip_gitignore}\n"
    );
    let digest = Sha256::digest(material.as_bytes());
    crate::hexutil::sha256_hex_of(digest.into())[..16].to_string()
}

fn root_fingerprint(root: &Path) -> Option<String> {
    // This gate is deliberately Git-only. Directory mtimes cover direct-child
    // changes, while HEAD/index evidence covers committed and staged deep
    // changes. Without a real .git directory we cannot prove freshness cheaply,
    // so callers must walk. Unstaged deep edits are handled by watch mode or a
    // subsequent directory/index change; this is an opportunistic cold-process
    // startup optimization, not a replacement for active change observation.
    let git = root.join(".git");
    if !git.is_dir() {
        return None;
    }
    let mut entries = Vec::new();
    entries.push((String::new(), sig_of(&fs::metadata(root).ok()?)));
    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).ok()?;
        if meta.is_dir() {
            entries.push((
                entry.file_name().to_string_lossy().into_owned(),
                sig_of(&meta),
            ));
        }
    }
    entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));

    let head_path = git.join("HEAD");
    let index_path = git.join("index");
    // FIFO/socket open blocks; skip the cheap fingerprint rather than hang.
    crate::path::refuse_non_regular_file(&head_path).ok()?;
    let head = fs::read(&head_path).ok()?;
    let head_sig = sig_of(&fs::metadata(head_path).ok()?);
    let index_sig = sig_of(&fs::metadata(index_path).ok()?);
    let mut digest = Sha256::new();
    for (name, (mtime, len)) in entries {
        digest.update(name.as_bytes());
        digest.update(mtime.to_le_bytes());
        digest.update(len.to_le_bytes());
    }
    digest.update(&head);
    digest.update(head_sig.0.to_le_bytes());
    digest.update(head_sig.1.to_le_bytes());
    digest.update(index_sig.0.to_le_bytes());
    digest.update(index_sig.1.to_le_bytes());
    Some(crate::hexutil::sha256_hex_of(digest.finalize().into()))
}

fn force_full_index_refresh() -> bool {
    matches!(
        std::env::var("FSZERO_INDEX_REFRESH").ok().as_deref(),
        Some("full") | Some("FULL")
    )
}

fn rel_has_ignored_component(rel: &str) -> bool {
    rel.split(['/', '\\']).any(is_ignored_dir_name)
}

/// Porcelain v1 `-z` records: `XY path\0`, or `XY orig\0new\0` for rename/copy.
fn parse_git_porcelain_z(bytes: &[u8]) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut rest = bytes;
    while rest.len() >= 3 {
        let x = rest[0];
        let y = rest[1];
        let mut i = 2;
        if rest.get(i) == Some(&b' ') {
            i += 1;
        }
        if i >= rest.len() {
            break;
        }
        let tail = &rest[i..];
        let Some(end) = tail.iter().position(|&b| b == 0) else {
            break;
        };
        let path = String::from_utf8_lossy(&tail[..end]).replace('\\', "/");
        if !path.is_empty() && !rel_has_ignored_component(&path) {
            out.insert(path);
        }
        rest = &tail[end + 1..];
        if matches!(x, b'R' | b'C') || matches!(y, b'R' | b'C') {
            let Some(end2) = rest.iter().position(|&b| b == 0) else {
                break;
            };
            let path2 = String::from_utf8_lossy(&rest[..end2]).replace('\\', "/");
            if !path2.is_empty() && !rel_has_ignored_component(&path2) {
                out.insert(path2);
            }
            rest = &rest[end2 + 1..];
        }
    }
    out
}

/// Worktree paths git considers dirty, staged, or untracked. `None` when
/// this is not a usable git checkout (caller must fall back to a full stat).
fn git_worktree_delta(root: &Path) -> Option<HashSet<String>> {
    if !root.join(".git").exists() {
        return None;
    }
    let output = Command::new("git")
        .args([
            "-C",
            &root.to_string_lossy(),
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "-z",
            "-uall",
            "--no-renames",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_git_porcelain_z(&output.stdout))
}

fn load_dirty_keys(bytes: Option<Vec<u8>>) -> HashSet<String> {
    bytes
        .and_then(|raw| String::from_utf8(raw).ok())
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn render_dirty_keys(keys: &HashSet<String>) -> Vec<u8> {
    let mut lines: Vec<&str> = keys.iter().map(String::as_str).collect();
    lines.sort_unstable();
    lines.join("\n").into_bytes()
}

fn sig_of(meta: &fs::Metadata) -> FileSig {
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    (mtime, meta.len())
}

fn parse_index_manifest(bytes: &[u8]) -> Option<(u64, String, HashMap<String, FileSig>)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.lines();
    let generation = lines.next()?.strip_prefix("gen=")?.parse().ok()?;
    let second = lines.next();
    let (producer, first_sig) = match second {
        Some(line) => match line.strip_prefix("producer=") {
            Some(rest) => (rest.to_string(), None),
            None => (String::new(), Some(line)),
        },
        None => (String::new(), None),
    };
    let mut sigs = HashMap::new();
    let mut push_sig = |line: &str| -> Option<()> {
        let mut parts = line.splitn(3, '\t');
        let key = parts.next()?;
        let mtime = parts.next()?.parse().ok()?;
        let len = parts.next()?.parse().ok()?;
        sigs.insert(key.to_string(), (mtime, len));
        Some(())
    };
    if let Some(line) = first_sig {
        push_sig(line)?;
    }
    for line in lines {
        push_sig(line)?;
    }
    Some((generation, producer, sigs))
}

fn render_index_manifest(generation: u64, producer: &str, sigs: &[(String, FileSig)]) -> String {
    use std::fmt::Write;
    let mut out = format!("gen={generation}\nproducer={producer}\n");
    for (key, (mtime, len)) in sigs {
        let _ = writeln!(out, "{key}\t{mtime}\t{len}");
    }
    out
}

/// Pending call edges for a single file. Kept after the per-batch parse so
/// the final call insert can be filtered through the complete symbol set
/// after the persisted-symbol merge.
struct PendingFileCalls {
    file_key: String,
    calls: Vec<(String, String, usize)>,
}

/// Insert call edges whose callee is in the known symbol set (shared build + refresh).
fn insert_known_call_edges(
    ast: &crate::ast_store::AstStore,
    file_key: &str,
    calls: &[(String, String, usize)],
    known: &HashSet<&str>,
    generation: i64,
) {
    for (caller, callee, line) in calls {
        if known.contains(callee.as_str()) {
            ast.insert_call_edge(file_key, caller, callee, *line as i64, generation);
        }
    }
}

/// Thread-safe per-file processing: read, validate, and parse structural data.
/// No mutation of shared state — the sequential merge in build_index handles that.
fn ingest_one_file(path: &Path, do_parse: bool) -> Option<Option<ExtractedFile>> {
    // Warm start clean files skip the read entirely; content refs stay lazy
    // for both warm and cold indexes.
    if !do_parse {
        return None;
    }
    // Indexer may skip FIFOs/sockets; hanging on open is not allowed.
    crate::path::refuse_non_regular_file(path).ok()?;
    let bytes = fs::read(path).ok()?;
    if looks_binary_bytes(&bytes) {
        return None;
    }
    let txt = String::from_utf8(bytes).ok()?;
    if is_structural_source_file(path) {
        Some(Some(extract_structural(path, &txt)))
    } else {
        Some(None)
    }
}

/// Parallel file ingestion over a bounded rayon pool: read + parse fan out with
/// work-stealing while session mutations and DB writes stay sequential. Output
/// order is deterministic because rayon's ordered collect preserves input order.
/// Cold builds default to at most four threads. FSZERO_INDEX_THREADS overrides
/// the cap; FSZERO_INGEST_THREADS remains available for ingest benchmarks.
fn ingest_map_item(
    i: usize,
    path: &std::path::Path,
    file_key: &str,
    dirty: &HashSet<&str>,
    incremental: bool,
) -> Option<(usize, Option<ExtractedFile>)> {
    let is_clean = incremental && !dirty.contains(file_key);
    let extracted = ingest_one_file(path, !is_clean)?;
    Some((i, extracted))
}

fn parallel_ingest_files(
    files: &[(std::path::PathBuf, String, FileSig)],
    dirty: &HashSet<&str>,
    incremental: bool,
) -> Vec<(usize, Option<ExtractedFile>)> {
    let env_threads = crate::budget::env_usize("FSZERO_INDEX_THREADS")
        .or_else(|| crate::budget::env_usize("FSZERO_INGEST_THREADS"));
    let available = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    // Profiled on M5 Max (fszero-gauntlet-r0): with tree-sitter extraction the
    // old min(4, cores/2) cap left P-cores idle during cold ingest; wall dropped
    // ~1572ms → ~500ms at 16 threads and plateaued past that. Default to all
    // cores — ingest is read+parse fan-out with sequential merge/persist around
    // it — still env-overridable (the explicit override above wins).
    //
    // Without `fszero-ast-sgrep` (packaging-shim builds), extraction is the
    // line-heuristic (~µs/file) and pool coordination costs more than the work
    // (profiled: CLI cold index +1.6% from a 16-thread spawn for ~5ms of work),
    // so those builds stay serial unless an override demands otherwise.
    #[cfg(not(feature = "fszero-ast-sgrep"))]
    let default_threads = 1;
    #[cfg(feature = "fszero-ast-sgrep")]
    let default_threads = available;
    let num_threads = env_threads.unwrap_or(default_threads);

    if num_threads <= 1 || files.len() < 64 {
        return files
            .iter()
            .enumerate()
            .filter_map(|(i, (path, file_key, _))| {
                ingest_map_item(i, path, file_key, dirty, incremental)
            })
            .collect();
    }

    use rayon::prelude::*;

    // Persistent pool: building a fresh ThreadPool per call re-spawns workers
    // and strands the ast-sgrep-lang thread-local tree-sitter parser caches on
    // dead threads, so every build re-paid parser construction. One
    // process-global pool keeps those caches warm across builds. Sizing inputs
    // are read at FIRST parallel use (same env contract; later FSZERO_INDEX_THREADS
    // edits don't resize — same class as LazyBigramIndex init-time env reads).
    static INGEST_POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    let run = || {
        files
            .par_iter()
            .enumerate()
            .filter_map(|(i, (path, file_key, _))| {
                ingest_map_item(i, path, file_key, dirty, incremental)
            })
            .collect::<Vec<_>>()
    };
    let pool = INGEST_POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .build()
            .expect("ingest thread pool")
    });
    pool.install(run)
}

/// Cross-process advisory lock serializing index builds over one store
/// (fszero-hym). Lock file lives next to the store DB so every child of the
/// same root contends on the same inode. Any failure (no durable store,
/// unwritable dir, lock error) degrades to the unlocked status quo — the
/// build itself never depends on the lock.
fn acquire_index_build_lock(
    store_db: Option<&Path>,
    blocked: &mut bool,
) -> Result<Option<std::fs::File>, String> {
    if std::env::var("FSZERO_INDEX_LOCK").ok().as_deref() == Some("0") {
        return Ok(None);
    }
    let wait_ms = std::env::var("FSZERO_INDEX_LOCK_WAIT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120_000);
    acquire_index_build_lock_with_wait(store_db, blocked, wait_ms)
}

fn acquire_index_build_lock_with_wait(
    store_db: Option<&Path>,
    blocked: &mut bool,
    wait_ms: u64,
) -> Result<Option<std::fs::File>, String> {
    let Some(lock_path) = store_db.map(|path| path.with_extension("indexlock")) else {
        return Ok(None);
    };
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&lock_path)
        .map_err(|e| {
            format!(
                "machine_permit_io: open index lock {}: {e}",
                lock_path.display()
            )
        })?;
    if file.try_lock().is_err() {
        // Contended: another process is building right now. Record that we
        // blocked so the caller can refresh its store connection afterwards
        // (fszero-fi2) and take the incremental path off the winner's build.
        *blocked = true;
        let wait_started = std::time::Instant::now();
        // Never proceed unlocked after contention: that multiplies a cold build
        // across every waiter. Zero retains the explicit wait-forever mode; the
        // long default returns a retryable busy error when the holder is stuck.
        let result = if wait_ms == 0 {
            file.lock()
                .map_err(|e| format!("machine_permit_io: lock {}: {e}", lock_path.display()))
        } else {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(wait_ms);
            loop {
                if file.try_lock().is_ok() {
                    break Ok(());
                }
                if std::time::Instant::now() >= deadline {
                    break Err(format!(
                        "machine_permit_busy: index build lock {} remained contended for {wait_ms}ms \
(raise wait with FSZERO_INDEX_LOCK_WAIT_MS; set FSZERO_INDEX_LOCK=0 to opt out of the single-indexer lock; \
lock file is store.db.indexlock beside the durable store)",
                        lock_path.display()
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        };
        crate::runtime_metrics::record_index_lock_wait(wait_started.elapsed().as_micros() as u64);
        result?;
    }
    Ok(Some(file))
}

impl FSZeroSession {
    pub fn build_index(&mut self) -> Result<(), String> {
        self.build_index_inner()
    }

    fn build_index_inner(&mut self) -> Result<(), String> {
        let mut phases = PhaseTimer::new();
        self.index.symbols.clear();
        self.index.indexed_file_keys.clear();
        self.index.file_sig.clear();
        // Lazy bigrams are query-filled / incrementally upserted — never bulk
        // rebuilt here (fszero-up8 / 9ot). Drop stale entries on rebuild.
        self.lazy_bigrams = crate::search_prefilter_eval::LazyBigramIndex::new();
        self.clear_query_caches();
        let Some(root) = self.root.clone() else {
            self.index.ast_generation += 1;
            self.index_initialized = true;
            self.watch_reconcile.clear_pre_index_deferred();
            return Ok(());
        };

        // Single-indexer guard (fszero-hym): concurrent children cold-indexing
        // the same root each fan read+parse across every core — N× the CPU
        // and thermal load for one usable index. The first process to take
        // the lock pays the cold build; blocked processes acquire after its
        // commit, see the fresh manifest, and fall through to the incremental
        // (near-zero-write) path. FSZERO_INDEX_LOCK=0 opts out. Released on
        // drop (fd close), including on panic.
        let mut lock_blocked = false;
        let _index_lock = if self.persist_ast_index {
            acquire_index_build_lock(self.recovery.store_db_path(), &mut lock_blocked)?
        } else {
            None
        };
        if lock_blocked {
            self.recovery.reopen_durable();
        }
        phases.mark("index_lock");

        let expected_producer = index_producer_fingerprint();
        let manifest = if self.persist_ast_index {
            self.recovery
                .expand(INDEX_MANIFEST_KEY)
                .and_then(|bytes| parse_index_manifest(&bytes))
        } else {
            None
        };
        let cheap_fingerprint = root_fingerprint(&root);
        let stored_fingerprint = self
            .recovery
            .expand(INDEX_ROOT_FINGERPRINT_KEY)
            .and_then(|bytes| String::from_utf8(bytes).ok());
        if let (Some((generation, producer, sigs)), Some(current_fingerprint)) =
            (manifest.as_ref(), cheap_fingerprint.as_ref())
        {
            let rows_present =
                self.recovery.ast.has_rows() || !sigs.keys().any(|key| is_structural_file_key(key));
            if producer == &expected_producer
                && stored_fingerprint.as_deref() == Some(current_fingerprint.as_str())
                && rows_present
            {
                self.index.ast_generation = *generation;
                for (key, sig) in sigs {
                    self.index.indexed_file_keys.insert(key.clone());
                    let mtime = std::time::UNIX_EPOCH
                        + std::time::Duration::from_nanos(sig.0.min(u64::MAX as u128) as u64);
                    self.index.file_sig.insert(key.clone(), (mtime, sig.1));
                }
                self.index.symbols = self.recovery.ast.query_all_symbols(*generation as i64);
                self.last_index_build = IndexBuildReport {
                    mode: "warm_skip",
                    incremental: true,
                    producer_matched: true,
                    dirty: 0,
                    removed: 0,
                    ingested: 0,
                    files_walked: 0,
                    truncated: false,
                };
                self.index_initialized = true;
                self.watch_reconcile.clear_pre_index_deferred();
                phases.finish(serde_json::json!({ "mode": "warm_skip", "files_walked": 0, "producer_matched": true, }));
                return Ok(());
            }
        }

        // One stat per file, reused for both walk filtering and signatures.
        let walk = walk_rs_files_with_report(&root);
        let walk_truncated = walk.truncated;
        let current: Vec<(std::path::PathBuf, String, FileSig)> = walk
            .files
            .into_iter()
            .map(|(path, meta)| {
                let file_key = relative_file_key_fast(&root, &path);
                let sig = sig_of(&meta);
                (path, file_key, sig)
            })
            .collect();
        if walk_truncated {
            eprintln!(
                "fszero: warning: index walk truncated at FSZERO_INDEX_MAX_FILES={} (walked {} files)",
                crate::ast::walk::walk_max_files(),
                current.len()
            );
        }
        phases.mark("walk");

        // Manifest-driven incremental persistence: files whose (mtime, len)
        // signature matches the stored manifest keep their existing AST rows
        // and blobs; only changed/new/removed files touch the durable store.
        // The manifest is also keyed by a producer fingerprint (engine commit
        // + schema + relevant config) so a binary/schema/config upgrade
        // invalidates derived AST rows without requiring a wiped store
        // (fszero-1it). Profiled: the fsqlite pager (get_page/write_page_data)
        // was ~85% of a fresh 10k-file index, and the legacy clear-and-rewrite
        // ran on EVERY process start, so an unchanged workspace must cost
        // zero DB writes here.
        let (generation, prev_sigs, mut incremental, producer_matched) = match manifest {
            Some((generation, producer, sigs)) if producer == expected_producer => {
                (generation, sigs, true, true)
            }
            Some((generation, _, _)) => {
                self.index.ast_generation = generation.saturating_add(1).max(1);
                (self.index.ast_generation, HashMap::new(), false, false)
            }
            None => {
                self.index.ast_generation += 1;
                (self.index.ast_generation, HashMap::new(), false, false)
            }
        };
        if incremental
            && !self.recovery.ast.has_rows()
            && current
                .iter()
                .any(|(_, key, _)| is_structural_file_key(key))
        {
            // Manifest without rows means the store was wiped or the persist
            // failed midway: rebuild everything — LOUDLY (fszero-krl: a full
            // rebuild in a session that expected warm is never silent).
            eprintln!(
                "fszero: index manifest present but AST rows missing — store was wiped or a persist tore; running one full cold rebuild ({} files)",
                current.len()
            );
            incremental = false;
        }
        self.index.ast_generation = generation;
        let ast_generation = generation as i64;

        let mut dirty: HashSet<&str> = HashSet::new();
        let mut removed: Vec<&str> = Vec::new();
        if incremental {
            let current_keys: HashSet<&str> =
                current.iter().map(|(_, key, _)| key.as_str()).collect();
            for (_, key, sig) in &current {
                if prev_sigs.get(key) != Some(sig) {
                    dirty.insert(key.as_str());
                }
            }
            removed.extend(
                prev_sigs
                    .keys()
                    .map(String::as_str)
                    .filter(|key| !current_keys.contains(key)),
            );
        }
        let need_db_writes = !incremental || !dirty.is_empty() || !removed.is_empty();
        phases.mark("manifest_diff");

        // One transaction around the WHOLE build: ingest_file writes content
        // refs/certs/facts per walked file, and in autocommit each of those
        // inserts runs the adaptive WAL autocheckpoint — profiled at 83% of a
        // fresh 23k-file index (checkpoint writer + pager read/write storm).
        // Batching turns tens of thousands of autocommit txns into one.
        // Skipped entirely (including the checkpoint pragmas) when nothing
        // changed on disk.
        let began = if need_db_writes {
            self.recovery.begin_batch()
        } else {
            false
        };
        if self.persist_ast_index {
            if !incremental {
                self.recovery.ast.clear_all();
            }
            for key in &removed {
                self.recovery.ast.clear_for_file(key);
                let _ = self.recovery.clear_file_chunks(key);
            }
        }
        // Populate file list and signatures from walk data for ALL files.
        // Clean files on warm start are skipped during ingest (no file read);
        // their structural rows come from the durable AST sidecar.
        // The path cache stays lazy: resolve_existing_path_cached inserts
        // the first time a caller actually needs a path, so the batch loop
        // avoids formatting and cloning 100k+ PathBuf entries up front.
        for (_, file_key, sig) in &current {
            self.index.indexed_file_keys.insert(file_key.clone());
            let mtime = std::time::UNIX_EPOCH
                + std::time::Duration::from_nanos(sig.0.min(u64::MAX as u128) as u64);
            self.index.file_sig.insert(file_key.clone(), (mtime, sig.1));
        }
        phases.mark("prepare");
        let mut total_parse = std::time::Duration::ZERO;
        let mut total_merge = std::time::Duration::ZERO;
        let mut total_persist = std::time::Duration::ZERO;
        let mut ingested_count = 0usize;

        let mut pending_file_calls: Vec<PendingFileCalls> = Vec::new();

        for chunk in current.chunks(INGEST_BATCH_FILES) {
            let parse_start = std::time::Instant::now();
            let ingest_results = parallel_ingest_files(chunk, &dirty, incremental);
            total_parse += parse_start.elapsed();
            ingested_count += ingest_results.len();

            let merge_start = std::time::Instant::now();
            for (chunk_index, extracted) in &ingest_results {
                let file_key = &chunk[*chunk_index].1;
                if let Some(extracted) = extracted {
                    self.index.symbols.extend(
                        extracted
                            .fns
                            .iter()
                            .map(|f| (f.name.clone(), file_key.clone())),
                    );
                }
            }
            total_merge += merge_start.elapsed();

            let persist_start = std::time::Instant::now();
            if self.persist_ast_index {
                for (chunk_index, extracted) in ingest_results {
                    let (_, file_key, _) = &chunk[chunk_index];
                    let Some(extracted) = extracted else {
                        continue;
                    };
                    if !is_structural_file_key(file_key) {
                        continue;
                    }
                    if incremental {
                        if !dirty.contains(file_key.as_str()) {
                            continue;
                        }
                        self.recovery.ast.clear_for_file(file_key);
                    }
                    self.insert_fn_and_import_nodes(
                        file_key,
                        &extracted.fns,
                        &extracted.imports,
                        ast_generation,
                    );
                    if !extracted.calls.is_empty() {
                        pending_file_calls.push(PendingFileCalls {
                            file_key: file_key.clone(),
                            calls: extracted.calls,
                        });
                    }
                }
            }
            total_persist += persist_start.elapsed();
        }

        // On incremental warm start, merge persisted clean symbols with the
        // newly parsed dirty symbols so the in-memory index is complete.
        if incremental {
            let symbol_merge_start = std::time::Instant::now();
            let mut persisted = self.recovery.ast.query_all_symbols(ast_generation);
            persisted.retain(|(_, file_key)| !dirty.contains(file_key.as_str()));
            self.index.symbols.extend(persisted);
            total_merge += symbol_merge_start.elapsed();
        }

        phases.accumulate("parallel_ingest", total_parse);
        phases.accumulate("merge", total_merge);

        // Optional local semantic tier (fszero-9wn): memoize frankensearch
        // hash embeddings per chunk digest in the shared CAS. Feature-off
        // builds compile this out -- cold-index path is unchanged.
        #[cfg(feature = "fszero-semantic-local")]
        {
            let semantic_start = std::time::Instant::now();
            let mut semantic_files: Vec<(std::path::PathBuf, String)> = Vec::new();
            for (path, file_key, _) in &current {
                if !incremental || dirty.contains(file_key.as_str()) {
                    semantic_files.push((path.clone(), file_key.clone()));
                }
            }

            let n = crate::semantic_local::ingest_semantic_chunks(
                &mut self.recovery,
                &root,
                &semantic_files,
            );
            phases.accumulate("semantic_local", semantic_start.elapsed());
            let _ = n;
        }

        // Filter pending calls through the complete set of known fn/method names
        // and insert the surviving edges. Count this work in ast_persist.
        let filter_start = std::time::Instant::now();
        if self.persist_ast_index {
            let symbols = mem::take(&mut self.index.symbols);
            let known: HashSet<&str> = symbols.iter().map(|(name, _)| name.as_str()).collect();
            for pfc in &pending_file_calls {
                insert_known_call_edges(
                    &self.recovery.ast,
                    &pfc.file_key,
                    &pfc.calls,
                    &known,
                    ast_generation,
                );
            }
            self.index.symbols = symbols;
        }
        total_persist += filter_start.elapsed();

        // Manifest write is part of the AST persistence phase.
        let manifest_start = std::time::Instant::now();
        if self.persist_ast_index && need_db_writes {
            let sigs: Vec<(String, FileSig)> = current
                .iter()
                .map(|(_, key, sig)| (key.clone(), *sig))
                .collect();
            self.recovery.put_key(
                INDEX_MANIFEST_KEY,
                render_index_manifest(generation, &expected_producer, &sigs).as_bytes(),
            );
        }
        if self.persist_ast_index {
            // Reuse the PRE-build fingerprint (cheap_fingerprint) rather than
            // re-walking the root here. Storing the pre-build value means any
            // file added/changed DURING this build produces a mismatch on the
            // next open, forcing a rebuild that picks it up — the post-build
            // value could mask exactly those mid-build changes and serve a
            // stale index via warm-skip. Saves a second full root scan + SHA.
            if let Some(fingerprint) = cheap_fingerprint.as_ref() {
                self.recovery
                    .put_key(INDEX_ROOT_FINGERPRINT_KEY, fingerprint.as_bytes());
            }
        }

        total_persist += manifest_start.elapsed();

        phases.accumulate("ast_persist", total_persist);

        if need_db_writes {
            self.recovery.end_batch(began);
        }
        phases.mark("txn_commit");

        phases.mark("searcher_create");
        phases.finish(serde_json::json!({
            "files_walked": current.len(), "dirty": dirty.len(),
            "removed": removed.len(), "ingested": ingested_count,
            "incremental": incremental, "producer_matched": producer_matched,
            "truncated": walk_truncated,
        }));
        self.last_index_build = IndexBuildReport {
            mode: if incremental { "incremental" } else { "cold" },
            incremental,
            producer_matched,
            dirty: dirty.len(),
            removed: removed.len(),
            ingested: ingested_count,
            files_walked: current.len(),
            truncated: walk_truncated,
        };
        if self.persist_ast_index {
            let dirty_keys = git_worktree_delta(&root).unwrap_or_default();
            self.recovery
                .put_key(INDEX_DIRTY_KEYS_KEY, &render_dirty_keys(&dirty_keys));
        }
        self.index_initialized = true;
        self.watch_reconcile.clear_pre_index_deferred();
        Ok(())
    }

    pub fn ingest_file(&mut self, root: &Path, entry: &Path) -> Option<IndexedFile> {
        self.ingest_file_with(root, entry, None, true, false)
    }

    /// `persist_blob=false` skips the content-ref SHA256 + store write for
    /// files the incremental build proved unchanged; the read cache
    /// repopulates lazily on first read.
    fn ingest_file_with(
        &mut self,
        root: &Path,
        entry: &Path,
        file_key: Option<String>,
        persist_blob: bool,
        skip_parse: bool,
    ) -> Option<IndexedFile> {
        let Ok(meta) = fs::metadata(entry) else {
            return None;
        };
        let metadata_before = meta.modified().ok().map(|modified| (meta.len(), modified));
        // Metadata already in hand; refuse still gates the content open.
        crate::path::refuse_non_regular_file(entry).ok()?;
        let Ok(bytes) = fs::read(entry) else {
            return None;
        };
        if looks_binary_bytes(&bytes) {
            return None;
        }
        // Single from_utf8: validate + convert in one pass instead of two.
        let Ok(txt) = String::from_utf8(bytes.clone()) else {
            return None;
        };
        let file_key = file_key.unwrap_or_else(|| relative_file_key(root, entry));
        self.index.indexed_file_keys.insert(file_key.clone());
        // Incremental upsert only (fszero-up8): same bytes already loaded for
        // extract — never a bulk rebuild of the whole corpus.
        if crate::ast_sgrep::literal_prefilter_from_env()
            == crate::ast_sgrep::LiteralPrefilter::BigramMemmem
        {
            self.lazy_bigrams
                .upsert_file(&file_key, txt.as_bytes(), &meta);
        }
        if let Ok(mtime) = meta.modified() {
            self.index
                .file_sig
                .insert(file_key.clone(), (mtime, meta.len()));
        }
        if persist_blob {
            let content_ref: std::sync::Arc<str> =
                std::sync::Arc::from(self.recovery.put_content_ref(&bytes));
            let metadata_after = fs::metadata(entry).ok().and_then(|after| {
                after
                    .modified()
                    .ok()
                    .map(|modified| (after.len(), modified))
            });
            if let (Some(before), Some((_, mtime))) = (metadata_before, metadata_after)
                && Some(before) == metadata_after
            {
                self.caches.content.insert(
                    entry.to_path_buf(),
                    ReadCacheEntry {
                        bytes: std::sync::Arc::new(bytes),
                        mtime,
                        content_ref,
                    },
                );
            }
        }
        // Match resolve_existing_path_cached: rooted sessions key by relative arg only.
        self.caches.paths.insert(
            file_key.to_string(),
            std::sync::Arc::new(entry.to_path_buf()),
        );
        // Skip tree-sitter parse on warm start for unchanged files: the AST
        // is already persisted. Symbols load from DB in build_index after the
        // file loop. Saves ~80% of warm-start CPU (profiled).
        let extracted = if skip_parse || !is_structural_source_file(entry) {
            super::adapter::ExtractedFile {
                fns: Vec::new(),
                imports: Vec::new(),
                calls: Vec::new(),
            }
        } else {
            extract_structural(entry, &txt)
        };
        let fns = extracted.fns;
        if !skip_parse {
            self.index
                .symbols
                .extend(fns.iter().map(|f| (f.name.clone(), file_key.clone())));
        }
        Some(IndexedFile {
            file_key,
            fns,
            imports: extracted.imports,
            calls: extracted.calls,
        })
    }

    fn insert_fn_and_import_nodes(
        &mut self,
        file_key: &str,
        fns: &[IndexedFn],
        imports: &[IndexedImport],
        ast_generation: i64,
    ) {
        // No ast_edges rows: SEQ edges (i -> i+1) were written for every fn
        // and never queried anywhere — pure btree-insert cost in the hottest
        // cold-build loop (profiled: ast_persist 65% of a 10k cold index).
        for def in fns {
            self.recovery.ast.insert_symbol_node(
                file_key,
                def.span_start as i64,
                def.span_end as i64,
                &def.name,
                def.kind.as_db_kind(),
                ast_generation,
            );
        }

        for import in imports {
            self.recovery.ast.insert_import_node(
                file_key,
                import.span_start as i64,
                import.span_end as i64,
                &import.name,
                ast_generation,
            );
        }
    }

    /// Desired persisted span rows for one file after a fresh extraction.
    fn desired_ast_spans(
        fns: &[IndexedFn],
        imports: &[IndexedImport],
    ) -> Vec<super::super::ast_store::AstSpanKey> {
        let mut desired = Vec::with_capacity(fns.len() + imports.len());
        for def in fns {
            desired.push(super::super::ast_store::AstSpanKey {
                kind: def.kind.as_db_kind().to_string(),
                symbol: def.name.clone(),
                span_start: def.span_start as i64,
                span_end: def.span_end as i64,
            });
        }
        for import in imports {
            desired.push(super::super::ast_store::AstSpanKey {
                kind: "import".to_string(),
                symbol: import.name.clone(),
                span_start: import.span_start as i64,
                span_end: import.span_end as i64,
            });
        }
        desired
    }

    pub fn reindex_path(&mut self, path: &Path) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let file_key = relative_file_key(&root, path);
        // Insert at the CURRENT generation: bumping here made every other
        // file's persisted rows invisible to `version = ?` structural queries
        // after a single-file reindex.
        let ast_generation = self.index.ast_generation as i64;
        self.index.symbols.retain(|(_, file)| file != &file_key);
        self.index.indexed_file_keys.remove(&file_key);
        self.lazy_bigrams.remove(&file_key);
        self.clear_query_caches();
        if !is_supported_source_file(path) {
            // File gone or unsupported: drop every persisted AST row for it.
            if self.persist_ast_index {
                self.recovery.ast.clear_for_file(&file_key);
            }
            return;
        }
        let Some(file) = self.ingest_file(&root, path) else {
            if self.persist_ast_index {
                self.recovery.ast.clear_for_file(&file_key);
            }
            return;
        };
        if self.persist_ast_index && is_structural_source_file(path) {
            let began = self.recovery.begin_batch();
            let symbols = mem::take(&mut self.index.symbols);
            let known: HashSet<&str> = symbols.iter().map(|(name, _)| name.as_str()).collect();
            // fszero-4y2.1: AST-diff upsert — rewrite only changed/added/
            // removed symbol+import rows; unchanged spans stay put.
            let desired = Self::desired_ast_spans(&file.fns, &file.imports);
            let _stats =
                self.recovery
                    .ast
                    .upsert_spans_diff(&file.file_key, &desired, ast_generation);
            // Call edges still file-scoped replace (cheaper than span-keyed
            // edge identity for now; bead scope is symbol/import rows).
            self.recovery.ast.clear_call_edges_for_file(&file.file_key);
            insert_known_call_edges(
                &self.recovery.ast,
                &file.file_key,
                &file.calls,
                &known,
                ast_generation,
            );
            self.index.symbols = symbols;
            self.recovery.end_batch(began);
        } else if self.persist_ast_index {
            self.recovery.ast.clear_for_file(&file_key);
        }
    }

    /// Re-index files whose on-disk mtime/size diverges from the in-memory index.
    ///
    /// One-shot processes have no watcher. A full-tree `stat` is correct but
    /// not instant at 100k files. When git is usable, only restat:
    /// * paths `git status` reports (unstaged, staged, untracked)
    /// * paths that were dirty vs HEAD when we last indexed (so a
    ///   revert-to-HEAD after we indexed the dirty bytes is still seen)
    ///
    /// In-place edits do not change the parent directory mtime, so a
    /// directory-mtime walk is not a substitute. `FSZERO_INDEX_REFRESH=full`
    /// forces the old all-files stat pass.
    pub fn refresh_stale_index_files(&mut self) {
        if self.watch_active() && self.watch_index_trusted() {
            self.last_index_refresh = IndexRefreshReport {
                mode: "watch_skip",
                ..IndexRefreshReport::default()
            };
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };

        let (mode, keys): (&'static str, Vec<String>) = if force_full_index_refresh() {
            (
                "full_stat",
                self.index.indexed_file_keys.iter().cloned().collect(),
            )
        } else if let Some(git_paths) = git_worktree_delta(&root) {
            let mut candidates = git_paths;
            if self.persist_ast_index {
                candidates.extend(load_dirty_keys(self.recovery.expand(INDEX_DIRTY_KEYS_KEY)));
            }
            ("git_candidates", candidates.into_iter().collect())
        } else {
            (
                "full_stat",
                self.index.indexed_file_keys.iter().cloned().collect(),
            )
        };

        let mut stated = 0usize;
        let mut reindexed = 0usize;
        for file_key in &keys {
            let path = root.join(file_key);
            let stale = match fs::metadata(&path) {
                Ok(meta) => {
                    stated += 1;
                    let mtime = meta.modified().ok();
                    let len = meta.len();
                    self.index
                        .file_sig
                        .get(file_key)
                        .is_none_or(|(old_mtime, old_len)| {
                            mtime != Some(*old_mtime) || len != *old_len
                        })
                }
                Err(_) => {
                    stated += 1;
                    true
                }
            };
            if stale {
                self.reindex_path(&path);
                reindexed += 1;
            }
        }

        if mode == "git_candidates" && self.persist_ast_index {
            let dirty_keys = git_worktree_delta(&root).unwrap_or_default();
            self.recovery
                .put_key(INDEX_DIRTY_KEYS_KEY, &render_dirty_keys(&dirty_keys));
        }

        self.last_index_refresh = IndexRefreshReport {
            mode,
            candidates: keys.len(),
            stated,
            reindexed,
        };

        // Known-key metadata refresh can clear truncated-rescan untrust
        // (deleted files reindexed away). Overflow still needs a complete
        // rescan for brand-new files never in the index (fszero-w2g.3/.47).
        if self.watch_active() {
            self.watch_reconcile.untrusted_removals = false;
        }
    }

    pub fn store_edit_cert(
        &mut self,
        path: &Path,
        pre_ref: &str,
        post_ref: &str,
        old: &str,
        new: &str,
    ) -> String {
        self.store_edit_cert_with_metadata(path, pre_ref, post_ref, old, new, 0, -1, "")
    }

    pub fn store_edit_cert_with_metadata(
        &mut self,
        path: &Path,
        pre_ref: &str,
        post_ref: &str,
        old: &str,
        new: &str,
        pre_mtime_ns: i64,
        pre_mode: i64,
        pre_xattrs: &str,
    ) -> String {
        let cert = format!(
            "path={}\npre={}\npost={}\npre_mtime_ns={}\npre_mode={}\npre_xattrs={}\nold_len={}\nnew_len={}\nversion={}\nast_generation={}\n",
            path.display(),
            pre_ref,
            post_ref,
            pre_mtime_ns,
            pre_mode,
            pre_xattrs,
            old.len(),
            new.len(),
            self.version,
            self.index.ast_generation,
        );
        let cert_ref = self
            .recovery
            .put_named_payload("last_cert", cert.as_bytes());
        self.recovery.put_fact(
            pre_ref,
            "edited_to",
            post_ref,
            &cert_ref,
            self.version,
            "fszero",
        );
        cert_ref
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn ingest_one_file_skips_fifo_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hang.rs");
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .expect("spawn mkfifo");
        assert!(status.success(), "mkfifo failed: {status}");

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(ingest_one_file(&path, true));
        });
        let result = rx
            .recv_timeout(Duration::from_millis(1500))
            .expect("ingest_one_file hung on FIFO instead of skipping");
        assert!(
            result.is_none(),
            "indexer must skip FIFO ingest rather than hang"
        );
    }
}
