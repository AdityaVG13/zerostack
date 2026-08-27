//! Fail-open Claude Code hook adapters.
use crate::cli_args::{HookArgs, HookTarget};
use serde_json::{Value, json};
use std::io::{Read, Write};

const SKIP_PROGRAMS: &[&str] = &[
    "vim",
    "vi",
    "nano",
    "less",
    "more",
    "top",
    "htop",
    "ssh",
    "python -i",
    "irb",
    "psql",
    "mysql",
    "docker exec -it",
    "git rebase -i",
    "git add -i",
    "sudo",
];
const STATE_PROGRAMS: &[&str] = &["cd", "export", "unset", "alias"];
const READ_GUARD_DEFAULT_MAX_BYTES: u64 = 65536;

pub(crate) fn handle_hook(args: HookArgs) {
    match args.target {
        HookTarget::ClaudeCode(hook) => run_claude_code_hook(&hook.mode),
        HookTarget::ClaudeCodeSessionStart(hook) => run_hook(
            "claude-code-session-start",
            r#"printf '%s\n' '{"hook_event_name":"SessionStart","source":"resume","cwd":"."}' | tokenzero hook claude-code-session-start"#,
            |input| session_start_decision(input, hook.max_tokens),
        ),
    }
}

fn run_hook(target: &str, example: &str, decide: impl FnOnce(&str) -> Option<Value>) {
    let mut input = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("error: failed to read JSON for hook {target}: {error}");
        std::process::exit(1);
    }
    if input.trim().is_empty() {
        hook_input_error(target, example, "stdin was empty");
    }
    if let Err(error) = serde_json::from_str::<Value>(&input) {
        hook_input_error(
            target,
            example,
            &format!("stdin was not valid JSON: {error}"),
        );
    }
    if let Some(decision) = decide(&input) {
        let _ = writeln!(std::io::stdout(), "{decision}");
    }
}

fn hook_input_error(target: &str, example: &str, problem: &str) -> ! {
    eprintln!("error: hook {target} requires JSON on stdin ({problem})");
    eprintln!("usage: {example}");
    std::process::exit(2);
}

fn session_start_decision(input: &str, max_tokens: usize) -> Option<Value> {
    let payload: Value = serde_json::from_str(input).ok()?;
    if payload.get("hook_event_name")?.as_str()? != "SessionStart"
        || !matches!(payload.get("source")?.as_str()?, "compact" | "resume")
    {
        return None;
    }
    let cwd = std::path::Path::new(payload.get("cwd")?.as_str()?);
    let cache = tokenzero_engine::default_recovery_cache_path(cwd);
    let pack = tokenzero_engine::session_pack(&cache, max_tokens.max(50))?;
    Some(json!({"hookSpecificOutput": {
        "hookEventName": "SessionStart", "additionalContext": pack
    }}))
}

fn run_claude_code_hook(mode: &str) {
    run_hook(
        "claude-code",
        r#"printf '%s\n' '{"tool_name":"Bash","tool_input":{"command":"git status"}}' | tokenzero hook claude-code"#,
        |input| {
            let exe = std::env::current_exe().ok()?;
            claude_code_decision(
                mode,
                input,
                exe.to_str()?,
                no_wrap_enabled(std::env::var("TOKENZERO_NO_WRAP").ok()),
            )
        },
    );
}

fn claude_code_decision(mode: &str, input: &str, self_exe: &str, no_wrap: bool) -> Option<Value> {
    let payload: Value = serde_json::from_str(input).ok()?;
    let tool = payload.get("tool_name")?.as_str()?;
    if no_wrap {
        return None;
    }
    if tool == "Read" {
        if !matches!(mode, "rewrite" | "guide") {
            return None;
        }
        return read_guard_decision(
            payload.get("tool_input")?.as_object()?,
            read_guard_threshold(std::env::var("TOKENZERO_READ_GUARD_MAX_BYTES").ok()),
        );
    }
    if tool != "Bash" {
        return None;
    }
    let input = payload.get("tool_input")?.as_object()?;
    let command = input.get("command")?.as_str()?;
    match mode {
        "rewrite" => {
            let mut updated = input.clone();
            updated.insert(
                "command".into(),
                Value::String(rewrite_decision(command, self_exe)?),
            );
            Some(json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse", "permissionDecision": "allow",
                "updatedInput": Value::Object(updated)
            }}))
        }
        "guide" if !should_skip(command) => Some(json!({"hookSpecificOutput": {
            "hookEventName": "PreToolUse", "permissionDecision": "deny",
            "permissionDecisionReason": "TokenZero routing: use the TokenZero MCP tools (read/find/grep/glob/tree/shell) for this operation, or run it as `tokenzero run -- <command>` to keep output compact and recoverable."
        }})),
        _ => None,
    }
}

