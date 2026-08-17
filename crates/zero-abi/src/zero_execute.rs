//! Execute result envelope and continuation state machine
//! (ZS-ADAPTER-003, ZS-EXEC-003).
//!
//! `ZeroExecuteResult` is the one stable semantic tool surface result.
//! `abi_version` is pinned to `zerostack.execute`. The six base
//! `kind`s are the schema enum, and `Cancelled`/`FailedNoAuthority` are the
//! two D5 adapter outcomes that extend it.
//!
//! Fail-closed laws:
//! - `completed(...)` accepts only a `Safe` verdict (ZS-KERNEL-004: removing
//!   one required premise can never produce `Completed`).
//! - Every kind-specific constructor requires its mandatory roots or payload
//!   (typed `ZeroExecuteError` on violation).
//! - `Cancelled` and `FailedNoAuthority` must NOT carry `successor_root`.
//! - The continuation machine's `allowed_transition` is a total function that
//!   hard-rejects the D5 forbidden transitions regardless of arguments:
//!   `Unknown -> Authorized`, `Executing -> Committed`, and
//!   `WaitingDecision -> Executing`.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::verdict::SafetyVerdictV1;

pub const ZERO_EXECUTE_ABI_VERSION: &str = "zerostack.execute";

/// The eight result kinds of the execute surface.
///
/// Serialization is PascalCase exactly as the JSON schema spells them:
/// `"Completed"`, `"DecisionRequired"`, `"EvidenceExpansionRequired"`,
/// `"VerificationUnknown"`, `"BaselineFallbackRequired"`,
/// `"RejectedNoMutation"`, plus the D5 adapter extensions `"Cancelled"` and
/// `"FailedNoAuthority"`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub enum ZeroExecuteKind {
    Completed,
    DecisionRequired,
    EvidenceExpansionRequired,
    VerificationUnknown,
    BaselineFallbackRequired,
    RejectedNoMutation,
    Cancelled,
    FailedNoAuthority,
}

impl ZeroExecuteKind {
    /// The PascalCase wire spelling of this kind, exactly as the JSON schema
    /// enumerates the six base kinds.
    pub fn kind_name(self) -> &'static str {
        match self {
            ZeroExecuteKind::Completed => "Completed",
            ZeroExecuteKind::DecisionRequired => "DecisionRequired",
            ZeroExecuteKind::EvidenceExpansionRequired => "EvidenceExpansionRequired",
            ZeroExecuteKind::VerificationUnknown => "VerificationUnknown",
            ZeroExecuteKind::BaselineFallbackRequired => "BaselineFallbackRequired",
            ZeroExecuteKind::RejectedNoMutation => "RejectedNoMutation",
            ZeroExecuteKind::Cancelled => "Cancelled",
            ZeroExecuteKind::FailedNoAuthority => "FailedNoAuthority",
        }
    }

    /// Whether this kind is one of the six enumerated by the canonical
    /// JSON schema (`zero_execute_result.schema.json`). The two D5
    /// adapter outcomes, `Cancelled` and `FailedNoAuthority`, extend it.
    pub fn is_base_schema_kind(self) -> bool {
        matches!(
            self,
            ZeroExecuteKind::Completed
                | ZeroExecuteKind::DecisionRequired
                | ZeroExecuteKind::EvidenceExpansionRequired
                | ZeroExecuteKind::VerificationUnknown
                | ZeroExecuteKind::BaselineFallbackRequired
                | ZeroExecuteKind::RejectedNoMutation
        )
    }
}

/// Inclusive audit event range, schema-required on every envelope.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuditEventRangeV1 {
    pub start: u64,
    pub end: u64,
}

impl AuditEventRangeV1 {
    pub fn new(start: u64, end: u64) -> Result<Self, ZeroExecuteError> {
        let range = Self { start, end };
        range.validate()?;
        Ok(range)
    }

    pub fn validate(&self) -> Result<(), ZeroExecuteError> {
        if self.start > self.end {
            return Err(ZeroExecuteError::InvalidAuditRange {
                start: self.start,
                end: self.end,
            });
        }
        Ok(())
    }
}

