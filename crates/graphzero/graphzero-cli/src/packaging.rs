//! Mutually exclusive package surfaces for GraphZero (graphzero-o2uq.3).
//!
//! Product rule: users install the hub-backed `graphzero-mcp` compatibility
//! surface or the planner-free `graphzero-worker` raw worker. The installer
//! writes one client registration and replaces any prior surface. Standalone
//! CodeMode server startup fails closed.
//!
//! **Compile-time exclusivity:** `surface-mcp` is the only linked server
//! feature. `surface-codemode` remains an empty compatibility sentinel for
//! packaging checks; it never links a JavaScript runtime or server.

#[cfg(all(feature = "surface-mcp", feature = "surface-codemode"))]
compile_error!(
    "graphzero: surface-mcp and surface-codemode are mutually exclusive. \
Build graphzero-mcp with --features tokenzero,surface-mcp OR graphzero-codemode \
with --features tokenzero,surface-codemode. The graphzero binary is an installer \
shim only — never a dual-runtime package."
);

use graphzero_engine::operation_abi::{SEMANTIC_CONTRACT_VERSION, contract_digest_hex};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Package artifact names (release binaries / packages).
pub const ARTIFACT_MCP: &str = "graphzero-mcp";
pub const ARTIFACT_CODEMODE: &str = "graphzero-codemode";
/// Compatibility shim name — never exposes both surfaces itself.
pub const ARTIFACT_SHIM: &str = "graphzero";

/// Reject network-transport flags before either packaged MCP surface starts.
///
/// GraphZero servers are stdio-only and inherit the local OS user's authority.
/// Remote access belongs in an authenticated gateway, never an implicit socket
/// listener inside the engine process.
pub fn assert_stdio_only_args(args: &[String]) -> Result<(), String> {
    const NETWORK_FLAGS: &[&str] = &[
        "--bind",
        "--host",
        "--listen",
        "--port",
        "--transport",
        "--http",
        "--tcp",
        "--sse",
        "--websocket",
    ];
    if let Some(argument) = args.iter().skip(1).find(|argument| {
        NETWORK_FLAGS.iter().any(|flag| {
            argument.as_str() == *flag
                || argument
                    .strip_prefix(flag)
                    .is_some_and(|suffix| suffix.starts_with('='))
        })
    }) {
        return Err(format!(
            "refused network transport argument {argument:?}: GraphZero is stdio-only; use an authenticated token/mTLS gateway for remote access"
        ));
    }
    Ok(())
}

/// Install-state filename under the install prefix / config dir.
pub const INSTALL_STATE_FILE: &str = "install-state.json";

/// Client registration file written by the installer (single surface).
pub const CLIENT_CONFIG_FILE: &str = "client-config.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageSurface {
    Mcp,
    Codemode,
}

impl PackageSurface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
            Self::Codemode => "codemode",
        }
    }

    pub fn artifact_name(self) -> &'static str {
        match self {
            Self::Mcp => ARTIFACT_MCP,
            Self::Codemode => ARTIFACT_CODEMODE,
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mcp" | "fastmcp" | "per-op" | "per_op" | ARTIFACT_MCP => Ok(Self::Mcp),
            "codemode" | "code-mode" | "code_mode" | ARTIFACT_CODEMODE => Ok(Self::Codemode),
            other => Err(format!(
                "unknown package surface {other:?}; require 'mcp' or 'codemode' (artifacts {ARTIFACT_MCP} / {ARTIFACT_CODEMODE})"
            )),
        }
    }

    /// Selection matrix for client install docs. Both client classes use the
    /// hub MCP compatibility surface; the raw worker is hosted separately.
    pub fn recommended_for_client(_native_codemode_client: bool) -> Self {
        Self::Mcp
    }
}

impl std::fmt::Display for PackageSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Compile-time surfaces enabled in this binary (feature matrix).
///
/// Shim profile (default): empty. Exactly one surface for release artifacts.
/// Dual surface is a `compile_error!` (see module docs).
pub fn compile_time_surfaces() -> Vec<PackageSurface> {
    let mut out = Vec::new();
    #[cfg(feature = "surface-mcp")]
    out.push(PackageSurface::Mcp);
    #[cfg(feature = "surface-codemode")]
    out.push(PackageSurface::Codemode);
    out
}

