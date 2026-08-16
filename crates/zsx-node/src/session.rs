//! The `NativeZsxSession` N-API class.
//!
//! Construction only records configuration. `initialize()` and the first
//! `execute()` build the real full `zsx_core::ZsxSession` on libuv's worker
//! pool, so engine startup never blocks Node's event loop. `status` remains a
//! fast sync read; reconcile and shutdown also run on the worker pool.

use std::sync::Arc;
use std::time::Duration;

use crate::core::{DEFAULT_TIMEOUT_MS, SessionCore};
use crate::error;
use crate::tasks::{ControlTask, ExecuteTask, ReconcilePendingTask};
use napi::bindgen_prelude::{AbortSignal, AsyncTask};
use napi_derive::napi;

/// One real full ZSX session exposed to Node.js.
#[napi]
pub struct NativeZsxSession {
    core: Arc<SessionCore>,
}

#[napi]
impl NativeZsxSession {
    /// Record one session rooted at `root`, with optional mutable state
    /// isolated below `state_root`.
    ///
    /// The canonical in-process zsx-core composition is initialized lazily by
    /// an async task. No process is spawned and no socket is opened.
    #[napi(constructor)]
    pub fn new(
        root: String,
        session_id: Option<String>,
        state_root: Option<String>,
    ) -> napi::Result<Self> {
        Ok(Self {
            core: Arc::new(SessionCore::new(root, session_id, state_root)),
        })
    }

    /// Initialize all three domain engines asynchronously. Calling this is
    /// optional because `execute()` initializes lazily, but hosts can await it
    /// to surface startup failures before accepting a tool call.
    #[napi]
    pub fn initialize(&self) -> AsyncTask<ControlTask> {
        AsyncTask::new(ControlTask::initialize(Arc::clone(&self.core)))
    }

    /// Execute one plan asynchronously.
    ///
    /// Assigns a bounded per-generation request id and returns a Promise of
    /// the canonical zsx envelope:
    /// `{ protocol, ok, generation, request_id, result?, error? }`.
    /// `timeoutMs` defaults to 30000. When `signal` aborts, that one request
    /// is cancelled: if the async work has not started the Promise rejects
    /// with `AbortError`; if it is in flight and execute already finished,
    /// the Promise resolves `{ ok: false, result, error: { code: "commit_race" } }`.
    /// A late backend error still resolves `{ ok: false, error: { code: "cancelled" } }`.
    #[napi]
    pub fn execute(
        &self,
        plan: String,
        timeout_ms: Option<u32>,
        signal: Option<AbortSignal>,
    ) -> napi::Result<AsyncTask<ExecuteTask>> {
        let generation = self.core.generation();
        let request_id = self
            .core
            .allocate_request_id()
            .map_err(|err| error::zsx_error("execute", &err))?;
        self.core.begin_request();
        let timeout =
            Duration::from_millis(u64::from(timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS as u32)));
        let task = ExecuteTask::new(
            Arc::clone(&self.core),
            generation,
            request_id,
            plan,
            timeout,
        );
        if let Some(sig) = signal.as_ref() {
            // Flag the N-API task and cancel the exact zsx-core request token.
            let flag = task.cancelled_flag();
            let core = Arc::clone(&self.core);
            sig.on_abort(move || {
                flag.store(true, std::sync::atomic::Ordering::Release);
                core.abort_request(generation, request_id);
            });
        }
        Ok(AsyncTask::with_optional_signal(task, signal))
    }

    /// Session status: generation, worker state, in-flight requests, aborted
    /// requests, queue capacity, and the child-process spawn count (0 on the
    /// canonical in-process path).
    #[napi]
    pub fn status(&self) -> napi::Result<SessionStatus> {
        let generation = self.core.generation();
        let ready = self.core.is_ready();
        Ok(SessionStatus {
            generation: generation as u32,
            state: if self.core.is_terminated() {
                "stopped".to_string()
            } else if ready {
                "running".to_string()
            } else {
                "idle".to_string()
            },
            inflight: self.core.inflight() as u32,
            aborted: self.core.aborted() as u32,
            queue_capacity: zsx_core::SESSION_EXECUTION_QUEUE_CAPACITY as u32,
            spawns: zsx_core::process_spawn_count() as u32,
        })
    }

    /// Classify every durable mutation attempt under the session store without
    /// redispatching effects. Harnesses call this before manual generation
    /// replacement after an interrupted native process.
    #[napi]
    pub fn reconcile_pending(&self) -> AsyncTask<ReconcilePendingTask> {
        AsyncTask::new(ReconcilePendingTask::new(Arc::clone(&self.core)))
    }

    /// Manual reconcile: replace the session generation (zsx-core
    /// `replace(.., SessionReplacementReason::Manual)`). Cancels in-flight
    /// requests, advances the generation, and resets the request id space.
    #[napi]
    pub fn reconcile(&self) -> AsyncTask<ControlTask> {
        AsyncTask::new(ControlTask::reconcile(Arc::clone(&self.core)))
    }

    /// Shutdown the session: stops only the in-process worker thread.
    /// Idempotent; returns the generation the session stopped at.
    ///
    /// The host hook races native shutdown against an 800ms ceiling so a
    /// stalled control/settlement/join cannot keep a Node listener pending
    /// past the host's 2000ms exit deadline. A timeout resolves the same
    /// receipt with `reason: "host_shutdown_timeout"` and does not replay
    /// native shutdown.
    #[napi]
    pub fn shutdown(&self) -> AsyncTask<ControlTask> {
        AsyncTask::new(ControlTask::shutdown(Arc::clone(&self.core)))
    }
}

/// Snapshot returned by `status()`.
#[napi(object)]
pub struct SessionStatus {
    /// Active session generation (u32 snapshot; reconciles are far rarer
    /// than the u64 space zsx-core keeps).
    pub generation: u32,
    /// `"idle"` before lazy initialization, `"running"` while the session
    /// worker is alive, and `"stopped"` after `shutdown()`.
    pub state: String,
    /// Requests admitted but not yet settled.
    pub inflight: u32,
    /// Requests cancelled through their AbortSignal.
    pub aborted: u32,
    /// zsx-core session execution queue capacity.
    pub queue_capacity: u32,
    /// Child processes spawned by zsx-core code (0 for the in-process path).
    pub spawns: u32,
}
