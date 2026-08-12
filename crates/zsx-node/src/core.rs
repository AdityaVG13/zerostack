//! Session core shared across N-API async tasks.
//!
//! [`SessionCore`] keeps construction inputs until the first async task asks
//! for the canonical [`ZsxSession`]. Building the three-engine session can be
//! expensive, so it must happen in `Task::compute` on libuv's worker pool,
//! never in the JavaScript constructor on Pi's TUI thread.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zsx_core::{
    SessionReplacementReason, SessionReplacementReceipt, ZsxAttemptJournalStatus,
    ZsxExecutionResult, ZsxSession, ZsxSessionError, ZsxSessionFailureCode,
};

/// Canonical initial generation used by [`ZsxSession::builder`].
pub const INITIAL_GENERATION: u64 = 1;

/// Upper bound of the per-generation request id space.
///
/// Request ids are assigned monotonically inside `[1, MAX_REQUEST_ID]` and
/// reset when a generation is reconciled. zsx-core rejects duplicate ids
/// within a generation, so the bound keeps the id space finite and explicit;
/// exhaustion is a typed error instead of a silent wrap.
pub const MAX_REQUEST_ID: u64 = u32::MAX as u64;

/// Default execution timeout, matching the historical 30s `zsx` default.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Addon-level failure code for a request cancelled through its AbortSignal.
///
/// zsx-core has no `Cancelled` failure variant today; the addon resolves the
/// typed envelope itself (see the expected-signatures report).
pub const CODE_CANCELLED: &str = "cancelled";

/// Addon-level failure code for a panic contained by `catch_unwind`.
pub const CODE_PANIC: &str = "internal_panic";

struct SessionConfig {
    root: String,
    session_id: Option<String>,
    state_root: Option<String>,
}

pub struct SessionCore {
    config: SessionConfig,
    session: Mutex<Option<Arc<ZsxSession>>>,
    ready: AtomicBool,
    generation: AtomicU64,
    next_request_id: AtomicU64,
    inflight: AtomicUsize,
    aborted: AtomicU64,
    terminated: AtomicBool,
}

impl SessionCore {
    /// Record construction inputs without opening engines or starting the
    /// canonical session worker. Call [`Self::initialize`] from an N-API task.
    pub fn new(root: String, session_id: Option<String>, state_root: Option<String>) -> Self {
        Self {
            config: SessionConfig {
                root,
                session_id,
                state_root,
            },
            session: Mutex::new(None),
            ready: AtomicBool::new(false),
            generation: AtomicU64::new(INITIAL_GENERATION),
            next_request_id: AtomicU64::new(0),
            inflight: AtomicUsize::new(0),
            aborted: AtomicU64::new(0),
            terminated: AtomicBool::new(false),
        }
    }

    fn failure(&self, code: ZsxSessionFailureCode, detail: impl Into<String>) -> ZsxSessionError {
        ZsxSessionError {
            code,
            generation: self.generation(),
            request_id: None,
            detail: detail.into(),
            retry_after_ms: None,
        }
    }

