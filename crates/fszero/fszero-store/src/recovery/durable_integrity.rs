//! Independent, fail-closed durable-store integrity gate. Stock SQLite
//! validates an existing DB/WAL snapshot before fsqlite can open or
//! mutate it. Failure recovery only writes unique sibling destinations.

use rusqlite::{Connection as OracleConnection, ErrorCode, OpenFlags, types::Value};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{PAYLOAD_TAG_INLINE, decode_packed_locator, pack_gen_path};

const GATE_VERSION: u32 = 5;
const FSQLITE_VERSION: &str = "0.1.19";
const VULNERABLE_FSQLITE_VERSION: &str = "0.1.15";
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const FORCE_ENV: &str = "FSZERO_FORCE_INTEGRITY_CHECK";
/// Upper bound on forensic+salvage siblings kept next to one store. These are
/// full copies of the DB; unbounded creation once cost 32 GB across 656 pairs.
const MAX_SNAPSHOT_DESTINATIONS: usize = 3;
/// Byte budget for retained forensic/salvage siblings. A count cap alone does not
/// bound bytes: each sibling is a full copy of the store, so 120 siblings of one
/// 640 MB store reached 61 GB. Oldest siblings are pruned back to this budget.
const SNAPSHOT_RETENTION_ENV: &str = "FSZERO_SNAPSHOT_RETENTION_BYTES";
const DEFAULT_SNAPSHOT_RETENTION_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Records which store state a forensic snapshot captured, so a later refusal
/// of the same state can recognise its own earlier copy instead of remaking it.
const FINGERPRINT_FILE: &str = "STORE-FINGERPRINT.json";

const CURRENT_TABLES: &str = "
CREATE TABLE payloads (key TEXT PRIMARY KEY, value BLOB);
CREATE TABLE payload_lru (key TEXT PRIMARY KEY, tick INTEGER);
CREATE TABLE meta (k TEXT PRIMARY KEY, v INTEGER);
CREATE TABLE integrity_state (id INTEGER PRIMARY KEY, violations INTEGER NOT NULL, detail TEXT NOT NULL);
CREATE TABLE edit_intents (id INTEGER PRIMARY KEY, root TEXT NOT NULL, path TEXT NOT NULL, state TEXT NOT NULL, pre BLOB NOT NULL, post BLOB NOT NULL, pre_ref TEXT NOT NULL, post_ref TEXT NOT NULL, pre_mtime_ns INTEGER NOT NULL, pre_mode INTEGER NOT NULL, pre_xattrs TEXT NOT NULL, created_ns INTEGER NOT NULL);
CREATE TABLE mutation_log (seq INTEGER PRIMARY KEY, ts INTEGER NOT NULL, op TEXT NOT NULL, path TEXT NOT NULL, pre_ref TEXT NOT NULL, post_ref TEXT NOT NULL, created INTEGER NOT NULL DEFAULT 0, session_window INTEGER NOT NULL DEFAULT 0, agent TEXT NOT NULL DEFAULT '', pre_mtime_ns INTEGER NOT NULL DEFAULT 0, pre_mode INTEGER NOT NULL DEFAULT -1, pre_xattrs TEXT NOT NULL DEFAULT '');
CREATE TABLE ast_nodes (id INTEGER PRIMARY KEY, file_key TEXT, kind TEXT, span_start INTEGER, span_end INTEGER, symbol TEXT, parent INTEGER, version INTEGER DEFAULT 0);
CREATE TABLE ast_edges (src INTEGER, dst INTEGER, kind TEXT);
CREATE TABLE call_edges (file_key TEXT, caller TEXT, callee TEXT, line INTEGER, version INTEGER DEFAULT 0, PRIMARY KEY(file_key, caller, callee, line, version));
CREATE TABLE facts (subject_ref TEXT, predicate TEXT, object_ref TEXT, evidence_ref TEXT, version INTEGER, agent TEXT, PRIMARY KEY(subject_ref, predicate, object_ref, evidence_ref, version, agent));
CREATE TABLE worlds (wid TEXT PRIMARY KEY, state TEXT NOT NULL, cert_ref TEXT NOT NULL, created_ts INTEGER NOT NULL, session_window INTEGER NOT NULL DEFAULT 0);
CREATE TABLE world_edits (wid TEXT NOT NULL, ord INTEGER NOT NULL, path TEXT NOT NULL, cert_ref TEXT NOT NULL, PRIMARY KEY (wid, ord));
CREATE TABLE memory_paths (path TEXT PRIMARY KEY, store_key TEXT NOT NULL, content_ref TEXT NOT NULL, updated_ts INTEGER NOT NULL);
CREATE TABLE pack_validation_pending (key TEXT PRIMARY KEY, generation INTEGER NOT NULL, offset INTEGER NOT NULL, len INTEGER NOT NULL);
CREATE TABLE store_migrations (name TEXT PRIMARY KEY, version INTEGER NOT NULL, cursor TEXT NOT NULL);
CREATE TABLE memory_backfill_pending (store_key TEXT PRIMARY KEY);
CREATE TABLE chunk_blobs (digest TEXT PRIMARY KEY, content_ref TEXT NOT NULL, len INTEGER NOT NULL);
CREATE TABLE file_chunks (path TEXT NOT NULL, ordinal INTEGER NOT NULL, start_byte INTEGER NOT NULL, end_byte INTEGER NOT NULL, digest TEXT NOT NULL, content_ref TEXT NOT NULL, PRIMARY KEY(path, ordinal));
CREATE TABLE access_log (ts INTEGER NOT NULL, op TEXT NOT NULL, path TEXT NOT NULL, content_hash TEXT NOT NULL, session_window INTEGER NOT NULL);
";

const CURRENT_INDEXES: &str = "
CREATE INDEX idx_payload_lru_tick ON payload_lru(tick);
CREATE INDEX idx_mutation_log_path ON mutation_log(path);
CREATE INDEX idx_ast_nodes_symbol ON ast_nodes(kind, symbol);
CREATE INDEX idx_ast_nodes_file ON ast_nodes(file_key, version);
CREATE INDEX idx_call_edges_callee ON call_edges(callee, version);
CREATE INDEX idx_facts_subject ON facts(subject_ref, predicate);
CREATE INDEX idx_worlds_state ON worlds(state);
CREATE INDEX idx_memory_paths_prefix ON memory_paths(path);
CREATE INDEX idx_file_chunks_digest ON file_chunks(digest);
CREATE INDEX idx_access_log_path ON access_log(path);
CREATE INDEX idx_access_log_ts ON access_log(ts);
CREATE INDEX idx_access_log_window ON access_log(session_window);
";

