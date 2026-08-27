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

/// Contract-focused golden vectors. Full surface parity is graphzero-o2uq.7.
pub fn golden_vectors() -> Vec<GoldenVector> {
    vec![
        GoldenVector {
            id: "blast_success_shape",
            op: "blast",
            tags: &["success", "ref_recovery"],
            args: json!({ "intent": "rename alpha", "budget": 1 }),
            expected_ok: Some(
                DomainResult::new(
                    "blast",
                    json!({
                        "ack": "ok",
                        "surface": "blast",
                    }),
                )
                .with_refs(vec!["gz://query/example".into()]),
            ),
            expected_err: None,
        },
        GoldenVector {
            id: "snap_validation_missing_query",
            op: "snap",
            tags: &["typed_failure"],
            args: json!({}),
            expected_ok: None,
            expected_err: Some(
                DomainError::new(DomainErrorKind::Validation, "snap requires query or symbol")
                    .with_op("snap"),
            ),
        },
        GoldenVector {
            id: "expand_ref_recovery",
            op: "expand",
            tags: &["success", "ref_recovery"],
            args: json!({ "reference": "gz://blob/deadbeef" }),
            expected_ok: Some(
                DomainResult::new(
                    "expand",
                    json!({
                        "ack": "ok",
                        "truncated": true,
                    }),
                )
                .with_refs(vec!["gz://blob/deadbeef".into()]),
            ),
            expected_err: None,
        },
        GoldenVector {
            id: "remember_mutation",
            op: "remember",
            tags: &["success", "mutation"],
            args: json!({ "text": "prefer ref-first snaps", "kind": "decision" }),
            expected_ok: Some(
                DomainResult::new(
                    "remember",
                    json!({
                        "ack": "ok",
                        "mutability": "store_only",
                    }),
                )
                .with_refs(vec!["gz://mem/example".into()]),
            ),
            expected_err: None,
        },
        GoldenVector {
            id: "index_mutation_store_only",
            op: "index",
            tags: &["mutation"],
            args: json!({ "path": "." }),
            expected_ok: Some(DomainResult::new(
                "index",
                json!({ "ack": "ok", "mutability": "store_only" }),
            )),
            expected_err: None,
        },
        GoldenVector {
            id: "execute_code_deadline",
            op: "execute_code",
            tags: &["typed_failure", "deadline"],
            args: json!({ "plan": "while(true){}", "form": "js" }),
            expected_ok: None,
            expected_err: Some(
                DomainError::new(DomainErrorKind::DeadlineExceeded, "max_wall_ms exceeded")
                    .with_op("execute_code")
                    .with_retryable(true),
            ),
        },
        GoldenVector {
            id: "execute_code_cancelled",
            op: "execute_code",
            tags: &["typed_failure", "cancellation"],
            args: json!({ "plan": "callers:alpha", "form": "recipe" }),
            expected_ok: None,
            expected_err: Some(
                DomainError::new(DomainErrorKind::Cancelled, "client cancelled")
                    .with_op("execute_code")
                    .with_retryable(true),
            ),
        },
        GoldenVector {
            id: "policy_deny_repo_mutation",
            op: "remember",
            tags: &["typed_failure", "mutation"],
            args: json!({ "text": "x", "repo": "/outside/root" }),
            expected_ok: None,
            expected_err: Some(
                DomainError::new(DomainErrorKind::Policy, "root policy denied")
                    .with_op("remember")
                    .with_retryable(false),
            ),
        },
        GoldenVector {
            id: "busy_analysis_permit",
            op: "blast",
            tags: &["typed_failure"],
            args: json!({ "intent": "hot path" }),
            expected_ok: None,
            expected_err: Some(
                DomainError::new(DomainErrorKind::Busy, "analysis permit busy")
                    .with_op("blast")
                    .with_retryable(true),
            ),
        },
        GoldenVector {
            id: "expand_missing_ref",
            op: "expand",
            tags: &["typed_failure", "ref_recovery"],
            args: json!({ "reference": "gz://blob/0000000000000000000000000000000000000000000000000000000000000000" }),
            expected_ok: None,
            expected_err: Some(
                DomainError::new(DomainErrorKind::NotFound, "ref not found")
                    .with_op("expand")
                    .with_retryable(false),
            ),
        },
    ]
}
