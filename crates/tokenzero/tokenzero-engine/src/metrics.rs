//! Per-tool call observability for the MCP server.
//!
//! Tracks call counts, error counts, slow-call counts, and latency per
//! canonical tool. In-process counters cover the current session; a small
//! JSON sidecar next to the recovery cache accumulates the same counters
//! across sessions (each flush merges dirty deltas under an exclusive flock
//! so concurrent processes accumulate rather than clobber; atomic rename
//! prevents partial writes). Exposed via `resource://tokenzero/metrics`.
//!
//! All recording is fail-open: a poisoned lock, contended flock, or
//! unwritable sidecar never propagates an error into a tool call.

use fs4::FileExt;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokenzero_core::MCP_SCHEMA_VERSION;

/// Default latency above which a call is flagged "slow". Override with
/// `TOKENZERO_SLOW_TOOL_MS`.
const DEFAULT_SLOW_TOOL_MS: u64 = 2000;

/// Coalesce persistent sidecar RMW so warm MCP tools do not pay a full
/// read-modify-write of `tool-metrics.json` on every call. Session counters
/// stay exact; the on-disk cumulative sidecar remains approximate (already
/// documented) and is flushed on this interval, on snapshot, and on drop.
const PERSIST_COALESCE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Default)]
struct ToolStat {
    calls: u64,
    errors: u64,
    slow_calls: u64,
    total_ms: u64,
    max_ms: u64,
}

impl ToolStat {
    fn record(&mut self, ms: u64, is_error: bool, slow: bool) {
        self.calls += 1;
        self.total_ms += ms;
        self.max_ms = self.max_ms.max(ms);
        if is_error {
            self.errors += 1;
        }
        if slow {
            self.slow_calls += 1;
        }
    }

    fn from_json(value: &Value) -> Self {
        let u = |key: &str| value.get(key).and_then(Value::as_u64).unwrap_or(0);
        Self {
            calls: u("calls"),
            errors: u("errors"),
            slow_calls: u("slow_calls"),
            total_ms: u("total_ms"),
            max_ms: u("max_ms"),
        }
    }

    fn to_json(&self) -> Value {
        let avg_ms = self.total_ms.checked_div(self.calls).unwrap_or(0);
        json!({
            "calls": self.calls,
            "errors": self.errors,
            "slow_calls": self.slow_calls,
            "total_ms": self.total_ms,
            "max_ms": self.max_ms,
            "avg_ms": avg_ms,
        })
    }
}

#[derive(Debug)]
pub(crate) struct ToolMetrics {
    /// Sidecar JSON path, derived from the recovery-cache path.
    path: PathBuf,
    slow_ms: u64,
    /// This process's counters; resets when the server exits.
    session: Mutex<BTreeMap<String, ToolStat>>,
    /// In-memory mirror of the sidecar; updated on every record so
    /// snapshots stay accurate even when disk writes fail (fail-open).
    persisted: Mutex<BTreeMap<String, ToolStat>>,
    /// Most recent in-process engine/persistence split per canonical tool.
    /// Exposed only through the metrics resource for measurement and diagnosis.
    last_attribution_us: Mutex<BTreeMap<String, (u64, u64)>>,
    /// Pending one-call deltas not yet merged to the on-disk sidecar.
    dirty: Mutex<BTreeMap<String, ToolStat>>,
    last_disk_flush: Mutex<Instant>,
    /// In-process dirty-take / RMW companion. Not the cross-process persist
    /// gate — that is `MetricsPersistLock` (flock). Lock order is flock then
    /// `persist`; there is no persist-then-flock path.
    persist: Mutex<()>,
    tmp_nonce: AtomicU64,
}