/// Optional carry fields shared by every envelope kind. Root strings are
/// canonical content references; `None` means the kind does not carry them.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZeroExecuteFields {
    pub continuation_handle: Option<String>,
    pub project_root: Option<String>,
    pub successor_root: Option<String>,
    pub decision_view_root: Option<String>,
    pub result_root: Option<String>,
    pub exact_delta_root: Option<String>,
    pub verification_receipt_root: Option<String>,
    pub successor_certificate_root: Option<String>,
    pub cache_report_root: Option<String>,
    pub question: Option<String>,
    pub choices: Vec<Value>,
    pub unknown_reasons: Vec<String>,
    pub no_mutation_receipt_root: Option<String>,
}

/// The zero execute result envelope.
///
/// Serialization matches `zero_execute_result.schema.json` field for
/// field. Deserialization is fail-closed: `abi_version` must be the
/// `zerostack.execute` constant, `audit_event_range` must be valid, and the
/// per-kind mandatory fields must be present (enforced by
/// [`ZeroExecuteResult::validate`]).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZeroExecuteResult {
    abi_version: String,
    kind: ZeroExecuteKind,
    #[serde(default)]
    continuation_handle: Option<String>,
    #[serde(default)]
    project_root: Option<String>,
    #[serde(default)]
    successor_root: Option<String>,
    #[serde(default)]
    decision_view_root: Option<String>,
    #[serde(default)]
    result_root: Option<String>,
    #[serde(default)]
    exact_delta_root: Option<String>,
    #[serde(default)]
    verification_receipt_root: Option<String>,
    #[serde(default)]
    successor_certificate_root: Option<String>,
    resource_ledger_root: String,
    #[serde(default)]
    cache_report_root: Option<String>,
    #[serde(default)]
    question: Option<String>,
    #[serde(default)]
    choices: Vec<Value>,
    #[serde(default)]
    unknown_reasons: Vec<String>,
    #[serde(default)]
    no_mutation_receipt_root: Option<String>,
    audit_event_range: AuditEventRangeV1,
}

impl ZeroExecuteResult {
    fn base(
        kind: ZeroExecuteKind,
        fields: ZeroExecuteFields,
        resource_ledger_root: impl Into<String>,
        audit_event_range: AuditEventRangeV1,
    ) -> Self {
        Self {
            abi_version: ZERO_EXECUTE_ABI_VERSION.to_owned(),
            kind,
            continuation_handle: fields.continuation_handle,
            project_root: fields.project_root,
            successor_root: fields.successor_root,
            decision_view_root: fields.decision_view_root,
            result_root: fields.result_root,
            exact_delta_root: fields.exact_delta_root,
            verification_receipt_root: fields.verification_receipt_root,
            successor_certificate_root: fields.successor_certificate_root,
            resource_ledger_root: resource_ledger_root.into(),
            cache_report_root: fields.cache_report_root,
            question: fields.question,
            choices: fields.choices,
            unknown_reasons: fields.unknown_reasons,
            no_mutation_receipt_root: fields.no_mutation_receipt_root,
            audit_event_range,
        }
    }

    /// Construct a `Completed` envelope. ZS-KERNEL-004 acceptance: this is
    /// the ONLY path that yields `Completed`, and it requires a `Safe`
    /// verdict. An `Unknown` or `Unsafe` verdict is rejected with
    /// [`ZeroExecuteError::VerdictNotSafe`] -- removing a required premise
    /// can never produce `Completed`.
    pub fn completed(
        verdict: SafetyVerdictV1,
        fields: ZeroExecuteFields,
        resource_ledger_root: impl Into<String>,
        audit_event_range: AuditEventRangeV1,
    ) -> Result<Self, ZeroExecuteError> {
        if verdict != SafetyVerdictV1::Safe {
            return Err(ZeroExecuteError::VerdictNotSafe);
        }
        if fields.successor_root.is_none() {
            return Err(ZeroExecuteError::MissingRequiredField("successor_root"));
        }
        if fields.result_root.is_none() {
            return Err(ZeroExecuteError::MissingRequiredField("result_root"));
        }
        if fields.verification_receipt_root.is_none() {
            return Err(ZeroExecuteError::MissingRequiredField(
                "verification_receipt_root",
            ));
        }
        let result = Self::base(
            ZeroExecuteKind::Completed,
            fields,
            resource_ledger_root,
            audit_event_range,
        );
        result.validate()?;
        Ok(result)
    }

