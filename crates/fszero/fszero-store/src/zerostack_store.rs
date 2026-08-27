//! Unified `.zerostack/` store-root resolution for FSZero durable store.
//!
//! Resolution and project-key hashing are owned by hub `zero-store`
//! ([`zero_store::ResolvedStore`], [`zero_store::project_key`]). FSZero keeps
//! engine-specific SQLite paths and migration helpers on top of that contract.
//!
//! **Repository isolation (kflx / fszero-1nos):** every canonical repository
//! root gets its own physical SQLite metadata database. A global
//! `ZEROSTACK_STORE_ROOT` may host many repos, but metadata lives under
//! `…/projects/<project_key>/fszero/store.sqlite3` keyed by the hub project
//! key — never a single shared `…/fszero/store.sqlite3` for unrelated roots.
//! Only immutable digest-addressed CAS blobs (`…/blobs`) may be shared.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use zero_store::{
    self, Engine, PROJECTS_DIR, ResolvedStore, StoreEnv, absolutize, ensure_layout, project_key,
    store_is_under_project_root,
};

/// Opt-in env names for honoring a shared/meta store pin.
///
/// Hub canonical name is `ZEROSTACK_SHARED_STORE`; `FSZERO_SHARED_STORE` is the
/// engine alias passed into [`StoreEnv::from_process`].
pub const SHARED_STORE_OPT_IN_ENVS: &[&str] = &["FSZERO_SHARED_STORE", "ZEROSTACK_SHARED_STORE"];

/// Global pin env names (same set as hub `zero_store::STORE_ROOT_ENVS`).
pub const STORE_ROOT_ENVS: &[&str] = zero_store::STORE_ROOT_ENVS;

/// Engine alias opt-in names (excludes the hub canonical name, which
/// [`StoreEnv::from_process`] already checks).
const SHARED_STORE_OPT_IN_ALIASES: &[&str] = &[SHARED_STORE_OPT_IN_ENVS[0]];

/// Whether the process has opted into a shared/meta `ZEROSTACK_STORE_ROOT`.
pub fn shared_store_opt_in_from_env() -> bool {
    StoreEnv::from_process(SHARED_STORE_OPT_IN_ALIASES).shared_opt_in
}

fn first_env(names: &[&str]) -> Option<std::ffi::OsString> {
    names.iter().find_map(std::env::var_os)
}

/// Resolve FSZero against the live process environment via hub `zero-store`.
pub fn resolve_fszero(repo_root: &Path) -> ResolvedStore {
    ResolvedStore::resolve_from_process(repo_root, Engine::FsZero, SHARED_STORE_OPT_IN_ALIASES)
}

/// Pure store-root selection (zerostack-pi1), delegated to hub `zero-store`.
///
/// Precedence matches TokenZero / GraphZero / hub [`ResolvedStore::resolve`]:
///
/// 1. `<repo>/.zerostack` when that directory exists.
/// 2. Else the pin (`ZEROSTACK_STORE_ROOT` / `ZERO_STACK_STORE_ROOT`), but
///    **only** when the process opted in — absolute, or relative to
///    `repo_root` (never process cwd).
/// 3. Else `None` (caller falls back to legacy `<repo>/.fszero`).
pub fn resolve_store_root_with_env(
    repo_root: &Path,
    store_root_pin: Option<&OsStr>,
    shared_opt_in: bool,
) -> Option<PathBuf> {
    let env = StoreEnv::new(store_root_pin.map(|s| s.to_os_string()), shared_opt_in);
    ResolvedStore::resolve(repo_root, Engine::FsZero, &env)
        .unified_root()
        .map(|p| p.to_path_buf())
}

/// Active unified store directory, if any. See [`resolve_store_root_with_env`].
pub fn zerostack_store_or_detect(repo_root: &Path) -> Option<PathBuf> {
    resolve_store_root_with_env(
        repo_root,
        first_env(STORE_ROOT_ENVS).as_deref(),
        shared_store_opt_in_from_env(),
    )
}

