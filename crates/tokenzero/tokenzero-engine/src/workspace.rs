//! Workspace root and cache-path resolution shared by CodeMode, MCP, and CLI.
//!
//! Frozen precedence invariants:
//! - workspace root: explicit root, then TOKENZERO_ROOT, then process cwd;
//! - cache path: explicit cache path, TOKENZERO_CACHE_PATH, project .zerostack,
//!   then project .tokenzero;
//! - store root: project .zerostack always wins over a shared environment pin;
//! - ZEROSTACK_STORE_ROOT and ZERO_STACK_STORE_ROOT are ignored unless
//!   TOKENZERO_SHARED_STORE or ZEROSTACK_SHARED_STORE explicitly opts in;
//! - relative shared pins resolve against the workspace root, and equal project
//!   basenames never share a store accidentally.
//!
//! Resolution delegates to the hub `zero_store` crate (tokenzero-mivh): one
//! algorithm, three engines. TokenZero contributes only its own opt-in alias
//! and directory names. An opted-in external pin is project-namespaced as
//! `<pin>/projects/<project-key>/tokenzero`, which replaces the old
//! collision-prone raw-pin cache path.
//!
//! The store_root_precedence integration tests pin every ordering and isolation
//! case. Resolution intentionally does not require a selected path to exist.

use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use zero_store::{
    Engine, ResolvedStore, StoreEnv, StoreMode,
    store_is_under_project_root as zero_store_store_is_under_project_root,
};

/// Env vars that opt in to using `ZEROSTACK_STORE_ROOT` as a shared meta store.
pub const SHARED_STORE_OPT_IN_ENVS: &[&str] = &["TOKENZERO_SHARED_STORE", "ZEROSTACK_SHARED_STORE"];

/// Global pin env names.
pub const STORE_ROOT_ENVS: &[&str] = &["ZEROSTACK_STORE_ROOT", "ZERO_STACK_STORE_ROOT"];

/// Engine-local opt-in aliases passed to the hub resolver. The hub already
/// reads `ZEROSTACK_SHARED_STORE`; TokenZero keeps its own alias in addition.
const ENGINE_OPT_IN_ALIASES: &[&str] = &["TOKENZERO_SHARED_STORE"];

/// Workspace root for TokenZero persistence (CLI, CodeMode, MCP).
pub fn tokenzero_work_root(explicit_root: Option<PathBuf>) -> PathBuf {
    explicit_root
        .or_else(|| std::env::var_os("TOKENZERO_ROOT").map(PathBuf::from))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// Default single-root allowlist for a workspace.
pub fn default_allowed_roots(root: &Path) -> Vec<PathBuf> {
    vec![root.to_path_buf()]
}

fn push_unique_path(paths: &mut Vec<PathBuf>, candidate: PathBuf) {
    let candidate_cmp = candidate
        .canonicalize()
        .unwrap_or_else(|_| candidate.clone());
    let exists = paths
        .iter()
        .any(|path| path.canonicalize().unwrap_or_else(|_| path.clone()) == candidate_cmp);
    if !exists {
        paths.push(candidate);
    }
}

/// Merge explicit allowed roots with the workspace root, deduplicating by canonical path.
pub fn allowed_roots_for_workspace(root: &Path, explicit: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = if explicit.is_empty() {
        default_allowed_roots(root)
    } else {
        explicit.to_vec()
    };
    push_unique_path(&mut roots, root.to_path_buf());
    roots
}

fn env_truthy(value: &OsStr) -> bool {
    let raw = value.to_string_lossy();
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "on" | "true" | "yes"
    )
}

/// Whether the process has opted into a shared/meta `ZEROSTACK_STORE_ROOT`.
pub fn shared_store_opt_in_from_env() -> bool {
    for name in SHARED_STORE_OPT_IN_ENVS {
        if let Some(v) = env::var_os(name)
            && env_truthy(&v)
        {
            return true;
        }
    }
    false
}

fn first_env(names: &[&str]) -> Option<OsString> {
    names.iter().find_map(env::var_os)
}

