//! N-API async tasks.
//!
//! Every `compute` here runs on the libuv threadpool and is wrapped in
//! `catch_unwind(AssertUnwindSafe)` so a backend panic becomes a typed
//! envelope instead of unwinding across the FFI boundary.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use napi::bindgen_prelude::{ToNapiValue, TypeName, ValueType};
use napi::{Env, Result, Task};

use crate::core::SessionCore;
use crate::envelope::Envelope;
use crate::error;

/// Owned, lifetime-free JS value produced by task resolution.
///
/// Wraps the canonical serde_json envelope; the napi conversion delegates to
/// napi's own serde_json path (feature `serde-json`), which only builds plain
/// data values.
pub struct JsEnvelope(serde_json::Value);

impl TypeName for JsEnvelope {
    fn type_name() -> &'static str {
        "Object"
    }

    fn value_type() -> ValueType {
        ValueType::Object
    }
}

impl ToNapiValue for JsEnvelope {
    unsafe fn to_napi_value(env: napi::sys::napi_env, val: Self) -> Result<napi::sys::napi_value> {
        // SAFETY: delegates to napi's serde_json conversion, which builds
        // plain data values only (no functions, no cyclic references).
        unsafe { <&serde_json::Value as ToNapiValue>::to_napi_value(env, &val.0) }
    }
}

/// One asynchronous `execute(plan, timeoutMs, signal?)` request.
pub struct ExecuteTask {
    core: Arc<SessionCore>,
    generation: u64,
    request_id: u64,
    plan: String,
    timeout: Duration,
    cancelled: Arc<AtomicBool>,
}

impl ExecuteTask {
    pub fn new(
        core: Arc<SessionCore>,
        generation: u64,
        request_id: u64,
        plan: String,
        timeout: Duration,
    ) -> Self {
        Self {
            core,
            generation,
            request_id,
            plan,
            timeout,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The per-request abort flag; set by the AbortSignal callback.
    pub fn cancelled_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }

    /// Per-request cancellation outcome: the request aborted.
    fn cancelled(&self) -> Envelope {
        Envelope::cancelled(self.generation, self.request_id)
    }
}

impl Task for ExecuteTask {
    type Output = Envelope;
    type JsValue = JsEnvelope;

    fn compute(&mut self) -> Result<Envelope> {
        // Abort before admission into zsx-core (or while queued): skip work.
        if self.cancelled.load(Ordering::Acquire) {
            return Ok(self.cancelled());
        }
        let session = match catch_unwind(AssertUnwindSafe(|| self.core.initialize())) {
            Ok(Ok(session)) => session,
            Ok(Err(err)) => {
                return Ok(Envelope::from_zsx_error(
                    self.generation,
                    self.request_id,
                    &err,
                ));
            }
            Err(_panic) => return Ok(Envelope::panic(self.generation, self.request_id)),
        };
        // Abort may arrive while lazy initialization runs. Re-check before
        // dispatch so a cancelled cold request cannot execute a mutation.
        if self.cancelled.load(Ordering::Acquire) {
            return Ok(self.cancelled());
        }
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            self.core.execute_ready(
                &session,
                self.generation,
                self.request_id,
                &self.plan,
                self.timeout,
            )
        }));
        match outcome {
            Ok(Ok(result)) if !self.cancelled.load(Ordering::Acquire) => {
                Ok(Envelope::ok(self.generation, self.request_id, result))
            }
            // Aborted while in flight: discard the late result.
            Ok(Ok(_)) => Ok(self.cancelled()),
            Ok(Err(err)) if !self.cancelled.load(Ordering::Acquire) => Ok(
                Envelope::from_zsx_error(self.generation, self.request_id, &err),
            ),
            Ok(Err(_)) => Ok(self.cancelled()),
            Err(_panic) => Ok(Envelope::panic(self.generation, self.request_id)),
        }
    }

    fn resolve(&mut self, _env: Env, output: Envelope) -> Result<JsEnvelope> {
        Ok(JsEnvelope(output.to_value()))
    }

    fn finally(self, _env: Env) -> Result<()> {
        self.core.finish_request();
        Ok(())
    }
}