    /// Construct a `DecisionRequired` envelope: a question, a nonempty choice
    /// set, and a continuation handle are mandatory.
    pub fn decision_required(
        fields: ZeroExecuteFields,
        resource_ledger_root: impl Into<String>,
        audit_event_range: AuditEventRangeV1,
    ) -> Result<Self, ZeroExecuteError> {
        if fields.question.is_none() {
            return Err(ZeroExecuteError::MissingRequiredField("question"));
        }
        if fields.choices.is_empty() {
            return Err(ZeroExecuteError::EmptyChoices);
        }
        if fields.continuation_handle.is_none() {
            return Err(ZeroExecuteError::MissingRequiredField(
                "continuation_handle",
            ));
        }
        let result = Self::base(
            ZeroExecuteKind::DecisionRequired,
            fields,
            resource_ledger_root,
            audit_event_range,
        );
        result.validate()?;
        Ok(result)
    }

    /// Construct an `EvidenceExpansionRequired` envelope: a continuation
    /// handle is mandatory.
    pub fn evidence_expansion_required(
        fields: ZeroExecuteFields,
        resource_ledger_root: impl Into<String>,
        audit_event_range: AuditEventRangeV1,
    ) -> Result<Self, ZeroExecuteError> {
        if fields.continuation_handle.is_none() {
            return Err(ZeroExecuteError::MissingRequiredField(
                "continuation_handle",
            ));
        }
        let result = Self::base(
            ZeroExecuteKind::EvidenceExpansionRequired,
            fields,
            resource_ledger_root,
            audit_event_range,
        );
        result.validate()?;
        Ok(result)
    }

    /// Construct a `VerificationUnknown` envelope: nonempty unknown reasons
    /// are mandatory (the reasons are what the fallback decision records).
    pub fn verification_unknown(
        fields: ZeroExecuteFields,
        resource_ledger_root: impl Into<String>,
        audit_event_range: AuditEventRangeV1,
    ) -> Result<Self, ZeroExecuteError> {
        if fields.unknown_reasons.is_empty() {
            return Err(ZeroExecuteError::EmptyUnknownReasons);
        }
        let result = Self::base(
            ZeroExecuteKind::VerificationUnknown,
            fields,
            resource_ledger_root,
            audit_event_range,
        );
        result.validate()?;
        Ok(result)
    }

    /// Construct a `BaselineFallbackRequired` envelope: nonempty unknown
    /// reasons are mandatory.
    pub fn baseline_fallback_required(
        fields: ZeroExecuteFields,
        resource_ledger_root: impl Into<String>,
        audit_event_range: AuditEventRangeV1,
    ) -> Result<Self, ZeroExecuteError> {
        if fields.unknown_reasons.is_empty() {
            return Err(ZeroExecuteError::EmptyUnknownReasons);
        }
        let result = Self::base(
            ZeroExecuteKind::BaselineFallbackRequired,
            fields,
            resource_ledger_root,
            audit_event_range,
        );
        result.validate()?;
        Ok(result)
    }

    /// Construct a `RejectedNoMutation` envelope: a no-mutation receipt root
    /// is mandatory.
    pub fn rejected_no_mutation(
        fields: ZeroExecuteFields,
        resource_ledger_root: impl Into<String>,
        audit_event_range: AuditEventRangeV1,
    ) -> Result<Self, ZeroExecuteError> {
        if fields.no_mutation_receipt_root.is_none() {
            return Err(ZeroExecuteError::MissingRequiredField(
                "no_mutation_receipt_root",
            ));
        }
        let result = Self::base(
            ZeroExecuteKind::RejectedNoMutation,
            fields,
            resource_ledger_root,
            audit_event_range,
        );
        result.validate()?;
        Ok(result)
    }

