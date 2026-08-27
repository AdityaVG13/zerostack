//! P2.2 walking skeleton: opt-in warm daemon stem (unix socket, zero busy-wait idle).
//!
//! Full stem/muscle/hibernation ships incrementally; this module exposes a
//! resident stem that keeps a mmap'd [`Snapshot`] open and serves warm queries.

#[cfg(unix)]
use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::io::{BufRead, BufWriter};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
#[cfg(all(unix, test))]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(unix)]
use std::sync::{
    Arc,
    mpsc::{self, Receiver, RecvTimeoutError},
};
use std::thread;
use std::time::{Duration, Instant};
#[cfg(unix)]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use anyhow::Context;
use anyhow::{Result, bail};
use graphzero_types::child_identity;
#[cfg(unix)]
use notify::event::{CreateKind, ModifyKind, RemoveKind};
#[cfg(unix)]
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
#[cfg(unix)]
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

#[cfg(unix)]
use super::query::QueryEngine;
#[cfg(unix)]
use crate::Snapshot;

const DAEMON_DIR: &str = "daemon";
const SOCKET_NAME: &str = "stem.sock";
const STATE_NAME: &str = "state.json";
const PID_NAME: &str = "stem.pid";
const IDENTITY_NAME: &str = child_identity::IDENTITY_FILE_NAME;
const DAEMON_TEMP_ATTEMPTS: u64 = 64;

/// Bounded grace for Linux pidfd escalation of a detached stem (unused on
/// platforms without an exact detached signal handle).
#[cfg(any(target_os = "linux", target_os = "android"))]
const ESCALATION_GRACE: Duration = Duration::from_secs(5);
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const ESCALATION_GRACE: Duration = Duration::from_secs(0);
/// Bounded wait for identity-bound stem exit after an acknowledged Shutdown RPC.
const STEM_EXIT_WAIT: Duration = Duration::from_secs(5);
#[cfg(unix)]
static QUERY_COUNT: AtomicU64 = AtomicU64::new(0);
static INDEX_NOTIFY_COUNT: AtomicU64 = AtomicU64::new(0);
#[cfg(all(unix, test))]
static FAIL_NEXT_WATCH_INDEX: AtomicBool = AtomicBool::new(false);
/// Test-only: artificial hold inside Snap after StemState unlock (graphzero-or15b).
#[cfg(all(unix, test))]
static SNAP_HOLD_MS: AtomicU64 = AtomicU64::new(0);
/// Test-only: concurrent Snap RPCs observed in warm (peak).
#[cfg(all(unix, test))]
static SNAP_IN_FLIGHT: AtomicU64 = AtomicU64::new(0);
#[cfg(all(unix, test))]
static SNAP_IN_FLIGHT_PEAK: AtomicU64 = AtomicU64::new(0);

/// Directory under `.graphzero/` holding socket, pid, and state.
pub fn daemon_dir(store_root: &Path) -> PathBuf {
    store_root.join(DAEMON_DIR)
}

pub fn socket_path(store_root: &Path) -> PathBuf {
    daemon_dir(store_root).join(SOCKET_NAME)
}

pub fn state_path(store_root: &Path) -> PathBuf {
    daemon_dir(store_root).join(STATE_NAME)
}

pub fn pid_path(store_root: &Path) -> PathBuf {
    daemon_dir(store_root).join(PID_NAME)
}

/// Verified identity record written by the stem at spawn (pid + native start
/// identity + owner session + worker generation). Status and teardown bind to
/// this record; no bare-PID signal is ever authorized without it.
pub fn identity_path(store_root: &Path) -> PathBuf {
    daemon_dir(store_root).join(IDENTITY_NAME)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonMode {
    Cold,
    Warm,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StemMetrics {
    pub queries_served: u64,
    pub index_notifications: u64,
    pub snapshot_id: u64,
    pub idle: bool,
    pub events_seen: u64,
    pub files_reindexed: u64,
    pub reconciliations: u64,
    pub last_index_error: Option<String>,
    pub last_update_unix_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub daemon: String,
    pub mode: DaemonMode,
    pub enabled: bool,
    pub socket: Option<String>,
    pub pid: Option<u32>,
    pub stem: Option<StemMetrics>,
    pub note: String,
}

#[cfg(unix)]
struct StemState {
    store_root: PathBuf,
    repo_root: PathBuf,
    /// Arc so Snap can clone under a short `Mutex` critical section and run
    /// `QueryEngine::warm` + `to_json` without holding StemState (graphzero-p12n4).
    snapshot: Arc<Snapshot>,
    events_seen: u64,
    files_reindexed: u64,
    reconciliations: u64,
    last_index_error: Option<String>,
    last_update_unix_ms: Option<u64>,
    /// Set by an authenticated `Shutdown` RPC; the serve loop breaks and the
    /// stem self-cleans its artifacts.
    shutdown_requested: std::sync::atomic::AtomicBool,
    /// Immutable ChildBinding captured at spawn (installed before serving).
    /// Shutdown authorization compares against this, never against mutable
    /// `state.json`, so a state rewrite cannot authorize a wrong-owner/
    /// wrong-generation teardown.
    binding: Option<child_identity::ChildBinding>,
}

/// Test-only hook: record that the index changed (P2.3 `daemon_notifies_on_index_change`).
pub fn notify_index_change(store_root: &Path) -> Result<()> {
    INDEX_NOTIFY_COUNT.fetch_add(1, Ordering::Relaxed);
    #[cfg(unix)]
    if socket_path(store_root).exists() {
        let req = ClientRequest::NotifyIndex {};
        let _ = daemon_client_request(store_root, &req);
    }
    #[cfg(not(unix))]
    let _ = store_root;
    Ok(())
}

pub fn index_notification_count() -> u64 {
    INDEX_NOTIFY_COUNT.load(Ordering::Relaxed)
}

pub fn is_enabled(store_root: &Path) -> bool {
    state_path(store_root)
        .exists()
        .then(|| fs::read_to_string(state_path(store_root)).ok())
        .flatten()
        .and_then(|s| serde_json::from_str::<PersistedState>(&s).ok())
        .is_some_and(|st| st.enabled)
}

fn create_daemon_temp(dir: &Path, file_name: &str) -> Result<(PathBuf, fs::File)> {
    for sequence in 0..DAEMON_TEMP_ATTEMPTS {
        let temp = dir.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            sequence
        ));
        match fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)
        {
            Ok(file) => return Ok((temp, file)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.into()),
        }
    }
    bail!("failed to allocate daemon temp file after {DAEMON_TEMP_ATTEMPTS} attempts")
}

