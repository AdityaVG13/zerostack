//! Package identities for classic MCP compatibility and the planner-free raw worker.
//!
//! `tokenzero-mcp` is an explicit classic MCP compatibility artifact. The
//! `tokenzero-codemode` artifact name is retained for rollout compatibility, but
//! its package semantic is `raw-worker`: ZeroStack owns discovery, registration,
//! planning, and composition. No TokenZero process embeds both catalogs.

use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use crate::{contract_digest_hex, SEMANTIC_CONTRACT_VERSION};

/// Package artifact names (release binaries / packages).
pub const ARTIFACT_MCP: &str = "tokenzero-mcp";
pub const ARTIFACT_RAW_WORKER: &str = "tokenzero-codemode";
/// Compatibility shim name — never exposes both surfaces itself.
pub const ARTIFACT_SHIM: &str = "tokenzero";

/// Install-state filename under the install prefix / config dir.
pub const INSTALL_STATE_FILE: &str = "install-state.json";

/// Client registration file written by the installer (single surface).
pub const CLIENT_CONFIG_FILE: &str = "client-config.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageSurface {
    Mcp,
    #[serde(rename = "raw-worker", alias = "codemode")]
    RawWorker,
}

impl PackageSurface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
            Self::RawWorker => "raw-worker",
        }
    }

    pub fn artifact_name(self) -> &'static str {
        match self {
            Self::Mcp => ARTIFACT_MCP,
            Self::RawWorker => ARTIFACT_RAW_WORKER,
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mcp" | "fastmcp" | "per-op" | "per_op" | ARTIFACT_MCP => Ok(Self::Mcp),
            "raw-worker" | "raw_worker" | "worker" | "codemode" | "code-mode" | "code_mode"
            | ARTIFACT_RAW_WORKER => Ok(Self::RawWorker),
            other => Err(format!(
                "unknown package surface {other:?}; require 'mcp' or 'raw-worker' (artifacts {ARTIFACT_MCP} / {ARTIFACT_RAW_WORKER})"
            )),
        }
    }

    /// Aggregate hosts use the raw worker; classic MCP-only clients use the
    /// compatibility server.
    pub fn recommended_for_client(aggregate_host: bool) -> Self {
        if aggregate_host {
            Self::RawWorker
        } else {
            Self::Mcp
        }
    }
}

impl std::fmt::Display for PackageSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Compile-time compatibility surfaces enabled in this binary.
///
/// The raw worker is a separate bin-only package and never appears here.
pub fn compile_time_surfaces() -> Vec<PackageSurface> {
    // `mut` is used when a surface feature pushes; empty without markers.
    #[allow(unused_mut)]
    let mut out = Vec::new();
    #[cfg(feature = "surface-mcp")]
    out.push(PackageSurface::Mcp);
    out
}

/// Fail closed if a future build accidentally compiles multiple catalog surfaces.
pub fn reject_dual_compiled_surfaces() -> Result<(), String> {
    let surfaces = compile_time_surfaces();
    if surfaces.len() > 1 {
        return Err(dual_surface_diagnostic(
            "binary compiled multiple catalog surfaces; one process must never contain both catalogs",
        ));
    }
    Ok(())
}

/// Immutable package surface for single-surface release binaries.
pub fn baked_package_surface() -> Option<PackageSurface> {
    let surfaces = compile_time_surfaces();
    if surfaces.len() == 1 {
        return Some(surfaces[0]);
    }
    if let Ok(name) = env::current_exe() {
        if let Some(stem) = name.file_name().and_then(|s| s.to_str()) {
            if stem == ARTIFACT_MCP || stem.starts_with("tokenzero-mcp") {
                return Some(PackageSurface::Mcp);
            }
            if stem == ARTIFACT_RAW_WORKER || stem.starts_with("tokenzero-codemode") {
                return Some(PackageSurface::RawWorker);
            }
        }
    }
    if let Ok(v) = env::var("TOKENZERO_PACKAGE_SURFACE") {
        if let Ok(s) = PackageSurface::parse(&v) {
            return Some(s);
        }
    }
    None
}

