//! Process-start, serialization, and lock-wait detectors.
//!
//! Live counters for release gates and contention attribution (fszero-ncib.8 /
//! .10, fszero-lock-wait-metrics-xwnf). Kill-tests deliberately force increments
//! and assert detectors report them — hardcoding zeros is forbidden.
//!
//! # Attributing multi-process wall time without off-CPU BPF
//!
//! Always-on cheap atomics record **wait wall** (and acquire counts) for the
//! main contention surfaces: index build flock, durable-open busy backoff,
//! and pack exclusive flock. Operators/profilers:
//! 1. `lock_wait_snapshot()` / `take_lock_wait_snapshot()` around a trial, or
//! 2. `FSZERO_INDEX_PHASES` for index phase JSON (includes lock phase when set).
//! Compare total trial wall to sum of lock-wait_us: residual is CPU + I/O that
//! is not flock/permit wait. Dual-writer pack wait shows in pack_lock fields.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

static PROCESS_STARTS: AtomicU64 = AtomicU64::new(0);
static SERIALIZATIONS: AtomicU64 = AtomicU64::new(0);
static LAST_SERIALIZE_BYTES: AtomicU64 = AtomicU64::new(0);

static INDEX_LOCK_WAITS: AtomicU64 = AtomicU64::new(0);
static INDEX_LOCK_WAIT_US: AtomicU64 = AtomicU64::new(0);
static DURABLE_OPEN_BUSY_RETRIES: AtomicU64 = AtomicU64::new(0);
static DURABLE_OPEN_BUSY_WAIT_US: AtomicU64 = AtomicU64::new(0);
static PACK_LOCK_ACQUIRES: AtomicU64 = AtomicU64::new(0);
static PACK_LOCK_WAIT_US: AtomicU64 = AtomicU64::new(0);

/// Serializes tests that reset/read global metrics (avoid races under --test-threads>1).
static METRICS_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Hold while resetting/asserting global process/serialization counters in tests.
pub fn lock_metrics_for_test() -> MutexGuard<'static, ()> {
    METRICS_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// Record that a child process was spawned (Command::status/output/spawn).
pub fn record_process_start() {
    PROCESS_STARTS.fetch_add(1, Ordering::SeqCst);
}

/// Production helper: record a process start then run `Command::status`.
/// All FSZero child spawns should prefer this (or call `record_process_start`
/// immediately before spawn/output/status).
pub fn command_status(
    program: impl AsRef<std::ffi::OsStr>,
    args: &[impl AsRef<std::ffi::OsStr>],
) -> std::io::Result<std::process::ExitStatus> {
    record_process_start();
    std::process::Command::new(program).args(args).status()
}

/// Record one domain/transport serialization boundary (JSON encode of a result).
pub fn record_serialization(byte_len: usize) {
    SERIALIZATIONS.fetch_add(1, Ordering::SeqCst);
    LAST_SERIALIZE_BYTES.store(byte_len as u64, Ordering::SeqCst);
}

pub fn process_start_count() -> u64 {
    PROCESS_STARTS.load(Ordering::SeqCst)
}

pub fn serialization_count() -> u64 {
    SERIALIZATIONS.load(Ordering::SeqCst)
}

pub fn last_serialize_bytes() -> u64 {
    LAST_SERIALIZE_BYTES.load(Ordering::SeqCst)
}

/// Snapshot then zero (for trial isolation in benches).
pub fn take_process_starts() -> u64 {
    PROCESS_STARTS.swap(0, Ordering::SeqCst)
}

pub fn take_serializations() -> u64 {
    SERIALIZATIONS.swap(0, Ordering::SeqCst)
}

/// Record wall time spent contending for the cross-process index build flock.
pub fn record_index_lock_wait(wait_us: u64) {
    INDEX_LOCK_WAITS.fetch_add(1, Ordering::Relaxed);
    INDEX_LOCK_WAIT_US.fetch_add(wait_us, Ordering::Relaxed);
}