fn atomic_write_daemon_file(store_root: &Path, file_name: &str, bytes: &[u8]) -> Result<()> {
    let dir = daemon_dir(store_root);
    fs::create_dir_all(&dir)?;
    let (temp, mut file) = create_daemon_temp(&dir, file_name)?;
    let destination = dir.join(file_name);

    let result = (|| -> Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        super::replace_file(&temp, &destination)?;
        if let Ok(dir_handle) = fs::File::open(&dir) {
            let _ = dir_handle.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

pub fn write_enabled_state(store_root: &Path, repo_root: &Path, enabled: bool) -> Result<()> {
    // Preserve the bound owner session and worker generation from the previous
    // state so unrelated callers cannot reset the generation counter.
    let previous = read_persisted_state(store_root);
    write_enabled_state_with(
        store_root,
        repo_root,
        enabled,
        &previous.owner_session,
        previous.generation,
    )
}

/// Write enabled state bound to an explicit owner session and worker
/// generation. `enable` uses this to persist the next generation exactly once;
/// the stem child then binds that same generation without incrementing again.
pub fn write_enabled_state_with(
    store_root: &Path,
    repo_root: &Path,
    enabled: bool,
    owner_session: &str,
    generation: u64,
) -> Result<()> {
    let state = PersistedState {
        enabled,
        repo_root: repo_root.to_path_buf(),
        owner_session: owner_session.to_string(),
        generation,
    };
    let bytes = serde_json::to_vec(&state)?;
    atomic_write_daemon_file(store_root, STATE_NAME, &bytes)
}

#[derive(Serialize, Deserialize, Default)]
struct PersistedState {
    enabled: bool,
    repo_root: PathBuf,
    /// Owner session bound to the daemon generation; `#[serde(default)]` keeps
    /// old state files (written before this field existed) fully compatible.
    #[serde(default)]
    owner_session: String,
    /// Worker generation; bumped once per `daemon enable` respawn. `0` for
    /// legacy state or direct `daemon run` without an enable write.
    #[serde(default)]
    generation: u64,
}

fn read_persisted_state(store_root: &Path) -> PersistedState {
    fs::read_to_string(state_path(store_root))
        .ok()
        .and_then(|text| serde_json::from_str::<PersistedState>(&text).ok())
        .unwrap_or_default()
}

/// Resolve the owner session the CLI binds a daemon respawn to.
pub fn daemon_owner_session() -> String {
    std::env::var("ZEROSTACK_SESSION_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            child_identity::ProcessIdentity::current()
                .map(|identity| format!("graphzero:daemon:{}", identity.encode()))
                .unwrap_or_else(|_| format!("graphzero:daemon:pid-{}", std::process::id()))
        })
}

/// The worker generation for the next respawn: strictly greater than the
/// currently persisted generation. Called only by `daemon enable` (the parent),
/// never by the stem child, so the counter cannot double-increment.
pub fn next_daemon_generation(store_root: &Path) -> u64 {
    read_persisted_state(store_root)
        .generation
        .saturating_add(1)
}

/// The owner session + worker generation a live stem is expected to be bound
/// to. Single source of truth shared by the stem (capture/bind), the stem-side
/// Shutdown validator, and `disable_daemon` (expected values before signaling).
fn expected_daemon_binding(store_root: &Path) -> (String, u64) {
    let state = read_persisted_state(store_root);
    let owner = if state.owner_session.is_empty() {
        daemon_owner_session()
    } else {
        state.owner_session.clone()
    };
    (owner, state.generation)
}

#[cfg(unix)]
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum ClientRequest {
    Ping,
    Status,
    Snap {
        symbol: String,
        budget: usize,
    },
    NotifyIndex,
    /// Authenticated graceful stop carrying the expected owner session and
    /// worker generation; the stem validates both immediately before
    /// accepting shutdown. Preferred teardown over any signal.
    Shutdown {
        owner_session: String,
        generation: u64,
    },
}

#[cfg(unix)]
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum ServerResponse {
    Pong,
    Status { status: DaemonStatus },
    Snap { capsule_json: String },
    Ok,
    Error { message: String },
}

#[cfg(unix)]
fn init_stem_shared(store_root: &Path, repo_root: &Path) -> Result<Arc<Mutex<StemState>>> {
    let snapshot = Arc::new(Snapshot::open(store_root, Some(repo_root))?);
    Ok(Arc::new(Mutex::new(StemState {
        store_root: store_root.to_path_buf(),
        repo_root: repo_root.to_path_buf(),
        snapshot,
        events_seen: 0,
        files_reindexed: 0,
        reconciliations: 0,
        last_index_error: None,
        last_update_unix_ms: None,
        shutdown_requested: std::sync::atomic::AtomicBool::new(false),
        binding: None,
    })))
}

/// Capture and persist the exact child identity immediately after `spawn`,
/// while the parent still owns the unreaped [`std::process::Child`] handle.
/// The stem later verifies this record rather than replacing it.
#[cfg(unix)]
pub fn capture_spawned_stem_identity(
    store_root: &Path,
    pid: u32,
    owner_session: &str,
    generation: u64,
) -> Result<()> {
    let binding = child_identity::ChildBinding::capture_pid(pid, owner_session, generation)
        .with_context(|| "capture spawned stem identity before accepting work")?;
    atomic_write_daemon_file(store_root, IDENTITY_NAME, binding.encode().as_bytes())?;
    atomic_write_daemon_file(store_root, PID_NAME, pid.to_string().as_bytes())
}

#[cfg(not(unix))]
pub fn capture_spawned_stem_identity(
    _store_root: &Path,
    _pid: u32,
    _owner_session: &str,
    _generation: u64,
) -> Result<()> {
    bail!("verified detached daemon identity is unsupported on this platform")
}

/// Bounded cleanup when spawn succeeded but detached identity registration
/// failed. The owned unreaped Child handle is exact on every supported OS.
pub fn terminate_unregistered_stem(
    child: std::process::Child,
    owner_session: &str,
    generation: u64,
) -> Result<()> {
    let child = child_identity::VerifiedChild::capture(child, owner_session, generation);
    child
        .signal_graceful_for(owner_session, generation, Duration::from_secs(1))
        .map_err(|error| anyhow::anyhow!("terminate unregistered stem: {error}"))?;
    child
        .revoke()
        .map_err(|error| anyhow::anyhow!("revoke unregistered stem: {error}"))
}

/// Verify the parent-captured stem identity, or self-capture only for the
/// explicit foreground/test entry that has no spawning control process.
#[cfg(unix)]
fn register_stem_process(
    store_root: &Path,
    repo_root: &Path,
) -> Result<child_identity::ChildBinding> {
    let pid = std::process::id();
    let (owner_session, generation) = expected_daemon_binding(store_root);
    let binding = if let Some(binding) = read_daemon_binding(store_root) {
        binding
            .verify_owner(&owner_session, generation)
            .map_err(|error| anyhow::anyhow!("spawned stem binding mismatch: {error}"))?;
        if binding.pid != pid || !binding.is_live() {
            bail!("spawned stem identity does not name the foreground stem process");
        }
        binding
    } else {
        // Direct `daemon run` and same-process tests have no parent capture.
        let binding = child_identity::ChildBinding::capture_pid(pid, &owner_session, generation)
            .with_context(|| "capture foreground stem identity before accepting work")?;
        atomic_write_daemon_file(store_root, IDENTITY_NAME, binding.encode().as_bytes())?;
        atomic_write_daemon_file(store_root, PID_NAME, pid.to_string().as_bytes())?;
        binding
    };
    write_enabled_state_with(store_root, repo_root, true, &owner_session, generation)?;
    Ok(binding)
}

#[cfg(unix)]
fn log_daemon_client_error(err: &anyhow::Error) {
    eprintln!(
        "{{\"daemon_error\":\"{}\"}}",
        err.to_string().replace('"', "'")
    );
}

#[cfg(unix)]
const WATCH_DEBOUNCE: Duration = Duration::from_millis(25);

#[cfg(unix)]
fn prepare_watcher(
    repo_root: &Path,
) -> Result<(RecommendedWatcher, Receiver<notify::Result<Event>>)> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = tx.send(event);
    })
    .context("create recursive repository watcher")?;
    watcher
        .watch(repo_root, RecursiveMode::Recursive)
        .context("watch repository recursively")?;
    Ok((watcher, rx))
}

