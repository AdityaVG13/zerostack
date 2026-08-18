//! Single-process canonical ZSX composition core.
//!
//! zsx-core owns the safe in-process [`DomainAdapter`] contract, the
//! aggregate connector that dispatches three registered domain adapters
//! (FSZero, GraphZero, TokenZero) in memory, and the aggregate session
//! authority (generation, bounded queue, approvals, cancellation,
//! replacement, shutdown). It depends on `zero-codemode` only for the
//! confined interpreter host; the canonical in-process path never spawns a
//! worker process and never serializes through NDJSON or a session socket.
//!
//! The call-scoped Wave 10 supervisor ([`supervisor`]) is the one
//! process-backed path: its one-shot isolate profile spawns a short-lived
//! kernel child over stdio pipes and kills/reaps the exact process tree on
//! every terminal path; the embedded reentrant profile runs fresh per-call
//! runtimes on the calling thread. Both profiles share the zerokernel
//! protocol envelope and leave the native path untouched as the fallback.
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
/// K0 W9-E live rooted evidence and the guest wave-9 route
/// (`zerostack-fhcj`).
pub mod guest_w9e;
mod help;
mod lookup;
mod lower;
/// K0 capability broker: parse / resolve / normalize / inject / validate
/// preflight boundary for the Wave 10 supervisor (zerostack-pvwg).
pub mod preflight;
/// Bounded one-file read grants for explicit absolute reads outside the
/// session root (`fs.readGrant` / `zero.fs.read_grant`).
pub mod read_grant;
mod residency;
mod envelope;
mod session;
pub mod supervisor;
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
pub use guest_w9e::{SupervisorGuestWave9, W9eEvidence};
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
/// Wave 10 call-scoped daemonless execute supervisor (embedded reentrant and
/// one-shot isolate profiles over the same zerokernel protocol envelope).
pub use supervisor::{
    KERNEL_CHILD_COMMAND, KERNEL_CHILD_MAX_REQUEST_BYTES, KERNEL_CHILD_MAX_RESPONSE_BYTES,
    KERNEL_CHILD_STDERR_CAPTURE_BYTES, ONESHOT_CANCEL_POLL, ONESHOT_EXIT_SETTLE,
    ONESHOT_KILL_GRACE, ONESHOT_SETTLE_GRACE, SESSION_ID_ENV, STORE_ROOT_ENV,
    SUPERVISOR_IDLE_WAIT, OneShotChild, Supervisor, SupervisorBuilder, SupervisorError,
    SupervisorProfile,
};
#[cfg(all(feature = "fszero", feature = "graphzero", feature = "tokenzero"))]
pub use supervisor::run_kernel_child;
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
/// The canonical in-process path never increments this counter. The
/// supervisor's one-shot isolate profile increments it through the same
/// instrumentation (one short-lived kernel child per call, killed and
/// reaped on every terminal path), so tests can prove "one spawn per
/// one-shot call, zero survivors" deterministically.
static PROCESS_SPAWNS: AtomicU64 = AtomicU64::new(0);

/// Total child processes spawned by zsx-core code (0 for the canonical
/// in-process path; one per one-shot supervisor call).
pub fn process_spawn_count() -> u64 {
    PROCESS_SPAWNS.load(Ordering::Relaxed)
}

/// Instrument one child-process spawn. Only process-backed compatibility
/// code (the supervisor one-shot profile) may call this; the in-process
/// path must not.
#[allow(dead_code)]
pub(crate) fn record_process_spawn() {
    PROCESS_SPAWNS.fetch_add(1, Ordering::Relaxed);
}
