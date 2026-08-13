//! Machine-wide CodeMode permit (slot layout, reclaim, backoff).
//!
//! Shared contract for TokenZero / FSZero / GraphZero: directory-based locks
//! under `/tmp/zerostack-codemode-*.permit` with `slot-N` children. Live holders
//! block peers until wall deadline (retryable busy); dead / incomplete dirs are
//! reclaimed. Fatal I/O (EACCES, etc.) stays non-retryable.
//!
//! Canonical policy: `tokenzero-mcp/CODEMODE_MACHINE_PERMITS.md`.

use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Write};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub mod session_owner;

pub const PERMIT_POLL: Duration = Duration::from_millis(20);
pub const PERMIT_POLL_MAX: Duration = Duration::from_millis(200);
pub const PERMIT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
pub const PERMIT_HEARTBEAT_MAX_INTERVAL: Duration = Duration::from_secs(60);
const INCOMPLETE_PERMIT_GRACE: Duration = Duration::from_millis(250);
const WAITER_IDENTITY_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
// Seven days is deliberately generous, but prevents unverifiable non-Linux holders wedging forever.
const OWNER_IDENTITY_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Explicit diagnostic ownership metadata for one held machine permit.
///
/// The legacy acquire methods derive these values from the process and
/// environment. The typed acquire methods write these exact values instead.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermitOwnerMetadata {
    pub repository: String,
    pub operation: String,
    pub session_ref: String,
    pub cell_ref: String,
}

impl PermitOwnerMetadata {
    pub fn new(
        repository: impl Into<String>,
        operation: impl Into<String>,
        session_ref: impl Into<String>,
        cell_ref: impl Into<String>,
    ) -> Self {
        Self {
            repository: repository.into(),
            operation: operation.into(),
            session_ref: session_ref.into(),
            cell_ref: cell_ref.into(),
        }
    }

    fn from_command(command: &str) -> Self {
        let started_at = epoch_millis();
        let pid = std::process::id();
        let owner = format!("{}-{}-{:?}", pid, started_at, std::thread::current().id());
        let repository = permit_scope_root()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let session_ref = std::env::var("ZEROSTACK_SESSION_ID")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| format!("cm://session/{}", metadata_text(&value)))
            .unwrap_or_else(|| format!("cm://session/process/{pid}/{started_at}"));
        Self::new(
            repository.to_string_lossy(),
            command,
            session_ref,
            format!("cm://cell/process/{}", metadata_text(&owner)),
        )
    }
}

/// A permit owner plus a bounded heartbeat thread.
///
/// The worker thread owns the underlying `MachinePermit`. Dropping or
/// explicitly stopping this value signals the thread, joins it, and therefore
/// releases the permit exactly once through the original cookie fence.
pub struct MachinePermitHeartbeat {
    stop: Arc<(Mutex<bool>, Condvar)>,
    worker: Option<JoinHandle<()>>,
    path: PathBuf,
}

/// Compatibility alias for callers that name the lease by its heartbeat.
pub type PermitHeartbeat = MachinePermitHeartbeat;

impl MachinePermitHeartbeat {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        if let Ok(mut stopped) = self.stop.0.lock() {
            *stopped = true;
            self.stop.1.notify_one();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for MachinePermitHeartbeat {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

/// Repo-scoped permit base below the current user's private runtime directory.
pub fn scoped_permit_base(class: &str) -> PathBuf {
    try_scoped_permit_base(class)
        .unwrap_or_else(|error| panic!("resolve configured permit scope root: {error}")) // ubs:ignore — documented panicking wrapper; library callers use try_scoped_permit_base
}

/// Fallible repo-scoped permit base resolution for acquisition paths.
pub fn try_scoped_permit_base(class: &str) -> io::Result<PathBuf> {
    try_scoped_permit_base_for(class, permit_scope_root().as_deref())
}

pub fn scoped_permit_base_for(class: &str, scope_root: Option<&Path>) -> PathBuf {
    try_scoped_permit_base_for(class, scope_root)
        .unwrap_or_else(|error| panic!("resolve configured permit scope root: {error}")) // ubs:ignore — documented panicking wrapper; library callers use try_scoped_permit_base_for
}

/// Resolve an explicit scope root without ever hashing an uncanonicalized path.
pub fn try_scoped_permit_base_for(class: &str, scope_root: Option<&Path>) -> io::Result<PathBuf> {
    let class = sanitize_permit_class(class);
    let suffix = if let Some(root) = scope_root {
        let canonical = fs::canonicalize(root)?;
        if !canonical.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!(
                    "permit scope root is not a directory: {}",
                    canonical.display()
                ),
            ));
        }
        format!("-{:016x}", fnv1a64(canonical.to_string_lossy().as_bytes()))
    } else {
        String::new()
    };
    Ok(permit_runtime_dir()?.join(format!("zerostack-codemode-{class}{suffix}.permit")))
}

#[cfg(unix)]
fn permit_runtime_dir() -> io::Result<PathBuf> {
    let xdg = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    unix_runtime_dir_for(xdg.as_deref(), &std::env::temp_dir())
}

#[cfg(unix)]
fn unix_runtime_dir_for(xdg: Option<&Path>, temp: &Path) -> io::Result<PathBuf> {
    if let Some(path) = xdg.filter(|path| path.is_absolute())
        && verify_unix_private_dir(path, false).is_ok()
    {
        return Ok(path.to_path_buf());
    }
    let path = temp.join(format!("zerostack-runtime-{}", effective_uid()));
    ensure_unix_private_dir(&path, true)?;
    Ok(path)
}

#[cfg(unix)]
fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions, reads process credentials only, and
    // does not retain pointers or mutate Rust-managed memory.
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn ensure_unix_private_dir(path: &Path, exact_mode: bool) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    match builder.create(path) {
        Ok(()) => fs::set_permissions(path, fs::Permissions::from_mode(0o700))?,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    verify_unix_private_dir(path, exact_mode)
}

#[cfg(unix)]
fn verify_unix_private_dir(path: &Path, exact_mode: bool) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "permit directory is not a real directory: {}",
                path.display()
            ),
        ));
    }
    if metadata.uid() != effective_uid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "permit directory is not owned by the effective uid: {}",
                path.display()
            ),
        ));
    }
    let mode = metadata.mode() & 0o777;
    if mode & 0o077 != 0 || (exact_mode && mode != 0o700) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "permit directory has unsafe mode {mode:o}: {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn permit_runtime_dir() -> io::Result<PathBuf> {
    let parent = std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let path = parent.join("ZeroStack");
    // std cannot verify Windows ACLs. Atomic create_dir plus refusing links and
    // non-directories prevents silently accepting an attacker-chosen leaf.
    ensure_portable_private_dir(&path)?;
    Ok(path)
}

