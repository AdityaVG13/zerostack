//! Capability handshake and private raw worker protocol (fszero-ncib.4).
//!
//! Shared `zerostack.surface` manifest fields are identical across FSZero,
//! GraphZero, TokenZero, and peer ZeroStack components. The private raw worker
//! is an **internal** mode of the selected FastMCP or CodeMode artifact — never
//! a third user-facing installation. It invokes the typed domain dispatcher
//! directly and never plans, parses JavaScript, starts a sandbox, or rewrites
//! envelopes.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU32, Ordering};

use super::dispatcher::{DispatchOutcome, dispatch_raw_worker};
use super::operation_abi::{
    DomainError, DomainResult, OPERATION_ABI_NAME, OPERATION_ABI_VERSION, operation_abi_digest,
};
use super::session::FSZeroSession;

/// Shared ZeroStack surface capability schema id.
pub const SURFACE_MANIFEST_SCHEMA: &str = "zerostack.surface";
/// Semantic contract family name (stable across digests).
pub const SEMANTIC_CONTRACT_NAME: &str = "fszero.operation_abi";
/// Private raw worker protocol version.
pub const RAW_WORKER_VERSION: &str = "1.0.0";
/// FSZero ref scheme.
pub const REF_SCHEME: &str = "fz";
/// FSZero ref protocol major version.
pub const REF_VERSION: &str = "1";

/// Selected user-facing installation surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectedSurface {
    Mcp,
    Codemode,
}

impl SelectedSurface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
            Self::Codemode => "codemode",
        }
    }

    /// Parse install/handshake surface names (aliases accepted).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mcp" | "fastmcp" | "per-op" | "per_op" => Some(Self::Mcp),
            "codemode" | "code-mode" | "code_mode" => Some(Self::Codemode),
            _ => None,
        }
    }
}

/// Who owns planning or compression for this composition.
///
/// Client-native CodeMode + raw FSZero worker: both owners are `Client`.
/// ZeroStack outer router composing FSZero: both owners are `OuterRouter`.
/// Server CodeMode artifact: both owners are `Server`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ownership {
    Client,
    Server,
    OuterRouter,
    None,
}

impl Ownership {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Server => "server",
            Self::OuterRouter => "outer_router",
            Self::None => "none",
        }
    }
}

/// Fixed-field capability record (handshake without catalog listing).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfaceCapability {
    /// Always `zerostack.surface`.
    pub schema: String,
    pub surface: SelectedSurface,
    pub planner_owner: Ownership,
    pub compression_owner: Ownership,
    pub semantic_contract_name: String,
    pub semantic_contract_version: String,
    pub semantic_contract_digest: String,
    /// Registry digest (FSZero equates this with the ABI digest today).
    pub operation_registry_digest: String,
    pub ref_scheme: String,
    pub ref_version: String,
    /// Supported plan forms on **this** process (empty for raw worker / FastMCP).
    pub plan_forms: Vec<String>,
    pub raw_worker_version: String,
    pub cancellation: bool,
    pub transactions: bool,
    pub streaming: bool,
    pub limits: Value,
}

/// Peer handshake request. Does not include the operation catalog.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HandshakeRequest {
    /// Required: peer's expected contract digest (hex). Mismatch fails closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_contract_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_contract_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_worker_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect_surface: Option<SelectedSurface>,
    /// Declared ownership so traces stay honest (defaults applied if omitted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner_owner: Option<Ownership>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compression_owner: Option<Ownership>,
}

/// Successful handshake acknowledgement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HandshakeAck {
    pub ok: bool,
    pub capability: SurfaceCapability,
    pub compatibility: Value,
}

/// Per-call trace fields required by the epic (planner/compression ownership).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkerTrace {
    pub planner_owner: String,
    pub compression_owner: String,
    pub surface: String,
    pub contract_digest: String,
    pub boundary_count: u32,
    pub raw_worker_version: String,
}

