//! In-process GraphZero domain adapter over the embedded engine.
//!
//! [`GraphZeroAdapter`] implements the [`DomainAdapter`] contract for the
//! GraphZero engine by calling the canonical in-process dispatcher
//! (`graphzero_query::private_worker_dispatch` — the same entry the
//! raw-worker-v2 path funnels through) with an [`EngineContext`] derived from
//! the embedded store/repo roots, and by converting the typed
//! `DomainResult` / `DomainError` outcomes into the [`WorkerResult`] envelope
//! the aggregate connector validates. No raw-worker framing crosses this
//! boundary: the module contains no `Command::spawn`, no NDJSON codec, no
//! socket, no MCP transport, and no CodeMode runtime.
//!
//! Identity mirrors the GraphZero raw-worker-v2 binding
//! (`graphzero_query::surface_handshake::v2::RawWorkerV2`):
//!
//! - engine [`EngineIdentity::GraphZero`], ref scheme `gz://`;
//! - `worker_revision` from `ZEROSTACK_WORKER_REVISION` with the
//!   `graphzero-query` crate version fallback;
//! - `semantic_contract_version` from `SEMANTIC_CONTRACT_VERSION`;
//! - both digests from [`contract_digest_hex`] (the v2 worker binds the
//!   operation-registry digest to the same contract digest).
//!
//! Outcome conversion mirrors the v2 worker frame mapping: `ReadOnly` ops
//! report `EffectClass::ReadOnly`, `StoreOnly` (and unresolvable) ops report
//! `EffectClass::Irreversible`, approvals and revert are never claimed, and
//! `DomainResult.refs` pass through verbatim as engine-owned refs. The
//! response echoes `request.trace` verbatim, as the connector requires.
//!
//! Cancellation and deadline are checked before dispatch; the connector
//! re-checks both at every boundary. In-flight preemption stays with the
//! dispatcher, which reports typed `cancelled` / `deadline_exceeded` errors
//! with the domain retryability taxonomy.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use graphzero_query::dispatcher::{AdapterKind, EngineContext};
use graphzero_query::operation_abi::Mutability;
use graphzero_query::surface_handshake::v2::worker_revision;
use graphzero_query::{
    EmbeddedGraphZero, SEMANTIC_CONTRACT_VERSION, contract_digest_hex, private_worker_dispatch,
    resolve_operation,
};
use zero_abi::{
    ApprovalMetadata, ApprovalState, CallRequest, EffectClass, EngineIdentity, RefOwnership,
    RevertMetadata, WorkerError, WorkerResult, WorkerResultMetadata,
};

use crate::adapter::{AdapterBinding, AdapterCall, AdapterError, AdapterResponse, DomainAdapter};
use crate::connector::now_ms;

/// In-process GraphZero domain adapter.
///
/// Owns one [`EmbeddedGraphZero`] handle (store/repo roots, shared CAS) and
/// dispatches every canonical call through the embedded engine's
/// [`EngineContext`] + registry dispatcher. `Clone` is cheap; the connector
/// serializes calls per engine.
#[derive(Clone, Debug)]
pub struct GraphZeroAdapter {
    embedded: EmbeddedGraphZero,
    session_id: String,
    binding: AdapterBinding,
}

impl GraphZeroAdapter {
    /// Build a GraphZero adapter for one session.
    ///
    /// `repo_root` is the session root the connector authorizes. The store
    /// root defaults to `<repo_root>/.graphzero`, matching the raw-worker
    /// default (`GRAPHZERO_STORE` defaults to `<repo>/.graphzero`).
    pub fn new(repo_root: impl Into<PathBuf>, session_id: impl Into<String>) -> Self {
        let repo_root = repo_root.into();
        let embedded = EmbeddedGraphZero::new(repo_root.join(".graphzero"), Some(repo_root));
        let session_id = session_id.into();
        let binding = AdapterBinding::new(
            EngineIdentity::GraphZero,
            worker_revision(),
            SEMANTIC_CONTRACT_VERSION,
            contract_digest_hex(),
            contract_digest_hex(),
            "gz://",
        )
        .expect("graphzero adapter binding is valid");
        Self {
            embedded,
            session_id,
            binding,
        }
    }

    /// The embedded engine handle (store/repo access, blob publication).
    pub fn embedded(&self) -> &EmbeddedGraphZero {
        &self.embedded
    }

    /// The GraphZero store root for this adapter.
    pub fn store_root(&self) -> &Path {
        self.embedded.store_root()
    }

    /// The repo root this adapter indexes, when one was configured.
    pub fn repo_root(&self) -> Option<&Path> {
        self.embedded.repo_root()
    }

    /// The session id this adapter reports in result ownership.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Execution context for one canonical call.
    ///
    /// Converts the absolute wall-clock `deadline_unix_ms` into the
    /// dispatcher's monotonic `Instant` budget; a deadline already in the
    /// past is rejected by [`DomainAdapter::call`] before this is built.
    fn engine_context(&self, request: &CallRequest) -> EngineContext {
        let mut context = EngineContext::for_paths(
            self.embedded
                .repo_root()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".")),
            self.embedded.store_root().to_path_buf(),
            AdapterKind::PrivateWorker,
        );
        if let Some(deadline_unix_ms) = request.deadline_unix_ms
            && let Some(remaining_ms) = deadline_unix_ms.checked_sub(now_ms())
        {
            context = context.with_deadline(Instant::now() + Duration::from_millis(remaining_ms));
        }
        context
    }
}

impl DomainAdapter for GraphZeroAdapter {
    fn engine(&self) -> EngineIdentity {
        EngineIdentity::GraphZero
    }

    fn binding(&self) -> AdapterBinding {
        self.binding.clone()
    }

    fn call(&self, call: AdapterCall<'_>) -> Result<AdapterResponse, AdapterError> {
        let request = call.request;
        let trace = request.trace.clone();
        // Preflight: stop before any dispatch when the session cancelled or
        // the wall deadline already expired. The connector re-checks both at
        // every boundary; these mirror the v2 worker's pre-dispatch refusal.
        if call.cancellation.is_cancelled() {
            return Err(AdapterError::new(
                "cancelled",
                "graphzero adapter cancelled before dispatch",
                false,
                Some(trace),
            ));
        }
        if request.deadline_expired(now_ms()) {
            return Err(AdapterError::new(
                "deadline_exceeded",
                "graphzero adapter deadline expired before dispatch",
                false,
                Some(trace),
            ));
        }
        let context = self.engine_context(request);
        match private_worker_dispatch(&context, &request.op, &request.args) {
            Ok(result) => Ok(AdapterResponse {
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
                            session_id: self.session_id.clone(),
                            refs: result.refs,
                            snapshot: None,
                        },
                        trace,
                    },
                },
                engine_timeline: None,
                worker_token_accounting: None,
            }),
            Err(error) => Err(AdapterError {
                error: Box::new(WorkerError {
                    kind: error.kind.as_str().into(),
                    message: error.message,
                    retryable: error.retryable,
                    details: None,
                }),
                trace: Some(Box::new(trace)),
                engine_timeline: None,
                worker_token_accounting: None,
            }),
        }
    }
}

/// Effect class for one canonical operation, mirroring the raw-worker-v2
/// classification (`graphzero_query::surface_handshake::v2::effect_class_for_op`).
fn effect_class_for_op(op: &str) -> EffectClass {
    match resolve_operation(op).map(|operation| operation.mutability) {
        Some(Mutability::ReadOnly) => EffectClass::ReadOnly,
        Some(Mutability::StoreOnly) | None => EffectClass::Irreversible,
    }
}
