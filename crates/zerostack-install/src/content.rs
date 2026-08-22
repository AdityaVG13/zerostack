use crate::*;
use crate::McpToolSurface;

pub(crate) fn content_for(
    row: &InstallWrite,
    root: &Path,
    previous: Option<&str>,
    surface: McpToolSurface,
) -> std::io::Result<Vec<u8>> {
    let previous = previous.unwrap_or_default();
    let text = match row.capability.as_str() {
        "mcp"
            if Path::new(&row.path)
                .extension()
                .and_then(|ext| ext.to_str())
                == Some("toml") =>
        {
            merge_toml_mcp(previous, root, Path::new(&row.path), row.global, surface)
        }
        "mcp" => merge_json_mcp(previous, root, row.global, surface),
        "instructions" => merge_instructions(previous, surface),
        "shell" => Ok(shell_launcher_content(root, row.global)),
        "cli" => Ok(cli_launcher_content(root, row.global)),
        "cli-shim" => Ok(windows_posix_cli_shim_content()),
        "hooks" => merge_json_hooks(previous, &hook_command(root, row.global)),
        "shim" => shim_content(Path::new(&row.path), root, row.global),
        "cli-runtime" => return current_exe_bytes(),
        _ => Ok(serde_json::json!({
            "schema_version": "tokenzero.runtime_manifest.v1",
            "runtime": "rust",
            "external_runtime_required": false,
            "binary": runtime_manifest_binary(root, row.global),
            "source_binary": current_exe_string(),
            "global_launcher": tokenzero_command(root, row.global)
        })
        .to_string()
            + "\n"),
    }?;
    Ok(text.into_bytes())
}

/// Parse `previous` as a JSON object document (empty input -> empty object),
/// preserving the exact error wording each merge fn reported before dedup.
fn parse_json_object(previous: &str, what: &str) -> std::io::Result<Map<String, Value>> {
    if previous.trim().is_empty() {
        return Ok(Map::new());
    }
    let value: Value = serde_json::from_str(previous)
        .map_err(|err| Error::new(ErrorKind::InvalidData, format!("invalid JSON: {err}")))?;
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(Error::new(
            ErrorKind::InvalidData,
            format!("{what} must be an object"),
        )),
    }
}

/// `object[key]` as a mutable object, inserting an empty one if absent.
fn object_entry<'a>(
    object: &'a mut Map<String, Value>,
    key: &str,
    what: &str,
) -> std::io::Result<&'a mut Map<String, Value>> {
    object
        .entry(key)
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| Error::new(ErrorKind::InvalidData, format!("{what} must be an object")))
}

/// Replace the TokenZero-owned entries in the `hooks.<key>` array, leaving
/// every foreign entry untouched.
fn upsert_tokenzero_hooks(
    hooks_object: &mut Map<String, Value>,
    key: &str,
    new_entries: Vec<Value>,
) -> std::io::Result<()> {
    let entries = hooks_object
        .entry(key)
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidData,
                format!("Claude settings hooks.{key} field must be an array"),
            )
        })?;
    entries.retain(|entry| !is_tokenzero_hook_entry(entry));
    entries.extend(new_entries);
    Ok(())
}

pub(crate) fn merge_json_mcp(
    previous: &str,
    root: &Path,
    global: bool,
    mcp_surface: McpToolSurface,
) -> std::io::Result<String> {
    let mut object = parse_json_object(previous, "JSON MCP config")?;
    let servers = object_entry(
        &mut object,
        "mcpServers",
        "JSON MCP config mcpServers field",
    )?;
    servers.insert(
        "tokenzero".to_string(),
        mcp_server_json(root, global, mcp_surface),
    );
    Ok(serde_json::to_string_pretty(&Value::Object(object))? + "\n")
}

