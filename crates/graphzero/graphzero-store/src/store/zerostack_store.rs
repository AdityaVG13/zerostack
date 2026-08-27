//! Unified `.zerostack/` store-root resolution for GraphZero snapshot data.
//!
//! Multi-project isolation: process-global `ZEROSTACK_STORE_ROOT` does **not**
//! pin every call root into one store by default. Default store is derived from
//! the call root. Shared/meta store requires explicit opt-in
//! (`GRAPHZERO_SHARED_STORE` / `ZEROSTACK_SHARED_STORE` = 1|on|true|yes).
//!
//! When a shared pin is outside the project root, graphzero data is further
//! namespaced under `projects/<project_key>/graphzero` so unrelated roots never
//! share facts or index snapshots.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use zero_store::{Engine, ResolvedStore, StoreEnv, StoreMode};

/// Env vars that opt in to using `ZEROSTACK_STORE_ROOT` as a shared meta store.
pub const SHARED_STORE_OPT_IN_ENVS: &[&str] = &["GRAPHZERO_SHARED_STORE", "ZEROSTACK_SHARED_STORE"];

/// Global pin env names (absolute store root for multi-project share when opt-in).
pub const STORE_ROOT_ENVS: &[&str] = zero_store::STORE_ROOT_ENVS;

/// Hex length of the stable project key used under a shared pin.
pub const PROJECT_KEY_HEX_LEN: usize = zero_store::PROJECT_KEY_HEX_LEN;

fn first_env(names: &[&str]) -> Option<OsString> {
    names.iter().find_map(std::env::var_os)
}

fn resolved(repo_root: &Path, pin: Option<&OsStr>, shared_opt_in: bool) -> ResolvedStore {
    ResolvedStore::resolve(
        repo_root,
        Engine::GraphZero,
        &StoreEnv::new(pin.map(OsString::from), shared_opt_in),
    )
}

fn pin_as_given(repo_root: &Path, pin: Option<&OsStr>) -> Option<PathBuf> {
    let pin = pin.filter(|value| !value.is_empty())?;
    let path = PathBuf::from(pin);
    Some(if path.is_absolute() {
        path
    } else {
        repo_root.join(path)
    })
}

/// Keep caller path spelling. Hub `ResolvedStore` absolutizes (and rch remaps
/// `/Users` → `/home`); GZ isolation tests and doctor reports compare against
/// the path the caller passed in.
fn graphzero_dir_for(repo_root: &Path, pin: Option<&OsStr>, chosen: &ResolvedStore) -> PathBuf {
    match chosen.mode() {
        StoreMode::Legacy => repo_root.join(Engine::GraphZero.legacy_dir_name()),
        StoreMode::LocalUnified => repo_root
            .join(zero_store::LOCAL_STORE_DIR)
            .join(Engine::GraphZero.dir_name()),
        StoreMode::PinnedInsideProject => pin_as_given(repo_root, pin)
            .unwrap_or_else(|| chosen.engine_dir().to_path_buf())
            .join(Engine::GraphZero.dir_name()),
        StoreMode::SharedNamespaced => pin_as_given(repo_root, pin)
            .unwrap_or_else(|| chosen.unified_root().unwrap().to_path_buf())
            .join(zero_store::PROJECTS_DIR)
            .join(project_store_key(repo_root))
            .join(Engine::GraphZero.dir_name()),
    }
}

/// Whether the process has opted into a shared/meta `ZEROSTACK_STORE_ROOT`.
pub fn shared_store_opt_in_from_env() -> bool {
    StoreEnv::from_process(&["GRAPHZERO_SHARED_STORE"]).shared_opt_in
}

/// Stable project key from a repo root path (sha256 prefix of the absolute path).
///
/// Canonicalization is preferred when the path exists; otherwise the absolute
/// (or as-given) path string is hashed so two distinct roots never collide.
pub fn project_store_key(repo_root: &Path) -> String {
    zero_store::project_key(repo_root)
}