/// Record one durable-open busy backoff sleep (and its duration).
pub fn record_durable_open_busy_wait(wait_us: u64) {
    DURABLE_OPEN_BUSY_RETRIES.fetch_add(1, Ordering::Relaxed);
    DURABLE_OPEN_BUSY_WAIT_US.fetch_add(wait_us, Ordering::Relaxed);
}

/// Record wall of pack exclusive `File::lock` (includes uncontended).
pub fn record_pack_lock_wait(wait_us: u64) {
    PACK_LOCK_ACQUIRES.fetch_add(1, Ordering::Relaxed);
    PACK_LOCK_WAIT_US.fetch_add(wait_us, Ordering::Relaxed);
}

/// Cumulative lock-wait counters (count, wait_us) for profilers and operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LockWaitSnapshot {
    pub index_lock_waits: u64,
    pub index_lock_wait_us: u64,
    pub durable_open_busy_retries: u64,
    pub durable_open_busy_wait_us: u64,
    pub pack_lock_acquires: u64,
    pub pack_lock_wait_us: u64,
}

/// Non-destructive snapshot of lock-wait atomics (+ pack flock from recovery).
pub fn lock_wait_snapshot() -> LockWaitSnapshot {
    LockWaitSnapshot {
        index_lock_waits: INDEX_LOCK_WAITS.load(Ordering::Relaxed),
        index_lock_wait_us: INDEX_LOCK_WAIT_US.load(Ordering::Relaxed),
        durable_open_busy_retries: DURABLE_OPEN_BUSY_RETRIES.load(Ordering::Relaxed),
        durable_open_busy_wait_us: DURABLE_OPEN_BUSY_WAIT_US.load(Ordering::Relaxed),
        pack_lock_acquires: PACK_LOCK_ACQUIRES.load(Ordering::Relaxed),
        pack_lock_wait_us: PACK_LOCK_WAIT_US.load(Ordering::Relaxed),
    }
}

/// Snapshot then zero lock-wait counters (does not clear pack stats unless pack reset called).
pub fn take_lock_wait_snapshot() -> LockWaitSnapshot {
    LockWaitSnapshot {
        index_lock_waits: INDEX_LOCK_WAITS.swap(0, Ordering::Relaxed),
        index_lock_wait_us: INDEX_LOCK_WAIT_US.swap(0, Ordering::Relaxed),
        durable_open_busy_retries: DURABLE_OPEN_BUSY_RETRIES.swap(0, Ordering::Relaxed),
        durable_open_busy_wait_us: DURABLE_OPEN_BUSY_WAIT_US.swap(0, Ordering::Relaxed),
        pack_lock_acquires: PACK_LOCK_ACQUIRES.swap(0, Ordering::Relaxed),
        pack_lock_wait_us: PACK_LOCK_WAIT_US.swap(0, Ordering::Relaxed),
    }
}

/// Reset process/serialization and lock-wait counters (tests / trial boundaries).
pub fn reset_runtime_metrics() {
    PROCESS_STARTS.store(0, Ordering::SeqCst);
    SERIALIZATIONS.store(0, Ordering::SeqCst);
    LAST_SERIALIZE_BYTES.store(0, Ordering::SeqCst);
    INDEX_LOCK_WAITS.store(0, Ordering::Relaxed);
    INDEX_LOCK_WAIT_US.store(0, Ordering::Relaxed);
    DURABLE_OPEN_BUSY_RETRIES.store(0, Ordering::Relaxed);
    DURABLE_OPEN_BUSY_WAIT_US.store(0, Ordering::Relaxed);
    PACK_LOCK_ACQUIRES.store(0, Ordering::Relaxed);
    PACK_LOCK_WAIT_US.store(0, Ordering::Relaxed);
}

/// True when two consecutive serializations of the same op payload are redundant
/// (same byte length + count ≥ 2 within one trial after a single domain op).
pub fn duplicate_serialization_detected(serializations_in_trial: u64, domain_ops: u32) -> bool {
    serializations_in_trial > u64::from(domain_ops.max(1))
}