/// Combined semantic contract digest advertised by help/doctor/SBOM/uninstall.
pub fn semantic_contract_digest() -> String {
    contract_digest_hex()
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
        "selection_matrix": {
            "aggregate_host": ARTIFACT_RAW_WORKER,
            "classic_mcp_client": ARTIFACT_MCP,
            "rule": "ZeroStack aggregate host -> planner-free raw worker; classic MCP client -> tokenzero-mcp"
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
        Self::for_surface_on_platform(surface, prefix, binary_path, current_platform())
    }

    fn for_surface_on_platform(
        surface: PackageSurface,
        prefix: &Path,
        binary_path: &Path,
        platform: &str,
    ) -> Self {
        Self {
            surface,
            artifact: surface.artifact_name().to_string(),
            binary_path: binary_path.display().to_string(),
            prefix: prefix.display().to_string(),
            semantic_contract_digest: semantic_contract_digest(),
            package_version: env!("CARGO_PKG_VERSION").to_string(),
            installed_at_unix: now_unix(),
            platform: platform.to_string(),
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

pub fn parse_install_platform(value: &str) -> Result<&'static str, String> {
    match value {
        "macos" => Ok("macos"),
        "linux" => Ok("linux"),
        "windows" => Ok("windows"),
        _ => Err(format!(
            "TOKENZERO_INSTALL_PLATFORM must be one of: macos, linux, windows (got {value:?})"
        )),
    }
}

fn selected_install_platform() -> Result<&'static str, String> {
    match env::var("TOKENZERO_INSTALL_PLATFORM") {
        Ok(value) => parse_install_platform(&value),
        Err(env::VarError::NotPresent) => Ok(current_platform()),
        Err(env::VarError::NotUnicode(_)) => {
            Err("TOKENZERO_INSTALL_PLATFORM must be valid UTF-8".to_string())
        }
    }
}

/// Default install prefix: `$TOKENZERO_INSTALL_PREFIX` or `~/.tokenzero-install`.
pub fn default_install_prefix() -> PathBuf {
    if let Ok(p) = env::var("TOKENZERO_INSTALL_PREFIX") {
        return PathBuf::from(p);
    }
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".tokenzero-install");
    }
    PathBuf::from(".tokenzero-install")
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
        PackageSurface::RawWorker => (
            binary_path.display().to_string(),
            vec!["raw-worker".to_string()],
        ),
    };
    ClientConfig {
        name: format!("TokenZero ({})", surface.as_str()),
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
    zero_store::atomic_write_file(path, bytes)
}

/// Install one surface. Replaces prior surface cleanly (binary selection + client config).
///
/// Never starts a stdio server. Safe to call from packaging subcommands and install.sh.
pub fn install_surface(
    surface: PackageSurface,
    prefix: &Path,
    binary_path: &Path,
) -> Result<InstallState, String> {
    reject_dual_env_selection()?;
    let platform = selected_install_platform()?;

    let prev = load_install_state(prefix)?;
    if let Some(prev) = &prev {
        if prev.surface != surface {
            uninstall_surface(prefix)?;
        }
    }

    let config = client_config_for(surface, binary_path);
    write_client_config(prefix, &config)?;
    let state = InstallState::for_surface_on_platform(surface, prefix, binary_path, platform);
    write_install_state(prefix, &state)?;

    let shim_path = prefix.join("shim-target");
    fs::write(&shim_path, surface.as_str()).map_err(|e| format!("write shim-target: {e}"))?;

    Ok(state)
}

/// Remove install state, client config, and shim marker for this prefix.
pub fn uninstall_surface(prefix: &Path) -> Result<Option<InstallState>, String> {
    let prev = load_install_state(prefix)?;
    let errors: Vec<String> = [
        install_state_path(prefix),
        prefix.join(CLIENT_CONFIG_FILE),
        prefix.join("shim-target"),
    ]
    .iter()
    .filter_map(|path| remove_install_file(path).err())
    .collect();
    if errors.is_empty() {
        Ok(prev)
    } else {
        Err(errors.join("; "))
    }
}

fn remove_install_file(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("remove {}: {err}", path.display())),
    }
}

/// Detect dual surface selection attempts (env) and fail closed.
pub fn reject_dual_env_selection() -> Result<(), String> {
    if let Ok(v) = env::var("TOKENZERO_SURFACE") {
        let lower = v.to_ascii_lowercase();
        if lower.contains(',')
            || lower.contains('+')
            || lower == "both"
            || lower == "all"
            || (lower.contains("mcp") && lower.contains("codemode"))
        {
            return Err(dual_surface_diagnostic(
                "TOKENZERO_SURFACE requests more than one package surface",
            ));
        }
    }
    if env::var_os("TOKENZERO_ENABLE_MCP").is_some()
        && env::var_os("TOKENZERO_ENABLE_CODEMODE").is_some()
    {
        return Err(dual_surface_diagnostic(
            "both TOKENZERO_ENABLE_MCP and TOKENZERO_ENABLE_CODEMODE are set",
        ));
    }
    Ok(())
}