/// Upsert only the TokenZero-owned `hooks.PreToolUse` and
/// `hooks.SessionStart` entries into a Claude settings document, mirroring
/// `merge_json_mcp`'s one-key merge: every other hook and setting survives
/// the round-trip (key order may change). The SessionStart entry restores
/// the TokenZero session pack after compaction/resume.
pub(crate) fn merge_json_hooks(previous: &str, hook_command: &str) -> std::io::Result<String> {
    let mut object = parse_json_object(previous, "Claude settings JSON")?;
    let hooks_object = object_entry(&mut object, "hooks", "Claude settings hooks field")?;
    upsert_tokenzero_hooks(
        hooks_object,
        "PreToolUse",
        vec![
            hook_matcher_entry("Bash", hook_command),
            // Same adapter command: the hook dispatches on tool_name, so the
            // Read matcher reuses it for the unbounded-large-Read guard.
            hook_matcher_entry("Read", hook_command),
        ],
    )?;
    upsert_tokenzero_hooks(
        hooks_object,
        "SessionStart",
        vec![serde_json::json!({
            "hooks": [{
                "type": "command",
                // `<launcher> hook claude-code` + suffix = the SessionStart
                // adapter; the shared needle keeps both entries TokenZero-owned.
                "command": format!("{hook_command}-session-start"),
                "timeout": 10,
            }]
        })],
    )?;
    Ok(serde_json::to_string_pretty(&Value::Object(object))? + "\n")
}

fn hook_matcher_entry(matcher: &str, hook_command: &str) -> Value {
    serde_json::json!({
        "matcher": matcher,
        "hooks": [{
            "type": "command",
            "command": hook_command,
            "timeout": 10,
        }]
    })
}

/// One AI harness detected on this machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedAgent {
    pub agent: String,
    pub evidence: String,
    /// Whether `tokenzero install --agent <agent>` can wire it today;
    /// unsupported harnesses are adapted manually per docs/install.md.
    pub supported: bool,
}

/// Probe the machine for known AI coding harnesses: config directories under
/// the home root plus launcher binaries on PATH. Pure over its inputs so
/// tests pass a fixture home and PATH string; detection never writes.
pub fn detect_present_agents(home: &Path, path_env: Option<&str>) -> Vec<DetectedAgent> {
    type Probe = (
        &'static str,
        &'static [&'static str],
        &'static [&'static str],
        bool,
    );
    const PROBES: &[Probe] = &[
        ("claude", &[".claude", ".claude.json"], &["claude"], true),
        ("codex", &[".codex"], &["codex"], true),
        ("cursor", &[".cursor"], &["cursor-agent", "cursor"], true),
        ("gemini", &[".gemini"], &["gemini"], true),
        ("factory", &[".factory"], &["droid"], true),
        ("opencode", &[".config/opencode"], &["opencode"], true),
        ("grok", &[".grok"], &["grok"], true),
        ("windsurf", &[".codeium/windsurf"], &["windsurf"], false),
        ("cline", &[".cline"], &[], false),
        ("aider", &[".aider.conf.yml"], &["aider"], false),
        ("zed", &[".config/zed"], &["zed"], false),
        ("crush", &[".config/crush"], &["crush"], false),
    ];
    PROBES
        .iter()
        .filter_map(|&(agent, homes, binaries, supported)| {
            let home_evidence = homes.iter().find_map(|rel| {
                let path = home.join(rel);
                path.exists().then(|| format!("{} exists", path.display()))
            });
            let evidence = home_evidence.or_else(|| {
                path_env.and_then(|path| {
                    std::env::split_paths(path).find_map(|dir| {
                        binaries
                            .iter()
                            .find(|binary| is_executable_file(&dir.join(binary)))
                            .map(|binary| format!("{binary} on PATH"))
                    })
                })
            });
            evidence.map(|evidence| DetectedAgent {
                agent: agent.to_string(),
                evidence,
                supported,
            })
        })
        .collect()
}

