use crate::*;
use crate::McpToolSurface;

pub fn inspect_client_surface(write: &InstallWrite, root: &Path) -> ClientSurfaceStatus {
    let path = PathBuf::from(&write.path);
    let exists = path.exists();
    let checks = if exists {
        client_surface_checks(write, &path, root)
    } else {
        Default::default()
    };
    let installed = exists && !checks.is_empty() && checks.iter().all(|check| check.ok);
    ClientSurfaceStatus {
        path: write.path.clone(),
        action: write.action.clone(),
        capability: write.capability.clone(),
        global: write.global,
        exists,
        installed,
        state: if installed {
            "installed"
        } else if exists {
            "mixed"
        } else {
            "missing"
        }
        .into(),
        checks,
    }
}

const CLI_NEEDLES: &[(&str, &str)] = &[
    ("launcher_mentions_tokenzero", "tokenzero"),
    ("launcher_targets_runtime", "tokenzero-runtime-"),
];
const CLI_SHIM_NEEDLES: &[(&str, &str)] = &[
    ("launcher_mentions_tokenzero", "tokenzero"),
    ("shim_delegates_to_launcher", "tokenzero.cmd"),
];
const SHELL_NEEDLES: &[(&str, &str)] = &[("shell_launcher_mentions_run", " run -- ")];
const INSTRUCTION_NEEDLES: &[(&str, &str)] = &[("instructions_pointer", "tokenzero")];
const DEFAULT_NEEDLES: &[(&str, &str)] = &[("mentions_tokenzero", "tokenzero")];

pub(crate) fn client_surface_checks(
    write: &InstallWrite,
    path: &Path,
    root: &Path,
) -> Vec<ClientSurfaceCheck> {
    match write.capability.as_str() {
        "mcp" => mcp_surface_checks(path, root, write.global),
        "cli-runtime" => runtime_binary_checks(path),
        "runtime" => runtime_manifest_checks(path, root, write.global),
        "hooks" => hooks_surface_checks(path, root, write.global),
        "shim" => shim_surface_checks(path, root, write.global),
        capability => text_surface_checks(
            path,
            match capability {
                "cli" => CLI_NEEDLES,
                "cli-shim" => CLI_SHIM_NEEDLES,
                "shell" => SHELL_NEEDLES,
                "instructions" => INSTRUCTION_NEEDLES,
                _ => DEFAULT_NEEDLES,
            },
        ),
    }
}

fn failure(name: &str, detail: String) -> Vec<ClientSurfaceCheck> {
    vec![client_check(name, false, detail)]
}
macro_rules! checks {
    ($($name:literal => $ok:expr, $detail:expr);+ $(;)?) => {
        vec![$(client_check($name, $ok, $detail)),+]
    };
}

fn read_text_or_fail(path: &Path, name: &str) -> Result<String, Vec<ClientSurfaceCheck>> {
    fs::read_to_string(path).map_err(|err| failure(name, format!("read error: {err}")))
}

fn parse_json_or_fail(text: &str, name: &str) -> Result<Value, Vec<ClientSurfaceCheck>> {
    serde_json::from_str(text).map_err(|err| failure(name, format!("invalid JSON: {err}")))
}

fn read_json(
    path: &Path,
    read_name: &str,
    parse_name: &str,
) -> Result<Value, Vec<ClientSurfaceCheck>> {
    parse_json_or_fail(&read_text_or_fail(path, read_name)?, parse_name)
}

pub(crate) fn hooks_surface_checks(
    path: &Path,
    root: &Path,
    global: bool,
) -> Vec<ClientSurfaceCheck> {
    let parsed = match read_json(path, "hooks_settings_readable", "hooks_settings_json") {
        Ok(value) => value,
        Err(checks) => return checks,
    };
    let entries = |key| {
        parsed
            .get("hooks")
            .and_then(|hooks| hooks.get(key))
            .and_then(Value::as_array)
    };
    let entry = entries("PreToolUse")
        .and_then(|items| items.iter().find(|entry| is_tokenzero_hook_entry(entry)));
    let expected_command = hook_command(root, global);
    let command_ok = entry.is_some_and(|entry| {
        entry.get("matcher").and_then(Value::as_str) == Some("Bash")
            && entry
                .get("hooks")
                .and_then(Value::as_array)
                .is_some_and(|hooks| {
                    hooks.iter().any(|hook| {
                        hook.get("type").and_then(Value::as_str) == Some("command")
                            && hook.get("command").and_then(Value::as_str)
                                == Some(expected_command.as_str())
                    })
                })
    });
    let session = entries("SessionStart")
        .and_then(|items| items.iter().find(|entry| is_tokenzero_hook_entry(entry)));
    checks! {
        "hooks_pretooluse_tokenzero_entry" => entry.is_some(),
            "hooks.PreToolUse contains the TokenZero entry".into();
        "hooks_command_targets_installed_runtime" => command_ok,
            format!("expected Bash matcher running {expected_command}");
        "hooks_session_start_tokenzero_entry" => session.is_some(),
            "hooks.SessionStart restores the TokenZero session pack".into();
    }
}