impl ToolMetrics {
    pub(crate) fn new(cache_path: &Path) -> Self {
        let path = cache_path.with_file_name("tool-metrics.json");
        let slow_ms = std::env::var("TOKENZERO_SLOW_TOOL_MS")
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .filter(|ms| *ms > 0)
            .unwrap_or(DEFAULT_SLOW_TOOL_MS);
        let persisted = load_persisted_from_path(&path);
        Self {
            path,
            slow_ms,
            session: Mutex::new(BTreeMap::new()),
            persisted: Mutex::new(persisted),
            last_attribution_us: Mutex::new(BTreeMap::new()),
            dirty: Mutex::new(BTreeMap::new()),
            last_disk_flush: Mutex::new(Instant::now() - PERSIST_COALESCE),
            persist: Mutex::new(()),
            tmp_nonce: AtomicU64::new(0),
        }
    }

    /// Record one tool call. Never errors (fail-open).
    pub(crate) fn record(&self, tool: &str, elapsed: Duration, is_error: bool) {
        let ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
        let slow = ms >= self.slow_ms;

        if let Ok(mut session) = self.session.lock() {
            session
                .entry(tool.to_string())
                .or_default()
                .record(ms, is_error, slow);
        }

        // Keep the in-memory cumulative mirror exact for this process.
        if let Ok(mut mirror) = self.persisted.lock() {
            mirror
                .entry(tool.to_string())
                .or_default()
                .record(ms, is_error, slow);
        }
        if let Ok(mut dirty) = self.dirty.lock() {
            dirty
                .entry(tool.to_string())
                .or_default()
                .record(ms, is_error, slow);
        }

        // Disk RMW is coalesced: warm MCP paths should not rewrite the
        // sidecar on every microsecond-scale tools/call.
        let due = self
            .last_disk_flush
            .lock()
            .map(|at| at.elapsed() >= PERSIST_COALESCE)
            .unwrap_or(true);
        if due {
            self.flush_persisted();
        }
    }

    /// Merge dirty deltas into the on-disk sidecar (fail-open).
    pub(crate) fn flush_persisted(&self) {
        // SAFETY: flock is the persist gate for `tool-metrics.json`. Two MCP
        // processes (P1, P2) both RMW this sidecar; an in-process `persist`
        // mutex cannot serialize them. Acquire flock first so a contended
        // wait does not hold `persist`. Dirty is not taken until both gates
        // are held, so a skipped flock leaves deltas for the next flush.
        let Some(_lock) = MetricsPersistLock::acquire(&self.path) else {
            return;
        };
        let _persist = match self.persist.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        let dirty = match self.dirty.lock() {
            Ok(mut guard) => {
                if guard.is_empty() {
                    return;
                }
                std::mem::take(&mut *guard)
            }
            Err(_) => return,
        };
        // Reload from disk so concurrent server processes accumulate.
        let mut persisted = self.load_persisted();
        for (tool, delta) in dirty {
            let entry = persisted.entry(tool).or_default();
            entry.calls += delta.calls;
            entry.errors += delta.errors;
            entry.slow_calls += delta.slow_calls;
            entry.total_ms += delta.total_ms;
            entry.max_ms = entry.max_ms.max(delta.max_ms);
        }
        if let Ok(mut mirror) = self.persisted.lock() {
            *mirror = persisted.clone();
        }
        let _ = self.write_persisted(&persisted);
        if let Ok(mut at) = self.last_disk_flush.lock() {
            *at = Instant::now();
        }
    }

    pub(crate) fn record_attribution(&self, tool: &str, engine: Duration, persist: Duration) {
        let engine_us = u64::try_from(engine.as_micros()).unwrap_or(u64::MAX);
        let persist_us = u64::try_from(persist.as_micros()).unwrap_or(u64::MAX);
        if let Ok(mut attribution) = self.last_attribution_us.lock() {
            attribution.insert(tool.to_string(), (engine_us, persist_us));
        }
    }