    /// Construct a `Cancelled` envelope (D5 adapter extension). No extra
    /// roots are required, and `successor_root` is rejected if present.
    pub fn cancelled(
        fields: ZeroExecuteFields,
        resource_ledger_root: impl Into<String>,
        audit_event_range: AuditEventRangeV1,
    ) -> Result<Self, ZeroExecuteError> {
        if fields.successor_root.is_some() {
            return Err(ZeroExecuteError::ForbiddenRoot("successor_root"));
        }
        let result = Self::base(
            ZeroExecuteKind::Cancelled,
            fields,
            resource_ledger_root,
            audit_event_range,
        );
        result.validate()?;
        Ok(result)
    }

    /// Construct a `FailedNoAuthority` envelope (D5 adapter extension). No
    /// extra roots are required, and `successor_root` is rejected if present.
    pub fn failed_no_authority(
        fields: ZeroExecuteFields,
        resource_ledger_root: impl Into<String>,
        audit_event_range: AuditEventRangeV1,
    ) -> Result<Self, ZeroExecuteError> {
        if fields.successor_root.is_some() {
            return Err(ZeroExecuteError::ForbiddenRoot("successor_root"));
        }
        let result = Self::base(
            ZeroExecuteKind::FailedNoAuthority,
            fields,
            resource_ledger_root,
            audit_event_range,
        );
        result.validate()?;
        Ok(result)
    }

    /// The fail-closed kind for a non-`Safe` verdict, used by the
    /// no-completion constructor path. `Safe` maps to `Completed` here only
    /// as the label of the completing kind -- never as permission to skip
    /// [`ZeroExecuteResult::completed`].
    pub fn kind_for_verdict(verdict: &SafetyVerdictV1) -> ZeroExecuteKind {
        match verdict {
            SafetyVerdictV1::Safe => ZeroExecuteKind::Completed,
            SafetyVerdictV1::Unsafe { .. } => ZeroExecuteKind::RejectedNoMutation,
            SafetyVerdictV1::Unknown { .. } => ZeroExecuteKind::VerificationUnknown,
        }
    }

    /// Construct an envelope for an `Unsafe` or `Unknown` verdict without any
    /// completion path. `Unsafe` maps to `RejectedNoMutation` (no-mutation
    /// receipt required), `Unknown` maps to `VerificationUnknown` (reasons
    /// required). A `Safe` verdict is rejected here: the only way to produce
    /// `Completed` is [`ZeroExecuteResult::completed`], which re-checks the
    /// verdict. This constructor therefore has no reachable `Completed` path.
    pub fn from_verdict_never_completed(
        verdict: &SafetyVerdictV1,
        fields: ZeroExecuteFields,
        resource_ledger_root: impl Into<String>,
        audit_event_range: AuditEventRangeV1,
    ) -> Result<Self, ZeroExecuteError> {
        match verdict {
            SafetyVerdictV1::Safe => Err(ZeroExecuteError::VerdictMustNotBeSafe),
            SafetyVerdictV1::Unsafe { .. } => {
                Self::rejected_no_mutation(fields, resource_ledger_root, audit_event_range)
            }
            SafetyVerdictV1::Unknown { .. } => {
                Self::verification_unknown(fields, resource_ledger_root, audit_event_range)
            }
        }
    }

