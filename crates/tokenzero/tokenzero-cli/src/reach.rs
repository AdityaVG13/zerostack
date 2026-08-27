use anyhow::Result;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

type Json = serde_json::Value;

const BYPASSED_HOSTS: &[(&str, &str, &str)] = &[
    (
        "Claude Code",
        "CLAUDE.md / instructions",
        "run tokenzero install --plan --instructions before applying any global write",
    ),
    (
        "Cursor",
        "MCP",
        "configure TokenZero MCP explicitly; no daemon required",
    ),
    (
        "Gemini",
        "CLI instructions",
        "add local instructions or MCP route where supported",
    ),
    (
        "Copilot",
        "MCP / editor integration",
        "configure MCP or editor task wrapper; no daemon required",
    ),
    (
        "OpenCode",
        "shell wrapper",
        "use tokenzero run/read/find/tree explicitly or install a local wrapper plan",
    ),
];

fn reach_row(
    host: &str,
    surface: &str,
    intercepted: bool,
    repair_action: &str,
    evidence: Value,
) -> Json {
    json!({"host":host,"surface":surface,"intercepted":intercepted,"bypassed":!intercepted,"unsupported":false,"repairable":true,"repair_action":repair_action,"evidence":evidence})
}

pub(crate) fn run_reach(root: PathBuf, output_json: Option<PathBuf>) -> Result<Json> {
    let agents = root.join("AGENTS.md");
    let wrapper_audit = installed_tokenzero_command_audit();
    let wrapper_intercepted = wrapper_audit["resolved_is_current_exe"] == true;
    let wrapper_evidence = wrapper_audit["resolved_path"]
        .as_str()
        .filter(|p| !p.is_empty())
        .unwrap_or("tokenzero command not found on PATH");
    let wrapper_repair = if wrapper_intercepted {
        "PATH tokenzero resolves to the current executable"
    } else {
        "use the current worktree release binary for verification or run an explicitly approved install apply before relying on global tokenzero"
    };
    let mut rows = vec![reach_row(
        "Codex",
        "AGENTS.md",
        agents.exists(),
        if agents.exists() {
            "thin local policy pointer detected"
        } else {
            "add TokenZero pointer to AGENTS.md or run install plan"
        },
        json!(agents.display().to_string()),
    )];
    rows.extend(BYPASSED_HOSTS.iter().map(|&(host, surface, repair)| {
        reach_row(
            host,
            surface,
            false,
            repair,
            json!("plan-only; no host config mutated"),
        )
    }));
    let mut local = reach_row(
        "Local shell",
        "tokenzero command",
        wrapper_intercepted,
        wrapper_repair,
        json!(wrapper_evidence),
    );
    local["details"] = wrapper_audit.clone();
    rows.push(local);
    let intercepted = rows.iter().filter(|row| row["intercepted"] == true).count();
    let all_intercepted = intercepted == rows.len();
    let report = json!({
        "schema_version":"tokenzero.reach.v1","status":if all_intercepted{"ok"}else{"partial"},"ok":all_intercepted,"exit_code":0,"root":root.display().to_string(),
        "daemon_required":false,"global_writes":false,"installed_wrapper_audit":wrapper_audit,
        "global_tokenzero_release_verification_trusted":wrapper_intercepted,
        "approved_install_required_for_global_update":!wrapper_intercepted,
        "release_verification_binary":wrapper_audit["current_exe"].as_str().unwrap_or_default(),"rows":rows
    });
    if let Some(output) = output_json {
        write_json_artifact(&output, &report)?
    }
    Ok(report)
}

pub(crate) fn installed_tokenzero_command_audit() -> Json {
    let current_exe = std::env::current_exe().ok();
    let candidates = tokenzero_path_candidates();
    let resolved_path = candidates.first();
    let resolved_is_current_exe = current_exe
        .as_ref()
        .zip(resolved_path)
        .is_some_and(|(a, b)| same_path_for_audit(a, b));
    let current_exe_on_path = current_exe.as_ref().is_some_and(|current| {
        candidates
            .iter()
            .any(|candidate| same_path_for_audit(current, candidate))
    });
    let status = if candidates.is_empty() {
        "missing"
    } else if resolved_is_current_exe {
        "current_exe"
    } else {
        "external_or_wrapper"
    };
    json!({
        "schema_version":"tokenzero.installed_wrapper_audit.v1","status":status,"command":"tokenzero",
        "current_exe":current_exe.as_ref().map(|p|p.display().to_string()).unwrap_or_default(),
        "resolved_path":resolved_path.map(|p|p.display().to_string()).unwrap_or_default(),
        "candidate_paths":candidates.iter().map(|p|p.display().to_string()).collect::<Vec<_>>(),
        "candidate_count":candidates.len(),"resolved_is_current_exe":resolved_is_current_exe,
        "current_exe_on_path":current_exe_on_path,"approved_install_required_for_global_update":!resolved_is_current_exe,
        "daemon_required":false,"global_writes":false
    })
}

fn tokenzero_path_candidates() -> Vec<PathBuf> {
    let Some(path) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    let mut candidates: Vec<PathBuf> = Vec::new();
    for dir in std::env::split_paths(&path) {
        for name in tokenzero_command_names() {
            let candidate = dir.join(name);
            if is_tokenzero_command_candidate(&candidate)
                && !candidates
                    .iter()
                    .any(|p| same_path_for_audit(p, &candidate))
            {
                candidates.push(candidate)
            }
        }
    }
    candidates
}

fn is_tokenzero_command_candidate(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn tokenzero_command_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &[
            "tokenzero.exe",
            "tokenzero.cmd",
            "tokenzero.bat",
            "tokenzero.ps1",
            "tokenzero",
        ]
    } else {
        &["tokenzero"]
    }
}

fn same_path_for_audit(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    if cfg!(windows) {
        left.display()
            .to_string()
            .eq_ignore_ascii_case(&right.display().to_string())
    } else {
        left == right
    }
}

fn write_json_artifact(path: &Path, report: &Json) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?
    }
    tokenzero_engine::render::write_atomic(
        path,
        (serde_json::to_string_pretty(report)? + "\n").as_bytes(),
    )?;
    Ok(())
}
