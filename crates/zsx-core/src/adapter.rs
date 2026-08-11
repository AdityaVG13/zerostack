//! Safe in-process domain-adapter contract, owned by zsx-core.
//!
//! This is the one contract an engine repository (FSZero, GraphZero, or
//! TokenZero) implements to run inside the single-process ZSX composition.
//! The engine keeps its domain APIs and operation registry; it must NOT bring
//! CodeMode, MCP, FastMCP, a JavaScript runtime, or any harness authority.
//!
//! A `DomainAdapter` receives canonical raw-worker-v2 [`CallRequest`]s
//! directly in memory: no worker process, no NDJSON framing, and no session
//! socket. The aggregate connector in this crate performs every worker-side
//! validation the process path used to rely on (approval grant consumption,
//! typed frame validation, result binding, ref reachability, telemetry
//! validation), so an adapter cannot weaken the boundary by accident.
//!
//! # Implementing an adapter
//!
//! ```ignore
//! use std::sync::Arc;
//! use zsx_core::{AdapterBinding, AdapterCall, AdapterError, AdapterResponse, DomainAdapter};
//! use zero_abi::{EngineIdentity, WorkerResult, /* ... */};
//!
//! #[derive(Clone)]
//! pub struct FsAdapter { /* engine-owned state */ }
//!
//! impl DomainAdapter for FsAdapter {
//!     fn engine(&self) -> EngineIdentity { EngineIdentity::FsZero }
//!
//!     fn binding(&self) -> AdapterBinding {
//!         AdapterBinding {
//!             engine: EngineIdentity::FsZero,
//!             worker_revision: env!("CARGO_PKG_VERSION").into(),
//!             semantic_contract_version: "fszero.codemode.v1".into(),
//!             semantic_contract_digest: <64-lower-hex>.into(),
//!             operation_registry_digest: <64-lower-hex>.into(),
//!             ref_scheme: "fz://".into(),
//!         }
//!     }
//!
//!     fn call(&self, call: AdapterCall<'_>) -> Result<AdapterResponse, AdapterError> {
//!         // Honor call.cancellation and call.request.deadline_unix_ms.
//!         // Echo call.request.trace verbatim in the response metadata.
//!         // Return refs owned by this engine; the core verifies each ref
//!         // against the shared CAS and retains it for GC reachability.
//!         # todo!()
//!     }
//! }
//!
//! let session = zsx_core::ZsxSession::builder("/repo")
//!     .fszero(Arc::new(FsAdapter::new()))
//!     .graphzero(Arc::new(graph_adapter))
//!     .tokenzero(Arc::new(token_adapter))
//!     .build()?;
//! ```
//!
//! # Contract obligations (enforced by the core)
//!
//! - `engine()` must match the builder slot it is registered into.
//! - `binding()` digests must be 64-character lowercase hex.
//! - `call()` must stop at [`AdapterCall::cancellation`] and at
//!   `request.deadline_unix_ms`; the core re-checks both at every boundary.
//! - The response `result.metadata.trace` must equal `request.trace` exactly.
//! - `result.metadata.ownership.engine` must equal the adapter engine and
//!   `ownership.session_id` must equal the session id given to the builder.
//! - Every ref in `ownership.refs` must be a canonical
//!   `<ref_scheme>blob/<64-hex>` ref owned by the adapter engine and present
//!   in the shared CAS; the core verifies and retains each one.
//! - Approval-required operations must return `approval.state == Granted`
//!   only when the request carries a validated grant; the core consumes the
//!   grant before the call and fails `Required`/`Denied` results closed.
//! - Optional `engine_timeline` and `worker_token_accounting` payloads must
//!   pass the zero-abi typed validators; the core validates them.

use std::fmt;

use zero_abi::{
    CallRequest, EngineIdentity, EngineStageTimelineV1, WorkerError, WorkerResult,
    WorkerTokenAccountingV1, WorkerTrace,
};
use zero_codemode::worker::CancellationSignal;

/// Immutable identity a domain adapter advertises for one session.
///
/// Field names and semantics mirror the raw-worker-v2 `WorkerBinding` so an
/// in-process adapter stays byte-compatible with the process contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterBinding {
    pub engine: EngineIdentity,
    /// Engine-owned revision string, surfaced in worker traces.
    pub worker_revision: String,
    /// Engine-owned semantic contract version (e.g. `fszero.codemode.v1`).
    pub semantic_contract_version: String,
    /// 64-character lowercase hex semantic contract digest.
    pub semantic_contract_digest: String,
    /// 64-character lowercase hex operation-registry digest.
    pub operation_registry_digest: String,
    /// Canonical ref scheme prefix, e.g. `fz://`.
    pub ref_scheme: String,
}

