#![forbid(unsafe_code)]

use fs4::{FileExt, TryLockError};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{BufRead, BufReader, Error as IoError, ErrorKind, Result as IoResult, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokenzero_core::{savings_ratio, savings_ratio_u64, PULSE_SCHEMA_VERSION};
use zero_store::{Engine, ResolvedStore};

mod eprocess;
pub use eprocess::{AnytimeFailureMonitor, EProcessSnapshot, MonitorConfigError};

trait IntoIo<T> {
    fn into_io(self) -> IoResult<T>;
}

impl<T, E: Into<Box<dyn std::error::Error + Send + Sync>>> IntoIo<T> for Result<T, E> {
    fn into_io(self) -> IoResult<T> {
        self.map_err(|err| IoError::new(ErrorKind::InvalidData, err))
    }
}

const EVENT_SQL_COLUMNS: &str = "schema_version, event, timestamp_unix, tool, mode, raw_tokens, visible_tokens, recovery_tokens, task_lossless, cache_hit, retry_count, failure, exact_ref_count, latency_ms, source_hash, session_id, call_id, ref_ids, tokenizer_id";
pub const PULSE_SOURCE_OF_TRUTH: &str = "jsonl";
pub const PULSE_SYNC_SCHEMA_VERSION: &str = "pulse-sync-v1";
const PULSE_SYNC_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const PULSE_EVENT_LOCK_TIMEOUT: Duration = Duration::from_secs(30);
const TOKENIZER_COMPONENT_MAX_LEN: usize = 64;
const TOKENIZER_ID_ERROR: &str = "tokenizer id must name a real tokenizer or use estimator:<name>";

pub use tokenzero_core::{
    preflight_tokenizer_id, TokenizerIdPreflightError, UNLABELED_ESTIMATE_TOKENIZER_PREFIX,
};

/// Built-in production counts use TokenZero's deliberately labelled lexical
/// gauge. The core gauges are approximate until an exact tokenizer adapter is
/// linked; provider adapters may supply an exact `provider/model@<digest>` id.
fn default_tokenizer_id() -> String {
    "estimator:tokenzero-core".to_string()
}

fn is_tokenizer_slug(component: &str) -> bool {
    let bytes = component.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= TOKENIZER_COMPONENT_MAX_LEN
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// Validate the Pulse tokenizer-id grammar at every trust boundary.
///
/// Three labeled classes, never conflated:
/// - `estimator:<slug>` — approximate lexical/family/byte gauges
/// - `tiktoken:<encoding>` — bundled BPE, exact for that vocab, **not**
///   `ExactTokenizerIdentity` (no provider-locked revision digest)
/// - `provider/model@<64hex>` — `ExactTokenizerIdentity::ledger_identity`
///
/// Bare `Q99`, `exact`, MCP registry labels, and unlabeled model ids fail.
/// `estimate:` is refused by [`preflight_tokenizer_id`] before grammar match
/// so it cannot be smuggled as an `estimator:` alias.
fn valid_tokenizer_id(id: &str) -> bool {
    tokenzero_core::preflight_tokenizer_id(id).is_ok() && pulse_tokenizer_grammar(id)
}

fn pulse_tokenizer_grammar(id: &str) -> bool {
    if let Some(name) = id.strip_prefix("estimator:") {
        return is_tokenizer_slug(name);
    }
    if let Some(encoding) = id.strip_prefix("tiktoken:") {
        return is_tokenizer_slug(encoding);
    }
    let Some((provider_and_model, digest)) = id.rsplit_once('@') else {
        return false;
    };
    let Some((provider, model)) = provider_and_model.split_once('/') else {
        return false;
    };
    is_tokenizer_slug(provider)
        && is_tokenizer_slug(model)
        && digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Pulse count class for a grammar-valid tokenizer id.
///
/// `estimator:` is approximate. `tiktoken:` is bundled BPE (kernel-certified
/// for that vocab, never Pulse ExactTokenizerIdentity). `exact` is only
/// `provider/model@hex`.
pub fn pulse_counts_class(tokenizer_id: &str) -> &'static str {
    if tokenizer_id.is_empty() {
        "empty"
    } else if tokenizer_id.starts_with("estimator:") {
        "estimator"
    } else if tokenizer_id.starts_with("tiktoken:") {
        "tiktoken"
    } else {
        "exact"
    }
}

/// True only for a single ExactTokenizerIdentity (`provider/model@hex`).
/// `estimator:` and `tiktoken:` never certify CLI/MCP totals as exact.
pub fn pulse_counts_certified(tokenizer_id: &str) -> bool {
    pulse_counts_class(tokenizer_id) == "exact"
}

/// Seal CLI/MCP aggregate tokenizer labels. Mixed ids are not one unit:
/// `certified` stays false and `savings_commensurate` is false so a savings
/// field cannot be read as exact/Q99 across estimators.
fn seal_report_tokenizers(ids: &BTreeSet<String>) -> (String, String, bool, bool) {
    match ids.len() {
        0 => (String::new(), "empty".to_string(), false, true),
        1 => {
            let id = ids.iter().next().expect("one tokenizer id");
            (
                id.clone(),
                pulse_counts_class(id).to_string(),
                pulse_counts_certified(id),
                true,
            )
        }
        _ => ("mixed".to_string(), "mixed".to_string(), false, false),
    }
}

fn tokenizer_id_refusal(id: &str) -> &'static str {
    match tokenzero_core::preflight_tokenizer_id(id) {
        Err(error) => error.as_str(),
        Ok(()) => TOKENIZER_ID_ERROR,
    }
}

macro_rules! pulse_structs {
    ($( $(#[$struct_attr:meta])* $name:ident { $($(#[$field_attr:meta])* $field:ident $ty:ty;)* })*) => {
        $(
            #[derive(Debug, Clone, Serialize, Deserialize)]
            $(#[$struct_attr])*
            pub struct $name {
                $(
                    $(#[$field_attr])*
                    pub $field: $ty,
                )*
            }
        )*
    };
}

pulse_structs! {
    PulseEvent {
        schema_version String;
        event String;
        timestamp_unix u64;
        tool String;
        mode String;
        raw_tokens usize;
        visible_tokens usize;
        recovery_tokens usize;
        /// Tokenizer used for all counts; estimators are explicitly labelled.
        #[serde(default = "default_tokenizer_id")] tokenizer_id String;
        task_lossless bool;
        cache_hit bool;
        retry_count usize;
        failure bool;
        exact_ref_count usize;
        latency_ms u128;
        /// `tool_call` stores the first 64 bits (16 hex characters) of its
        /// source hint's SHA-256 here. Direct construction/deserialization is
        /// not validated, so callers must never place raw payloads in this
        /// correlatable field.
        source_hash Option<String>;
        /// Serving session id for expand-time attribution. Stored verbatim in
        /// the local ledger when supplied; it is not anonymized.
        #[serde(default, skip_serializing_if = "Option::is_none")] session_id Option<String>;
        /// Call id within the session (e.g. JSON-RPC id). Stored verbatim in the
        /// local ledger when supplied; it is not anonymized.
        #[serde(default, skip_serializing_if = "Option::is_none")] call_id Option<String>;
        /// Serve/expand tz:// refs — RACC join keys. Stored verbatim in the local
        /// ledger when supplied and potentially correlatable with local payloads.
        #[serde(default, skip_serializing_if = "Vec::is_empty")] ref_ids Vec<String>;
    }
    #[derive(Default)]
    PulseReport {
        schema_version String;
        status String;
        event_count usize;
        raw_tokens usize;
        visible_tokens usize;
        recovery_tokens usize;
        task_lossless_tokens usize;
        failures usize;
        cache_hits usize;
        exact_ref_count usize;
        visible_savings f64;
        recovery_adjusted_savings f64;
        /// Net spent = visible + recovery. Expand charges belong here, not
        /// only in visible_savings (which understates task spend).
        #[serde(default)] spent_tokens usize;
        /// Corrupt/non-empty unparsable ledger lines.
        #[serde(default)] skipped_lines usize;
        /// Common tokenizer_id, `mixed` when events disagree, empty when none.
        #[serde(default)] tokenizer_id String;
        /// estimator | tiktoken | exact | mixed | empty
        #[serde(default)] counts_class String;
        /// True only for one ExactTokenizerIdentity. Estimator aggregates
        /// never certify CLI stats/pulse JSON as exact.
        #[serde(default)] certified bool;
        /// False when tokenizer ids mix; savings then are not one-unit.
        #[serde(default)] savings_commensurate bool;
    }
    PulseSyncMeta {
        schema_version String;
        source_of_truth String;
        ledger_sha256 String;
        event_count usize;
        skipped_lines usize;
        updated_unix u64;
    }
    PulseSyncStatus {
        ok bool;
        source_of_truth String;
        ledger_path PathBuf;
        sqlite_path PathBuf;
        meta_path PathBuf;
        event_count usize;
        skipped_lines usize;
        ledger_sha256 String;
    }
    PulseDoctorReport {
        ok bool;
        source_of_truth String;
        ledger_path PathBuf;
        sqlite_path PathBuf;
        meta_path PathBuf;
        event_count usize;
        skipped_lines usize;
        ledger_sha256 String;
        sqlite_integrity String;
        marker_match bool;
        hot_index_used bool;
    }
}

macro_rules! data_row {
    ($ty:ident; $($field:ident = $value:expr;)*) => {
        $ty {
            $($field: $value,)*
        }
    };
}
impl PulseEvent {
    #[allow(clippy::too_many_arguments)]
    pub fn tool_call(
        tool: &str,
        mode: &str,
        raw_tokens: usize,
        visible_tokens: usize,
        recovery_tokens: usize,
        exact_ref_count: usize,
        latency_ms: u128,
        source_hint: Option<&str>,
    ) -> Self {
        data_row! { PulseEvent;
            schema_version = PULSE_SCHEMA_VERSION.to_string();
            event = "tool_call".to_string();
            timestamp_unix = now_unix();
            tool = tool.to_string();
            mode = mode.to_string();
            raw_tokens = raw_tokens;
            visible_tokens = visible_tokens;
            recovery_tokens = recovery_tokens;
            tokenizer_id = default_tokenizer_id();
            task_lossless = pulse_task_lossless(raw_tokens, visible_tokens, recovery_tokens);
            cache_hit = false;
            retry_count = 0;
            failure = false;
            exact_ref_count = exact_ref_count;
            latency_ms = latency_ms;
            source_hash = source_hint.map(hash_hint);
            session_id = None;
            call_id = None;
            ref_ids = Vec::new();
        }
    }

    pub fn with_attribution(
        mut self,
        session_id: Option<String>,
        call_id: Option<String>,
        ref_ids: Vec<String>,
    ) -> Self {
        self.session_id = session_id;
        self.call_id = call_id;
        self.ref_ids = ref_ids;
        self
    }

    pub fn with_tokenizer_id(mut self, tokenizer_id: &str) -> Result<Self, &'static str> {
        if !valid_tokenizer_id(tokenizer_id) {
            return Err(tokenizer_id_refusal(tokenizer_id));
        }
        self.tokenizer_id = tokenizer_id.to_string();
        Ok(self)
    }
}

/// Lossless means omitted visible mass was charged back as recovery.
/// `visible < raw` with `recovery == 0` is lossy (or a worse wrapper with
/// no expand path); it must not inflate `task_lossless_tokens`.
pub fn pulse_task_lossless(
    raw_tokens: usize,
    visible_tokens: usize,
    recovery_tokens: usize,
) -> bool {
    !(visible_tokens < raw_tokens && recovery_tokens == 0)
}

pub fn default_ledger_path(root: &Path) -> PathBuf {
    ResolvedStore::resolve_from_process(root, Engine::TokenZero, &["TOKENZERO_SHARED_STORE"])
        .engine_dir()
        .join("pulse/events.jsonl")
}

fn with_pulse_lock<T>(
    path: &Path,
    timeout: Duration,
    action: impl FnOnce() -> IoResult<T>,
) -> IoResult<T> {
    let _lock = acquire_pulse_lock_wait(path, timeout)?;
    action()
}

pub fn verify_open_regular_file(path: &Path, file: &fs::File, label: &str) -> IoResult<()> {
    if file.metadata()?.is_file() && fs::symlink_metadata(path)?.file_type().is_file() {
        Ok(())
    } else {
        Err(IoError::new(
            ErrorKind::InvalidData,
            format!("{label} path must be a regular file"),
        ))
    }
}

#[derive(Clone, Copy)]
pub enum PulseFileOpenMode {
    Append,
    ReadWrite,
}

#[cfg(unix)]
pub fn open_nofollow(path: &Path, mode: PulseFileOpenMode) -> IoResult<(fs::File, bool)> {
    use rustix::fs::{openat, Mode, OFlags, CWD};

    let access = match mode {
        PulseFileOpenMode::Append => OFlags::WRONLY | OFlags::APPEND,
        PulseFileOpenMode::ReadWrite => OFlags::RDWR,
    };
    let base = access | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
    let permissions = Mode::from_bits_truncate(0o666);
    let open = |flags| {
        openat(CWD, path, flags, permissions)
            .map(fs::File::from)
            .map_err(IoError::from)
    };
    match open(base | OFlags::CREATE | OFlags::EXCL) {
        Ok(file) => Ok((file, true)),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            open(base).map(|file| (file, false))
        }
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
pub fn open_nofollow(path: &Path, mode: PulseFileOpenMode) -> IoResult<(fs::File, bool)> {
    use std::os::windows::fs::OpenOptionsExt;

    // Prevent CreateFileW from traversing a reparse point. The opened handle
    // and path are both validated as regular files before any mutation.
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let open = |create_new: bool| {
        let mut options = fs::OpenOptions::new();
        options
            .create_new(create_new)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        match mode {
            PulseFileOpenMode::Append => {
                options.append(true);
            }
            PulseFileOpenMode::ReadWrite => {
                options.read(true).write(true);
            }
        }
        options.open(path)
    };
    match open(true) {
        Ok(file) => Ok((file, true)),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            open(false).map(|file| (file, false))
        }
        Err(error) => Err(error),
    }
}

#[cfg(not(any(unix, windows)))]
pub fn open_nofollow(path: &Path, mode: PulseFileOpenMode) -> IoResult<(fs::File, bool)> {
    let open = |create_new: bool| {
        let mut options = fs::OpenOptions::new();
        options.create_new(create_new);
        match mode {
            PulseFileOpenMode::Append => {
                options.append(true);
            }
            PulseFileOpenMode::ReadWrite => {
                options.read(true).write(true);
            }
        }
        options.open(path)
    };
    match open(true) {
        Ok(file) => Ok((file, true)),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            open(false).map(|file| (file, false))
        }
        Err(error) => Err(error),
    }
}

pub fn record_event(path: &Path, event: &PulseEvent) -> IoResult<()> {
    if !valid_tokenizer_id(&event.tokenizer_id) {
        return Err(IoError::new(
            ErrorKind::InvalidInput,
            tokenizer_id_refusal(&event.tokenizer_id),
        ));
    }
    // Serialize before taking the cross-process lock so formatting work never
    // lengthens the critical section. The lock then orders event appends with
    // sync/import/export and protects the complete append + durability barrier.
    let mut line = serde_json::to_vec(event).into_io()?;
    line.push(b'\n');
    with_pulse_lock(path, PULSE_EVENT_LOCK_TIMEOUT, || {
        let (mut file, created) = open_nofollow(path, PulseFileOpenMode::Append)?;
        verify_open_regular_file(path, &file, "Pulse ledger")?;
        file.write_all(&line)?;
        file.sync_data()?;
        if created {
            sync_parent(path)?;
        }
        Ok(())
    })
}

pub fn sync_jsonl_to_sqlite(path: &Path) -> IoResult<PulseSyncStatus> {
    with_pulse_lock(path, PULSE_SYNC_LOCK_TIMEOUT, || {
        sync_jsonl_to_sqlite_locked(path)
    })
}

pub fn export_jsonl(path: &Path, output: &Path) -> IoResult<PulseSyncStatus> {
    with_pulse_lock(path, PULSE_SYNC_LOCK_TIMEOUT, || {
        let status = sync_jsonl_to_sqlite_locked(path)?;
        atomic_export_sqlite_jsonl(&status.sqlite_path, output)?;
        write_sidecar_meta(
            &export_meta_path(output),
            &meta_from_scan(&scan_jsonl(output, |_| Ok(()))?),
        )?;
        Ok(status)
    })
}

pub fn import_jsonl(input: &Path, path: &Path) -> IoResult<PulseSyncStatus> {
    with_pulse_lock(path, PULSE_SYNC_LOCK_TIMEOUT, || {
        let input_source = ensure_import_not_older(input, path)?;
        atomic_import_valid_jsonl(input, path, &input_source.scan)?;
        sync_jsonl_to_sqlite_locked(path)
    })
}

pub fn doctor_jsonl_sqlite(path: &Path) -> IoResult<PulseDoctorReport> {
    let status = sync_jsonl_to_sqlite(path)?;
    let conn = open_sqlite(&status.sqlite_path)?;
    let sqlite_integrity = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
        .into_io()?;
    let sqlite_meta = read_sqlite_meta(&conn)?;
    let sidecar_meta = read_sidecar_meta(&status.meta_path)?;
    let marker_match = sqlite_meta.ledger_sha256 == status.ledger_sha256
        && sidecar_meta.ledger_sha256 == status.ledger_sha256
        && sqlite_meta.event_count == status.event_count
        && sidecar_meta.event_count == status.event_count;
    let hot_index_used = hot_index_is_used(&conn)?;
    Ok(data_row! { PulseDoctorReport;
        ok = status.ok && sqlite_integrity == "ok" && marker_match && hot_index_used;
        source_of_truth = status.source_of_truth;
        ledger_path = status.ledger_path;
        sqlite_path = status.sqlite_path;
        meta_path = status.meta_path;
        event_count = status.event_count;
        skipped_lines = status.skipped_lines;
        ledger_sha256 = status.ledger_sha256;
        sqlite_integrity = sqlite_integrity;
        marker_match = marker_match;
        hot_index_used = hot_index_used;
    })
}

pub fn render_text(report: &PulseReport) -> String {
    let tokenizer = if report.tokenizer_id.is_empty() {
        "-"
    } else {
        report.tokenizer_id.as_str()
    };
    let mut out = format!(
        "pulse {}: events={} tokenizer={} counts_class={} certified={} commensurate={} visible_savings={:.2}% recovery_adjusted_savings={:.2}% failures={}\n",
        report.status,
        report.event_count,
        tokenizer,
        if report.counts_class.is_empty() {
            "empty"
        } else {
            report.counts_class.as_str()
        },
        report.certified,
        report.savings_commensurate,
        report.visible_savings * 100.0,
        report.recovery_adjusted_savings * 100.0,
        report.failures
    );
    if report.skipped_lines > 0 {
        out.push_str(&format!(
            "pulse warning: skipped {} corrupt ledger line(s)\n",
            report.skipped_lines
        ));
    }
    out
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn sync_jsonl_to_sqlite_locked(path: &Path) -> IoResult<PulseSyncStatus> {
    let sqlite_path = sqlite_path_for_ledger(path);
    let meta_path = meta_path_for_ledger(path);
    for p in [path.parent(), sqlite_path.parent()].into_iter().flatten() {
        fs::create_dir_all(p)?;
    }

    let scan = sync_jsonl_into_sqlite_cache(path, &sqlite_path)?;

    let meta = meta_from_scan(&scan);
    write_sidecar_meta(&meta_path, &meta)?;

    Ok(data_row! { PulseSyncStatus;
        ok = scan.skipped_lines == 0;
        source_of_truth = PULSE_SOURCE_OF_TRUTH.to_string();
        ledger_path = path.to_path_buf();
        sqlite_path = sqlite_path;
        meta_path = meta_path;
        event_count = scan.event_count;
        skipped_lines = scan.skipped_lines;
        ledger_sha256 = scan.ledger_sha256;
    })
}

fn open_sqlite(path: &Path) -> IoResult<Connection> {
    let conn = Connection::open(path).into_io()?;
    conn.busy_timeout(Duration::from_secs(5)).into_io()?;
    for (key, val) in [
        ("journal_mode", "WAL"),
        ("synchronous", "NORMAL"),
        ("fullfsync", "ON"),
        ("wal_autocheckpoint", "1000"),
        ("foreign_keys", "ON"),
    ] {
        conn.pragma_update(None, key, val).into_io()?;
    }
    Ok(conn)
}

fn sync_jsonl_into_sqlite_cache(ledger_path: &Path, sqlite_path: &Path) -> IoResult<JsonlScan> {
    let scan = scan_jsonl(ledger_path, |_| Ok(()))?;
    let mut conn = open_or_rebuild_sqlite(sqlite_path)?;
    // Marker-equality fast path: skip DELETE+rebuild when meta matches scan.
    if let Ok(sqlite_meta) = read_sqlite_meta(&conn) {
        let meta_path = meta_path_for_ledger(ledger_path);
        if meta_matches_scan(&sqlite_meta, &scan)
            && read_sidecar_meta(&meta_path)
                .map(|meta| meta_matches_scan(&meta, &scan))
                .unwrap_or(false)
        {
            return Ok(scan);
        }
    }
    match write_sqlite_events_from_jsonl(&mut conn, ledger_path) {
        Ok(scan) => Ok(scan),
        Err(err) if sqlite_cache_can_rebuild(&err) => {
            drop(conn);
            remove_sqlite_cache_files(sqlite_path)?;
            let mut conn = open_sqlite(sqlite_path)?;
            init_sqlite(&conn)?;
            write_sqlite_events_from_jsonl(&mut conn, ledger_path)
        }
        Err(err) => Err(err),
    }
}

fn open_or_rebuild_sqlite(path: &Path) -> IoResult<Connection> {
    let open = || {
        let conn = open_sqlite(path)?;
        init_sqlite(&conn)?;
        Ok(conn)
    };
    match open() {
        Ok(conn) => Ok(conn),
        Err(err) if sqlite_cache_can_rebuild(&err) => {
            remove_sqlite_cache_files(path)?;
            open()
        }
        Err(err) => Err(err),
    }
}

fn sqlite_cache_can_rebuild(err: &IoError) -> bool {
    err.kind() == ErrorKind::InvalidData
        && [
            "file is not a database",
            "database disk image is malformed",
            "not a database",
            "has no column named",
            "no such column",
            "no such table",
        ]
        .iter()
        .any(|needle| err.to_string().contains(needle))
}

pub fn remove_sqlite_cache_files(path: &Path) -> IoResult<()> {
    for suffix in ["", "-wal", "-shm"] {
        match fs::remove_file(sqlite_sidecar_path(path, suffix)) {
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            result => result?,
        }
    }
    Ok(())
}

/// JSON-encode ref ids for the sqlite sidecar; NULL when empty.
fn ref_ids_to_column(ref_ids: &[String]) -> IoResult<Option<String>> {
    if ref_ids.is_empty() {
        Ok(None)
    } else {
        serde_json::to_string(ref_ids).map(Some).into_io()
    }
}

fn ref_ids_from_column(column: Option<String>) -> rusqlite::Result<Vec<String>> {
    match column.as_deref() {
        None | Some("") => Ok(Vec::new()),
        Some(text) => serde_json::from_str(text)
            .map_err(|err| rusqlite::Error::InvalidColumnName(format!("ref_ids JSON: {err}"))),
    }
}

pub fn sqlite_sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut target = path.as_os_str().to_os_string();
    target.push(suffix);
    PathBuf::from(target)
}

fn init_sqlite(conn: &Connection) -> IoResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS events (
            line_no INTEGER PRIMARY KEY,
            schema_version TEXT NOT NULL, event TEXT NOT NULL, timestamp_unix INTEGER NOT NULL,
            tool TEXT NOT NULL, mode TEXT NOT NULL,
            raw_tokens INTEGER NOT NULL, visible_tokens INTEGER NOT NULL, recovery_tokens INTEGER NOT NULL,
            task_lossless INTEGER NOT NULL, cache_hit INTEGER NOT NULL, retry_count INTEGER NOT NULL,
            failure INTEGER NOT NULL, exact_ref_count INTEGER NOT NULL, latency_ms INTEGER NOT NULL,
            source_hash TEXT, session_id TEXT, call_id TEXT, ref_ids TEXT, tokenizer_id TEXT NOT NULL DEFAULT 'estimator:tokenzero-core', record_hash TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS idx_events_tool_time ON events(tool, timestamp_unix DESC);
        CREATE INDEX IF NOT EXISTS idx_events_event_time ON events(event, timestamp_unix DESC);",
    )
    .into_io()?;
    for (column, ddl) in [
        ("session_id", "ALTER TABLE events ADD COLUMN session_id TEXT"),
        ("call_id", "ALTER TABLE events ADD COLUMN call_id TEXT"),
        ("ref_ids", "ALTER TABLE events ADD COLUMN ref_ids TEXT"),
        (
            "tokenizer_id",
            "ALTER TABLE events ADD COLUMN tokenizer_id TEXT NOT NULL DEFAULT 'estimator:tokenzero-core'",
        ),
    ] {
        if sqlite_events_has_column(conn, column)? {
            continue;
        }
        conn.execute(ddl, []).into_io()?;
    }
    Ok(())
}

fn sqlite_events_has_column(conn: &Connection, column: &str) -> IoResult<bool> {
    let mut stmt = conn
        .prepare("SELECT 1 FROM pragma_table_info('events') WHERE name = ?1")
        .into_io()?;
    stmt.exists(params![column]).into_io()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonlScan {
    pub event_count: usize,
    pub skipped_lines: usize,
    pub ledger_sha256: String,
}

fn write_sqlite_events_from_jsonl(conn: &mut Connection, path: &Path) -> IoResult<JsonlScan> {
    let tx = conn.transaction().into_io()?;
    tx.execute("DELETE FROM events", []).into_io()?;
    let scan = {
        let mut stmt = tx.prepare(
            "INSERT INTO events (line_no, schema_version, event, timestamp_unix, tool, mode, raw_tokens, visible_tokens, recovery_tokens, task_lossless, cache_hit, retry_count, failure, exact_ref_count, latency_ms, source_hash, session_id, call_id, ref_ids, tokenizer_id, record_hash) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)",
        ).into_io()?;
        let mut line_no = 0i64;
        scan_jsonl(path, |event| {
            line_no += 1;
            stmt.execute(params![
                line_no,
                &event.schema_version,
                &event.event,
                event.timestamp_unix as i64,
                &event.tool,
                &event.mode,
                clamp_i64(event.raw_tokens),
                clamp_i64(event.visible_tokens),
                clamp_i64(event.recovery_tokens),
                bool_i64(event.task_lossless),
                bool_i64(event.cache_hit),
                clamp_i64(event.retry_count),
                bool_i64(event.failure),
                clamp_i64(event.exact_ref_count),
                clamp_u128_i64(event.latency_ms),
                event.source_hash.as_deref(),
                event.session_id.as_deref(),
                event.call_id.as_deref(),
                ref_ids_to_column(&event.ref_ids)?,
                &event.tokenizer_id,
                hex_sha256(&serde_json::to_vec(event).into_io()?),
            ])
            .into_io()?;
            Ok(())
        })?
    };
    for (k, v) in [
        ("schema_version", PULSE_SYNC_SCHEMA_VERSION.to_string()),
        ("source_of_truth", PULSE_SOURCE_OF_TRUTH.to_string()),
        ("ledger_sha256", scan.ledger_sha256.clone()),
        ("event_count", scan.event_count.to_string()),
        ("skipped_lines", scan.skipped_lines.to_string()),
        ("updated_unix", now_unix().to_string()),
    ] {
        tx.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![k, v],
        )
        .into_io()?;
    }
    tx.commit().into_io()?;
    Ok(scan)
}

fn read_sqlite_meta(conn: &Connection) -> IoResult<PulseSyncMeta> {
    Ok(data_row! { PulseSyncMeta;
        schema_version = sqlite_meta_value(conn, "schema_version")?;
        source_of_truth = sqlite_meta_value(conn, "source_of_truth")?;
        ledger_sha256 = sqlite_meta_value(conn, "ledger_sha256")?;
        event_count = sqlite_meta_value(conn, "event_count")?.parse().into_io()?;
        skipped_lines = sqlite_meta_value(conn, "skipped_lines")?.parse().into_io()?;
        updated_unix = sqlite_meta_value(conn, "updated_unix")?.parse().into_io()?;
    })
}

fn sqlite_meta_value(conn: &Connection, key: &str) -> IoResult<String> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
        row.get::<_, String>(0)
    })
    .into_io()
}

