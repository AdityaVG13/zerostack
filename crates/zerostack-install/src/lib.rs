#![forbid(unsafe_code)]
#![recursion_limit = "2048"]

mod package_audit;

use serde::{Deserialize, Serialize};

/// MCP launch surface an agent config will use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpToolSurface {
    #[default]
    Classic,
    CodeMode,
}

impl McpToolSurface {
    pub const ENV: &'static str = "ZERO_INSTALL_MCP_TOOL_SURFACE";

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Classic => "mcp",
            Self::CodeMode => "codemode",
        }
    }
}

impl std::str::FromStr for McpToolSurface {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().replace(['_', ' '], "-").as_str() {
            "" | "mcp" | "classic" | "aliases" | "full" => Ok(Self::Classic),
            "codemode" | "code-mode" => Ok(Self::CodeMode),
            other => Err(format!(
                "unsupported MCP launch mode '{other}'; use mcp or codemode"
            )),
        }
    }
}

/// Install-plan schema version for this engine.
pub const INSTALL_SCHEMA_VERSION: &str = "zerostack.install_plan.v1";

/// Semantic contract version advertised by doctor/SBOM/uninstall.
pub const SEMANTIC_CONTRACT_VERSION: &str = "1.0.0";

/// Deterministic stand-in digest over the engine's declared semantic inputs.
/// TokenZero's op-abi registry digest is domain-side and stays there.
pub fn contract_digest_hex() -> String {
    use sha2::{Digest, Sha256};
    let identity = format!(
        "zerostack-install|{}|{}",
        SEMANTIC_CONTRACT_VERSION,
        env!("CARGO_PKG_VERSION")
    );
    let digest = Sha256::digest(identity.as_bytes());
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push(char::from_digit(u32::from(b >> 4), 16).unwrap());
        out.push(char::from_digit(u32::from(b & 0x0f), 16).unwrap());
    }
    out
}
pub mod packaging;
pub use package_audit::package_audit;
pub use package_audit::zip::{
    zip_crc32, zip_eocd_candidates, zip_local_header, zip_payload_error_detail, zip_u16_at,
    zip_u32_at,
};
pub use package_audit::{
    ArchivePayloadError, MAX_TOP_LEVEL_ARCHIVE_BYTES, MAX_ZIP_TOTAL_UNCOMPRESSED_BYTES,
    ZIP_DATA_DESCRIPTOR_SIGNATURE, ZIP_FLAG_DATA_DESCRIPTOR, ZIP_FLAG_ENCRYPTED,
    ZIP64_EOCD_LOCATOR_SIGNATURE, ZIP64_EOCD_RECORD_SIGNATURE, ZIP64_EXTENDED_INFORMATION_EXTRA,
    deflate_decompress_bytes,
};
pub use packaging::{
    ARTIFACT_MCP, ARTIFACT_RAW_WORKER, ARTIFACT_SHIM, CLIENT_CONFIG_FILE, ClientConfig,
    InstallState, PackageSurface, assert_packaged_surface_features, assert_surface_compiled,
    baked_package_surface, client_config_for, compile_time_surfaces, current_platform,
    default_install_prefix, dual_surface_diagnostic, install_surface, load_install_state,
    modes_from_args, package_identity, parse_install_platform, reject_dual_compiled_surfaces,
    reject_dual_env_selection, resolve_startup_surface, sbom_document, semantic_contract_digest,
    surface_compiled_in, uninstall_report, uninstall_surface, write_client_config,
};

use fs4::{FileExt, TryLockError};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Error, ErrorKind, Write};
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(windows)]
const WINDOWS_USER_PATH_REGISTRY: &str = "HKCU\\Environment\\Path";