/// The TokenZero-owned PreToolUse entry is the one whose hook command invokes
/// `... hook claude-code` from a tokenzero binary; both fragments are matched
/// separately because the launcher/runtime file name varies per install
/// (`tokenzero`, `tokenzero.cmd`, `tokenzero-runtime-<hash>`).
pub(crate) fn is_tokenzero_hook_entry(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks
                .iter()
                .filter_map(|hook| hook.get("command")?.as_str())
                .any(|command| {
                    command.contains("tokenzero") && command.contains("hook claude-code")
                })
        })
}

/// Claude Code runs hook commands through a shell, so the launcher path is
/// quoted (homes with spaces are a tested install target).
pub(crate) fn hook_command(root: &Path, global: bool) -> String {
    let launcher = tokenzero_command(root, global);
    if cfg!(windows) {
        format!("\"{launcher}\" hook claude-code")
    } else {
        format!("{} hook claude-code", shell_quote(&launcher))
    }
}

pub(crate) fn merge_toml_mcp(
    previous: &str,
    root: &Path,
    path: &Path,
    global: bool,
    mcp_surface: McpToolSurface,
) -> std::io::Result<String> {
    let mut merged = strip_tokenzero_managed_toml(previous)
        .trim_end()
        .to_string();
    if !merged.is_empty() {
        merged.push_str("\n\n");
    }
    merged.push_str(&toml_mcp_block(root, path, global, mcp_surface));
    toml::from_str::<toml::Value>(&merged)
        .map_err(|err| Error::new(ErrorKind::InvalidData, format!("invalid TOML: {err}")))?;
    Ok(merged)
}

const INSTRUCTIONS_START: &str = "<!-- tokenzero:rust-core:start -->";
const INSTRUCTIONS_END: &str = "<!-- tokenzero:rust-core:end -->";

/// Upsert only TokenZero's managed instruction block. Every byte outside the
/// reserved markers remains unchanged, so existing project law is never
/// replaced by generated installer content.
pub(crate) fn merge_instructions(
    previous: &str,
    surface: McpToolSurface,
) -> std::io::Result<String> {
    let starts = previous
        .match_indices(INSTRUCTIONS_START)
        .collect::<Vec<_>>();
    let ends = previous.match_indices(INSTRUCTIONS_END).collect::<Vec<_>>();
    let managed = instructions_content(surface);

    match (starts.as_slice(), ends.as_slice()) {
        ([], []) => {
            let mut merged = previous.to_string();
            if !merged.is_empty() {
                if !merged.ends_with('\n') {
                    merged.push('\n');
                }
                if !merged.ends_with("\n\n") {
                    merged.push('\n');
                }
            }
            merged.push_str(&managed);
            Ok(merged)
        }
        ([(start, _)], [(end, _)]) if *end > *start => {
            let after = *end + INSTRUCTIONS_END.len();
            let managed_without_boundary = managed.strip_suffix('\n').ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidData,
                    "TokenZero managed instructions must end with LF",
                )
            })?;
            Ok(format!(
                "{}{}{}",
                &previous[..*start],
                managed_without_boundary,
                &previous[after..]
            ))
        }
        _ => Err(Error::new(
            ErrorKind::InvalidData,
            "AGENTS.md has malformed or duplicate TokenZero instruction markers",
        )),
    }
}

fn instructions_content(_surface: McpToolSurface) -> String {
    let body = "Use `tokenzero read/find/tree/run/expand` or MCP aliases. Rust Core runs as a standalone binary for normal use.";
    format!("{INSTRUCTIONS_START}\n{body}\n{INSTRUCTIONS_END}\n")
}

pub(crate) fn mcp_server_json(root: &Path, global: bool, mcp_surface: McpToolSurface) -> Value {
    serde_json::json!({
        "type": "stdio",
        "command": mcp_command(root, global),
        "args": mcp_args(root),
        "env": mcp_env(root, mcp_surface)
    })
}