pub(crate) fn shim_surface_checks(
    path: &Path,
    root: &Path,
    global: bool,
) -> Vec<ClientSurfaceCheck> {
    let text = match read_text_or_fail(path, "shim_readable") {
        Ok(text) => text,
        Err(checks) => return checks,
    };
    let launcher = tokenzero_command(root, global);
    checks! {
        "shim_executable" => is_executable_file(path), "executable shim script".into();
        "shim_guards_on_env" => text.contains("TOKENZERO_SHIM") && text.contains("TOKENZERO_INNER"),
            "guards on TOKENZERO_SHIM/TOKENZERO_INNER".into();
        "shim_targets_installed_runtime" => text.contains(&launcher) && text.contains("-x \"$TZ\""),
            format!("expected launcher {launcher} behind an -x guard");
    }
}

pub(crate) fn runtime_binary_checks(path: &Path) -> Vec<ClientSurfaceCheck> {
    match fs::metadata(path) {
        Ok(metadata) => vec![client_check(
            "runtime_copy_present",
            metadata.is_file() && metadata.len() > 0,
            format!("{} bytes", metadata.len()),
        )],
        Err(err) => failure("runtime_copy_present", format!("metadata error: {err}")),
    }
}

pub(crate) fn runtime_manifest_checks(
    path: &Path,
    root: &Path,
    global: bool,
) -> Vec<ClientSurfaceCheck> {
    let parsed = match read_json(path, "runtime_manifest_readable", "runtime_manifest_json") {
        Ok(value) => value,
        Err(checks) => return checks,
    };
    let binary = runtime_manifest_binary(root, global);
    let launcher = tokenzero_command(root, global);
    checks! {
        "runtime_manifest_binary" => parsed.get("binary").and_then(Value::as_str) == Some(binary.as_str()),
            format!("expected {binary}");
        "runtime_manifest_launcher" => parsed.get("global_launcher").and_then(Value::as_str) == Some(launcher.as_str()),
            format!("expected {launcher}");
        "runtime_manifest_no_external_runtime" => parsed.get("external_runtime_required").and_then(Value::as_bool) == Some(false),
            "external_runtime_required=false".into();
    }
}

pub(crate) fn text_surface_checks(
    path: &Path,
    needles: &[(&str, &str)],
) -> Vec<ClientSurfaceCheck> {
    let text = match read_text_or_fail(path, "text_readable") {
        Ok(text) => text.to_ascii_lowercase(),
        Err(checks) => return checks,
    };
    needles
        .iter()
        .map(|&(name, needle)| {
            client_check(
                name,
                text.contains(&needle.to_ascii_lowercase()),
                format!("contains {needle}"),
            )
        })
        .collect()
}

pub(crate) fn mcp_surface_checks(
    path: &Path,
    root: &Path,
    global: bool,
) -> Vec<ClientSurfaceCheck> {
    if path.extension().and_then(|ext| ext.to_str()) == Some("toml") {
        toml_mcp_surface_checks(path, root, global)
    } else {
        json_mcp_surface_checks(path, root, global)
    }
}

trait McpConfigValue: Sized {
    fn child(&self, key: &str) -> Option<&Self>;
    fn text(&self) -> Option<&str>;
    fn strings(&self) -> Option<Vec<String>>;
}