#[derive(Debug, Serialize, Deserialize)]
struct Attestation {
    gate_version: u32,
    fsqlite_version: String,
    fingerprint: BTreeMap<String, String>,
}

#[derive(Debug, Default, Serialize)]
struct TableSalvage {
    imported: u64,
    duplicate: u64,
    unreadable: u64,
    failed: u64,
}

#[derive(Debug, Default, Serialize)]
struct PayloadValidation {
    source_count: Option<u64>,
    destination_count: u64,
    count_matches: bool,
    order_matches: bool,
    readable_locators: u64,
    unreadable_locators: u64,
    verified_content_hashes: u64,
    invalid_content_hashes: u64,
    status: String,
}

#[derive(Debug, Serialize)]
struct SalvageReport {
    tables: BTreeMap<String, TableSalvage>,
    integrity_check: Vec<String>,
    destination_integrity_ok: bool,
    payload: PayloadValidation,
    verified: bool,
    caveat: &'static str,
}

#[derive(Debug)]
pub(super) struct IntegrityGuard {
    _connection: OracleConnection,
}

/// Typed integrity-gate refusal. The variant distinguishes repair failure, data loss,
/// and contention; the payload carries operator detail.
#[derive(Debug)]
pub(super) enum GateError {
    /// Findings were repairable in principle but the repair did not hold.
    Benign(String),
    /// Findings imply data loss; the source was quarantined, not repaired.
    Destructive(String),
    /// Another writer holds the store right now. Retrying is correct.
    Busy(String),
    /// The store could not be opened or inspected at all.
    OpenFailed(String),
}

impl GateError {
    /// True only for contention, which SQLite defines as retryable.
    pub(super) fn is_busy(&self) -> bool {
        matches!(self, Self::Busy(_))
    }

    /// Destructive findings plus "this is not a SQLite file at all".
    /// Permission and missing-parent failures stay fail-closed because their stores
    /// cannot be replaced safely.
    pub(super) fn is_resettable_live_file(&self) -> bool {
        match self {
            Self::Destructive(_) => true,
            Self::OpenFailed(detail) => {
                let lower = detail.to_ascii_lowercase();
                !lower.contains("permission")
                    && !lower.contains("read-only")
                    && (lower.contains("file is not a database")
                        || lower.contains("disk image is malformed")
                        || lower.contains("not a database")
                        || lower.contains("unsupported file format"))
            }
            Self::Benign(_) | Self::Busy(_) => false,
        }
    }

    fn detail(&self) -> &str {
        match self {
            Self::Benign(detail)
            | Self::Destructive(detail)
            | Self::Busy(detail)
            | Self::OpenFailed(detail) => detail,
        }
    }
}

impl std::fmt::Display for GateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.detail())
    }
}

fn is_busy_error(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

pub(super) fn gate_existing_store(db_path: &Path) -> Result<IntegrityGuard, GateError> {
    // Snapshot siblings are only ever created by this gate, so store open is the one
    // place that always observes them. Pruning here keeps the bound automatic rather
    // than a manual cleanup step, and failures must never block a healthy store from opening.
    let _ = prune_snapshot_destinations(db_path);
    // The gate takes a writer-excluding lock, so under multi-engine contention it is the first thing
    // to block.
    gate_existing_store_with_timeout(
        db_path,
        super::durable_busy_attempt_wait().min(BUSY_TIMEOUT),
    )
}

fn gate_existing_store_with_timeout(
    db_path: &Path,
    timeout: Duration,
) -> Result<IntegrityGuard, GateError> {
    if !db_path.is_file() {
        return Err(GateError::OpenFailed(format!(
            "store missing: {}",
            db_path.display()
        )));
    }
    let conn = OracleConnection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| {
        GateError::OpenFailed(failure_without_snapshot(
            db_path,
            format!("stock SQLite open failed: {error}"),
        ))
    })?;
    conn.busy_timeout(timeout).map_err(|error| {
        GateError::OpenFailed(failure_without_snapshot(
            db_path,
            format!("busy timeout setup failed: {error}"),
        ))
    })?;
    conn.execute_batch("BEGIN IMMEDIATE").map_err(|error| {
        let detail = failure_without_snapshot(
            db_path,
            format!("writer-excluding lock failed after {timeout:?}: {error}"),
        );
        if is_busy_error(&error) {
            GateError::Busy(detail)
        } else {
            GateError::OpenFailed(detail)
        }
    })?;

    // FSZero appends a pack only while holding the same SQLite write
    // transaction. Once BEGIN IMMEDIATE succeeds, DB/WAL and all generations
    // therefore form a stable snapshot until this transaction is released.
    gate_while_locked(&conn, db_path)?;
    Ok(IntegrityGuard { _connection: conn })
}

/// The non-`ok` lines of an `integrity_check` report. SQLite returns the whole report as
/// a single row with embedded newlines, led by a `*** in database main ***` banner.
/// Classifying the raw rows would therefore see one opaque blob and never recognise anything.
fn integrity_findings(rows: &[String]) -> Vec<&str> {
    rows.iter()
        .flat_map(|row| row.lines())
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && *line != "ok"
                && !(line.starts_with("*** ") && line.ends_with(" ***"))
        })
        .collect()
}

/// True for findings stock SQLite can repair without losing rows. Three shapes were observed on
/// real shared stores, all structural rather than data loss * `Page N: never used` - fsqlite grew
/// the page count without linking the tail pages into the freelist.
fn is_repairable_finding(row: &str) -> bool {
    is_leaked_page_finding(row)
        || is_row_order_finding(row)
        || row.starts_with("wrong # of entries in index ")
}

/// `Page N: never used`.
fn is_leaked_page_finding(row: &str) -> bool {
    let Some(rest) = row.strip_prefix("Page ") else {
        return false;
    };
    let Some(number) = rest.strip_suffix(": never used") else {
        return false;
    };
    !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
}

/// `rowid N out of order`, however SQLite prefixes it with page location.
fn is_row_order_finding(row: &str) -> bool {
    row.contains("rowid ") && row.ends_with(" out of order")
}

