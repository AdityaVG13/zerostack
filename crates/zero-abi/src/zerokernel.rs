//! Engine- and transport-neutral `zerokernel/v1` read-only execute protocol.
//!
//! Request/response pair used by embedded and one-shot profiles. No daemon,
//! no background pool, no mutation/effect authority. Every operational root
//! is injected; the model never retypes session or project identity.
//!
//! Fail-closed laws (this bead, engine execution out of scope):
//! - `abi_version` must be `zerokernel/v1`.
//! - Budgets are finite and positive; zero or unbounded budgets fail.
//! - Unknown fields fail closed via `deny_unknown_fields`.
//! - Invalid root combinations fail (e.g. expected session root without session).
//! - Failure responses must NOT carry successor roots and must prove roots unchanged.
//! - No `mutation`/`effect`/`daemon`/`pool` fields exist; extra fields are rejected.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::decision::DecisionRequiredV1;
use crate::schema::canonical_json;

pub const ZEROKERNEL_ABI_VERSION_V1: &str = "zerokernel/v1";
/// Alias accepted in docs; canonical on wire is `zerokernel/v1`.
pub const ZERO_EXECUTE_V1_ABI_VERSION: &str = ZEROKERNEL_ABI_VERSION_V1;

pub const MAX_WALL_MS_V1: u64 = 300_000;
pub const MAX_CPU_MS_V1: u64 = 300_000;
pub const MAX_MEMORY_BYTES_V1: u64 = 512 * 1024 * 1024;
pub const MAX_CALLS_V1: u32 = 1024;
pub const MAX_PREVIEW_CHARS_V1: u32 = 4096;
pub const MAX_PROGRAM_BYTES_V1: usize = 64 * 1024;
pub const MAX_ROOT_BYTES_V1: usize = 256;

/// Finite budget. Every field must be >0 and bounded. Unbounded budgets are
/// not representable; zero budgets fail closed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FiniteBudgetV1 {
    pub wall_ms: u64,
    pub cpu_ms: u64,
    pub memory_bytes: u64,
    pub max_calls: u32,
}

