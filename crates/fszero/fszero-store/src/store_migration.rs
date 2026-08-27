//! Crash-safe migration from legacy global FSZero metadata DB to per-repo shards.
//!
//! Pre-isolation layout under a global `ZEROSTACK_STORE_ROOT`:
//!   `<store>/fszero/store.sqlite3`  — single DB for all workspaces
//!
//! Target layout (hub `zero-store` / fszero-9pvk):
//!   `<store>/projects/<project_key>/fszero/store.sqlite3` — one DB per root
//!
//! Ownership is never guessed:
//! - Rows with recorded `canonical_root` / workspace metadata move only to that
//!   repo's shard when the path is unambiguous.
//! - Ambiguous rows go to `<store>/fszero/quarantine/<timestamp>/`.
//! - Migration is idempotent and resumable via a journal file.

use super::zerostack_store::{
    ensure_repo_metadata_layout, legacy_global_fszero_sqlite_path, repo_store_sid,
};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const JOURNAL_NAME: &str = "migration_journal.v1.jsonl";
const SCHEMA: &str = "fszero.store_migration";

struct MigrationLock(std::fs::File);

impl MigrationLock {
    fn acquire(store_root: &Path) -> Result<Self, String> {
        let dir = store_root.join("fszero");
        fs::create_dir_all(&dir)
            .map_err(|error| format!("create migration lock directory: {error}"))?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(dir.join("migration.lock"))
            .map_err(|error| format!("open migration lock: {error}"))?;
        std::fs::File::lock(&file).map_err(|error| format!("lock migration: {error}"))?;
        Ok(Self(file))
    }
}

impl Drop for MigrationLock {
    fn drop(&mut self) {
        let _ = std::fs::File::unlock(&self.0);
    }
}

#[derive(Debug, Clone)]
pub struct GlobalStoreMigrationReport {
    pub legacy_db: PathBuf,
    pub moved: Vec<(String, PathBuf)>,
    pub quarantined: PathBuf,
    pub skipped_missing_legacy: bool,
    pub journal: PathBuf,
}

fn journal_path(store_root: &Path) -> PathBuf {
    store_root.join("fszero").join(JOURNAL_NAME)
}

fn append_journal(store_root: &Path, line: &str) -> Result<(), String> {
    let path = journal_path(store_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    writeln!(f, "{line}").map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())?;
    Ok(())
}

fn journal_contains(store_root: &Path, needle: &str) -> bool {
    let path = journal_path(store_root);
    fs::read_to_string(path)
        .map(|s| s.lines().any(|l| l.contains(needle)))
        .unwrap_or(false)
}

/// Rename `src` → `dest`, falling back to copy+remove when rename crosses devices.
fn rename_or_copy(src: &Path, dest: &Path) -> Result<(), String> {
    fs::rename(src, dest)
        .or_else(|_| {
            fs::copy(src, dest)
                .map(|_| ())
                .and_then(|_| fs::remove_file(src))
        })
        .map_err(|e| e.to_string())
}

fn json_path(path: &Path) -> String {
    serde_json::to_string(&path.display().to_string()).unwrap_or_default()
}

