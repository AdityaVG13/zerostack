//! Capability handshake and private raw worker protocol (graphzero-o2uq.4).
//!
//! Shared `zerostack.surface` manifest fields are identical across GraphZero,
//! TokenZero, and peer ZeroStack components. The private raw worker is an
//! **internal** mode of the selected FastMCP or CodeMode artifact — never a
//! third user-facing installation. It invokes the typed domain dispatcher
//! directly and never plans, parses JavaScript, starts a sandbox, or rewrites
//! envelopes.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::dispatcher::{
    AdapterKind, DispatchOutcome, EngineContext, private_worker_dispatch as raw_dispatch,
};
use crate::operation_abi::{
    DomainError, DomainErrorKind, DomainResult, SEMANTIC_CONTRACT_VERSION, contract_digest_hex,
};

/// Shared ZeroStack surface capability schema id.
pub const SURFACE_MANIFEST_SCHEMA: &str = "zerostack.surface";
/// Semantic contract family name (stable across digests).
pub const SEMANTIC_CONTRACT_NAME: &str = "graphzero.operation_abi";
/// Private raw worker protocol version.
pub const RAW_WORKER_VERSION: &str = "1.0.0";
/// GraphZero ref scheme.
pub const REF_SCHEME: &str = "gz";
/// GraphZero ref protocol major version.
pub const REF_VERSION: &str = "1";

/// Selected user-facing installation surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectedSurface {
    /// Lean per-operation FastMCP (`graphzero-mcp`).
    Mcp,
    /// Server-side CodeMode (`graphzero-codemode`).
    Codemode,
}

impl SelectedSurface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
            Self::Codemode => "codemode",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "mcp" | "fastmcp" => Some(Self::Mcp),
            "codemode" | "code_mode" => Some(Self::Codemode),
            _ => None,
        }
    }
}

/// Who owns planning or compression for this composition.
///
/// Client-native CodeMode + raw GraphZero worker: both owners are `Client`.
/// ZeroStack outer router composing GraphZero: both owners are `OuterRouter`.
/// Server CodeMode artifact: both owners are `Server`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ownership {
    Client,
    Server,
    OuterRouter,
    /// Raw FastMCP endpoint: no planner layer on the server.
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
    /// Registry digest; GraphZero equates this with the contract digest today.
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
    /// Actionable peer-facing compatibility summary (local values).
    pub compatibility: Value,
}

/// Per-call trace fields required by the epic (planner/compression ownership).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkerTrace {
    pub planner_owner: String,
    pub compression_owner: String,
    pub surface: String,
    pub contract_digest: String,
    /// Domain↔adapter boundary crossings for this call (always 1 on the raw path).
    pub boundary_count: u32,
    pub raw_worker_version: String,
}

/// Inbound private-worker frames (stable JSON protocol).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerRequestFrame {
    Handshake {
        #[serde(default)]
        request: HandshakeRequest,
    },
    Call {
        op: String,
        #[serde(default)]
        args: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        /// Cooperative cancel for this call only (does not stick on the process).
        #[serde(default)]
        cancelled: bool,
        /// When true, set a past deadline so the call returns `deadline_exceeded`.
        #[serde(default)]
        deadline_exceeded: bool,
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
    json!({
        "max_logical_ops": 1000,
        "max_output_bytes": 65536,
        "max_code_bytes": 65536,
        "cancellation": "cooperative_or_deadline",
    })
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
        // Server owns CodeMode planning on the codemode artifact.
        (SelectedSurface::Codemode, Ownership::Server) => {
            vec!["recipe".into(), "json".into(), "js".into()]
        }
        // Raw path / client-native / outer router: no plan forms on this process.
        _ => Vec::new(),
    };
    SurfaceCapability {
        schema: SURFACE_MANIFEST_SCHEMA.into(),
        surface,
        planner_owner,
        compression_owner,
        semantic_contract_name: SEMANTIC_CONTRACT_NAME.into(),
        semantic_contract_version: SEMANTIC_CONTRACT_VERSION.into(),
        semantic_contract_digest: digest.clone(),
        operation_registry_digest: digest,
        ref_scheme: REF_SCHEME.into(),
        ref_version: REF_VERSION.into(),
        plan_forms,
        raw_worker_version: RAW_WORKER_VERSION.into(),
        cancellation: true,
        transactions: false,
        streaming: false,
        limits: default_worker_limits(),
    }
}

