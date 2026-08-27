//! Typed operation ABI concepts shared by FastMCP, CodeMode, and private worker.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Semantic contract version for the TokenZero operation ABI (tokenzero-irx9.1).
///
/// Bump MAJOR for breaking name/schema/error changes, MINOR for additive ops,
/// PATCH for description/docs-only. Digest changes whenever registry content
/// that participates in the digest changes.
pub const SEMANTIC_CONTRACT_VERSION: &str = "1.0.0";

/// Default shell timeout advertised in the shell input schema (seconds).
pub const ABI_DEFAULT_SHELL_TIMEOUT_SECS: u64 = 60;

/// Default CodeMode hard wall ceiling advertised in execute_code limits schema (ms).
/// Matches `tokenzero_mcp::codemode::store::HARD_MAX_WALL_MS` (env override is
/// deployment-local and does not change the published contract maximum).
pub const ABI_HARD_MAX_WALL_MS: u64 = 5000;

/// Stable identifier for a canonical operation (string form is `Operation::name`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OperationId(pub &'static str);

/// Whether the operation mutates durable workspace or store state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mutability {
    /// No durable side effects (reads, discovery, diagnostics).
    ReadOnly,
    /// May write workspace files (edit) or run side-effecting shell/fetch.
    WorkspaceMutating,
    /// May write recovery store / cache / journals without editing workspace files.
    StoreOnly,
}

impl Mutability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::WorkspaceMutating => "workspace_mutating",
            Self::StoreOnly => "store_only",
        }
    }
}

/// Relative cost class for budgeting and fusion decisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostClass {
    Cheap,
    Medium,
    Heavy,
}

impl CostClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cheap => "cheap",
            Self::Medium => "medium",
            Self::Heavy => "heavy",
        }
    }
}

/// Who may invoke the operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityRequirement {
    /// Any authenticated local client (default product surface).
    Public,
    /// Private raw worker / hub composition only (tokenzero-irx9.4).
    PrivateWorker,
}

impl CapabilityRequirement {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::PrivateWorker => "private_worker",
        }
    }
}

/// How refs produced by the operation are owned for RACC recovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefOwnership {
    None,
    Blob,
    Session,
    /// Multiple ref kinds (batch, mixed capsules).
    Multi,
    Execution,
}

impl RefOwnership {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Blob => "blob",
            Self::Session => "session",
            Self::Multi => "multi",
            Self::Execution => "execution",
        }
    }
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

impl CancellationSemantics {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Cooperative => "cooperative",
            Self::Deadline => "deadline",
        }
    }
}

/// Migration posture for aliases and legacy spellings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationStatus {
    /// Canonical name; preferred in new callers.
    Canonical,
    /// Accepted alias; prefer canonical; no removal without evidence.
    LegacyAlias,
    /// CodeMode-only control / helper (not a classic FastMCP domain tool).
    CodemodeControl,
    /// MCP resource surface (URI), not a tools/call target.
    Resource,
}

impl MigrationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Canonical => "canonical",
            Self::LegacyAlias => "legacy_alias",
            Self::CodemodeControl => "codemode_control",
            Self::Resource => "resource",
        }
    }
}

/// Where the operation appears on user-facing catalogs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceExposure {
    /// Listed as a classic FastMCP tool (`--mode=mcp`).
    pub fastmcp_tool: bool,
    /// Projected as aggregate-host control metadata, never classic MCP.
    pub codemode_mcp_tool: bool,
    /// Aggregate binding path (`zero.read`, `codemode.search`, …), if any.
    pub codemode_binding: Option<&'static str>,
    /// Resource URI (`resource://tokenzero/...`), if this entry is a resource.
    pub resource_uri: Option<&'static str>,
}

/// JSON Schema fragment for operation arguments (always `type: object`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperationArgs {
    pub schema: Value,
}

/// JSON Schema fragment for the normalized domain success/error envelope.
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
    InvalidPattern,
    InvalidRef,
    InvalidUrl,
    HunkNotFound,
    AmbiguousHunk,
    NoOpHunk,
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
            Self::InvalidPattern => "invalid_pattern",
            Self::InvalidRef => "invalid_ref",
            Self::InvalidUrl => "invalid_url",
            Self::HunkNotFound => "hunk_not_found",
            Self::AmbiguousHunk => "ambiguous_hunk",
            Self::NoOpHunk => "no_op_hunk",
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
#[derive(Clone, Debug, Serialize, Deserialize)]
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
    pub value: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
    pub op: String,
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
}

/// One canonical operation in the registry.
#[derive(Clone, Debug)]
pub struct Operation {
    /// Canonical FastMCP tool name (e.g. `tz_read`) or control/resource id.
    pub name: &'static str,
    /// Short agent-facing summary.
    pub description: &'static str,
    /// Accepted alternate spellings / legacy names / bare aliases.
    pub aliases: &'static [&'static str],
    pub mutability: Mutability,
    pub capability: CapabilityRequirement,
    pub cost_class: CostClass,
    pub ref_ownership: RefOwnership,
    pub cancellation: CancellationSemantics,
    pub migration: MigrationStatus,
    pub exposure: SurfaceExposure,
    /// Capability tags mirrored from the public catalog (discoverable contract).
    pub capabilities: &'static [&'static str],
    /// Catalog cluster (`material`, `execution`, …).
    pub cluster: &'static str,
    pub args: OperationArgs,
    /// Complete output schema for the normalized domain result envelope.
    pub results: OperationResults,
    /// Documented domain error kinds this op may return.
    pub error_kinds: &'static [DomainErrorKind],
    /// Server-accepted argument aliases not in advertised schema (discoverable).
    pub arg_aliases: Value,
}