macro_rules! impl_mcp_config_value {
    ($type:ty) => {
        impl McpConfigValue for $type {
            fn child(&self, key: &str) -> Option<&Self> {
                self.get(key)
            }
            fn text(&self) -> Option<&str> {
                self.as_str()
            }
            fn strings(&self) -> Option<Vec<String>> {
                self.as_array()?
                    .iter()
                    .map(|item| item.as_str().map(str::to_owned))
                    .collect()
            }
        }
    };
}
impl_mcp_config_value!(Value);
impl_mcp_config_value!(toml::Value);

fn parsed_mcp_surface_checks<T: McpConfigValue>(
    parsed: &T,
    table: &str,
    root: &Path,
    global: bool,
) -> Vec<ClientSurfaceCheck> {
    let server = parsed
        .child(table)
        .and_then(|servers| servers.child("tokenzero"));
    let env = server.and_then(|value| value.child("env"));
    let field = |name| env.and_then(|value| value.child(name)).and_then(T::text);
    let args = server
        .and_then(|value| value.child("args"))
        .and_then(T::strings);
    mcp_server_checks(
        server.is_some(),
        server
            .and_then(|value| value.child("command"))
            .and_then(T::text),
        args.as_deref(),
        field("TOKENZERO_ALLOWED_ROOTS"),
        field("TOKENZERO_CACHE_PATH"),
        field(McpToolSurface::ENV),
        root,
        global,
    )
}

pub(crate) fn json_mcp_surface_checks(
    path: &Path,
    root: &Path,
    global: bool,
) -> Vec<ClientSurfaceCheck> {
    match read_json(path, "mcp_json_readable", "mcp_json_parse") {
        Ok(parsed) => parsed_mcp_surface_checks(&parsed, "mcpServers", root, global),
        Err(checks) => checks,
    }
}

pub(crate) fn toml_mcp_surface_checks(
    path: &Path,
    root: &Path,
    global: bool,
) -> Vec<ClientSurfaceCheck> {
    let text = match read_text_or_fail(path, "mcp_toml_readable") {
        Ok(text) => text,
        Err(checks) => return checks,
    };
    match toml::from_str::<toml::Value>(&text) {
        Ok(parsed) => parsed_mcp_surface_checks(&parsed, "mcp_servers", root, global),
        Err(err) => failure("mcp_toml_parse", format!("invalid TOML: {err}")),
    }
}

fn normalize_path_sep(value: &str) -> String {
    value.replace('\\', "/")
}
fn path_str_matches(actual: Option<&str>, expected: &str) -> bool {
    actual.map(normalize_path_sep) == Some(normalize_path_sep(expected))
}
fn path_args_match(actual: Option<&[String]>, expected: &[String]) -> bool {
    let normalize = |args: &[String]| {
        args.iter()
            .map(|arg| normalize_path_sep(arg))
            .collect::<Vec<_>>()
    };
    actual.map(normalize) == Some(normalize(expected))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn mcp_server_checks(
    server_present: bool,
    command: Option<&str>,
    args: Option<&[String]>,
    allowed_roots: Option<&str>,
    cache: Option<&str>,
    tool_surface: Option<&str>,
    root: &Path,
    global: bool,
) -> Vec<ClientSurfaceCheck> {
    let expected_command = mcp_command(root, global);
    let expected_args = mcp_args(root);
    let allowed = root.display().to_string();
    let expected_cache = cache_path(root).display().to_string();
    checks! {
        "mcp_tokenzero_server_present" => server_present,
            "mcpServers.tokenzero or mcp_servers.tokenzero".into();
        "mcp_command_targets_installed_runtime" => path_str_matches(command, &expected_command),
            format!("expected {expected_command}");
        "mcp_args_match" => path_args_match(args, &expected_args),
            format!("expected {:?}", expected_args);
        "mcp_allowed_roots_match" => path_str_matches(allowed_roots, &allowed),
            format!("expected {allowed}");
        "mcp_cache_path_match" => path_str_matches(cache, &expected_cache),
            format!("expected {expected_cache}");
        "mcp_tool_surface_valid" => tool_surface.and_then(|value| value.parse::<McpToolSurface>().ok()).is_some(),
            format!("expected {} to be classic", McpToolSurface::ENV);
    }
}

pub(crate) fn client_check(name: &str, ok: bool, detail: String) -> ClientSurfaceCheck {
    ClientSurfaceCheck {
        name: name.into(),
        ok,
        detail,
    }
}