    /// Snapshot for `resource://tokenzero/metrics`.
    pub(crate) fn snapshot(&self) -> Value {
        // Flush so the metrics resource reflects recent calls.
        self.flush_persisted();
        let cumulative = match self.persisted.lock() {
            Ok(persisted) => Self::map_to_json(&persisted),
            Err(_) => Self::map_to_json(&self.load_persisted()),
        };
        let session = match self.session.lock() {
            Ok(session) => Self::map_to_json(&session),
            Err(_) => json!({}),
        };
        let last_attribution_us = self
            .last_attribution_us
            .lock()
            .map(|samples| {
                samples
                    .iter()
                    .map(|(tool, (engine_us, persist_us))| {
                        (
                            tool.clone(),
                            json!({ "engine_us": engine_us, "persist_us": persist_us }),
                        )
                    })
                    .collect::<serde_json::Map<String, Value>>()
            })
            .unwrap_or_default();
        json!({
            "schema_version": MCP_SCHEMA_VERSION,
            "status": "ok",
            "slow_threshold_ms": self.slow_ms,
            "persistent_path": self.path.display().to_string(),
            "cumulative": cumulative,
            "session": session,
            "last_attribution_us": last_attribution_us,
            "next_actions": [
                "cumulative counts persist across sessions in the sidecar next to the recovery cache; session counts reset when the server process exits.",
                "Set TOKENZERO_SLOW_TOOL_MS to change the slow-call threshold."
            ]
        })
    }

    fn map_to_json(stats: &BTreeMap<String, ToolStat>) -> Value {
        let tools: serde_json::Map<String, Value> = stats
            .iter()
            .map(|(name, stat)| (name.clone(), stat.to_json()))
            .collect();
        let totals = stats.values().fold(ToolStat::default(), |mut acc, stat| {
            acc.calls += stat.calls;
            acc.errors += stat.errors;
            acc.slow_calls += stat.slow_calls;
            acc.total_ms += stat.total_ms;
            acc.max_ms = acc.max_ms.max(stat.max_ms);
            acc
        });
        json!({ "tools": Value::Object(tools), "totals": totals.to_json() })
    }

    fn load_persisted(&self) -> BTreeMap<String, ToolStat> {
        load_persisted_from_path(&self.path)
    }

    fn write_persisted(&self, stats: &BTreeMap<String, ToolStat>) -> std::io::Result<()> {
        let payload = json!({
            "schema": 1,
            "slow_threshold_ms": self.slow_ms,
            "tools": stats
                .iter()
                .map(|(name, stat)| (name.clone(), stat.to_json()))
                .collect::<serde_json::Map<String, Value>>(),
        });
        let body = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Atomic-ish write: unique temp (pid + monotonic nonce) then rename.
        let nonce = self.tmp_nonce.fetch_add(1, Ordering::Relaxed);
        let tmp = self
            .path
            .with_extension(format!("tmp-{}-{}", std::process::id(), nonce));
        std::fs::write(&tmp, body.as_bytes())?;
        match std::fs::rename(&tmp, &self.path) {
            Ok(()) => Ok(()),
            Err(err) => {
                let _ = std::fs::remove_file(&tmp);
                Err(err)
            }
        }
    }
}

fn metrics_lock_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(|| "tool-metrics.json".into(), |name| name.to_os_string());
    name.push(".lock");
    path.with_file_name(name)
}

/// RAII exclusive flock over a sibling lock file for the metrics sidecar.
struct MetricsPersistLock {
    file: std::fs::File,
}

impl MetricsPersistLock {
    fn acquire(sidecar: &Path) -> Option<Self> {
        let lock_path = metrics_lock_path(sidecar);
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).ok()?;
        }
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .ok()?;
        const LOCK_ATTEMPTS: usize = 50;
        for attempt in 0..LOCK_ATTEMPTS {
            match FileExt::try_lock(&file) {
                Ok(()) => return Some(Self { file }),
                Err(_) if attempt + 1 < LOCK_ATTEMPTS => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return None,
            }
        }
        None
    }
}

impl Drop for MetricsPersistLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn load_persisted_from_path(path: &Path) -> BTreeMap<String, ToolStat> {
    let mut out = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return out;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return out; // corrupt sidecar: start fresh rather than fail
    };
    if let Some(tools) = value.get("tools").and_then(Value::as_object) {
        for (name, stat) in tools {
            out.insert(name.clone(), ToolStat::from_json(stat));
        }
    }
    out
}

impl Drop for ToolMetrics {
    fn drop(&mut self) {
        self.flush_persisted();
    }
}

