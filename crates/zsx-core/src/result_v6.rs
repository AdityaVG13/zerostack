//! V6-R1: session outcome projection onto the V6 zero execute result
//! envelope (ZS-ADAPTER-003, ZS-EXEC-003).
//!
//! The session execute path now emits the 8-kind [`ZeroExecuteResultV6`]
//! envelope alongside the legacy result. The projection obeys one honesty
//! law: **a kind is emitted only when the session can prove that outcome.**
//! The session holds no safety verdict and no content roots for a plain
//! execution, so it never claims `Completed` (which requires a `Safe`
//! verdict plus successor/result/verification receipt roots), and it never
//! launders a transport/lifecycle failure into a semantic kind.
//!
//! Provable kinds at the session boundary:
//!
//! | Outcome | Envelope kind | Proof |
//! |---|---|---|
//! | Uncovered decision point aborts the plan | `DecisionRequired` | typed [`DecisionRequiredV1`] payload: question, choices, decision id (bound as the continuation handle) |
//! | Request cancelled through its token | `Cancelled` | `cancel_request` recorded the request in the cancellation slot |
//! | Approval/permit admission rejection | `FailedNoAuthority` | typed approval validation failure (`InvalidApproval`/`ApprovalReplay`) |
//! | Plain success, other failures | no envelope | no V6 kind is provable (no verdict/roots; lifecycle/transport failure) |
//!
//! `resource_ledger_root` and `audit_event_range` are mandatory envelope
//! fields the session cannot derive, so the harness supplies them through
//! [`SessionEnvelopeContextV1`]; the session never fabricates a root.
//!
//! The explicit legacy conversion [`legacy_envelope_value`] renders the
//! envelope in the `zerostack.zsx.v1` shape (`{protocol, ok, generation,
//! request_id, result?, error?}`) so existing consumers keep working with
//! unchanged failure codes (`decision_required`, `cancelled`, ...) while the
//! V6 envelope carries the semantic payload.

use serde_json::{Value, json};
use zero_abi::{
    AuditEventRangeV1, DecisionRequiredV1, ZeroExecuteErrorV6, ZeroExecuteFieldsV6,
    ZeroExecuteKindV6, ZeroExecuteResultV6,
};

/// Protocol label of the legacy zsx envelope shape the conversion emits,
/// identical to the label existing consumers already resolve.
pub const SESSION_V6_ENVELOPE_LEGACY_PROTOCOL: &str = "zerostack.zsx.v1";

/// The envelope fields the session cannot prove by itself, supplied by the
/// harness. Fail-closed construction: an empty ledger root or an invalid
/// audit range is rejected, so an envelope can never carry a fabricated
/// anchor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionEnvelopeContextV1 {
    /// Root string of the session resource ledger (e.g. the archive root
    /// hex of a finalized dominance receipt). Must be nonempty.
    pub resource_ledger_root: String,
    /// Inclusive audit event range the envelope must carry.
    pub audit_event_range: AuditEventRangeV1,
}

impl SessionEnvelopeContextV1 {
    /// Fail-closed construction: validates both fields immediately.
    pub fn new(
        resource_ledger_root: impl Into<String>,
        audit_event_range: AuditEventRangeV1,
    ) -> Result<Self, String> {
        let context = Self {
            resource_ledger_root: resource_ledger_root.into(),
            audit_event_range,
        };
        context.validate()?;
        Ok(context)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.resource_ledger_root.is_empty() {
            return Err(
                "resource_ledger_root must be nonempty: the session never fabricates a ledger anchor"
                    .into(),
            );
        }
        self.audit_event_range
            .validate()
            .map_err(|error| error.to_string())
    }
}

fn with_project_root(project_root: Option<&str>) -> ZeroExecuteFieldsV6 {
    ZeroExecuteFieldsV6 {
        project_root: project_root.map(str::to_owned),
        ..Default::default()
    }
}

/// `DecisionRequired` envelope for an uncovered semantic decision point
/// (ZS-EXEC-003). The typed payload's question and choices become the
/// envelope's question/choices; its decision id is bound as the
/// continuation handle scoped to this execution, so a future resume API
/// (ZS-ADAPTER-004) has an opaque anchor. The zero-abi constructor
/// re-validates: question, nonempty choices, and continuation handle are
/// mandatory for this kind.
pub fn decision_required(
    payload: &DecisionRequiredV1,
    generation: u64,
    request_id: u64,
    project_root: Option<&str>,
    ledger: &SessionEnvelopeContextV1,
) -> Result<ZeroExecuteResultV6, ZeroExecuteErrorV6> {
    let fields = ZeroExecuteFieldsV6 {
        continuation_handle: Some(format!(
            "zsx://g{generation}-r{request_id}/{}",
            payload.decision_id
        )),
        question: Some(payload.question.clone()),
        choices: payload.choices.iter().cloned().map(Value::String).collect(),
        ..with_project_root(project_root)
    };
    ZeroExecuteResultV6::decision_required(
        fields,
        ledger.resource_ledger_root.clone(),
        ledger.audit_event_range,
    )
}