fn hot_index_is_used(conn: &Connection) -> IoResult<bool> {
    let mut stmt = conn
        .prepare("EXPLAIN QUERY PLAN SELECT line_no FROM events WHERE tool = ?1 ORDER BY timestamp_unix DESC LIMIT 10")
        .into_io()?;
    for detail in stmt
        .query_map(["read"], |row| row.get::<_, String>(3))
        .into_io()?
    {
        if detail.into_io()?.contains("idx_events_tool_time") {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn write_sidecar_meta(path: &Path, meta: &PulseSyncMeta) -> IoResult<()> {
    let bytes = serde_json::to_vec_pretty(meta).into_io()?;
    atomic_write(path, &bytes)
}

pub fn read_sidecar_meta(path: &Path) -> IoResult<PulseSyncMeta> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes).into_io()
}

struct VerifiedImportSource {
    scan: JsonlScan,
    meta: Option<PulseSyncMeta>,
}

macro_rules! reject {
    ($kind:ident, $message:expr $(,)?) => {
        return Err(IoError::new(ErrorKind::$kind, $message))
    };
}

fn ensure_import_not_older(input: &Path, current_ledger: &Path) -> IoResult<VerifiedImportSource> {
    if !fs::metadata(input)?.is_file() {
        reject!(InvalidInput, "import source is not a regular file");
    }
    let input_source = verify_import_source(input)?;
    let current_scan = scan_jsonl(current_ledger, |_| Ok(()))?;
    if input_source.scan.ledger_sha256 == current_scan.ledger_sha256 {
        return Ok(input_source);
    }

    let Some(current_meta) = read_trusted_sidecar_meta(&meta_path_for_ledger(current_ledger))?
    else {
        if current_scan.event_count == 0 && current_scan.skipped_lines == 0 {
            return Ok(input_source);
        }
        reject!(
            InvalidInput,
            "current Pulse ledger has no version marker; refusing to overwrite it",
        );
    };
    let Some(input_meta) = &input_source.meta else {
        reject!(
            InvalidInput,
            "import snapshot has no version marker; refusing to overwrite the current Pulse ledger",
        );
    };
    if !meta_matches_scan(&current_meta, &current_scan) {
        if current_scan.skipped_lines > 0 && input_meta.updated_unix > current_meta.updated_unix {
            return Ok(input_source);
        }
        reject!(
            InvalidInput,
            "current Pulse ledger has unsynced changes; run `tokenzero pulse sync` before importing a different snapshot",
        );
    }
    if input_meta.updated_unix <= current_meta.updated_unix {
        reject!(
            InvalidInput,
            "import snapshot is not newer than the current Pulse ledger marker"
        );
    }
    Ok(input_source)
}

fn verify_import_source(input: &Path) -> IoResult<VerifiedImportSource> {
    let scan = scan_jsonl(input, |_| Ok(()))?;
    if scan.skipped_lines > 0 {
        reject!(InvalidData, "import source contains corrupt JSONL line(s)");
    }
    let meta = read_trusted_sidecar_meta(&export_meta_path(input))?;
    if meta
        .as_ref()
        .is_some_and(|meta| !meta_matches_scan(meta, &scan))
    {
        reject!(
            InvalidInput,
            "import snapshot marker does not match source JSONL"
        );
    }
    Ok(VerifiedImportSource { scan, meta })
}

fn read_trusted_sidecar_meta(path: &Path) -> IoResult<Option<PulseSyncMeta>> {
    match read_sidecar_meta(path) {
        Ok(meta)
            if meta.schema_version == PULSE_SYNC_SCHEMA_VERSION
                && meta.source_of_truth == PULSE_SOURCE_OF_TRUTH =>
        {
            Ok(Some(meta))
        }
        Ok(_) => Err(IoError::new(
            ErrorKind::InvalidInput,
            format!(
                "Pulse marker has an unexpected schema or source at {}",
                path.display()
            ),
        )),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

fn meta_from_scan(scan: &JsonlScan) -> PulseSyncMeta {
    data_row! { PulseSyncMeta;
        schema_version = PULSE_SYNC_SCHEMA_VERSION.to_string();
        source_of_truth = PULSE_SOURCE_OF_TRUTH.to_string();
        ledger_sha256 = scan.ledger_sha256.clone();
        event_count = scan.event_count;
        skipped_lines = scan.skipped_lines;
        updated_unix = now_unix();
    }
}

fn meta_matches_scan(meta: &PulseSyncMeta, scan: &JsonlScan) -> bool {
    meta.ledger_sha256 == scan.ledger_sha256
        && meta.event_count == scan.event_count
        && meta.skipped_lines == scan.skipped_lines
}

fn ensure_parent(path: &Path) -> IoResult<()> {
    path.parent().map_or(Ok(()), fs::create_dir_all)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> IoResult<()> {
    ensure_parent(path)?;
    zero_store::atomic_write_file(path, bytes)
}

fn pulse_event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PulseEvent> {
    Ok(data_row! { PulseEvent;
        schema_version = row.get(0)?;
        event = row.get(1)?;
        timestamp_unix = i64_u64(row.get(2)?);
        tool = row.get(3)?;
        mode = row.get(4)?;
        raw_tokens = i64_usize(row.get(5)?);
        visible_tokens = i64_usize(row.get(6)?);
        recovery_tokens = i64_usize(row.get(7)?);
        task_lossless = i64_bool(row.get(8)?);
        cache_hit = i64_bool(row.get(9)?);
        retry_count = i64_usize(row.get(10)?);
        failure = i64_bool(row.get(11)?);
        exact_ref_count = i64_usize(row.get(12)?);
        latency_ms = i64_u128(row.get(13)?);
        source_hash = row.get(14)?;
        session_id = row.get(15)?;
        call_id = row.get(16)?;
        ref_ids = ref_ids_from_column(row.get(17)?)?;
        tokenizer_id = row.get(18)?;
    })
}

fn atomic_export_sqlite_jsonl(sqlite_path: &Path, output: &Path) -> IoResult<()> {
    ensure_parent(output)?;
    let conn = open_sqlite(sqlite_path)?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {EVENT_SQL_COLUMNS} FROM events ORDER BY line_no ASC"
        ))
        .into_io()?;
    let mut buf = Vec::new();
    for row in stmt.query_map([], pulse_event_from_row).into_io()? {
        serde_json::to_writer(&mut buf, &row.into_io()?).into_io()?;
        buf.extend_from_slice(b"\n");
    }
    zero_store::atomic_write_file(output, &buf)
}

pub fn atomic_import_valid_jsonl(
    input: &Path,
    output: &Path,
    expected_scan: &JsonlScan,
) -> IoResult<()> {
    ensure_parent(output)?;
    let mut buf = Vec::new();
    let copied_scan = scan_reader(
        BufReader::new(fs::File::open(input)?),
        |line, _, corrupt| {
            if corrupt {
                reject!(InvalidData, "import source contains corrupt JSONL line(s)");
            }
            buf.extend_from_slice(line);
            Ok(())
        },
    )?;
    if &copied_scan != expected_scan {
        reject!(
            InvalidInput,
            "import source changed while it was being copied"
        );
    }
    zero_store::atomic_write_file(output, &buf)
}

fn sync_parent(path: &Path) -> IoResult<()> {
    path.parent().map_or(Ok(()), |parent| {
        match fs::File::open(parent).and_then(|file| file.sync_all()) {
            Err(err) if !(cfg!(windows) && err.kind() == ErrorKind::PermissionDenied) => Err(err),
            _ => Ok(()),
        }
    })
}

pub struct PulseLock {
    file: fs::File,
}

impl Drop for PulseLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub fn acquire_pulse_lock(path: &Path) -> IoResult<PulseLock> {
    let lock_path = lock_path_for_ledger(path);
    ensure_parent(&lock_path)?;
    let (mut file, _created) = open_nofollow(&lock_path, PulseFileOpenMode::ReadWrite)?;
    verify_open_regular_file(&lock_path, &file, "Pulse lock")?;
    match FileExt::try_lock(&file) {
        Ok(()) => {}
        Err(TryLockError::Error(err)) if err.kind() != ErrorKind::WouldBlock => return Err(err),
        Err(_) => return Err(pulse_lock_held_error(&lock_path)),
    }

    // Keep the lock-file anchor stable across processes (do not unlink on drop).
    file.set_len(0)?;
    let token = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    writeln!(file, "token={token}")?;
    writeln!(file, "pid={}", std::process::id())?;
    writeln!(file, "created_unix={}", now_unix())?;
    Ok(PulseLock { file })
}

fn pulse_lock_held_error(lock_path: &Path) -> IoError {
    IoError::new(
        ErrorKind::WouldBlock,
        format!("pulse sync lock is held at {}", lock_path.display()),
    )
}

// macOS advisory locks can transiently surface EINVAL while another writer is
// cycling the same lock anchor under heavy local contention. Treat it like
// WouldBlock only for bounded wait paths; direct lock acquisition still returns
// the platform error.
fn acquire_pulse_lock_wait(path: &Path, timeout: Duration) -> IoResult<PulseLock> {
    let start = Instant::now();
    let lock_path = lock_path_for_ledger(path);
    loop {
        match acquire_pulse_lock(path) {
            Ok(lock) => return Ok(lock),
            Err(err) if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::InvalidInput) => {
                if start.elapsed() >= timeout {
                    return Err(pulse_lock_held_error(&lock_path));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(err) => return Err(err),
        }
    }
}

fn scan_reader<R: BufRead>(
    mut reader: R,
    mut on_line: impl FnMut(&[u8], Option<&PulseEvent>, bool) -> IoResult<()>,
) -> IoResult<JsonlScan> {
    let (mut hasher, mut line) = (Sha256::new(), Vec::new());
    let (mut event_count, mut skipped_lines) = (0, 0);
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        hasher.update(&line);
        match parse_event_line(&line) {
            Ok(event) => {
                event_count += usize::from(event.is_some());
                on_line(&line, event.as_ref(), false)?;
            }
            Err(()) => {
                skipped_lines += 1;
                on_line(&line, None, true)?;
            }
        }
    }
    Ok(JsonlScan {
        event_count,
        skipped_lines,
        ledger_sha256: zero_abi::hex_lower_32(hasher.finalize().into()),
    })
}

pub fn scan_jsonl<F>(path: &Path, mut on_event: F) -> IoResult<JsonlScan>
where
    F: FnMut(&PulseEvent) -> IoResult<()>,
{
    match fs::File::open(path) {
        Ok(file) => scan_reader(BufReader::new(file), |_, event, _| {
            event.map_or(Ok(()), &mut on_event)
        }),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(JsonlScan {
            event_count: 0,
            skipped_lines: 0,
            ledger_sha256: hex_sha256(&[]),
        }),
        Err(err) => Err(err),
    }
}

pub fn parse_event_line(line: &[u8]) -> Result<Option<PulseEvent>, ()> {
    let trimmed = line.trim_ascii();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let event = serde_json::from_slice::<PulseEvent>(trimmed).map_err(|_| ())?;
    if event.schema_version != PULSE_SCHEMA_VERSION || !valid_tokenizer_id(&event.tokenizer_id) {
        return Err(());
    }
    Ok(Some(event))
}

pub fn report_for_path(path: &Path) -> IoResult<PulseReport> {
    let mut report = PulseReport {
        schema_version: PULSE_SCHEMA_VERSION.to_string(),
        status: "ok".to_string(),
        ..PulseReport::default()
    };
    let mut tokenizer_ids = BTreeSet::new();
    let scan = scan_jsonl(path, |event| {
        tokenizer_ids.insert(event.tokenizer_id.clone());
        report.raw_tokens = report.raw_tokens.saturating_add(event.raw_tokens);
        report.visible_tokens = report.visible_tokens.saturating_add(event.visible_tokens);
        report.recovery_tokens = report.recovery_tokens.saturating_add(event.recovery_tokens);
        if event.task_lossless && !event.failure {
            report.task_lossless_tokens = report
                .task_lossless_tokens
                .saturating_add(event.visible_tokens.saturating_add(event.recovery_tokens));
        }
        report.failures += usize::from(event.failure);
        report.cache_hits += usize::from(event.cache_hit);
        report.exact_ref_count = report.exact_ref_count.saturating_add(event.exact_ref_count);
        Ok(())
    })?;
    report.event_count = scan.event_count;
    report.skipped_lines = scan.skipped_lines;
    let (tokenizer_id, counts_class, certified, commensurate) =
        seal_report_tokenizers(&tokenizer_ids);
    report.tokenizer_id = tokenizer_id;
    report.counts_class = counts_class;
    report.certified = certified;
    report.savings_commensurate = commensurate;
    if report.skipped_lines > 0 {
        report.status = "degraded".to_string();
    } else if !commensurate {
        // Mixed tokenizer classes are not one billed unit. Do not look ok.
        report.status = "mixed_tokenizer".to_string();
    }
    // Signed: spent>raw is a negative ratio, never a clamped 0% save.
    report.spent_tokens = report.visible_tokens.saturating_add(report.recovery_tokens);
    report.visible_savings = savings_ratio(report.raw_tokens, report.visible_tokens);
    report.recovery_adjusted_savings = savings_ratio(report.raw_tokens, report.spent_tokens);
    Ok(report)
}

pub fn hex_sha256(bytes: &[u8]) -> String {
    zero_abi::sha256_hex(bytes)
}

macro_rules! simple_fns {
    ($($name:ident($arg:ident: $arg_ty:ty) -> $out:ty $body:block)*) => {
        $(
            pub fn $name($arg: $arg_ty) -> $out $body
        )*
    };
}

fn ledger_sibling(path: &Path, name: &str) -> PathBuf {
    path.parent().unwrap_or_else(|| Path::new(".")).join(name)
}
simple_fns! {
    sqlite_path_for_ledger(path: &Path) -> PathBuf {
        ledger_sibling(path, "events.sqlite")
    }
    meta_path_for_ledger(path: &Path) -> PathBuf {
        ledger_sibling(path, "events.meta.json")
    }
    export_meta_path(path: &Path) -> PathBuf {
        path.with_extension("meta.json")
    }
    lock_path_for_ledger(path: &Path) -> PathBuf {
        ledger_sibling(path, "sync.lock")
    }
    clamp_i64(value: usize) -> i64 {
        value.min(i64::MAX as usize) as i64
    }
    clamp_u128_i64(value: u128) -> i64 {
        value.min(i64::MAX as u128) as i64
    }
    bool_i64(value: bool) -> i64 {
        i64::from(value)
    }
    i64_bool(value: i64) -> bool {
        value != 0
    }
    i64_usize(value: i64) -> usize {
        value.max(0) as usize
    }
    i64_u64(value: i64) -> u64 {
        value.max(0) as u64
    }
    i64_u128(value: i64) -> u128 {
        value.max(0) as u128
    }
}

pub fn hash_hint(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hasher.finalize()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

// Session Ledger (bfu): rocket-equation token-turn pricing (mass × turns remaining).
// Spec: ZeroStack Mars doc section 0 — DPMT is the headline metric.
pub const SESSION_LEDGER_SCHEMA_VERSION: &str = "session-ledger-v3";

/// Token-turns for a chronological mass series: mass at turn index `i` (0-based)
/// of `N` turns contributes `mass * (N - i)` (rides for remaining turns including current).
pub fn token_turns_for_masses(masses: &[usize]) -> u64 {
    let n = masses.len();
    masses
        .iter()
        .enumerate()
        .map(|(i, &mass)| {
            let turns_remaining = n.saturating_sub(i) as u64;
            (mass as u64).saturating_mul(turns_remaining)
        })
        .fold(0u64, u64::saturating_add)
}

/// Decisions per million token-turns. `decisions` is the turn/tool-call count until a
/// finer decision counter exists. Returns `None` when `token_turns == 0`.
pub fn dpmt(decisions: usize, token_turns: u64) -> Option<f64> {
    if token_turns == 0 {
        None
    } else {
        Some((decisions as f64) * 1_000_000.0 / (token_turns as f64))
    }
}

pulse_structs! {
    #[derive(Default)]
    SessionLedgerEntry {
        session_id String;
        tokenizer_id String;
        /// estimator | tiktoken | exact | empty
        #[serde(default)] counts_class String;
        /// True only for ExactTokenizerIdentity rows (provider/model@hex).
        #[serde(default)] certified bool;
        turns usize;
        raw_tokens usize;
        visible_tokens usize;
        recovery_tokens usize;
        exact_ref_count usize;
        failures usize;
        cache_hits usize;
        /// Visible mass × turns_remaining (rocket-equation carried cost).
        visible_token_turns u64;
        /// Recovery mass (M_rec) × turns_remaining.
        recovery_token_turns u64;
        /// Recovery-adjusted cost: visible_token_turns + recovery_token_turns.
        recovery_adjusted_token_turn_cost u64;
        /// Raw mass × turns_remaining (same schedule, uncompressed mass).
        raw_token_turns u64;
        /// M_full - (M_vis + M_rec); intentionally signed and may be negative.
        token_turn_savings i128;
        /// Signed savings ratio over raw token-turns.
        recovery_adjusted_savings f64;
        /// Decisions per million visible token-turns; absent when token-turns are zero.
        #[serde(default, skip_serializing_if = "Option::is_none")] dpmt Option<f64>;
        tools BTreeMap<String, usize>;
        source_hash Option<String>;
    }
    SessionLedgerReport {
        schema_version String;
        /// Common tokenizer_id, `mixed` when sessions disagree, empty when none.
        tokenizer_id String;
        /// estimator | tiktoken | exact | mixed | empty
        counts_class String;
        /// True only when every row is the same ExactTokenizerIdentity.
        /// Estimator and tiktoken totals never certify as exact/Q99.
        certified bool;
        /// False when tokenizer ids mix; headline savings/DPMT are not one-unit.
        savings_commensurate bool;
        total_sessions usize;
        total_turns usize;
        total_raw_tokens usize;
        total_visible_tokens usize;
        total_recovery_tokens usize;
        total_exact_refs usize;
        total_failures usize;
        total_cache_hits usize;
        total_visible_token_turns u64;
        total_recovery_token_turns u64;
        total_recovery_adjusted_token_turn_cost u64;
        total_raw_token_turns u64;
        total_token_turn_savings i128;
        total_recovery_adjusted_savings f64;
        /// Headline metric: decisions per million visible token-turns (DPMT).
        #[serde(default, skip_serializing_if = "Option::is_none")] dpmt Option<f64>;
        sessions Vec<SessionLedgerEntry>;
    }
}

#[derive(Default)]
struct SessionAcc {
    entry: SessionLedgerEntry,
}

fn signed_savings_ratio(raw: u64, charged: u64) -> f64 {
    savings_ratio_u64(raw, charged)
}

impl SessionLedgerReport {
    pub fn from_ledger(path: &Path) -> IoResult<Self> {
        let mut timelines: BTreeMap<String, Vec<PulseEvent>> = BTreeMap::new();
        scan_jsonl(path, |event| {
            let session_id = event
                .session_id
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            timelines.entry(session_id).or_default().push(event.clone());
            Ok(())
        })?;

        let mut sessions: BTreeMap<(String, String), SessionAcc> = BTreeMap::new();
        for (session_id, events) in timelines {
            let horizon = events.len();
            for (index, event) in events.into_iter().enumerate() {
                let tokenizer_id = event.tokenizer_id.clone();
                let key = (session_id.clone(), tokenizer_id.clone());
                let acc = sessions.entry(key).or_insert_with(|| SessionAcc {
                    entry: SessionLedgerEntry {
                        session_id: session_id.clone(),
                        counts_class: pulse_counts_class(&tokenizer_id).to_string(),
                        certified: pulse_counts_certified(&tokenizer_id),
                        tokenizer_id,
                        source_hash: event.source_hash.clone(),
                        ..SessionLedgerEntry::default()
                    },
                });
                let remaining = horizon.saturating_sub(index) as u64;
                acc.entry.turns += 1;
                acc.entry.raw_tokens += event.raw_tokens;
                acc.entry.visible_tokens += event.visible_tokens;
                acc.entry.recovery_tokens += event.recovery_tokens;
                acc.entry.visible_token_turns = acc
                    .entry
                    .visible_token_turns
                    .saturating_add((event.visible_tokens as u64).saturating_mul(remaining));
                acc.entry.recovery_token_turns = acc
                    .entry
                    .recovery_token_turns
                    .saturating_add((event.recovery_tokens as u64).saturating_mul(remaining));
                acc.entry.raw_token_turns = acc
                    .entry
                    .raw_token_turns
                    .saturating_add((event.raw_tokens as u64).saturating_mul(remaining));
                acc.entry.exact_ref_count += event.exact_ref_count;
                acc.entry.failures += usize::from(event.failure);
                acc.entry.cache_hits += usize::from(event.cache_hit);
                *acc.entry.tools.entry(event.tool).or_insert(0) += 1;
            }
        }

        let sessions_vec: Vec<SessionLedgerEntry> = sessions
            .into_values()
            .map(|mut acc| {
                acc.entry.recovery_adjusted_token_turn_cost = acc
                    .entry
                    .visible_token_turns
                    .saturating_add(acc.entry.recovery_token_turns);
                acc.entry.token_turn_savings = i128::from(acc.entry.raw_token_turns)
                    - i128::from(acc.entry.recovery_adjusted_token_turn_cost);
                acc.entry.recovery_adjusted_savings = signed_savings_ratio(
                    acc.entry.raw_token_turns,
                    acc.entry.recovery_adjusted_token_turn_cost,
                );
                acc.entry.dpmt = dpmt(acc.entry.turns, acc.entry.recovery_adjusted_token_turn_cost);
                acc.entry
            })
            .collect();
        let sum = |f: fn(&SessionLedgerEntry) -> usize| sessions_vec.iter().map(f).sum::<usize>();
        let sum_u64 = |f: fn(&SessionLedgerEntry) -> u64| {
            sessions_vec.iter().map(f).fold(0u64, u64::saturating_add)
        };
        let total_turns = sum(|entry| entry.turns);
        let total_visible_token_turns = sum_u64(|entry| entry.visible_token_turns);
        let total_recovery_token_turns = sum_u64(|entry| entry.recovery_token_turns);
        let total_recovery_adjusted_token_turn_cost =
            total_visible_token_turns.saturating_add(total_recovery_token_turns);
        let total_raw_token_turns = sum_u64(|entry| entry.raw_token_turns);
        let tokenizer_ids: BTreeSet<String> = sessions_vec
            .iter()
            .map(|entry| entry.tokenizer_id.clone())
            .collect();
        let (tokenizer_id, counts_class, certified, commensurate) =
            seal_report_tokenizers(&tokenizer_ids);
        Ok(Self {
            schema_version: SESSION_LEDGER_SCHEMA_VERSION.to_string(),
            tokenizer_id,
            counts_class,
            certified,
            savings_commensurate: commensurate,
            total_sessions: sessions_vec.len(),
            total_turns,
            total_raw_tokens: sum(|entry| entry.raw_tokens),
            total_visible_tokens: sum(|entry| entry.visible_tokens),
            total_recovery_tokens: sum(|entry| entry.recovery_tokens),
            total_exact_refs: sum(|entry| entry.exact_ref_count),
            total_failures: sum(|entry| entry.failures),
            total_cache_hits: sum(|entry| entry.cache_hits),
            total_visible_token_turns,
            total_recovery_token_turns,
            total_recovery_adjusted_token_turn_cost,
            total_raw_token_turns,
            total_token_turn_savings: i128::from(total_raw_token_turns)
                - i128::from(total_recovery_adjusted_token_turn_cost),
            total_recovery_adjusted_savings: signed_savings_ratio(
                total_raw_token_turns,
                total_recovery_adjusted_token_turn_cost,
            ),
            // Headline DPMT over mixed tokenizer ids is not one billed unit.
            dpmt: if commensurate {
                dpmt(total_turns, total_recovery_adjusted_token_turn_cost)
            } else {
                None
            },
            sessions: sessions_vec,
        })
    }

    pub fn schema_json() -> serde_json::Value {
        serde_json::json!({
            "schema_version": SESSION_LEDGER_SCHEMA_VERSION,
            "description": "Recovery-adjusted per-session token-turn ledger keyed by tokenizer id",
            "privacy": {
                "scope": "local Pulse JSONL/SQLite and explicit local exports; no uploader exists",
                "session_id": "stored verbatim in Pulse events and exposed as the session aggregate key; correlatable and not anonymized",
                "call_id": "stored verbatim in Pulse events; omitted from this aggregate report",
                "ref_ids": "stored verbatim in Pulse events as stable local join keys; this aggregate exposes only exact_ref_count",
                "source_hash": "public event field is unvalidated; tool_call normally stores the first 64 bits (16 hex characters) of source_hint SHA-256, which is correlatable, guessable for low-entropy hints, and collision-prone",
                "upload": "none"
            },
            "pricing": {
                "token_turns": "M_vis and M_rec each sum mass_i * (N - i); recovery_adjusted_token_turn_cost = M_vis + M_rec",
                "token_turn_savings": "M_full - (M_vis + M_rec); signed and may be negative",
                "dpmt": "decisions * 1e6 / recovery_adjusted_token_turn_cost"
            },
            "entry": {
                "session_id": "string — verbatim local Pulse session identifier (MCP session id or 'unknown'); correlatable and not anonymized",
                "tokenizer_id": "estimator:<slug>, tiktoken:<encoding-slug>, or provider/model@<64 lowercase hex identity digest>. tiktoken: is bundled BPE (certified for that vocab) and is not ExactTokenizerIdentity. Built-in tool_call counts use estimator:tokenzero-core until an exact adapter is linked",
                "counts_class": "estimator | tiktoken | exact | empty — never Q99",
                "certified": "bool — true only for ExactTokenizerIdentity (provider/model@hex); estimator and tiktoken rows are false",
                "turns": "usize — number of tool calls in this session (decision count proxy)",
                "raw_tokens": "usize — total raw (uncompressed) tokens across all turns",
                "visible_tokens": "usize — total visible (compressed) tokens across all turns",
                "recovery_tokens": "usize — tokens recovered via expand (charged back to original serve)",
                "exact_ref_count": "usize — total exact refs emitted across all turns",
                "failures": "usize — number of failed tool calls",
                "cache_hits": "usize — number of cache-hit serves",
                "visible_token_turns": "u64 — M_vis",
                "recovery_token_turns": "u64 — M_rec",
                "recovery_adjusted_token_turn_cost": "u64 — M_vis + M_rec",
                "raw_token_turns": "u64 — M_full",
                "token_turn_savings": "i128 — M_full - M_vis - M_rec",
                "recovery_adjusted_savings": "f64 — signed ratio; may be negative",
                "dpmt": "Option<f64> — decisions per million recovery-adjusted token-turns",
                "tools": "BTreeMap<String, usize> — per-tool call counts",
                "source_hash": "Option<String> — unvalidated public event field; tool_call normally supplies a 64-bit truncated SHA-256 correlation digest"
            },
            "report": {
                "schema_version": SESSION_LEDGER_SCHEMA_VERSION,
                "tokenizer_id": "common id, mixed, or empty — never unlabeled estimate: or Q99",
                "counts_class": "estimator | tiktoken | exact | mixed | empty",
                "certified": "bool — true only when every row is the same provider/model@hex identity",
                "savings_commensurate": "bool — false when tokenizer ids mix; headline savings/DPMT then must not be read as one-unit exact/Q99",
                "total_sessions": "usize",
                "total_turns": "usize",
                "total_raw_tokens": "usize",
                "total_visible_tokens": "usize",
                "total_recovery_tokens": "usize",
                "total_exact_refs": "usize",
                "total_failures": "usize",
                "total_cache_hits": "usize",
                "total_visible_token_turns": "u64",
                "total_recovery_token_turns": "u64",
                "total_recovery_adjusted_token_turn_cost": "u64",
                "total_raw_token_turns": "u64",
                "total_token_turn_savings": "i128",
                "total_recovery_adjusted_savings": "f64",
                "dpmt": "Option<f64> — headline DPMT across all sessions",
                "sessions": "Vec<SessionLedgerEntry>"
            },
            "cli": {
                "stats": "tokenzero session-ledger stats [--json] [--root PATH]",
                "export": "tokenzero session-ledger export [--json] [--root PATH]",
                "schema": "tokenzero session-ledger schema"
            }
        })
    }

    pub fn render_text(&self) -> String {
        let mut out = String::from(
            "Session Cost Ledger (session-ledger-v3)\n═══════════════════════════════════════\n\n",
        );
        let tokenizer = if self.tokenizer_id.is_empty() {
            "-"
        } else {
            self.tokenizer_id.as_str()
        };
        writeln!(
            out,
            "Tokenizer: {tokenizer}  counts_class={}  certified={}  commensurate={}",
            self.counts_class, self.certified, self.savings_commensurate
        )
        .unwrap();
        match self.dpmt {
            Some(dpmt) => writeln!(
                out,
                "DPMT (headline): {dpmt:.4} decisions / million recovery-adjusted token-turns"
            )
            .unwrap(),
            None if !self.savings_commensurate => {
                out.push_str("DPMT (headline): n/a (mixed tokenizer ids; not commensurate)\n")
            }
            None => out.push_str("DPMT (headline): n/a (no recovery-adjusted token-turns)\n"),
        }
        writeln!(
            out,
            "Token-turns: visible={} recovery={} adjusted={} raw={} net_savings={} ({:.2}%)",
            self.total_visible_token_turns,
            self.total_recovery_token_turns,
            self.total_recovery_adjusted_token_turn_cost,
            self.total_raw_token_turns,
            self.total_token_turn_savings,
            self.total_recovery_adjusted_savings * 100.0,
        )
        .unwrap();
        writeln!(
            out,
            "Sessions: {}  Turns: {}  Raw: {}  Visible: {}  Recovered: {}  Refs: {}  Failures: {}\n",
            self.total_sessions,
            self.total_turns,
            self.total_raw_tokens,
            self.total_visible_tokens,
            self.total_recovery_tokens,
            self.total_exact_refs,
            self.total_failures,
        )
        .unwrap();
        out.push_str("Per-session/tokenizer breakdown:\n───────────────────────────────────────\n");
        for entry in &self.sessions {
            let dpmt = entry
                .dpmt
                .map(|value| format!("{value:.4}"))
                .unwrap_or_else(|| "n/a".to_string());
            writeln!(
                out,
                "  {} [{}] — turns={} adjusted_tt={} (visible={} recovery={}) raw_tt={} net={} ({:.2}%) dpmt={} refs={} failures={}",
                entry.session_id,
                entry.tokenizer_id,
                entry.turns,
                entry.recovery_adjusted_token_turn_cost,
                entry.visible_token_turns,
                entry.recovery_token_turns,
                entry.raw_token_turns,
                entry.token_turn_savings,
                entry.recovery_adjusted_savings * 100.0,
                dpmt,
                entry.exact_ref_count,
                entry.failures,
            )
            .unwrap();
            out.push_str("    tools: ");
            for (index, (tool, count)) in entry.tools.iter().enumerate() {
                write!(out, "{}{tool}:{count}", if index == 0 { "" } else { ", " }).unwrap();
            }
            out.push('\n');
        }
        out
    }
}