#[cfg(unix)]
fn ignored_watch_path(repo_root: &Path, store_root: &Path, path: &Path) -> bool {
    if path.starts_with(store_root) {
        return true;
    }
    let relative = path.strip_prefix(repo_root).unwrap_or(path);
    relative.components().any(|component| {
        let name = component.as_os_str();
        name == ".git" || name == "target"
    })
}

/// What a single watcher event asks the stem to do.
#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WatchEventOutcome {
    /// No repository path survived filtering; the stem has no work to do.
    Ignored,
    /// At least one path was queued for incremental reindex.
    Incremental,
    /// The event cannot be resolved incrementally; a full reconcile is needed.
    Reconcile,
}

#[cfg(unix)]
fn collect_event_paths(
    event: Event,
    repo_root: &Path,
    store_root: &Path,
    paths: &mut BTreeSet<PathBuf>,
) -> WatchEventOutcome {
    if event.kind.is_access() {
        return WatchEventOutcome::Ignored;
    }
    let relevant: Vec<PathBuf> = event
        .paths
        .into_iter()
        .filter(|path| !ignored_watch_path(repo_root, store_root, path))
        .collect();
    if relevant.is_empty() {
        return WatchEventOutcome::Ignored;
    }

    match event.kind {
        EventKind::Create(CreateKind::File) | EventKind::Remove(RemoveKind::File) => {}
        EventKind::Modify(ModifyKind::Name(_)) => {
            let Some(destination) = relevant.last() else {
                return WatchEventOutcome::Reconcile;
            };
            if destination.is_dir() || !destination.exists() {
                return WatchEventOutcome::Reconcile;
            }
        }
        EventKind::Create(_) | EventKind::Modify(_) => {
            if relevant.iter().any(|path| path.is_dir()) {
                return WatchEventOutcome::Reconcile;
            }
        }
        // A removed directory cannot be distinguished by metadata after the
        // event. Only an explicitly file-typed removal is safe incrementally.
        EventKind::Remove(_) | EventKind::Any | EventKind::Other => {
            return WatchEventOutcome::Reconcile;
        }
        _ => return WatchEventOutcome::Reconcile,
    }
    paths.extend(relevant);
    WatchEventOutcome::Incremental
}