/// `Cancelled` envelope for a request cancelled through its token. No
/// mandatory roots; the zero-abi constructor also rejects a `successor_root`
/// for this kind, keeping the fail-closed adapter outcome shape.
pub fn cancelled(
    project_root: Option<&str>,
    ledger: &SessionEnvelopeContextV1,
) -> Result<ZeroExecuteResultV6, ZeroExecuteErrorV6> {
    ZeroExecuteResultV6::cancelled(
        with_project_root(project_root),
        ledger.resource_ledger_root.clone(),
        ledger.audit_event_range,
    )
}

/// `FailedNoAuthority` envelope for an approval/permit admission rejection.
/// The D5 adapter extension carries no mandatory roots; the legacy error
/// keeps the detailed rejection message.
pub fn failed_no_authority(
    project_root: Option<&str>,
    ledger: &SessionEnvelopeContextV1,
) -> Result<ZeroExecuteResultV6, ZeroExecuteErrorV6> {
    ZeroExecuteResultV6::failed_no_authority(
        with_project_root(project_root),
        ledger.resource_ledger_root.clone(),
        ledger.audit_event_range,
    )
}

/// Legacy failure code for a V6 kind, in the snake_case vocabulary existing
/// consumers already switch on (`cancelled`, `decision_required`, ...).
pub fn legacy_kind_code(kind: ZeroExecuteKindV6) -> &'static str {
    match kind {
        ZeroExecuteKindV6::Completed => "completed",
        ZeroExecuteKindV6::DecisionRequired => "decision_required",
        ZeroExecuteKindV6::EvidenceExpansionRequired => "evidence_expansion_required",
        ZeroExecuteKindV6::VerificationUnknown => "verification_unknown",
        ZeroExecuteKindV6::BaselineFallbackRequired => "baseline_fallback_required",
        ZeroExecuteKindV6::RejectedNoMutation => "rejected_no_mutation",
        ZeroExecuteKindV6::Cancelled => "cancelled",
        ZeroExecuteKindV6::FailedNoAuthority => "failed_no_authority",
    }
}

/// Human-readable detail line for the legacy error object of a non-`Completed`
/// kind. Kept stable and message-free of user content (the V6 envelope
/// carries roots, not prose).
fn legacy_kind_detail(envelope: &ZeroExecuteResultV6) -> String {
    match envelope.kind() {
        ZeroExecuteKindV6::Completed => "completed".into(),
        ZeroExecuteKindV6::DecisionRequired => format!(
            "decision required: {}",
            envelope.question().unwrap_or("uncovered semantic decision point")
        ),
        ZeroExecuteKindV6::EvidenceExpansionRequired => {
            "evidence expansion required".into()
        }
        ZeroExecuteKindV6::VerificationUnknown => "verification unknown".into(),
        ZeroExecuteKindV6::BaselineFallbackRequired => {
            "baseline fallback required".into()
        }
        ZeroExecuteKindV6::RejectedNoMutation => "rejected: no mutation".into(),
        ZeroExecuteKindV6::Cancelled => "request cancelled".into(),
        ZeroExecuteKindV6::FailedNoAuthority => "failed: no authority".into(),
    }
}

/// Explicit legacy conversion: render a V6 envelope in the
/// `zerostack.zsx.v1` shape existing consumers already resolve.
///
/// `ok` is true only for `Completed`; a `DecisionRequired` envelope carries
/// its question/choices/continuation handle in `result` (mirroring the
/// legacy `commit_race` shape of error plus result) and a snake_case error
/// code matching the legacy failure-code vocabulary.
pub fn legacy_envelope_value(
    envelope: &ZeroExecuteResultV6,
    generation: u64,
    request_id: u64,
) -> Value {
    let kind = envelope.kind();
    let ok = kind == ZeroExecuteKindV6::Completed;
    let mut result = serde_json::Map::new();
    result.insert("protocol".into(), json!(SESSION_V6_ENVELOPE_LEGACY_PROTOCOL));
    result.insert("ok".into(), json!(ok));
    result.insert("generation".into(), json!(generation));
    result.insert("request_id".into(), json!(request_id));
    if kind == ZeroExecuteKindV6::DecisionRequired {
        let mut decision = serde_json::Map::new();
        decision.insert("question".into(), json!(envelope.question()));
        decision.insert("choices".into(), json!(envelope.choices()));
        decision.insert(
            "continuation_handle".into(),
            json!(envelope.continuation_handle()),
        );
        result.insert("result".into(), Value::Object(decision));
    }
    if !ok {
        let mut error = serde_json::Map::new();
        error.insert("code".into(), json!(legacy_kind_code(kind)));
        error.insert("detail".into(), json!(legacy_kind_detail(envelope)));
        result.insert("error".into(), Value::Object(error));
    }
    Value::Object(result)
}

#[cfg(test)]
#[path = "../../../tests/rust/zsx-core/unit/result_v6.rs"]
mod tests;