    /// Per-kind validation of a fully constructed (or deserialized) envelope.
    /// Enforces the schema contract: correct `abi_version`, valid audit
    /// range, and the kind-specific mandatory fields.
    pub fn validate(&self) -> Result<(), ZeroExecuteError> {
        if self.abi_version != ZERO_EXECUTE_ABI_VERSION {
            return Err(ZeroExecuteError::WrongAbiVersion {
                actual: self.abi_version.clone(),
            });
        }
        self.audit_event_range.validate()?;
        match self.kind {
            ZeroExecuteKind::Completed => {
                if self.successor_root.is_none() {
                    return Err(ZeroExecuteError::MissingRequiredField("successor_root"));
                }
                if self.result_root.is_none() {
                    return Err(ZeroExecuteError::MissingRequiredField("result_root"));
                }
                if self.verification_receipt_root.is_none() {
                    return Err(ZeroExecuteError::MissingRequiredField(
                        "verification_receipt_root",
                    ));
                }
            }
            ZeroExecuteKind::DecisionRequired => {
                if self.question.is_none() {
                    return Err(ZeroExecuteError::MissingRequiredField("question"));
                }
                if self.choices.is_empty() {
                    return Err(ZeroExecuteError::EmptyChoices);
                }
                if self.continuation_handle.is_none() {
                    return Err(ZeroExecuteError::MissingRequiredField(
                        "continuation_handle",
                    ));
                }
            }
            ZeroExecuteKind::EvidenceExpansionRequired => {
                if self.continuation_handle.is_none() {
                    return Err(ZeroExecuteError::MissingRequiredField(
                        "continuation_handle",
                    ));
                }
            }
            ZeroExecuteKind::VerificationUnknown
            | ZeroExecuteKind::BaselineFallbackRequired => {
                if self.unknown_reasons.is_empty() {
                    return Err(ZeroExecuteError::EmptyUnknownReasons);
                }
            }
            ZeroExecuteKind::RejectedNoMutation => {
                if self.no_mutation_receipt_root.is_none() {
                    return Err(ZeroExecuteError::MissingRequiredField(
                        "no_mutation_receipt_root",
                    ));
                }
            }
            ZeroExecuteKind::Cancelled | ZeroExecuteKind::FailedNoAuthority => {
                if self.successor_root.is_some() {
                    return Err(ZeroExecuteError::ForbiddenRoot("successor_root"));
                }
            }
        }
        Ok(())
    }

    pub fn abi_version(&self) -> &str {
        &self.abi_version
    }

    pub fn kind(&self) -> ZeroExecuteKind {
        self.kind
    }

    pub fn continuation_handle(&self) -> Option<&str> {
        self.continuation_handle.as_deref()
    }

    pub fn project_root(&self) -> Option<&str> {
        self.project_root.as_deref()
    }

    pub fn successor_root(&self) -> Option<&str> {
        self.successor_root.as_deref()
    }

    pub fn decision_view_root(&self) -> Option<&str> {
        self.decision_view_root.as_deref()
    }

    pub fn result_root(&self) -> Option<&str> {
        self.result_root.as_deref()
    }

    pub fn exact_delta_root(&self) -> Option<&str> {
        self.exact_delta_root.as_deref()
    }

    pub fn verification_receipt_root(&self) -> Option<&str> {
        self.verification_receipt_root.as_deref()
    }

    pub fn successor_certificate_root(&self) -> Option<&str> {
        self.successor_certificate_root.as_deref()
    }

    pub fn resource_ledger_root(&self) -> &str {
        &self.resource_ledger_root
    }

    pub fn cache_report_root(&self) -> Option<&str> {
        self.cache_report_root.as_deref()
    }

    pub fn question(&self) -> Option<&str> {
        self.question.as_deref()
    }

    pub fn choices(&self) -> &[Value] {
        &self.choices
    }

    pub fn unknown_reasons(&self) -> &[String] {
        &self.unknown_reasons
    }

    pub fn no_mutation_receipt_root(&self) -> Option<&str> {
        self.no_mutation_receipt_root.as_deref()
    }

    pub fn audit_event_range(&self) -> AuditEventRangeV1 {
        self.audit_event_range
    }
}