#[cfg(unix)]
fn update_time_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(unix)]
fn open_fresh_snapshot(store_root: &Path, repo_root: &Path) -> Result<Arc<Snapshot>> {
    // Invalidate only this store's process-cache entry. A process-wide clear
    // forced every concurrent MCP/CodeMode root to cold-open after one-file
    // watch edits (graphzero-n4xyy).
    Snapshot::invalidate_open_cache_for(store_root);
    Ok(Arc::new(Snapshot::open(store_root, Some(repo_root))?))
}

#[cfg(unix)]
fn publish_watch_batch(
    shared: &Arc<Mutex<StemState>>,
    paths: Vec<PathBuf>,
    reconcile: bool,
    events_seen: u64,
) {
    let (repo_root, store_root) = {
        let mut state = shared.lock();
        state.events_seen = state.events_seen.saturating_add(events_seen);
        (state.repo_root.clone(), state.store_root.clone())
    };

    if !reconcile && paths.is_empty() {
        return;
    }
    #[cfg(test)]
    let injected_failure = FAIL_NEXT_WATCH_INDEX.swap(false, Ordering::SeqCst);
    #[cfg(not(test))]
    let injected_failure = false;
    let result = if injected_failure {
        Err(anyhow::anyhow!("injected watcher index failure"))
    } else if reconcile {
        super::indexer::reconcile_repo_without_notify(&repo_root, &store_root)
            .map(|entry| (entry, 0usize))
    } else {
        super::indexer::index_changed_paths_without_notify(&repo_root, &store_root, &paths)
            .map(|indexed| (indexed.entry, indexed.stats.reparsed_files))
    };
    match result {
        Ok((_entry, reparsed_files)) => match open_fresh_snapshot(&store_root, &repo_root) {
            Ok(snapshot) => {
                let mut state = shared.lock();
                state.snapshot = snapshot;
                state.files_reindexed = state.files_reindexed.saturating_add(reparsed_files as u64);
                if reconcile {
                    state.reconciliations = state.reconciliations.saturating_add(1);
                }
                state.last_index_error = None;
                state.last_update_unix_ms = Some(update_time_unix_ms());
            }
            Err(error) => {
                shared.lock().last_index_error = Some(error.to_string());
            }
        },
        Err(error) => {
            shared.lock().last_index_error = Some(error.to_string());
        }
    }
}

#[cfg(unix)]
fn watcher_event_loop(shared: Arc<Mutex<StemState>>, receiver: Receiver<notify::Result<Event>>) {
    loop {
        let Ok(first) = receiver.recv() else {
            break;
        };
        let (repo_root, store_root) = {
            let state = shared.lock();
            (state.repo_root.clone(), state.store_root.clone())
        };
        let mut paths = BTreeSet::new();
        // Only events that survive filtering are counted. Counting ignored
        // events made `events_seen` advance forever under unrelated churn
        // (editor scratch files, build output), so an observer waiting for the
        // stem to settle could never see it settle even with no work pending.
        let mut events_seen = 0u64;
        let mut reconcile = false;
        let absorb = |outcome: WatchEventOutcome, seen: &mut u64, reconcile: &mut bool| {
            if outcome != WatchEventOutcome::Ignored {
                *seen = seen.saturating_add(1);
            }
            *reconcile |= outcome == WatchEventOutcome::Reconcile;
        };
        let first_outcome = match first {
            Ok(event) => collect_event_paths(event, &repo_root, &store_root, &mut paths),
            Err(_) => WatchEventOutcome::Reconcile,
        };
        absorb(first_outcome, &mut events_seen, &mut reconcile);
        // The debounce window is extended only by events the stem will act on.
        // Backends that report reads (inotify emits IN_OPEN/IN_ACCESS; FSEvents
        // does not) otherwise keep the window alive forever while an observer
        // polls the pid/state files, so the batch is never published at all.
        let mut deadline = Instant::now() + WATCH_DEBOUNCE;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let outcome = match receiver.recv_timeout(remaining) {
                Ok(Ok(event)) => collect_event_paths(event, &repo_root, &store_root, &mut paths),
                Ok(Err(_)) => WatchEventOutcome::Reconcile,
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            };
            if outcome != WatchEventOutcome::Ignored {
                deadline = Instant::now() + WATCH_DEBOUNCE;
            }
            absorb(outcome, &mut events_seen, &mut reconcile);
        }
        publish_watch_batch(&shared, paths.into_iter().collect(), reconcile, events_seen);
    }
}

