//! Disk-backed, per-scope session seen-set.

use crate::session::{ServeKey, ServedRecord, SessionMemory, SessionRollup};
use fs4::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokenzero_core::sha256_hex;

pub const SESSION_SCOPE_ENV: &str = "TOKENZERO_SESSION_SCOPE";
const REF_INDEX_PATH_ENV: &str = "TOKENZERO_REF_INDEX_PATH";
pub const MAX_SESSION_MEMORY_RECORDS: usize = 512;
const LOCK_RETRIES: usize = 240;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(25);
const STATE_VERSION: u32 = 2;
/// Compact eagerly — large journals turn every resume into a CPU storm.
const JOURNAL_COMPACT_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone)]
pub struct SessionPersistence {
    path: PathBuf,
    cache_path: PathBuf,
    scope_id: String,
    last_persisted: Arc<Mutex<Option<String>>>,
}

impl SessionPersistence {
    pub(crate) fn for_cache(cache_path: &Path, session_dedup: bool) -> Option<Self> {
        session_dedup.then(|| Self {
            path: session_memory_path(cache_path),
            cache_path: cache_path.to_path_buf(),
            scope_id: session_scope_id(cache_path),
            last_persisted: Arc::new(Mutex::new(None)),
        })
    }

    pub(crate) fn load_into(&self, memory: &mut SessionMemory) {
        let Ok(_lock) = SessionPersistLock::acquire(session_lock_path(&self.path)) else {
            return;
        };
        let Some(state) = load_state(&self.path) else {
            return;
        };
        let Some(scope) = state.scopes.get(&self.scope_id) else {
            return;
        };
        // v1 has no watermark, so its first resumed turn must serve full.
        //
        // H3: Prefer O(1) on-disk sidecar/CAS proofs before constructing
        // RecoveryStore::new (full snapshot+journal parse, ~20+ ms on S4_whole).
        // When every residual blob proves on disk, skip the store entirely.
        // When any ref is unproven, fall back once to RecoveryStore::has_ref_local
        // so inline-only / non-sidecar blobs restore exactly (isomorphism).
        // Full has_ref is never used here (ref-index walks reload journals).
        let records = if state.version >= STATE_VERSION {
            let mut out = HashMap::new();
            let mut ref_available: HashMap<&str, bool> = HashMap::new();
            let mut unresolved: Vec<&PersistedRecordEntry> = Vec::new();
            for (idx, entry) in scope.records.iter().enumerate() {
                if crate::wall::check_active_wall_deadline_every(
                    idx,
                    crate::wall::WALL_CHECK_EVERY_N,
                )
                .is_some()
                {
                    // Abort mid-load rather than burning past the host wall.
                    return;
                }
                let blob = entry.record.blob_ref.as_str();
                let available = *ref_available.entry(blob).or_insert_with(|| {
                    tokenzero_recovery::blob_ref_proven_on_disk(&self.cache_path, blob)
                });
                if available {
                    out.insert(entry.key.clone(), entry.record.clone());
                } else {
                    unresolved.push(entry);
                }
            }
            if !unresolved.is_empty() {
                let store = tokenzero_recovery::RecoveryStore::new(Some(self.cache_path.clone()));
                for entry in unresolved {
                    let blob = entry.record.blob_ref.as_str();
                    let available = match ref_available.get(blob).copied() {
                        Some(true) => true,
                        Some(false) | None => {
                            let ok = store.has_ref_local(blob);
                            ref_available.insert(blob, ok);
                            ok
                        }
                    };
                    if available {
                        out.insert(entry.key.clone(), entry.record.clone());
                    }
                }
            }
            out
        } else {
            HashMap::new()
        };
        memory.restore_from_persist(records, scope.rollup.clone(), scope.session_hwm);
    }

    /// Persist the snapshot. Fail-closed: dropping `persist_inner` would leave
    /// in-memory session memory claiming a durable seen-set that resume cannot
    /// restore.
    pub(crate) fn persist(&self, snapshot: &SessionPersistSnapshot) -> std::io::Result<bool> {
        crate::perf_profile::_profile_session_persist(|| self.persist_inner(snapshot))
    }