macro_rules! install_records {
    ($($vis:vis struct $name:ident { $($(#[$field_meta:meta])* $field_vis:vis $field:ident: $ty:ty),* $(,)? })*) => {$ (
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        $vis struct $name {
            $($(#[$field_meta])* $field_vis $field: $ty),*
        }
    )*};
}

install_records! {
    pub struct InstallWrite { pub path: String, pub action: String, pub backup_id: String, pub capability: String, pub global: bool }
    pub struct ClientSurfaceCheck { pub name: String, pub ok: bool, pub detail: String }
    pub struct ClientSurfaceStatus { pub path: String, pub action: String, pub capability: String, pub global: bool, pub exists: bool, pub installed: bool, pub state: String, pub checks: Vec<ClientSurfaceCheck> }
    pub struct RollbackInfo { pub id: String, pub available: bool, pub manifest_path: String }
    pub struct InstallPlan { pub schema_version: String, pub status: String, pub dry_run: bool, pub detected_surfaces: Vec<String>, pub mcp_surface: McpToolSurface, pub writes: Vec<InstallWrite>, pub rollback: RollbackInfo, pub global_writes_allowed: bool }
    pub struct AppliedInstall { pub schema_version: String, pub status: String, pub dry_run: bool, pub written: Vec<String>, pub rollback: RollbackInfo, pub verification: Vec<VerificationRow> }
    pub struct VerificationRow { pub path: String, pub observed_sha256: String, pub byte_count: usize, pub verified: bool }
    struct RollbackManifest { schema_version: String, id: String, created_unix: u64, entries: Vec<RollbackEntry> }
    struct RollbackEntry {
        path: String,
        existed: bool,
        previous_content: Option<String>,
        #[serde(default)]
        previous_bytes: Option<Vec<u8>>,
        previous_sha256: Option<String>,
        /// SHA-256 of the bytes written by this install. Rollback refuses when the
        /// live path no longer matches, so post-install user edits are not wiped.
        #[serde(default)]
        installed_sha256: Option<String>,
    }
}

type PendingContent = Vec<u8>;

pub fn plan(root: &Path, global: bool, capabilities: &[String]) -> InstallPlan {
    plan_for_agents(root, global, capabilities, &[], McpToolSurface::Classic)
}

pub fn plan_for_agents(
    root: &Path,
    global: bool,
    capabilities: &[String],
    agents: &[String],
    mcp_surface: McpToolSurface,
) -> InstallPlan {
    let rollback_id = rollback_id();
    let selected = if capabilities.is_empty() {
        ["mcp", "instructions", "shell"]
            .map(str::to_string)
            .to_vec()
    } else {
        capabilities.to_vec()
    };
    let mut writes = Vec::new();
    for capability in &selected {
        append_capability_writes(&mut writes, root, global, &rollback_id, capability, agents);
    }
    let manifest_path = root
        .join(".tokenzero/install")
        .join(format!("{rollback_id}.json"))
        .display()
        .to_string();
    InstallPlan {
        schema_version: INSTALL_SCHEMA_VERSION.to_string(),
        status: "planned".to_string(),
        dry_run: true,
        detected_surfaces: selected,
        mcp_surface,
        writes,
        rollback: RollbackInfo {
            id: rollback_id,
            available: true,
            manifest_path,
        },
        global_writes_allowed: global,
    }
}

fn append_capability_writes(
    writes: &mut Vec<InstallWrite>,
    root: &Path,
    global: bool,
    rollback_id: &str,
    capability: &str,
    agents: &[String],
) {
    let simple = match capability {
        "instructions" => Some((root.join("AGENTS.md"), "patch")),
        "shell" => Some((shell_launcher_path(root, global), "write")),
        "runtime" => Some((root.join(".tokenzero/install/runtime.json"), "write")),
        "mcp" if !global => Some((root.join(".tokenzero/mcp-server.json"), "merge")),
        "mcp" => {
            append_cli_writes(writes, root, global, rollback_id);
            for path in global_json_mcp_paths_for_agents(root, agents)
                .into_iter()
                .chain(global_toml_mcp_paths_for_agents(root, agents))
            {
                push_write(writes, path, "merge", "mcp", rollback_id, global);
            }
            None
        }
        "cli" => {
            append_cli_writes(writes, root, global, rollback_id);
            None
        }
        "hooks" if agents.is_empty() || agents.iter().any(|agent| agent == "claude") => {
            Some((root.join(".claude/settings.json"), "merge"))
        }
        "shim" => {
            let shim_dir = shims_dir(root);
            for tool in SHIM_TOOLS {
                if resolve_real_tool(tool, &shim_dir).is_some() {
                    push_write(
                        writes,
                        shim_dir.join(tool),
                        "write",
                        "shim",
                        rollback_id,
                        global,
                    );
                }
            }
            None
        }
        _ => None,
    };
    if let Some((path, action)) = simple {
        push_write(writes, path, action, capability, rollback_id, global);
    }
}

fn append_cli_writes(writes: &mut Vec<InstallWrite>, root: &Path, global: bool, rollback_id: &str) {
    if global {
        push_write(
            writes,
            installed_runtime_binary_path(root),
            "copy",
            "cli-runtime",
            rollback_id,
            true,
        );
    }
    for (path, capability) in [
        (tokenzero_launcher_path(root, global), "cli"),
        (root.join(".tokenzero/install/runtime.json"), "runtime"),
    ] {
        push_write(writes, path, "write", capability, rollback_id, global);
    }
    #[cfg(windows)]
    if global {
        push_write(
            writes,
            root.join(".tokenzero/bin/tokenzero"),
            "write",
            "cli-shim",
            rollback_id,
            true,
        );
        if is_real_windows_user_root(root) {
            push_write(
                writes,
                PathBuf::from(WINDOWS_USER_PATH_REGISTRY),
                "prepend",
                "path",
                rollback_id,
                true,
            );
        }
    }
}

#[cfg(windows)]
const CLAUDE_DESKTOP_REL: &str = "AppData/Roaming/Claude/claude_desktop_config.json";
#[cfg(not(windows))]
const CLAUDE_DESKTOP_REL: &str = "Library/Application Support/Claude/claude_desktop_config.json";
#[cfg(windows)]
const CURSOR_DESKTOP_REL: &str = "AppData/Roaming/Cursor/User/mcp.json";
#[cfg(not(windows))]
const CURSOR_DESKTOP_REL: &str = "Library/Application Support/Cursor/User/mcp.json";

const AGENT_JSON_MCP_RELS: &[(&str, &[&str])] = &[
    (
        "claude",
        &[".claude.json", ".claude/mcp.json", CLAUDE_DESKTOP_REL],
    ),
    ("cursor", &[".cursor/mcp.json", CURSOR_DESKTOP_REL]),
    ("factory", &[".factory/mcp.json"]),
    ("gemini", &[".gemini/settings.json"]),
    ("opencode", &[".config/opencode/mcp.json"]),
];
const DEFAULT_JSON_MCP_RELS: &[&str] = &[
    ".tokenzero/mcp.json",
    ".tokenzero/mcp-server.json",
    ".config/tokenzero/mcp-server.json",
    ".mcp.json",
    ".claude.json",
    ".claude/mcp.json",
    ".cursor/mcp.json",
    ".gemini/settings.json",
    ".config/opencode/mcp.json",
    ".factory/mcp.json",
];
const DEFAULT_AGENT_MCP_NAMES: &[&str] = &[
    "claude", "codex", "cursor", "droid", "factory", "gemini", "grok", "opencode",
];
const TOML_MCP_AGENTS: &[(&str, &str)] = &[
    ("codex", ".codex/config.toml"),
    ("grok", ".grok/config.toml"),
];

fn agent_json_mcp_relpaths(agent: &str) -> &'static [&'static str] {
    AGENT_JSON_MCP_RELS
        .iter()
        .find(|(name, _)| *name == agent)
        .map_or(&[], |(_, paths)| paths)
}

fn global_json_mcp_paths_for_agents(root: &Path, agents: &[String]) -> Vec<PathBuf> {
    if agents.is_empty() {
        return DEFAULT_JSON_MCP_RELS
            .iter()
            .copied()
            .chain([CLAUDE_DESKTOP_REL, CURSOR_DESKTOP_REL])
            .map(|rel| root.join(rel))
            .chain(
                DEFAULT_AGENT_MCP_NAMES
                    .iter()
                    .map(|agent| root.join(format!(".config/tokenzero/agents/{agent}.mcp.json"))),
            )
            .collect();
    }
    agents
        .iter()
        .flat_map(|agent| {
            std::iter::once(root.join(format!(".config/tokenzero/agents/{agent}.mcp.json"))).chain(
                agent_json_mcp_relpaths(agent)
                    .iter()
                    .map(|rel| root.join(rel)),
            )
        })
        .collect()
}

fn global_toml_mcp_paths_for_agents(root: &Path, agents: &[String]) -> Vec<PathBuf> {
    TOML_MCP_AGENTS
        .iter()
        .filter(|(agent, _)| {
            agents.is_empty() || agents.iter().any(|selected| selected.as_str() == *agent)
        })
        .map(|(_, rel)| root.join(rel))
        .collect()
}

/// Read-heavy tools agents reach for via bare shell; each gets a PATH shim.
const SHIM_TOOLS: &[&str] = &[
    "cat", "head", "tail", "grep", "rg", "find", "ls", "tree", "wc",
];

fn shims_dir(root: &Path) -> PathBuf {
    root.join(".tokenzero").join("shims")
}

fn resolve_real_tool(tool: &str, shim_dir: &Path) -> Option<PathBuf> {
    resolve_real_tool_in(tool, shim_dir, std::env::var_os("PATH")?.as_os_str())
}

/// First executable named `tool` on `path_var`, skipping the shim directory
/// itself and any other generated tokenzero shim, so the baked-in REAL path
/// can never point back into the wrapper layer.
fn resolve_real_tool_in(
    tool: &str,
    shim_dir: &Path,
    path_var: &std::ffi::OsStr,
) -> Option<PathBuf> {
    for dir in std::env::split_paths(path_var) {
        if dir.as_os_str().is_empty() || same_path(&dir, shim_dir) || is_shim_layer_dir(&dir) {
            continue;
        }
        let candidate = dir.join(tool);
        if is_executable_file(&candidate) && !is_generated_shim(&candidate) {
            return Some(candidate);
        }
        if cfg!(windows) {
            let exe = dir.join(format!("{tool}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.canonicalize().unwrap_or_else(|_| a.to_path_buf())
        == b.canonicalize().unwrap_or_else(|_| b.to_path_buf())
}

fn is_shim_layer_dir(dir: &Path) -> bool {
    dir.ends_with(".tokenzero/shims")
}

fn is_generated_shim(path: &Path) -> bool {
    let mut prefix = [0u8; 64];
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let read = std::io::Read::read(&mut file, &mut prefix).unwrap_or(0);
    String::from_utf8_lossy(&prefix[..read]).contains("tokenzero shim")
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

fn push_write(
    writes: &mut Vec<InstallWrite>,
    path: PathBuf,
    action: &str,
    capability: &str,
    rollback_id: &str,
    global: bool,
) {
    let path_string = path.display().to_string();
    if writes.iter().any(|existing| existing.path == path_string) {
        return;
    }
    writes.push(InstallWrite {
        path: path_string,
        action: action.to_string(),
        backup_id: rollback_id.to_string(),
        capability: capability.to_string(),
        global,
    });
}

fn prepare_write(
    row: &InstallWrite,
    root: &Path,
    mcp_surface: McpToolSurface,
) -> std::io::Result<(PendingContent, RollbackEntry)> {
    let path = PathBuf::from(&row.path);
    let path_write = is_windows_user_path_write(row);
    let binary_write = row.action == "copy" && row.capability == "cli-runtime";
    let previous = if path_write {
        windows_user_path()?
    } else if binary_write {
        None
    } else {
        match fs::read(&path) {
            Ok(bytes) => Some(String::from_utf8(bytes).map_err(|err| {
                Error::new(
                    ErrorKind::InvalidData,
                    format!(
                        "refusing to replace non-UTF-8 text file {}: {err}",
                        path.display()
                    ),
                )
            })?),
            Err(err) if err.kind() == ErrorKind::NotFound => None,
            Err(err) => return Err(err),
        }
    };
    let content = if path_write {
        windows_path_with_tokenzero_bin(root, previous.as_deref()).into_bytes()
    } else {
        content_for(row, root, previous.as_deref(), mcp_surface)?
    };
    let existing_bytes = if !path_write && binary_write {
        fs::read(&path).ok()
    } else {
        None
    };
    let previous_bytes = existing_bytes
        .as_ref()
        .filter(|bytes| bytes.as_slice() != content.as_slice())
        .cloned();
    let previous_sha256 = previous_bytes
        .as_deref()
        .map(sha256_bytes)
        .or_else(|| existing_bytes.as_deref().map(sha256_bytes))
        .or_else(|| previous.as_deref().map(sha256));
    let installed_sha256 = Some(sha256_bytes(&content));
    Ok((
        content,
        RollbackEntry {
            path: row.path.clone(),
            existed: if path_write {
                previous.is_some()
            } else {
                path.exists()
            },
            previous_sha256,
            previous_content: previous,
            previous_bytes,
            installed_sha256,
        },
    ))
}

pub fn apply(
    root: &Path,
    global: bool,
    capabilities: &[String],
) -> std::io::Result<AppliedInstall> {
    apply_for_agents(root, global, capabilities, &[], McpToolSurface::Classic)
}

pub fn apply_for_agents(
    root: &Path,
    global: bool,
    capabilities: &[String],
    agents: &[String],
    mcp_surface: McpToolSurface,
) -> std::io::Result<AppliedInstall> {
    let mut plan = plan_for_agents(root, global, capabilities, agents, mcp_surface);
    plan.dry_run = false;
    let mut manifest = RollbackManifest {
        schema_version: "tokenzero.rollback.v1".to_string(),
        id: plan.rollback.id.clone(),
        created_unix: now_unix(),
        entries: Vec::new(),
    };
    // Phase 1: snapshot each target's existing content and compute its new
    // content BEFORE mutating anything, recording the rollback manifest from that
    // snapshot. The manifest is then persisted FIRST (below), so a crash partway
    // through Phase 2 still leaves a complete, usable rollback record.
    let mut pending: Vec<(PathBuf, PendingContent, InstallWrite)> = Vec::new();
    for row in &plan.writes {
        if row.global && !global {
            continue;
        }
        let path = PathBuf::from(&row.path);
        let (content, entry) = prepare_write(row, root, plan.mcp_surface)?;
        manifest.entries.push(entry);
        pending.push((path, content, row.clone()));
    }

    let manifest_path = PathBuf::from(&plan.rollback.manifest_path);
    atomic_write(
        &manifest_path,
        (serde_json::to_string_pretty(&manifest)? + "\n").as_bytes(),
    )?;

    // Phase 2: apply each write atomically (temp in the same dir -> fsync ->
    // rename). A user's existing editor/agent config is never observed truncated
    // or partial: readers see either the prior content or the fully written new
    // content, even if the process is killed or the disk fills mid-write.
    let mut written = Vec::new();
    let mut verification = Vec::new();
    for (path, content, row) in &pending {
        let observed = if is_windows_user_path_write(row) {
            let text = std::str::from_utf8(content).map_err(|_| {
                Error::new(ErrorKind::InvalidData, "Windows Path write must be text")
            })?;
            write_windows_user_path(text)?;
            windows_user_path()?.unwrap_or_default().into_bytes()
        } else {
            if row.action != "copy" || fs::read(path).ok().as_deref() != Some(content) {
                atomic_write(path, content)?;
            }
            make_executable_if_needed(row, path)?;
            fs::read(path)?
        };
        verification.push(verify_install_write(&row.path, content, observed)?);
        written.push(row.path.clone());
    }
    written.push(manifest_path.display().to_string());
    Ok(AppliedInstall {
        schema_version: INSTALL_SCHEMA_VERSION.to_string(),
        status: "ok".to_string(),
        dry_run: false,
        written,
        rollback: plan.rollback,
        verification,
    })
}

pub fn rollback(root: &Path, rollback_id: &str) -> std::io::Result<serde_json::Value> {
    let manifest_path = if rollback_id == "latest" {
        latest_manifest(root).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no rollback manifest")
        })?
    } else {
        root.join(".tokenzero/install")
            .join(format!("{rollback_id}.json"))
    };
    let manifest: RollbackManifest = serde_json::from_str(&fs::read_to_string(&manifest_path)?)?;
    // Optimistic-concurrency precondition: refuse before any mutation when a
    // path drifted after install. Otherwise restoring the pre-install snapshot
    // silently erases later user configuration.
    for entry in &manifest.entries {
        refuse_rollback_on_post_install_drift(entry)?;
    }
    let mut restored = Vec::new();
    let mut removed = Vec::new();
    for entry in manifest.entries {
        if is_windows_user_path_entry(&entry.path) {
            if entry.existed {
                write_windows_user_path(entry.previous_content.as_deref().unwrap_or_default())?;
                restored.push(entry.path);
            } else {
                delete_windows_user_path()?;
                removed.push(entry.path);
            }
            continue;
        }
        let path = PathBuf::from(&entry.path);
        if entry.existed {
            if let Some(bytes) = entry.previous_bytes {
                atomic_write(&path, &bytes)?;
                set_executable(&path)?;
                restored.push(entry.path);
            } else if let Some(content) = entry.previous_content {
                atomic_write(&path, content.as_bytes())?;
                restored.push(entry.path);
            }
        } else if path.exists() {
            fs::remove_file(&path)?;
            removed.push(entry.path);
        }
    }
    Ok(serde_json::json!({
        "schema_version": "tokenzero.rollback.v1",
        "status": "ok",
        "rollback_id": manifest.id,
        "restored": restored,
        "removed": removed,
        "manifest_path": manifest_path.display().to_string(),
    }))
}

fn refuse_rollback_on_post_install_drift(entry: &RollbackEntry) -> std::io::Result<()> {
    let Some(expected) = entry.installed_sha256.as_deref() else {
        return Ok(());
    };
    let current = current_rollback_target_sha256(entry)?;
    if current.as_deref() == Some(expected) {
        return Ok(());
    }
    Err(Error::new(
        ErrorKind::InvalidData,
        format!(
            "rollback conflict: {} changed after install; refusing to overwrite later edits",
            entry.path
        ),
    ))
}

fn current_rollback_target_sha256(entry: &RollbackEntry) -> std::io::Result<Option<String>> {
    if is_windows_user_path_entry(&entry.path) {
        return Ok(windows_user_path()?.map(|text| sha256(&text)));
    }
    let path = PathBuf::from(&entry.path);
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(sha256_bytes(&fs::read(&path)?)))
}

macro_rules! install_helpers {
    ($(fn $name:ident($($arg:ident: $ty:ty),*) -> $return:ty = $value:expr;)*) => {$ (
        fn $name($($arg: $ty),*) -> $return {
            $value
        }
    )*};
}

install_helpers! {
    fn tokenzero_command(root: &Path, global: bool) -> String = tokenzero_launcher_path(root, global).display().to_string();
    fn mcp_command(root: &Path, global: bool) -> String = if global { runtime_manifest_binary(root, true) } else { tokenzero_command(root, false) };
    fn current_exe_string() -> String = std::env::current_exe().ok().map(|path| path.display().to_string()).unwrap_or_else(|| "tokenzero".to_string());
    fn installed_runtime_binary_name() -> String = {
        let hash = current_exe_hash_prefix().unwrap_or_else(|| "current".to_string());
        format!("tokenzero-runtime-{hash}{}", if cfg!(windows) { ".exe" } else { "" })
    };
    fn runtime_manifest_binary(root: &Path, global: bool) -> String = if global { installed_runtime_binary_path(root).display().to_string() } else { current_exe_string() };
    fn shell_quote(value: &str) -> String = format!("'{}'", value.replace('\'', "'\\''"));
    fn cli_launcher_content(root: &Path, global: bool) -> String = {
        let target = runtime_manifest_binary(root, global);
        if cfg!(windows) { format!("@echo off\r\n\"{}\" %*\r\nexit /b %ERRORLEVEL%\r\n", target) }
        else { format!("#!/bin/sh\nexec {} \"$@\"\n", shell_quote(&target)) }
    };
    fn windows_posix_cli_shim_content() -> String = "#!/bin/sh\nDIR=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)\nexec \"$DIR/tokenzero.cmd\" \"$@\"\n".to_string();
    fn shell_launcher_content(root: &Path, global: bool) -> String = if cfg!(windows) {
        format!("@echo off\r\ncall \"{}\" run -- %*\r\nexit /b %ERRORLEVEL%\r\n", tokenzero_command(root, global))
    } else {
        format!("#!/bin/sh\nexec {} run -- \"$@\"\n", shell_quote(&tokenzero_command(root, global)))
    };
    fn tokenzero_launcher_path(root: &Path, global: bool) -> PathBuf = if global {
        root.join(".tokenzero/bin").join(if cfg!(windows) { "tokenzero.cmd" } else { "tokenzero" })
    } else { PathBuf::from(current_exe_string()) };
    fn shell_launcher_path(root: &Path, _global: bool) -> PathBuf = root.join(".tokenzero/bin").join(if cfg!(windows) { "tokenzero-shell.cmd" } else { "tokenzero-shell" });
    fn cache_path(root: &Path) -> PathBuf = root.join(".tokenzero/recovery-cache.json");
    fn current_exe_bytes() -> std::io::Result<Vec<u8>> = fs::read(std::env::current_exe()?);
    fn installed_runtime_binary_path(root: &Path) -> PathBuf = root.join(".tokenzero/bin").join(installed_runtime_binary_name());
    fn current_exe_hash_prefix() -> Option<String> = Some(sha256_bytes(&current_exe_bytes().ok()?).chars().take(16).collect());
}

fn shim_content(path: &Path, root: &Path, global: bool) -> std::io::Result<String> {
    let tool = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "shim path has no tool name"))?;
    let real = resolve_real_tool(tool, &shims_dir(root)).ok_or_else(|| {
        Error::new(
            ErrorKind::NotFound,
            format!("{tool} not found on PATH outside the shim directory"),
        )
    })?;
    // The wrap targets the STABLE launcher (which execs the current
    // versioned runtime) and is guarded on its executability: `exec` of a
    // missing path exits the non-interactive shell before any fallback line,
    // so a stale target would otherwise hard-fail every shimmed utility.
    Ok(format!(
        "#!/bin/sh\n\
         # tokenzero shim for {tool} — generated; real binary resolved at install time.\n\
         REAL={real}\n\
         TZ={launcher}\n\
         if [ \"$TOKENZERO_SHIM\" = \"1\" ] && [ -z \"$TOKENZERO_INNER\" ] && [ -x \"$TZ\" ]; then\n\
         \x20 TOKENZERO_INNER=1 exec \"$TZ\" run -- \"$REAL\" \"$@\"\n\
         fi\n\
         exec \"$REAL\" \"$@\"\n",
        real = shell_quote(&real.display().to_string()),
        launcher = shell_quote(&tokenzero_command(root, global)),
    ))
}

fn path_within_root(root: &Path, path: &Path) -> std::io::Result<bool> {
    Ok(canonicalize_existing_or_parent(path)?.starts_with(root.canonicalize()?))
}

fn canonicalize_existing_or_parent(path: &Path) -> std::io::Result<PathBuf> {
    if path.exists() {
        return path.canonicalize();
    }
    let parent = path
        .parent()
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "path has no parent"))?
        .canonicalize()?;
    let name = path
        .file_name()
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "path has no file name"))?;
    Ok(parent.join(name))
}