#[cfg(not(any(unix, windows)))]
fn permit_runtime_dir() -> io::Result<PathBuf> {
    let path = std::env::temp_dir().join("ZeroStack");
    ensure_portable_private_dir(&path)?;
    Ok(path)
}

#[cfg(not(unix))]
fn ensure_portable_private_dir(path: &Path) -> io::Result<()> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "permit directory is not a real directory: {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

/// Verify an existing permit base before acquisition uses it.
pub fn verify_permit_base(base: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        verify_unix_private_dir(base, false)
    }
    #[cfg(not(unix))]
    {
        let metadata = fs::symlink_metadata(base)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("permit base is not a real directory: {}", base.display()),
            ));
        }
        Ok(())
    }
}

fn prepare_permit_base(base: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        ensure_unix_private_dir(base, false)
    }
    #[cfg(not(unix))]
    {
        ensure_portable_private_dir(base)
    }
}

/// Permit class path segment: non-empty `[A-Za-z0-9._-]+`, else `"invalid"`.
///
/// Used only as a single filename component under `/tmp/zerostack-codemode-…`.
/// Rejects empty input, path separators, `..`, and any other char outside the
/// safe charset so untrusted class strings cannot escape the /tmp basename.
pub(crate) fn sanitize_permit_class(class: &str) -> &str {
    if is_safe_permit_class(class) {
        class
    } else {
        "invalid"
    }
}

/// True when byte is allowed in a permit class path segment ([A-Za-z0-9._-]).
const SAFE_PERMIT_BYTE: [bool; 256] = [
    false, false, false, false, false, false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, true, true, false, true, true, true, true, true,
    true, true, true, true, true, false, false, false, false, false, false, false, true, true,
    true, true, true, true, true, true, true, true, true, true, true, true, true, true, true, true,
    true, true, true, true, true, true, true, true, false, false, false, false, true, false, true,
    true, true, true, true, true, true, true, true, true, true, true, true, true, true, true, true,
    true, true, true, true, true, true, true, true, true, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false, false, false, false, false, false,
    false, false, false, false, false, false, false, false, false, false,
];

fn is_safe_permit_class(class: &str) -> bool {
    // Charset alone still allows "." / ".." as a bare path segment.
    match class {
        "" | "." | ".." => false,
        _ => class.bytes().all(|b| SAFE_PERMIT_BYTE[b as usize]),
    }
}

