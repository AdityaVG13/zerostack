//! Engine- and transport-neutral `zerokernel` read-only execute protocol.
//!
//! Request/response pair used by embedded and one-shot profiles. No daemon,
//! no background pool, no mutation/effect authority. Every operational root
//! is injected; the model never retypes session or project identity.
//!
//! Fail-closed laws (this bead, engine execution out of scope):
//! - `abi_version` must be `zerokernel`.
//! - Budgets are finite and positive; zero or unbounded budgets fail.
//! - Unknown fields fail closed via `deny_unknown_fields`.
//! - Invalid root combinations fail (e.g. expected session root without session).
//! - Failure responses must NOT carry successor roots and must prove roots unchanged.
//! - No `mutation`/`effect`/`daemon`/`pool` fields exist; extra fields are rejected.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::decision::DecisionRequired;
use crate::schema::canonical_json;

pub const ZEROKERNEL_ABI_VERSION: &str = "zerokernel";

pub const MAX_WALL_MS: u64 = 300_000;
pub const MAX_CPU_MS: u64 = 300_000;
pub const MAX_MEMORY_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_CALLS: u32 = 1024;
pub const MAX_KERNEL_PREVIEW_CHARS: u32 = 4096;
pub const MAX_PROGRAM_BYTES: usize = 64 * 1024;
pub const MAX_ROOT_BYTES: usize = 256;

/// Finite budget. Every field must be >0 and bounded. Unbounded budgets are
/// not representable; zero budgets fail closed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FiniteBudget {
    pub wall_ms: u64,
    pub cpu_ms: u64,
    pub memory_bytes: u64,
    pub max_calls: u32,
}