    fn persist_inner(&self, snapshot: &SessionPersistSnapshot) -> std::io::Result<bool> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        ensure_private_dir(parent)?;
        let _lock = SessionPersistLock::acquire(session_lock_path(&self.path))?;
        let delta = PersistedDelta {
            version: STATE_VERSION,
            scope_id: self.scope_id.clone(),
            records: snapshot.changed.clone(),
            rollup: snapshot.rollup.clone(),
            session_hwm: snapshot.session_hwm,
        };
        let delta_body = serde_json::to_string(&delta)?;
        let last = self
            .last_persisted
            .lock()
            .ok()
            .and_then(|guard| guard.clone());
        // SAFETY: `last_persisted` is a skip-cache, not the persist gate
        // (`SessionPersistLock` flock). Copy-out so the in-process mutex is
        // not live across `last_complete_journal_line` journal I/O.
        // The flock is held across snapshot, journal compare, write, and
        // skip-cache update, so concurrent persist cannot interleave. A stale
        // skip-cache (None) only causes an extra identical-line compare/write,
        // never a skipped newer delta.
        if last.as_deref() == Some(delta_body.as_str())
            && last_complete_journal_line(&self.path).as_deref() == Some(delta_body.as_str())
        {
            return Ok(false);
        }

        let base = fs::read_to_string(&self.path)
            .ok()
            .and_then(|body| serde_json::from_str::<SessionMemoryState>(&body).ok());
        if base
            .as_ref()
            .is_none_or(|state| state.version < STATE_VERSION)
        {
            let mut state = base.unwrap_or_default();
            let mut records = snapshot.all_records.clone();
            normalize_records(&mut records);
            state.version = STATE_VERSION;
            state.scopes.insert(
                self.scope_id.clone(),
                PersistedScope {
                    records,
                    rollup: snapshot.rollup.clone(),
                    session_hwm: snapshot.session_hwm,
                },
            );
            let body = serde_json::to_string_pretty(&state)?;
            atomic_write_json(&self.path, &body)?;
            remove_journal(&self.path)?;
            append_json_line(&session_journal_path(&self.path), &delta_body)?;
        } else {
            append_json_line(&session_journal_path(&self.path), &delta_body)?;
            compact_if_needed(&self.path, JOURNAL_COMPACT_BYTES)?;
        }
        if let Ok(mut last) = self.last_persisted.lock() {
            *last = Some(delta_body);
        }
        Ok(true)
    }
}

pub fn session_memory_path(cache_path: &Path) -> PathBuf {
    user_memory_root(cache_path).join("session-memory.json")
}

fn user_memory_root(cache_path: &Path) -> PathBuf {
    if let Some(path) = SESSION_ROOT_TEST_OVERRIDE.with(|slot| slot.borrow().clone()) {
        return path;
    }
    std::env::var_os(REF_INDEX_PATH_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            cache_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf()
        })
}

pub fn session_scope_id(cache_path: &Path) -> String {
    std::env::var(SESSION_SCOPE_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            let digest = sha256_hex(&cache_path.to_string_lossy());
            format!("workspace:{}", &digest[..16])
        })
}

