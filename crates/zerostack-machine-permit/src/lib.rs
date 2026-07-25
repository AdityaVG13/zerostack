//! Machine-wide CodeMode permit (slot layout, reclaim, backoff).
//!
//! Shared contract for TokenZero / FSZero / GraphZero: directory-based locks
//! under `/tmp/zerostack-codemode-*.permit` with `slot-N` children. Live holders
//! block peers until wall deadline (retryable busy); dead / incomplete dirs are
//! reclaimed. Fatal I/O (EACCES, etc.) stays non-retryable.
//!
//! Canonical policy: `tokenzero-mcp/CODEMODE_MACHINE_PERMITS.md`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const PERMIT_POLL: Duration = Duration::from_millis(20);
pub const PERMIT_POLL_MAX: Duration = Duration::from_millis(200);
const INCOMPLETE_PERMIT_GRACE: Duration = Duration::from_millis(250);


/// Repo-scoped permit base: `/tmp/zerostack-codemode-<class>-<hash16>.permit`.
///
/// Scope comes from `ZEROSTACK_PERMIT_SCOPE_ROOT` or the per-child root envs
/// the CodeMode hub already sets (`FSZERO_ROOT`, `TOKENZERO_ROOT`,
/// `GZ_REPO_ROOT`), so concurrent repos stop serializing through one
/// machine-global slot. Without a scope (bare CLI), fall back to the legacy
/// machine-global base so unrelated processes keep excluding each other.
pub fn scoped_permit_base(class: &str) -> PathBuf {
    scoped_permit_base_for(class, permit_scope_root().as_deref())
}