impl FiniteBudget {
    pub fn new(wall_ms: u64, cpu_ms: u64, memory_bytes: u64, max_calls: u32) -> Result<Self, ZerokernelError> {
        let b = Self { wall_ms, cpu_ms, memory_bytes, max_calls };
        b.validate()?;
        Ok(b)
    }
    pub fn validate(&self) -> Result<(), ZerokernelError> {
        if self.wall_ms == 0 || self.cpu_ms == 0 || self.memory_bytes == 0 || self.max_calls == 0 {
            return Err(ZerokernelError::ZeroBudget);
        }
        if self.wall_ms > MAX_WALL_MS || self.cpu_ms > MAX_CPU_MS || self.memory_bytes > MAX_MEMORY_BYTES || self.max_calls > MAX_CALLS {
            return Err(ZerokernelError::BudgetTooLarge);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReturnKind {
    Inline,
    Reference,
}

/// Return policy for the read-only result. No streaming / daemon variants.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReturnPolicy {
    pub kind: ReturnKind,
    pub max_preview_chars: u32,
}

impl ReturnPolicy {
    pub fn new(kind: ReturnKind, max_preview_chars: u32) -> Result<Self, ZerokernelError> {
        let p = Self { kind, max_preview_chars };
        p.validate()?;
        Ok(p)
    }
    pub fn validate(&self) -> Result<(), ZerokernelError> {
        if self.max_preview_chars == 0 || self.max_preview_chars > MAX_KERNEL_PREVIEW_CHARS {
            return Err(ZerokernelError::InvalidReturnPolicy("max_preview_chars out of bounds".into()));
        }
        Ok(())
    }
}

/// Injected operational roots. Every root is supplied by the caller;
/// the model never synthesizes session or project identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RootBindings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<String>,
    pub project_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_manifest_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_session_root: Option<String>,
}

impl RootBindings {
    pub fn new(
        workspace_root: Option<String>,
        project_root: String,
        request_root: Option<String>,
        capability_manifest_root: Option<String>,
        expected_session_root: Option<String>,
    ) -> Result<Self, ZerokernelError> {
        let r = Self { workspace_root, project_root, request_root, capability_manifest_root, expected_session_root };
        r.validate()?;
        Ok(r)
    }
    fn validate_root(label: &str, value: &str) -> Result<(), ZerokernelError> {
        if value.is_empty() || value.len() > MAX_ROOT_BYTES {
            return Err(ZerokernelError::InvalidRoot(format!("{label} empty or too long")));
        }
        if value.contains('\0') {
            return Err(ZerokernelError::InvalidRoot(format!("{label} contains null")));
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<(), ZerokernelError> {
        Self::validate_root("project_root", &self.project_root)?;
        if let Some(v) = &self.workspace_root { Self::validate_root("workspace_root", v)?; }
        if let Some(v) = &self.request_root { Self::validate_root("request_root", v)?; }
        if let Some(v) = &self.capability_manifest_root { Self::validate_root("capability_manifest_root", v)?; }
        if let Some(v) = &self.expected_session_root { Self::validate_root("expected_session_root", v)?; }
        Ok(())
    }
}

/// Exact handles returned by the kernel (opaque, content-addressed).
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExactHandles {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_handle: Option<String>,
}

impl ExactHandles {
    pub fn validate(&self) -> Result<(), ZerokernelError> {
        if let Some(h) = &self.session_handle {
            if h.is_empty() || h.len() > MAX_ROOT_BYTES { return Err(ZerokernelError::InvalidHandle("session_handle".into())); }
        }
        if let Some(h) = &self.continuation_handle {
            if h.is_empty() || h.len() > MAX_ROOT_BYTES { return Err(ZerokernelError::InvalidHandle("continuation_handle".into())); }
        }
        Ok(())
    }
}

/// Preflight report: read-only checks before execution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreflightReport {
    pub ok: bool,
    #[serde(default)]
    pub checked_roots: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub errors: Vec<String>,
}

impl PreflightReport {
    pub fn validate(&self) -> Result<(), ZerokernelError> {
        if self.ok && !self.errors.is_empty() {
            return Err(ZerokernelError::InvalidPreflight("ok=true with errors".into()));
        }
        Ok(())
    }
}

/// Resource and call ledger (bounded, monotonic).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KernelResourceLedger {
    pub wall_ms_used: u64,
    pub cpu_ms_used: u64,
    pub calls_made: u32,
    pub bytes_out: u32,
}

impl KernelResourceLedger {
    pub fn validate(&self, budget: Option<&FiniteBudget>) -> Result<(), ZerokernelError> {
        if let Some(b) = budget {
            if self.wall_ms_used > b.wall_ms || self.cpu_ms_used > b.cpu_ms || self.calls_made > b.max_calls {
                return Err(ZerokernelError::LedgerExceedsBudget);
            }
        }
        Ok(())
    }
}

/// Snapshot of injected roots at a point in time.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RootSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<String>,
    pub project_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_root: Option<String>,
}

impl RootSnapshot {
    pub fn validate(&self) -> Result<(), ZerokernelError> {
        RootBindings::validate_root("project_root", &self.project_root)?;
        if let Some(v) = &self.workspace_root { RootBindings::validate_root("workspace_root", v)?; }
        if let Some(v) = &self.session_root { RootBindings::validate_root("session_root", v)?; }
        Ok(())
    }
}

/// Evidence that a failed execution left roots unchanged, or that a
/// successful one produced new roots. Failures must prove unchanged.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RootEvidence {
    pub before: RootSnapshot,
    pub after: RootSnapshot,
    pub unchanged: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor_root: Option<String>,
}