/// Whether `store` is under `root` (same project). Used by doctor / namespacing.
pub fn store_is_under_project_root(store: &Path, root: &Path) -> bool {
    zero_store::store_is_under_project_root(store, root)
}

/// Pure store-root selection for the unified ZeroStack directory.
///
/// Algorithm:
/// 1. If `repo_root/.zerostack` exists as a directory → use it (project-local).
/// 2. Else if `shared_opt_in` and a non-empty store pin is set → use that pin
///    (absolute, or relative to `repo_root`).
/// 3. Else → `None` (caller falls back to legacy `repo_root/.graphzero`).
///
/// Global pin without opt-in is ignored so unrelated projects never share a
/// process-wide store by accident.
pub fn resolve_store_root_with_env(
    repo_root: &Path,
    store_root_pin: Option<&OsStr>,
    shared_opt_in: bool,
) -> Option<PathBuf> {
    match resolved(repo_root, store_root_pin, shared_opt_in).mode() {
        StoreMode::Legacy => None,
        StoreMode::LocalUnified => Some(repo_root.join(zero_store::LOCAL_STORE_DIR)),
        StoreMode::PinnedInsideProject | StoreMode::SharedNamespaced => {
            pin_as_given(repo_root, store_root_pin)
        }
    }
}

/// Map a unified ZeroStack store directory to the GraphZero subtree, namespacing
/// when the store sits outside the project root (shared meta).
pub fn graphzero_subdir_for_store(store: &Path, repo_root: &Path) -> PathBuf {
    if store_is_under_project_root(store, repo_root) {
        store.join("graphzero")
    } else {
        store
            .join("projects")
            .join(project_store_key(repo_root))
            .join("graphzero")
    }
}

/// Pure GraphZero snapshot store root (testable without process env).
pub fn resolve_graphzero_store_root_with_env(
    repo_root: &Path,
    store_root_pin: Option<&OsStr>,
    shared_opt_in: bool,
) -> PathBuf {
    graphzero_dir_for(
        repo_root,
        store_root_pin,
        &resolved(repo_root, store_root_pin, shared_opt_in),
    )
}

/// Active unified ZeroStack store directory, if any (live env).
///
/// Precedence: project-local `.zerostack` when present; else
/// `ZEROSTACK_STORE_ROOT` / `ZERO_STACK_STORE_ROOT` only with shared-store
/// opt-in.
pub fn zerostack_store_or_detect(repo_root: &Path) -> Option<PathBuf> {
    let env = StoreEnv::from_process(&["GRAPHZERO_SHARED_STORE"]);
    resolve_store_root_with_env(repo_root, env.pin.as_deref(), env.shared_opt_in)
}

/// GraphZero snapshot store root: namespaced under unified store when shared,
/// else `<repo>/.graphzero`.
pub fn resolve_graphzero_store_root(repo_root: &Path) -> PathBuf {
    let env = StoreEnv::from_process(&["GRAPHZERO_SHARED_STORE"]);
    resolve_graphzero_store_root_with_env(repo_root, env.pin.as_deref(), env.shared_opt_in)
}

/// True when a published index exists at the resolved store root (unified or legacy).
pub fn graphzero_index_present(repo_root: &Path) -> bool {
    super::manifest::manifest_path(&resolve_graphzero_store_root(repo_root)).exists()
}

/// Doctor / status snapshot of effective store resolution for a root.
#[derive(Debug, Clone)]
pub struct StoreResolutionReport {
    pub effective_graphzero_store: PathBuf,
    pub effective_zerostack_root: Option<PathBuf>,
    pub shared_store_opt_in: bool,
    pub global_pin_set: bool,
    pub global_pin_value: Option<PathBuf>,
    pub isolation_mode: &'static str,
    /// True when effective store is not under the project root (shared meta).
    pub store_project_mismatch: bool,
    pub warnings: Vec<String>,
}