/// Capability for client-native CodeMode composing GraphZero as a raw worker
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
        "schema": local.schema,
        "surface": local.surface.as_str(),
        "semantic_contract_name": local.semantic_contract_name,
        "semantic_contract_version": local.semantic_contract_version,
        "semantic_contract_digest": local.semantic_contract_digest,
        "operation_registry_digest": local.operation_registry_digest,
        "raw_worker_version": local.raw_worker_version,
        "ref_scheme": local.ref_scheme,
        "ref_version": local.ref_version,
        "plan_forms": local.plan_forms,
        "supported_handshake": ["semantic_contract_digest", "semantic_contract_version", "raw_worker_version", "expect_surface"],
    })
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
        return Err(DomainError::new(
            DomainErrorKind::Policy,
            format!(
                "local surface schema {} is not {SURFACE_MANIFEST_SCHEMA}",
                local.schema
            ),
        ));
    }

    if let Some(ref expected) = request.semantic_contract_digest {
        let expected = expected.trim().to_ascii_lowercase();
        if expected != local.semantic_contract_digest {
            return Err(DomainError::new(
                DomainErrorKind::Policy,
                format!(
                    "semantic contract digest mismatch: peer expected {expected}, local is {}; \
upgrade/downgrade one side or regenerate vectors (contract {} {})",
                    local.semantic_contract_digest,
                    local.semantic_contract_name,
                    local.semantic_contract_version
                ),
            )
            .with_op("handshake"));
        }
    } else {
        return Err(DomainError::new(
            DomainErrorKind::Validation,
            "handshake requires semantic_contract_digest so mismatched peers fail closed \
before execution",
        )
        .with_op("handshake"));
    }

    if let Some(ref ver) = request.semantic_contract_version
        && ver != &local.semantic_contract_version
    {
        return Err(DomainError::new(
            DomainErrorKind::Policy,
            format!(
                "semantic contract version mismatch: peer expected {ver}, local is {}",
                local.semantic_contract_version
            ),
        )
        .with_op("handshake"));
    }

    if let Some(ref ver) = request.raw_worker_version
        && ver != &local.raw_worker_version
    {
        return Err(DomainError::new(
            DomainErrorKind::Policy,
            format!(
                "raw_worker_version mismatch: peer expected {ver}, local is {}",
                local.raw_worker_version
            ),
        )
        .with_op("handshake"));
    }

    if let Some(expect) = request.expect_surface
        && expect != local.surface
    {
        return Err(DomainError::new(
            DomainErrorKind::Policy,
            format!(
                "surface mismatch: peer expected {}, local installation is {}",
                expect.as_str(),
                local.surface.as_str()
            ),
        )
        .with_op("handshake"));
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
    /// Create a worker bound to the installation surface with default ownership.
    pub fn new(surface: SelectedSurface) -> Self {
        // Default: raw path with no server planner (FastMCP-style composition).
        let local = local_capability(surface, Ownership::None, Ownership::None);
        Self {
            local,
            session: None,
            calls_after_handshake: AtomicU32::new(0),
        }
    }

    /// Client-native CodeMode composition (one client planner + one final serialize).
    pub fn for_client_native(surface: SelectedSurface) -> Self {
        Self {
            local: client_native_raw_worker_capability(surface),
            session: None,
            calls_after_handshake: AtomicU32::new(0),
        }
    }

    /// Outer ZeroStack router composition.
    pub fn for_outer_router(surface: SelectedSurface) -> Self {
        Self {
            local: outer_router_raw_worker_capability(surface),
            session: None,
            calls_after_handshake: AtomicU32::new(0),
        }
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
            DomainError::new(
                DomainErrorKind::Policy,
                "private raw worker requires successful handshake before call \
(semantic_contract_digest must match)",
            )
            .with_op("private_worker")
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

    /// Execute one domain operation after handshake.
    ///
    /// Invokes the typed dispatcher only. Does not start a sandbox, parse JS,
    /// plan, compact, or re-enter FastMCP.
    pub fn call(
        &self,
        ctx: &EngineContext,
        op: &str,
        args: &Value,
    ) -> Result<(DomainResult, WorkerTrace), DomainError> {
        let cap = self.require_session()?;
        debug_assert_eq!(ctx.adapter, AdapterKind::PrivateWorker);

        // Reject plan / sandbox meta ops at the worker edge (defense in depth).
        if is_forbidden_worker_op(op) {
            return Err(DomainError::new(
                DomainErrorKind::Policy,
                format!(
                    "private raw worker refuses plan/sandbox op '{op}'; use the typed \
domain operation name only"
                ),
            )
            .with_op(op));
        }

        let outcome = raw_dispatch(ctx, op, args);
        self.calls_after_handshake.fetch_add(1, Ordering::SeqCst);
        let mut trace = self.build_trace(cap);
        match outcome {
            Ok(mut result) => {
                // Attach ownership telemetry without changing domain value semantics.
                let tele = json!({
                    "planner_owner": trace.planner_owner,
                    "compression_owner": trace.compression_owner,
                    "surface": trace.surface,
                    "contract_digest": trace.contract_digest,
                    "boundary_count": trace.boundary_count,
                    "raw_worker_version": trace.raw_worker_version,
                });
                result.telemetry = Some(match result.telemetry.take() {
                    Some(Value::Object(mut m)) => {
                        if let Value::Object(t) = tele {
                            for (k, v) in t {
                                m.insert(k, v);
                            }
                        }
                        Value::Object(m)
                    }
                    Some(other) => json!({"prior": other, "worker": tele}),
                    None => tele,
                });
                // Single planner + single final serializer law for client-native path.
                if matches!(
                    cap.planner_owner,
                    Ownership::Client | Ownership::OuterRouter
                ) && trace.boundary_count != 1
                {
                    return Err(DomainError::new(
                        DomainErrorKind::Runtime,
                        "raw worker boundary_count invariant violated",
                    )
                    .with_op(op));
                }
                let _ = &mut trace;
                Ok((result, trace))
            }
            Err(e) => Err(e),
        }
    }

    /// Handle one framed request (handshake or call).
    pub fn handle_frame(
        &mut self,
        ctx: &EngineContext,
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
                cancelled,
                deadline_exceeded,
            } => {
                // Per-call preflight flags (shipped stdio artifact must surface
                // typed cancel/deadline without a sticky process-wide context).
                let mut call_ctx = ctx.clone();
                if *cancelled {
                    call_ctx.cancelled = true;
                }
                if *deadline_exceeded {
                    call_ctx.deadline =
                        Some(std::time::Instant::now() - std::time::Duration::from_millis(1));
                }
                match self.call(&call_ctx, op, args) {
                    Ok((result, trace)) => WorkerResponseFrame::Result {
                        request_id: request_id.clone(),
                        result,
                        trace,
                    },
                    Err(error) => {
                        let trace = self.session.as_ref().map(|c| self.build_trace(c));
                        let compatibility = if error.op.as_deref() == Some("handshake")
                            || error.message.contains("digest")
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
                }
            }
        }
    }

    /// Parse a JSON frame and handle it.
    pub fn handle_json(
        &mut self,
        ctx: &EngineContext,
        frame: &Value,
    ) -> Result<WorkerResponseFrame, DomainError> {
        let req: WorkerRequestFrame = serde_json::from_value(frame.clone()).map_err(|e| {
            DomainError::new(
                DomainErrorKind::Validation,
                format!("invalid private worker frame: {e}"),
            )
            .with_op("private_worker")
        })?;
        Ok(self.handle_frame(ctx, &req))
    }

    pub fn calls_after_handshake(&self) -> u32 {
        self.calls_after_handshake.load(Ordering::SeqCst)
    }
}