/// True when this build is the compatibility shim (no package surface linked).
pub fn is_compatibility_shim_build() -> bool {
    compile_time_surfaces().is_empty()
}

/// Immutable package surface for single-surface release binaries.
pub fn baked_package_surface() -> Option<PackageSurface> {
    let surfaces = compile_time_surfaces();
    if surfaces.len() == 1 {
        return Some(surfaces[0]);
    }
    if let Ok(name) = env::current_exe() {
        if let Some(stem) = name.file_name().and_then(|s| s.to_str()) {
            if stem == ARTIFACT_MCP || stem.starts_with("graphzero-mcp") {
                return Some(PackageSurface::Mcp);
            }
            if stem == ARTIFACT_CODEMODE || stem.starts_with("graphzero-codemode") {
                return Some(PackageSurface::Codemode);
            }
        }
    }
    if let Ok(v) = env::var("GRAPHZERO_PACKAGE_SURFACE") {
        if let Ok(s) = PackageSurface::parse(&v) {
            // Env cannot invent a surface that was not compiled into this artifact.
            if surface_compiled_in(s) {
                return Some(s);
            }
        }
    }
    None
}

/// Combined semantic contract digest advertised by help/doctor/SBOM/uninstall.
pub fn semantic_contract_digest() -> String {
    contract_digest_hex()
}

/// Whether the hub-backed MCP transport is compiled into this binary.
pub fn fastmcp_runtime_linked() -> bool {
    cfg!(feature = "surface-mcp")
}

/// GraphZero never links a JavaScript runtime.
pub fn codemode_js_runtime_linked() -> bool {
    false
}

/// Static packaging report for docs / doctor / SBOM assertions.
pub fn runtime_dependency_matrix() -> serde_json::Value {
    serde_json::json!({
        "zero_mcp_fastmcp": fastmcp_runtime_linked(),
        "graphzero_engine_js_runtime_compiled": graphzero_engine::codemode::js_runtime_compiled(),
        "raw_worker": "graphzero-worker (crates/graphzero/graphzero-codemode)",
        "exclusivity_rule": "graphzero-mcp links hub zero-mcp (fastmcp carrier); graphzero-worker is raw-worker-v2 only; GraphZero query retains recipe/JSON-DAG execution",
        "build_matrix": {
            "graphzero": "--features tokenzero  # compatibility shim only",
            "graphzero-mcp": "--features tokenzero,surface-mcp  # hub transport",
            "graphzero-worker": "cargo build -p graphzero-worker --bin graphzero-codemode",
            "surface-codemode": "empty compatibility sentinel",
            "dual_surfaces": "compile_error"
        },
        "is_compatibility_shim_build": is_compatibility_shim_build()
    })
}

/// Fail closed if a single-surface package claim disagrees with linked runtimes.
///
/// Unconditional: dual surface features are already a compile error; this also
/// rejects peer runtime flags and shim-only builds used as servers.
pub fn assert_surface_runtime_exclusivity(surface: PackageSurface) -> Result<(), String> {
    // Process-level dual selection (env/argv) always rejected.
    reject_dual_env_selection()?;
    if compile_time_surfaces().len() > 1 {
        return Err(dual_surface_diagnostic(
            "binary compiled with more than one package surface (impossible if feature matrix is correct)",
        ));
    }
    match surface {
        PackageSurface::Mcp => {
            if !fastmcp_runtime_linked() {
                return Err(
                    "graphzero-mcp package claim requires surface-mcp / hub zero-mcp fastmcp carrier".into(),
                );
            }
            // CLI feature exclusivity (compile_error already blocks dual surfaces).
            if codemode_js_runtime_linked() {
                return Err(dual_surface_diagnostic(
                    "graphzero-mcp must not enable surface-codemode; rebuild with --features tokenzero,surface-mcp only",
                ));
            }
        }
        PackageSurface::Codemode => {
            if fastmcp_runtime_linked() {
                return Err(dual_surface_diagnostic(
                    "graphzero-codemode raw-worker package must not enable surface-mcp; build graphzero-worker without server features",
                ));
            }
            if codemode_js_runtime_linked() || graphzero_engine::codemode::js_runtime_compiled() {
                return Err(
                    "graphzero-codemode raw-worker package must not compile a JavaScript runtime"
                        .into(),
                );
            }
        }
    }
    Ok(())
}