fn read_guard_decision(input: &serde_json::Map<String, Value>, threshold: u64) -> Option<Value> {
    if input.contains_key("limit") || input.contains_key("offset") {
        return None;
    }
    let path = input.get("file_path")?.as_str()?;
    let size = std::fs::metadata(path).ok()?.len();
    if size <= threshold {
        return None;
    }
    Some(json!({"hookSpecificOutput": {
        "hookEventName": "PreToolUse", "permissionDecision": "deny",
        "permissionDecisionReason": format!(
            "TokenZero routing: this file is {} KB; an unbounded Read would put roughly {} tokens in context with no recovery ref. Use tz_read on the tokenzero MCP server for a compact capsule with exact refs, or re-call Read with limit/offset for just the slice you need (a bounded native Read is always allowed, and is required before Edit).",
            size / 1024, size / 4
        )
    }}))
}

fn read_guard_threshold(value: Option<String>) -> u64 {
    match value.as_deref().map(str::parse::<u64>) {
        Some(Ok(0)) => u64::MAX,
        Some(Ok(bytes)) => bytes,
        _ => READ_GUARD_DEFAULT_MAX_BYTES,
    }
}

pub(crate) fn rewrite_decision(command: &str, self_exe: &str) -> Option<String> {
    (!should_skip(command)).then(|| {
        format!(
            "{} run -- sh -c {}",
            single_quote(self_exe),
            single_quote(command)
        )
    })
}

fn no_wrap_enabled(value: Option<String>) -> bool {
    value.is_some_and(|value| !matches!(value.to_ascii_lowercase().as_str(), "0" | "false" | "off"))
}

fn should_skip(command: &str) -> bool {
    let command = command.trim();
    if command.is_empty()
        || command.contains("tokenzero")
        || command.starts_with("TOKENZERO_NO_WRAP=")
        || command.contains("<<")
    {
        return true;
    }
    let Some(segments) = top_level_segments(command) else {
        return true;
    };
    segments.iter().any(|segment| {
        let segment = segment.trim();
        segment.is_empty()
            || STATE_PROGRAMS
                .iter()
                .chain(SKIP_PROGRAMS)
                .any(|program| starts_with_program(segment, program))
    })
}

fn top_level_segments(command: &str) -> Option<Vec<String>> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let (mut single, mut double) = (false, false);
    while let Some(ch) = chars.next() {
        if single {
            single = ch != '\'';
            current.push(ch);
            continue;
        }
        if double {
            if ch == '\\' {
                current.push(ch);
                if let Some(next) = chars.next() {
                    current.push(next);
                }
                continue;
            }
            double = ch != '"';
            current.push(ch);
            continue;
        }
        match ch {
            '\\' => {
                current.push(ch);
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            '\'' => {
                single = true;
                current.push(ch);
            }
            '"' => {
                double = true;
                current.push(ch);
            }
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                segments.push(std::mem::take(&mut current));
            }
            '&' if current.ends_with('>') || chars.peek() == Some(&'>') => current.push(ch),
            '&' => return None,
            '|' => {
                if chars.peek() == Some(&'|') {
                    chars.next();
                }
                segments.push(std::mem::take(&mut current));
            }
            ';' => segments.push(std::mem::take(&mut current)),
            _ => current.push(ch),
        }
    }
    segments.push(current);
    Some(segments)
}

fn starts_with_program(command: &str, program: &str) -> bool {
    command
        .strip_prefix(program)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

fn single_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\"'\"'"))
}