/// Fail-closed construction and validation error for the execute envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ZeroExecuteError {
    InvalidAuditRange { start: u64, end: u64 },
    WrongAbiVersion { actual: String },
    VerdictNotSafe,
    VerdictMustNotBeSafe,
    MissingRequiredField(&'static str),
    ForbiddenRoot(&'static str),
    EmptyChoices,
    EmptyUnknownReasons,
    /// A decision-bearing envelope requested a decision view but the typed
    /// view failed fail-closed construction or certification (ZS-VIEW-010).
    InvalidDecisionView(String),
}

impl fmt::Display for ZeroExecuteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAuditRange { start, end } => {
                write!(formatter, "audit range start {start} exceeds end {end}")
            }
            Self::WrongAbiVersion { actual } => {
                write!(formatter, "abi_version must be {ZERO_EXECUTE_ABI_VERSION}, got {actual}")
            }
            Self::VerdictNotSafe => {
                write!(formatter, "Completed requires a Safe verdict; Unknown/Unsafe must not complete")
            }
            Self::VerdictMustNotBeSafe => {
                write!(formatter, "no-completion constructor rejects a Safe verdict by design")
            }
            Self::MissingRequiredField(field) => {
                write!(formatter, "kind requires field {field}")
            }
            Self::ForbiddenRoot(root) => {
                write!(formatter, "kind must not carry {root}")
            }
            Self::EmptyChoices => write!(formatter, "DecisionRequired requires nonempty choices"),
            Self::EmptyUnknownReasons => {
                write!(formatter, "Unknown/fallback kinds require nonempty unknown_reasons")
            }
            Self::InvalidDecisionView(detail) => {
                write!(formatter, "decision view construction failed: {detail}")
            }
        }
    }
}

impl Error for ZeroExecuteError {}

/// The 14-state D5 continuation machine.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationStateV1 {
    Bound,
    Snapshotted,
    Resolved,
    WaitingDecision,
    Planned,
    Executing,
    DeltaSealed,
    Verifying,
    Authorized,
    Committed,
    Restored,
    Rejected,
    Unknown,
    Cancelled,
}

impl ContinuationStateV1 {
    /// Total allowed-transition predicate for the D5 continuation machine.
    ///
    /// Legal forward chain:
    /// `Bound -> Snapshotted -> Resolved -> Planned -> Executing ->
    /// DeltaSealed -> Verifying -> {Authorized, Rejected, Unknown}` and
    /// `Authorized -> Committed`. `Resolved` may also branch to
    /// `WaitingDecision`; `WaitingDecision -> Planned` is allowed ONLY when a
    /// contingent policy is supplied (`policy_supplied == true`). Any
    /// non-terminal state may cancel. `Unknown` may only restore from the
    /// frozen raw baseline (`Unknown -> Restored`) or cancel.
    ///
    /// Forbidden regardless of arguments (D5): `Unknown -> Authorized`,
    /// `Executing -> Committed`, `WaitingDecision -> Executing`. Terminal
    /// states (`Committed`, `Restored`, `Rejected`, `Cancelled`) have no
    /// outgoing edges.
    pub fn allowed_transition(
        from: ContinuationStateV1,
        to: ContinuationStateV1,
        policy_supplied: bool,
    ) -> bool {
        use ContinuationStateV1::{
            Authorized, Bound, Cancelled, Committed, DeltaSealed, Executing, Planned, Rejected,
            Resolved, Restored, Snapshotted, Unknown, Verifying, WaitingDecision,
        };
        let direct = match (from, to) {
            (Bound, Snapshotted)
            | (Snapshotted, Resolved)
            | (Resolved, Planned)
            | (Resolved, WaitingDecision)
            | (Planned, Executing)
            | (Executing, DeltaSealed)
            | (DeltaSealed, Verifying)
            | (Verifying, Authorized)
            | (Verifying, Rejected)
            | (Verifying, Unknown)
            | (Authorized, Committed)
            | (Unknown, Restored) => true,
            (WaitingDecision, Planned) => policy_supplied,
            _ => false,
        };
        if direct {
            return true;
        }
        // Cancellation escape from any non-terminal state, including
        // WaitingDecision and Unknown (Unknown -> Cancelled is explicit in
        // the direct set and covered here as well).
        to == Cancelled && !Self::is_terminal(from)
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            ContinuationStateV1::Committed
                | ContinuationStateV1::Restored
                | ContinuationStateV1::Rejected
                | ContinuationStateV1::Cancelled
        )
    }
}