/// Inbound private-worker frames (stable JSON protocol).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerRequestFrame {
    Handshake {
        request: HandshakeRequest,
    },
    Call {
        op: String,
        #[serde(default)]
        args: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },
}

/// Outbound private-worker frames.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerResponseFrame {
    HandshakeAck {
        ack: HandshakeAck,
    },
    Result {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        result: DomainResult,
        trace: WorkerTrace,
        /// Ownership telemetry for hub composition (mirrors WorkerTrace).
        telemetry: Value,
    },
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        error: DomainError,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trace: Option<WorkerTrace>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        compatibility: Option<Value>,
    },
}

/// Default limits advertised on the capability record (not the full catalog).
pub fn default_worker_limits() -> Value {
    json!({ "max_logical_ops": 1000, "max_output_bytes": 65536, "max_code_bytes": 65536, "cancellation": "cooperative_or_deadline", })
}

/// Live semantic contract digest (operation ABI digest hex).
pub fn contract_digest_hex() -> String {
    operation_abi_digest()
}

/// Build the local capability record for a selected installation surface.
///
/// Plan forms are empty when this process is acting as the **raw worker**
/// (client or outer router owns planning). Server CodeMode may advertise
/// recipe/json/js for its own execute path, but the private worker session
/// itself still never plans.
pub fn local_capability(
    surface: SelectedSurface,
    planner_owner: Ownership,
    compression_owner: Ownership,
) -> SurfaceCapability {
    let digest = contract_digest_hex();
    let plan_forms = match (surface, planner_owner) {
        (SelectedSurface::Codemode, Ownership::Server) => {
            vec!["recipe".into(), "json".into(), "js".into()]
        }
        _ => Vec::new(),
    };
    SurfaceCapability {
        schema: SURFACE_MANIFEST_SCHEMA.into(),
        surface,
        planner_owner,
        compression_owner,
        semantic_contract_name: SEMANTIC_CONTRACT_NAME.into(),
        semantic_contract_version: OPERATION_ABI_VERSION.into(),
        semantic_contract_digest: digest.clone(),
        operation_registry_digest: digest,
        ref_scheme: REF_SCHEME.into(),
        ref_version: REF_VERSION.into(),
        plan_forms,
        raw_worker_version: RAW_WORKER_VERSION.into(),
        cancellation: true,
        transactions: true,
        streaming: false,
        limits: default_worker_limits(),
    }
}

/// Capability for client-native CodeMode composing FSZero as a raw worker
/// (exactly one planner and one final serializer — both on the client).
pub fn client_native_raw_worker_capability(surface: SelectedSurface) -> SurfaceCapability {
    local_capability(surface, Ownership::Client, Ownership::Client)
}

/// Capability for ZeroStack outer-router composition.
pub fn outer_router_raw_worker_capability(surface: SelectedSurface) -> SurfaceCapability {
    local_capability(surface, Ownership::OuterRouter, Ownership::OuterRouter)
}

fn compatibility_payload(local: &SurfaceCapability) -> Value {
    json!({
        "schema": local.schema, "surface": local.surface.as_str(), "semantic_contract_name": local.semantic_contract_name,
        "semantic_contract_version": local.semantic_contract_version, "semantic_contract_digest": local.semantic_contract_digest,
        "operation_registry_digest": local.operation_registry_digest, "raw_worker_version": local.raw_worker_version,
        "ref_scheme": local.ref_scheme, "ref_version": local.ref_version, "plan_forms": local.plan_forms, "abi_name": OPERATION_ABI_NAME,
        "supported_handshake": [
            "semantic_contract_digest", "semantic_contract_version",
            "raw_worker_version", "expect_surface"],
    })
}

#[inline]
fn handshake_mismatch(field: &str, peer: &str, local: &str) -> DomainError {
    DomainError::incompatible_contract(format!(
        "{field} mismatch: peer expected {peer}, local is {local}"
    ))
}