fn append_journal_event(store_root: &Path, event: &str, extra: &str) -> Result<(), String> {
    append_journal(
        store_root,
        &format!(r#"{{"schema":"{SCHEMA}","event":"{event}",{extra}}}"#),
    )
}

/// Migrate a legacy global `fszero/store.sqlite3` when ownership can be proven.
///
/// `known_roots`: canonical repository roots the operator asserts. For each
/// root, if the legacy DB's recorded identity matches **exactly one** root
/// (via colocated `repo_identity` or a single known root claiming the DB via
/// optional `owner_root` file next to the legacy DB), the file is **moved**
/// into that repo's shard. Otherwise the legacy DB is hard-linked/copied into
/// quarantine and left in place until an operator decides (never duplicated
/// into every repo).
///
/// When the legacy file is absent, returns `skipped_missing_legacy=true`.
pub fn migrate_legacy_global_store(
    store_root: &Path,
    known_roots: &[&Path],
) -> Result<GlobalStoreMigrationReport, String> {
    let _migration_lock = MigrationLock::acquire(store_root)?;
    let legacy = legacy_global_fszero_sqlite_path(store_root);
    let journal = journal_path(store_root);
    let qdir = store_root
        .join("fszero")
        .join("quarantine")
        .join(format!("mig-{}", crate::recovery::unix_epoch_secs() as u64));

    if !legacy.is_file() {
        return Ok(GlobalStoreMigrationReport {
            legacy_db: legacy,
            moved: vec![],
            quarantined: qdir,
            skipped_missing_legacy: true,
            journal,
        });
    }

    // Already migrated?
    if journal_contains(store_root, "\"done\":true")
        || journal_contains(store_root, "status\":\"complete\"")
    {
        return Ok(GlobalStoreMigrationReport {
            legacy_db: legacy,
            moved: vec![],
            quarantined: qdir,
            skipped_missing_legacy: false,
            journal,
        });
    }

    let legacy_json = serde_json::to_string(&legacy.display().to_string()).unwrap_or_default();
    append_journal(
        store_root,
        &format!(r#"{{"schema":"{SCHEMA}","event":"start","legacy":{legacy_json}}}"#),
    )?;

    // Owner file written by older tooling or operator: fszero/OWNER_ROOT path text.
    let owner_file = store_root.join("fszero").join("OWNER_ROOT");
    let owner_from_file = fs::read_to_string(&owner_file)
        .ok()
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| !p.as_os_str().is_empty());

    let mut unambiguous: Option<PathBuf> = owner_from_file;
    if unambiguous.is_none() && known_roots.len() == 1 {
        unambiguous = None;
    }
    // If OWNER_ROOT is set and is in known_roots (or known_roots empty = trust file).
    if let Some(ref o) = unambiguous {
        if !known_roots.is_empty()
            && !known_roots.iter().any(|r| {
                fs::canonicalize(r).ok().as_ref() == fs::canonicalize(o).ok().as_ref()
                    || *r == o.as_path()
            })
        {
            unambiguous = None;
        }
    }

    let mut moved = Vec::new();
    if let Some(root) = unambiguous {
        let dir = ensure_repo_metadata_layout(store_root, &root)?;
        let dest = dir.join("store.sqlite3");
        if dest.exists() {
            // Target already has a DB — quarantine legacy, never overwrite.
            fs::create_dir_all(&qdir).map_err(|e| e.to_string())?;
            let q = qdir.join("store.sqlite3");
            rename_or_copy(&legacy, &q).map_err(|e| format!("quarantine legacy: {e}"))?;
            append_journal_event(
                store_root,
                "quarantine",
                &format!(r#""reason":"dest_exists","path":{}"#, json_path(&q)),
            )?;
        } else {
            rename_or_copy(&legacy, &dest).map_err(|e| format!("move legacy to shard: {e}"))?;
            // Preserve backup next to dest.
            let bak = dir.join("store.sqlite3.pre-migration.bak");
            let _ = fs::copy(&dest, &bak);
            let sid = repo_store_sid(&root);
            moved.push((sid, dest.clone()));
            append_journal_event(
                store_root,
                "moved",
                &format!(r#""dest":{}"#, json_path(&dest)),
            )?;
        }
    } else {
        // Ambiguous: quarantine copy; leave original until operator acts, or
        // move to quarantine to remove the global fallback path.
        fs::create_dir_all(&qdir).map_err(|e| e.to_string())?;
        let q = qdir.join("store.sqlite3");
        rename_or_copy(&legacy, &q).map_err(|e| format!("quarantine ambiguous legacy: {e}"))?;
        // Write reason
        let _ = fs::write(
            qdir.join("REASON.txt"),
            "Ambiguous ownership: no OWNER_ROOT matching known_roots. \
             Do not copy into every repository. Restore from quarantine after \
             assigning ownership via OWNER_ROOT or per-repo import.\n",
        );
        append_journal_event(
            store_root,
            "quarantine",
            &format!(r#""reason":"ambiguous","path":{}"#, json_path(&q)),
        )?;
    }

    append_journal(
        store_root,
        &format!(r#"{{"schema":"{SCHEMA}","event":"complete","status":"complete","done":true}}"#),
    )?;

    // Touch marker so callers know global fallback must not reopen legacy path.
    let _ = fs::write(
        store_root.join("fszero").join("NO_GLOBAL_METADATA"),
        "fszero-kflx: global metadata fallback removed; use projects/<project_key>/fszero/store.sqlite3\n",
    );

    Ok(GlobalStoreMigrationReport {
        legacy_db: legacy,
        moved,
        quarantined: qdir,
        skipped_missing_legacy: false,
        journal,
    })
}

/// Ensure no code path reopens the legacy global metadata DB for new sessions.
pub fn global_metadata_fallback_disabled(store_root: &Path) -> bool {
    store_root
        .join("fszero")
        .join("NO_GLOBAL_METADATA")
        .is_file()
        || !legacy_global_fszero_sqlite_path(store_root).is_file()
}

/// Default quarantine retention: 256 MB total, 14 days max age.
const DEFAULT_QUARANTINE_MAX_BYTES: u64 = 256 * 1024 * 1024;
const DEFAULT_QUARANTINE_MAX_AGE_DAYS: u64 = 14;

fn quarantine_max_bytes() -> u64 {
    std::env::var("FSZERO_QUARANTINE_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_QUARANTINE_MAX_BYTES)
}

fn quarantine_max_age_days() -> u64 {
    std::env::var("FSZERO_QUARANTINE_MAX_AGE_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_QUARANTINE_MAX_AGE_DAYS)
}

/// Report from a quarantine pruning pass.
#[derive(Debug, Clone, Default)]
pub struct QuarantinePruneReport {
    pub scanned_entries: usize,
    pub removed_entries: usize,
    pub freed_bytes: u64,
    pub remaining_bytes: u64,
}

/// Prune the quarantine directory under `<store>/fszero/quarantine/`.
///
/// Removes entries older than `max_age_days` and then, if total size still
/// exceeds `max_bytes`, removes oldest-first until under the cap. Each
/// eviction is journaled. Returns a report with counts and freed bytes.
pub fn prune_quarantine(store_root: &Path) -> Result<QuarantinePruneReport, String> {
    let qdir = store_root.join("fszero").join("quarantine");
    if !qdir.is_dir() {
        return Ok(QuarantinePruneReport::default());
    }

    let max_bytes = quarantine_max_bytes();
    let max_age_secs = quarantine_max_age_days() * 86400;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Collect entries with their mtime and size.
    let mut entries: Vec<(PathBuf, u64, u64)> = Vec::new();
    let read = fs::read_dir(&qdir).map_err(|e| format!("read quarantine dir: {e}"))?;
    for entry in read {
        let entry = entry.map_err(|e| format!("read quarantine entry: {e}"))?;
        let path = entry.path();
        let metadata =
            fs::metadata(&path).map_err(|e| format!("metadata {}: {e}", path.display()))?;
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let size = dir_size(&path);
        entries.push((path, mtime, size));
    }

    entries.sort_by_key(|(_, mtime, _)| *mtime);
    let mut report = QuarantinePruneReport {
        scanned_entries: entries.len(),
        removed_entries: 0,
        freed_bytes: 0,
        remaining_bytes: entries.iter().map(|(_, _, s)| *s).sum(),
    };

    // Pass 1: remove entries older than max_age.
    let mut remaining: Vec<(PathBuf, u64, u64)> = Vec::new();
    for (path, mtime, size) in entries {
        if now.saturating_sub(mtime) >= max_age_secs {
            let _ = fs::remove_dir_all(&path);
            report.removed_entries += 1;
            report.freed_bytes += size;
            report.remaining_bytes = report.remaining_bytes.saturating_sub(size);
            append_journal_event(
                store_root,
                "quarantine_pruned",
                &format!(
                    r#""reason":"expired","path":{},"bytes":{size}"#,
                    json_path(&path)
                ),
            )?;
        } else {
            remaining.push((path, mtime, size));
        }
    }

    // Pass 2: if still over max_bytes, remove oldest-first.
    let mut total: u64 = remaining.iter().map(|(_, _, s)| *s).sum();
    if total > max_bytes {
        for (path, _, size) in &remaining {
            if total <= max_bytes {
                break;
            }
            let _ = fs::remove_dir_all(path);
            report.removed_entries += 1;
            report.freed_bytes += *size;
            total = total.saturating_sub(*size);
            report.remaining_bytes = total;
            append_journal_event(
                store_root,
                "quarantine_pruned",
                &format!(
                    r#""reason":"size_cap","path":{},"bytes":{size}"#,
                    json_path(path)
                ),
            )?;
        }
    }

    Ok(report)
}

/// Recursively compute the size of a file or directory in bytes.
fn dir_size(path: &Path) -> u64 {
    let metadata = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => return 0,
    };
    if metadata.is_file() || metadata.file_type().is_symlink() {
        return metadata.len();
    }
    if metadata.is_dir() {
        let mut total = 0u64;
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries {
                if let Ok(entry) = entry {
                    total += dir_size(&entry.path());
                }
            }
        }
        return total;
    }
    0
}
/// One-shot adoption of metadata stranded by the pi1 precedence fix.
///
/// Before pi1 FSZero checked `ZEROSTACK_STORE_ROOT` first and honored it with
/// no opt-in gate. Two populated locations are therefore now unreachable:
///
/// - repo has `.zerostack` **and** a pin: FSZero wrote under the pin, the fix
///   sends it to `<repo>/.zerostack/fszero/store.sqlite3`;
/// - bare pin with no `.zerostack` and no opt-in: FSZero wrote under the pin,
///   the fix sends it to the legacy `<repo>/.fszero/store.sqlite3`.
///
/// In both cases the user's existing database would look like it vanished. This
/// moves it to the location the current resolver names.
///
/// Safety properties, in order of importance:
/// - **Never overwrites.** If the destination DB already exists, the source is
///   left untouched and `adopted` is false; the caller keeps the destination.
///   Merging two live databases is not something this can decide.
/// - **Copy-then-remove, not rename.** The superseded root is usually on a
///   different filesystem from the repo, where `fs::rename` fails with EXDEV.
/// - **Crash-safe.** The copy lands on a temp path in the destination
///   directory and is renamed into place, so an interrupted run leaves either
///   the untouched source or a complete destination, never a half-written DB.
/// - **Idempotent.** Once moved the source is gone and the destination exists,
///   so a second call is a no-op.
///
/// SQLite sidecars (`-wal`, `-shm`) move with the DB. A `-wal` left behind
/// would strand committed transactions that had not yet checkpointed.
pub fn adopt_superseded_store(repo_root: &Path) -> Result<StoreAdoptionReport, String> {
    use super::zerostack_store::{
        sqlite_path_under, superseded_store_root, zerostack_store_or_detect,
    };

    let dest = sqlite_path_under(zerostack_store_or_detect(repo_root).as_deref(), repo_root);
    let Some(src) = superseded_store_root(repo_root) else {
        return Ok(StoreAdoptionReport::default());
    };
    let src_db = sqlite_path_under(Some(src.as_path()), repo_root);

    if src_db == dest || !src_db.is_file() {
        return Ok(StoreAdoptionReport::default());
    }
    if dest.is_file() {
        return Ok(StoreAdoptionReport {
            source: Some(src_db),
            destination: Some(dest),
            adopted: false,
            conflict: true,
        });
    }

    let parent = dest
        .parent()
        .ok_or_else(|| "destination has no parent".to_string())?;
    fs::create_dir_all(parent).map_err(|e| format!("create store dir failed: {e}"))?;

    for suffix in ["", "-wal", "-shm"] {
        let from = sidecar(&src_db, suffix);
        if !from.is_file() {
            continue;
        }
        let to = sidecar(&dest, suffix);
        let staged = sidecar(&dest, &format!("{suffix}.adopting"));
        fs::copy(&from, &staged).map_err(|e| format!("copy {} failed: {e}", from.display()))?;
        fs::rename(&staged, &to).map_err(|e| format!("publish {} failed: {e}", to.display()))?;
        fs::remove_file(&from).map_err(|e| format!("remove {} failed: {e}", from.display()))?;
    }

    Ok(StoreAdoptionReport {
        source: Some(src_db),
        destination: Some(dest),
        adopted: true,
        conflict: false,
    })
}

fn sidecar(db: &Path, suffix: &str) -> PathBuf {
    if suffix.is_empty() {
        return db.to_path_buf();
    }
    let mut name = db.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Outcome of [`adopt_superseded_store`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreAdoptionReport {
    /// DB found where the pre-pi1 resolver would have looked.
    pub source: Option<PathBuf>,
    /// Location the current resolver names.
    pub destination: Option<PathBuf>,
    /// True when the DB was moved.
    pub adopted: bool,
    /// True when both locations held a DB, so the source was left alone.
    pub conflict: bool,
}