/// Package identity block shared by doctor / help / SBOM / uninstall.
pub fn package_identity(surface: PackageSurface) -> serde_json::Value {
    serde_json::json!({
        "artifact": surface.artifact_name(),
        "surface": surface.as_str(),
        "shim": ARTIFACT_SHIM,
        "package_version": env!("CARGO_PKG_VERSION"),
        "semantic_contract_version": SEMANTIC_CONTRACT_VERSION,
        "semantic_contract_digest": semantic_contract_digest(),
        "engine": "graphzero",
        "runtime_dependencies": runtime_dependency_matrix(),
        "selection_matrix": {
            "native_codemode_client": ARTIFACT_MCP,
            "raw_worker": ARTIFACT_CODEMODE,
            "rule": "hub zero-codemode hosts JavaScript; hub zero-mcp hosts MCP transport; graphzero-worker executes raw-worker-v2; never run a standalone GraphZero CodeMode server"
        }
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallState {
    pub surface: PackageSurface,
    pub artifact: String,
    pub binary_path: String,
    pub prefix: String,
    pub semantic_contract_digest: String,
    pub package_version: String,
    pub installed_at_unix: u64,
    pub platform: String,
    /// Client config path written by this install (relative or absolute).
    pub client_config: String,
}

impl InstallState {
    pub fn for_surface(surface: PackageSurface, prefix: &Path, binary_path: &Path) -> Self {
        Self {
            surface,
            artifact: surface.artifact_name().to_string(),
            binary_path: binary_path.display().to_string(),
            prefix: prefix.display().to_string(),
            semantic_contract_digest: semantic_contract_digest(),
            package_version: env!("CARGO_PKG_VERSION").to_string(),
            installed_at_unix: now_unix(),
            platform: current_platform().to_string(),
            client_config: prefix.join(CLIENT_CONFIG_FILE).display().to_string(),
        }
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn current_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "other"
    }
}

/// Default install prefix: `$GRAPHZERO_INSTALL_PREFIX` or `~/.graphzero-install`.
pub fn default_install_prefix() -> PathBuf {
    if let Ok(p) = env::var("GRAPHZERO_INSTALL_PREFIX") {
        return PathBuf::from(p);
    }
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".graphzero-install");
    }
    PathBuf::from(".graphzero-install")
}

pub fn install_state_path(prefix: &Path) -> PathBuf {
    prefix.join(INSTALL_STATE_FILE)
}

pub fn load_install_state(prefix: &Path) -> Result<Option<InstallState>, String> {
    let path = install_state_path(prefix);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("read install state: {e}"))?;
    let state: InstallState =
        serde_json::from_str(&raw).map_err(|e| format!("parse install state: {e}"))?;
    Ok(Some(state))
}

pub fn write_install_state(prefix: &Path, state: &InstallState) -> Result<(), String> {
    fs::create_dir_all(prefix).map_err(|e| format!("create install prefix: {e}"))?;
    let path = install_state_path(prefix);
    let raw = serde_json::to_string_pretty(state).map_err(|e| format!("serialize state: {e}"))?;
    atomic_write(&path, raw.as_bytes()).map_err(|e| format!("write install state: {e}"))
}

/// Client registration payload — single surface only.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientConfig {
    pub name: String,
    pub surface: PackageSurface,
    pub command: String,
    pub args: Vec<String>,
    pub semantic_contract_digest: String,
    pub package_version: String,
}