/// The store root the pre-pi1 resolver would have chosen: pin first, ungated,
/// then `<repo>/.zerostack`. Used only to find data left at a location the
/// current resolver no longer selects.
pub fn superseded_store_root(repo_root: &Path) -> Option<PathBuf> {
    if let Some(v) = first_env(STORE_ROOT_ENVS).filter(|v| !v.is_empty()) {
        let path = PathBuf::from(v);
        return Some(if path.is_absolute() {
            path
        } else {
            repo_root.join(path)
        });
    }
    let candidate = repo_root.join(zero_store::LOCAL_STORE_DIR);
    candidate.is_dir().then_some(candidate)
}

/// True when `store` is outside the canonical repo tree (global shared host).
///
/// Relative env values resolve under the repo and stay repo-local; an absolute
/// env (or a path that does not sit under the repo) is a multi-tenant host and
/// must shard metadata by project key.
pub fn store_is_global_host(store: &Path, repo_root: &Path) -> bool {
    !store_is_under_project_root(store, repo_root)
}

/// Hub project key for metadata sharding under a global store host (16 hex,
/// no `sid-` prefix — the `projects/` path component owns the namespace).
pub fn repo_store_sid(repo_root: &Path) -> String {
    project_key(repo_root)
}

/// Directory holding one repo's metadata under a global ZeroStack store host.
///
/// Layout: `<store>/projects/<project_key>/fszero/` (hub `zero-store` contract).
pub fn repo_metadata_dir(store_root: &Path, repo_root: &Path) -> PathBuf {
    store_root
        .join(PROJECTS_DIR)
        .join(project_key(repo_root))
        .join(Engine::FsZero.dir_name())
}

/// Pre-hub-cutover shard: `<store>/fszero/repos/sid-<hex>/`.
fn legacy_repos_shard_sqlite(store_root: &Path, repo_root: &Path) -> PathBuf {
    store_root
        .join(Engine::FsZero.dir_name())
        .join("repos")
        .join(format!("sid-{}", project_key(repo_root)))
        .join("store.sqlite3")
}

/// SQLite path for this repository's metadata.
///
/// - Legacy / local: `<repo>/.fszero/store.sqlite3`
/// - Repo-local unified: `<repo>/.zerostack/fszero/store.sqlite3` (or relative
///   `ZEROSTACK_STORE_ROOT` under the repo)
/// - Global host (`ZEROSTACK_STORE_ROOT` outside the repo):
///   `<store>/projects/<project_key>/fszero/store.sqlite3`
///   so two roots never share one metadata DB.
pub fn fszero_store_sqlite_path(repo_root: &Path) -> PathBuf {
    resolve_fszero(repo_root).engine_dir().join("store.sqlite3")
}

/// The metadata DB path implied by a given store root (or the legacy location
/// when there is none). Factored out so migration can name the path the
/// superseded resolver would have chosen without duplicating the layout rules.
pub fn sqlite_path_under(store: Option<&Path>, repo_root: &Path) -> PathBuf {
    match store {
        Some(store) if store_is_global_host(store, repo_root) => {
            repo_metadata_dir(store, repo_root).join("store.sqlite3")
        }
        Some(store) => store.join("fszero/store.sqlite3"),
        None => repo_root.join(".fszero/store.sqlite3"),
    }
}

/// Legacy pre-isolation path that collapsed all repos into one DB under a
/// global host. Used by migration only.
pub fn legacy_global_fszero_sqlite_path(store_root: &Path) -> PathBuf {
    store_root.join("fszero/store.sqlite3")
}

pub fn ensure_unified_store_layout(store_root: &Path) -> Result<(), String> {
    fs::create_dir_all(store_root.join(Engine::FsZero.dir_name()))
        .map_err(|e| format!("create zerostack fszero dir failed: {e}"))
}

