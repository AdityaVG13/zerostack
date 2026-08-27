//! Typed operation ABI concepts shared by FastMCP, CodeMode, and private worker.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Semantic contract version for the GraphZero operation ABI.
///
/// Bump MAJOR for breaking name/schema/error changes, MINOR for additive ops,
/// PATCH for description/docs-only. Digest changes whenever registry content
/// that participates in the digest changes.
pub const SEMANTIC_CONTRACT_VERSION: &str = "1.0.0";

/// Stable identifier for a canonical operation (string form is `Operation::name`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OperationId(pub &'static str);

/// Whether the operation mutates GraphZero store state (never repo files).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mutability {
    /// No durable side effects.
    ReadOnly,
    /// May write GraphZero store (memory, reservations, index shards) only.
    StoreOnly,
}

/// Relative cost class for budgeting and fusion decisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostClass {
    Cheap,
    Medium,
    Heavy,
}

/// Who may invoke the operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityRequirement {
    /// Any authenticated local client (default product surface).
    Public,
    /// Private raw worker / hub composition only (graphzero-o2uq.4).
    PrivateWorker,
}

/// How refs produced by the operation are owned for RACC recovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefOwnership {
    None,
    Query,
    Blob,
    Mem,
    /// Multiple ref kinds (e.g. expand recovery, mixed capsules).
    Multi,
    Execution,
}

/// Cancellation contract for in-flight work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationSemantics {
    /// Operation ignores cancel (completes or fails on its own).
    None,
    /// Cooperative: best-effort stop; may return `cancelled` or finish.
    Cooperative,
    /// Hard deadline: must surface `deadline_exceeded` when wall budget elapses.
    Deadline,
}

/// Migration posture for aliases and legacy spellings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationStatus {
    /// Canonical name; preferred in new callers.
    Canonical,
    /// Accepted alias; prefer canonical; no removal without evidence.
    LegacyAlias,
    /// Orient sub-surface routed through `orient` / `query`, not a top-level tool.
    OrientSubSurface,
}

/// Where the operation appears on user-facing catalogs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceExposure {
    /// Listed as a lean FastMCP tool (`--mode=mcp`).
    pub fastmcp_tool: bool,
    /// CodeMode binding path (`graph.blast`, `ctx.ref`, …), if any.
    pub codemode_binding: Option<&'static str>,
    /// CodeMode meta tools (`gz_execute_code`, …) — not domain dispatch targets.
    pub codemode_meta: bool,
}

/// JSON Schema fragment for operation arguments (always `type: object`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperationArgs {
    pub schema: Value,
}

/// JSON Schema fragment for the normalized domain success/error envelope.
///
/// Owned by the registry so FastMCP `outputSchema` and CodeMode describe
/// metadata cannot drift from the ABI (graphzero-o2uq.1 review fix).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperationResults {
    pub schema: Value,
}

/// Typed domain error kind (surface-independent).
///
/// Harnesses must branch on `kind` + `retryable`, not display strings alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainErrorKind {
    Validation,
    Policy,
    Sandbox,
    Runtime,
    Substrate,
    Busy,
    Approval,
    Cancelled,
    DeadlineExceeded,
    NotFound,
    Unauthorized,
}

impl DomainErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "validation",
            Self::Policy => "policy",
            Self::Sandbox => "sandbox",
            Self::Runtime => "runtime",
            Self::Substrate => "substrate",
            Self::Busy => "busy",
            Self::Approval => "approval",
            Self::Cancelled => "cancelled",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::NotFound => "not_found",
            Self::Unauthorized => "unauthorized",
        }
    }

    pub fn default_retryable(self) -> bool {
        matches!(
            self,
            Self::Busy | Self::Approval | Self::DeadlineExceeded | Self::Cancelled
        )
    }
}

/// Transport-neutral domain error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainError {
    pub kind: DomainErrorKind,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_ref: Option<String>,
}

impl DomainError {
    pub fn new(kind: DomainErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retryable: kind.default_retryable(),
            op: None,
            recovery_ref: None,
        }
    }

    pub fn with_op(mut self, op: impl Into<String>) -> Self {
        self.op = Some(op.into());
        self
    }

    pub fn with_recovery_ref(mut self, r: impl Into<String>) -> Self {
        self.recovery_ref = Some(r.into());
        self
    }

    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }
}

/// Transport-neutral successful domain result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DomainResult {
    /// Primary value (may be compact ack + refs under budget).
    pub value: Value,
    /// Durable refs emitted for RACC recovery.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
    /// Operation that produced this result.
    pub op: String,
    /// Optional structured telemetry (wall_ms, logical_ops, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<Value>,
}

impl DomainResult {
    pub fn new(op: impl Into<String>, value: Value) -> Self {
        Self {
            value,
            refs: Vec::new(),
            op: op.into(),
            telemetry: None,
        }
    }

    pub fn with_refs(mut self, refs: Vec<String>) -> Self {
        self.refs = refs;
        self
    }

    pub fn with_telemetry(mut self, telemetry: Value) -> Self {
        self.telemetry = Some(telemetry);
        self
    }

    /// Mirror the first durable engine ref into the model-visible value.
    /// Transport metadata remains authoritative; this alias only makes the
    /// advertised `result.ref` field usable without parsing an outer envelope.
    pub fn expose_primary_ref(mut self) -> Self {
        let Some(primary) = self.refs.first().cloned() else {
            return self;
        };
        match &mut self.value {
            Value::Object(object) => {
                object.entry("ref").or_insert(Value::String(primary));
            }
            value => {
                let payload = std::mem::take(value);
                *value = serde_json::json!({
                    "ack": "C",
                    "ref": primary,
                    "value": payload,
                });
            }
        }
        self
    }
}

/// One canonical operation in the registry.
#[derive(Clone, Debug)]
pub struct Operation {
    /// Canonical short name (e.g. `blast`, `orient`).
    pub name: &'static str,
    /// Human description (also used for FastMCP tool text when exposed).
    pub description: &'static str,
    /// Accepted alternate spellings / legacy names.
    pub aliases: &'static [&'static str],
    pub mutability: Mutability,
    pub capability: CapabilityRequirement,
    pub cost_class: CostClass,
    pub ref_ownership: RefOwnership,
    pub cancellation: CancellationSemantics,
    pub migration: MigrationStatus,
    pub exposure: SurfaceExposure,
    pub args: OperationArgs,
    /// Complete output schema for the normalized domain result envelope.
    pub results: OperationResults,
    /// Documented domain error kinds this op may return.
    pub error_kinds: &'static [DomainErrorKind],
    /// When true, op is an orient router target (`surface` param), not only top-level.
    pub is_orient_router: bool,
}