pub fn dual_surface_diagnostic(detail: &str) -> String {
    format!(
        "tokenzero: dual package surface rejected (fail closed): {detail}. \
Keep {ARTIFACT_MCP} in classic compatibility mode; ZeroStack launches \
{ARTIFACT_RAW_WORKER} only as a planner-free raw worker. Do not register both \
catalogs in one client process."
    )
}

/// Parse CLI args for dual `--mode=` selection.
pub fn modes_from_args(args: &[String]) -> Result<Option<PackageSurface>, String> {
    let mut mcp = false;
    let mut codemode = false;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if let Some(rest) = a.strip_prefix("--mode=") {
            match rest {
                "mcp" => mcp = true,
                "codemode" => codemode = true,
                "both" | "all" | "mcp+codemode" | "codemode+mcp" => {
                    return Err(dual_surface_diagnostic(&format!("invalid --mode={rest}")));
                }
                _ => {}
            }
            i += 1;
            continue;
        }
        if a == "--mode" {
            if let Some(rest) = args.get(i + 1) {
                match rest.as_str() {
                    "mcp" => mcp = true,
                    "codemode" => codemode = true,
                    "both" | "all" | "mcp+codemode" | "codemode+mcp" => {
                        return Err(dual_surface_diagnostic(&format!("invalid --mode {rest}")));
                    }
                    _ => {}
                }
                i += 2;
                continue;
            }
        }
        if let Some(rest) = a.strip_prefix("--tool-surface=") {
            match rest {
                "mcp" => mcp = true,
                "codemode" => codemode = true,
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
        return Err(
            "tokenzero: engine-local CodeMode mode was removed; launch plans through the ZeroStack aggregate host and keep tokenzero-mcp in classic mode"
                .to_string(),
        );
    }
    Ok(None)
}

/// Argument shapes a stdio surface binary legitimately accepts before serving.
///
/// Anything else is a caller mistake (usually a `tokenzero` CLI verb aimed at the
/// wrong binary) and must fail loudly.
const STDIO_SURFACE_ALLOWED_FLAG_PREFIXES: &[&str] = &[
    "--mode",
    "--tool-surface",
    "--root",
    "--repo",
    "--log-level",
    "--surface",
];

/// Reject argv a stdio surface binary cannot serve (tokenzero-j0cn).
///
/// `tokenzero-codemode expand --raw <ref>` used to fall through every packaging
/// branch into the stdio server, which then read EOF from a non-tty stdin and exited
/// 0 with no output. Valid and invalid refs were indistinguishable, so the silent
/// empty success masked whatever the caller actually got wrong. Surface binaries only
/// speak classic MCP or raw-worker v2 over stdio; CLI verbs belong to `tokenzero`.
pub fn reject_non_stdio_args(artifact: &str, args: &[String]) -> Result<(), String> {
    // args[0] is the executable path.
    for arg in args.iter().skip(1) {
        if arg.starts_with('-') {
            let name = arg.split('=').next().unwrap_or(arg);
            if STDIO_SURFACE_ALLOWED_FLAG_PREFIXES.contains(&name) {
                continue;
            }
            return Err(format!(
                "{artifact}: unsupported option {arg:?}. This artifact only serves the stdio surface; it has no CLI subcommands. Run `tokenzero {arg}` for CLI operations (expand, ingest, run, capabilities), or `{artifact} help` for the surface contract."
            ));
        }
        // A bare positional at this point is an unrecognized subcommand: every
        // supported one (help/sbom/doctor/install/uninstall/raw-worker) already
        // returned before this check.
        return Err(format!(
            "{artifact}: unknown subcommand {arg:?}. This artifact only serves the stdio surface; it has no CLI subcommands. Run `tokenzero {arg}` for CLI operations (expand, ingest, run, capabilities), or `{artifact} help` for the surface contract."
        ));
    }
    Ok(())
}

/// Resolve the single surface this process is allowed to start.
///
/// Dual compiled surfaces, dual argv, and dual env always fail closed.
/// When no surface feature is baked, resolve from install state / shim-target
/// (selected symlink compatibility) or explicit `--mode` / env.
pub fn resolve_startup_surface(args: &[String]) -> Result<PackageSurface, String> {
    reject_dual_compiled_surfaces()?;
    reject_dual_env_selection()?;
    let from_args = modes_from_args(args)?;

    if let Some(baked) = baked_package_surface() {
        if let Some(requested) = from_args {
            if requested != baked {
                return Err(format!(
                    "tokenzero: artifact {} is locked to surface '{}'; refused request for '{}'. \
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
        // Surface must be compiled into this process when starting a catalog server.
        assert_surface_compiled(s)?;
        return Ok(s);
    }

    if let Ok(v) = env::var("TOKENZERO_SURFACE") {
        let s = PackageSurface::parse(&v)?;
        assert_surface_compiled(s)?;
        return Ok(s);
    }

    let prefix = default_install_prefix();
    if let Some(state) = load_install_state(&prefix)? {
        assert_surface_compiled(state.surface)?;
        return Ok(state.surface);
    }

    let shim = prefix.join("shim-target");
    if let Ok(raw) = fs::read_to_string(&shim) {
        let s = PackageSurface::parse(raw.trim())?;
        assert_surface_compiled(s)?;
        return Ok(s);
    }

    // Single-surface default feature (surface-mcp) when nothing else selects.
    let surfaces = compile_time_surfaces();
    if surfaces.len() == 1 {
        return Ok(surfaces[0]);
    }
    Err(dual_surface_diagnostic(
        "no package surface selected and no single surface is baked into this binary; \
install tokenzero-mcp for classic MCP; the ZeroStack aggregate host launches tokenzero-codemode as a raw worker",
    ))
}

/// Whether this build can start the given surface (feature matrix on consumer crate).
pub fn surface_compiled_in(surface: PackageSurface) -> bool {
    compile_time_surfaces().contains(&surface)
}

pub fn assert_surface_compiled(surface: PackageSurface) -> Result<(), String> {
    if surface_compiled_in(surface) {
        Ok(())
    } else {
        Err(format!(
            "tokenzero: package surface '{}' was not compiled into this binary (artifact features). \
Rebuild with --features surface-{} or install {}.",
            surface.as_str(),
            surface.as_str(),
            surface.artifact_name()
        ))
    }
}

/// Fail closed for packaged single-surface artifacts that accidentally include both features.
///
/// Dual compilation always fails closed (process mutual exclusion) — there is no
/// dual-catalog "dev default" path (tokenzero-irx9.3).
pub fn assert_packaged_surface_features(locked: PackageSurface) -> Result<(), String> {
    reject_dual_compiled_surfaces()?;
    let surfaces = compile_time_surfaces();
    if surfaces.is_empty() {
        return Err(format!(
            "tokenzero: no package surface compiled; rebuild with --features surface-{}",
            locked.as_str()
        ));
    }
    if !surfaces.contains(&locked) {
        return Err(format!(
            "tokenzero: package surface '{}' was not compiled into this binary. Rebuild with --features surface-{} or install {}.",
            locked.as_str(),
            locked.as_str(),
            locked.artifact_name()
        ));
    }
    if surfaces.len() == 1 && surfaces[0] != locked {
        return Err(format!(
            "tokenzero: binary locked to '{}' but compiled for '{}'",
            locked.as_str(),
            surfaces[0].as_str()
        ));
    }
    Ok(())
}

/// Software bill of materials identity for the selected surface.
pub fn sbom_document(surface: PackageSurface) -> serde_json::Value {
    let mut identity = package_identity(surface);
    if let serde_json::Value::Object(ref mut map) = identity {
        map.insert(
            "sbom".into(),
            serde_json::json!({
                "format": "tokenzero-sbom-v1",
                "components": [
                    {
                        "type": "application",
                        "name": surface.artifact_name(),
                        "version": env!("CARGO_PKG_VERSION"),
                        "surface": surface.as_str(),
                    },
                    {
                        "type": "library",
                        "name": "tokenzero-core",
                        "role": "shared-core",
                        "semantic_contract_digest": semantic_contract_digest(),
                        "semantic_contract_version": SEMANTIC_CONTRACT_VERSION,
                    }
                ],
                "platform": current_platform(),
                "mutually_exclusive_with": match surface {
                    PackageSurface::Mcp => ARTIFACT_RAW_WORKER,
                    PackageSurface::RawWorker => ARTIFACT_MCP,
                }
            }),
        );
    }
    identity
}

/// Uninstall report identity for CLI.
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