pub(crate) fn mcp_args(root: &Path) -> Vec<String> {
    vec![
        "mcp-server".to_string(),
        "--allowed-root".to_string(),
        root.display().to_string(),
        "--cache-path".to_string(),
        cache_path(root).display().to_string(),
    ]
}

pub(crate) fn mcp_env(root: &Path, mcp_surface: McpToolSurface) -> Map<String, Value> {
    // Insertion order is part of the generated-file contract (JSON/TOML byte identity).
    [
        ("TOKENZERO_ALLOWED_ROOTS", root.display().to_string()),
        (
            "TOKENZERO_CACHE_PATH",
            cache_path(root).display().to_string(),
        ),
        ("TOKENZERO_DEFAULT_MODE", "auto".to_string()),
        (McpToolSurface::ENV, mcp_surface.as_str().to_string()),
        ("TOKENZERO_MAX_OUTPUT_BYTES", "2000000".to_string()),
        ("TOKENZERO_SHELL_TIMEOUT", "30".to_string()),
        ("TOKENZERO_CACHE_BLOBS", "512".to_string()),
        ("TOKENZERO_CACHE_UNITS", "8192".to_string()),
        ("TOKENZERO_MCP_IDLE_TIMEOUT_SECS", "0".to_string()),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), Value::String(value)))
    .collect()
}

pub(crate) fn toml_mcp_block(
    root: &Path,
    path: &Path,
    global: bool,
    mcp_surface: McpToolSurface,
) -> String {
    let include_codex_tool_approvals = path
        .components()
        .any(|component| component.as_os_str() == ".codex");
    let mut block = format!(
        "# tokenzero:mcp:start\n\
         [mcp_servers.tokenzero]\n\
         command = {}\n\
         args = {}\n\
         enabled = true\n\
         startup_timeout_sec = 15\n\
         tool_timeout_sec = 120\n\n\
         [mcp_servers.tokenzero.env]\n{}\n",
        toml_string(&mcp_command(root, global)),
        toml_string_array(&mcp_args(root)),
        toml_env_lines(root, mcp_surface)
    );
    if include_codex_tool_approvals {
        for tool in ["shell", "read", "find", "tree"] {
            block.push_str(&format!(
                "\n[mcp_servers.tokenzero.tools.{tool}]\napproval_mode = \"approve\"\n"
            ));
        }
    }
    block.push_str("# tokenzero:mcp:end\n");
    block
}

pub(crate) fn toml_env_lines(root: &Path, mcp_surface: McpToolSurface) -> String {
    let env = mcp_env(root, mcp_surface);
    env.into_iter()
        .map(|(key, value)| {
            let value = value.as_str().unwrap_or_default();
            format!("{key} = {}", toml_string(value))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn strip_tokenzero_managed_toml(input: &str) -> String {
    let mut output = Vec::new();
    let mut skipping = false;
    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed == "# tokenzero:mcp:start" {
            skipping = true;
        } else if trimmed == "# tokenzero:mcp:end" {
            skipping = false;
        } else {
            if let Some(table) = toml_table_name(line) {
                skipping =
                    table == "mcp_servers.tokenzero" || table.starts_with("mcp_servers.tokenzero.");
            }
            if !skipping {
                output.push(line);
            }
        }
    }
    let mut text = output.join("\n");
    if input.ends_with('\n') && !text.is_empty() {
        text.push('\n');
    }
    text
}

pub(crate) fn toml_table_name(line: &str) -> Option<String> {
    let trimmed = line.trim();
    (!trimmed.starts_with("[[") && trimmed.starts_with('[') && trimmed.ends_with(']'))
        .then(|| trimmed[1..trimmed.len() - 1].trim().to_string())
}

pub(crate) fn toml_string_array(values: &[String]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| toml_string(value))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

pub(crate) fn toml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

#[cfg(test)]
#[path = "../../../tests/install/inline/content__instruction_merge_tests.rs"]
mod instruction_merge_tests;