/// Pure store-root selection; the frozen precedence is documented at module level.
///
/// Delegates to the hub `zero_store::ResolvedStore`. `Legacy` is returned only
/// for an existing `.tokenzero`; new repositories resolve to project-local
/// `.zerostack` or an explicitly opted-in pin.
pub fn resolve_store_root_with_env(
    repo_root: &Path,
    store_root_pin: Option<&OsStr>,
    shared_opt_in: bool,
) -> Option<PathBuf> {
    let env = StoreEnv::new(store_root_pin.map(OsString::from), shared_opt_in);
    let resolved = ResolvedStore::resolve(repo_root, Engine::TokenZero, &env);
    match resolved.mode() {
        StoreMode::Legacy => None,
        _ => resolved.unified_root().map(Path::to_path_buf),
    }
}

/// Default recovery cache for a root under an explicit hub environment.
///
/// Once a unified store resolves (project-local `.zerostack` or an opted-in
/// pin), the cache is the hub engine file
/// `<store>/tokenzero/recovery-cache.json` unconditionally, including the
/// project-namespaced `<pin>/projects/<project-key>/tokenzero` shape for
/// external pins. This removes the old history-dependent legacy tiebreak that
/// made two processes pick different files for the same repo.
pub fn default_recovery_cache_path_with_env(repo_root: &Path, env: &StoreEnv) -> PathBuf {
    let resolved = ResolvedStore::resolve(repo_root, Engine::TokenZero, env);
    match resolved.mode() {
        StoreMode::Legacy => resolved.engine_dir().join("recovery-cache.json"),
        _ => resolved
            .engine_file("recovery-cache.json")
            .unwrap_or_else(|_| resolved.engine_dir().join("recovery-cache.json")),
    }
}

/// Default recovery cache when --cache-path is omitted.
///
/// After wqw.8 this is the single shared store for CLI expand and CodeMode.
pub fn default_recovery_cache_path(repo_root: &Path) -> PathBuf {
    default_recovery_cache_path_with_env(repo_root, &StoreEnv::from_process(ENGINE_OPT_IN_ALIASES))
}

/// Honor explicit --cache-path, then TOKENZERO_CACHE_PATH, then the default cache.
pub fn resolve_recovery_cache_path(repo_root: &Path, explicit: Option<PathBuf>) -> PathBuf {
    resolve_recovery_cache_path_with_env(repo_root, explicit, env::var_os("TOKENZERO_CACHE_PATH"))
}

