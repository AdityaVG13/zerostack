//! Normalize CodeMode `fs.*` returns to hub `zero-result` (fszero-r7bo).
//!
//! Public `zero.fs.*` plan results must use the tagged envelope from
//! `zero_abi::ZeroResult`. Wrong-field access is a typed error on the Rust
//! side; JS hosts must not synthesize empty strings for missing fields.

use serde_json::{Value, json};
use zero_abi::{ZeroResultAccessError, ZeroResult};

/// Build a `zero-result` envelope from one FSZero CodeMode step.
///
/// - Canonical `fz://` / `gz://` / `tz://` recovery keys become `content.kind=ref`.
/// - Non-canonical keys (legacy aliases such as `ls_manifest`) stay inline so
///   the envelope never emits a schema-rejected ref.
/// - Failures are always inline `{ok:false, detail, method}` under ack `X0`
///   (or the provided error ack).
pub fn zero_result_from_fs_step(
    ack: &str,
    ok: bool,
    method: &str,
    recovery_key: &str,
    payload_wire: &Value,
    detail: Option<&str>,
) -> ZeroResult {
    let ack = normalize_ack(ack, ok);
    if !ok {
        return inline_or_x0(
            &ack,
            json!({
                "ok": false,
                "method": method,
                "detail": detail.unwrap_or("failed"),
            }),
        );
    }

    if let Some(reference) = canonical_zeroref(recovery_key).or_else(|| {
        payload_wire
            .get("ref")
            .and_then(Value::as_str)
            .and_then(canonical_zeroref)
    }) {
        let preview = payload_wire
            .get("preview")
            .and_then(Value::as_str)
            .map(|s| truncate_chars(s, zero_abi::MAX_PREVIEW_CHARS))
            .or_else(|| detail.map(|d| truncate_chars(d, zero_abi::MAX_PREVIEW_CHARS)));
        match ZeroResult::reference(ack.clone(), reference, preview) {
            Ok(result) => return result,
            Err(_) => {}
        }
    }

    let value = if let Some(text) = payload_wire.as_str() {
        json!(text)
    } else if payload_wire.is_null() {
        json!({
            "ok": true,
            "method": method,
            "detail": detail,
        })
    } else {
        payload_wire.clone()
    };
    inline_or_x0(&ack, value)
}

fn normalize_ack(ack: &str, ok: bool) -> String {
    let trimmed = ack.trim();
    if (1..=zero_abi::MAX_ACK_CHARS).contains(&trimmed.chars().count()) {
        trimmed.to_string()
    } else if ok {
        "ok".into()
    } else {
        "X0".into()
    }
}

fn inline_or_x0(ack: &str, value: Value) -> ZeroResult {
    ZeroResult::inline(ack, value).unwrap_or_else(|_| {
        ZeroResult::inline(
            "X0",
            json!({"ok": false, "detail": "invalid zero-result ack"}),
        )
        .expect("X0 inline always valid")
    })
}

fn canonical_zeroref(reference: &str) -> Option<&str> {
    let valid_scheme = ["fz://", "gz://", "tz://"]
        .iter()
        .any(|prefix| reference.starts_with(prefix));
    let suffix = reference
        .split_once("://")
        .map(|(_, suffix)| suffix)
        .unwrap_or_default();
    if valid_scheme && !suffix.is_empty() && !reference.chars().any(char::is_whitespace) {
        Some(reference)
    } else {
        None
    }
}

fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// Serialize a result for the JS/CodeMode wire (only the canonical tagged shape).
pub fn zero_result_to_wire(result: &ZeroResult) -> Value {
    serde_json::to_value(result).expect("ZeroResult always serializes")
}

/// Loud failure when a consumer asks for the wrong content kind.
pub fn wrong_accessor_message(err: ZeroResultAccessError) -> String {
    err.to_string()
}

#[cfg(test)]
#[path = "../../../../../tests/fszero/unit/fs-zero/zero_result_tests.rs"]
mod tests;