const EXECUTABLE_CAPABILITIES: &[&str] = &["cli", "cli-shim", "cli-runtime", "shell", "shim"];

fn make_executable_if_needed(row: &InstallWrite, path: &Path) -> std::io::Result<()> {
    if EXECUTABLE_CAPABILITIES.contains(&row.capability.as_str()) {
        set_executable(path)
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn set_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Tighten the nearest enclosing `.tokenzero` directory to owner-only on
/// unix. TokenZero-owned state (cache, manifests, shims, installed binaries)
/// is private to the user; agent config directories outside `.tokenzero`
/// keep their platform defaults. Best-effort: a chmod failure never blocks
/// the write itself.
fn restrict_tokenzero_dir(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut cursor = Some(path);
        while let Some(dir) = cursor {
            if dir.file_name().is_some_and(|name| name == ".tokenzero") {
                let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
                break;
            }
            cursor = dir.parent();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Write `content` to `path` atomically via hub `atomic_write_file`.
/// Restrict `.tokenzero` parent mode first; the hub owns temp+fsync+rename.
fn atomic_write(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent)?;
        restrict_tokenzero_dir(parent);
    }
    zero_store::atomic_write_file(path, content)
}

fn latest_manifest(root: &Path) -> Option<PathBuf> {
    fs::read_dir(root.join(".tokenzero/install"))
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().and_then(|v| v.to_str()) == Some("json")
                && path
                    .file_stem()
                    .and_then(|v| v.to_str())
                    .is_some_and(|stem| stem.starts_with("rollback-"))
        })
        .max()
}

