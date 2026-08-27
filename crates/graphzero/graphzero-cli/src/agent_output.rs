//! Optional `{schema_version, data, meta}` stdout envelope for agents (R-006).

use serde_json::{Value, json};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

static JSON_ENVELOPE: AtomicBool = AtomicBool::new(false);

pub fn set_json_envelope(enabled: bool) {
    JSON_ENVELOPE.store(enabled, Ordering::SeqCst);
}

pub fn json_envelope_enabled() -> bool {
    JSON_ENVELOPE.load(Ordering::SeqCst)
}

/// Render one agent-facing JSON line: either wrapped envelope or raw payload string.
fn agent_json_line(verb: &str, data: Value, envelope: bool) -> Result<String, serde_json::Error> {
    if envelope {
        serde_json::to_string(&json!({
            "schema_version": 1,
            "data": data,
            "meta": { "verb": verb, "envelope": "graphzero-cli-v1" }
        }))
    } else if data.is_string() {
        Ok(data.as_str().unwrap_or("").to_string())
    } else {
        serde_json::to_string(&data)
    }
}

fn emit_agent_json_to<W: Write>(mut writer: W, verb: &str, data: Value) -> io::Result<()> {
    let line = agent_json_line(verb, data, json_envelope_enabled())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    writeln!(writer, "{line}")
}

/// Print agent-facing JSON: either wrapped envelope or raw payload string.
pub fn emit_agent_json(verb: &str, data: Value) {
    if let Err(err) = emit_agent_json_to(io::stdout().lock(), verb, data) {
        eprintln!("graphzero: failed to emit agent json: {err}");
        std::process::exit(1);
    }
}

fn parse_agent_json_payload(raw: &str, verb: &str) -> Value {
    let verb = verb.to_string();
    serde_json::from_str(raw).unwrap_or_else(move |e| {
        json!({
            "error": "malformed JSON payload",
            "detail": e.to_string(),
            "hint": "Ensure the payload is valid JSON",
            "verb": verb,
            "error_kind": "invalid_agent_json",
        })
    })
}

/// Parse stdout line as JSON Value; on failure return structured error object.
pub fn emit_agent_json_from_str(verb: &str, raw: &str) {
    emit_agent_json(verb, parse_agent_json_payload(raw, verb));
}