pub fn scoped_permit_base_for(class: &str, scope_root: Option<&Path>) -> PathBuf {
    let Some(root) = scope_root else {
        return PathBuf::from(format!("/tmp/zerostack-codemode-{class}.permit"));
    };
    let canonical = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let scope = fnv1a64(canonical.to_string_lossy().as_bytes());
    PathBuf::from(format!("/tmp/zerostack-codemode-{class}-{scope:016x}.permit"))
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
pub struct MachinePermit(PathBuf, String);

impl MachinePermit {
    /// Path of the held permit directory (`base/slot-N` or legacy exclusive).
    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn acquire_slots(
        base: &Path,
        slots: usize,
        deadline: Instant,
        command: &str,
    ) -> Result<Self, AcquireError> {
        Self::acquire_slots_with_wake(base, slots, deadline, command, PermitWake::new)
    }

    fn acquire_slots_with_wake(
        base: &Path,
        slots: usize,
        deadline: Instant,
        command: &str,
        make_wake: impl FnOnce(&Path) -> PermitWake,
    ) -> Result<Self, AcquireError> {
        // Always use base/slot-N — even when slots==1 — so mixed concurrency
        // envs cannot stack an exclusive base lock with slot children.
        // Pool size is the caller's requested budget (from env); do not freeze
        // capacity to the first asker — that would let CONCURRENCY=1 starve the
        // family-wide cores/4 analysis budget.
        let waiter = WaiterIntent::create(base)?;
        let mut wake = make_wake(base);
        let mut attempt = 0u32;
        loop {
            // Events wake the FIFO head immediately. The timeout is only a
            // lost-event safety net; younger waiters retain exponential
            // backoff so one directory event cannot cause an N-way scan storm.
            let has_preceding = waiter.has_preceding_competitor()?;
            if !has_preceding && !legacy_exclusive_busy(base) {
                let _ = fs::create_dir_all(base);
                for idx in 0..slots {
                    let path = base.join(format!("slot-{idx}"));
                    match Self::try_create(&path, command) {
                        Ok(permit) => return Ok(permit),
                        Err(TryPermit::Busy) => {}
                        Err(TryPermit::Fatal(e)) => return Err(AcquireError::Fatal(e)),
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err(AcquireError::Busy(format!(
                    "codemode permit {} is held by live process(es) across {slots} slots",
                    base.display()
                )));
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
    pub fn acquire(
        path: &Path,
        deadline: Instant,
        command: &str,
    ) -> Result<Self, AcquireError> {
        let mut attempt = 0u32;
        loop {
            match Self::try_create(path, command) {
                Ok(permit) => return Ok(permit),
                Err(TryPermit::Busy) => {
                    if Instant::now() >= deadline {
                        return Err(AcquireError::Busy(format!(
                            "codemode permit {} is held by a live process",
                            path.display()
                        )));
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
        match fs::create_dir(path) {
            Ok(()) => {
                let owner = format!(
                    "{}-{}-{:?}",
                    std::process::id(),
                    epoch_millis(),
                    std::thread::current().id()
                );
                if let Err(e) = write_metadata(path, &owner, command) {
                    cleanup_owned(path, &owner);
                    return Err(TryPermit::Fatal(format!(
                        "write codemode permit metadata: {e}"
                    )));
                }
                Ok(Self(path.to_path_buf(), owner))
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if reclaim_dead(path) {
                    return Self::try_create(path, command);
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
        let owner = format!(
            "{}-{started_at}-{:?}",
            std::process::id(),
            std::thread::current().id()
        );
        let path = waiters.join(&owner);
        fs::create_dir(&path).map_err(|e| {
            AcquireError::Fatal(format!(
                "create codemode permit waiter {}: {e}",
                path.display()
            ))
        })?;
        Ok(Self {
            path,
            owner,
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
        let waiters = self
            .path
            .parent()
            .expect("waiter intent always has a parent");
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
            let path = entry.path();
            if path == self.path {
                continue;
            }
            if !entry
                .file_type()
                .map_err(|e| {
                    AcquireError::Fatal(format!(
                        "inspect codemode permit waiter {}: {e}",
                        path.display()
                    ))
                })?
                .is_dir()
            {
                return Err(AcquireError::Fatal(format!(
                    "codemode permit waiter is not a directory: {}",
                    path.display()
                )));
            }
            if let Some((pid, started_at)) = waiter_key(&path) {
                if !process_alive(pid) && remove_waiter(&path) {
                    continue;
                }
                live.push((
                    started_at,
                    entry.file_name().to_string_lossy().into_owned(),
                ));
                continue;
            }
            if reclaim_dead(&path) {
                continue;
            }
            let owner = fs::read_to_string(path.join("owner"))
                .ok()
                .filter(|owner| !owner.is_empty())
                .unwrap_or_else(|| entry.file_name().to_string_lossy().into_owned());
            let started_at = fs::read_to_string(path.join("started_at"))
                .ok()
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(0);
            live.push((started_at, owner));
        }
        Ok(live)
    }
}

impl Drop for WaiterIntent {
    fn drop(&mut self) {
        if self.path.file_name().and_then(|name| name.to_str()) == Some(&self.owner) {
            remove_waiter(&self.path);
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

impl Drop for MachinePermit {
    fn drop(&mut self) {
        cleanup_owned(&self.0, &self.1);
    }
}

fn write_metadata(path: &Path, owner: &str, command: &str) -> std::io::Result<()> {
    // Write ownership first so an error in any later metadata write remains
    // removable by the acquiring RAII guard.
    fs::write(path.join("owner"), owner)?;
    fs::write(path.join("pid"), std::process::id().to_string())?;
    fs::write(
        path.join("repository"),
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .to_string_lossy()
            .chars()
            .take(1024)
            .collect::<String>(),
    )?;
    fs::write(path.join("command"), command)?;
    fs::write(path.join("started_at"), epoch_millis().to_string())
}

fn waiter_key(path: &Path) -> Option<(u32, u128)> {
    let mut parts = path.file_name()?.to_str()?.splitn(3, '-');
    let pid = parts.next()?.parse().ok()?;
    let started_at = parts.next()?.parse().ok()?;
    parts.next()?;
    Some((pid, started_at))
}

fn remove_waiter(path: &Path) -> bool {
    match fs::remove_dir(path) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
            remove_permit(path)
        }
        Err(_) => false,
    }
}

const PERMIT_METADATA: &[&str] = &["pid", "repository", "command", "started_at", "owner"];

fn remove_permit(path: &Path) -> bool {
    for name in PERMIT_METADATA {
        let _ = fs::remove_file(path.join(name));
    }
    fs::remove_dir(path).is_ok()
}

fn cleanup_owned(path: &Path, owner: &str) {
    if fs::read_to_string(path.join("owner")).ok().as_deref() == Some(owner) {
        remove_permit(path);
    }
}

fn reclaim_dead(path: &Path) -> bool {
    let pid = fs::read_to_string(path.join("pid"))
        .ok()
        .and_then(|pid| pid.trim().parse::<u32>().ok());
    if let Some(pid) = pid {
        return !process_alive(pid) && remove_permit(path);
    }

    // A process can die after create_dir() but before writing pid. Without a
    // bounded incomplete-state recovery, that empty permit blocks every
    // CodeMode client forever. The grace period avoids racing a live writer.
    let stale = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age >= INCOMPLETE_PERMIT_GRACE);
    stale && remove_permit(path)
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
        Ok(Self { fd })
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
                            libc::read(
                                self.fd,
                                buffer.as_mut_ptr().cast(),
                                buffer.len(),
                            )
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
            unsafe {
                libc::close(directory);
            }
            return Err(error);
        }
        // SAFETY: zero is a valid initial representation for kevent before all
        // required fields are assigned below.
        let mut change: libc::kevent = unsafe { std::mem::zeroed() };
        change.ident = directory as libc::uintptr_t;
        change.filter = libc::EVFILT_VNODE;
        change.flags = libc::EV_ADD | libc::EV_CLEAR;
        change.fflags =
            libc::NOTE_WRITE | libc::NOTE_DELETE | libc::NOTE_RENAME | libc::NOTE_ATTRIB;
        // SAFETY: queue/directory are live and change points to one event.
        let registered =
            unsafe { libc::kevent(queue, &change, 1, std::ptr::null_mut(), 0, std::ptr::null()) };
        if registered < 0 {
            let error = std::io::Error::last_os_error();
            unsafe {
                libc::close(queue);
                libc::close(directory);
            }
            return Err(error);
        }
        Ok(Self { queue, directory })
    }

    fn wait(&mut self, timeout: Duration) -> std::io::Result<bool> {
        let seconds = timeout.as_secs().min(libc::time_t::MAX as u64) as libc::time_t;
        let nanos = timeout.subsec_nanos() as libc::c_long;
        let timespec = libc::timespec {
            tv_sec: seconds,
            tv_nsec: nanos,
        };
        // SAFETY: queue is live, event points to writable storage, and timespec
        // remains valid for the duration of kevent.
        let mut event: libc::kevent = unsafe { std::mem::zeroed() };
        let ready = unsafe {
            libc::kevent(
                self.queue,
                std::ptr::null(),
                0,
                &mut event,
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
}

#[cfg(windows)]
impl NativeWake {
    fn new(base: &Path) -> std::io::Result<Self> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::Storage::FileSystem::{
            FindFirstChangeNotificationW, FILE_NOTIFY_CHANGE_ATTRIBUTES,
            FILE_NOTIFY_CHANGE_DIR_NAME,
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
            Ok(Self { handle })
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

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos", windows)))]
struct NativeWake;

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos", windows)))]
impl NativeWake {
    fn new(_: &Path) -> std::io::Result<Self> {
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
    }

    fn wait(&mut self, _: Duration) -> std::io::Result<bool> {
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return true;
    }
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return true;
    };
    // SAFETY: kill(pid, 0) sends no signal and only queries whether the PID
    // exists and is signalable. `pid` is a validated positive process ID.
    let result = unsafe { libc::kill(pid, 0) };
    unix_kill_result_is_alive(
        result,
        std::io::Error::last_os_error().raw_os_error(),
    )
}

#[cfg(unix)]
fn unix_kill_result_is_alive(result: libc::c_int, errno: Option<i32>) -> bool {
    result == 0 || errno != Some(libc::ESRCH)
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: the handle is closed on every successful OpenProcess path, and
    // GetExitCodeProcess writes to a valid local `u32`.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return true;
        }
        let mut exit_code = 0;
        let queried = GetExitCodeProcess(handle, &mut exit_code);
        let _ = CloseHandle(handle);
        queried == 0 || exit_code == STILL_ACTIVE as u32
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
#[path = "lib_inline_tests.rs"]
mod tests;