fn is_forbidden_worker_op(op: &str) -> bool {
    matches!(
        op,
        "execute_code"
            | "gz_execute_code"
            | "codemode_search"
            | "gz_codemode_search"
            | "codemode_describe"
            | "gz_codemode_describe"
            | "tools/call"
            | "tools/list"
    )
}

/// Handshake-gated private worker dispatch helper used by composition callers.
///
/// Prefer [`PrivateRawWorker`] for multi-call sessions. This convenience builds
/// a one-shot client-native session when `expected_digest` is provided.
pub fn private_worker_dispatch_checked(
    ctx: &EngineContext,
    op: &str,
    args: &Value,
    expected_digest: &str,
    surface: SelectedSurface,
) -> DispatchOutcome {
    let mut worker = PrivateRawWorker::for_client_native(surface);
    let req = HandshakeRequest {
        semantic_contract_digest: Some(expected_digest.into()),
        semantic_contract_version: Some(SEMANTIC_CONTRACT_VERSION.into()),
        raw_worker_version: Some(RAW_WORKER_VERSION.into()),
        expect_surface: Some(surface),
        planner_owner: Some(Ownership::Client),
        compression_owner: Some(Ownership::Client),
    };
    worker.handshake(&req)?;
    worker.call(ctx, op, args).map(|(r, _)| r)
}