thread_local! {
    static SESSION_ROOT_TEST_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Test harness helper: pin session-memory root while `f` runs.
/// Public so integration tests in tokenzero-mcp can exercise the same override.
pub fn with_session_root<R>(root: &Path, f: impl FnOnce() -> R) -> R {
    SESSION_ROOT_TEST_OVERRIDE.with(|slot| {
        let previous = slot.replace(Some(root.to_path_buf()));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        slot.replace(previous);
        match result {
            Ok(value) => value,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    })
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SessionMemoryState {
    version: u32,
    #[serde(default)]
    scopes: BTreeMap<String, PersistedScope>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct PersistedScope {
    #[serde(default)]
    records: Vec<PersistedRecordEntry>,
    #[serde(default)]
    rollup: SessionRollup,
    #[serde(default)]
    session_hwm: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedRecordEntry {
    key: ServeKey,
    record: ServedRecord,
    #[serde(default)]
    seq: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedDelta {
    version: u32,
    scope_id: String,
    #[serde(default)]
    records: Vec<PersistedRecordEntry>,
    rollup: SessionRollup,
    session_hwm: u64,
}

/// Changed records (journal) plus full live records (rewrite) captured
/// under the session mutex. Persist merges under the flock only.
pub(crate) struct SessionPersistSnapshot {
    changed: Vec<PersistedRecordEntry>,
    all_records: Vec<PersistedRecordEntry>,
    rollup: SessionRollup,
    session_hwm: u64,
}

impl SessionPersistSnapshot {
    pub(crate) fn from_memory(memory: &SessionMemory, changed_keys: &[ServeKey]) -> Self {
        let live = memory.records_snapshot();
        let changed = changed_keys
            .iter()
            .filter_map(|key| {
                live.get(key).map(|record| PersistedRecordEntry {
                    key: key.clone(),
                    record: record.clone(),
                    seq: 0,
                })
            })
            .collect();
        let all_records = live
            .iter()
            .map(|(key, record)| PersistedRecordEntry {
                key: key.clone(),
                record: record.clone(),
                seq: 0,
            })
            .collect();
        Self {
            changed,
            all_records,
            rollup: memory.persisted_rollup(),
            session_hwm: memory.session_hwm(),
        }
    }
}

fn load_state(path: &Path) -> Option<SessionMemoryState> {
    let mut state =
        serde_json::from_str::<SessionMemoryState>(&fs::read_to_string(path).ok()?).ok()?;
    if state.version < STATE_VERSION {
        return Some(state);
    }
    let journal = fs::read_to_string(session_journal_path(path)).unwrap_or_default();
    let complete = journal.rfind('\n').map_or("", |end| &journal[..=end]);
    for line in complete.lines().filter(|line| !line.trim().is_empty()) {
        let delta: PersistedDelta = serde_json::from_str(line).ok()?;
        if delta.version != STATE_VERSION {
            return None;
        }
        apply_delta(&mut state, delta);
    }
    for scope in state.scopes.values_mut() {
        normalize_records(&mut scope.records);
    }
    Some(state)
}

fn apply_delta(state: &mut SessionMemoryState, delta: PersistedDelta) {
    let scope = state.scopes.entry(delta.scope_id).or_default();
    let mut records: HashMap<_, _> = scope
        .records
        .drain(..)
        .map(|entry| (entry.key.clone(), entry))
        .collect();
    records.extend(
        delta
            .records
            .into_iter()
            .map(|entry| (entry.key.clone(), entry)),
    );
    scope.records = records.into_values().collect();
    scope.rollup = delta.rollup;
    scope.session_hwm = scope.session_hwm.max(delta.session_hwm);
    state.version = STATE_VERSION;
}

fn normalize_records(records: &mut Vec<PersistedRecordEntry>) {
    sort_records(records);
    if records.len() > MAX_SESSION_MEMORY_RECORDS {
        records.drain(..records.len() - MAX_SESSION_MEMORY_RECORDS);
    }
    for (idx, entry) in records.iter_mut().enumerate() {
        entry.seq = idx as u64 + 1;
    }
}

fn sort_records(records: &mut [PersistedRecordEntry]) {
    records.sort_by_cached_key(|entry| serde_json::to_string(&entry.key).unwrap_or_default());
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(|| "session-memory.json".into(), |name| name.to_os_string());
    name.push(suffix);
    path.with_file_name(name)
}

fn session_journal_path(path: &Path) -> PathBuf {
    sidecar_path(path, ".journal")
}

fn last_complete_journal_line(path: &Path) -> Option<String> {
    let body = fs::read_to_string(session_journal_path(path)).ok()?;
    body.strip_suffix('\n')?
        .lines()
        .next_back()
        .map(str::to_owned)
}

fn append_json_line(path: &Path, body: &str) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.read(true).append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    let len = file.metadata()?.len();
    if len > 0 {
        file.seek(SeekFrom::End(-1))?;
        let mut tail = [0];
        file.read_exact(&mut tail)?;
        if tail[0] != b'\n' {
            let bytes = fs::read(path)?;
            file.set_len(
                bytes
                    .iter()
                    .rposition(|byte| *byte == b'\n')
                    .map_or(0, |idx| idx + 1) as u64,
            )?;
        }
    }
    file.seek(SeekFrom::End(0))?;
    // tokenzero-nxyd: one write syscall per journal append (body + newline together).
    let mut line = Vec::with_capacity(body.len() + 1);
    line.extend_from_slice(body.as_bytes());
    line.push(b'\n');
    file.write_all(&line)?;
    file.flush()
}

fn compact_if_needed(path: &Path, max_bytes: u64) -> std::io::Result<()> {
    let journal = session_journal_path(path);
    if fs::metadata(&journal).is_ok_and(|metadata| metadata.len() > max_bytes) {
        compact_journal(path)?;
    }
    Ok(())
}

fn compact_journal(path: &Path) -> std::io::Result<()> {
    let state = load_state(path).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid session persistence journal",
        )
    })?;
    let body = serde_json::to_string_pretty(&state)?;
    atomic_write_json(path, &body)?;
    remove_journal(path)
}

fn remove_journal(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(session_journal_path(path)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn ensure_private_dir(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(path)?.permissions().mode() & 0o7777 != 0o700 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

fn atomic_write_json(path: &Path, body: &str) -> std::io::Result<()> {
    zero_store::atomic_write_file(path, body.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn session_lock_path(path: &Path) -> PathBuf {
    sidecar_path(path, ".lock")
}

struct SessionPersistLock(fs::File);

impl SessionPersistLock {
    fn acquire(path: PathBuf) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        for attempt in 0..LOCK_RETRIES {
            match FileExt::try_lock(&file) {
                Ok(()) => {
                    // tokenzero-nxyd: the flock alone carries the lock; the pid
                    // breadcrumb was never read and cost one write() per acquire.
                    return Ok(Self(file));
                }
                Err(_) if attempt + 1 < LOCK_RETRIES => std::thread::sleep(LOCK_RETRY_DELAY),
                Err(err) => return Err(err.into()),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            format!("could not acquire session persist lock: {}", path.display()),
        ))
    }
}

impl Drop for SessionPersistLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

#[cfg(test)]
mod persist_fail_closed_tests {
    use super::*;
    use crate::session::{ServeKey, ServedRecord, SessionMemory};
    use std::time::SystemTime;

    fn snapshot_with(label: &str) -> SessionPersistSnapshot {
        let mut memory = SessionMemory::default();
        memory.record(
            ServeKey::File {
                path: PathBuf::from(label),
                start: None,
                end: None,
            },
            ServedRecord {
                content_sha256: format!("{label}-hash"),
                blob_ref: format!("tz://{label}"),
                file_ref: format!("file://{label}"),
                raw_tokens: 1,
                line_count: 1,
                byte_len: label.len(),
                served_at: SystemTime::UNIX_EPOCH,
                serve_count: 1,
            },
        );
        SessionPersistSnapshot::from_memory(&memory, &[])
    }

    #[test]
    fn persist_returns_err_when_root_is_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("blocker");
        fs::write(&blocker, b"not-a-dir").unwrap();
        let cache = dir.path().join("cache.json");
        fs::write(&cache, b"{}").unwrap();
        let err = with_session_root(&blocker, || {
            let persist = SessionPersistence::for_cache(&cache, true).expect("persist");
            persist.persist(&snapshot_with("blocked"))
        })
        .expect_err("file-as-root must fail closed, not drop persist_inner");
        assert_ne!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn persist_soak_lite_repeated_deltas_land_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("session-root");
        fs::create_dir_all(&root).unwrap();
        let cache = dir.path().join("cache.json");
        fs::write(&cache, b"{}").unwrap();
        with_session_root(&root, || {
            let persist = SessionPersistence::for_cache(&cache, true).expect("persist");
            for i in 0..32 {
                persist
                    .persist(&snapshot_with(&format!("k{i}")))
                    .expect("persist cycle");
            }
        });
        let memory_path = with_session_root(&root, || session_memory_path(&cache));
        assert!(
            memory_path.is_file(),
            "soak-lite must write session-memory.json at {}",
            memory_path.display()
        );
        let journal = session_journal_path(&memory_path);
        assert!(
            journal.is_file(),
            "soak-lite must append a journal at {}",
            journal.display()
        );
        let body = fs::read_to_string(&journal).unwrap();
        assert!(
            body.lines().filter(|line| !line.trim().is_empty()).count() >= 1,
            "journal should contain at least one complete delta: {body}"
        );
    }
}