/// Ensure per-repo metadata directory exists under a global host.
///
/// Uses hub [`ensure_layout`] when the pin resolves to `store_root`, and
/// one-shots a rename from the pre-cutover `fszero/repos/sid-*` shard when
/// present.
pub fn ensure_repo_metadata_layout(store_root: &Path, repo_root: &Path) -> Result<PathBuf, String> {
    let env = StoreEnv::new(Some(store_root.as_os_str().to_os_string()), true);
    let resolved = ResolvedStore::resolve(repo_root, Engine::FsZero, &env);
    if resolved
        .unified_root()
        .is_some_and(|root| absolutize(root) == absolutize(store_root))
    {
        ensure_layout(&resolved).map_err(|e| format!("ensure zero-store layout failed: {e}"))?;
    } else {
        // Caller passed an explicit host that differs from env resolution
        // (tests / migration). Create the hub shard path directly.
        let dir = repo_metadata_dir(store_root, repo_root);
        fs::create_dir_all(&dir).map_err(|e| format!("create repo metadata dir failed: {e}"))?;
    }

    let dir = repo_metadata_dir(store_root, repo_root);
    let dest = dir.join("store.sqlite3");
    let old = legacy_repos_shard_sqlite(store_root, repo_root);
    if old.is_file() && !dest.exists() {
        fs::create_dir_all(&dir).map_err(|e| format!("create repo metadata dir failed: {e}"))?;
        fs::rename(&old, &dest)
            .or_else(|_| {
                fs::copy(&old, &dest)
                    .map(|_| ())
                    .and_then(|_| fs::remove_file(&old))
            })
            .map_err(|e| format!("migrate legacy repos shard: {e}"))?;
    }

    // Record ownership so migration / doctor can prove binding.
    let meta = dir.join("repo_identity.json");
    if !meta.exists() {
        let canon = absolutize(repo_root);
        let key = project_key(repo_root);
        let body = format!(
            "{{\n  \"schema\": \"fszero.repo_identity\",\n  \"canonical_root\": {},\n  \"project_key\": {},\n  \"sid\": {}\n}}\n",
            serde_json::to_string(&canon.to_string_lossy().to_string())
                .unwrap_or_else(|_| "\"\"".into()),
            serde_json::to_string(&key).unwrap_or_else(|_| "\"\"".into()),
            serde_json::to_string(&format!("sid-{key}")).unwrap_or_else(|_| "\"\"".into()),
        );
        let _ = fs::write(&meta, body);
    }
    Ok(dir)
}

pub fn ensure_repo_gitignore(repo_root: &Path, unified: bool) -> Result<(), String> {
    let gitignore = repo_root.join(".gitignore");
    // FIFO/socket open blocks; refuse from metadata before content I/O.
    crate::path::refuse_non_regular_file(&gitignore)?;
    let existing = fs::read_to_string(&gitignore).unwrap_or_default();
    let want = if unified { ".zerostack/" } else { ".fszero/" };
    if existing.lines().any(|line| line.trim() == want) {
        return Ok(());
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(want);
    updated.push('\n');
    crate::path::refuse_non_regular_file(&gitignore)?;
    fs::write(&gitignore, updated).map_err(|e| format!("write .gitignore failed: {e}"))
}

/// Stable short store identity for a durable store root (or SQLite parent).
///
/// Hex portion matches hub [`project_key`] for the same path; the `sid-`
/// prefix is retained for FSZero wire / mint metadata compatibility.
pub fn store_id_for_path(path: &Path) -> String {
    format!("sid-{}", project_key(path))
}

/// Parent directory reported as the durable store root for a SQLite db path.
///
/// - Local / pinned-inside: `…/<unified>/fszero/store.sqlite3` → `<unified>`
/// - Shared namespaced: `…/projects/<key>/fszero/store.sqlite3` → store host
///   (parent of `projects/`)
/// - Legacy: `…/.fszero/store.sqlite3` → `.fszero`
/// - Pre-cutover shard: `…/fszero/repos/<sid>/store.sqlite3` → that shard dir
pub fn store_root_from_db_path(db_path: &Path) -> Option<PathBuf> {
    let parent = db_path.parent()?;
    if parent.file_name().and_then(|n| n.to_str()) != Some("fszero") {
        return Some(parent.to_path_buf());
    }
    let above_fszero = parent.parent()?;
    if above_fszero
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        == Some(PROJECTS_DIR)
    {
        return above_fszero.parent()?.parent().map(|p| p.to_path_buf());
    }
    Some(above_fszero.to_path_buf())
}

/// Classify a store-root display string for root_report telemetry.
pub fn effective_root_mode(store_root: &str) -> &'static str {
    if store_root == "memory" {
        "memory"
    } else if Path::new(store_root).file_name().and_then(|n| n.to_str()) == Some(".zerostack")
        || store_root.contains("/.zerostack")
    {
        "unified"
    } else {
        "legacy"
    }
}