fn gate_while_locked(conn: &OracleConnection, db_path: &Path) -> Result<(), GateError> {
    let fingerprint = fingerprint_store(db_path)
        .map_err(|error| GateError::Destructive(quarantine_locked(db_path, error)))?;
    let force = std::env::var_os(FORCE_ENV).is_some();
    if read_attestation(db_path).is_some_and(|attestation| {
        attestation_matches(&attestation, &fingerprint, FSQLITE_VERSION, force)
    }) {
        return Ok(());
    }

    let rows = integrity_rows(conn)
        .map_err(|error| GateError::Destructive(quarantine_locked(db_path, error)))?;
    let fingerprint = if rows.as_slice() == ["ok"] {
        fingerprint
    } else {
        self_heal_repairable_findings(conn, db_path, &rows)?
    };
    write_attestation(
        db_path,
        &Attestation {
            gate_version: GATE_VERSION,
            fsqlite_version: FSQLITE_VERSION.to_string(),
            fingerprint,
        },
    )
    .map_err(GateError::OpenFailed)
}

/// Repair a purely leaked-page report in place, or quarantine. Quarantining a benign finding was
/// catastrophic: every process that opened the store made a full raw+logical+salvage copy of it, so
/// N respawns cost N copies of the DB and its packs.
fn self_heal_repairable_findings(
    conn: &OracleConnection,
    db_path: &Path,
    rows: &[String],
) -> Result<BTreeMap<String, String>, GateError> {
    let findings = integrity_findings(rows);
    if findings.is_empty() || !findings.iter().all(|row| is_repairable_finding(row)) {
        return Err(GateError::Destructive(quarantine_locked(
            db_path,
            format!("stock SQLite PRAGMA main.integrity_check failed: {rows:?}"),
        )));
    }

    // Row counts are the loss oracle: VACUUM rewrites every b-tree and REINDEX
    // rebuilds every index, so a mis-ordered table that silently drops rows must
    // not be attested as healthy just because the report went quiet.
    let before_counts = table_row_counts(conn)
        .map_err(|error| GateError::Destructive(quarantine_locked(db_path, error)))?;

    // VACUUM cannot run inside the gate's BEGIN IMMEDIATE, so commit, repair, and immediately
    // re-acquire. The lock is briefly released, which is why integrity_check is re-run under
    // the re-acquired lock below rather than trusting the repair to have been the last write.
    if let Err(error) = conn.execute_batch("COMMIT; VACUUM; REINDEX; BEGIN IMMEDIATE") {
        return Err(GateError::Destructive(quarantine_locked(
            db_path,
            format!("integrity repair failed: VACUUM/REINDEX: {error}; findings: {findings:?}"),
        )));
    }

    let repaired = integrity_rows(conn)
        .map_err(|error| GateError::Destructive(quarantine_locked(db_path, error)))?;
    if repaired.as_slice() != ["ok"] {
        return Err(GateError::Benign(quarantine_locked(
            db_path,
            format!("integrity repair did not clear: before {findings:?}; after {repaired:?}"),
        )));
    }

    let after_counts = table_row_counts(conn)
        .map_err(|error| GateError::Destructive(quarantine_locked(db_path, error)))?;
    if let Some(lost) = lost_rows(&before_counts, &after_counts) {
        return Err(GateError::Destructive(quarantine_locked(
            db_path,
            format!("integrity repair lost rows: {lost}; findings: {findings:?}"),
        )));
    }
    eprintln!(
        "fszero durable integrity gate: self-healed {} finding(s) via VACUUM/REINDEX; source={}",
        findings.len(),
        db_path.display()
    );
    fingerprint_store(db_path)
        .map_err(|error| GateError::Destructive(quarantine_locked(db_path, error)))
}

/// Row count per user table, keyed by table name.
fn table_row_counts(conn: &OracleConnection) -> Result<BTreeMap<String, u64>, String> {
    let mut statement = conn
        .prepare(
            "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .map_err(|error| format!("list tables: {error}"))?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("list tables: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("list tables: {error}"))?;
    drop(statement);

    let mut counts = BTreeMap::new();
    for name in names {
        let count: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {}", quote_identifier(&name)),
                [],
                |row| row.get(0),
            )
            .map_err(|error| format!("count {name}: {error}"))?;
        counts.insert(name, count.max(0) as u64);
    }
    Ok(counts)
}

/// The first table that lost rows or disappeared across a repair, if any.
fn lost_rows(before: &BTreeMap<String, u64>, after: &BTreeMap<String, u64>) -> Option<String> {
    before.iter().find_map(|(name, before_count)| {
        let after_count = after.get(name).copied().unwrap_or(0);
        (after_count < *before_count).then(|| format!("{name}: {before_count} -> {after_count}"))
    })
}

fn attestation_matches(
    attestation: &Attestation,
    fingerprint: &BTreeMap<String, String>,
    writer_version: &str,
    force: bool,
) -> bool {
    !force
        && writer_version != VULNERABLE_FSQLITE_VERSION
        && attestation.gate_version == GATE_VERSION
        && attestation.fsqlite_version == writer_version
        && attestation.fingerprint == *fingerprint
}