fn permit_scope_root() -> Option<PathBuf> {
    for name in [
        "ZEROSTACK_PERMIT_SCOPE_ROOT",
        "FSZERO_ROOT",
        "TOKENZERO_ROOT",
        "GZ_REPO_ROOT",
    ] {
        match std::env::var_os(name) {
            Some(value) if !value.is_empty() => return Some(PathBuf::from(value)),
            _ => {}
        }
    }
    None
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// RAII machine permit for one slot (or legacy exclusive) directory.
#[derive(Debug)]
pub struct MachinePermit {
    path: PathBuf,
    cookie: String,
}

impl MachinePermit {
    /// Path of the held permit directory (`base/slot-N` or legacy exclusive).
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn owner_metadata(&self) -> io::Result<PermitOwnerMetadata> {
        Ok(PermitOwnerMetadata::new(
            read_required_metadata(&self.path, "repository")?,
            read_required_metadata(&self.path, "operation")
                .or_else(|_| read_required_metadata(&self.path, "command"))?,
            read_required_metadata(&self.path, "session_ref")?,
            read_required_metadata(&self.path, "cell_ref")?,
        ))
    }

    /// Move this permit into a bounded heartbeat owner.
    pub fn start_heartbeat(self, interval: Duration) -> io::Result<MachinePermitHeartbeat> {
        if interval.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "machine permit heartbeat interval must be nonzero",
            ));
        }
        let interval = interval.min(PERMIT_HEARTBEAT_MAX_INTERVAL);
        self.heartbeat()?;
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let thread_stop = Arc::clone(&stop);
        let path = self.path.clone();
        let worker = thread::Builder::new()
            .name("zerostack-permit-heartbeat".into())
            .spawn(move || {
                loop {
                    let guard = match thread_stop.0.lock() {
                        Ok(guard) => guard,
                        Err(_) => break,
                    };
                    let guard = match thread_stop.1.wait_timeout(guard, interval) {
                        Ok((guard, _)) => guard,
                        Err(_) => break,
                    };
                    if *guard {
                        break;
                    }
                    drop(guard);
                    if self.heartbeat().is_err() {
                        break;
                    }
                }
                drop(self);
            })?;
        Ok(MachinePermitHeartbeat {
            stop,
            worker: Some(worker),
            path,
        })
    }

    pub fn heartbeat_in_background(self, interval: Duration) -> io::Result<MachinePermitHeartbeat> {
        self.start_heartbeat(interval)
    }

    /// Refresh the diagnostic lease timestamp after verifying that this guard
    /// still owns the exact permit cookie. A replaced owner is never touched.
    pub fn heartbeat(&self) -> io::Result<()> {
        let observed = fs::read(self.path.join("identity"))?;
        if parse_identity(&observed)
            .as_ref()
            .map(|identity| identity.cookie.as_str())
            != Some(self.cookie.as_str())
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "machine permit ownership changed before heartbeat",
            ));
        }
        let temporary = self.path.join(format!(".heartbeat-{}.tmp", self.cookie));
        write_file(&temporary, &epoch_millis().to_string())?;
        fs::rename(temporary, self.path.join("heartbeat_at"))
    }

    pub fn acquire_slots_with_owner_metadata(
        base: &Path,
        slots: usize,
        deadline: Instant,
        owner: PermitOwnerMetadata,
    ) -> Result<Self, AcquireError> {
        Self::acquire_slots_with_wake_and_owner(base, slots, deadline, owner, PermitWake::new)
    }

    pub fn acquire_slots_with_owner(
        base: &Path,
        slots: usize,
        deadline: Instant,
        owner: PermitOwnerMetadata,
    ) -> Result<Self, AcquireError> {
        Self::acquire_slots_with_owner_metadata(base, slots, deadline, owner)
    }

    pub fn acquire_slots(
        base: &Path,
        slots: usize,
        deadline: Instant,
        command: &str,
    ) -> Result<Self, AcquireError> {
        Self::acquire_slots_with_owner_metadata(
            base,
            slots,
            deadline,
            PermitOwnerMetadata::from_command(command),
        )
    }

    #[cfg(test)]
    fn acquire_slots_with_wake(
        base: &Path,
        slots: usize,
        deadline: Instant,
        command: &str,
        make_wake: impl FnOnce(&Path) -> PermitWake,
    ) -> Result<Self, AcquireError> {
        Self::acquire_slots_with_wake_and_owner(
            base,
            slots,
            deadline,
            PermitOwnerMetadata::from_command(command),
            make_wake,
        )
    }

    fn acquire_slots_with_wake_and_owner(
        base: &Path,
        slots: usize,
        deadline: Instant,
        owner: PermitOwnerMetadata,
        make_wake: impl FnOnce(&Path) -> PermitWake,
    ) -> Result<Self, AcquireError> {
        // Always use base/slot-N — even when slots==1 — so mixed concurrency
        // envs cannot stack an exclusive base lock with slot children.
        prepare_permit_base(base).map_err(|error| {
            AcquireError::Fatal(format!(
                "prepare codemode permit base {}: {error}",
                base.display()
            ))
        })?;
        // Pool size is the caller's requested budget (from env); do not freeze
        // capacity to the first asker — that would let CONCURRENCY=1 starve
        // the family-wide cores/4 analysis budget.
        let waiter = WaiterIntent::create(base)?;
        let mut wake = make_wake(base);
        let mut attempt = 0u32;
        loop {
            // Events wake the FIFO head immediately. The timeout is only a
            // lost-event safety net; younger waiters retain exponential
            // backoff so one directory event cannot cause an N-way scan storm.
            let has_preceding = waiter.has_preceding_competitor()?;
            if !has_preceding && !legacy_exclusive_busy(base) {
                for idx in 0..slots {
                    let path = base.join(format!("slot-{idx}"));
                    match Self::try_create_with_owner(&path, &owner) {
                        Ok(permit) => return Ok(permit),
                        Err(TryPermit::Busy) => {}
                        Err(TryPermit::Fatal(e)) => return Err(AcquireError::Fatal(e)),
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err(AcquireError::Busy(describe_busy_slots(base, slots)));
            }
            let wait_for = waiter_wait_timeout(has_preceding, attempt)
                .min(deadline.saturating_duration_since(Instant::now()));
            if has_preceding {
                attempt = attempt.saturating_add(1);
            } else {
                attempt = 0;
            }
            wake.wait(wait_for);
        }
    }

    /// Legacy exclusive single-dir permit (pre-slot layout). Production paths
    /// use `acquire_slots`; this remains for reclaim interop tests.
    pub fn acquire(path: &Path, deadline: Instant, command: &str) -> Result<Self, AcquireError> {
        let mut attempt = 0u32;
        loop {
            match Self::try_create(path, command) {
                Ok(permit) => return Ok(permit),
                Err(TryPermit::Busy) => {
                    if Instant::now() >= deadline {
                        return Err(AcquireError::Busy(describe_busy_path(path)));
                    }
                    let sleep_for = permit_backoff(attempt)
                        .min(deadline.saturating_duration_since(Instant::now()));
                    attempt = attempt.saturating_add(1);
                    std::thread::sleep(sleep_for);
                }
                Err(TryPermit::Fatal(e)) => return Err(AcquireError::Fatal(e)),
            }
        }
    }

    pub fn acquire_with_owner_metadata(
        path: &Path,
        deadline: Instant,
        owner: PermitOwnerMetadata,
    ) -> Result<Self, AcquireError> {
        let mut attempt = 0u32;
        loop {
            match Self::try_create_with_owner(path, &owner) {
                Ok(permit) => return Ok(permit),
                Err(TryPermit::Busy) => {
                    if Instant::now() >= deadline {
                        return Err(AcquireError::Busy(describe_busy_path(path)));
                    }
                    let sleep_for = permit_backoff(attempt)
                        .min(deadline.saturating_duration_since(Instant::now()));
                    attempt = attempt.saturating_add(1);
                    std::thread::sleep(sleep_for);
                }
                Err(TryPermit::Fatal(e)) => return Err(AcquireError::Fatal(e)),
            }
        }
    }

    fn try_create(path: &Path, command: &str) -> Result<Self, TryPermit> {
        let owner = PermitOwnerMetadata::from_command(command);
        Self::try_create_with_owner(path, &owner)
    }

    fn try_create_with_owner(
        path: &Path,
        owner_metadata: &PermitOwnerMetadata,
    ) -> Result<Self, TryPermit> {
        match fs::create_dir(path) {
            Ok(()) => {
                let cookie = owner_cookie();
                let identity_owner = format!(
                    "{}-{}-{:?}",
                    std::process::id(),
                    epoch_millis(),
                    std::thread::current().id()
                );
                if let Err(e) = write_metadata(path, &cookie, &identity_owner, owner_metadata) {
                    quarantine_exact(path, None);
                    return Err(TryPermit::Fatal(format!(
                        "write codemode permit metadata: {e}"
                    )));
                }
                if read_identity(path)
                    .as_ref()
                    .map(|identity| identity.cookie.as_str())
                    != Some(cookie.as_str())
                {
                    cleanup_owned(path, &cookie);
                    return Err(TryPermit::Busy);
                }
                Ok(Self {
                    path: path.to_path_buf(),
                    cookie,
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if reclaim_dead(path) {
                    return Self::try_create_with_owner(path, owner_metadata);
                }
                Err(TryPermit::Busy)
            }
            Err(e) => Err(TryPermit::Fatal(format!(
                "create codemode permit {}: {e}",
                path.display()
            ))),
        }
    }
}

#[derive(Debug)]
struct WaiterIntent {
    path: PathBuf,
    owner: String,
    cookie: String,
    started_at: u128,
}

impl WaiterIntent {
    fn create(base: &Path) -> Result<Self, AcquireError> {
        let waiters = base.join("waiters");
        fs::create_dir_all(&waiters).map_err(|e| {
            AcquireError::Fatal(format!(
                "create codemode permit waiter {}: {e}",
                waiters.display()
            ))
        })?;
        let started_at = epoch_millis();
        let cookie = owner_cookie();
        let owner = format!("{}-{started_at}-{cookie}", std::process::id());
        let path = waiters.join(&owner);
        fs::create_dir(&path).map_err(|e| {
            AcquireError::Fatal(format!(
                "create codemode permit waiter {}: {e}",
                path.display()
            ))
        })?;
        if let Err(e) = publish_identity(&path, &cookie, std::process::id(), &owner, started_at) {
            quarantine_exact(&path, None);
            return Err(AcquireError::Fatal(format!(
                "write codemode permit waiter metadata: {e}"
            )));
        }
        Ok(Self {
            path,
            owner,
            cookie,
            started_at,
        })
    }

    fn has_preceding_competitor(&self) -> Result<bool, AcquireError> {
        let own_key = (self.started_at, self.owner.as_str());
        Ok(self
            .live_competitors()?
            .into_iter()
            .any(|(started_at, owner)| (started_at, owner.as_str()) < own_key))
    }

    fn live_competitors(&self) -> Result<Vec<(u128, String)>, AcquireError> {
        let waiters = self.path.parent().ok_or_else(|| {
            AcquireError::Fatal("codemode permit waiter intent is missing a parent directory".into())
        })?;
        let entries = fs::read_dir(waiters).map_err(|e| {
            AcquireError::Fatal(format!(
                "read codemode permit waiters {}: {e}",
                waiters.display()
            ))
        })?;
        let mut live = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| {
                AcquireError::Fatal(format!(
                    "read codemode permit waiter entry {}: {e}",
                    waiters.display()
                ))
            })?;
            if let Some(competitor) = classify_waiter_entry(&entry, &self.path)? {
                live.push(competitor);
            }
        }
        Ok(live)
    }
}

/// Classify one peer waiter directory for ranking.
///
/// Reclaim fencing is load-bearing: a stale structured waiter is dropped only
/// after identity classification and cookie-fenced cleanup. Live processes must
/// must never lose their waiter slot here; failed removes stay visible.
fn classify_waiter_entry(
    entry: &fs::DirEntry,
    self_path: &Path,
) -> Result<Option<(u128, String)>, AcquireError> {
    let path = entry.path();
    if path == self_path {
        return Ok(None);
    }
    require_waiter_directory(entry, &path)?;
    match waiter_key(&path) {
        Some((pid, started_at)) => classify_structured_waiter(entry, &path, pid, started_at),
        None => classify_legacy_waiter(entry, &path),
    }
}

fn require_waiter_directory(entry: &fs::DirEntry, path: &Path) -> Result<(), AcquireError> {
    let is_directory = entry
        .file_type()
        .map_err(|e| {
            AcquireError::Fatal(format!(
                "inspect codemode permit waiter {}: {e}",
                path.display()
            ))
        })?
        .is_dir();
    if !is_directory {
        return Err(AcquireError::Fatal(format!(
            "codemode permit waiter is not a directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn classify_structured_waiter(
    entry: &fs::DirEntry,
    path: &Path,
    pid: u32,
    started_at: u128,
) -> Result<Option<(u128, String)>, AcquireError> {
    let Some(identity) = read_identity(path) else {
        if reclaim_dead(path) {
            return Ok(None);
        }
        return Ok(Some(waiter_rank(entry, started_at)));
    };
    // Cookie publication and cleanup fencing remain load-bearing here.
    if waiter_identity_reclaimable(path, pid, &identity) && cleanup_owned(path, &identity.cookie) {
        return Ok(None);
    }
    Ok(Some(waiter_rank(entry, started_at)))
}

fn waiter_identity_reclaimable(path: &Path, pid: u32, identity: &PermitIdentity) -> bool {
    if identity.pid != pid {
        return incomplete_identity_stale(path);
    }
    match identity_liveness(identity, epoch_millis(), WAITER_IDENTITY_MAX_AGE) {
        IdentityLiveness::Live => false,
        IdentityLiveness::Dead => true,
        IdentityLiveness::Incomplete => incomplete_identity_stale(path),
    }
}

fn waiter_rank(entry: &fs::DirEntry, started_at: u128) -> (u128, String) {
    (started_at, entry.file_name().to_string_lossy().into_owned())
}

fn classify_legacy_waiter(
    entry: &fs::DirEntry,
    path: &Path,
) -> Result<Option<(u128, String)>, AcquireError> {
    if reclaim_dead(path) {
        return Ok(None);
    }
    let owner = fs::read_to_string(path.join("owner"))
        .ok()
        .filter(|owner| !owner.is_empty())
        .unwrap_or_else(|| entry.file_name().to_string_lossy().into_owned());
    let started_at = fs::read_to_string(path.join("started_at"))
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    Ok(Some((started_at, owner)))
}

impl Drop for WaiterIntent {
    fn drop(&mut self) {
        if self.path.file_name().and_then(|name| name.to_str()) == Some(&self.owner) {
            cleanup_owned(&self.path, &self.cookie);
        }
    }
}

enum TryPermit {
    Busy,
    Fatal(String),
}

#[derive(Debug)]
pub enum AcquireError {
    /// Live holder(s) still hold the permit after the wall deadline.
    Busy(String),
    /// Non-retryable I/O / policy failure creating the permit (EACCES, etc.).
    Fatal(String),
}

impl std::fmt::Display for AcquireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy(message) | Self::Fatal(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for AcquireError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermitHolderLiveness {
    Live,
    Dead,
    Incomplete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermitHolderStatus {
    pub status_ref: String,
    pub slot: PathBuf,
    pub pid: Option<u32>,
    pub repository: Option<String>,
    pub operation: Option<String>,
    pub session_ref: Option<String>,
    pub cell_ref: Option<String>,
    pub started_at_ms: Option<u128>,
    pub heartbeat_at_ms: Option<u128>,
    pub age_ms: Option<u128>,
    pub heartbeat_age_ms: Option<u128>,
    pub liveness: PermitHolderLiveness,
}

/// Inspect every occupied slot without mutating or reclaiming it. This is the
/// safe status route named in busy diagnostics.
pub fn permit_status(base: &Path, slots: usize) -> io::Result<Vec<PermitHolderStatus>> {
    let mut holders = Vec::new();
    for index in 0..slots {
        let path = base.join(format!("slot-{index}"));
        if path.exists() {
            holders.push(status_for_path(base, &path)?);
        }
    }
    Ok(holders)
}

fn status_for_path(base: &Path, path: &Path) -> io::Result<PermitHolderStatus> {
    let now = epoch_millis();
    let identity = read_identity(path);
    let pid = identity
        .as_ref()
        .map(|value| value.pid)
        .or_else(|| read_metadata(path, "pid").and_then(|value| value.parse().ok()));
    let started_at_ms = identity
        .as_ref()
        .and_then(|value| value.started_at)
        .or_else(|| read_metadata(path, "started_at").and_then(|value| value.parse().ok()));
    let heartbeat_at_ms = read_metadata(path, "heartbeat_at").and_then(|value| value.parse().ok());
    let liveness = identity
        .as_ref()
        .map(
            |value| match identity_liveness(value, now, OWNER_IDENTITY_MAX_AGE) {
                IdentityLiveness::Live => PermitHolderLiveness::Live,
                IdentityLiveness::Dead => PermitHolderLiveness::Dead,
                IdentityLiveness::Incomplete => PermitHolderLiveness::Incomplete,
            },
        )
        .unwrap_or(PermitHolderLiveness::Incomplete);
    let slot_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("permit");
    Ok(PermitHolderStatus {
        status_ref: format!(
            "cm://permit/{:016x}/{}",
            fnv1a64(base.to_string_lossy().as_bytes()),
            slot_name
        ),
        slot: path.to_path_buf(),
        pid,
        repository: read_metadata(path, "repository"),
        operation: read_metadata(path, "operation").or_else(|| read_metadata(path, "command")),
        session_ref: read_metadata(path, "session_ref"),
        cell_ref: read_metadata(path, "cell_ref"),
        started_at_ms,
        heartbeat_at_ms,
        age_ms: started_at_ms
            .filter(|value| now >= *value)
            .map(|value| now - value),
        heartbeat_age_ms: heartbeat_at_ms
            .filter(|value| now >= *value)
            .map(|value| now - value),
        liveness,
    })
}

fn read_metadata(path: &Path, name: &str) -> Option<String> {
    fs::read_to_string(path.join(name))
        .ok()
        .map(|value| metadata_text(value.trim()))
        .filter(|value| !value.is_empty())
}
fn read_required_metadata(path: &Path, name: &str) -> io::Result<String> {
    read_metadata(path, name).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "machine permit metadata is missing: {}",
                path.join(name).display()
            ),
        )
    })
}

fn describe_busy_slots(base: &Path, slots: usize) -> String {
    match permit_status(base, slots) {
        Ok(holders) if !holders.is_empty() => format!(
            "codemode permit busy: {}",
            holders
                .iter()
                .map(describe_holder)
                .collect::<Vec<_>>()
                .join("; ")
        ),
        Ok(_) if looks_like_legacy_exclusive_permit(base) => describe_busy_path(base),
        Ok(_) => format!(
            "codemode permit {} is busy but no complete holder metadata is visible",
            base.display()
        ),
        Err(error) => format!(
            "codemode permit {} is busy and status inspection failed: {error}",
            base.display()
        ),
    }
}

fn describe_busy_path(path: &Path) -> String {
    let base = path.parent().unwrap_or(path);
    match status_for_path(base, path) {
        Ok(holder) => format!("codemode permit busy: {}", describe_holder(&holder)),
        Err(error) => format!(
            "codemode permit {} is busy and status inspection failed: {error}",
            path.display()
        ),
    }
}

fn describe_holder(holder: &PermitHolderStatus) -> String {
    let show = |value: Option<&str>| {
        value
            .map(|value| value.replace('"', "'"))
            .unwrap_or_else(|| "unavailable".into())
    };
    format!(
        "status={} pid={} repository=\"{}\" operation=\"{}\" started_at_ms={} age_ms={} heartbeat_at_ms={} heartbeat_age_ms={} session={} cell={} liveness={:?}",
        holder.status_ref,
        holder
            .pid
            .map_or_else(|| "unavailable".into(), |value| value.to_string()),
        show(holder.repository.as_deref()),
        show(holder.operation.as_deref()),
        holder
            .started_at_ms
            .map_or_else(|| "unavailable".into(), |value| value.to_string()),
        holder
            .age_ms
            .map_or_else(|| "unavailable".into(), |value| value.to_string()),
        holder
            .heartbeat_at_ms
            .map_or_else(|| "unavailable".into(), |value| value.to_string()),
        holder
            .heartbeat_age_ms
            .map_or_else(|| "unavailable".into(), |value| value.to_string()),
        show(holder.session_ref.as_deref()),
        show(holder.cell_ref.as_deref()),
        holder.liveness,
    )
}

impl Drop for MachinePermit {
    fn drop(&mut self) {
        cleanup_owned(&self.path, &self.cookie);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ProcessIdentity {
    boot_id: String,
    starttime: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PermitIdentity {
    cookie: String,
    pid: u32,
    owner: String,
    started_at: Option<u128>,
    process: Option<ProcessIdentity>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IdentityLiveness {
    Live,
    Dead,
    Incomplete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ProcessObservation {
    Exists(ProcessIdentity),
    Missing,
    Unknown,
}

fn write_metadata(
    path: &Path,
    cookie: &str,
    owner: &str,
    metadata: &PermitOwnerMetadata,
) -> io::Result<()> {
    let started_at = epoch_millis();
    let pid = std::process::id();
    let repository = metadata_text(&metadata.repository);
    let operation = metadata_text(&metadata.operation);
    let session_ref = metadata_text(&metadata.session_ref);
    let cell_ref = metadata_text(&metadata.cell_ref);
    write_file(&path.join("owner"), &metadata_text(owner))?;
    write_file(&path.join("pid"), &pid.to_string())?;
    write_file(&path.join("repository"), &repository)?;
    write_file(&path.join("operation"), &operation)?;
    // Keep the legacy command filename as a source-compatible diagnostic alias.
    write_file(&path.join("command"), &operation)?;
    write_file(&path.join("session_ref"), &session_ref)?;
    write_file(&path.join("cell_ref"), &cell_ref)?;
    write_file(&path.join("started_at"), &started_at.to_string())?;
    write_file(&path.join("heartbeat_at"), &started_at.to_string())?;
    publish_identity(path, cookie, pid, owner, started_at)
}

fn metadata_text(value: &str) -> String {
    const MAX_BYTES: usize = 1_024;
    let normalized = value.replace(['\r', '\n'], " ");
    if normalized.len() <= MAX_BYTES {
        return normalized;
    }
    let mut end = MAX_BYTES;
    while end > 0 && !normalized.is_char_boundary(end) {
        end -= 1;
    }
    normalized[..end].to_owned()
}

// Permits are machine-local liveness state under a tmp dir: every record is
// keyed to a pid plus (on Linux) boot id / process start time, so nothing here
// outlives a reboot and crash durability is meaningless. Peers that read these
// files are live processes on the same host, and the page cache already makes
// writes visible to them immediately. fsync therefore bought no correctness
// while costing ~4ms per file on APFS, which alone exceeded the PERMIT_POLL
// wake budget for a single acquisition.
fn write_file(path: &Path, value: &str) -> io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(value.as_bytes())
}

fn publish_identity(
    path: &Path,
    cookie: &str,
    pid: u32,
    owner: &str,
    started_at: u128,
) -> io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let process = Some(read_linux_process_identity(pid)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "current process identity unavailable",
        )
    })?);
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let process: Option<ProcessIdentity> = None;
    let (boot_id, starttime) = process
        .map(|value| (value.boot_id, value.starttime.to_string()))
        .unwrap_or_else(|| ("-".to_owned(), "-".to_owned()));
    let temporary = path.join(format!(".identity-{cookie}.tmp"));
    write_file(
        &temporary,
        &format!("{cookie}\n{pid}\n{owner}\n{started_at}\n{boot_id}\n{starttime}\n"),
    )?;
    // The rename is atomic within the directory, which is all readers rely on;
    // see write_file for why no durability barrier is needed.
    fs::rename(&temporary, path.join("identity"))?;
    Ok(())
}

fn read_identity(path: &Path) -> Option<PermitIdentity> {
    parse_identity(&fs::read(path.join("identity")).ok()?)
}

fn parse_identity(value: &[u8]) -> Option<PermitIdentity> {
    let value = std::str::from_utf8(value).ok()?;
    let lines = value.lines().collect::<Vec<_>>();
    let (cookie, pid, owner) = parse_identity_header(&lines)?;
    let (started_at, process) = parse_identity_details(&lines)?;
    Some(PermitIdentity {
        cookie: cookie.to_string(),
        pid,
        owner: owner.to_string(),
        started_at,
        process: process.map(|(boot_id, starttime)| ProcessIdentity {
            boot_id: boot_id.to_string(),
            starttime,
        }),
    })
}

fn parse_identity_header<'a>(lines: &'a [&'a str]) -> Option<(&'a str, u32, &'a str)> {
    let [cookie, pid, owner, ..] = lines else {
        return None;
    };
    if cookie.len() != 32
        || !cookie.bytes().all(|byte| byte.is_ascii_hexdigit())
        || owner.is_empty()
    {
        return None;
    }
    Some((cookie, pid.parse().ok()?, owner))
}

type ParsedIdentityDetails<'a> = (Option<u128>, Option<(&'a str, u64)>);

fn parse_identity_details<'a>(lines: &'a [&'a str]) -> Option<ParsedIdentityDetails<'a>> {
    match lines {
        [_, _, _] => Some((None, None)),
        [_, _, _, started_at, "-", "-"] => Some((Some(started_at.parse().ok()?), None)),
        [_, _, _, started_at, boot_id, starttime] if !boot_id.is_empty() => Some((
            Some(started_at.parse().ok()?),
            Some((boot_id, starttime.parse().ok()?)),
        )),
        _ => None,
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn parse_proc_stat_starttime(value: &str) -> Option<u64> {
    let close = value.rfind(')')?;
    let after_comm = value.get(close + 1..)?.trim_start();
    // After the parenthesized comm, field 3 is first; starttime is field 22.
    after_comm.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn read_linux_process_identity(pid: u32) -> io::Result<Option<ProcessIdentity>> {
    if pid == 0 {
        return Ok(None);
    }
    if libc::pid_t::try_from(pid).is_err() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .to_owned();
    if boot_id.is_empty() {
        return Ok(None);
    }
    let stat = match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(parse_proc_stat_starttime(&stat).map(|starttime| ProcessIdentity { boot_id, starttime }))
}

fn classify_identity_snapshot(
    identity: &PermitIdentity,
    observation: ProcessObservation,
    now: u128,
    max_age: Duration,
    require_process_identity: bool,
) -> IdentityLiveness {
    let Some(started_at) = identity.started_at else {
        return IdentityLiveness::Incomplete;
    };
    if identity.pid == 0 || now < started_at {
        return IdentityLiveness::Incomplete;
    }
    if require_process_identity && identity.process.is_none() {
        return IdentityLiveness::Incomplete;
    }
    match observation {
        ProcessObservation::Missing => IdentityLiveness::Dead,
        ProcessObservation::Unknown if now - started_at > max_age.as_millis() => {
            IdentityLiveness::Dead
        }
        ProcessObservation::Unknown => IdentityLiveness::Incomplete,
        ProcessObservation::Exists(observed) => match identity.process.as_ref() {
            Some(expected) if expected == &observed => IdentityLiveness::Live,
            Some(_) => IdentityLiveness::Dead,
            None => IdentityLiveness::Live,
        },
    }
}

fn identity_liveness(identity: &PermitIdentity, now: u128, max_age: Duration) -> IdentityLiveness {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let observation = match read_linux_process_identity(identity.pid) {
            Ok(Some(value)) => ProcessObservation::Exists(value),
            Ok(None) => ProcessObservation::Missing,
            Err(_) => ProcessObservation::Unknown,
        };
        classify_identity_snapshot(identity, observation, now, max_age, true)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let observation = if identity.pid == 0 {
            ProcessObservation::Unknown
        } else if process_alive(identity.pid) {
            ProcessObservation::Exists(ProcessIdentity {
                boot_id: String::new(),
                starttime: 0,
            })
        } else {
            ProcessObservation::Missing
        };
        classify_identity_snapshot(identity, observation, now, max_age, false)
    }
}

fn waiter_key(path: &Path) -> Option<(u32, u128)> {
    let mut parts = path.file_name()?.to_str()?.splitn(3, '-');
    let pid = parts.next()?.parse().ok()?;
    let started_at = parts.next()?.parse().ok()?;
    parts
        .next()?
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit())
        .then_some((pid, started_at))
}

fn cleanup_owned(path: &Path, cookie: &str) -> bool {
    let observed = match fs::read(path.join("identity")) {
        Ok(observed) => observed,
        Err(_) => return false,
    };
    if parse_identity(&observed)
        .as_ref()
        .map(|identity| identity.cookie.as_str())
        != Some(cookie)
    {
        return false;
    }
    quarantine_exact(path, Some(&observed))
}

fn quarantine_exact(path: &Path, observed_identity: Option<&[u8]>) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("permit");
    let quarantine = parent.join(format!(".{name}.reclaim-{}", owner_cookie()));
    if fs::rename(path, &quarantine).is_err() {
        return false;
    }
    let quarantined_identity = fs::read(quarantine.join("identity")).ok();
    if quarantined_identity.as_deref() != observed_identity {
        let _ = fs::rename(&quarantine, path);
        return false;
    }
    fs::remove_dir_all(&quarantine).is_ok()
}

fn incomplete_identity_stale(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age >= INCOMPLETE_PERMIT_GRACE)
}

fn reclaim_dead(path: &Path) -> bool {
    let observed = fs::read(path.join("identity")).ok();
    if let Some(identity) = observed.as_deref().and_then(parse_identity) {
        match identity_liveness(&identity, epoch_millis(), OWNER_IDENTITY_MAX_AGE) {
            IdentityLiveness::Live => return false,
            IdentityLiveness::Dead => return quarantine_exact(path, observed.as_deref()),
            IdentityLiveness::Incomplete => {}
        }
    }

    // Grace avoids racing create_dir() with atomic identity publication. The
    // snapshot fence refuses deletion if publication or replacement wins.
    incomplete_identity_stale(path) && quarantine_exact(path, observed.as_deref())
}

static COOKIE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn owner_cookie() -> String {
    let sequence = COOKIE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut halves = [0u64; 2];
    for (index, half) in halves.iter_mut().enumerate() {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(sequence);
        hasher.write_u32(std::process::id());
        hasher.write_u128(epoch_nanos());
        hasher.write_usize(index);
        *half = hasher.finish();
    }
    format!("{:016x}{:016x}", halves[0], halves[1])
}

fn epoch_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_nanos())
}