fn rollback_id() -> String {
    format!("rollback-{}", now_unix())
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn verify_install_write(
    path: &str,
    expected: &[u8],
    observed: Vec<u8>,
) -> std::io::Result<VerificationRow> {
    let verified = observed.as_slice() == expected;
    let row = VerificationRow {
        path: path.to_string(),
        observed_sha256: sha256_bytes(&observed),
        byte_count: observed.len(),
        verified,
    };
    if verified {
        return Ok(row);
    }
    Err(Error::new(
        ErrorKind::InvalidData,
        format!(
            "install verification failed for {path}: observed sha256 {} != expected {}",
            row.observed_sha256,
            sha256_bytes(expected)
        ),
    ))
}

fn sha256(text: &str) -> String {
    sha256_bytes(text.as_bytes())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

mod content;
mod doctor;
mod inspect;
mod windows_path;

pub(crate) use content::*;
pub(crate) use windows_path::*;

pub use content::{DetectedAgent, detect_present_agents};
pub use doctor::{
    doctor, doctor_capabilities, doctor_exit_codes, doctor_explain, doctor_fix, doctor_ls,
    doctor_robot_docs, doctor_robot_triage, doctor_undo,
};
pub use inspect::inspect_client_surface;

#[cfg(test)]
#[path = "../../../tests/install/inline/lib__rollback_drift_tests.rs"]
mod rollback_drift_tests;
