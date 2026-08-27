//! Transport-neutral domain kernel (tokenzero-irx9.2).
//!
//! One in-process implementation of every registry domain operation.
//! Adapters must call [`crate::dispatcher::dispatch_operation`]; they must not
//! re-implement auth/root/mutation/ref/telemetry here or below.

use crate::expand_params::ExpandParams;
use crate::{
    EditHunk, ServeOptions, TokenZeroEngine, annotate_write_failure, shell_timeout_from_millis,
    shell_timeout_from_secs,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokenzero_core::operation_abi::{
    MigrationStatus, Operation, all_operations, resolve_operation,
};
use tokenzero_core::{
    Accounting, ChannelSeparation, ContentType, Mode, ToolResponse, count_tokens,
    detect_content_type, shell_display_command_from_argv_for_platform,
};
use tokenzero_filters::{discover, rewrite_command};
use tokenzero_runtime::{ExecutionMode, plan_command_for_platform};
use zero_abi::{TOKEN_JOB_OPERATION, TokenJobPollRequest, TokenJobPollResult, TokenJobStatus};

/// Domain-kernel dispatch errors (no JSON-RPC / MCP framing).
#[derive(Debug, Clone)]
pub enum DomainDispatchError {
    UnknownTool(String),
    InvalidArgs {
        op: String,
        message: String,
    },
    /// Adapter-owned control/composition/resource ops must not enter the kernel.
    TransportOnly(String),
}

impl DomainDispatchError {
    pub fn message_text(&self) -> String {
        match self {
            Self::UnknownTool(name) => format!("unknown tool: {name}"),
            Self::InvalidArgs { message, .. } => message.clone(),
            Self::TransportOnly(name) => {
                format!("{name} is transport-control only; not a domain engine op")
            }
        }
    }
}

/// Embedded-only dispatch error. This stays below every public CLI/MCP result
/// shape and contains no transport framing.
#[derive(Debug, Clone)]
pub struct EmbeddedDispatchError {
    pub kind: &'static str,
    pub message: String,
}

impl EmbeddedDispatchError {
    fn validation(message: impl Into<String>) -> Self {
        Self {
            kind: "validation",
            message: message.into(),
        }
    }

    fn runtime(message: impl Into<String>) -> Self {
        Self {
            kind: "runtime",
            message: message.into(),
        }
    }

    fn invalid_result(message: impl Into<String>) -> Self {
        Self {
            kind: "invalid_result",
            message: message.into(),
        }
    }
}

/// Execute the two embedded job seams without promoting either into a registry
/// domain operation. Both raw-worker v2 and the in-process ZeroStack adapter
/// call this domain-owned function; it contains no transport framing.
pub fn execute_embedded_value(
    engine: &TokenZeroEngine,
    op_name: &str,
    args: &Value,
) -> Option<Result<Value, EmbeddedDispatchError>> {
    if op_name == TOKEN_JOB_OPERATION {
        return Some(execute_raw_worker_job(engine, args));
    }
    if matches!(op_name, "shell" | "tz_shell" | "zero.shell") {
        return match args.get("background") {
            Some(Value::Bool(true)) => Some(execute_raw_worker_background_shell(engine, args)),
            Some(Value::Bool(false)) | None => None,
            Some(_) => Some(Err(EmbeddedDispatchError::validation(
                "shell background must be a boolean",
            ))),
        };
    }
    None
}

fn execute_raw_worker_background_shell(
    engine: &TokenZeroEngine,
    args: &Value,
) -> Result<Value, EmbeddedDispatchError> {
    let (command, argv) = arg_command(args).map_err(EmbeddedDispatchError::validation)?;
    if argv.is_some() {
        return Err(EmbeddedDispatchError::validation(
            "background shell requires command and does not accept argv",
        ));
    }
    let launched = engine
        .shell_background(
            &command,
            arg_str(args, "cwd").map(Path::new),
            arg_shell_timeout(args),
        )
        .map_err(EmbeddedDispatchError::runtime)?;
    let object = launched.as_object().ok_or_else(|| {
        EmbeddedDispatchError::invalid_result("background launch was not an object")
    })?;
    let id = object
        .get("job")
        .and_then(Value::as_str)
        .ok_or_else(|| EmbeddedDispatchError::invalid_result("background launch omitted job"))?;
    TokenJobPollRequest::new(id)
        .and_then(|request| request.validate().map(|()| request))
        .map_err(|error| {
            EmbeddedDispatchError::invalid_result(format!("invalid background job id: {error}"))
        })?;
    let cursor = required_u64(object, "cursor")?;
    let version = required_u64(object, "version")?;
    if cursor != 0 || version != 0 {
        return Err(EmbeddedDispatchError::invalid_result(
            "background launch cursor and version must start at zero",
        ));
    }
    // `launched` also contains the private on-disk log path. Raw-worker v2
    // intentionally emits only the stable session handle and initial cursors.
    Ok(json!({"job": id, "cursor": cursor, "version": version}))
}

fn execute_raw_worker_job(
    engine: &TokenZeroEngine,
    args: &Value,
) -> Result<Value, EmbeddedDispatchError> {
    let request: TokenJobPollRequest = serde_json::from_value(args.clone()).map_err(|error| {
        EmbeddedDispatchError::validation(format!("invalid job arguments: {error}"))
    })?;
    request.validate().map_err(|error| {
        EmbeddedDispatchError::validation(format!("invalid job arguments: {error}"))
    })?;
    let since = usize::try_from(request.since)
        .map_err(|_| EmbeddedDispatchError::validation("job since exceeds this platform"))?;
    let tail_bytes = usize::try_from(request.tail_bytes)
        .map_err(|_| EmbeddedDispatchError::validation("job tailBytes exceeds this platform"))?;
    let internal = engine
        .shell_job_wait(
            &request.id,
            Duration::from_millis(request.wait_ms),
            since,
            tail_bytes,
        )
        .map_err(|message| {
            if message.starts_with("unknown background job:") {
                EmbeddedDispatchError {
                    kind: "not_found",
                    message,
                }
            } else {
                EmbeddedDispatchError::runtime(message)
            }
        })?;
    typed_job_result(&request.id, &internal)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InternalJobPoll {
    status: TokenJobStatus,
    pid: Option<u32>,
    exit_code: Option<i32>,
    tail: Option<String>,
    tail_utf8_lossless: Option<bool>,
    tail_bytes: Option<u64>,
    log: Option<String>,
    log_bytes: Option<u64>,
    cursor: u64,
    version: u64,
    changed: Option<bool>,
    unchanged: Option<bool>,
    next_poll_ms: Option<u64>,
}

fn typed_job_result(id: &str, internal: &Value) -> Result<Value, EmbeddedDispatchError> {
    let wire: InternalJobPoll = serde_json::from_value(internal.clone()).map_err(|error| {
        EmbeddedDispatchError::invalid_result(format!("invalid internal job result: {error}"))
    })?;
    let InternalJobPoll {
        status,
        pid,
        exit_code,
        tail,
        tail_utf8_lossless,
        tail_bytes,
        log: _private_log,
        log_bytes,
        cursor,
        version,
        changed,
        unchanged,
        next_poll_ms,
    } = wire;
    let unchanged = unchanged.unwrap_or(false);
    let changed = match changed {
        Some(value) => value,
        None if unchanged => false,
        None => {
            return Err(EmbeddedDispatchError::invalid_result(
                "job poll result omitted changed/unchanged state",
            ));
        }
    };
    if changed == unchanged {
        return Err(EmbeddedDispatchError::invalid_result(
            "job poll changed and unchanged fields contradict",
        ));
    }
    let (tail, tail_utf8_lossless, tail_bytes, log_bytes) = if changed {
        (
            tail.ok_or_else(|| EmbeddedDispatchError::invalid_result("job poll omitted tail"))?,
            tail_utf8_lossless.ok_or_else(|| {
                EmbeddedDispatchError::invalid_result("job poll omitted tailUtf8Lossless")
            })?,
            tail_bytes.ok_or_else(|| {
                EmbeddedDispatchError::invalid_result("job poll omitted tailBytes")
            })?,
            log_bytes.ok_or_else(|| {
                EmbeddedDispatchError::invalid_result("job poll omitted logBytes")
            })?,
        )
    } else {
        (String::new(), true, 0, cursor)
    };
    let result = TokenJobPollResult::new(
        id,
        status,
        pid,
        exit_code,
        tail,
        tail_utf8_lossless,
        tail_bytes,
        log_bytes,
        cursor,
        version,
        changed,
        next_poll_ms,
    )
    .map_err(|error| {
        EmbeddedDispatchError::invalid_result(format!("invalid typed job result: {error}"))
    })?;
    result.validate().map_err(|error| {
        EmbeddedDispatchError::invalid_result(format!("invalid typed job result: {error}"))
    })?;
    serde_json::to_value(result)
        .map_err(|error| EmbeddedDispatchError::invalid_result(error.to_string()))
}

fn required_u64(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<u64, EmbeddedDispatchError> {
    object.get(field).and_then(Value::as_u64).ok_or_else(|| {
        EmbeddedDispatchError::invalid_result(format!("job poll result omitted unsigned {field}"))
    })
}

/// Registry-metadata classification: domain ops are Canonical/LegacyAlias and
/// not Resource. Adapter-owned control/composition ops are CodemodeControl or
/// Resource. No hard-coded name mask.
///
/// Lives in the domain kernel so `dispatcher` can depend inward without a
/// `domain` ↔ `dispatcher` module cycle.
pub fn operation_is_domain(op: &Operation) -> bool {
    match op.migration {
        MigrationStatus::Canonical | MigrationStatus::LegacyAlias => {
            op.exposure.resource_uri.is_none()
        }
        MigrationStatus::CodemodeControl | MigrationStatus::Resource => false,
    }
}

/// Whether `op_name` (canonical or alias) is a domain engine operation.
pub fn is_domain_operation(op_name: &str) -> bool {
    resolve_operation(op_name)
        .map(operation_is_domain)
        .unwrap_or(false)
}

/// Canonical domain ops exposed on FastMCP (for exhaustive tests).
pub fn domain_fastmcp_ops() -> Vec<&'static str> {
    all_operations()
        .iter()
        .filter(|op| op.exposure.fastmcp_tool && operation_is_domain(op))
        .map(|op| op.name)
        .collect()
}

/// Every registry domain operation (exhaustive, metadata-driven).
pub fn all_domain_operations() -> Vec<&'static Operation> {
    all_operations()
        .iter()
        .filter(|op| operation_is_domain(op))
        .collect()
}

/// Execute one canonical domain operation without transport framing.
pub fn execute_domain_op(
    engine: &TokenZeroEngine,
    op_name: &str,
    args: &Value,
) -> Result<ToolResponse, DomainDispatchError> {
    let op = resolve_operation(op_name)
        .ok_or_else(|| DomainDispatchError::UnknownTool(op_name.to_string()))?;
    if !operation_is_domain(op) {
        return Err(DomainDispatchError::TransportOnly(op.name.to_string()));
    }
    let canonical = op.name;
    let bare = canonical.strip_prefix("tz_").unwrap_or(canonical);
    // Legacy compact alias maps to ingest kernel path.
    let bare = if op_name == "compact" { "ingest" } else { bare };

    let map_args = |message: String| DomainDispatchError::InvalidArgs {
        op: canonical.to_string(),
        message,
    };

    let response = match bare {
        "read" => engine.read_with_options(
            &arg_path_list(args, "path").map_err(map_args)?,
            arg_mode(args),
            arg_u64(args, "start_line"),
            arg_u64(args, "end_line"),
            arg_bool(args, "raw"),
            arg_u64_or(args, "max_files", 20),
            arg_u64_or(args, "max_visible_tokens", 4000),
            arg_serve_options(args),
        ),
        "find" => {
            let query = arg_string_any(args, &["query", "pattern"]).map_err(map_args)?;
            let path = arg_path_list(args, "path").unwrap_or_else(|_| vec![PathBuf::from(".")]);
            engine.find_with_options(
                query,
                &path,
                arg_mode(args),
                arg_u64_or(args, "max_files", 20),
                arg_u64_or(args, "max_visible_tokens", 4000),
                arg_serve_options(args),
            )
        }
        "grep" => {
            let query = arg_string_any(args, &["query", "pattern"]).map_err(map_args)?;
            let path = arg_path_list(args, "path").unwrap_or_else(|_| vec![PathBuf::from(".")]);
            engine.grep_with_options(
                query,
                &path,
                arg_mode(args),
                arg_u64_or(args, "max_files", 20),
                arg_u64_or(args, "max_visible_tokens", 4000),
                arg_serve_options(args),
            )
        }
        "recall" => engine.recall(
            arg_string_any(args, &["query", "pattern"]).map_err(map_args)?,
            arg_u64_or(args, "max_hits", 50),
            arg_mode(args),
            arg_u64_or(args, "max_visible_tokens", 4000),
        ),
        "glob" => engine.glob(
            arg_string_any(args, &["pattern", "glob", "query"]).map_err(map_args)?,
            &arg_paths_or_dot(args),
            arg_bool(args, "include_hidden"),
            arg_mode(args),
            arg_u64_or(args, "max_files", 200),
            arg_u64_or(args, "max_visible_tokens", 4000),
        ),
        "tree" => engine.tree(
            &arg_paths_or_dot(args),
            arg_u64_or(args, "depth", 2),
            arg_bool(args, "include_hidden"),
            arg_mode(args),
            arg_u64_or(args, "max_files", 200),
            arg_u64_or(args, "max_visible_tokens", 4000),
        ),
        "edit" => {
            let path = arg_string_any(args, &["path"]).map_err(map_args)?;
            let mut response = engine.edit(
                Path::new(path),
                &arg_edit_hunks(args).map_err(map_args)?,
                arg_bool(args, "create"),
                arg_bool(args, "dry_run"),
                arg_mode(args),
                arg_u64_or(args, "max_visible_tokens", 4000),
            );
            if response.status == "error"
                && let Some(error) = response.error.as_mut()
            {
                error.message = annotate_write_failure(&error.message, false);
            }
            response
        }
        "shell" => {
            if args.get("background") == Some(&Value::Bool(true)) {
                match execute_raw_worker_background_shell(engine, args) {
                    Ok(launched) => job_launch_response(launched),
                    Err(err) => {
                        return Err(DomainDispatchError::InvalidArgs {
                            op: canonical.to_string(),
                            message: err.message,
                        });
                    }
                }
            } else {
                let (command, argv) = arg_command(args).map_err(map_args)?;
                let env = arg_env_map(args);
                engine.shell(
                    &command,
                    argv,
                    arg_str(args, "cwd").map(Path::new),
                    arg_mode(args),
                    arg_str(args, "rewrite"),
                    arg_bool(args, "no_rewrite"),
                    env,
                    arg_str(args, "stdin"),
                    arg_shell_timeout(args),
                )
            }
        }
        "ingest" => {
            let text = arg_string_any(args, &["text", "input"]).map_err(map_args)?;
            let tool = if op_name == "compact" {
                "compact"
            } else {
                arg_str(args, "source").unwrap_or("mcp-ingest")
            };
            let kind = content_type_from_arg(args, text);
            engine.ingest(text, kind, arg_mode(args), tool)
        }
        "expand" => {
            engine.expand_with_params(ExpandParams::from_tool_args(args).map_err(map_args)?)
        }
        "mem" => engine.mem(),
        "cache_pack" => engine.cache_pack(arg_str(args, "scope").unwrap_or("agent")),
        "rewrite" => {
            let (command, _) = arg_command(args).map_err(map_args)?;
            pretty_json_response(
                "rewrite",
                Mode::Hybrid,
                &rewrite_command(&command, arg_str(args, "mode").unwrap_or("safe"), true),
                Some(count_tokens(&command)),
            )
        }
        "discover" => pretty_json_response("discover", Mode::Hybrid, &discover(), None),
        "report_tool_issue" => {
            let tool = arg_string_any(args, &["tool", "name", "tool_name", "surface"])
                .map_err(map_args)?;
            let summary =
                arg_string_any(args, &["summary", "message", "title"]).map_err(map_args)?;
            let detail = arg_string_any(args, &["detail", "body", "repro", "context"])
                .ok()
                .or(Some(summary));
            match crate::record_tool_issue(
                &engine.config.cache_path,
                tool,
                summary,
                detail,
                Some(engine.session_id()),
            ) {
                Ok(report) => {
                    pretty_json_response("report_tool_issue", Mode::Structured, &report, None)
                }
                Err(message) => ToolResponse::error(
                    "report_tool_issue",
                    "not_reportable",
                    message,
                    Some("use tool=zero_execute (or tz_execute_code / zero.token.*) for CodeMode failures".into()),
                ),
            }
        }
        "batch" => {
            batch_response(engine, args).map_err(|message| DomainDispatchError::InvalidArgs {
                op: "tz_batch".into(),
                message,
            })?
        }
        "fetch" => engine.fetch(
            arg_string_any(args, &["url", "uri"]).map_err(map_args)?,
            arg_u64(args, "ttl_seconds"),
            arg_bool(args, "fresh"),
            arg_mode(args),
            arg_u64_or(args, "max_visible_tokens", 4000),
        ),
        other => {
            return Err(DomainDispatchError::UnknownTool(format!(
                "{canonical} (bare={other})"
            )));
        }
    };
    Ok(attach_channels(response, bare, args))
}

/// vz89.11: attach the opt-in machine-action channel. Gate off leaves the
/// response untouched, so default serialization stays byte-identical.
fn attach_channels(response: ToolResponse, bare: &str, args: &Value) -> ToolResponse {
    attach_channels_gated(
        response,
        bare,
        args,
        tokenzero_core::channel_separation_enabled(),
    )
}

/// Pure core of the gate so tests can drive both directions without touching
/// process env (the engine crate forbids unsafe env mutation).
fn attach_channels_gated(
    mut response: ToolResponse,
    bare: &str,
    args: &Value,
    enabled: bool,
) -> ToolResponse {
    if !enabled {
        return response;
    }
    response.channels = Some(ChannelSeparation {
        action: bare.to_string(),
        status_line: channel_status_line(bare, args),
        // Per-tool responses are always between tool calls: action-only, never
        // prose. The single receipt-derived user_message belongs to the terminal
        // plan envelope (see codemode exec::terminal_user_message).
        user_message: None,
    });
    response
}

/// Deterministic harness-renderable status line derived from the operation
/// and its arguments; no model prose involved (vz89.11).
fn channel_status_line(bare: &str, args: &Value) -> String {
    fn clip(text: &str, max: usize) -> String {
        let mut out: String = text.chars().take(max).collect();
        if text.chars().count() > max {
            out.push('…');
        }
        out
    }
    fn str_arg<'a>(args: &'a Value, keys: &[&str]) -> Option<&'a str> {
        keys.iter()
            .find_map(|key| args.get(*key).and_then(Value::as_str))
    }
    let paths = args.get("path").and_then(|value| match value {
        Value::String(single) => Some(clip(single, 80)),
        Value::Array(items) => {
            let joined = items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            (!joined.is_empty()).then(|| clip(&joined, 80))
        }
        _ => None,
    });
    let query = str_arg(args, &["query", "pattern"]).map(|text| clip(text, 60));
    match bare {
        "read" => format!("Reading {}", paths.unwrap_or_else(|| "file".into())),
        "find" => format!("Finding {}", query.unwrap_or_default()),
        "grep" => format!("Searching for {}", query.unwrap_or_default()),
        "glob" => format!(
            "Globbing {}",
            str_arg(args, &["pattern", "glob", "query"])
                .map(|text| clip(text, 60))
                .unwrap_or_default()
        ),
        "tree" => format!("Listing {}", paths.unwrap_or_else(|| ".".into())),
        "edit" => format!("Editing {}", paths.unwrap_or_default()),
        "shell" => {
            let command = str_arg(args, &["command"]).map(str::to_string).or_else(|| {
                args.get("argv").and_then(Value::as_array).map(|argv| {
                    argv.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
            });
            format!(
                "Running {}",
                command.map(|c| clip(&c, 80)).unwrap_or_default()
            )
        }
        "ingest" => "Storing payload".to_string(),
        "expand" => format!("Expanding {}", str_arg(args, &["ref"]).unwrap_or("ref")),
        "mem" => "Inspecting recovery cache".to_string(),
        "cache_pack" => "Building cache pack".to_string(),
        "rewrite" => "Planning rewrite".to_string(),
        "discover" => "Discovering capabilities".to_string(),
        "fetch" => format!(
            "Fetching {}",
            str_arg(args, &["url", "uri"])
                .map(|text| clip(text, 80))
                .unwrap_or_default()
        ),
        "report_tool_issue" => "Reporting tool issue".to_string(),
        "batch" => "Running batch ops".to_string(),
        other => format!("Running {other}"),
    }
}

pub fn batch_response(engine: &TokenZeroEngine, args: &Value) -> Result<ToolResponse, String> {
    let ops = batch_ops(args)?;
    let mut sections = Vec::with_capacity(ops.len());
    let mut refs: Vec<tokenzero_core::RefRecord> = Vec::new();
    let mut listed: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut raw_tokens = 0usize;
    let mut recovery_tokens = 0usize;
    let mut failed_ops = 0usize;
    let mut per_op = Vec::with_capacity(ops.len());
    for (index, (tool, op_args)) in ops.iter().enumerate() {
        let canonical = tool
            .strip_prefix("tz_")
            .map(|_| tool.as_str())
            .unwrap_or(tool.as_str());
        let position = index + 1;
        if canonical == "batch" || canonical == "tz_batch" {
            failed_ops += 1;
            sections.push(format!(
                "## {position} {tool} — error: nested batch is not allowed"
            ));
            per_op.push(json!({
                "tool": tool,
                "status": "error",
                "code": "nested_batch",
                "message": "nested batch is not allowed",
            }));
            continue;
        }
        // Sub-ops go through the shared domain kernel (not MCP framing).
        match execute_domain_op(engine, tool, op_args) {
            Ok(response) => {
                let text = response
                    .visible
                    .as_ref()
                    .map(|visible| visible.text.clone())
                    .or_else(|| {
                        response
                            .error
                            .as_ref()
                            .map(|error| format!("error: {} ({})", error.message, error.code))
                    })
                    .unwrap_or_default();
                sections.push(format!("## {position} {canonical}\n{text}"));
                if response.status == "ok" {
                    per_op.push(json!({"tool": tool, "status": "ok"}));
                } else {
                    failed_ops += 1;
                    let (code, message) = response
                        .error
                        .as_ref()
                        .map(|error| (error.code.as_str(), error.message.as_str()))
                        .unwrap_or(("operation_failed", "operation returned a non-ok status"));
                    per_op.push(json!({
                        "tool": tool,
                        "status": "error",
                        "code": code,
                        "message": message,
                    }));
                }
                if let Some(accounting) = &response.accounting {
                    raw_tokens += accounting.raw_tokens;
                    recovery_tokens += accounting.recovery_tokens;
                }
                for record in response.refs {
                    if listed.insert(record.ref_id.clone()) {
                        refs.push(record);
                    }
                }
            }
            Err(error) => {
                failed_ops += 1;
                let message = error.message_text();
                sections.push(format!("## {position} {canonical} — error: {message}"));
                per_op.push(json!({
                    "tool": tool,
                    "status": "error",
                    "code": "dispatch_error",
                    "message": message,
                }));
            }
        }
    }
    let text = sections.join("\n\n");
    let visible_tokens = count_tokens(&text);
    let exact_ref_tokens = refs.iter().map(|record| count_tokens(&record.ref_id)).sum();
    let mut response = ToolResponse::ok(
        "batch",
        arg_mode(args),
        text,
        refs,
        Accounting::measured(
            raw_tokens,
            visible_tokens,
            recovery_tokens,
            visible_tokens,
            0,
            Some(exact_ref_tokens),
        ),
    );
    response.telemetry = Some(json!({
        "ops": per_op.len(),
        "succeeded_ops": per_op.len().saturating_sub(failed_ops),
        "failed_ops": failed_ops,
        "per_op": per_op,
    }));
    if failed_ops > 0 {
        let error = ToolResponse::error(
            "batch",
            "batch_operation_failed",
            format!("{failed_ops} of {} batch operations failed", ops.len()),
            Some("inspect telemetry.per_op and retry only the failed operations".to_string()),
        );
        response.status = error.status;
        response.ack = error.ack;
        response.error = error.error;
    }
    Ok(response)
}

fn inline_response(tool: &str, mode: Mode, text: String, raw_tokens: usize) -> ToolResponse {
    let visible_tokens = count_tokens(&text);
    ToolResponse::ok(
        tool,
        mode,
        text,
        Vec::new(),
        Accounting::measured(raw_tokens, visible_tokens, 0, visible_tokens, 0, Some(0)),
    )
}

fn job_launch_response(launched: Value) -> ToolResponse {
    let job = launched
        .get("job")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let cursor = launched.get("cursor").cloned().unwrap_or(json!(0));
    let version = launched.get("version").cloned().unwrap_or(json!(0));
    let mut response = pretty_json_response("shell", Mode::Structured, &launched, None);
    response.telemetry = Some(json!({
        "job": job,
        "cursor": cursor,
        "version": version,
        "structuredContent": {
            "job": job,
            "cursor": cursor,
            "version": version
        }
    }));
    response
}

fn pretty_json_response(
    tool: &str,
    mode: Mode,
    value: &impl serde::Serialize,
    raw_tokens: Option<usize>,
) -> ToolResponse {
    let text = serde_json::to_string_pretty(value).unwrap_or_default();
    let tokens = raw_tokens.unwrap_or_else(|| count_tokens(&text));
    inline_response(tool, mode, text, tokens)
}

fn batch_ops(args: &Value) -> Result<Vec<(String, Value)>, String> {
    const MAX_BATCH_OPS: usize = 16;
    let raw = args
        .get("ops")
        .ok_or_else(|| "missing ops: an array of {tool, args} objects".to_string())?;
    // Stub-schema clients may send the array JSON-encoded as a string.
    let parsed;
    let items = match raw {
        Value::Array(items) => items,
        Value::String(text) => {
            parsed = serde_json::from_str::<Value>(text)
                .map_err(|err| format!("ops is not valid JSON: {err}"))?;
            parsed
                .as_array()
                .ok_or_else(|| "ops must be an array".to_string())?
        }
        _ => return Err("ops must be an array of {tool, args} objects".to_string()),
    };
    if items.is_empty() {
        return Err("ops must contain at least one op".to_string());
    }
    if items.len() > MAX_BATCH_OPS {
        return Err(format!("ops is capped at {MAX_BATCH_OPS} per batch"));
    }
    items
        .iter()
        .map(|item| {
            let tool = item
                .get("tool")
                .and_then(Value::as_str)
                .ok_or_else(|| "each op needs a tool name".to_string())?;
            let op_args = item.get("args").cloned().unwrap_or_else(|| json!({}));
            Ok((tool.to_string(), op_args))
        })
        .collect()
}

fn arg_mode(args: &Value) -> Mode {
    args.get("mode")
        .and_then(Value::as_str)
        .and_then(|v| v.parse().ok())
        .unwrap_or(Mode::Auto)
}

/// Per-call session-redundancy options: `fresh: true` bypasses the seen-set
/// dedup/diff layer for this call (the serve is still recorded).
fn arg_serve_options(args: &Value) -> ServeOptions {
    ServeOptions {
        fresh: arg_bool(args, "fresh"),
    }
}

fn arg_bool(args: &Value, key: &str) -> bool {
    args.get(key).is_some_and(|value| match value {
        Value::Bool(value) => *value,
        Value::String(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "true" | "1" | "yes"
        ),
        _ => false,
    })
}

fn arg_u64(args: &Value, key: &str) -> Option<usize> {
    coerce_u64(args.get(key)?).and_then(|value| usize::try_from(value).ok())
}

fn arg_u64_or(args: &Value, key: &str, default: usize) -> usize {
    arg_u64(args, key).unwrap_or(default)
}

fn arg_paths_or_dot(args: &Value) -> Vec<PathBuf> {
    arg_path_list(args, "path").unwrap_or_else(|_| vec![PathBuf::from(".")])
}

fn coerce_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    }
}