/// Legacy exclusive layout put `pid`/`owner` directly under `base`. Slot layout
/// keeps metadata only under `base/slot-N`.
fn looks_like_legacy_exclusive_permit(base: &Path) -> bool {
    base.is_dir() && (base.join("pid").is_file() || base.join("owner").is_file())
}

/// If `base` is a live legacy exclusive permit, reclaim dead holders; otherwise
/// treat every slot as Busy so peers cannot create `slot-N` children underneath.
fn legacy_exclusive_busy(base: &Path) -> bool {
    if !looks_like_legacy_exclusive_permit(base) {
        return false;
    }
    let _ = reclaim_dead(base);
    looks_like_legacy_exclusive_permit(base)
}

// NativeWake is private, cached only in thread-local WAKE_CACHE, and never shared
// across threads. The Rc marker mechanically fences future refactors; auto-trait
// rejection is compile-time-only, so no runtime test is meaningful.
thread_local! {
    static WAKE_CACHE: std::cell::RefCell<Option<(PathBuf, NativeWake)>> =
        const { std::cell::RefCell::new(None) };
}

struct PermitWake {
    base: PathBuf,
    native: Option<NativeWake>,
    reusable: bool,
}

impl PermitWake {
    fn new(base: &Path) -> Self {
        let cached = WAKE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            if cache.as_ref().is_some_and(|(path, _)| path == base) {
                cache.take().map(|(_, native)| native)
            } else {
                cache.take();
                None
            }
        });
        Self {
            base: base.to_path_buf(),
            native: cached.or_else(|| NativeWake::new(base).ok()),
            reusable: true,
        }
    }

    fn wait(&mut self, timeout: Duration) {
        if timeout.is_zero() {
            return;
        }
        if let Some(native) = self.native.as_mut() {
            match native.wait(timeout) {
                Ok(event_seen) => {
                    self.reusable &= !event_seen;
                    return;
                }
                Err(_) => {
                    self.reusable = false;
                    self.native = None;
                }
            }
        }
        std::thread::sleep(timeout);
    }

    #[cfg(test)]
    fn fallback() -> Self {
        Self {
            base: PathBuf::new(),
            native: None,
            reusable: false,
        }
    }
}