pub fn resolve_recovery_cache_path_with_env(
    repo_root: &Path,
    explicit: Option<PathBuf>,
    env_value: Option<OsString>,
) -> PathBuf {
    explicit
        .or_else(|| {
            env_value
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| default_recovery_cache_path(repo_root))
}

/// Whether `store` is under `root` (same project). Delegates to the hub
/// spelling-stable containment check.
pub fn store_is_under_project_root(store: &Path, root: &Path) -> bool {
    zero_store_store_is_under_project_root(store, root)
}

/// Doctor / status snapshot of effective store resolution for a root.
#[derive(Debug, Clone)]
pub struct StoreResolutionReport {
    pub effective_cache_path: PathBuf,
    pub effective_store_root: Option<PathBuf>,
    pub shared_store_opt_in: bool,
    pub global_pin_set: bool,
    pub global_pin_value: Option<PathBuf>,
    pub isolation_mode: &'static str,
    /// True when the effective store is an un-namespaced external root.
    /// Hub project namespacing makes this impossible for active pins, so it
    /// is always false today; the field is retained for the doctor contract.
    pub store_project_mismatch: bool,
    pub mismatch_summary: Option<String>,
    /// Exact hub store-mode wire label (additive).
    pub store_mode: Option<&'static str>,
    /// Hub project key, present only in `shared_namespaced` mode (additive).
    pub project_key: Option<String>,
    /// Resolved engine directory, e.g. `<store>/tokenzero` or `.tokenzero`.
    pub effective_engine_dir: PathBuf,
    /// Resolved CAS host: the unified store root, or the engine directory in
    /// legacy mode.
    pub cas_host: PathBuf,
}

/// Pure resolution report for tests and doctor.
pub fn store_resolution_report_with_env(
    repo_root: &Path,
    explicit_cache: Option<PathBuf>,
    tokenzero_cache_path: Option<OsString>,
    store_root_pin: Option<OsString>,
    shared_opt_in: bool,
) -> StoreResolutionReport {
    let global_pin_set = store_root_pin.as_ref().is_some_and(|v| !v.is_empty());
    let global_pin_value = store_root_pin
        .as_ref()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    // One explicit hub environment drives the resolved store, the effective
    // store root, and the default cache path, so the report can never drift
    // from the effective store by re-reading live process env (tokenzero-mivh).
    let env = StoreEnv::new(store_root_pin, shared_opt_in);
    let resolved = ResolvedStore::resolve(repo_root, Engine::TokenZero, &env);
    let store = match resolved.mode() {
        StoreMode::Legacy => None,
        _ => resolved.unified_root().map(Path::to_path_buf),
    };
    let had_explicit =
        explicit_cache.is_some() || tokenzero_cache_path.as_ref().is_some_and(|v| !v.is_empty());
    let effective_cache_path = explicit_cache
        .or_else(|| {
            tokenzero_cache_path
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| default_recovery_cache_path_with_env(repo_root, &env));
    let effective_store_root = store.clone();
    let isolation_mode = if had_explicit {
        "explicit_cache"
    } else if shared_opt_in
        && global_pin_set
        && store
            .as_ref()
            .is_some_and(|s| !store_is_under_project_root(s, repo_root))
    {
        "shared_opt_in"
    } else {
        "per_root"
    };

    // Hub project namespacing isolates external pins by project key, so an
    // active shared pin is never a project mismatch and must not surface a
    // `tz-store-project-mismatch` warning. The ignored-pin info finding is
    // retained below.
    let store_project_mismatch = false;
    let mismatch_summary = if global_pin_set && !shared_opt_in {
        Some(format!(
            "ZEROSTACK_STORE_ROOT is set but ignored for isolation (wqw.2). Default store is under project root {}. Set TOKENZERO_SHARED_STORE=1 (or ZEROSTACK_SHARED_STORE=1) to opt into the shared/meta store.",
            repo_root.display()
        ))
    } else {
        None
    };

    StoreResolutionReport {
        effective_cache_path,
        effective_store_root,
        shared_store_opt_in: shared_opt_in,
        global_pin_set,
        global_pin_value,
        isolation_mode,
        store_project_mismatch,
        mismatch_summary,
        store_mode: Some(resolved.mode().as_str()),
        project_key: resolved.project_key().map(str::to_string),
        effective_engine_dir: resolved.engine_dir().to_path_buf(),
        cas_host: resolved.cas_host().to_path_buf(),
    }
}

/// Live-env doctor snapshot for a project root.
pub fn store_resolution_report(
    repo_root: &Path,
    explicit_cache: Option<PathBuf>,
) -> StoreResolutionReport {
    store_resolution_report_with_env(
        repo_root,
        explicit_cache,
        env::var_os("TOKENZERO_CACHE_PATH"),
        first_env(STORE_ROOT_ENVS),
        shared_store_opt_in_from_env(),
    )
}

/// JSON fragment for doctor / status surfaces.
pub fn store_resolution_json(
    repo_root: &Path,
    explicit_cache: Option<PathBuf>,
) -> serde_json::Value {
    let r = store_resolution_report(repo_root, explicit_cache);
    serde_json::json!({
        "schema_version": "tokenzero.store_resolution.v1",
        "effective_cache_path": r.effective_cache_path.display().to_string(),
        "effective_store_root": r.effective_store_root.as_ref().map(|p| p.display().to_string()),
        "shared_store_opt_in": r.shared_store_opt_in,
        "global_pin_set": r.global_pin_set,
        "global_pin_value": r.global_pin_value.as_ref().map(|p| p.display().to_string()),
        "isolation_mode": r.isolation_mode,
        "store_project_mismatch": r.store_project_mismatch,
        "mismatch_summary": r.mismatch_summary,
        "store_mode": r.store_mode,
        "project_key": r.project_key,
        "effective_engine_dir": r.effective_engine_dir.display().to_string(),
        "cas_host": r.cas_host.display().to_string(),
        "algorithm": "1) repo_root/.zerostack if present; 2) else an opted-in ZEROSTACK_STORE_ROOT; 3) else an existing legacy repo_root/.tokenzero; 4) otherwise create repo_root/.zerostack/tokenzero. Explicit cache paths still win; external pins remain project-namespaced.",
        "opt_in_envs": SHARED_STORE_OPT_IN_ENVS,
        "store_root_envs": STORE_ROOT_ENVS,
    })
}