pub fn client_config_for(surface: PackageSurface, binary_path: &Path) -> ClientConfig {
    let (command, args) = match surface {
        PackageSurface::Mcp => (
            binary_path.display().to_string(),
            vec!["--mode=mcp".to_string()],
        ),
        PackageSurface::Codemode => (binary_path.display().to_string(), Vec::new()),
    };
    ClientConfig {
        name: format!("GraphZero ({})", surface.as_str()),
        surface,
        command,
        args,
        semantic_contract_digest: semantic_contract_digest(),
        package_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

pub fn write_client_config(prefix: &Path, config: &ClientConfig) -> Result<PathBuf, String> {
    fs::create_dir_all(prefix).map_err(|e| format!("create install prefix: {e}"))?;
    let path = prefix.join(CLIENT_CONFIG_FILE);
    let raw = serde_json::to_string_pretty(config).map_err(|e| format!("serialize client: {e}"))?;
    atomic_write(&path, raw.as_bytes()).map_err(|e| format!("write client config: {e}"))?;
    Ok(path)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Validate that `binary_path` is a usable executable for install.
pub fn validate_install_binary(binary_path: &Path) -> Result<(), String> {
    if !binary_path.exists() {
        return Err(format!(
            "install binary does not exist: {}",
            binary_path.display()
        ));
    }
    let meta = fs::metadata(binary_path).map_err(|e| format!("stat install binary: {e}"))?;
    if !meta.is_file() {
        return Err(format!(
            "install binary is not a regular file: {}",
            binary_path.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        if mode & 0o111 == 0 {
            return Err(format!(
                "install binary is not executable: {}",
                binary_path.display()
            ));
        }
    }
    Ok(())
}

/// Install one surface. Replaces prior surface cleanly (binary selection + client config).
///
/// Atomic: writes state/config to temp files then renames. On failure after a
/// peer uninstall, does not leave dual client registrations.
pub fn install_surface(
    surface: PackageSurface,
    prefix: &Path,
    binary_path: &Path,
) -> Result<InstallState, String> {
    reject_dual_env_selection()?;
    validate_install_binary(binary_path)?;

    // Snapshot prior state for rollback if write fails mid-flight.
    let prev = load_install_state(prefix)?;
    let prev_cfg = prefix.join(CLIENT_CONFIG_FILE);
    let prev_cfg_backup = if prev_cfg.exists() {
        let raw = fs::read(&prev_cfg).ok();
        raw
    } else {
        None
    };
    let prev_state_raw = {
        let p = install_state_path(prefix);
        if p.exists() {
            fs::read_to_string(&p).ok()
        } else {
            None
        }
    };
    let prev_shim = prefix.join("shim-target");
    let prev_shim_raw = if prev_shim.exists() {
        fs::read_to_string(&prev_shim).ok()
    } else {
        None
    };

    if let Some(prev) = &prev {
        if prev.surface != surface {
            let _ = uninstall_surface(prefix);
        }
    }

    let config = client_config_for(surface, binary_path);
    let state = InstallState::for_surface(surface, prefix, binary_path);

    if let Err(e) = (|| -> Result<(), String> {
        write_client_config(prefix, &config)?;
        write_install_state(prefix, &state)?;
        let shim_path = prefix.join("shim-target");
        atomic_write(&shim_path, surface.as_str().as_bytes())
            .map_err(|err| format!("write shim-target: {err}"))?;
        Ok(())
    })() {
        // Rollback best-effort to prior single-surface state.
        if let Some(raw) = prev_state_raw {
            let _ = atomic_write(&install_state_path(prefix), raw.as_bytes());
        }
        if let Some(bytes) = prev_cfg_backup {
            let _ = atomic_write(&prev_cfg, &bytes);
        }
        if let Some(raw) = prev_shim_raw {
            let _ = atomic_write(&prev_shim, raw.as_bytes());
        }
        return Err(format!(
            "install failed (rolled back prior state if present): {e}"
        ));
    }

    Ok(state)
}

/// Paths uninstall removes under a prefix (install state, client config, shim).
pub fn uninstall_target_paths(prefix: &Path) -> [PathBuf; 3] {
    [
        install_state_path(prefix),
        prefix.join(CLIENT_CONFIG_FILE),
        prefix.join("shim-target"),
    ]
}

/// Preview uninstall without mutating the filesystem.
pub fn uninstall_surface_dry_run(prefix: &Path) -> Result<serde_json::Value, String> {
    let prev = load_install_state(prefix)?;
    let would_remove: Vec<String> = uninstall_target_paths(prefix)
        .into_iter()
        .filter(|path| path.exists())
        .map(|path| path.display().to_string())
        .collect();
    Ok(serde_json::json!({
        "dry_run": true,
        "would_remove": would_remove,
        "previous": uninstall_report(prev),
    }))
}

/// Remove install state, client config, and shim marker for this prefix.
/// Never starts a server. Returns previous state when present.
pub fn uninstall_surface(prefix: &Path) -> Result<Option<InstallState>, String> {
    let prev = load_install_state(prefix)?;
    let mut errs: Vec<String> = Vec::new();
    for path in uninstall_target_paths(prefix) {
        if path.exists() {
            if let Err(e) = fs::remove_file(&path) {
                errs.push(format!("remove {}: {e}", path.display()));
            }
        }
    }
    if !errs.is_empty() {
        return Err(format!("uninstall incomplete: {}", errs.join("; ")));
    }
    Ok(prev)
}

/// Fail closed for package/server entry: exactly one matching surface feature.
pub fn assert_packaged_surface_features(locked: PackageSurface) -> Result<(), String> {
    let surfaces = compile_time_surfaces();
    if surfaces.is_empty() {
        return Err(format!(
            "graphzero: compatibility shim has no package surface; install {} or rebuild with --features tokenzero,surface-{}",
            locked.artifact_name(),
            locked.as_str()
        ));
    }
    if surfaces.len() > 1 {
        // Unconditional (compile_error should already prevent this build).
        return Err(dual_surface_diagnostic(
            "binary compiled with both surface-mcp and surface-codemode; rebuild one surface only",
        ));
    }
    if !surfaces.contains(&locked) {
        return Err(format!(
            "graphzero: package surface '{}' was not compiled into this binary. Rebuild with --features surface-{} or install {}.",
            locked.as_str(),
            locked.as_str(),
            locked.artifact_name()
        ));
    }
    Ok(())
}

/// Catalog / server boundary: dual selection and dual runtimes always fail closed.
pub fn assert_server_surface_boundary(surface: PackageSurface) -> Result<(), String> {
    reject_dual_env_selection()?;
    if is_compatibility_shim_build() {
        return Err(format!(
            "graphzero: the compatibility shim does not host MCP/CodeMode servers. \
Install {} (selected surface) or rebuild the matching single-surface artifact.",
            surface.artifact_name()
        ));
    }
    assert_surface_compiled(surface)?;
    assert_packaged_surface_features(surface)?;
    assert_surface_runtime_exclusivity(surface)?;
    Ok(())
}

/// Detect dual surface selection attempts (env) and fail closed.
pub fn reject_dual_env_selection() -> Result<(), String> {
    if let Ok(v) = env::var("GRAPHZERO_SURFACE") {
        let lower = v.to_ascii_lowercase();
        if lower.contains(',')
            || lower.contains('+')
            || lower == "both"
            || lower == "all"
            || (lower.contains("mcp") && lower.contains("codemode"))
        {
            return Err(dual_surface_diagnostic(
                "GRAPHZERO_SURFACE requests more than one package surface",
            ));
        }
    }
    if env::var_os("GRAPHZERO_ENABLE_MCP").is_some()
        && env::var_os("GRAPHZERO_ENABLE_CODEMODE").is_some()
    {
        return Err(dual_surface_diagnostic(
            "both GRAPHZERO_ENABLE_MCP and GRAPHZERO_ENABLE_CODEMODE are set",
        ));
    }
    Ok(())
}

pub fn dual_surface_diagnostic(detail: &str) -> String {
    format!(
        "graphzero: dual package surface rejected (fail closed): {detail}. \
Install exactly one artifact ({ARTIFACT_MCP} or {ARTIFACT_CODEMODE}); \
native CodeMode clients install {ARTIFACT_MCP}; legacy MCP clients install {ARTIFACT_CODEMODE}. \
Do not register both catalogs in one client session."
    )
}

/// Parse CLI args for dual `--mode=` selection.
pub fn modes_from_args(args: &[String]) -> Result<Option<PackageSurface>, String> {
    let mut mcp = false;
    let mut codemode = false;
    for a in args {
        if let Some(rest) = a.strip_prefix("--mode=") {
            match rest {
                "mcp" => mcp = true,
                "codemode" => codemode = true,
                "both" | "all" | "mcp+codemode" | "codemode+mcp" => {
                    return Err(dual_surface_diagnostic(&format!("invalid --mode={rest}")));
                }
                _ => {}
            }
        }
        if a == "--mode" {
            // handled via adjacent token below
        }
    }
    // Also handle split form: --mode mcp
    let mut i = 0;
    while i + 1 < args.len() {
        if args[i] == "--mode" {
            match args[i + 1].as_str() {
                "mcp" => mcp = true,
                "codemode" => codemode = true,
                "both" | "all" => {
                    return Err(dual_surface_diagnostic(&format!(
                        "invalid --mode {}",
                        args[i + 1]
                    )));
                }
                _ => {}
            }
        }
        i += 1;
    }
    if mcp && codemode {
        return Err(dual_surface_diagnostic(
            "both --mode=mcp and --mode=codemode present on argv",
        ));
    }
    if mcp {
        return Ok(Some(PackageSurface::Mcp));
    }
    if codemode {
        return Ok(Some(PackageSurface::Codemode));
    }
    Ok(None)
}

/// Resolve the single surface this process is allowed to start.
pub fn resolve_startup_surface(args: &[String]) -> Result<PackageSurface, String> {
    reject_dual_env_selection()?;
    let from_args = modes_from_args(args)?;

    if let Some(baked) = baked_package_surface() {
        if let Some(requested) = from_args {
            if requested != baked {
                return Err(format!(
                    "graphzero: artifact {} is locked to surface '{}'; refused request for '{}'. \
Reinstall the matching package or use the {} compatibility shim after selecting one surface.",
                    baked.artifact_name(),
                    baked.as_str(),
                    requested.as_str(),
                    ARTIFACT_SHIM
                ));
            }
        }
        return Ok(baked);
    }

    if let Some(s) = from_args {
        return Ok(s);
    }

    if let Ok(v) = env::var("GRAPHZERO_SURFACE") {
        return PackageSurface::parse(&v);
    }

    let prefix = default_install_prefix();
    if let Some(state) = load_install_state(&prefix)? {
        return Ok(state.surface);
    }

    let shim = prefix.join("shim-target");
    if let Ok(raw) = fs::read_to_string(&shim) {
        return PackageSurface::parse(raw.trim());
    }

    // Single-surface artifact default when no install state / mode flag.
    #[cfg(feature = "surface-mcp")]
    {
        return Ok(PackageSurface::Mcp);
    }
    #[cfg(all(not(feature = "surface-mcp"), feature = "surface-codemode"))]
    {
        return Ok(PackageSurface::Codemode);
    }
    // Compatibility shim: no baked package surface (not a dual runtime).
    Err(
        "graphzero: compatibility shim has no baked package surface; \
install graphzero-mcp or graphzero-codemode (or invoke that artifact), \
or pass --mode= only after install state selects one surface"
            .into(),
    )
}

/// Whether this build can start the given surface (feature matrix).
pub fn surface_compiled_in(surface: PackageSurface) -> bool {
    compile_time_surfaces().contains(&surface)
}

pub fn assert_surface_compiled(surface: PackageSurface) -> Result<(), String> {
    if surface_compiled_in(surface) {
        Ok(())
    } else {
        Err(format!(
            "graphzero: package surface '{}' was not compiled into this binary (artifact features). \
Rebuild with --features surface-{} or install {}.",
            surface.as_str(),
            surface.as_str(),
            surface.artifact_name()
        ))
    }
}

/// Software bill of materials identity for the selected surface.
pub fn sbom_document(surface: PackageSurface) -> serde_json::Value {
    let mut identity = package_identity(surface);
    if let serde_json::Value::Object(ref mut map) = identity {
        map.insert(
            "sbom".into(),
            serde_json::json!({
                "format": "graphzero-sbom-v1",
                "components": [
                    {
                        "type": "application",
                        "name": surface.artifact_name(),
                        "version": env!("CARGO_PKG_VERSION"),
                        "surface": surface.as_str(),
                    },
                    {
                        "type": "library",
                        "name": "graphzero-engine",
                        "version": env!("CARGO_PKG_VERSION"),
                        "role": "shared-core",
                        "semantic_contract_digest": semantic_contract_digest(),
                    }
                ],
                "platform": current_platform(),
                "mutually_exclusive_with": match surface {
                    PackageSurface::Mcp => ARTIFACT_CODEMODE,
                    PackageSurface::Codemode => ARTIFACT_MCP,
                }
            }),
        );
    }
    identity
}

/// Uninstall report identity line for CLI.
pub fn uninstall_report(prev: Option<InstallState>) -> serde_json::Value {
    match prev {
        Some(s) => serde_json::json!({
            "uninstalled": true,
            "artifact": s.artifact,
            "surface": s.surface.as_str(),
            "semantic_contract_digest": s.semantic_contract_digest,
            "package_version": s.package_version,
            "prefix": s.prefix,
        }),
        None => serde_json::json!({
            "uninstalled": false,
            "reason": "no install state present",
        }),
    }
}