impl FiniteBudgetV1 {
    pub fn new(wall_ms: u64, cpu_ms: u64, memory_bytes: u64, max_calls: u32) -> Result<Self, ZerokernelErrorV1> {
        let b = Self { wall_ms, cpu_ms, memory_bytes, max_calls };
        b.validate()?;
        Ok(b)
    }
    pub fn validate(&self) -> Result<(), ZerokernelErrorV1> {
        if self.wall_ms == 0 || self.cpu_ms == 0 || self.memory_bytes == 0 || self.max_calls == 0 {
            return Err(ZerokernelErrorV1::ZeroBudget);
        }
        if self.wall_ms > MAX_WALL_MS_V1 || self.cpu_ms > MAX_CPU_MS_V1 || self.memory_bytes > MAX_MEMORY_BYTES_V1 || self.max_calls > MAX_CALLS_V1 {
            return Err(ZerokernelErrorV1::BudgetTooLarge);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReturnKindV1 {
    Inline,
    Reference,
}

/// Return policy for the read-only result. No streaming / daemon variants.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReturnPolicyV1 {
    pub kind: ReturnKindV1,
    pub max_preview_chars: u32,
}

impl ReturnPolicyV1 {
    pub fn new(kind: ReturnKindV1, max_preview_chars: u32) -> Result<Self, ZerokernelErrorV1> {
        let p = Self { kind, max_preview_chars };
        p.validate()?;
        Ok(p)
    }
    pub fn validate(&self) -> Result<(), ZerokernelErrorV1> {
        if self.max_preview_chars == 0 || self.max_preview_chars > MAX_PREVIEW_CHARS_V1 {
            return Err(ZerokernelErrorV1::InvalidReturnPolicy("max_preview_chars out of bounds".into()));
        }
        Ok(())
    }
}

/// Injected operational roots. Every root is supplied by the caller;
/// the model never synthesizes session or project identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RootBindingsV1 {
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

impl RootBindingsV1 {
    pub fn new(
        workspace_root: Option<String>,
        project_root: String,
        request_root: Option<String>,
        capability_manifest_root: Option<String>,
        expected_session_root: Option<String>,
    ) -> Result<Self, ZerokernelErrorV1> {
        let r = Self { workspace_root, project_root, request_root, capability_manifest_root, expected_session_root };
        r.validate()?;
        Ok(r)
    }
    fn validate_root(label: &str, value: &str) -> Result<(), ZerokernelErrorV1> {
        if value.is_empty() || value.len() > MAX_ROOT_BYTES_V1 {
            return Err(ZerokernelErrorV1::InvalidRoot(format!("{label} empty or too long")));
        }
        if value.contains('\0') {
            return Err(ZerokernelErrorV1::InvalidRoot(format!("{label} contains null")));
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<(), ZerokernelErrorV1> {
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
pub struct ExactHandlesV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_handle: Option<String>,
}

impl ExactHandlesV1 {
    pub fn validate(&self) -> Result<(), ZerokernelErrorV1> {
        if let Some(h) = &self.session_handle {
            if h.is_empty() || h.len() > MAX_ROOT_BYTES_V1 { return Err(ZerokernelErrorV1::InvalidHandle("session_handle".into())); }
        }
        if let Some(h) = &self.continuation_handle {
            if h.is_empty() || h.len() > MAX_ROOT_BYTES_V1 { return Err(ZerokernelErrorV1::InvalidHandle("continuation_handle".into())); }
        }
        Ok(())
    }
}

/// Preflight report: read-only checks before execution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreflightReportV1 {
    pub ok: bool,
    #[serde(default)]
    pub checked_roots: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub errors: Vec<String>,
}

impl PreflightReportV1 {
    pub fn validate(&self) -> Result<(), ZerokernelErrorV1> {
        if self.ok && !self.errors.is_empty() {
            return Err(ZerokernelErrorV1::InvalidPreflight("ok=true with errors".into()));
        }
        Ok(())
    }
}

/// Resource and call ledger (bounded, monotonic).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceLedgerV1 {
    pub wall_ms_used: u64,
    pub cpu_ms_used: u64,
    pub calls_made: u32,
    pub bytes_out: u32,
}

impl ResourceLedgerV1 {
    pub fn validate(&self, budget: Option<&FiniteBudgetV1>) -> Result<(), ZerokernelErrorV1> {
        if let Some(b) = budget {
            if self.wall_ms_used > b.wall_ms || self.cpu_ms_used > b.cpu_ms || self.calls_made > b.max_calls {
                return Err(ZerokernelErrorV1::LedgerExceedsBudget);
            }
        }
        Ok(())
    }
}

/// Snapshot of injected roots at a point in time.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RootSnapshotV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<String>,
    pub project_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_root: Option<String>,
}

impl RootSnapshotV1 {
    pub fn validate(&self) -> Result<(), ZerokernelErrorV1> {
        RootBindingsV1::validate_root("project_root", &self.project_root)?;
        if let Some(v) = &self.workspace_root { RootBindingsV1::validate_root("workspace_root", v)?; }
        if let Some(v) = &self.session_root { RootBindingsV1::validate_root("session_root", v)?; }
        Ok(())
    }
}

/// Evidence that a failed execution left roots unchanged, or that a
/// successful one produced new roots. Failures must prove unchanged.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RootEvidenceV1 {
    pub before: RootSnapshotV1,
    pub after: RootSnapshotV1,
    pub unchanged: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor_root: Option<String>,
}

impl RootEvidenceV1 {
    pub fn validate_for_kind(&self, kind: ZerokernelResultKindV1) -> Result<(), ZerokernelErrorV1> {
        self.before.validate()?;
        self.after.validate()?;
        if let Some(s) = &self.successor_root { RootBindingsV1::validate_root("successor_root", s)?; }
        let eq = self.before == self.after;
        if self.unchanged != eq {
            return Err(ZerokernelErrorV1::RootEvidenceMismatch("unchanged flag disagrees with before/after".into()));
        }
        if kind == ZerokernelResultKindV1::Failed {
            if !self.unchanged { return Err(ZerokernelErrorV1::FailureMustBeUnchanged); }
            if self.successor_root.is_some() { return Err(ZerokernelErrorV1::FailureWithSuccessorRoot); }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub enum ZerokernelResultKindV1 {
    Completed,
    DecisionRequired,
    Failed,
}

/// Read-only execute request (transport-neutral). Embedded and one-shot
/// profiles serialize identically; no transport/daemon field exists.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZerokernelExecuteRequestV1 {
    pub abi_version: String,
    pub program: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub budget: FiniteBudgetV1,
    pub return_policy: ReturnPolicyV1,
    pub roots: RootBindingsV1,
}

impl ZerokernelExecuteRequestV1 {
    pub fn new(
        program: String,
        session: Option<String>,
        budget: FiniteBudgetV1,
        return_policy: ReturnPolicyV1,
        roots: RootBindingsV1,
    ) -> Result<Self, ZerokernelErrorV1> {
        let r = Self {
            abi_version: ZEROKERNEL_ABI_VERSION_V1.to_owned(),
            program,
            session,
            budget,
            return_policy,
            roots,
        };
        r.validate()?;
        Ok(r)
    }
    pub fn validate(&self) -> Result<(), ZerokernelErrorV1> {
        if self.abi_version != ZEROKERNEL_ABI_VERSION_V1 {
            return Err(ZerokernelErrorV1::WrongAbiVersion { actual: self.abi_version.clone() });
        }
        if self.program.is_empty() || self.program.len() > MAX_PROGRAM_BYTES_V1 {
            return Err(ZerokernelErrorV1::EmptyProgram);
        }
        if let Some(s) = &self.session {
            if s.is_empty() || s.len() > MAX_ROOT_BYTES_V1 { return Err(ZerokernelErrorV1::InvalidHandle("session".into())); }
        }
        self.budget.validate()?;
        self.return_policy.validate()?;
        self.roots.validate()?;
        if self.roots.expected_session_root.is_some() && self.session.is_none() {
            return Err(ZerokernelErrorV1::InvalidRootCombination("expected_session_root without session".into()));
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
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ZerokernelErrorV1> {
        let v: Self = serde_json::from_slice(bytes).map_err(|e| ZerokernelErrorV1::InvalidJson(e.to_string()))?;
        v.validate()?;
        Ok(v)
    }
}

/// Read-only execute response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZerokernelExecuteResponseV1 {
    pub abi_version: String,
    pub kind: ZerokernelResultKindV1,
    pub handles: ExactHandlesV1,
    pub preflight: PreflightReportV1,
    pub ledger: ResourceLedgerV1,
    pub root_evidence: RootEvidenceV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<DecisionRequiredV1>,
}

impl ZerokernelExecuteResponseV1 {
    pub fn completed(
        handles: ExactHandlesV1,
        preflight: PreflightReportV1,
        ledger: ResourceLedgerV1,
        root_evidence: RootEvidenceV1,
        result: Value,
    ) -> Result<Self, ZerokernelErrorV1> {
        let r = Self {
            abi_version: ZEROKERNEL_ABI_VERSION_V1.to_owned(),
            kind: ZerokernelResultKindV1::Completed,
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
        handles: ExactHandlesV1,
        preflight: PreflightReportV1,
        ledger: ResourceLedgerV1,
        root_evidence: RootEvidenceV1,
        decision: DecisionRequiredV1,
    ) -> Result<Self, ZerokernelErrorV1> {
        let r = Self {
            abi_version: ZEROKERNEL_ABI_VERSION_V1.to_owned(),
            kind: ZerokernelResultKindV1::DecisionRequired,
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
        handles: ExactHandlesV1,
        preflight: PreflightReportV1,
        ledger: ResourceLedgerV1,
        root_evidence: RootEvidenceV1,
    ) -> Result<Self, ZerokernelErrorV1> {
        let r = Self {
            abi_version: ZEROKERNEL_ABI_VERSION_V1.to_owned(),
            kind: ZerokernelResultKindV1::Failed,
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
    pub fn validate(&self) -> Result<(), ZerokernelErrorV1> {
        if self.abi_version != ZEROKERNEL_ABI_VERSION_V1 {
            return Err(ZerokernelErrorV1::WrongAbiVersion { actual: self.abi_version.clone() });
        }
        self.handles.validate()?;
        self.preflight.validate()?;
        self.root_evidence.validate_for_kind(self.kind)?;
        // kind-specific payload requirements
        match self.kind {
            ZerokernelResultKindV1::Completed => {
                if self.result.is_none() { return Err(ZerokernelErrorV1::MissingField("result")); }
                if self.decision.is_some() { return Err(ZerokernelErrorV1::ForbiddenField("decision on Completed")); }
            }
            ZerokernelResultKindV1::DecisionRequired => {
                if self.decision.is_none() { return Err(ZerokernelErrorV1::MissingField("decision")); }
                if self.result.is_some() { return Err(ZerokernelErrorV1::ForbiddenField("result on DecisionRequired")); }
            }
            ZerokernelResultKindV1::Failed => {
                if self.result.is_some() { return Err(ZerokernelErrorV1::ForbiddenField("result on Failed")); }
                if self.decision.is_some() { return Err(ZerokernelErrorV1::ForbiddenField("decision on Failed")); }
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
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ZerokernelErrorV1> {
        let v: Self = serde_json::from_slice(bytes).map_err(|e| ZerokernelErrorV1::InvalidJson(e.to_string()))?;
        v.validate()?;
        Ok(v)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ZerokernelErrorV1 {
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

impl fmt::Display for ZerokernelErrorV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongAbiVersion { actual } => write!(f, "abi_version must be {ZEROKERNEL_ABI_VERSION_V1}, got {actual}"),
            Self::EmptyProgram => write!(f, "program must be 1..{} bytes", MAX_PROGRAM_BYTES_V1),
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
impl Error for ZerokernelErrorV1 {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::{DecisionRequiredV1, ObservationClassV1};
    use serde_json::json;

    fn sample_budget() -> FiniteBudgetV1 { FiniteBudgetV1::new(5000, 5000, 64 * 1024 * 1024, 32).unwrap() }
    fn sample_policy() -> ReturnPolicyV1 { ReturnPolicyV1::new(ReturnKindV1::Inline, 512).unwrap() }
    fn sample_roots() -> RootBindingsV1 {
        RootBindingsV1::new(None, "proj_abc123".into(), None, Some("cap_root_123".into()), None).unwrap()
    }
    fn sample_snapshot() -> RootSnapshotV1 {
        RootSnapshotV1 { workspace_root: None, project_root: "proj_abc123".into(), session_root: Some("sess_1".into()) }
    }
    fn sample_handles() -> ExactHandlesV1 { ExactHandlesV1 { session_handle: Some("sess_1".into()), continuation_handle: None } }
    fn sample_preflight() -> PreflightReportV1 { PreflightReportV1 { ok: true, checked_roots: vec!["proj_abc123".into()], warnings: vec![], errors: vec![] } }
    fn sample_ledger() -> ResourceLedgerV1 { ResourceLedgerV1 { wall_ms_used: 10, cpu_ms_used: 5, calls_made: 1, bytes_out: 100 } }
    fn sample_evidence_unchanged() -> RootEvidenceV1 {
        let snap = sample_snapshot();
        RootEvidenceV1 { before: snap.clone(), after: snap, unchanged: true, successor_root: None }
    }
    fn sample_evidence_changed() -> RootEvidenceV1 {
        RootEvidenceV1 {
            before: RootSnapshotV1 { workspace_root: None, project_root: "proj_abc123".into(), session_root: Some("sess_1".into()) },
            after: RootSnapshotV1 { workspace_root: None, project_root: "proj_abc123".into(), session_root: Some("sess_2".into()) },
            unchanged: false,
            successor_root: Some("succ_1".into()),
        }
    }

    #[test]
    fn round_trip_request_canonical() {
        let req = ZerokernelExecuteRequestV1::new("return 42;".into(), Some("sess_1".into()), sample_budget(), sample_policy(), sample_roots()).unwrap();
        let bytes = req.canonical_bytes();
        let back = ZerokernelExecuteRequestV1::from_canonical_bytes(&bytes).unwrap();
        assert_eq!(req, back);
        // embedded vs one-shot serialize identically
        let embedded = ZerokernelExecuteRequestV1::new("return 42;".into(), Some("sess_1".into()), sample_budget(), sample_policy(), sample_roots()).unwrap();
        let oneshot = ZerokernelExecuteRequestV1::new("return 42;".into(), Some("sess_1".into()), sample_budget(), sample_policy(), sample_roots()).unwrap();
        assert_eq!(embedded.canonical_json(), oneshot.canonical_json());
    }

    #[test]
    fn round_trip_response_completed_and_decision() {
        let completed = ZerokernelExecuteResponseV1::completed(sample_handles(), sample_preflight(), sample_ledger(), sample_evidence_changed(), json!({"ok":true,"value":42})).unwrap();
        let bytes = completed.canonical_bytes();
        assert_eq!(completed, ZerokernelExecuteResponseV1::from_canonical_bytes(&bytes).unwrap());

        let obs = ObservationClassV1::new("test.class").unwrap();
        let decision = DecisionRequiredV1 { decision_id: "d1".into(), observation_class: obs, question: "choose?".into(), choices: vec!["a".into(), "b".into()], observed_value: "c".into() };
        let dr = ZerokernelExecuteResponseV1::decision_required(sample_handles(), sample_preflight(), sample_ledger(), sample_evidence_unchanged(), decision).unwrap();
        let bytes2 = dr.canonical_bytes();
        assert_eq!(dr, ZerokernelExecuteResponseV1::from_canonical_bytes(&bytes2).unwrap());
    }

    #[test]
    fn round_trip_failed_proves_unchanged() {
        let failed = ZerokernelExecuteResponseV1::failed(sample_handles(), sample_preflight(), sample_ledger(), sample_evidence_unchanged()).unwrap();
        let bytes = failed.canonical_bytes();
        let back = ZerokernelExecuteResponseV1::from_canonical_bytes(&bytes).unwrap();
        assert!(back.root_evidence.unchanged);
        assert!(back.root_evidence.successor_root.is_none());
        assert_eq!(failed, back);
    }

    #[test]
    fn unknown_fields_fail_closed() {
        let budget = sample_budget();
        let policy = sample_policy();
        let roots = sample_roots();
        let req = ZerokernelExecuteRequestV1::new("p".into(), None, budget, policy, roots).unwrap();
        let mut v = serde_json::to_value(req).unwrap();
        v["daemon"] = json!(true);
        assert!(serde_json::from_value::<ZerokernelExecuteRequestV1>(v).is_err());

        let mut v2 = json!({"abi_version": ZEROKERNEL_ABI_VERSION_V1, "kind":"Failed", "handles":{}, "preflight":{"ok":true,"checked_roots":[],"warnings":[],"errors":[]}, "ledger":{"wall_ms_used":1,"cpu_ms_used":1,"calls_made":1,"bytes_out":0}, "root_evidence":{"before":{"project_root":"p"},"after":{"project_root":"p"},"unchanged":true}, "unknown":1});
        assert!(serde_json::from_value::<ZerokernelExecuteResponseV1>(v2).is_err());
    }

    #[test]
    fn zero_and_unbounded_budgets_fail() {
        assert!(FiniteBudgetV1::new(0, 100, 1024, 1).is_err());
        assert!(FiniteBudgetV1::new(100, 0, 1024, 1).is_err());
        assert!(FiniteBudgetV1::new(1, 1, 0, 1).is_err());
        assert!(FiniteBudgetV1::new(1, 1, 1024, 0).is_err());
        // unbounded via too large
        assert!(FiniteBudgetV1::new(MAX_WALL_MS_V1+1, 100, 1024, 1).is_err());
        // string instead of number fails serde (unbounded not representable)
        let v = json!({"wall_ms":"unbounded","cpu_ms":100,"memory_bytes":1024,"max_calls":1});
        assert!(serde_json::from_value::<FiniteBudgetV1>(v).is_err());
        // zero via json
        let v2 = json!({"wall_ms":0,"cpu_ms":100,"memory_bytes":1024,"max_calls":1});
        let b: Result<FiniteBudgetV1,_> = serde_json::from_value(v2);
        assert!(b.is_ok()); // deserializes but validate fails
        assert!(b.unwrap().validate().is_err());
    }

    #[test]
    fn mutation_effect_daemon_fields_rejected() {
        for field in ["mutation","effect","daemon","pool","background_pool","write_authority"] {
            let v = json!({
                "abi_version": ZEROKERNEL_ABI_VERSION_V1,
                "program": "return 1",
                "budget": {"wall_ms":100,"cpu_ms":100,"memory_bytes":1024,"max_calls":1},
                "return_policy": {"kind":"inline","max_preview_chars":100},
                "roots": {"project_root":"p"},
                field: true
            });
            assert!(serde_json::from_value::<ZerokernelExecuteRequestV1>(v).is_err(), "field {field} should be rejected");
        }
    }

    #[test]
    fn invalid_root_combinations_fail() {
        // expected_session_root without session
        let roots = RootBindingsV1::new(None, "p".into(), None, None, Some("sess_root".into())).unwrap();
        let req = ZerokernelExecuteRequestV1::new("prog".into(), None, sample_budget(), sample_policy(), roots);
        assert!(req.is_err());
    }

    #[test]
    fn failure_with_successor_root_rejected() {
        let mut evidence = sample_evidence_unchanged();
        evidence.successor_root = Some("succ".into());
        // unchanged still true but successor present should fail
        let res = ZerokernelExecuteResponseV1::failed(sample_handles(), sample_preflight(), sample_ledger(), evidence);
        assert!(res.is_err());
        // also changed evidence with Failed should fail
        let mut evidence2 = sample_evidence_changed();
        evidence2.unchanged = false;
        let res2 = ZerokernelExecuteResponseV1::failed(sample_handles(), sample_preflight(), sample_ledger(), evidence2);
        assert!(res2.is_err());
    }

    #[test]
    fn wrong_abi_version_rejected() {
        let mut req = ZerokernelExecuteRequestV1::new("p".into(), None, sample_budget(), sample_policy(), sample_roots()).unwrap();
        req.abi_version = "wrong/v1".into();
        assert!(req.validate().is_err());
    }
}