/// Validate a peer handshake request against local capability.
///
/// Fails **before** any domain execution when digests/versions/surfaces mismatch.
/// Error messages include actionable local + expected values (no catalog dump).
pub fn validate_handshake(
    local: &SurfaceCapability,
    request: &HandshakeRequest,
) -> Result<HandshakeAck, DomainError> {
    if local.schema != SURFACE_MANIFEST_SCHEMA {
        return Err(DomainError::incompatible_contract(format!(
            "local surface schema {} is not {SURFACE_MANIFEST_SCHEMA}",
            local.schema
        )));
    }

    let Some(ref expected) = request.semantic_contract_digest else {
        return Err(DomainError::invalid_argument(
            "handshake requires semantic_contract_digest so mismatched peers fail closed before execution",
        ));
    };
    let expected = expected.trim().to_ascii_lowercase();
    if expected != local.semantic_contract_digest {
        return Err(DomainError::incompatible_contract(format!(
            "semantic contract digest mismatch: peer expected {expected}, local is {}; upgrade/downgrade one side or regenerate vectors (contract {} {})",
            local.semantic_contract_digest,
            local.semantic_contract_name,
            local.semantic_contract_version
        )));
    }

    if let Some(ref ver) = request.semantic_contract_version
        && ver != &local.semantic_contract_version
    {
        return Err(handshake_mismatch(
            "semantic contract version",
            ver,
            &local.semantic_contract_version,
        ));
    }

    if let Some(ref ver) = request.raw_worker_version
        && ver != &local.raw_worker_version
    {
        return Err(handshake_mismatch(
            "raw_worker_version",
            ver,
            &local.raw_worker_version,
        ));
    }

    if let Some(expect) = request.expect_surface
        && expect != local.surface
    {
        return Err(DomainError::incompatible_contract(format!(
            "surface mismatch: peer expected {}, local installation is {}",
            expect.as_str(),
            local.surface.as_str()
        )));
    }

    // Apply peer-declared ownership for honest traces (composition truth).
    let mut capability = local.clone();
    if let Some(p) = request.planner_owner {
        capability.planner_owner = p;
    }
    if let Some(c) = request.compression_owner {
        capability.compression_owner = c;
    }
    // Raw worker path never advertises plan forms after composition handshake.
    if matches!(
        capability.planner_owner,
        Ownership::Client | Ownership::OuterRouter | Ownership::None
    ) {
        capability.plan_forms.clear();
    }

    Ok(HandshakeAck {
        ok: true,
        capability: capability.clone(),
        compatibility: compatibility_payload(&capability),
    })
}

/// Session state for the private raw worker protocol.
#[derive(Debug)]
pub struct PrivateRawWorker {
    local: SurfaceCapability,
    session: Option<SurfaceCapability>,
    calls_after_handshake: AtomicU32,
}

impl PrivateRawWorker {
    fn from_local(local: SurfaceCapability) -> Self {
        Self {
            local,
            session: None,
            calls_after_handshake: AtomicU32::new(0),
        }
    }

    /// Create a worker bound to the installation surface with default ownership.
    pub fn new(surface: SelectedSurface) -> Self {
        Self::from_local(local_capability(surface, Ownership::None, Ownership::None))
    }

    /// Client-native CodeMode composition (one client planner + one final serialize).
    pub fn for_client_native(surface: SelectedSurface) -> Self {
        Self::from_local(client_native_raw_worker_capability(surface))
    }

    /// Outer ZeroStack router composition.
    pub fn for_outer_router(surface: SelectedSurface) -> Self {
        Self::from_local(outer_router_raw_worker_capability(surface))
    }

    pub fn local_capability(&self) -> &SurfaceCapability {
        &self.local
    }

    pub fn is_handshook(&self) -> bool {
        self.session.is_some()
    }

    pub fn session_capability(&self) -> Option<&SurfaceCapability> {
        self.session.as_ref()
    }