/// Pure resolution report for tests and doctor.
pub fn store_resolution_report_with_env(
    repo_root: &Path,
    store_root_pin: Option<OsString>,
    shared_opt_in: bool,
) -> StoreResolutionReport {
    let global_pin_set = store_root_pin.as_ref().is_some_and(|v| !v.is_empty());
    let global_pin_value = store_root_pin
        .as_ref()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let chosen = resolved(repo_root, store_root_pin.as_deref(), shared_opt_in);
    let store = resolve_store_root_with_env(repo_root, store_root_pin.as_deref(), shared_opt_in);
    let effective_graphzero_store =
        graphzero_dir_for(repo_root, store_root_pin.as_deref(), &chosen);

    let store_outside = store
        .as_ref()
        .is_some_and(|s| !store_is_under_project_root(s, repo_root));
    let isolation_mode = match chosen.mode() {
        StoreMode::SharedNamespaced => "shared_namespaced",
        StoreMode::LocalUnified | StoreMode::PinnedInsideProject => "per_root_unified",
        StoreMode::Legacy => "per_root",
    };

    let store_project_mismatch = shared_opt_in && global_pin_set && store_outside;

    let mut warnings = Vec::new();
    if store_project_mismatch {
        warnings.push(format!(
            "shared/global store multi-root risk: effective store {} is outside project root {} (GRAPHZERO_SHARED_STORE / ZEROSTACK_SHARED_STORE opt-in). Graph facts and index are namespaced under projects/<project_key>/graphzero per root; still prefer per-root stores when possible.",
            store
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            repo_root.display()
        ));
    } else if global_pin_set && !shared_opt_in {
        warnings.push(format!(
            "ZEROSTACK_STORE_ROOT is set but ignored for isolation. Default store is under project root {}. Set GRAPHZERO_SHARED_STORE=1 (or ZEROSTACK_SHARED_STORE=1) to opt into the shared/meta store (data is then namespaced per project key).",
            repo_root.display()
        ));
    }

    StoreResolutionReport {
        effective_graphzero_store,
        effective_zerostack_root: store,
        shared_store_opt_in: shared_opt_in,
        global_pin_set,
        global_pin_value,
        isolation_mode,
        store_project_mismatch,
        warnings,
    }
}

/// Live-env doctor snapshot for a project root.
pub fn store_resolution_report(repo_root: &Path) -> StoreResolutionReport {
    store_resolution_report_with_env(
        repo_root,
        first_env(STORE_ROOT_ENVS),
        shared_store_opt_in_from_env(),
    )
}

/// JSON fragment for doctor / status surfaces.
pub fn store_resolution_json(repo_root: &Path) -> serde_json::Value {
    let r = store_resolution_report(repo_root);
    serde_json::json!({
        "schema_version": "graphzero.store_resolution.v1",
        "effective_graphzero_store": r.effective_graphzero_store.display().to_string(),
        "effective_zerostack_root": r.effective_zerostack_root.as_ref().map(|p| p.display().to_string()),
        "shared_store_opt_in": r.shared_store_opt_in,
        "global_pin_set": r.global_pin_set,
        "global_pin_value": r.global_pin_value.as_ref().map(|p| p.display().to_string()),
        "isolation_mode": r.isolation_mode,
        "store_project_mismatch": r.store_project_mismatch,
        "warnings": r.warnings,
        "project_key": project_store_key(repo_root),
        "algorithm": "1) repo_root/.zerostack if present → graphzero/; 2) else an opted-in ZEROSTACK_STORE_ROOT, project-namespaced when outside the repository; 3) else preserve an existing repo_root/.graphzero; 4) otherwise create repo_root/.zerostack/graphzero.",
        "opt_in_envs": SHARED_STORE_OPT_IN_ENVS,
        "store_root_envs": STORE_ROOT_ENVS,
    })
}