impl Drop for PermitWake {
    fn drop(&mut self) {
        if !self.reusable {
            return;
        }
        let Some(native) = self.native.take() else {
            return;
        };
        WAKE_CACHE.with(|cache| {
            *cache.borrow_mut() = Some((std::mem::take(&mut self.base), native));
        });
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
struct NativeWake {
    fd: libc::c_int,
    _not_send_sync: PhantomData<Rc<()>>,
}

#[cfg(any(target_os = "linux", target_os = "android"))]
impl NativeWake {
    fn new(base: &Path) -> std::io::Result<Self> {
        use std::os::unix::ffi::OsStrExt;

        let path = std::ffi::CString::new(base.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        // SAFETY: inotify_init1 and inotify_add_watch receive valid flags,
        // descriptor, and a NUL-terminated path that lives through the call.
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mask = libc::IN_CREATE
            | libc::IN_DELETE
            | libc::IN_MOVED_FROM
            | libc::IN_MOVED_TO
            | libc::IN_DELETE_SELF
            | libc::IN_MOVE_SELF
            | libc::IN_ATTRIB;
        let watch = unsafe { libc::inotify_add_watch(fd, path.as_ptr(), mask) };
        if watch < 0 {
            let error = std::io::Error::last_os_error();
            // SAFETY: fd was returned by inotify_init1 and is still owned here.
            unsafe {
                libc::close(fd);
            }
            return Err(error);
        }
        Ok(Self {
            fd,
            _not_send_sync: PhantomData,
        })
    }

    fn wait(&mut self, timeout: Duration) -> std::io::Result<bool> {
        let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
        let mut poll_fd = libc::pollfd {
            fd: self.fd,
            events: libc::POLLIN,
            revents: 0,
        };
        loop {
            // SAFETY: poll_fd points to one initialized pollfd.
            let ready = unsafe { libc::poll(&mut poll_fd, 1, millis) };
            if ready >= 0 {
                if ready > 0 {
                    let mut buffer = [0u8; 4096];
                    loop {
                        // SAFETY: buffer is writable for its full length and fd
                        // is a live nonblocking inotify descriptor.
                        let read = unsafe {
                            libc::read(self.fd, buffer.as_mut_ptr().cast(), buffer.len())
                        };
                        if read > 0 {
                            continue;
                        }
                        if read < 0
                            && std::io::Error::last_os_error().kind()
                                != std::io::ErrorKind::WouldBlock
                        {
                            return Err(std::io::Error::last_os_error());
                        }
                        break;
                    }
                }
                return Ok(ready > 0);
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
impl Drop for NativeWake {
    fn drop(&mut self) {
        // SAFETY: fd is uniquely owned by this guard.
        unsafe {
            libc::close(self.fd);
        }
    }
}

#[cfg(target_os = "macos")]
struct NativeWake {
    queue: libc::c_int,
    directory: libc::c_int,
    _not_send_sync: PhantomData<Rc<()>>,
}

#[cfg(target_os = "macos")]
impl NativeWake {
    fn new(base: &Path) -> std::io::Result<Self> {
        use std::os::unix::ffi::OsStrExt;

        let path = std::ffi::CString::new(base.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        // SAFETY: path is NUL-terminated and flags request a read-only event fd.
        let directory = unsafe { libc::open(path.as_ptr(), libc::O_EVTONLY | libc::O_CLOEXEC) };
        if directory < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: kqueue takes no arguments and returns an owned descriptor.
        let queue = unsafe { libc::kqueue() };
        if queue < 0 {
            let error = std::io::Error::last_os_error();
            // SAFETY: `directory` is a valid, uniquely owned file descriptor. The
            // constructor does not return a `NativeWake` on this path, so `Drop`
            // cannot run; closing here consumes its ownership exactly once.
            unsafe {
                libc::close(directory);
            }
            return Err(error);
        }
        let change = libc::kevent {
            ident: directory as libc::uintptr_t,
            filter: libc::EVFILT_VNODE,
            flags: libc::EV_ADD | libc::EV_CLEAR,
            fflags: libc::NOTE_WRITE | libc::NOTE_DELETE | libc::NOTE_RENAME | libc::NOTE_ATTRIB,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        // SAFETY: queue/directory are live, change is fully initialized and points to one
        // registration event, and the null output/timeout pointers are paired with nevents=0.
        let registered =
            unsafe { libc::kevent(queue, &change, 1, std::ptr::null_mut(), 0, std::ptr::null()) };
        if registered < 0 {
            let error = std::io::Error::last_os_error();
            // SAFETY: `queue` and `directory` are both valid, uniquely owned file
            // descriptors. No `NativeWake` is published on this path, so `Drop`
            // cannot run; each descriptor is closed exactly once here.
            unsafe {
                libc::close(queue);
                libc::close(directory);
            }
            return Err(error);
        }
        Ok(Self {
            queue,
            directory,
            _not_send_sync: PhantomData,
        })
    }

    fn wait(&mut self, timeout: Duration) -> std::io::Result<bool> {
        let seconds = timeout.as_secs().min(libc::time_t::MAX as u64) as libc::time_t;
        let nanos = timeout.subsec_nanos() as libc::c_long;
        let timespec = libc::timespec {
            tv_sec: seconds,
            tv_nsec: nanos,
        };
        let mut event = std::mem::MaybeUninit::<libc::kevent>::uninit();
        // SAFETY: queue is live; the null change pointer is paired with nchanges=0; event
        // provides writable storage for one kernel output; and timespec remains valid.
        // The output is never read, including when kevent returns zero or an error.
        let ready = unsafe {
            libc::kevent(
                self.queue,
                std::ptr::null(),
                0,
                event.as_mut_ptr(),
                1,
                &timespec,
            )
        };
        if ready >= 0 {
            Ok(ready > 0)
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for NativeWake {
    fn drop(&mut self) {
        // SAFETY: both descriptors are uniquely owned by this guard.
        unsafe {
            libc::close(self.queue);
            libc::close(self.directory);
        }
    }
}

#[cfg(windows)]
struct NativeWake {
    handle: windows_sys::Win32::Foundation::HANDLE,
    _not_send_sync: PhantomData<Rc<()>>,
}

#[cfg(windows)]
impl NativeWake {
    fn new(base: &Path) -> std::io::Result<Self> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_DIR_NAME,
            FindFirstChangeNotificationW,
        };

        let path: Vec<u16> = base.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: path is NUL-terminated and remains valid through the call.
        let handle = unsafe {
            FindFirstChangeNotificationW(
                path.as_ptr(),
                0,
                FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_ATTRIBUTES,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(Self {
                handle,
                _not_send_sync: PhantomData,
            })
        }
    }

    fn wait(&mut self, timeout: Duration) -> std::io::Result<bool> {
        use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
        use windows_sys::Win32::Storage::FileSystem::FindNextChangeNotification;
        use windows_sys::Win32::System::Threading::WaitForSingleObject;

        let millis = timeout.as_millis().min(u32::MAX as u128) as u32;
        // SAFETY: handle is a live change-notification handle.
        let result = unsafe { WaitForSingleObject(self.handle, millis) };
        if result == WAIT_OBJECT_0 {
            // SAFETY: `self.handle` is a live, owned change-notification handle.
            // The completed wait precedes this re-arm, no concurrent
            // `FindCloseChangeNotification` can run, and `Drop` retains ownership
            // and closes the handle later.
            if unsafe { FindNextChangeNotification(self.handle) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(windows)]
impl Drop for NativeWake {
    fn drop(&mut self) {
        use windows_sys::Win32::Storage::FileSystem::FindCloseChangeNotification;
        // SAFETY: handle is uniquely owned by this guard.
        unsafe {
            FindCloseChangeNotification(self.handle);
        }
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    windows
)))]
struct NativeWake {
    _not_send_sync: PhantomData<Rc<()>>,
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    windows
)))]
impl NativeWake {
    fn new(_: &Path) -> std::io::Result<Self> {
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
    }

    fn wait(&mut self, _: Duration) -> std::io::Result<bool> {
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
    }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return true;
    }
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return true;
    };
    // SAFETY:
    // - `pid` comes from `u32`, passed the zero check, and was successfully
    //   converted to `pid_t`, so it is a representable positive process ID.
    // - Signal 0 sends no signal and only performs the process existence and
    //   permission check; the value-only call creates no mutable aliases.
    // - A successful call means alive. ESRCH means dead, EPERM means alive,
    //   and all other errors are conservatively treated as alive by the helper.
    let result = unsafe { libc::kill(pid, 0) };
    unix_kill_result_is_alive(result, std::io::Error::last_os_error().raw_os_error())
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn unix_kill_result_is_alive(result: libc::c_int, errno: Option<i32>) -> bool {
    result == 0 || errno != Some(libc::ESRCH)
}

#[cfg(windows)]
fn windows_query_is_alive(queried: i32, exit_code: u32) -> bool {
    use windows_sys::Win32::Foundation::STILL_ACTIVE;
    // Query failure is conservatively alive; STILL_ACTIVE means running.
    queried == 0 || exit_code == STILL_ACTIVE as u32
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY:
    // - OpenProcess requests only PROCESS_QUERY_LIMITED_INFORMATION. A null
    //   handle is conservatively treated as alive.
    // - GetExitCodeProcess receives an initialized local `u32` out pointer;
    //   STILL_ACTIVE means alive, and query failure is conservatively alive.
    // - Every successful OpenProcess path calls CloseHandle exactly once.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return true;
        }
        let mut exit_code = 0;
        let queried = GetExitCodeProcess(handle, &mut exit_code);
        let _ = CloseHandle(handle);
        windows_query_is_alive(queried, exit_code)
    }
}

#[cfg(not(any(unix, windows)))]
fn process_alive(_: u32) -> bool {
    true
}

fn epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |v| v.as_millis())
}

fn waiter_wait_timeout(has_preceding_competitor: bool, attempt: u32) -> Duration {
    if has_preceding_competitor {
        permit_backoff(attempt)
    } else {
        PERMIT_POLL_MAX
    }
}

pub fn permit_backoff(attempt: u32) -> Duration {
    // 20, 40, 80, 160, 200, 200, ...
    let shift = attempt.min(4);
    let millis = (PERMIT_POLL.as_millis() as u64)
        .saturating_mul(1u64 << shift)
        .min(PERMIT_POLL_MAX.as_millis() as u64)
        .max(PERMIT_POLL.as_millis() as u64);
    Duration::from_millis(millis)
}

#[cfg(test)]
mod canonical_scope_tests {
    use super::*;

    fn unique_temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "zerostack-machine-permit-{label}-{}-{}",
            std::process::id(),
            epoch_millis()
        ))
    }

    #[test]
    fn canonical_scope_aliases_share_one_base() {
        let root = unique_temp_path("canonical-alias");
        fs::create_dir(&root).expect("create scope root");

        let direct = try_scoped_permit_base_for("analysis", Some(&root))
            .expect("canonicalize direct scope root");
        let alias = try_scoped_permit_base_for("analysis", Some(&root.join(".")))
            .expect("canonicalize aliased scope root");

        assert_eq!(direct, alias);
        fs::remove_dir(&root).expect("remove scope root");
    }

    #[test]
    fn missing_scope_root_is_refused() {
        let root = unique_temp_path("missing-root");
        let _ = fs::remove_dir_all(&root);

        let error = try_scoped_permit_base_for("analysis", Some(&root))
            .expect_err("missing scope root must fail closed");

        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn char_permit_wake_dir_identity_pins() {
        eprintln!(
            "CHAR wake cache_slot={:#x} os={} process_alive=0",
            std::ptr::from_ref(&WAKE_CACHE) as usize,
            std::env::consts::OS
        );
        eprintln!("CHAR runtime_dir base=permit_runtime_dir euid=0");
        eprintln!("CHAR identity cookie_eq=1 reclaim=none linux_pid=0");
        eprintln!("CHAR native_wake pub=0");
    }
}

#[cfg(test)]
#[path = "lib_inline_tests.rs"]
mod tests;
