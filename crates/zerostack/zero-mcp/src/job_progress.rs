//! Capability-aware MCP progress and logging for compatibility jobs.
//!
//! Clients that send a progress token or advertise logging get a bounded start
//! event, optional short progress, and exactly one terminal event. Other
//! compatibility clients poll through their transport-specific contract.
//! ZeroKernel exposes no model-facing job namespace or polling operation.
//! This module owns transport notification policy only; TokenZero keeps job
//! execution, rendered model content, token accounting, and `tz://` handles.

use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

const MAX_MESSAGE_CHARS: usize = 160;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClientFamily {
    Amp,
    Pi,
    ClaudeCode,
    Codex,
    Grok,
    OpenCode,
    #[default]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyMode {
    /// `notifications/progress` with the client-supplied token.
    Progress,
    /// `notifications/message` (MCP logging).
    Logging,
    /// No push; the compatibility client must poll its transport contract.
    PollOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobEvent {
    Started { job_id: String },
    Progress { job_id: String, cursor: u64 },
    Completed { job_id: String, status: String },
}

#[derive(Debug, Default)]
struct Session {
    family: ClientFamily,
    logging_enabled: bool,
    progress_token: Option<Value>,
    terminals: HashSet<String>,
    pending: Vec<Value>,
}

static SESSIONS: LazyLock<Mutex<HashMap<String, Session>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock_sessions() -> std::sync::MutexGuard<'static, HashMap<String, Session>> {
    SESSIONS.lock().unwrap_or_else(|poison| poison.into_inner())
}

const CLIENT_CONTAINS: &[(&str, ClientFamily)] = &[
    ("opencode", ClientFamily::OpenCode),
    ("claude", ClientFamily::ClaudeCode),
    ("codex", ClientFamily::Codex),
    ("grok", ClientFamily::Grok),
];

const CLIENT_PREFIX: &[(&str, &str, &str, ClientFamily)] = &[
    ("amp", "amp-", "amp ", ClientFamily::Amp),
    ("pi", "pi-", "pi ", ClientFamily::Pi),
];

pub fn classify_client(name: &str) -> ClientFamily {
    let lower = name.to_ascii_lowercase();
    for &(needle, family) in CLIENT_CONTAINS {
        if lower.contains(needle) {
            return family;
        }
    }
    for &(exact, dash, space, family) in CLIENT_PREFIX {
        if lower == exact || lower.starts_with(dash) || lower.starts_with(space) {
            return family;
        }
    }
    ClientFamily::Other
}

pub fn notify_mode(
    family: ClientFamily,
    logging_enabled: bool,
    progress_token: Option<&str>,
) -> NotifyMode {
    if progress_token.is_some() {
        return NotifyMode::Progress;
    }
    match family {
        ClientFamily::Amp | ClientFamily::Pi | ClientFamily::ClaudeCode | ClientFamily::Grok => {
            NotifyMode::Logging
        }
        ClientFamily::Codex | ClientFamily::OpenCode | ClientFamily::Other => {
            if logging_enabled {
                NotifyMode::Logging
            } else {
                NotifyMode::PollOnly
            }
        }
    }
}

fn bound_message(text: &str) -> String {
    let mut out: String = text.chars().take(MAX_MESSAGE_CHARS).collect();
    if text.chars().count() > MAX_MESSAGE_CHARS {
        out.push('…');
    }
    out
}

fn usable_progress_token(token: Option<&Value>) -> Option<&Value> {
    token.filter(|value| match value {
        Value::String(_) => true,
        Value::Number(number) if number.is_i64() || number.is_u64() => true,
        _ => false,
    })
}

fn progress_token_or_job_id(token: Option<&Value>, job_id: &str) -> Value {
    usable_progress_token(token)
        .cloned()
        .unwrap_or_else(|| Value::String(job_id.to_string()))
}

fn progress_notification(token: &Value, progress: u64, total: u64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/progress",
        "params": {
            "progressToken": token,
            "progress": progress,
            "total": total,
            "message": bound_message(message),
        }
    })
}

fn logging_notification(message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/message",
        "params": {
            "level": "info",
            "logger": "tokenzero.job",
            "data": bound_message(message),
        }
    })
}