#[cfg(unix)]
fn spawn_watcher_loop(
    shared: Arc<Mutex<StemState>>,
    watcher: RecommendedWatcher,
    receiver: Receiver<notify::Result<Event>>,
) {
    thread::spawn(move || {
        let _watcher = watcher;
        watcher_event_loop(shared, receiver);
    });
}

/// Accept loop: one thread per connection so concurrent Snaps can progress while
/// another client is in warm (graphzero-or15b). StemState is only held for short
/// Arc clones / status / tip swaps (see graphzero-p12n4); unbounded spawn is fine
/// for the warm-stem skeleton (query clients are agent/host tools, not open web).
#[cfg(unix)]
fn serve_incoming_loop(shared: &Arc<Mutex<StemState>>, listener: &UnixListener) {
    // Nonblocking accept with a short poll so an authenticated Shutdown RPC can
    // break the loop promptly (idle cost is a 10ms sleep per poll).
    let _ = listener.set_nonblocking(true);
    loop {
        if shared
            .lock()
            .shutdown_requested
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            break;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let shared = Arc::clone(shared);
                thread::spawn(move || {
                    if let Err(e) = handle_client(&shared, stream) {
                        log_daemon_client_error(&e);
                    }
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }
}

/// Run the stem event loop until an authenticated Shutdown RPC or listener
/// error. On graceful stop the stem self-cleans its daemon artifacts.
#[cfg(unix)]
pub fn run_stem(store_root: &Path, repo_root: &Path) -> Result<()> {
    // Install the recursive watcher before reconciliation so edits racing
    // startup queue behind a complete baseline instead of falling into a gap.
    let (watcher, receiver) = prepare_watcher(repo_root)?;
    super::indexer::reconcile_repo_without_notify(repo_root, store_root)?;

    fs::create_dir_all(daemon_dir(store_root))?;
    restrict_daemon_permissions(store_root)?;
    let sock = socket_path(store_root);
    let _ = fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).with_context(|| format!("bind {}", sock.display()))?;
    restrict_daemon_permissions(store_root)?;

    let shared = init_stem_shared(store_root, repo_root)?;
    // Install the captured binding before serving so Shutdown authorization
    // compares against the immutable spawn-time identity, never mutable state.
    let binding = register_stem_process(store_root, repo_root)?;
    shared.lock().binding = Some(binding);
    spawn_watcher_loop(Arc::clone(&shared), watcher, receiver);
    serve_incoming_loop(&shared, &listener);
    // Graceful shutdown requested: remove our own artifacts (idempotent with
    // `disable_daemon`, which may already have removed them).
    let _ = fs::remove_file(&sock);
    let _ = fs::remove_file(pid_path(store_root));
    let _ = fs::remove_file(identity_path(store_root));
    Ok(())
}

/// The warm daemon requires unix domain sockets. On other platforms it is
/// explicitly unsupported; cold mode remains the fully functional default.
#[cfg(not(unix))]
pub fn run_stem(_store_root: &Path, _repo_root: &Path) -> Result<()> {
    bail!(
        "the warm daemon requires unix domain sockets and is unavailable on this platform; cold mode remains fully functional"
    )
}

#[cfg(unix)]
fn read_client_request(stream: &UnixStream) -> Result<ClientRequest> {
    let mut reader = std::io::BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    Ok(serde_json::from_str(line.trim())?)
}

#[cfg(unix)]
fn dispatch_client_request(
    shared: &Mutex<StemState>,
    req: ClientRequest,
) -> Result<ServerResponse> {
    Ok(match req {
        ClientRequest::Ping => ServerResponse::Pong,
        ClientRequest::Status => ServerResponse::Status {
            status: build_status(&shared.lock(), true),
        },
        ClientRequest::Snap { symbol, budget } => {
            // Clone Arc under short CS; warm + to_json run lock-free so NotifyIndex
            // / watch publish can swap the resident tip without waiting on query
            // latency (graphzero-p12n4).
            let (snapshot, store_root) = {
                let guard = shared.lock();
                (Arc::clone(&guard.snapshot), guard.store_root.clone())
            };
            #[cfg(test)]
            {
                let hold_ms = SNAP_HOLD_MS.load(Ordering::SeqCst);
                if hold_ms > 0 {
                    thread::sleep(Duration::from_millis(hold_ms));
                }
                let n = SNAP_IN_FLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
                let mut peak = SNAP_IN_FLIGHT_PEAK.load(Ordering::SeqCst);
                while n > peak {
                    match SNAP_IN_FLIGHT_PEAK.compare_exchange_weak(
                        peak,
                        n,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    ) {
                        Ok(_) => break,
                        Err(cur) => peak = cur,
                    }
                }
            }
            let capsule = QueryEngine::warm(snapshot.as_ref(), &symbol, budget)?;
            #[cfg(test)]
            {
                SNAP_IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
            }
            QUERY_COUNT.fetch_add(1, Ordering::Relaxed);
            ServerResponse::Snap {
                capsule_json: capsule.to_json(Some(&store_root)),
            }
        }
        ClientRequest::NotifyIndex => {
            let (store_root, repo_root) = {
                let guard = shared.lock();
                (guard.store_root.clone(), guard.repo_root.clone())
            };
            let snapshot = open_fresh_snapshot(&store_root, &repo_root)?;
            let mut guard = shared.lock();
            guard.snapshot = snapshot;
            guard.last_index_error = None;
            guard.last_update_unix_ms = Some(update_time_unix_ms());
            INDEX_NOTIFY_COUNT.fetch_add(1, Ordering::Relaxed);
            ServerResponse::Ok
        }
        // Handled before dispatch in `handle_client` with peer-euid + owner +
        // generation authentication; reaching this arm means an unauthenticated
        // path.
        ClientRequest::Shutdown { .. } => {
            bail!("shutdown must be authenticated by handle_client")
        }
    })
}

#[cfg(unix)]
fn write_server_response(stream: UnixStream, resp: &ServerResponse) -> Result<()> {
    let mut writer = BufWriter::new(stream);
    serde_json::to_writer(&mut writer, resp)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

#[cfg(unix)]
fn handle_client(shared: &Mutex<StemState>, stream: UnixStream) -> Result<()> {
    let req = read_client_request(&stream)?;
    if let ClientRequest::Shutdown {
        owner_session,
        generation,
    } = req
    {
        // Authenticated shutdown: the peer must run as the same effective user
        // AND present the exact owner session + worker generation captured at
        // spawn (the immutable StemState binding -- never a reread of mutable
        // `state.json`). Any mismatch is rejected immediately (fast Error
        // response, stem stays alive).
        let message = {
            let guard = shared.lock();
            if !child_identity::peer_is_same_user(&stream) {
                Some("unauthorized daemon shutdown attempt: peer euid mismatch".to_string())
            } else if let Some(binding) = &guard.binding {
                match binding.verify_owner(&owner_session, generation) {
                    Ok(()) => None,
                    Err(error) => Some(format!("daemon shutdown rejected: {error}")),
                }
            } else {
                Some("daemon shutdown rejected: stem identity binding not installed".to_string())
            }
        };
        if let Some(message) = message {
            return write_server_response(stream, &ServerResponse::Error { message });
        }
        shared
            .lock()
            .shutdown_requested
            .store(true, std::sync::atomic::Ordering::SeqCst);
        return write_server_response(stream, &ServerResponse::Ok);
    }
    let resp = dispatch_client_request(shared, req)?;
    write_server_response(stream, &resp)
}

#[cfg(unix)]
fn build_status(state: &StemState, resident: bool) -> DaemonStatus {
    DaemonStatus {
        daemon: if resident { "warm" } else { "disabled" }.into(),
        mode: if resident {
            DaemonMode::Warm
        } else {
            DaemonMode::Cold
        },
        enabled: resident,
        socket: Some(socket_path(&state.store_root).display().to_string()),
        pid: fs::read_to_string(pid_path(&state.store_root))
            .ok()
            .and_then(|s| s.trim().parse().ok()),
        stem: Some(StemMetrics {
            queries_served: QUERY_COUNT.load(Ordering::Relaxed),
            index_notifications: INDEX_NOTIFY_COUNT.load(Ordering::Relaxed),
            snapshot_id: state.snapshot.entry.snapshot_id,
            idle: true,
            events_seen: state.events_seen,
            files_reindexed: state.files_reindexed,
            reconciliations: state.reconciliations,
            last_index_error: state.last_index_error.clone(),
            last_update_unix_ms: state.last_update_unix_ms,
        }),
        note: "P2.2 stem walking skeleton; cold mode remains default when stem is not running"
            .into(),
    }
}

pub fn daemon_status(store_root: &Path) -> DaemonStatus {
    // Resident liveness binds the verified identity record; a bare-pid probe
    // (kill 0) is never used. Without a valid identity record the stem is
    // treated as cold even if a pid file exists (old or foreign pid).
    let resident = read_daemon_binding(store_root)
        .map(|binding| binding.is_live())
        .unwrap_or(false);

    if !resident {
        return DaemonStatus {
            daemon: "disabled".into(),
            mode: DaemonMode::Cold,
            enabled: is_enabled(store_root),
            socket: socket_path(store_root)
                .exists()
                .then(|| socket_path(store_root).display().to_string()),
            pid: None,
            stem: None,
            note: "cold mode is the default; run `graphzero daemon enable` for warm path".into(),
        };
    }

    #[cfg(unix)]
    if let Ok(json) = daemon_client_request(store_root, &ClientRequest::Status)
        && let Ok(ServerResponse::Status { status }) = serde_json::from_str(&json)
    {
        return status;
    }

    DaemonStatus {
        daemon: "warm".into(),
        mode: DaemonMode::Warm,
        enabled: true,
        socket: Some(socket_path(store_root).display().to_string()),
        pid: read_daemon_binding(store_root).map(|binding| binding.pid),
        stem: None,
        note: "stem process present (identity-verified); status RPC unavailable".into(),
    }
}

/// Read and parse the verified identity record written by the stem at spawn.
fn read_daemon_binding(store_root: &Path) -> Option<child_identity::ChildBinding> {
    fs::read_to_string(identity_path(store_root))
        .ok()
        .and_then(|text| child_identity::ChildBinding::decode(&text).ok())
}

pub fn disable_daemon(store_root: &Path) -> Result<()> {
    // Expected owner + generation come from the persisted state (written by
    // `enable`); every teardown step below binds to them.
    let (expected_owner, expected_generation) = expected_daemon_binding(store_root);

    // 0. Load the captured identity record and verify it against the
    //    persisted expected owner + generation BEFORE any teardown action.
    //    A live binding that does not match (e.g. state.json was rewritten)
    //    fails closed with artifacts preserved. A missing/malformed record
    //    while a socket exists is unverifiable: fail closed. Only a verified
    //    stale exit (record present, process gone) may proceed to cleanup.
    let loaded = read_daemon_binding(store_root);
    match loaded.as_ref() {
        Some(binding) if binding.is_live() => {
            binding.verify_owner(&expected_owner, expected_generation).map_err(|error| {
                anyhow::anyhow!(
                    "daemon binding mismatch (expected owner {expected_owner:?}, generation {expected_generation}): {error}; refusing to signal and preserving artifacts"
                )
            })?;
        }
        Some(_) => {
            // Stale exited identity: process gone; cleanup is safe.
        }
        None if socket_path(store_root).exists()
            || pid_path(store_root)
                .exists()
                .then(|| fs::read_to_string(pid_path(store_root)).ok())
                .flatten()
                .and_then(|text| text.trim().parse::<u32>().ok())
                .is_some_and(|pid| child_identity::ProcessIdentity::capture(pid).is_ok()) =>
        {
            return Err(anyhow::anyhow!(
                "daemon identity record missing or malformed while live stem artifacts exist; refusing to tear down an unverifiable process and preserving artifacts"
            ));
        }
        None => {
            // No socket and no live process at the stale/invalid pid: cleanup is safe.
        }
    }

    // 1. Authenticated Shutdown RPC carrying the expected owner + generation.
    let acked = request_stem_shutdown(store_root, &expected_owner, expected_generation);

    // 2. Bounded identity-bound wait for the stem to exit after the ACK.
    if acked {
        wait_for_stem_exit(store_root, STEM_EXIT_WAIT);
    }

    // 3. If the bound process is still live, exact-escalate (already verified
    //    against the expected owner + gen in step 0).
    if let Some(binding) = read_daemon_binding(store_root)
        && binding.is_live()
    {
        match escalate_detached_with_seam(&binding, ESCALATION_GRACE) {
            Ok(_) => {}
            Err(error) if !binding.is_live() => {
                // Escalation reported failure but the bound process is already
                // gone: treat as verified stale and proceed to cleanup.
                eprintln!("daemon escalation reported error after process exit: {error}");
            }
            Err(error) => {
                // Fail closed: the bound process is still live and no exact
                // signal was delivered. Preserve artifacts so a retry can
                // recover; never orphan a live process silently.
                return Err(anyhow::anyhow!(
                    "failed to terminate the running daemon stem (owner {expected_owner:?}, generation {expected_generation}): {error}; socket/pid/identity artifacts preserved for retry"
                ));
            }
        }
    }

    // 4. Verified exit or verified-stale: remove artifacts, write disabled.
    let _ = fs::remove_file(socket_path(store_root));
    let _ = fs::remove_file(pid_path(store_root));
    let _ = fs::remove_file(identity_path(store_root));
    let repo = read_persisted_state(store_root).repo_root;
    let repo = if repo.as_os_str().is_empty() {
        store_root.parent().unwrap_or(store_root).to_path_buf()
    } else {
        repo
    };
    write_enabled_state_with(
        store_root,
        &repo,
        false,
        &expected_owner,
        expected_generation,
    )?;
    Ok(())
}

/// Bounded wait for the stem to exit, observed through the identity record
/// (real detached daemon: process exit -> identity not live) or through the
/// stem's own artifact self-cleanup (same-process test stems, whose process
/// identity outlives the serve loop).
fn wait_for_stem_exit(store_root: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if stem_exited(store_root) {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn stem_exited(store_root: &Path) -> bool {
    let live = read_daemon_binding(store_root)
        .map(|binding| binding.is_live())
        .unwrap_or(false);
    if !live {
        return true;
    }
    // Same-process test stems keep the process (and its identity) alive; the
    // observable exit is the stem's self-cleanup of socket + identity files.
    !socket_path(store_root).exists() && !identity_path(store_root).exists()
}

/// Test-only escalation seam: forces `escalate_detached` to fail so the
/// fail-closed artifact-preservation path can be exercised deterministically
/// without platform fakery.
#[cfg(test)]
static FORCE_ESCALATION_FAILURE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
fn escalate_detached_with_seam(
    binding: &child_identity::ChildBinding,
    grace: Duration,
) -> Result<child_identity::SignalOutcome, child_identity::IdentityError> {
    if FORCE_ESCALATION_FAILURE.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(child_identity::IdentityError::Unsupported);
    }
    child_identity::escalate_detached(binding, grace)
}

#[cfg(not(test))]
fn escalate_detached_with_seam(
    binding: &child_identity::ChildBinding,
    grace: Duration,
) -> Result<child_identity::SignalOutcome, child_identity::IdentityError> {
    child_identity::escalate_detached(binding, grace)
}

/// Authenticated graceful stop over the owned socket. Returns true only when
/// the stem acknowledged the shutdown RPC (peer euid + owner + generation all
/// matched).
#[cfg(unix)]
fn request_stem_shutdown(store_root: &Path, owner_session: &str, generation: u64) -> bool {
    if !socket_path(store_root).exists() {
        return false;
    }
    let req = ClientRequest::Shutdown {
        owner_session: owner_session.to_string(),
        generation,
    };
    match daemon_client_request(store_root, &req) {
        Ok(json) => matches!(
            serde_json::from_str::<ServerResponse>(&json),
            Ok(ServerResponse::Ok)
        ),
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn request_stem_shutdown(_store_root: &Path, _owner_session: &str, _generation: u64) -> bool {
    false
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(test)]
enum ProcessProbe {
    Alive,
    PermissionDenied,
    Missing,
}

#[cfg(test)]
fn classify_process_probe(kill_rc: i32, errno: Option<i32>) -> ProcessProbe {
    if kill_rc == 0 {
        return ProcessProbe::Alive;
    }
    #[cfg(unix)]
    {
        if errno == Some(libc::EPERM) {
            return ProcessProbe::PermissionDenied;
        }
    }
    ProcessProbe::Missing
}

#[cfg(test)]
fn process_alive(pid: u32) -> bool {
    if pid <= 1 {
        return false;
    }
    #[cfg(unix)]
    {
        // SAFETY: signal 0 performs existence/permission probing only and does
        // not deliver a signal. Test-only helper; release status binds the
        // verified identity record instead of probing bare pids.
        let rc = unsafe { libc::kill(pid as i32, 0) };
        let errno = (rc == -1)
            .then(|| std::io::Error::last_os_error().raw_os_error())
            .flatten();
        matches!(
            classify_process_probe(rc, errno),
            ProcessProbe::Alive | ProcessProbe::PermissionDenied
        )
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

#[cfg(unix)]
pub fn try_warm_snap(store_root: &Path, symbol: &str, budget: usize) -> Result<Option<String>> {
    if !socket_path(store_root).exists() {
        return Ok(None);
    }
    let json = match daemon_client_request(
        store_root,
        &ClientRequest::Snap {
            symbol: symbol.to_string(),
            budget,
        },
    ) {
        Ok(j) => j,
        Err(_) => return Ok(None), // daemon not running; fall through to cold mode
    };
    let resp: ServerResponse = serde_json::from_str(&json)?;
    match resp {
        ServerResponse::Snap { capsule_json } => Ok(Some(capsule_json)),
        ServerResponse::Error { message } => bail!("daemon snap failed: {message}"),
        _ => bail!("unexpected daemon response"),
    }
}

/// No warm path without unix sockets: always the documented cold fallback.
#[cfg(not(unix))]
pub fn try_warm_snap(_store_root: &Path, _symbol: &str, _budget: usize) -> Result<Option<String>> {
    Ok(None)
}

#[cfg(unix)]
fn daemon_client_request(store_root: &Path, req: &ClientRequest) -> Result<String> {
    let sock = socket_path(store_root);
    let mut stream =
        UnixStream::connect(&sock).with_context(|| format!("connect {}", sock.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let payload = serde_json::to_string(req)?;
    writeln!(stream, "{payload}")?;
    let mut reader = std::io::BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    Ok(line.trim().to_string())
}

/// Spawn stem on a background thread (integration tests).
pub fn spawn_stem_for_test(store_root: PathBuf, repo_root: PathBuf) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        if let Err(error) = run_stem(&store_root, &repo_root) {
            eprintln!("graphzero test stem failed: {error:#}");
        }
    })
}

#[cfg(unix)]
fn restrict_daemon_permissions(store_root: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let dir = daemon_dir(store_root);
    if dir.is_dir() {
        let mut perms = fs::metadata(&dir)?.permissions();
        perms.set_mode(0o700);
        fs::set_permissions(&dir, perms)?;
    }
    let sock = socket_path(store_root);
    if sock.exists() {
        let mut perms = fs::metadata(&sock)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&sock, perms)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn restrict_daemon_permissions(_store_root: &Path) -> Result<()> {
    Ok(())
}