impl RootEvidence {
    pub fn validate_for_kind(&self, kind: ZerokernelResultKind) -> Result<(), ZerokernelError> {
        self.before.validate()?;
        self.after.validate()?;
        if let Some(s) = &self.successor_root { RootBindings::validate_root("successor_root", s)?; }
        let eq = self.before == self.after;
        if self.unchanged != eq {
            return Err(ZerokernelError::RootEvidenceMismatch("unchanged flag disagrees with before/after".into()));
        }
        if kind == ZerokernelResultKind::Failed {
            if !self.unchanged { return Err(ZerokernelError::FailureMustBeUnchanged); }
            if self.successor_root.is_some() { return Err(ZerokernelError::FailureWithSuccessorRoot); }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub enum ZerokernelResultKind {
    Completed,
    DecisionRequired,
    Failed,
}

/// Read-only execute request (transport-neutral). Embedded and one-shot
/// profiles serialize identically; no transport/daemon field exists.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZerokernelExecuteRequest {
    pub abi_version: String,
    pub program: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub budget: FiniteBudget,
    pub return_policy: ReturnPolicy,
    pub roots: RootBindings,
}

impl ZerokernelExecuteRequest {
    pub fn new(
        program: String,
        session: Option<String>,
        budget: FiniteBudget,
        return_policy: ReturnPolicy,
        roots: RootBindings,
    ) -> Result<Self, ZerokernelError> {
        let r = Self {
            abi_version: ZEROKERNEL_ABI_VERSION.to_owned(),
            program,
            session,
            budget,
            return_policy,
            roots,
        };
        r.validate()?;
        Ok(r)
    }
    pub fn validate(&self) -> Result<(), ZerokernelError> {
        if self.abi_version != ZEROKERNEL_ABI_VERSION {
            return Err(ZerokernelError::WrongAbiVersion { actual: self.abi_version.clone() });
        }
        if self.program.is_empty() || self.program.len() > MAX_PROGRAM_BYTES {
            return Err(ZerokernelError::EmptyProgram);
        }
        if let Some(s) = &self.session {
            if s.is_empty() || s.len() > MAX_ROOT_BYTES { return Err(ZerokernelError::InvalidHandle("session".into())); }
        }
        self.budget.validate()?;
        self.return_policy.validate()?;
        self.roots.validate()?;
        if self.roots.expected_session_root.is_some() && self.session.is_none() {
            return Err(ZerokernelError::InvalidRootCombination("expected_session_root without session".into()));
        }
        Ok(())
    }
    pub fn canonical_json(&self) -> String {
        let v = serde_json::to_value(self).expect("serialize request");
        canonical_json(&v)
    }
    pub fn canonical_bytes(&self) -> Vec<u8> {
        self.canonical_json().into_bytes()
    }
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ZerokernelError> {
        let v: Self = serde_json::from_slice(bytes).map_err(|e| ZerokernelError::InvalidJson(e.to_string()))?;
        v.validate()?;
        Ok(v)
    }
}

/// Read-only execute response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZerokernelExecuteResponse {
    pub abi_version: String,
    pub kind: ZerokernelResultKind,
    pub handles: ExactHandles,
    pub preflight: PreflightReport,
    pub ledger: KernelResourceLedger,
    pub root_evidence: RootEvidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<DecisionRequired>,
}

impl ZerokernelExecuteResponse {
    pub fn completed(
        handles: ExactHandles,
        preflight: PreflightReport,
        ledger: KernelResourceLedger,
        root_evidence: RootEvidence,
        result: Value,
    ) -> Result<Self, ZerokernelError> {
        let r = Self {
            abi_version: ZEROKERNEL_ABI_VERSION.to_owned(),
            kind: ZerokernelResultKind::Completed,
            handles,
            preflight,
            ledger,
            root_evidence,
            result: Some(result),
            decision: None,
        };
        r.validate()?;
        Ok(r)
    }
    pub fn decision_required(
        handles: ExactHandles,
        preflight: PreflightReport,
        ledger: KernelResourceLedger,
        root_evidence: RootEvidence,
        decision: DecisionRequired,
    ) -> Result<Self, ZerokernelError> {
        let r = Self {
            abi_version: ZEROKERNEL_ABI_VERSION.to_owned(),
            kind: ZerokernelResultKind::DecisionRequired,
            handles,
            preflight,
            ledger,
            root_evidence,
            result: None,
            decision: Some(decision),
        };
        r.validate()?;
        Ok(r)
    }
    pub fn failed(
        handles: ExactHandles,
        preflight: PreflightReport,
        ledger: KernelResourceLedger,
        root_evidence: RootEvidence,
    ) -> Result<Self, ZerokernelError> {
        let r = Self {
            abi_version: ZEROKERNEL_ABI_VERSION.to_owned(),
            kind: ZerokernelResultKind::Failed,
            handles,
            preflight,
            ledger,
            root_evidence,
            result: None,
            decision: None,
        };
        r.validate()?;
        Ok(r)
    }
    pub fn validate(&self) -> Result<(), ZerokernelError> {
        if self.abi_version != ZEROKERNEL_ABI_VERSION {
            return Err(ZerokernelError::WrongAbiVersion { actual: self.abi_version.clone() });
        }
        self.handles.validate()?;
        self.preflight.validate()?;
        self.root_evidence.validate_for_kind(self.kind)?;
        // kind-specific payload requirements
        match self.kind {
            ZerokernelResultKind::Completed => {
                if self.result.is_none() { return Err(ZerokernelError::MissingField("result")); }
                if self.decision.is_some() { return Err(ZerokernelError::ForbiddenField("decision on Completed")); }
            }
            ZerokernelResultKind::DecisionRequired => {
                if self.decision.is_none() { return Err(ZerokernelError::MissingField("decision")); }
                if self.result.is_some() { return Err(ZerokernelError::ForbiddenField("result on DecisionRequired")); }
            }
            ZerokernelResultKind::Failed => {
                if self.result.is_some() { return Err(ZerokernelError::ForbiddenField("result on Failed")); }
                if self.decision.is_some() { return Err(ZerokernelError::ForbiddenField("decision on Failed")); }
            }
        }
        Ok(())
    }
    pub fn canonical_json(&self) -> String {
        let v = serde_json::to_value(self).expect("serialize response");
        canonical_json(&v)
    }
    pub fn canonical_bytes(&self) -> Vec<u8> {
        self.canonical_json().into_bytes()
    }
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ZerokernelError> {
        let v: Self = serde_json::from_slice(bytes).map_err(|e| ZerokernelError::InvalidJson(e.to_string()))?;
        v.validate()?;
        Ok(v)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ZerokernelError {
    WrongAbiVersion { actual: String },
    EmptyProgram,
    ZeroBudget,
    BudgetTooLarge,
    InvalidReturnPolicy(String),
    InvalidRoot(String),
    InvalidRootCombination(String),
    InvalidHandle(String),
    InvalidPreflight(String),
    LedgerExceedsBudget,
    RootEvidenceMismatch(String),
    FailureMustBeUnchanged,
    FailureWithSuccessorRoot,
    MissingField(&'static str),
    ForbiddenField(&'static str),
    InvalidJson(String),
}

impl fmt::Display for ZerokernelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongAbiVersion { actual } => write!(f, "abi_version must be {ZEROKERNEL_ABI_VERSION}, got {actual}"),
            Self::EmptyProgram => write!(f, "program must be 1..{} bytes", MAX_PROGRAM_BYTES),
            Self::ZeroBudget => write!(f, "budget fields must be >0 (zero/unbounded rejected)"),
            Self::BudgetTooLarge => write!(f, "budget exceeds bounded maximum"),
            Self::InvalidReturnPolicy(s) => write!(f, "return policy invalid: {s}"),
            Self::InvalidRoot(s) => write!(f, "invalid root: {s}"),
            Self::InvalidRootCombination(s) => write!(f, "invalid root combination: {s}"),
            Self::InvalidHandle(s) => write!(f, "invalid handle: {s}"),
            Self::InvalidPreflight(s) => write!(f, "invalid preflight: {s}"),
            Self::LedgerExceedsBudget => write!(f, "ledger exceeds budget"),
            Self::RootEvidenceMismatch(s) => write!(f, "root evidence mismatch: {s}"),
            Self::FailureMustBeUnchanged => write!(f, "failed response must prove roots unchanged"),
            Self::FailureWithSuccessorRoot => write!(f, "failed response must not carry successor_root"),
            Self::MissingField(field) => write!(f, "kind requires field {field}"),
            Self::ForbiddenField(field) => write!(f, "kind must not carry {field}"),
            Self::InvalidJson(s) => write!(f, "invalid json: {s}"),
        }
    }
}
impl Error for ZerokernelError {}

