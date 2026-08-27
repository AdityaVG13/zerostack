//! Golden contract vectors for the operation ABI (not full engine e2e).

use serde_json::{Value, json};

use super::types::{DomainError, DomainErrorKind, DomainResult};

/// One golden vector describing expected normalized domain shape.
#[derive(Clone, Debug)]
pub struct GoldenVector {
    pub id: &'static str,
    pub op: &'static str,
    pub tags: &'static [&'static str],
    pub args: Value,
    pub expected_ok: Option<DomainResult>,
    pub expected_err: Option<DomainError>,
}

/// Contract-focused golden vectors covering success, typed failure, ref recovery,
/// mutation, deadline, and cancellation shapes.
pub fn golden_vectors() -> Vec<GoldenVector> {
    vec![
        GoldenVector {
            id: "read_success_shape",
            op: "tz_read",
            tags: &["success", "ref_recovery"],
            args: json!({ "path": "README.md" }),
            expected_ok: Some(
                DomainResult::new("tz_read", json!({ "text": "…", "status": "ok" }))
                    .with_refs(vec!["tz://blob/example".into()]),
            ),
            expected_err: None,
        },
        GoldenVector {
            id: "read_validation_missing_path",
            op: "tz_read",
            tags: &["typed_failure"],
            args: json!({}),
            expected_ok: None,
            expected_err: Some(
                DomainError::new(DomainErrorKind::Validation, "path is required")
                    .with_op("tz_read"),
            ),
        },
        GoldenVector {
            id: "expand_ref_recovery",
            op: "tz_expand",
            tags: &["success", "ref_recovery"],
            args: json!({ "ref": "tz://blob/deadbeef" }),
            expected_ok: Some(
                DomainResult::new("tz_expand", json!({ "text": "payload", "status": "ok" }))
                    .with_refs(vec!["tz://blob/deadbeef".into()]),
            ),
            expected_err: None,
        },
        GoldenVector {
            id: "expand_invalid_ref",
            op: "tz_expand",
            tags: &["typed_failure"],
            args: json!({ "ref": "not-a-ref" }),
            expected_ok: None,
            expected_err: Some(
                DomainError::new(DomainErrorKind::InvalidRef, "ref must match ^(tz|fz|gz)://")
                    .with_op("tz_expand"),
            ),
        },
        GoldenVector {
            id: "edit_mutation",
            op: "tz_edit",
            tags: &["success", "mutation"],
            args: json!({
                "path": "src/lib.rs",
                "edits": [{ "find": "foo", "replace": "bar" }]
            }),
            expected_ok: Some(
                DomainResult::new(
                    "tz_edit",
                    json!({ "text": "hunks_applied=1", "status": "ok" }),
                )
                .with_refs(vec!["tz://blob/undo".into()]),
            ),
            expected_err: None,
        },
        GoldenVector {
            id: "edit_hunk_not_found",
            op: "tz_edit",
            tags: &["typed_failure", "mutation"],
            args: json!({
                "path": "src/lib.rs",
                "edits": [{ "find": "missing", "replace": "x" }]
            }),
            expected_ok: None,
            expected_err: Some(
                DomainError::new(DomainErrorKind::HunkNotFound, "hunk find text not present")
                    .with_op("tz_edit"),
            ),
        },
        GoldenVector {
            id: "shell_deadline",
            op: "tz_shell",
            tags: &["typed_failure", "deadline"],
            args: json!({ "command": "sleep 999", "timeout_seconds": 1 }),
            expected_ok: None,
            expected_err: Some(
                DomainError::new(
                    DomainErrorKind::DeadlineExceeded,
                    "shell wall budget exceeded",
                )
                .with_op("tz_shell")
                .with_retryable(true),
            ),
        },
        // The millisecond spelling must reach the same deadline machinery as
        // the seconds spelling. It was previously an unrecognized key, so the
        // command outlived its timeout and reported success.
        GoldenVector {
            id: "shell_deadline_millis",
            op: "tz_shell",
            tags: &["typed_failure", "deadline"],
            args: json!({ "command": "sleep 999", "timeout_ms": 250 }),
            expected_ok: None,
            expected_err: Some(
                DomainError::new(
                    DomainErrorKind::DeadlineExceeded,
                    "shell wall budget exceeded",
                )
                .with_op("tz_shell")
                .with_retryable(true),
            ),
        },
        GoldenVector {
            id: "execute_code_cancelled",
            op: "tz_execute_code",
            tags: &["typed_failure", "cancellation"],
            args: json!({ "plan": "while(true){}", "form": "js" }),
            expected_ok: None,
            expected_err: Some(
                DomainError::new(DomainErrorKind::Cancelled, "plan cancelled by client")
                    .with_op("tz_execute_code")
                    .with_retryable(true),
            ),
        },
        GoldenVector {
            id: "fetch_invalid_url",
            op: "tz_fetch",
            tags: &["typed_failure"],
            args: json!({ "url": "file:///etc/passwd" }),
            expected_ok: None,
            expected_err: Some(
                DomainError::new(DomainErrorKind::InvalidUrl, "only http(s) schemes allowed")
                    .with_op("tz_fetch"),
            ),
        },
        GoldenVector {
            id: "ingest_success",
            op: "tz_ingest",
            tags: &["success", "ref_recovery", "mutation"],
            args: json!({ "text": "external payload" }),
            expected_ok: Some(
                DomainResult::new("tz_ingest", json!({ "text": "…", "status": "ok" }))
                    .with_refs(vec!["tz://blob/ingested".into()]),
            ),
            expected_err: None,
        },
    ]
}