fn arg_timeout_any(args: &Value, keys: &[&str]) -> Option<Duration> {
    keys.iter().find_map(|key| {
        args.get(*key)
            .and_then(coerce_u64)
            .map(|seconds| shell_timeout_from_secs(Some(seconds)))
    })
}

/// Shell deadline spellings accepted in milliseconds.
const SHELL_TIMEOUT_MS_KEYS: &[&str] = &["timeout_ms", "timeoutMs", "shell_timeout_ms"];

/// Shell deadline spellings accepted in seconds.
const SHELL_TIMEOUT_SECS_KEYS: &[&str] = &[
    "timeout_seconds",
    "timeout_secs",
    "timeout",
    "shell_timeout_seconds",
];

/// Resolves a shell deadline from any accepted spelling, in either unit.
///
/// `timeout_ms` was previously not among the keys consulted here, so callers
/// that spelled the deadline in milliseconds had it silently discarded: the
/// command ran to completion under the default 60s timeout and was reported as
/// a success. Measured before this fix, a `{ timeout_ms: 1000 }` request ran
/// 8048ms and returned status `ok`. Milliseconds are checked first because they
/// are the more precise unit, so a caller passing both gets the tighter bound
/// rather than a unit-dependent coin flip.
fn arg_shell_timeout(args: &Value) -> Option<Duration> {
    SHELL_TIMEOUT_MS_KEYS
        .iter()
        .find_map(|key| {
            args.get(*key)
                .and_then(coerce_u64)
                .map(|millis| shell_timeout_from_millis(Some(millis)))
        })
        .or_else(|| arg_timeout_any(args, SHELL_TIMEOUT_SECS_KEYS))
}