/// Control-plane operation carried by [`ControlTask`].
#[derive(Clone, Copy)]
pub enum ControlOp {
    /// Build the canonical three-engine session on libuv's worker pool.
    Initialize,
    /// Manual reconcile: replace the session generation
    /// (`zsx_core` `replace(.., SessionReplacementReason::Manual)`).
    Reconcile,
    /// Shutdown: stop only the in-process worker thread.
    Shutdown,
}

/// One asynchronous `reconcile()` or `shutdown()` call.
pub struct ControlTask {
    core: Arc<SessionCore>,
    op: ControlOp,
}

impl ControlTask {
    pub fn initialize(core: Arc<SessionCore>) -> Self {
        Self {
            core,
            op: ControlOp::Initialize,
        }
    }

    pub fn reconcile(core: Arc<SessionCore>) -> Self {
        Self {
            core,
            op: ControlOp::Reconcile,
        }
    }

    pub fn shutdown(core: Arc<SessionCore>) -> Self {
        Self {
            core,
            op: ControlOp::Shutdown,
        }
    }
}

/// Resolved receipt for a control-plane operation.
pub struct ControlOutcome {
    pub kind: &'static str,
    pub generation: u64,
    pub previous_generation: Option<u64>,
    pub reason: Option<String>,
}

impl Task for ControlTask {
    type Output = ControlOutcome;
    type JsValue = JsEnvelope;

    fn compute(&mut self) -> Result<ControlOutcome> {
        catch_unwind(AssertUnwindSafe(|| match self.op {
            ControlOp::Initialize => {
                let generation = self
                    .core
                    .initialize()
                    .and_then(|session| session.generation())
                    .map_err(|err| error::zsx_error("initialize", &err))?;
                Ok(ControlOutcome {
                    kind: "initialize",
                    generation,
                    previous_generation: None,
                    reason: None,
                })
            }
            ControlOp::Reconcile => {
                let receipt = self
                    .core
                    .reconcile()
                    .map_err(|err| error::zsx_error("reconcile", &err))?;
                Ok(ControlOutcome {
                    kind: "reconcile",
                    generation: receipt.generation,
                    previous_generation: Some(receipt.previous_generation),
                    reason: Some(receipt.reason.as_str().to_string()),
                })
            }
            ControlOp::Shutdown => {
                let generation = self
                    .core
                    .shutdown()
                    .map_err(|err| error::zsx_error("shutdown", &err))?;
                Ok(ControlOutcome {
                    kind: "shutdown",
                    generation,
                    previous_generation: None,
                    reason: None,
                })
            }
        }))
        .unwrap_or_else(|_| Err(error::panic_error("control")))
    }

    fn resolve(&mut self, _env: Env, output: ControlOutcome) -> Result<JsEnvelope> {
        let mut v = serde_json::json!({
            "kind": output.kind,
            "generation": output.generation,
        });
        if let Some(previous) = output.previous_generation {
            v.as_object_mut()
                .expect("control outcome is an object")
                .insert(
                    "previous_generation".to_string(),
                    serde_json::json!(previous),
                );
        }
        if let Some(reason) = output.reason {
            v.as_object_mut()
                .expect("control outcome is an object")
                .insert("reason".to_string(), serde_json::json!(reason));
        }
        Ok(JsEnvelope(v))
    }
}

/// Scan durable mutation attempts without blocking Node's event loop.
pub struct ReconcilePendingTask {
    core: Arc<SessionCore>,
}

impl ReconcilePendingTask {
    pub fn new(core: Arc<SessionCore>) -> Self {
        Self { core }
    }
}

impl Task for ReconcilePendingTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> Result<String> {
        catch_unwind(AssertUnwindSafe(|| {
            let statuses = self
                .core
                .reconcile_pending()
                .map_err(|err| error::zsx_error("reconcilePending", &err))?;
            serde_json::to_string(&statuses)
                .map_err(|err| error::message("reconcilePending", err.to_string()))
        }))
        .unwrap_or_else(|_| Err(error::panic_error("reconcilePending")))
    }

    fn resolve(&mut self, _env: Env, output: String) -> Result<String> {
        Ok(output)
    }
}
