//! Session core shared across N-API async tasks.
//!
//! [`SessionCore`] wraps the one real [`zsx_core::ZsxSession`] plus the
//! per-request bookkeeping (bounded request ids, in-flight and aborted
//! counters). It is `Send + Sync`, so N-API async tasks can carry it to the
//! libuv threadpool and back.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use zsx_core::{ZsxExecutionResult, ZsxSession, ZsxSessionError};

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

pub struct SessionCore {
    session: Arc<ZsxSession>,
    next_request_id: AtomicU64,
    inflight: AtomicUsize,
    aborted: AtomicU64,
    terminated: AtomicBool,
}

impl SessionCore {
    pub fn new(session: ZsxSession) -> Self {
        Self {
            session: Arc::new(session),
            next_request_id: AtomicU64::new(0),
            inflight: AtomicUsize::new(0),
            aborted: AtomicU64::new(0),
            terminated: AtomicBool::new(false),
        }
    }

    /// The underlying zsx-core session.
    pub fn session(&self) -> &ZsxSession {
        &self.session
    }

    /// Current session generation, read from zsx-core (authoritative).
    pub fn generation(&self) -> Result<u64, ZsxSessionError> {
        self.session.generation()
    }

    /// Allocate the next bounded request id for the current generation.
    ///
    /// Monotonic from 1; returns an error once `MAX_REQUEST_ID` is consumed
    /// (a manual reconcile resets the space for the next generation).
    pub fn allocate_request_id(&self) -> Result<u64, ZsxSessionError> {
        let mut next = self.next_request_id.load(Ordering::Relaxed);
        loop {
            let candidate = next + 1;
            if candidate > MAX_REQUEST_ID {
                return Err(ZsxSessionError {
                    code: zsx_core::ZsxSessionFailureCode::Backpressure,
                    generation: self.generation()?,
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
    pub fn abort_request(&self, generation: u64, request_id: u64) {
        self.aborted.fetch_add(1, Ordering::Relaxed);
        self.session
            .cancellation()
            .cancel_request(generation, request_id);
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

    /// Run one plan on the session worker thread.
    ///
    /// AbortSignal callbacks call [`Self::abort_request`], which cancels the
    /// matching zsx-core request token. No later dispatch for that request is
    /// admitted; later requests in the same generation use fresh tokens.
    pub fn execute(
        &self,
        generation: u64,
        request_id: u64,
        plan: &str,
        timeout: Duration,
    ) -> Result<ZsxExecutionResult, ZsxSessionError> {
        self.session.execute(generation, request_id, plan, timeout)
    }
}