fn arg_command(args: &Value) -> Result<(String, Option<Vec<String>>), String> {
    if let Some(value) = args.as_str() {
        return Ok((value.to_string(), None));
    }
    if let Some(items) = args.as_array() {
        let argv = string_array_arg(items, "argv")?;
        return Ok((display_command_for_argv(&argv), Some(argv)));
    }
    // Prefer structured argv when present so CLI/runtime plan fidelity is preserved.
    if let Some((key, items)) = ["argv", "args"].into_iter().find_map(|key| {
        args.get(key)
            .and_then(Value::as_array)
            .map(|items| (key, items))
    }) {
        let argv = string_array_arg(items, key)?;
        let display = arg_string_any(args, &["command", "cmd", "input", "script"])
            .map(|s| s.to_string())
            .unwrap_or_else(|_| display_command_for_argv(&argv));
        return Ok((display, Some(argv)));
    }
    if let Ok(command) = arg_string_any(args, &["command", "cmd", "input", "script"]) {
        return Ok((command.to_string(), None));
    }
    Err("missing command; expected command/cmd/input/script string or argv/args array".to_string())
}

fn display_command_for_argv(argv: &[String]) -> String {
    display_command_for_argv_on_platform(argv, tokenzero_runtime::current_platform())
}

fn display_command_for_argv_on_platform(argv: &[String], platform: &str) -> String {
    match plan_command_for_platform(argv, None, false, platform) {
        Ok(plan) if plan.execution_mode == ExecutionMode::Shell => argv.join(" "),
        _ => shell_display_command_from_argv_for_platform(argv, platform),
    }
}