fn integrity_rows(conn: &OracleConnection) -> Result<Vec<String>, String> {
    let mut statement = conn
        .prepare("PRAGMA main.integrity_check")
        .map_err(|error| format!("integrity_check prepare failed: {error}"))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("integrity_check query failed: {error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("integrity_check row failed: {error}"))
}

fn failure_without_snapshot(db_path: &Path, reason: String) -> String {
    format!(
        "durable integrity gate failed closed before a coherent writer-excluded snapshot could be acquired; source was not modified and no unlocked forensic copy was created: {reason}; source={}",
        db_path.display()
    )
}

fn quarantine_locked(db_path: &Path, reason: String) -> String {
    match create_forensic_and_salvage(db_path) {
        Ok((forensic, salvage, report)) => format!(
            "durable integrity gate rejected the source: {reason}; source was not repaired or replaced; forensic={}; salvage={}; report={}; salvage may contain data loss, was not auto-promoted, and must be reviewed before use",
            forensic.display(),
            salvage.display(),
            report.display()
        ),
        Err(error) => format!(
            "durable integrity gate rejected the source: {reason}; source was not repaired or replaced; forensic/salvage failed: {error}"
        ),
    }
}

fn create_forensic_and_salvage(db_path: &Path) -> Result<(PathBuf, PathBuf, PathBuf), String> {
    let fingerprint = snapshot_identity(db_path)?;
    let fingerprint_bytes =
        serde_json::to_vec(&fingerprint).map_err(|error| format!("render fingerprint: {error}"))?;
    // One store state needs one snapshot, not one per process that opened it.
    // Without this, a store that every respawn rejects produced a full copy of
    // the DB and its packs per PID.
    if let Some(previous) = existing_forensic_with_fingerprint(db_path, &fingerprint_bytes)? {
        return Err(format!(
            "an identical forensic snapshot of this exact store state already exists at {}; skipping duplicate copy",
            previous.display()
        ));
    }
    // A forensic+salvage pair consumes two sibling directories. Prune oldest
    // first so a stuck store can still retain new evidence instead of refusing
    // forever once the cap is full.
    make_room_for_snapshot_pair(db_path)?;
    let existing = existing_snapshot_destinations(db_path)?;
    if existing.len() + 2 > MAX_SNAPSHOT_DESTINATIONS {
        return Err(format!(
            "refusing to create another forensic/salvage snapshot: {} already exist (cap {MAX_SNAPSHOT_DESTINATIONS}); review and remove them first: {}",
            existing.len(),
            existing
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let forensic = unique_destination(db_path, "forensic")?;
    fs::create_dir(&forensic).map_err(|error| format!("create forensic destination: {error}"))?;
    write_synced(
        &forensic.join("INCOMPLETE"),
        b"snapshot incomplete
",
    )?;

    write_synced(&forensic.join(FINGERPRINT_FILE), &fingerprint_bytes)?;

    let mut copied = Vec::new();
    for source in store_files(db_path)? {
        let name = source
            .file_name()
            .ok_or_else(|| "source file has no name".to_string())?;
        let destination = forensic.join(name);
        // Packs are content-addressed and are the bulk of the bytes (tens of
        // MB each). Hardlink them so a snapshot costs an inode, not a copy;
        // fall back to a copy when the link cannot be made (cross-device).
        let linked = name.to_string_lossy().contains(".pack")
            && fs::hard_link(&source, &destination).is_ok();
        if !linked {
            fs::copy(&source, &destination)
                .map_err(|error| format!("copy {}: {error}", source.display()))?;
            File::open(&destination)
                .and_then(|file| file.sync_all())
                .map_err(|error| format!("fsync {}: {error}", destination.display()))?;
        }
        copied.push(destination);
    }
    let logical_snapshot = forensic.join("logical-snapshot.sqlite3");
    create_logical_snapshot(db_path, &logical_snapshot).map_err(|error| {
        format!(
            "raw forensic files retained under {} with INCOMPLETE marker; coherent SQLite snapshot failed: {error}",
            forensic.display()
        )
    })?;
    copied.push(logical_snapshot);
    copied.sort();
    let mut manifest_bytes = Vec::new();
    for path in &copied {
        writeln!(
            manifest_bytes,
            "{}  {}",
            hash_file(path)?,
            path.file_name().unwrap_or_default().to_string_lossy()
        )
        .map_err(|error| format!("render manifest: {error}"))?;
    }
    write_synced(&forensic.join("SHA256SUMS"), &manifest_bytes)?;
    sync_dir(&forensic)?;
    fs::remove_file(forensic.join("INCOMPLETE"))
        .map_err(|error| format!("remove incomplete marker: {error}"))?;
    sync_dir(&forensic)?;

    let salvage = unique_destination(db_path, "salvage")?;
    fs::create_dir(&salvage).map_err(|error| format!("create salvage destination: {error}"))?;
    let report = salvage_database(db_path, &forensic, &salvage).map_err(|error| {
        format!(
            "forensic snapshot retained at {}; salvage destination {} is incomplete: {error}",
            forensic.display(),
            salvage.display()
        )
    })?;
    Ok((forensic, salvage, report))
}

fn create_logical_snapshot(db_path: &Path, destination: &Path) -> Result<(), String> {
    let source = OracleConnection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("open coherent snapshot source: {error}"))?;
    let mut target = OracleConnection::open(destination)
        .map_err(|error| format!("create coherent snapshot destination: {error}"))?;
    let backup = rusqlite::backup::Backup::new(&source, &mut target)
        .map_err(|error| format!("initialize coherent SQLite backup: {error}"))?;
    backup
        .run_to_completion(128, Duration::from_millis(10), None)
        .map_err(|error| format!("copy coherent SQLite backup: {error}"))?;
    drop(backup);
    drop(target);
    File::open(destination)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("fsync coherent SQLite snapshot: {error}"))
}

fn salvage_database(
    db_path: &Path,
    forensic: &Path,
    destination: &Path,
) -> Result<PathBuf, String> {
    let name = db_path
        .file_name()
        .ok_or_else(|| "database has no filename".to_string())?;
    let raw_source_path = forensic.join(name);
    let source_path = forensic.join("logical-snapshot.sqlite3");
    let destination_path = destination.join(name);
    let source = OracleConnection::open_with_flags(
        &source_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("open forensic copy for salvage: {error}"))?;
    let mut target = OracleConnection::open(&destination_path)
        .map_err(|error| format!("create salvage database: {error}"))?;
    target
        .execute_batch("PRAGMA journal_mode=DELETE; PRAGMA foreign_keys=OFF;")
        .map_err(|error| format!("initialize salvage database: {error}"))?;
    target
        .execute_batch(CURRENT_TABLES)
        .map_err(|error| format!("create current salvage schema: {error}"))?;

    let mut reports = BTreeMap::new();
    for table in current_table_names() {
        reports.insert(
            table.to_string(),
            salvage_table(&source, &mut target, table),
        );
    }
    target
        .execute_batch(CURRENT_INDEXES)
        .map_err(|error| format!("rebuild salvage indexes: {error}"))?;
    let integrity_check = integrity_rows(&target).unwrap_or_else(|error| vec![error]);
    let destination_integrity_ok = integrity_check.as_slice() == ["ok"];

    copy_pack_generations(&raw_source_path, destination)?;
    let payload = validate_payloads(&source, &target, &destination_path).unwrap_or_else(|error| {
        PayloadValidation {
            status: format!("unverified: {error}"),
            ..PayloadValidation::default()
        }
    });
    let rows_clean = reports
        .values()
        .all(|table| table.unreadable == 0 && table.failed == 0);
    let verified = destination_integrity_ok && rows_clean && payload.status == "verified";
    drop(target);

    let report_path = destination.join("salvage-report.json");
    let report = SalvageReport {
        tables: reports,
        integrity_check,
        destination_integrity_ok,
        payload,
        verified,
        caveat: "verified=true means stock integrity_check passed, all readable source rows imported without row errors, payload count/order matched, every packed locator was readable, and every full content-address key matched its bytes. verified=false is an explicit unverified/data-loss warning. Packs remain byte-for-byte copies. This destination is never auto-promoted.",
    };
    write_synced(
        &report_path,
        &serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
    )?;
    File::open(&destination_path)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("fsync salvage database: {error}"))?;
    sync_dir(destination)?;
    Ok(report_path)
}

fn current_table_names() -> &'static [&'static str] {
    &[
        "payloads",
        "payload_lru",
        "meta",
        "integrity_state",
        "edit_intents",
        "mutation_log",
        "ast_nodes",
        "ast_edges",
        "call_edges",
        "facts",
        "worlds",
        "world_edits",
        "memory_paths",
        "pack_validation_pending",
        "store_migrations",
        "memory_backfill_pending",
        "chunk_blobs",
        "file_chunks",
        "access_log",
    ]
}