    fn lock_session(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, Option<Arc<ZsxSession>>>, ZsxSessionError> {
        self.session.lock().map_err(|_| {
            self.failure(
                ZsxSessionFailureCode::Internal,
                "native session initialization lock is poisoned",
            )
        })
    }

    /// Build the canonical session at most once. The caller must be an async
    /// N-API task because this may initialize all three local engines.
    pub fn initialize(&self) -> Result<Arc<ZsxSession>, ZsxSessionError> {
        if self.is_terminated() {
            return Err(self.failure(
                ZsxSessionFailureCode::Terminating,
                "native session is shut down",
            ));
        }
        let mut slot = self.lock_session()?;
        if let Some(session) = slot.as_ref() {
            return Ok(Arc::clone(session));
        }
        if self.is_terminated() {
            return Err(self.failure(
                ZsxSessionFailureCode::Terminating,
                "native session is shut down",
            ));
        }

        let mut builder = ZsxSession::builder(&self.config.root);
        if let Some(state_root) = self.config.state_root.as_ref() {
            builder = builder.with_state_root(state_root);
        }
        let session = match self.config.session_id.as_ref() {
            Some(session_id) => builder.with_session_id(session_id).build_canonical(),
            None => builder.build_canonical(),
        }?;
        let session = Arc::new(session);
        self.generation
            .store(session.generation()?, Ordering::Release);
        *slot = Some(Arc::clone(&session));
        self.ready.store(true, Ordering::Release);
        Ok(session)
    }

    fn initialized_session(&self) -> Result<Option<Arc<ZsxSession>>, ZsxSessionError> {
        Ok(self.lock_session()?.as_ref().map(Arc::clone))
    }

    /// Lock-free readiness snapshot for synchronous JavaScript status calls.
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    /// Current authoritative generation. The canonical builder always starts
    /// at one; successful reconcile updates this atomic before returning.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Allocate the next bounded request id for the current generation.
    pub fn allocate_request_id(&self) -> Result<u64, ZsxSessionError> {
        let mut next = self.next_request_id.load(Ordering::Relaxed);
        loop {
            let candidate = next + 1;
            if candidate > MAX_REQUEST_ID {
                return Err(ZsxSessionError {
                    code: ZsxSessionFailureCode::Backpressure,
                    generation: self.generation(),
                    request_id: None,
                    detail: format!(
                        "request id space exhausted ({}); reconcile the session to reset",
                        MAX_REQUEST_ID
                    ),
                    retry_after_ms: None,
                });
            }
            match self.next_request_id.compare_exchange_weak(
                next,
                candidate,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(candidate),
                Err(actual) => next = actual,
            }
        }
    }

    /// Reset the request id space after a generation replacement.
    pub fn reset_request_ids(&self) {
        self.next_request_id.store(0, Ordering::Relaxed);
    }

    /// Mark one admitted request as in flight (paired with `finish_request`).
    pub fn begin_request(&self) {
        self.inflight.fetch_add(1, Ordering::Relaxed);
    }

    /// Mark an in-flight request as settled (called from `Task::finally`).
    pub fn finish_request(&self) {
        self.inflight.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn inflight(&self) -> usize {
        self.inflight.load(Ordering::Relaxed)
    }

    /// Record and cancel one request through the canonical zsx-core hook.
    /// An abort before lazy initialization is handled by `ExecuteTask`'s flag
    /// and intentionally does not initialize the session just to cancel it.
    pub fn abort_request(&self, generation: u64, request_id: u64) {
        self.aborted.fetch_add(1, Ordering::Relaxed);
        if !self.is_ready() {
            return;
        }
        if let Ok(Some(session)) = self.initialized_session() {
            session
                .cancellation()
                .cancel_request(generation, request_id);
        }
    }

    pub fn aborted(&self) -> u64 {
        self.aborted.load(Ordering::Relaxed)
    }

    pub fn mark_terminated(&self) {
        self.terminated.store(true, Ordering::Release);
    }

    pub fn is_terminated(&self) -> bool {
        self.terminated.load(Ordering::Acquire)
    }

    /// Run one plan after lazy initialization. `ExecuteTask` checks its abort
    /// flag again between initialization and this call, so cancellation can
    /// never cross the cold-start boundary into a mutation.
    pub fn execute_ready(
        &self,
        session: &ZsxSession,
        generation: u64,
        request_id: u64,
        plan: &str,
        timeout: Duration,
    ) -> Result<ZsxExecutionResult, ZsxSessionError> {
        session.execute(generation, request_id, plan, timeout)
    }

    pub fn reconcile_pending(&self) -> Result<Vec<ZsxAttemptJournalStatus>, ZsxSessionError> {
        self.initialize()?.reconcile_all_attempts()
    }

    pub fn reconcile(&self) -> Result<SessionReplacementReceipt, ZsxSessionError> {
        let session = self.initialize()?;
        let receipt = session.replace(self.generation(), SessionReplacementReason::Manual)?;
        self.generation.store(receipt.generation, Ordering::Release);
        self.reset_request_ids();
        Ok(receipt)
    }

    pub fn shutdown(&self) -> Result<u64, ZsxSessionError> {
        self.mark_terminated();
        let Some(session) = self.initialized_session()? else {
            return Ok(self.generation());
        };
        session.shutdown()
    }
}