fn arg_path_list(args: &Value, key: &str) -> Result<Vec<PathBuf>, String> {
    let value = args.get(key).ok_or_else(|| format!("missing {key}"))?;
    if let Some(path) = value.as_str() {
        // Stub-schema clients may send a list as its JSON-encoded string.
        if path.trim_start().starts_with('[')
            && let Ok(paths) = serde_json::from_str::<Vec<String>>(path)
        {
            if paths.is_empty() {
                return Err(format!("invalid {key}; expected non-empty array"));
            }
            return Ok(paths.into_iter().map(PathBuf::from).collect());
        }
        return Ok(vec![PathBuf::from(path)]);
    }
    if let Some(items) = value.as_array() {
        return Ok(string_array_arg(items, key)?
            .into_iter()
            .map(PathBuf::from)
            .collect());
    }
    Err(format!("invalid {key}"))
}

fn arg_edit_hunks(args: &Value) -> Result<Vec<EditHunk>, String> {
    let value = args
        .get("edits")
        .ok_or_else(|| "missing edits".to_string())?;
    let items: Vec<Value> = match value {
        Value::Array(items) => items.clone(),
        Value::String(text) => serde_json::from_str(text).map_err(|_| {
            "invalid edits; expected a JSON array of {find, replace} objects".to_string()
        })?,
        _ => return Err("invalid edits; expected array of {find, replace} objects".to_string()),
    };
    if items.is_empty() {
        return Err("invalid edits; expected non-empty array".to_string());
    }
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let find = item
                .get("find")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("invalid edits[{index}].find; expected string"))?;
            let replace = item
                .get("replace")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("invalid edits[{index}].replace; expected string"))?;
            Ok(EditHunk {
                find: find.to_string(),
                replace: replace.to_string(),
                replace_all: arg_bool(item, "replace_all"),
            })
        })
        .collect()
}

