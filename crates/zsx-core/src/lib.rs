//! Single-process canonical ZSX composition core.
//!
//! zsx-core owns the safe in-process [`DomainAdapter`] contract, the
//! aggregate connector that dispatches three registered domain adapters
//! (FSZero, GraphZero, TokenZero) in memory, and the aggregate session
//! authority (generation, bounded queue, approvals, cancellation,
//! replacement, shutdown). It depends on `zero-codemode` only for the
//! confined interpreter host; it never spawns a worker process and never
//! serializes through NDJSON or a session socket.
//!
//! The previous session socket and raw-worker compatibility executables were
//! removed after all native adapters passed the cutover gates.
//!
//! # Quick start
//!
//! ```ignore
//! use std::sync::Arc;
//! use zsx_core::{DomainAdapter, ZsxSession};
//!
//! let session = ZsxSession::builder("/repo")
//!     .fszero(Arc::new(fszero_adapter))
//!     .graphzero(Arc::new(graphzero_adapter))
//!     .tokenzero(Arc::new(tokenzero_adapter))
//!     .build()?;
//! let result = session.execute(1, 1, "return await zero.fs.compound('list', {path:'.'});",
//!     std::time::Duration::from_secs(30))?;
//! ```

#![forbid(unsafe_code)]

mod adapter;
mod connector;
mod continuation;
mod dag_exec;
mod help;
mod lookup;
mod lower;
mod residency;
mod envelope;
mod session;
mod verdict;

/// Real FSZero engine adapter (feature `fszero`), over the immutable FSZero
/// revision API's canonical typed dispatcher (`FSZeroSession` +
/// `dispatch_codemode_method`). No worker process, NDJSON framing, session
/// socket, MCP, or CodeMode runtime is involved.
#[cfg(feature = "fszero")]
pub mod fszero;

/// Real GraphZero engine adapter (feature `graphzero`), over the embedded
/// GraphZero query crate's canonical in-process dispatch.
#[cfg(feature = "graphzero")]
pub mod graphzero;

/// Real TokenZero engine adapter (feature `tokenzero`), over the immutable
/// TokenZero revision API's canonical typed dispatcher
/// (`TokenZeroEngine` + `dispatch_operation`). No worker process, NDJSON
/// framing, session socket, MCP, or CodeMode runtime is involved.
#[cfg(feature = "tokenzero")]
pub mod tokenzero;

pub use adapter::{
    AdapterBinding, AdapterCall, AdapterContractError, AdapterError, AdapterResponse, DomainAdapter,
};
pub use connector::{
    SessionApprovalGrant, fs_write_grant_count_for_plan, harness_fs_write_grants,
};
pub use connector::ZsxAttemptJournalStatus;
pub use continuation::{
    CONTINUATION_REGISTRY_SCHEMA_VERSION, CONTINUATION_REGISTRY_WAL_SNAPSHOT,
    ContinuationFrame, ContinuationKey, ContinuationPersistRequest, ContinuationReceipt,
    ContinuationRecord, ContinuationRegistryError, ContinuationRegistry,
    ContinuationResumeBinding,
};
pub use lower::{METHODS, engine_for, lower};
pub use dag_exec::{
    DagExecError, DagExecutionOutcome, DagExecutor, DagNodeOutcome, ScheduleMode,
    StreamError, StreamSink,
};
pub use envelope::{
    ZSX_PROTOCOL, SessionEnvelopeContext, DecisionViewContext,
    legacy_envelope_value, legacy_kind_code,
};
pub use session::{
    DEFAULT_SHUTDOWN_WAIT_MS, SESSION_EXECUTION_QUEUE_CAPACITY, SESSION_EXECUTOR_START_TIMEOUT,
    SESSION_REPLACEMENT_SETTLE_TIMEOUT, SessionReplacementReason, SessionReplacementReceipt,
    ZsxBuilder, ZsxExecutionMetrics, ZsxExecutionResult, ZsxExecuteEnvelope, ZsxSession,
    ZsxSessionCancellation, ZsxSessionError, ZsxSessionFailureCode,
};
pub use verdict::{
    VERDICT_LOOP_RECEIPT_SCHEMA, VerdictDecision, VerdictLoopEnvelope, VerdictLoopReceipt,
    VerdictLoopResult,
};
/// Q99/residency gate (): session telemetry receipts measured by the
/// zero-gate W4 contracts. The gate itself is internal to the connector;
/// the report is the session's typed quality claim surface.
pub use residency::{
    SESSION_Q99_REPORT_SCHEMA, SessionQ99Report, SessionResidencyGate, TierQ99Report,
    tier_of_engine,
};
/// Bound untrusted error text for typed zsx envelopes.
pub use zero_codemode::{
    finalize_visible_error, GateRuleUsage, GateUsageReport,
};

use std::sync::atomic::{AtomicU64, Ordering};

/// Number of child processes spawned by zsx-core code since process start.
///
/// The canonical in-process path never increments this counter. Any future
/// process-backed compatibility adapter must increment it through the same
/// instrumentation, so fixture tests can prove "one process, no worker
/// spawn" deterministically.
static PROCESS_SPAWNS: AtomicU64 = AtomicU64::new(0);

/// Total child processes spawned by zsx-core code (0 for the canonical path).
pub fn process_spawn_count() -> u64 {
    PROCESS_SPAWNS.load(Ordering::Relaxed)
}

/// Instrument one child-process spawn. Only process-backed compatibility
/// code may call this; the in-process path must not.
#[allow(dead_code)]
pub(crate) fn record_process_spawn() {
    PROCESS_SPAWNS.fetch_add(1, Ordering::Relaxed);
}