    /// Perform handshake. Must succeed before [`Self::call`].
    pub fn handshake(&mut self, request: &HandshakeRequest) -> Result<HandshakeAck, DomainError> {
        let ack = validate_handshake(&self.local, request)?;
        self.session = Some(ack.capability.clone());
        self.calls_after_handshake.store(0, Ordering::SeqCst);
        Ok(ack)
    }

    fn require_session(&self) -> Result<&SurfaceCapability, DomainError> {
        self.session.as_ref().ok_or_else(|| {
            DomainError::permission_denied(
                "private raw worker requires successful handshake before call \
(semantic_contract_digest must match)",
            )
        })
    }

    fn build_trace(&self, cap: &SurfaceCapability) -> WorkerTrace {
        WorkerTrace {
            planner_owner: cap.planner_owner.as_str().into(),
            compression_owner: cap.compression_owner.as_str().into(),
            surface: cap.surface.as_str().into(),
            contract_digest: cap.semantic_contract_digest.clone(),
            // Raw worker: single domain boundary (no MCP re-entry, no nested plan).
            boundary_count: 1,
            raw_worker_version: cap.raw_worker_version.clone(),
        }
    }

    fn trace_telemetry(trace: &WorkerTrace) -> Value {
        json!({
            "planner_owner": trace.planner_owner, "compression_owner": trace.compression_owner,
            "surface": trace.surface, "contract_digest": trace.contract_digest,
            "boundary_count": trace.boundary_count, "raw_worker_version": trace.raw_worker_version,
        })
    }

    /// Execute one domain operation after handshake.
    ///
    /// Invokes the typed dispatcher only. Does not start a sandbox, parse JS,
    /// plan, compact, or re-enter FastMCP.
    pub fn call(
        &self,
        session: &mut FSZeroSession,
        op: &str,
        args: &Value,
    ) -> Result<(DomainResult, WorkerTrace, Value), DomainError> {
        let cap = self.require_session()?;

        if is_forbidden_worker_op(op) {
            return Err(DomainError::permission_denied(format!(
                "private raw worker refuses plan/sandbox op '{op}'; use the typed domain operation name only"
            )));
        }

        let outcome: DispatchOutcome = dispatch_raw_worker(session, op, args);
        self.calls_after_handshake.fetch_add(1, Ordering::SeqCst);
        let trace = self.build_trace(cap);
        if matches!(
            cap.planner_owner,
            Ownership::Client | Ownership::OuterRouter
        ) && trace.boundary_count != 1
        {
            return Err(DomainError::internal(
                "raw worker boundary_count invariant violated",
            ));
        }
        let telemetry = Self::trace_telemetry(&trace);
        if !outcome.result.ok {
            if let Some(err) = outcome.result.error.clone() {
                return Err(err);
            }
            return Err(DomainError::internal(format!(
                "raw worker op '{op}' failed without typed error"
            )));
        }
        // Inline recovered payload for composition clients (install smoke / outer
        // routers) so exact bytes are available without a second process hop.
        let mut result = outcome.result;
        let recovery_key_borrowed = outcome.recovery_key.as_deref();
        if let Some(key) = recovery_key_borrowed.or_else(|| result.refs.first().map(String::as_str))
        {
            if let Some(bytes) = session.expand(key) {
                let payload = match String::from_utf8(bytes) {
                    Ok(s) => {
                        let n = s.len();
                        json!({"ref": key, "payload_utf8": s, "bytes_len": n})
                    }
                    Err(e) => {
                        let b = e.into_bytes();
                        let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
                        json!({"ref": key, "payload_hex": hex, "bytes_len": b.len()})
                    }
                };
                result.value = Some(match result.value.take() {
                    Some(Value::Object(mut m)) => {
                        if let Value::Object(p) = payload {
                            m.extend(p);
                        }
                        Value::Object(m)
                    }
                    Some(other) => json!({"prior": other, "recovered": payload}),
                    None => payload,
                });
            }
        }
        Ok((result, trace, telemetry))
    }