/// Assert the private-worker module never embeds sandbox/runtime creation.
///
/// Used by packaging and integration tests (static source invariant).
/// Production body only (strip trailing unit-test module) so self-referential
/// string checks in this helper cannot false-positive.
pub fn private_worker_source_forbids_sandbox() -> bool {
    let src = include_str!("surface_handshake.rs");
    let prod = src.split("#[cfg(test)]").next().unwrap_or(src);
    // Build needles without embedding the forbidden tokens as contiguous literals
    // in the production body beyond this helper's own string construction.
    let needles = [
        ["rqui", "ckjs"].concat(),
        ["Runtime", "::", "new"].concat(),
        ["execute", "_with_", "host"].concat(),
        ["use crate", "::", "codemode"].concat(),
    ];
    // Exclude this function body from the scan window: everything before the
    // helper's doc comment start.
    let scan = prod
        .split("/// Assert the private-worker module never embeds")
        .next()
        .unwrap_or(prod);
    needles.iter().all(|n| !scan.contains(n.as_str()))
}

// ---------------------------------------------------------------------------
// Canonical raw-worker protocol v2 (`zerostack.raw_worker`).
//
// Additive alongside the retained v1 private worker above. Canonical contract:
// zero-abi `raw_worker` module with conformance schema
// `raw-worker-v2.schema.json` and fixtures `raw_worker_v2_frames.json`.
// The v2 worker speaks bounded NDJSON only, requires a full
// protocol/root/session/engine/digest binding at handshake, rejects planner,
// JavaScript, and MCP catalog operations, and reports truthful capabilities:
// cancellation is cooperative preflight only, so the worker never claims
// active cancellation (the sidecar may kill the process for that).
// ---------------------------------------------------------------------------

pub mod raw_worker {
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    use crate::operation_abi::{Mutability, resolve_operation};

    use super::{
        EngineContext, SEMANTIC_CONTRACT_VERSION, contract_digest_hex, is_forbidden_worker_op,
        raw_dispatch,
    };

    // The pinned hub crate is the only raw-worker-v2 wire authority. Re-export
    // its exact types and codecs so GraphZero cannot drift through a parallel
    // serde model or a locally recomputed protocol digest.
    pub use zero_abi::{
        ApprovalMetadata, ApprovalState, CallRequest, CancelRequest, DEFAULT_MAX_FRAME_BYTES,
        EffectClass, EngineIdentity, FrameCodecError, HandshakeAck, HandshakeRequest,
        ProtocolLimits, RAW_WORKER_PROTOCOL_VERSION, RefOwnership, RevertMetadata, ShutdownRequest,
        SnapshotIdentity, WorkerBinding, WorkerCapabilities, WorkerError, WorkerRequestFrame,
        WorkerResponseFrame, WorkerResult, WorkerResultMetadata, WorkerTrace, decode_request_frame,
        decode_response_frame, encode_frame, raw_worker_protocol_digest_hex,
        raw_worker_protocol_manifest, validate_handshake_request,
    };

    /// GraphZero engine id used by the domain registry and diagnostics.
    pub const ENGINE: &str = "graphzero";
    /// Canonical v2 scheme. The legacy surface handshake still uses `gz`.
    pub const CANONICAL_REF_SCHEME: &str = "gz://";

    /// Pure revision resolution: explicit env value wins, crate version is the
    /// fallback. Kept pure so tests do not mutate process env.
    pub fn resolve_worker_revision(env_value: Option<&str>) -> String {
        env_value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
    }

    /// Worker revision from ZEROSTACK_WORKER_REVISION with version fallback.
    pub fn worker_revision() -> String {
        resolve_worker_revision(std::env::var("ZEROSTACK_WORKER_REVISION").ok().as_deref())
    }