impl AdapterBinding {
    /// Fail-fast construction: rejects empty identities and non-hex digests.
    pub fn new(
        engine: EngineIdentity,
        worker_revision: impl Into<String>,
        semantic_contract_version: impl Into<String>,
        semantic_contract_digest: impl Into<String>,
        operation_registry_digest: impl Into<String>,
        ref_scheme: impl Into<String>,
    ) -> Result<Self, AdapterContractError> {
        let binding = Self {
            engine,
            worker_revision: worker_revision.into(),
            semantic_contract_version: semantic_contract_version.into(),
            semantic_contract_digest: semantic_contract_digest.into(),
            operation_registry_digest: operation_registry_digest.into(),
            ref_scheme: ref_scheme.into(),
        };
        binding.validate()?;
        Ok(binding)
    }

    /// Validate identity fields; the connector re-validates at registration.
    pub fn validate(&self) -> Result<(), AdapterContractError> {
        let lower_hex = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        };
        if self.worker_revision.is_empty() {
            return Err(AdapterContractError::Empty("worker_revision"));
        }
        if self.semantic_contract_version.is_empty() {
            return Err(AdapterContractError::Empty("semantic_contract_version"));
        }
        if !lower_hex(&self.semantic_contract_digest) {
            return Err(AdapterContractError::Digest(
                "semantic_contract_digest",
                self.semantic_contract_digest.clone(),
            ));
        }
        if !lower_hex(&self.operation_registry_digest) {
            return Err(AdapterContractError::Digest(
                "operation_registry_digest",
                self.operation_registry_digest.clone(),
            ));
        }
        if !self.ref_scheme.ends_with("://") {
            return Err(AdapterContractError::RefScheme(self.ref_scheme.clone()));
        }
        Ok(())
    }
}

/// Typed failure for invalid adapter identity metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdapterContractError {
    Empty(&'static str),
    Digest(&'static str, String),
    RefScheme(String),
}

impl fmt::Display for AdapterContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty(field) => write!(f, "adapter binding {field} must be non-empty"),
            Self::Digest(field, value) => write!(
                f,
                "adapter binding {field} must be 64-character lowercase hex, got {value:?}"
            ),
            Self::RefScheme(scheme) => {
                write!(
                    f,
                    "adapter binding ref_scheme must end in ://, got {scheme:?}"
                )
            }
        }
    }
}

impl std::error::Error for AdapterContractError {}

/// One in-process dispatch.
///
/// The connector owns the request and validates it before the call; the
/// adapter must honor [`Self::cancellation`] and `request.deadline_unix_ms`.
#[derive(Debug)]
pub struct AdapterCall<'a> {
    pub request: &'a CallRequest,
    /// Per-request cancellation token for the request that owns this
    /// dispatch. It shares one flag with the host runtime, so cancelling the
    /// request (or the session) stops the adapter call too; it is never a
    /// whole-session signal.
    pub cancellation: &'a CancellationSignal,
}

/// A validated in-process adapter result.
///
/// `result` is the same [`WorkerResult`] the raw-worker-v2 process path
/// returns; the connector binds, validates, and retains it identically.
#[derive(Clone, Debug)]
pub struct AdapterResponse {
    pub result: WorkerResult,
    /// Optional transport telemetry; must pass `validate_engine_stage_timeline_v1`.
    pub engine_timeline: Option<EngineStageTimelineV1>,
    /// Optional transport telemetry; must pass `validate_worker_token_accounting_v1`.
    pub worker_token_accounting: Option<WorkerTokenAccountingV1>,
}

/// A typed in-process adapter failure, mirroring `WorkerResponseFrame::Error`.
///
/// Payload fields are boxed so the `Err` variant stays small; field access
/// auto-derefs, so `error.error.kind` works as expected.
#[derive(Clone, Debug)]
pub struct AdapterError {
    pub error: Box<WorkerError>,
    pub trace: Option<Box<WorkerTrace>>,
    pub engine_timeline: Option<Box<EngineStageTimelineV1>>,
    pub worker_token_accounting: Option<Box<WorkerTokenAccountingV1>>,
}

impl AdapterError {
    /// Build a minimal typed error for a cancelled or failed operation.
    pub fn new(
        kind: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
        trace: Option<WorkerTrace>,
    ) -> Self {
        Self {
            error: Box::new(WorkerError {
                kind: kind.into(),
                message: message.into(),
                retryable,
                details: None,
            }),
            trace: trace.map(Box::new),
            engine_timeline: None,
            worker_token_accounting: None,
        }
    }
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.error.kind, self.error.message)
    }
}

impl std::error::Error for AdapterError {}

/// Safe in-process domain dispatch contract.
///
/// Implementations must be `Send + Sync`; the aggregate connector calls them
/// from a small fixed pool of dispatcher threads, one call at a time per
/// engine, with a bounded admission channel.
pub trait DomainAdapter: Send + Sync {
    /// The single engine this adapter serves. Must match the builder slot.
    fn engine(&self) -> EngineIdentity;

    /// Immutable identity for this session.
    fn binding(&self) -> AdapterBinding;

    /// Dispatch one canonical operation in-process.
    ///
    /// Return `Err` for a typed adapter failure (the connector rejects the
    /// completion); return `Ok` with a fully bound [`AdapterResponse`].
    fn call(&self, call: AdapterCall<'_>) -> Result<AdapterResponse, AdapterError>;
}