    /// Handle one framed request (handshake or call).
    pub fn handle_frame(
        &mut self,
        session: &mut FSZeroSession,
        frame: &WorkerRequestFrame,
    ) -> WorkerResponseFrame {
        match frame {
            WorkerRequestFrame::Handshake { request } => match self.handshake(request) {
                Ok(ack) => WorkerResponseFrame::HandshakeAck { ack },
                Err(error) => WorkerResponseFrame::Error {
                    request_id: None,
                    compatibility: Some(compatibility_payload(&self.local)),
                    error,
                    trace: None,
                },
            },
            WorkerRequestFrame::Call {
                op,
                args,
                request_id,
            } => match self.call(session, op, args) {
                Ok((result, trace, telemetry)) => WorkerResponseFrame::Result {
                    request_id: request_id.clone(),
                    result,
                    trace,
                    telemetry,
                },
                Err(error) => {
                    let trace = self.session.as_ref().map(|c| self.build_trace(c));
                    let compatibility = if error.message.contains("digest")
                        || error.message.contains("handshake")
                    {
                        Some(compatibility_payload(&self.local))
                    } else {
                        None
                    };
                    WorkerResponseFrame::Error {
                        request_id: request_id.clone(),
                        error,
                        trace,
                        compatibility,
                    }
                }
            },
        }
    }

    /// Parse a JSON frame and handle it.
    pub fn handle_json(
        &mut self,
        session: &mut FSZeroSession,
        frame: &Value,
    ) -> Result<WorkerResponseFrame, DomainError> {
        let req: WorkerRequestFrame = serde_json::from_value(frame.clone()).map_err(|e| {
            DomainError::invalid_argument(format!("invalid private worker frame: {e}"))
        })?;
        Ok(self.handle_frame(session, &req))
    }

    pub fn calls_after_handshake(&self) -> u32 {
        self.calls_after_handshake.load(Ordering::SeqCst)
    }
}

fn is_forbidden_worker_op(op: &str) -> bool {
    matches!(
        op,
        "execute_code"
            | "fz_execute_code"
            | "gz_execute_code"
            | "codemode_search"
            | "fz_codemode_search"
            | "gz_codemode_search"
            | "codemode_describe"
            | "fz_codemode_describe"
            | "gz_codemode_describe"
            | "tools/call"
            | "tools/list"
            | "fszero.exec"
    )
}

/// Handshake-gated private worker dispatch helper used by composition callers.
pub fn private_worker_dispatch_checked(
    session: &mut FSZeroSession,
    op: &str,
    args: &Value,
    expected_digest: &str,
    surface: SelectedSurface,
) -> Result<(DomainResult, WorkerTrace), DomainError> {
    let mut worker = PrivateRawWorker::for_client_native(surface);
    let req = HandshakeRequest {
        semantic_contract_digest: Some(expected_digest.into()),
        semantic_contract_version: Some(OPERATION_ABI_VERSION.into()),
        raw_worker_version: Some(RAW_WORKER_VERSION.into()),
        expect_surface: Some(surface),
        planner_owner: Some(Ownership::Client),
        compression_owner: Some(Ownership::Client),
    };
    worker.handshake(&req)?;
    worker.call(session, op, args).map(|(r, t, _)| (r, t))
}

/// Assert the private-worker module never embeds sandbox/runtime creation.
///
/// Used by packaging and integration tests (static source invariant).
pub fn private_worker_source_forbids_sandbox() -> bool {
    let src = include_str!("surface_handshake.rs");
    // Reject real sandbox/runtime wiring. Needles are split so this function body
    // does not match itself. String literals that *name* forbidden plan ops (for
    // policy rejection) are allowed; imports and Runtime construction are not.
    let needles = [
        ["rqui", "ckjs"].concat(),
        ["Runtime", "::new"].concat(),
        ["execute_js", "_plan"].concat(),
        ["use crate::", "codemode"].concat(),
        ["use crate::", "mcp_protocol"].concat(),
    ];
    needles.iter().all(|n| !src.contains(n))
}