fn string_array_arg(items: &[Value], label: &str) -> Result<Vec<String>, String> {
    if items.is_empty() {
        return Err(format!(
            "invalid {label}; expected non-empty array of strings"
        ));
    }
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("invalid {label}[{index}]; expected array of strings"))
        })
        .collect()
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn arg_string_any<'a>(args: &'a Value, keys: &[&str]) -> Result<&'a str, String> {
    keys.iter()
        .find_map(|key| args.get(*key).and_then(Value::as_str))
        .ok_or_else(|| format!("missing {}", keys.join("|")))
}

fn arg_env_map(args: &Value) -> Option<std::collections::BTreeMap<String, String>> {
    let obj = args.get("env")?.as_object()?;
    let mut out = std::collections::BTreeMap::new();
    for (k, v) in obj {
        if let Some(s) = v.as_str() {
            out.insert(k.clone(), s.to_string());
        }
    }
    Some(out)
}

fn content_type_from_arg(args: &Value, text: &str) -> ContentType {
    match arg_str(args, "content_type").unwrap_or("unknown") {
        "code" => ContentType::Code,
        "shell" | "tool-output" | "shell_output" => ContentType::ShellOutput,
        "diff" => ContentType::Diff,
        "json" | "json_config" => ContentType::JsonConfig,
        "markdown" | "pack" => ContentType::Markdown,
        "log" | "logs" => ContentType::Logs,
        "search_result" => ContentType::SearchResult,
        "tree" => ContentType::Tree,
        "unknown" => ContentType::Unknown,
        _ => detect_content_type(text, None),
    }
}
