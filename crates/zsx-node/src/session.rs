//! The `NativeZsxSession` N-API class.
//!
//! Constructor builds one real full `zsx_core::ZsxSession` (executor thread,
//! aggregate connector, confined host) with the three domain adapters
//! registered — the same composition `zsx exec` uses. `execute` is async;
//! `status` is a fast sync read; `reconcile` and `shutdown` run on the
//! threadpool because they can block on the session worker.

use std::sync::Arc;
use std::time::Duration;

use napi::bindgen_prelude::{AbortSignal, AsyncTask};
use napi_derive::napi;
use zsx_core::ZsxSession;

use crate::core::{DEFAULT_TIMEOUT_MS, SessionCore};
use crate::error;
use crate::tasks::{ControlTask, ExecuteTask};

/// One real full ZSX session exposed to Node.js.
#[napi]
pub struct NativeZsxSession {
    core: Arc<SessionCore>,
}

#[napi]
impl NativeZsxSession {
    /// Build one real full session rooted at `root`.
    ///
    /// This is the canonical in-process zsx-core composition: the session
    /// worker thread, the aggregate connector, and the confined interpreter
    /// host with the three domain adapters (FSZero, GraphZero, TokenZero)
    /// registered. No process is spawned and no socket is opened.
    #[napi(constructor)]
    pub fn new(root: String, session_id: Option<String>) -> napi::Result<Self> {
        let builder = ZsxSession::builder(&root);
        let session = match session_id {
            Some(session_id) => builder.with_session_id(session_id).build_canonical(),
            None => builder.build_canonical(),
        }
        .map_err(|err| error::zsx_error("constructor", &err))?;
        Ok(Self {
            core: Arc::new(SessionCore::new(session)),
        })
    }

    /// Execute one plan asynchronously.
    ///
    /// Assigns a bounded per-generation request id and returns a Promise of
    /// the canonical zsx envelope:
    /// `{ protocol, ok, generation, request_id, result | error: { code, detail, retry_after_ms? } }`.
    /// `timeoutMs` defaults to 30000. When `signal` aborts, that one request
    /// is cancelled: if the async work has not started the Promise rejects
    /// with `AbortError`; if it is in flight the late result is discarded
    /// and the Promise resolves `{ ok: false, error: { code: "cancelled" } }`,
    /// bounded by the request timeout.
    #[napi]
    pub fn execute(
        &self,
        plan: String,
        timeout_ms: Option<u32>,
        signal: Option<AbortSignal>,
    ) -> napi::Result<AsyncTask<ExecuteTask>> {
        let generation = self
            .core
            .generation()
            .map_err(|err| error::zsx_error("execute", &err))?;
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
        let generation = self
            .core
            .generation()
            .map_err(|err| error::zsx_error("status", &err))?;
        Ok(SessionStatus {
            generation: generation as u32,
            state: if self.core.is_terminated() {
                "stopped".to_string()
            } else {
                "running".to_string()
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
    pub fn reconcile_pending(&self) -> napi::Result<String> {
        let statuses = self
            .core
            .session()
            .reconcile_all_attempts()
            .map_err(|err| error::zsx_error("reconcilePending", &err))?;
        serde_json::to_string(&statuses)
            .map_err(|err| napi::Error::from_reason(format!("reconcilePending: {err}")))
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
    /// `"running"` while the session worker thread is alive, `"stopped"` after `shutdown()`.
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