fn salvage_table(
    source: &OracleConnection,
    target: &mut OracleConnection,
    table: &str,
) -> TableSalvage {
    let mut report = TableSalvage::default();
    let quoted = quote_identifier(table);
    let source_columns = match table_columns(source, table) {
        Ok(columns) if columns.is_empty() => return report,
        Ok(columns) => columns,
        Err(_) => {
            report.unreadable = 1;
            return report;
        }
    };
    let target_columns = match table_columns(target, table) {
        Ok(columns) => columns,
        Err(_) => {
            report.failed = 1;
            return report;
        }
    };
    let columns = target_columns
        .into_iter()
        .filter(|column| source_columns.contains(column))
        .collect::<Vec<_>>();
    if columns.is_empty() {
        report.failed = 1;
        return report;
    }
    let column_list = columns
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>()
        .join(",");
    let mut statement =
        match source.prepare(&format!("SELECT {column_list} FROM {quoted} NOT INDEXED")) {
            Ok(statement) => statement,
            Err(_) => {
                report.unreadable = 1;
                return report;
            }
        };
    let placeholders = (1..=columns.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(",");
    let insert = format!("INSERT INTO {quoted} ({column_list}) VALUES ({placeholders})");
    let transaction = match target.transaction() {
        Ok(transaction) => transaction,
        Err(_) => {
            report.failed = 1;
            return report;
        }
    };
    let mut rows = match statement.query([]) {
        Ok(rows) => rows,
        Err(_) => {
            report.unreadable = 1;
            return report;
        }
    };
    loop {
        match rows.next() {
            Ok(Some(row)) => {
                let values = (0..columns.len())
                    .map(|index| row.get::<_, Value>(index))
                    .collect::<Result<Vec<_>, _>>();
                let Ok(values) = values else {
                    report.unreadable += 1;
                    continue;
                };
                match transaction.execute(&insert, rusqlite::params_from_iter(values.iter())) {
                    Ok(0) => report.failed += 1,
                    Ok(_) => report.imported += 1,
                    Err(_) => report.failed += 1,
                }
            }
            Ok(None) => break,
            Err(_) => {
                report.unreadable += 1;
                break;
            }
        }
    }
    drop(rows);
    drop(statement);
    if transaction.commit().is_err() {
        report.failed += 1;
    }
    report
}

fn table_columns(conn: &OracleConnection, table: &str) -> Result<Vec<String>, String> {
    let mut statement = conn
        .prepare(&format!("PRAGMA table_info({})", quote_identifier(table)))
        .map_err(|error| format!("read {table} columns: {error}"))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| format!("query {table} columns: {error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("materialize {table} columns: {error}"))
}

fn validate_payloads(
    source: &OracleConnection,
    destination: &OracleConnection,
    destination_db: &Path,
) -> Result<PayloadValidation, String> {
    let source_keys = ordered_payload_keys(source).ok();
    let destination_keys = ordered_payload_keys(destination)?;
    let source_count = source_keys.as_ref().map(|keys| keys.len() as u64);
    let destination_count = destination_keys.len() as u64;
    let count_matches = source_count == Some(destination_count);
    let order_matches = source_keys.as_ref() == Some(&destination_keys);
    let generation = destination
        .query_row(
            "SELECT v FROM meta NOT INDEXED WHERE k = 'pack_gen'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0);
    let pack_path = pack_gen_path(destination_db, generation);
    let pack = File::open(&pack_path).ok();

    let mut validation = PayloadValidation {
        source_count,
        destination_count,
        count_matches,
        order_matches,
        ..PayloadValidation::default()
    };
    let mut statement = destination
        .prepare("SELECT key, value FROM payloads NOT INDEXED ORDER BY key")
        .map_err(|error| format!("prepare payload validation: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(|error| format!("query payload validation: {error}"))?;
    for row in rows {
        let (key, encoded) =
            row.map_err(|error| format!("read payload validation row: {error}"))?;
        let bytes = if let Some((offset, len)) = decode_packed_locator(&encoded) {
            match read_pack_extent(pack.as_ref(), offset, len) {
                Some(bytes) => {
                    validation.readable_locators += 1;
                    bytes
                }
                None => {
                    validation.unreadable_locators += 1;
                    continue;
                }
            }
        } else if encoded.first() == Some(&PAYLOAD_TAG_INLINE) {
            encoded[1..].to_vec()
        } else {
            encoded
        };
        if let Some(expected) = super::super::cas::full_blob_hash(&key) {
            let mut digest = Sha256::new();
            digest.update(&bytes);
            if fszero_core::hexutil::sha256_hex_of(digest.finalize().into()) == expected {
                validation.verified_content_hashes += 1;
            } else {
                validation.invalid_content_hashes += 1;
            }
        }
    }
    validation.status = if validation.count_matches
        && validation.order_matches
        && validation.unreadable_locators == 0
        && validation.invalid_content_hashes == 0
    {
        "verified".to_string()
    } else {
        "unverified".to_string()
    };
    Ok(validation)
}

fn ordered_payload_keys(conn: &OracleConnection) -> Result<Vec<String>, String> {
    let mut statement = conn
        .prepare("SELECT key FROM payloads NOT INDEXED ORDER BY key")
        .map_err(|error| format!("prepare ordered payload scan: {error}"))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("query ordered payload scan: {error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("read ordered payload scan: {error}"))
}

fn read_pack_extent(pack: Option<&File>, offset: u64, len: u32) -> Option<Vec<u8>> {
    let pack = pack?;
    let mut bytes = vec![0; len as usize];
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        pack.read_exact_at(&mut bytes, offset).ok()?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut filled = 0;
        while filled < bytes.len() {
            let read = pack
                .seek_read(&mut bytes[filled..], offset + filled as u64)
                .ok()?;
            if read == 0 {
                return None;
            }
            filled += read;
        }
    }
    Some(bytes)
}

fn copy_pack_generations(source_db: &Path, destination: &Path) -> Result<(), String> {
    for path in store_files(source_db)? {
        let file_name = path.file_name().unwrap_or_default().to_string_lossy();
        if file_name.contains(".pack") {
            let copied = destination.join(path.file_name().unwrap_or_default());
            fs::copy(&path, &copied)
                .map_err(|error| format!("preserve pack generation: {error}"))?;
            File::open(&copied)
                .and_then(|file| file.sync_all())
                .map_err(|error| format!("fsync pack generation: {error}"))?;
        }
    }
    Ok(())
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('\"', "\"\""))
}

fn attestation_path(db_path: &Path) -> PathBuf {
    PathBuf::from(format!(
        "{}.integrity-attestation-v{GATE_VERSION}.json",
        db_path.display()
    ))
}

fn read_attestation(db_path: &Path) -> Option<Attestation> {
    serde_json::from_slice(&fs::read(attestation_path(db_path)).ok()?).ok()
}

fn write_attestation(db_path: &Path, value: &Attestation) -> Result<(), String> {
    let path = attestation_path(db_path);
    let temp = PathBuf::from(format!(
        "{}.tmp-{}-{}",
        path.display(),
        std::process::id(),
        unique_stamp()
    ));
    write_synced(
        &temp,
        &serde_json::to_vec(value).map_err(|error| error.to_string())?,
    )?;
    fs::rename(&temp, &path).map_err(|error| format!("publish integrity attestation: {error}"))?;
    sync_dir(path.parent().unwrap_or_else(|| Path::new(".")))
}

fn mutation_epoch_path(db_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.mutation-epoch", db_path.display()))
}

fn read_mutation_epoch(db_path: &Path) -> u64 {
    fs::read_to_string(mutation_epoch_path(db_path))
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

fn write_mutation_epoch(db_path: &Path, epoch: u64) -> Result<(), String> {
    let path = mutation_epoch_path(db_path);
    let temp = PathBuf::from(format!(
        "{}.tmp-{}-{}",
        path.display(),
        std::process::id(),
        unique_stamp()
    ));
    write_synced(&temp, epoch.to_string().as_bytes())?;
    fs::rename(&temp, &path).map_err(|error| format!("publish mutation epoch: {error}"))?;
    sync_dir(path.parent().unwrap_or_else(|| Path::new(".")))
}

/// Logical mutation counter used as the integrity-gate attestation identity. Read-only
/// fsqlite opens bump DB/WAL/SHM mtimes without changing payload bytes. Fingerprinting
/// those mtimes forced a full `integrity_check` on every reopen. Missing sidecar == epoch 0.
pub(super) fn bump_mutation_epoch(db_path: &Path) -> Result<(), String> {
    write_mutation_epoch(db_path, read_mutation_epoch(db_path).saturating_add(1))
}

/// Drop the current attestation so the next open re-runs `integrity_check`.
pub(super) fn invalidate_attestation(db_path: &Path) {
    let _ = fs::remove_file(attestation_path(db_path));
}

fn fingerprint_store(db_path: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut result = BTreeMap::new();
    result.insert(
        "mutation_epoch".to_string(),
        read_mutation_epoch(db_path).to_string(),
    );
    Ok(result)
}

/// Size-only identity of durable files for forensic snapshot dedup. Omits mtime
/// (read-only opens bump it) and SHM (a lock file, not payload). Distinct from the
/// gate fingerprint: two corrupt DBs at epoch 0 must not collapse to one snapshot.
fn snapshot_identity(db_path: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut result = BTreeMap::new();
    for path in store_files(db_path)? {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if name.ends_with("-shm") {
            continue;
        }
        let metadata =
            fs::metadata(&path).map_err(|error| format!("stat {}: {error}", path.display()))?;
        result.insert(name, metadata.len().to_string());
    }
    Ok(result)
}

fn store_files(db_path: &Path) -> Result<Vec<PathBuf>, String> {
    let parent = db_path.parent().unwrap_or_else(|| Path::new("."));
    let base = db_path
        .file_name()
        .ok_or_else(|| "database has no filename".to_string())?
        .to_string_lossy();
    let mut paths = vec![db_path.to_path_buf()];
    for suffix in ["-wal", "-shm", "-fsqlite-ns-gate", "-fsqlite-ns-use"] {
        let path = parent.join(format!("{base}{suffix}"));
        if path.is_file() {
            paths.push(path);
        }
    }
    let pack_prefix = format!("{base}.pack");
    for entry in fs::read_dir(parent).map_err(|error| format!("list store directory: {error}"))? {
        let path = entry.map_err(|error| error.to_string())?.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if path.is_file() && (name == pack_prefix || name.starts_with(&format!("{pack_prefix}.g")))
        {
            paths.push(path);
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Existing `<store>.forensic-*` / `<store>.salvage-*` siblings, sorted.
fn existing_snapshot_destinations(db_path: &Path) -> Result<Vec<PathBuf>, String> {
    let parent = db_path.parent().unwrap_or_else(|| Path::new("."));
    let base = db_path
        .file_name()
        .ok_or_else(|| "database has no filename".to_string())?
        .to_string_lossy()
        .into_owned();
    let mut found = Vec::new();
    for entry in fs::read_dir(parent).map_err(|error| format!("list store directory: {error}"))? {
        let path = entry.map_err(|error| error.to_string())?.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if path.is_dir()
            && (name.starts_with(&format!("{base}.forensic-"))
                || name.starts_with(&format!("{base}.salvage-")))
        {
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}

/// Delete oldest forensic/salvage siblings until a new pair can be created without exceeding
/// [`MAX_SNAPSHOT_DESTINATIONS`]. The newest sibling always survives: it is the only copy that can
/// still explain the current refusal.
fn make_room_for_snapshot_pair(db_path: &Path) -> Result<(), String> {
    const PAIR: usize = 2;
    loop {
        let existing = existing_snapshot_destinations(db_path)?;
        if existing.len() + PAIR <= MAX_SNAPSHOT_DESTINATIONS {
            return Ok(());
        }
        if existing.len() <= 1 {
            return Ok(());
        }
        let oldest = existing
            .iter()
            .min_by_key(|path| snapshot_stamp(path))
            .expect("len > 1");
        fs::remove_dir_all(oldest)
            .map_err(|error| format!("prune oldest snapshot {}: {error}", oldest.display()))?;
    }
}

/// Sidecar files that belong to the live store (not forensic/salvage dirs).
fn live_store_sidecars(db_path: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for extra in [
        attestation_path(db_path),
        mutation_epoch_path(db_path),
        PathBuf::from(format!("{}.ast", db_path.display())),
    ] {
        if extra.is_file() {
            paths.push(extra);
        }
    }
    paths
}

fn rename_or_copy_file(source: &Path, dest: &Path) -> Result<(), String> {
    match fs::rename(source, dest) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(source, dest).map_err(|error| {
                format!("copy {} -> {}: {error}", source.display(), dest.display())
            })?;
            File::open(dest)
                .and_then(|file| file.sync_all())
                .map_err(|error| format!("fsync {}: {error}", dest.display()))?;
            fs::remove_file(source)
                .map_err(|error| format!("remove {} after copy: {error}", source.display()))?;
            Ok(())
        }
    }
}

/// Quarantine the live store after a destructive integrity finding so CreateOrOpen can mint a fresh
/// durable file. The workspace is the source of truth; this store is a cache.
pub(super) fn reset_live_store_after_destructive(
    db_path: &Path,
    reason: &str,
) -> Result<PathBuf, String> {
    if db_path.is_file() {
        let _ = make_room_for_snapshot_pair(db_path);
        match create_forensic_and_salvage(db_path) {
            Ok((forensic, salvage, report)) => {
                eprintln!(
                    "fszero durable integrity gate: retained forensic={} salvage={} report={}",
                    forensic.display(),
                    salvage.display(),
                    report.display()
                );
            }
            Err(error) if error.contains("identical forensic snapshot") => {}
            Err(error) => {
                eprintln!(
                    "fszero durable integrity gate: forensic retain during reset failed: {error}"
                );
            }
        }
    }

    let parent = db_path.parent().unwrap_or_else(|| Path::new("."));
    let qdir = parent
        .join("quarantine")
        .join(format!("reset-{}", unique_stamp()));
    fs::create_dir_all(&qdir)
        .map_err(|error| format!("create reset quarantine {}: {error}", qdir.display()))?;

    let mut moved = Vec::new();
    let mut sources = store_files(db_path).unwrap_or_else(|_| vec![db_path.to_path_buf()]);
    sources.extend(live_store_sidecars(db_path));
    sources.sort();
    sources.dedup();
    for source in sources {
        if !source.exists() {
            continue;
        }
        let name = source
            .file_name()
            .ok_or_else(|| "live store file has no name".to_string())?;
        let dest = qdir.join(name);
        rename_or_copy_file(&source, &dest)?;
        moved.push(name.to_string_lossy().into_owned());
    }

    let event = serde_json::json!({
        "schema": "fszero-store-reset",
        "reason": reason,
        "moved": moved,
        "note": "live store will be recreated empty; workspace files are the source of truth; forensic/salvage siblings retained when possible",
    });
    let event_bytes = serde_json::to_vec_pretty(&event)
        .map_err(|error| format!("render reset event: {error}"))?;
    // write_synced uses create_new; the dest dir is fresh so this is unique.
    write_synced(&qdir.join("RESET-EVENT.json"), &event_bytes)?;
    sync_dir(&qdir)?;
    eprintln!(
        "fszero durable integrity gate: live store reset after destructive failure; quarantine={}; workspace remains the source of truth",
        qdir.display()
    );
    Ok(qdir)
}

/// Nanosecond creation stamp encoded in a snapshot sibling name.
/// Sorting by name alone groups by kind before time, so forensic and
/// salvage siblings of the same incident would not stay adjacent in age order.
fn snapshot_stamp(path: &Path) -> u128 {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let Some((_, rest)) = name
        .rsplit_once(".forensic-")
        .or_else(|| name.rsplit_once(".salvage-"))
    else {
        return 0;
    };
    rest.split('-')
        .next()
        .unwrap_or_default()
        .parse()
        .unwrap_or(0)
}

fn directory_size(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0;
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        total += if metadata.is_dir() {
            directory_size(&entry.path())
        } else {
            metadata.len()
        };
    }
    total
}

/// Retained snapshot siblings with their byte sizes, newest first.
pub(super) fn snapshot_storage_stats(db_path: &Path) -> Result<Vec<(PathBuf, u64)>, String> {
    let mut found: Vec<(PathBuf, u64)> = existing_snapshot_destinations(db_path)?
        .into_iter()
        .map(|path| {
            let bytes = directory_size(&path);
            (path, bytes)
        })
        .collect();
    found.sort_by_key(|(path, _)| std::cmp::Reverse(snapshot_stamp(path)));
    Ok(found)
}

pub fn snapshot_retention_budget() -> u64 {
    std::env::var(SNAPSHOT_RETENTION_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .unwrap_or(DEFAULT_SNAPSHOT_RETENTION_BYTES)
}

fn prune_snapshot_destinations(db_path: &Path) -> Result<u64, String> {
    prune_snapshot_destinations_to(db_path, snapshot_retention_budget())
}

/// Indices into a newest-first `stats` slice that a retention pass must delete so the survivors
/// satisfy both the byte budget and the sibling count cap.
fn gc_target_indices(stats: &[(PathBuf, u64)], budget: u64, count_cap: usize) -> Vec<usize> {
    let mut delete = Vec::new();
    let mut total: u64 = stats.iter().map(|(_, bytes)| *bytes).sum();
    // `next` is the count of still-retained entries (indices 0..next).
    let mut next = stats.len();
    while next > 1 {
        if total <= budget && next <= count_cap {
            break;
        }
        next -= 1;
        total = total.saturating_sub(stats[next].1);
        delete.push(next);
    }
    delete
}

/// Delete snapshot siblings oldest-first until the retained set satisfies both the byte budget and
/// the sibling count cap.
fn prune_snapshot_destinations_to(db_path: &Path, budget: u64) -> Result<u64, String> {
    let stats = snapshot_storage_stats(db_path)?;
    let targets = gc_target_indices(&stats, budget, MAX_SNAPSHOT_DESTINATIONS);
    for index in &targets {
        let (path, _) = &stats[*index];
        fs::remove_dir_all(path)
            .map_err(|error| format!("prune snapshot {}: {error}", path.display()))?;
    }
    let retained: u64 = stats
        .iter()
        .enumerate()
        .filter(|(index, _)| !targets.contains(index))
        .map(|(_, (_, bytes))| *bytes)
        .sum();
    Ok(retained)
}

/// One snapshot sibling considered by a store-gc pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SnapshotGcEntry {
    /// Sibling directory name (relative; never an absolute path).
    pub name: String,
    /// `forensic` or `salvage`, derived from the sibling name.
    pub kind: String,
    /// Nanosecond creation stamp encoded in the sibling name.
    pub stamp: u128,
    /// Retained byte size of the sibling directory.
    pub bytes: u64,
}

/// Read-only retention plan for one store's forensic/salvage siblings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoreGcPlan {
    /// Store database file name this plan applies to.
    pub store: String,
    /// Byte budget the retained siblings are pruned back to.
    pub budget_bytes: u64,
    /// Maximum number of snapshot siblings retained next to one store.
    pub count_cap: usize,
    /// Sibling directories scanned.
    pub scanned: usize,
    /// Total bytes across all scanned siblings.
    pub total_bytes: u64,
    /// Siblings the retention pass would delete, oldest-first.
    pub delete: Vec<SnapshotGcEntry>,
    /// Bytes freed by deleting `delete`.
    pub delete_bytes: u64,
    /// Siblings that survive the retention pass, newest-first.
    pub retained: Vec<SnapshotGcEntry>,
    /// Bytes retained after the pass.
    pub retained_bytes: u64,
}

fn build_store_gc_plan(db_path: &Path, budget_bytes: u64) -> Result<StoreGcPlan, String> {
    let stats = snapshot_storage_stats(db_path)?;
    let targets = gc_target_indices(&stats, budget_bytes, MAX_SNAPSHOT_DESTINATIONS);
    let to_entry = |(path, bytes): &(PathBuf, u64)| SnapshotGcEntry {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        kind: if path.to_string_lossy().contains(".forensic-") {
            "forensic".to_string()
        } else {
            "salvage".to_string()
        },
        stamp: snapshot_stamp(path),
        bytes: *bytes,
    };
    let delete: Vec<SnapshotGcEntry> = targets
        .iter()
        .map(|index| to_entry(&stats[*index]))
        .collect();
    let retained: Vec<SnapshotGcEntry> = stats
        .iter()
        .enumerate()
        .filter(|(index, _)| !targets.contains(index))
        .map(|(_, entry)| to_entry(entry))
        .collect();
    Ok(StoreGcPlan {
        store: db_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        budget_bytes,
        count_cap: MAX_SNAPSHOT_DESTINATIONS,
        scanned: stats.len(),
        total_bytes: stats.iter().map(|(_, bytes)| *bytes).sum(),
        delete_bytes: delete.iter().map(|entry| entry.bytes).sum(),
        retained_bytes: retained.iter().map(|entry| entry.bytes).sum(),
        delete,
        retained,
    })
}

/// Read-only store-gc plan: computes which forensic/salvage siblings would be
/// deleted to satisfy the retention budget, without deleting anything. Safe to
/// run against a live store.
pub fn store_gc_plan(db_path: &Path, budget_bytes: u64) -> Result<StoreGcPlan, String> {
    build_store_gc_plan(db_path, budget_bytes)
}

/// Apply the store-gc retention pass: delete siblings oldest-first until both the byte budget and
/// the count cap hold, always keeping the newest sibling.
pub fn store_gc_apply(db_path: &Path, budget_bytes: u64) -> Result<StoreGcPlan, String> {
    let plan = build_store_gc_plan(db_path, budget_bytes)?;
    let parent = db_path.parent().unwrap_or_else(|| Path::new("."));
    for entry in &plan.delete {
        fs::remove_dir_all(parent.join(&entry.name))
            .map_err(|error| format!("store-gc remove {}: {error}", entry.name))?;
    }
    Ok(plan)
}

/// A completed forensic sibling that already captured this exact store state.
fn existing_forensic_with_fingerprint(
    db_path: &Path,
    fingerprint_bytes: &[u8],
) -> Result<Option<PathBuf>, String> {
    for path in existing_snapshot_destinations(db_path)? {
        if path.join("INCOMPLETE").exists() {
            continue;
        }
        if fs::read(path.join(FINGERPRINT_FILE)).is_ok_and(|bytes| bytes == fingerprint_bytes) {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn unique_destination(db_path: &Path, kind: &str) -> Result<PathBuf, String> {
    let parent = db_path.parent().unwrap_or_else(|| Path::new("."));
    let base = db_path
        .file_name()
        .ok_or_else(|| "database has no filename".to_string())?
        .to_string_lossy();
    let stamp = unique_stamp();
    for sequence in 0..1000 {
        let path = parent.join(format!(
            "{base}.{kind}-{stamp}-{}-{sequence}",
            std::process::id()
        ));
        if !path.exists() {
            return Ok(path);
        }
    }
    Err(format!("cannot allocate unique {kind} destination"))
}

fn unique_stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(fszero_core::hexutil::sha256_hex_of(
        digest.finalize().into(),
    ))
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    file.write_all(bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())
}

fn sync_dir(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(|error| format!("fsync directory {}: {error}", path.display()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}