/// Store id for a SQLite db path.
///
/// Layouts:
/// - `…/projects/<key>/fszero/store.sqlite3` → `sid-<key>`
/// - `…/fszero/repos/<sid>/store.sqlite3` → id of that shard parent
/// - `…/fszero/store.sqlite3` → id of the store root
/// - `…/.fszero/store.sqlite3` → id of `.fszero` parent (repo)
pub fn store_id_for_db_path(db_path: &Path) -> String {
    let parent = db_path.parent().unwrap_or(db_path);
    // …/projects/<key>/fszero/store.sqlite3
    if parent.file_name().and_then(|n| n.to_str()) == Some("fszero") {
        if let Some(key_dir) = parent.parent() {
            if key_dir
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                == Some(PROJECTS_DIR)
            {
                if let Some(key) = key_dir.file_name().and_then(|n| n.to_str()) {
                    return format!("sid-{key}");
                }
            }
            return store_id_for_path(key_dir);
        }
    }
    // …/fszero/repos/<sid>/store.sqlite3 (pre-cutover)
    if parent
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        == Some("repos")
    {
        return store_id_for_path(parent);
    }
    store_id_for_path(parent)
}

#[cfg(all(test, unix))]
mod gitignore_fifo_tests {
    use super::*;
    use std::os::unix::fs::FileTypeExt;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::Duration;

    const HANG_BUDGET: Duration = Duration::from_millis(1500);

    fn mkfifo(path: &Path) {
        let status = std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .expect("spawn mkfifo");
        assert!(
            status.success(),
            "mkfifo {} failed: {status}",
            path.display()
        );
    }

    fn within_timeout<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("gitignore-fifo".into())
            .spawn(move || {
                let _ = tx.send(f());
            })
            .expect("spawn timeout worker");
        match rx.recv_timeout(HANG_BUDGET) {
            Ok(value) => value,
            Err(RecvTimeoutError::Timeout) => {
                panic!(
                    "timed out after {HANG_BUDGET:?}: ensure_repo_gitignore hung on FIFO instead of failing closed"
                )
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!("timeout worker panicked before returning a fail-closed result")
            }
        }
    }

    #[test]
    fn ensure_repo_gitignore_refuses_fifo_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let gitignore = dir.path().join(".gitignore");
        mkfifo(&gitignore);

        let err = within_timeout({
            let root = dir.path().to_path_buf();
            move || {
                ensure_repo_gitignore(&root, true).expect_err("FIFO .gitignore must fail closed")
            }
        });
        assert!(
            err.contains("unsupported file kind") && err.contains("fifo"),
            "expected unsupported file kind fifo, got {err}"
        );
        let meta = fs::symlink_metadata(&gitignore).expect("fifo metadata");
        assert!(
            meta.file_type().is_fifo(),
            "{} must remain a FIFO",
            gitignore.display()
        );
    }

    #[test]
    fn ensure_repo_gitignore_still_writes_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        ensure_repo_gitignore(dir.path(), true).expect("regular .gitignore");
        let text = fs::read_to_string(dir.path().join(".gitignore")).expect("read");
        assert!(
            text.lines().any(|line| line.trim() == ".zerostack/"),
            "expected .zerostack/ line, got {text:?}"
        );
    }
}