    pub fn now_unix_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0)
    }

    fn finish_timed_call(
        mut response: WorkerResponseFrame,
        request: &CallRequest,
        started: Instant,
    ) -> WorkerResponseFrame {
        if !request
            .telemetry_request
            .as_ref()
            .is_some_and(|telemetry| telemetry.engine_stage_timeline)
        {
            return response;
        }
        let duration_ns = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
        let duration_ns = duration_ns.max(1);
        let timeline = zero_abi::EngineStageTimeline {
            total_ns: duration_ns,
            spans: vec![zero_abi::EngineStageSpan {
                stage: "graphzero.raw_worker_call".into(),
                start_ns: 0,
                duration_ns,
            }],
        };
        debug_assert!(zero_abi::validate_engine_stage_timeline(&timeline).is_ok());
        match &mut response {
            WorkerResponseFrame::Result {
                engine_timeline, ..
            }
            | WorkerResponseFrame::Error {
                engine_timeline, ..
            } => *engine_timeline = Some(timeline),
            _ => {}
        }
        response
    }

    pub(crate) fn effect_class_for_op(op: &str) -> EffectClass {
        match resolve_operation(op).map(|operation| operation.mutability) {
            Some(Mutability::ReadOnly) => EffectClass::ReadOnly,
            Some(Mutability::StoreOnly) => EffectClass::Irreversible,
            None => EffectClass::Irreversible,
        }
    }

    /// Session state for the canonical v2 raw worker.
    #[derive(Debug)]
    pub struct RawWorker {
        binding: WorkerBinding,
        capabilities: WorkerCapabilities,
        limits: ProtocolLimits,
        protocol_digest: String,
        handshook: bool,
    }

    impl RawWorker {
        pub fn new(root: impl Into<String>, session_id: impl Into<String>) -> Self {
            let digest = contract_digest_hex();
            Self {
                binding: WorkerBinding {
                    engine: EngineIdentity::GraphZero,
                    root: root.into(),
                    session_id: session_id.into(),
                    worker_revision: worker_revision(),
                    semantic_contract_version: SEMANTIC_CONTRACT_VERSION.into(),
                    semantic_contract_digest: digest.clone(),
                    operation_registry_digest: digest,
                    ref_scheme: CANONICAL_REF_SCHEME.into(),
                },
                capabilities: WorkerCapabilities {
                    cancellation: false,
                    deadlines: true,
                    approvals: false,
                    revert: false,
                    snapshots: false,
                },
                limits: ProtocolLimits::default(),
                protocol_digest: raw_worker_protocol_digest_hex(),
                handshook: false,
            }
        }

        pub fn binding(&self) -> &WorkerBinding {
            &self.binding
        }

        pub fn is_handshook(&self) -> bool {
            self.handshook
        }

        fn error_frame(
            request_id: Option<String>,
            kind: impl Into<String>,
            message: impl Into<String>,
            retryable: bool,
        ) -> WorkerResponseFrame {
            WorkerResponseFrame::Error {
                request_id,
                error: WorkerError {
                    kind: kind.into(),
                    message: message.into(),
                    retryable,
                    details: None,
                },
                trace: None,
                engine_timeline: None,
                worker_token_accounting: None,
            }
        }

        fn call_trace(&self, request: &CallRequest) -> WorkerTrace {
            WorkerTrace {
                runtime_id: request.trace.runtime_id.clone(),
                cell_id: request.trace.cell_id.clone(),
                request_id: request.request_id.clone(),
                trace_id: request.trace.trace_id.clone(),
                parent_span_id: request.trace.parent_span_id.clone(),
                worker_revision: self.binding.worker_revision.clone(),
                contract_digest: self.binding.semantic_contract_digest.clone(),
            }
        }

        /// Handle one canonical v2 frame.
        pub fn handle_frame(
            &mut self,
            ctx: &EngineContext,
            frame: &WorkerRequestFrame,
        ) -> WorkerResponseFrame {
            match frame {
                WorkerRequestFrame::Handshake { request } => {
                    match validate_handshake_request(request, &self.binding) {
                        Ok(()) => {
                            self.handshook = true;
                            WorkerResponseFrame::HandshakeAck {
                                ack: HandshakeAck {
                                    protocol_version: RAW_WORKER_PROTOCOL_VERSION.into(),
                                    binding: self.binding.clone(),
                                    capabilities: self.capabilities.clone(),
                                    limits: self.limits.clone(),
                                    protocol_digest: self.protocol_digest.clone(),
                                },
                            }
                        }
                        Err(error) => {
                            Self::error_frame(None, error.kind(), error.to_string(), false)
                        }
                    }
                }
                WorkerRequestFrame::Call { request } => {
                    let started = Instant::now();
                    if !self.handshook {
                        return finish_timed_call(
                            Self::error_frame(
                                Some(request.request_id.clone()),
                                "handshake_required",
                                "v2 call requires a completed handshake binding first",
                                false,
                            ),
                            request,
                            started,
                        );
                    }
                    if request.deadline_expired(now_unix_ms()) {
                        return finish_timed_call(
                            Self::error_frame(
                                Some(request.request_id.clone()),
                                "deadline_exceeded",
                                "call deadline_unix_ms is in the past; refusing to execute",
                                false,
                            ),
                            request,
                            started,
                        );
                    }
                    if request.trace.request_id != request.request_id
                        || request.trace.worker_revision != self.binding.worker_revision
                        || request.trace.contract_digest != self.binding.semantic_contract_digest
                    {
                        return finish_timed_call(
                            WorkerResponseFrame::Error {
                                request_id: Some(request.request_id.clone()),
                                error: WorkerError {
                                    kind: "trace_binding_mismatch".into(),
                                    message: "call trace does not match request/worker binding"
                                        .into(),
                                    retryable: false,
                                    details: None,
                                },
                                trace: Some(request.trace.clone()),
                                engine_timeline: None,
                                worker_token_accounting: None,
                            },
                            request,
                            started,
                        );
                    }
                    if is_forbidden_worker_op(&request.op) {
                        return finish_timed_call(
                            Self::error_frame(
                                Some(request.request_id.clone()),
                                "forbidden_op",
                                format!(
                                    "raw worker v2 refuses planner/JavaScript/MCP op '{}'; use a typed domain operation",
                                    request.op
                                ),
                                false,
                            ),
                            request,
                            started,
                        );
                    }
                    let trace = self.call_trace(request);
                    let response = match raw_dispatch(ctx, &request.op, &request.args) {
                        Ok(result) => WorkerResponseFrame::Result {
                            request_id: request.request_id.clone(),
                            result: WorkerResult {
                                value: result.value,
                                metadata: WorkerResultMetadata {
                                    effect: effect_class_for_op(&request.op),
                                    approval: ApprovalMetadata {
                                        state: ApprovalState::NotRequired,
                                        approval_id: None,
                                        policy: None,
                                    },
                                    revert: RevertMetadata {
                                        supported: false,
                                        journal_id: None,
                                        rollback_op: None,
                                    },
                                    ownership: RefOwnership {
                                        engine: EngineIdentity::GraphZero,
                                        session_id: self.binding.session_id.clone(),
                                        refs: result.refs,
                                        snapshot: None,
                                    },
                                    trace,
                                },
                            },
                            engine_timeline: None,
                            worker_token_accounting: None,
                        },
                        Err(error) => WorkerResponseFrame::Error {
                            request_id: Some(request.request_id.clone()),
                            error: WorkerError {
                                kind: error.kind.as_str().into(),
                                message: error.message,
                                retryable: error.retryable,
                                details: None,
                            },
                            trace: Some(trace),
                            engine_timeline: None,
                            worker_token_accounting: None,
                        },
                    };
                    finish_timed_call(response, request, started)
                }
                WorkerRequestFrame::Cancel { request } => {
                    // Truthful cancellation: nothing in flight is aborted here;
                    // active cancellation belongs to the sidecar (process kill).
                    WorkerResponseFrame::CancelAck {
                        request_id: request.request_id.clone(),
                        cancelled: false,
                    }
                }
                WorkerRequestFrame::Shutdown { .. } => {
                    self.handshook = false;
                    WorkerResponseFrame::ShutdownAck
                }
            }
        }

        /// Handle one raw NDJSON line end to end: bounded decode before parse,
        /// dispatch, then bounded encode before emit.
        pub fn handle_line(&mut self, ctx: &EngineContext, bytes: &[u8]) -> Vec<u8> {
            let response = match decode_request_frame(bytes, DEFAULT_MAX_FRAME_BYTES) {
                Ok(frame) => self.handle_frame(ctx, &frame),
                Err(error) => Self::error_frame(None, error.kind(), error.to_string(), false),
            };
            match encode_frame(&response, DEFAULT_MAX_FRAME_BYTES) {
                Ok(encoded) => encoded,
                Err(error) => {
                    let fallback = Self::error_frame(None, error.kind(), error.to_string(), false);
                    encode_frame(&fallback, DEFAULT_MAX_FRAME_BYTES).unwrap_or_else(|_| {
                        b"{\"kind\":\"error\",\"error\":{\"kind\":\"frame_too_large\",\"message\":\"response exceeds maximum frame bytes\",\"retryable\":false}}\n".to_vec()
                    })
                }
            }
        }
    }
}