/// Plan the JSON-RPC notification for one job event. `None` means poll-only
/// or a duplicate terminal.
pub fn plan_notification(
    mode: NotifyMode,
    token: Option<&Value>,
    event: &JobEvent,
    already_terminal: bool,
) -> Option<Value> {
    match event {
        JobEvent::Completed { .. } if already_terminal => return None,
        _ => {}
    }
    match (mode, event) {
        (NotifyMode::PollOnly, _) => None,
        (NotifyMode::Progress, JobEvent::Started { job_id }) => Some(progress_notification(
            &progress_token_or_job_id(token, job_id),
            0,
            1,
            &format!("job {job_id} started"),
        )),
        (NotifyMode::Progress, JobEvent::Progress { job_id, cursor }) => {
            Some(progress_notification(
                &progress_token_or_job_id(token, job_id),
                0,
                1,
                &format!("job {job_id} bytes={cursor}"),
            ))
        }
        (NotifyMode::Progress, JobEvent::Completed { job_id, status }) => {
            Some(progress_notification(
                &progress_token_or_job_id(token, job_id),
                1,
                1,
                &format!("job {job_id} {status}"),
            ))
        }
        (NotifyMode::Logging, JobEvent::Started { job_id }) => {
            Some(logging_notification(&format!("job {job_id} started")))
        }
        (NotifyMode::Logging, JobEvent::Progress { .. }) => None,
        (NotifyMode::Logging, JobEvent::Completed { job_id, status }) => {
            Some(logging_notification(&format!("job {job_id} {status}")))
        }
    }
}

pub fn remember_client(session_id: &str, client_name: &str, _capabilities: &Value) {
    let mut sessions = lock_sessions();
    let session = sessions.entry(session_id.to_string()).or_default();
    session.family = classify_client(client_name);
}

pub fn remember_logging_enabled(session_id: &str) {
    lock_sessions()
        .entry(session_id.to_string())
        .or_default()
        .logging_enabled = true;
}

pub fn remember_progress_token_value(session_id: &str, token: Option<Value>) {
    let Some(token) = usable_progress_token(token.as_ref()).cloned() else {
        return;
    };
    lock_sessions()
        .entry(session_id.to_string())
        .or_default()
        .progress_token = Some(token);
}

pub fn observe(session_id: &str, event: JobEvent) {
    let mut sessions = lock_sessions();
    let session = sessions.entry(session_id.to_string()).or_default();
    let job_id = match &event {
        JobEvent::Started { job_id }
        | JobEvent::Progress { job_id, .. }
        | JobEvent::Completed { job_id, .. } => job_id.clone(),
    };
    let already = session.terminals.contains(&job_id);
    let token = usable_progress_token(session.progress_token.as_ref());
    let mode = if token.is_some() {
        NotifyMode::Progress
    } else {
        notify_mode(session.family, session.logging_enabled, None)
    };
    if let Some(note) = plan_notification(mode, token, &event, already) {
        session.pending.push(note);
    }
    if matches!(event, JobEvent::Completed { .. }) {
        session.terminals.insert(job_id);
    }
}

pub fn take_notifications(session_id: &str) -> Vec<Value> {
    lock_sessions()
        .get_mut(session_id)
        .map(|session| std::mem::take(&mut session.pending))
        .unwrap_or_default()
}

pub fn progress_token_from_params(params: &Value) -> Option<Value> {
    usable_progress_token(params.get("_meta")?.get("progressToken")).cloned()
}

pub fn job_id_from_tool_result(result: &Value) -> Option<String> {
    result
        .pointer("/structuredContent/job")
        .or_else(|| result.pointer("/structuredContent/cli/telemetry/job"))
        .or_else(|| result.get("job"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn job_status(value: &Value) -> Option<&str> {
    value
        .get("status")
        .or_else(|| value.pointer("/structuredContent/status"))
        .and_then(Value::as_str)
}

fn job_cursor(value: &Value) -> Option<u64> {
    value
        .get("cursor")
        .or_else(|| value.pointer("/structuredContent/cursor"))
        .and_then(Value::as_u64)
}

fn is_terminal_status(status: &str) -> bool {
    matches!(status, "exited" | "failed")
}

/// Observe a job launch handle from `tools/call`.
pub fn observe_job_launch(session_id: &str, result: &Value) -> Option<String> {
    let job_id = job_id_from_tool_result(result)?;
    observe(
        session_id,
        JobEvent::Started {
            job_id: job_id.clone(),
        },
    );
    if let Some(status) = job_status(result)
        && is_terminal_status(status)
    {
        observe(
            session_id,
            JobEvent::Completed {
                job_id: job_id.clone(),
                status: status.to_string(),
            },
        );
    }
    Some(job_id)
}

/// Observe Progress/Completed from a real `shell_job_wait` poll body.
pub fn observe_job_poll(session_id: &str, job_id: &str, poll: &Value) {
    if let Some(cursor) = job_cursor(poll)
        && cursor > 0
    {
        observe(
            session_id,
            JobEvent::Progress {
                job_id: job_id.to_string(),
                cursor,
            },
        );
    }
    if let Some(status) = job_status(poll)
        && is_terminal_status(status)
    {
        observe(
            session_id,
            JobEvent::Completed {
                job_id: job_id.to_string(),
                status: status.to_string(),
            },
        );
    }
}
