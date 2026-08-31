//! Unified `.zerostack/` store-root resolution for FSZero durable store. Resolution and project-key
//! hashing are owned by hub `zero-store` ([`zero_store::ResolvedStore`],
//! [`zero_store::project_key`]).

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use zero_store::{
    self, Engine, PROJECTS_DIR, ResolvedStore, StoreEnv, absolutize, ensure_layout, project_key,
    store_is_under_project_root,
};

/// Opt-in env names for honoring a shared/meta store pin. Hub canonical name is
/// `ZEROSTACK_SHARED_STORE`; `FSZERO_SHARED_STORE` is the engine alias passed into
/// [`StoreEnv::from_process`].
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

/// Pure store-root selection, delegated to hub `zero-store`. Precedence matches TokenZero /
/// GraphZero / hub [`ResolvedStore::resolve`] 1. `<repo>/.zerostack` when that directory exists. 2.
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

/// True when `store` is outside the canonical repo tree (global shared host). Relative env
/// values resolve under the repo and stay repo-local; an absolute env (or a path that does
/// not sit under the repo) is a multi-tenant host and must shard metadata by project key.
pub fn store_is_global_host(store: &Path, repo_root: &Path) -> bool {
    !store_is_under_project_root(store, repo_root)
}

/// Directory holding one repo's metadata under a global ZeroStack store host.
/// Layout: `<store>/projects/<project_key>/fszero/` (hub `zero-store` contract).
pub fn repo_metadata_dir(store_root: &Path, repo_root: &Path) -> PathBuf {
    store_root
        .join(PROJECTS_DIR)
        .join(project_key(repo_root))
        .join(Engine::FsZero.dir_name())
}

/// SQLite path for this repository's metadata.
pub fn fszero_store_sqlite_path(repo_root: &Path) -> PathBuf {
    resolve_fszero(repo_root).engine_dir().join("store.sqlite3")
}

pub fn ensure_unified_store_layout(store_root: &Path) -> Result<(), String> {
    fs::create_dir_all(store_root.join(Engine::FsZero.dir_name()))
        .map_err(|e| format!("create zerostack fszero dir failed: {e}"))
}

/// Ensure the repository metadata directory exists under a global store host.
/// Uses [`ensure_layout`] when the resolved store matches `store_root`.
pub fn ensure_repo_metadata_layout(store_root: &Path, repo_root: &Path) -> Result<PathBuf, String> {
    let env = StoreEnv::new(Some(store_root.as_os_str().to_os_string()), true);
    let resolved = ResolvedStore::resolve(repo_root, Engine::FsZero, &env);
    if resolved
        .unified_root()
        .is_some_and(|root| absolutize(root) == absolutize(store_root))
    {
        ensure_layout(&resolved).map_err(|e| format!("ensure zero-store layout failed: {e}"))?;
    } else {
        // An explicit host can differ from process environment resolution.
        // Create its canonical repository metadata path directly.
        let dir = repo_metadata_dir(store_root, repo_root);
        fs::create_dir_all(&dir).map_err(|e| format!("create repo metadata dir failed: {e}"))?;
    }

    let dir = repo_metadata_dir(store_root, repo_root);

    // Bind this metadata directory to its repository identity.
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

/// Stable FSZero store identity for a durable store root or SQLite parent.
/// The hex portion matches the hub [`project_key`] digest for the same path.
pub fn store_id_for_path(path: &Path) -> String {
    format!("sid-{}", project_key(path))
}

/// Parent directory reported as the durable store root for a SQLite db path.
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

/// Store identity for a SQLite database path. Global metadata paths reuse their
/// `projects/<project_key>` component; local paths hash the database parent.
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
    store_id_for_path(parent)
}
