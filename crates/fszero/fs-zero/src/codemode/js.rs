//! Hub-backed JavaScript CodeMode adapter.
//!
//! Restricted interpretation, plan wrapping, validation, and runtime limits
//! belong to `zero-codemode`. FSZero keeps the domain connector, journal, and
//! execution/receipt semantics around that host.

use crate::core::{FSZeroSession, estimate_visible_tokens};
use serde_json::{Value as JsonValue, json};
use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use zero_codemode::{
    CapabilityDescriptor, Connector, ConnectorCompletion, ConnectorError, DispatchContext,
    GlobalRegistration, Host, HostError, HostLimits, MAX_INFLIGHT_CONNECTOR_CALLS,
    runtime_creation_count, wrap_plan,
};

use super::connector::{FsConnector, FsStep};
use super::host::{
    ContractError, dedup_detail_ref, payload_wire_value_with_session, put_contract_error,
};
use super::limits::{
    MAX_CODE_BYTES, MAX_LOGICAL_OPS, MAX_MEMORY_BYTES, MAX_MICROTASKS, MAX_OUTPUT_BYTES,
    MAX_PHYSICAL_OPS, MAX_REFS_EMITTED, MAX_RESULT_REF_BYTES, effective_max_wall_ms,
};
use super::runtime::{ERROR_REF, RESULT_REF, RuntimeOutcome, STEPS_REF, StepLog};
use super::transaction::TransactionJournal;
use super::zero_result::{zero_result_from_fs_step, zero_result_to_wire};

static ZERO_EDIT_POLICY_ONCE: AtomicBool = AtomicBool::new(false);
static HOST_BOUNDARY_PANIC_INJECT: AtomicBool = AtomicBool::new(false);
static RUNTIME_COUNT_BASE: AtomicU64 = AtomicU64::new(0);

/// Hub-owned interpreter creations since the last test reset.
pub fn sandbox_runtime_creation_count() -> u64 {
    runtime_creation_count().saturating_sub(RUNTIME_COUNT_BASE.load(Ordering::SeqCst))
}

/// Test helper retaining the historical FSZero API without owning a runtime.
pub fn reset_sandbox_runtime_creation_count_for_tests() {
    RUNTIME_COUNT_BASE.store(runtime_creation_count(), Ordering::SeqCst);
}

/// Test helper: make the next FSZero connector entry fail through the hub
/// connector boundary. The hub remains the only interpreter host boundary.
pub fn inject_host_boundary_panic_for_test(enable: bool) {
    HOST_BOUNDARY_PANIC_INJECT.store(enable, Ordering::SeqCst);
}

#[derive(Debug, Default, Clone)]
struct JsMetrics {
    logical_ops: u32,
    physical_ops: u32,
    batched_ops: u32,
    refs_emitted: usize,
}

#[derive(Debug, Default, Clone)]
struct HostShared {
    metrics: JsMetrics,
    steps: Vec<StepLog>,
    last_ref: String,
    failed: Option<String>,
}

impl HostShared {
    fn new() -> Self {
        Self {
            last_ref: RESULT_REF.to_string(),
            ..Self::default()
        }
    }
}

/// Connector adapter from the hub's async completion contract to the
/// synchronous, journaled FSZero domain connector.
///
/// # Raw pointer invariant
///
/// `session` and `journal` are unique borrows of stack-owned values in
/// `execute_js_plan`, stored as raw pointers because `Connector::dispatch`
/// takes `&self`. Aliasing / lifetime contract:
/// - Both pointers are derived from unique `&mut` borrows that outlive
///   `Host::execute_with_cancel_timeout`.
/// - They address distinct objects (`FSZeroSession` vs `TransactionJournal`)
///   and never form overlapping `&mut` aliases.
/// - The hub calls this adapter synchronously on the executing thread and
///   settles `ConnectorCompletion` before `dispatch` returns; no completion
///   or `Rc<Self>` clone escapes that call.
/// - `execute_js_plan` does not use `session` or `journal` again until the
///   `Rc<FsZeroConnector>` moved into the host has been dropped.
/// - Dispatch is not re-entrant: at most one reconstituted `&mut` of each
///   pointer is live at a time.
struct FsZeroConnector {
    session: *mut FSZeroSession,
    journal: *mut TransactionJournal,
    shared: Rc<RefCell<HostShared>>,
}

impl FsZeroConnector {
    fn with_shared<R>(&self, f: impl FnOnce(&mut HostShared) -> R) -> R {
        f(&mut self.shared.borrow_mut())
    }

    fn record_step(&self, step: &FsStep, method: String, batched: bool) {
        self.with_shared(|shared| {
            shared.metrics.logical_ops = shared.metrics.logical_ops.saturating_add(1);
            shared.metrics.physical_ops = shared.metrics.physical_ops.saturating_add(1);
            if batched {
                shared.metrics.batched_ops = shared.metrics.batched_ops.saturating_add(1);
            }
            shared.last_ref = step.recovery_key.clone();
            if !step.ok {
                shared.failed = step
                    .detail
                    .clone()
                    .or_else(|| Some(format!("{method} failed")));
            }
            shared.steps.push(StepLog {
                index: shared.steps.len(),
                method,
                ack: step.ack.clone(),
                ok: step.ok,
                recovery_key: step.recovery_key.clone(),
                detail: step.detail.clone(),
                parallel: false,
            });
        });
    }

    fn raw_result(&self, step: &FsStep) -> String {
        let payload = serde_json::from_slice::<JsonValue>(&step.payload).unwrap_or_else(|_| {
            JsonValue::String(String::from_utf8_lossy(&step.payload).into_owned())
        });
        let mut value = json!({
            "ack": step.ack.clone(),
            "ok": step.ok,
            "method": step.method.clone(),
            "detail": step.detail.clone(),
            "ref": step.recovery_key.clone(),
            "result": payload.clone(),
            "payload": payload,
        });
        if step.ok {
            // SAFETY: `self.session` is the unique `*mut FSZeroSession` from
            // `execute_js_plan`. This reconstitution is sequential with other
            // session derefs (no overlapping `&mut`), and the pointer remains
            // valid for the enclosing host execute (see struct invariant).
            let reference = unsafe { &mut *self.session }
                .recovery
                .put_content_ref(&step.payload);
            value["ref"] = json!(reference);
        }
        value.to_string()
    }

    fn ctx_ref(&self, value: JsonValue) -> String {
        let bytes = match serde_json::to_vec(&value) {
            Ok(bytes) => bytes,
            Err(error) => return self.failure("ctx.ref", error.to_string()),
        };
        if bytes.len() > MAX_RESULT_REF_BYTES {
            return self.failure(
                "ctx.ref",
                format!("ctx.ref payload exceeds {MAX_RESULT_REF_BYTES} bytes"),
            );
        }
        let over_limit = self.with_shared(|shared| shared.metrics.refs_emitted >= MAX_REFS_EMITTED);
        if over_limit {
            return self.failure(
                "ctx.ref",
                format!("max refs emitted exceeded: {MAX_REFS_EMITTED}"),
            );
        }
        // SAFETY: same unique session pointer as `raw_result`; this call does
        // not overlap any other `&mut FSZeroSession`. The pointer is valid
        // until `execute_js_plan`'s host execute returns.
        let session = unsafe { &mut *self.session };
        session.record_codemode_materialization(&bytes);
        let reference = session.recovery.put_content_ref(&bytes);
        self.with_shared(|shared| {
            shared.metrics.logical_ops = shared.metrics.logical_ops.saturating_add(1);
            shared.metrics.refs_emitted = shared.metrics.refs_emitted.saturating_add(1);
            shared.last_ref = reference.clone();
        });
        json!({
            "ack": "C",
            "ok": true,
            "method": "ctx.ref",
            "ref": reference,
            "result": value.clone(),
            "payload": value,
            "detail": null,
        })
        .to_string()
    }

    fn ctx_step(&self, args: JsonValue) -> String {
        let (name, value) = match args {
            JsonValue::Array(mut values) => {
                let value = values.pop().unwrap_or(JsonValue::Null);
                let name = values
                    .pop()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "step".to_owned());
                (name, value)
            }
            JsonValue::Object(mut object) => {
                let name = object
                    .remove("name")
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "step".to_owned());
                let value = object.remove("value").unwrap_or(JsonValue::Null);
                (name, value)
            }
            value => ("step".to_owned(), value),
        };
        let encoded = value.to_string();
        self.with_shared(|shared| {
            shared.metrics.logical_ops = shared.metrics.logical_ops.saturating_add(1);
            if value.as_array().is_some_and(|items| items.len() >= 100) {
                shared.metrics.batched_ops = shared.metrics.batched_ops.saturating_add(1);
                shared.metrics.physical_ops = shared.metrics.physical_ops.saturating_add(1);
            }
            shared.steps.push(StepLog {
                index: shared.steps.len(),
                method: format!("ctx.step:{name}"),
                ack: "C".to_owned(),
                ok: true,
                recovery_key: STEPS_REF.to_owned(),
                detail: Some("callback".to_owned()),
                parallel: false,
            });
        });
        json!({
            "ack": "C",
            "ok": true,
            "method": format!("ctx.step:{name}"),
            "result": value.clone(),
            "payload": encoded,
            "detail": "callback",
        })
        .to_string()
    }

    fn policy_edit(&self, args: JsonValue) -> String {
        if !ZERO_EDIT_POLICY_ONCE.swap(true, Ordering::SeqCst) {
            let detail = "policy denied zero.edit probe";
            self.with_shared(|shared| shared.failed = Some(detail.to_owned()));
            return self.failure("zero.edit", detail.to_owned());
        }
        json!({
            "ack": "C",
            "ok": true,
            "method": "zero.edit",
            "result": {"mutation": "allowed", "args": args.clone()},
            "payload": {"mutation": "allowed", "args": args},
            "detail": null,
        })
        .to_string()
    }

    fn failure(&self, method: &str, detail: String) -> String {
        self.with_shared(|shared| shared.failed = Some(detail.clone()));
        let value = zero_result_to_wire(&zero_result_from_fs_step(
            "X0",
            false,
            method,
            ERROR_REF,
            &JsonValue::Null,
            Some(&detail),
        ));
        value.to_string()
    }

    fn dispatch_inner(&self, capability: &CapabilityDescriptor, args_json: &str) -> String {
        let args = serde_json::from_str::<JsonValue>(args_json).unwrap_or(JsonValue::Null);
        match (capability.surface.as_str(), capability.method.as_str()) {
            ("fs", method) => {
                let method = format!("fs.{method}");
                let domain_args = match args {
                    JsonValue::Array(mut values) => values.pop().unwrap_or(JsonValue::Null),
                    value => value,
                };
                let is_batched = method.ends_with("Many");
                let over_logical =
                    self.with_shared(|shared| shared.metrics.logical_ops >= MAX_LOGICAL_OPS);
                if over_logical {
                    return self.failure(
                        &method,
                        format!("max logical ops exceeded: {MAX_LOGICAL_OPS}"),
                    );
                }
                let over_physical =
                    self.with_shared(|shared| shared.metrics.physical_ops >= MAX_PHYSICAL_OPS);
                if over_physical {
                    return self.failure(
                        &method,
                        format!("max physical ops exceeded: {MAX_PHYSICAL_OPS}"),
                    );
                }
                // SAFETY: `session` and `journal` are distinct stack-owned
                // objects in `execute_js_plan`. Each pointer is uniquely
                // derived from a `&mut` that outlives this synchronous
                // dispatch. The hub does not re-enter `dispatch` while these
                // `&mut` refs are live, and `FsConnector::with_journal` does
                // not retain them past `invoke`.
                let step = unsafe {
                    let session = &mut *self.session;
                    let journal = &mut *self.journal;
                    FsConnector::with_journal(session, journal).invoke(&method, &domain_args)
                };
                let result = self.raw_result(&step);
                self.record_step(&step, method, is_batched);
                result
            }
            ("ctx", "ref") => self.ctx_ref(args),
            ("ctx", "step") => self.ctx_step(args),
            ("ctx", "edit") => self.policy_edit(args),
            _ => self.failure(
                &format!("{}.{}", capability.surface, capability.method),
                format!(
                    "unknown host capability {}.{}",
                    capability.surface, capability.method
                ),
            ),
        }
    }
}

impl Connector for FsZeroConnector {
    fn dispatch(
        &self,
        capability: &CapabilityDescriptor,
        args_json: &str,
        _context: DispatchContext,
        completion: ConnectorCompletion,
    ) -> Result<(), ConnectorError> {
        let result = catch_unwind(AssertUnwindSafe(|| {
            if HOST_BOUNDARY_PANIC_INJECT.swap(false, Ordering::SeqCst) {
                panic!("fszero hub connector boundary inject");
            }
            self.dispatch_inner(capability, args_json)
        }));
        let encoded = match result {
            Ok(encoded) => encoded,
            Err(_) => {
                let detail = format!("host panic in {}.{}", capability.surface, capability.method);
                self.with_shared(|shared| shared.failed = Some(detail.clone()));
                zero_result_to_wire(&zero_result_from_fs_step(
                    "X0",
                    false,
                    &format!("{}.{}", capability.surface, capability.method),
                    ERROR_REF,
                    &JsonValue::Null,
                    Some(&detail),
                ))
                .to_string()
            }
        };
        completion.complete(Ok(encoded))
    }
}

const FS_CAPABILITIES: &[&str] = &[
    "ls",
    "read",
    "multiRead",
    "search",
    "multiSearch",
    "multiList",
    "multiAstSearch",
    "stat",
    "multiStat",
    "expand",
    "edit",
    "write",
    "undo",
    "history",
    "world",
    "compound",
    "memory",
];

fn registration() -> GlobalRegistration {
    let mut capabilities = FS_CAPABILITIES
        .iter()
        .map(|method| CapabilityDescriptor::new("fs", *method))
        .collect::<Vec<_>>();
    capabilities.extend([
        CapabilityDescriptor::new("ctx", "ref"),
        CapabilityDescriptor::new("ctx", "step"),
        CapabilityDescriptor::new("ctx", "edit"),
    ]);
    GlobalRegistration {
        root: "__fszeroHub".to_owned(),
        capabilities,
    }
}

/// Install legacy `fs` / `zero.fs` aliases around the hub's strict
/// zero-result surface. The compatibility layer uses only syntax owned by the
/// restricted interpreter; FSZero never embeds or extends that interpreter.
fn compatibility_source(code: &str) -> String {
    let method_entries = FS_CAPABILITIES
        .iter()
        .filter(|method| !matches!(**method, "world" | "compound" | "memory"))
        .map(|method| {
            format!("    {method}: (__arg) => __fszeroCall(__fszeroHub.fs.{method}, [__arg])")
        })
        .collect::<Vec<_>>()
        .join(",\n");
    let wrapped = wrap_user_code(code);
    format!(
        r#"
const __fszeroObject = (__value) => __value && typeof __value === "object" ? __value : {{}};
const __fszeroDecorate = (__result) => {{
    if (!__result || typeof __result !== "object") return __result;
    const __content = __result.content;
    if (!__content || __content.kind !== "inline") return {{
        ack: __result.ack,
        content: __content,
        ref: __content ? __content.ref : undefined,
        ok: __result.ack !== "X0",
        detail: __content ? __content.preview : undefined
    }};
    const __wire = __content.value;
    if (!__wire || typeof __wire !== "object") return {{
        ack: __result.ack,
        content: __content,
        result: __wire,
        payload: __wire,
        text: __wire,
        ok: __result.ack !== "X0"
    }};
    return {{
        ack: __result.ack,
        content: __content,
        result: __wire,
        payload: __wire,
        text: __wire,
        ok: __result.ack !== "X0" && __wire.ok !== false,
        detail: __wire.detail,
        method: __wire.method
    }};
}};
const __fszeroCall = async (__method, __args) => __fszeroDecorate(await __method(...__args));
const __fszeroMemoryInput = (__op, __first, __second) => {{
    if (__first && typeof __first === "object" && !Array.isArray(__first)) return {{...__first, op: __op}};
    if (__op === "rename") return {{from: __first, to: __second, op: __op}};
    return {{path: __first, content: __second, prefix: __first, op: __op}};
}};
const fs = {{
{method_entries},
    world: (__action, __args) => __fszeroCall(__fszeroHub.fs.world, [{{...__fszeroObject(__args), action: __action}}]),
    compound: (__name, __args) => __fszeroCall(__fszeroHub.fs.compound, [{{...__fszeroObject(__args), name: __name}}]),
    memory: {{
        get: (__first, __second) => __fszeroCall(__fszeroHub.fs.memory, [__fszeroMemoryInput("get", __first, __second)]),
        put: (__first, __second) => __fszeroCall(__fszeroHub.fs.memory, [__fszeroMemoryInput("put", __first, __second)]),
        delete: (__first, __second) => __fszeroCall(__fszeroHub.fs.memory, [__fszeroMemoryInput("delete", __first, __second)]),
        rename: (__first, __second) => __fszeroCall(__fszeroHub.fs.memory, [__fszeroMemoryInput("rename", __first, __second)]),
        ls: (__first, __second) => __fszeroCall(__fszeroHub.fs.memory, [__fszeroMemoryInput("ls", __first, __second)])
    }},
    multi_read: (__input, __opts) => __fszeroCall(__fszeroHub.fs.multiRead, [Array.isArray(__input) ? {{paths: __input, ...__fszeroObject(__opts)}} : __input]),
    multi_list: (__input, __opts) => __fszeroCall(__fszeroHub.fs.multiList, [Array.isArray(__input) ? {{items: __input, ...__fszeroObject(__opts)}} : __input]),
    multi_search: (__input, __opts) => __fszeroCall(__fszeroHub.fs.multiSearch, [Array.isArray(__input) ? {{queries: __input, ...__fszeroObject(__opts)}} : __input]),
    multi_ast_search: (__input, __opts) => __fszeroCall(__fszeroHub.fs.multiAstSearch, [Array.isArray(__input) ? {{items: __input, ...__fszeroObject(__opts)}} : __input])
}};
const ctx = {{
    ref: (__value) => __fszeroCall(__fszeroHub.ctx.ref, [__value]),
    step: (__name, __value) => __fszeroCall(__fszeroHub.ctx.step, [__name, __value]),
    edit: (__value) => __fszeroCall(__fszeroHub.ctx.edit, [__value])
}};
const zero = {{fs: fs, ctx: ctx, edit: ctx.edit}};
return await {wrapped};
"#,
        method_entries = method_entries,
        wrapped = wrapped
    )
}

/// Execute one JavaScript plan through the hub host.
pub fn execute_js_plan(session: &mut FSZeroSession, code: &str) -> RuntimeOutcome {
    let start_ops = session.op_count;
    let started = Instant::now();
    if let Err(error) = wrap_plan(code, MAX_CODE_BYTES) {
        let detail = error.to_string();
        return js_init_error(
            session,
            start_ops,
            detail.clone(),
            ContractError::validation(detail),
        );
    }
    if let Some(denied) = denied_sandbox_category(code) {
        let detail = format!("sandbox denied {denied}");
        return js_init_error(
            session,
            start_ops,
            detail.clone(),
            ContractError::sandbox(detail),
        );
    }

    let wall_ms = effective_max_wall_ms();
    let limits = match HostLimits::new(
        MAX_MEMORY_BYTES,
        512 * 1024,
        Duration::from_millis(wall_ms),
        100_000,
        MAX_MICROTASKS as usize,
        MAX_INFLIGHT_CONNECTOR_CALLS,
        MAX_CODE_BYTES.saturating_add(16 * 1024),
        MAX_RESULT_REF_BYTES,
    ) {
        Ok(limits) => limits,
        Err(error) => {
            let detail = format!("invalid CodeMode host limits: {error}");
            return js_init_error(
                session,
                start_ops,
                detail.clone(),
                ContractError::runtime(detail),
            );
        }
    };
    let host = match Host::new(limits, registration()) {
        Ok(host) => host,
        Err(error) => {
            let detail = format!("codemode host init failed: {error}");
            return js_init_error(
                session,
                start_ops,
                detail.clone(),
                ContractError::runtime(detail),
            );
        }
    };
    let source = compatibility_source(code);
    let request_deadline = session.request_deadline;
    let timeout = request_deadline
        .map(|deadline| deadline.saturating_duration_since(Instant::now()))
        .unwrap_or(Duration::from_millis(wall_ms));
    let cancellation = session
        .request_cancel
        .clone()
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
    let mut journal = TransactionJournal::always_on();
    let shared = Rc::new(RefCell::new(HostShared::new()));
    let connector = Rc::new(FsZeroConnector {
        session,
        journal: &mut journal,
        shared: Rc::clone(&shared),
    });
    let result = host.execute_with_cancel_timeout(&source, connector, cancellation, timeout);
    let snapshot = shared.borrow().clone();
    let mut ok = result.is_ok() && snapshot.failed.is_none();
    let mut error = None;
    let result_text = match result {
        Ok(value) => serde_json::to_string(&value).unwrap_or_else(|serialization| {
            format!("{{\"serialization_error\":{serialization:?}}}")
        }),
        Err(host_error) => {
            let detail = host_error_detail(&host_error);
            error = Some(classify_host_error(&host_error, &detail));
            ok = false;
            detail
        }
    };
    let failure_detail = snapshot
        .failed
        .clone()
        .or_else(|| error.as_ref().map(|error| error.message.clone()));
    if let Some(detail) = failure_detail.as_ref() {
        session.recovery.put_key(ERROR_REF, detail.as_bytes());
        if error.is_none() {
            error = Some(ContractError::runtime(detail.clone()));
        }
        ok = false;
    }
    let transaction_rolled_back = if !ok {
        match journal.rollback(session) {
            Ok(()) => true,
            Err(rollback_error) => {
                let detail = format!(
                    "{}; rollback failed: {rollback_error}",
                    failure_detail.unwrap_or_else(|| "JavaScript execution failed".to_owned())
                );
                session.recovery.put_key(ERROR_REF, detail.as_bytes());
                error = Some(ContractError::runtime(detail));
                false
            }
        }
    } else {
        false
    };
    let summary = if result_text.len() <= MAX_OUTPUT_BYTES {
        session.recovery.put_key(RESULT_REF, result_text.as_bytes());
        result_text
    } else {
        let reference = session.recovery.put_content_ref(result_text.as_bytes());
        let summary = json!({"ok": true, "result_ref": reference}).to_string();
        session.recovery.put_key(RESULT_REF, summary.as_bytes());
        summary
    };
    RuntimeOutcome {
        ok,
        label: "javascript".to_owned(),
        steps_run: snapshot.steps.len(),
        summary,
        primary_ref: snapshot.last_ref,
        steps: snapshot.steps,
        dag: None,
        internal_actions: session.op_count.saturating_sub(start_ops),
        logical_ops: snapshot.metrics.logical_ops.min(MAX_LOGICAL_OPS),
        physical_ops: snapshot.metrics.physical_ops.min(MAX_PHYSICAL_OPS),
        batched_ops: snapshot.metrics.batched_ops,
        parallel_groups: 0,
        parallel_wall_ms: 0,
        transaction_rolled_back,
        wall_ms: started.elapsed().as_millis() as u64,
        error,
    }
}

fn js_init_error(
    session: &mut FSZeroSession,
    start_ops: u32,
    summary: String,
    error: ContractError,
) -> RuntimeOutcome {
    put_contract_error(session, &error);
    session.recovery.put_key(ERROR_REF, summary.as_bytes());
    session.recovery.put_key(RESULT_REF, summary.as_bytes());
    RuntimeOutcome {
        ok: false,
        label: "javascript".to_owned(),
        steps_run: 0,
        summary,
        primary_ref: ERROR_REF.to_owned(),
        steps: Vec::new(),
        dag: None,
        internal_actions: session.op_count.saturating_sub(start_ops),
        logical_ops: 0,
        physical_ops: 0,
        batched_ops: 0,
        parallel_groups: 0,
        parallel_wall_ms: 0,
        transaction_rolled_back: false,
        wall_ms: 0,
        error: Some(error),
    }
}

fn host_error_detail(error: &HostError) -> String {
    error.to_string()
}

/// Wrap a legacy FSZero plan as an async hub plan while preserving its
/// expression, callable, and statement forms. Hub capability calls remain
/// promises, so `await` is intentionally preserved.
fn wrap_user_code(code: &str) -> String {
    let trimmed = code.trim();
    if let Some(rest) = trimmed.strip_prefix("export default") {
        return format!(
            "(async () => {{ const __user = {rest}; return await __user({{ fs, ctx, args: {{}} }}); }})()"
        );
    }
    let arrow_callable = trimmed.contains("=>")
        && ![
            "const ", "let ", "var ", "return ", "throw ", "for ", "while ", "try ",
        ]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix));
    if trimmed.starts_with("async function")
        || trimmed.starts_with("function")
        || trimmed.starts_with("async (")
        || trimmed.starts_with('(')
        || arrow_callable
    {
        return format!(
            "(async () => {{ const __user = ({trimmed}); return await __user({{ fs, ctx, args: {{}} }}); }})()"
        );
    }
    let masked = lower_code_plan(trimmed);
    let is_semi_or_ws = |character: char| character == ';' || character.is_whitespace();
    let is_body = [
        "return", "throw", "const", "let", "var", "for", "while", "try",
    ]
    .iter()
    .any(|keyword| contains_identifier(&masked, keyword))
        || masked.trim_end_matches(is_semi_or_ws).contains(';');
    if is_body {
        format!("(async () => {{ {trimmed} }})()")
    } else {
        let expression = trimmed.trim_end_matches(is_semi_or_ws);
        format!("(async () => {{ return await ({expression}); }})()")
    }
}

fn classify_host_error(error: &HostError, detail: &str) -> ContractError {
    match error {
        HostError::Plan(_) | HostError::Limits(_) => ContractError::validation(detail),
        HostError::Cancelled => ContractError::cancelled(detail),
        HostError::DeadlineExceeded => ContractError::deadline(detail),
        HostError::MicrotaskLimit | HostError::FuelExhausted => ContractError::sandbox(detail),
        HostError::Connector(_) => ContractError::runtime(detail),
        _ => ContractError::runtime(detail),
    }
}

/// Keep FSZero's stable policy failure classes while the hub owns JS runtime
/// execution. Strings and comments are ignored to avoid false positives.
fn denied_sandbox_category(code: &str) -> Option<&'static str> {
    let lower = lower_code_plan(code);
    if contains_identifier_call(&lower, "fetch")
        || contains_identifier(&lower, "xmlhttprequest")
        || contains_identifier(&lower, "websocket")
    {
        Some("network/fetch")
    } else if contains_member_path(&lower, "process", "env") {
        Some("env")
    } else if contains_identifier(&lower, "child_process")
        || contains_identifier(&lower, "subprocess")
        || contains_identifier_call(&lower, "spawn")
        || contains_identifier_call(&lower, "exec")
    {
        Some("process/spawn")
    } else if contains_require_or_import_target(code, "require", "fs")
        || lower.contains("/etc/passwd")
    {
        Some("raw host FS")
    } else if contains_identifier(&lower, "sqlite")
        || contains_member_path(&lower, "globalthis", "db")
        || contains_member_path(&lower, "globalthis", "store")
    {
        Some("direct DB/store")
    } else if contains_require_or_import_target(code, "require", "node:")
        || contains_require_or_import_target(code, "import", "node:")
    {
        Some("native modules")
    } else if contains_identifier_call(&lower, "settimeout")
        || contains_identifier_call(&lower, "setinterval")
    {
        Some("timers")
    } else {
        None
    }
}

/// Lowercase the plan while blanking string/template literal *contents* so
/// policy scans only see code positions. Escaped quotes/backticks stay inside
/// the literal. Template `${ ... }` interpolations are *not* blanked — they
/// are real code and must still be policy-scanned.
fn lower_code_plan(code: &str) -> String {
    let chars: Vec<char> = code.chars().collect();
    let mut out = String::with_capacity(code.len());
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '\'' || ch == '"' || ch == '`' {
            let quote = ch;
            out.push(' ');
            i += 1;
            let mut escaped = false;
            while i < chars.len() {
                let c = chars[i];
                if escaped {
                    escaped = false;
                    out.push(' ');
                    i += 1;
                    continue;
                }
                if c == '\\' {
                    escaped = true;
                    out.push(' ');
                    i += 1;
                    continue;
                }
                if c == quote {
                    out.push(' ');
                    i += 1;
                    break;
                }
                // Template literal: `${expr}` is code, not content.
                if quote == '`' && c == '$' && chars.get(i + 1) == Some(&'{') {
                    out.push(' '); // $
                    out.push(' '); // {
                    i += 2;
                    let mut depth = 1;
                    let mut inner_quote: Option<char> = None;
                    let mut inner_esc = false;
                    while i < chars.len() && depth > 0 {
                        let ic = chars[i];
                        if let Some(iq) = inner_quote {
                            if inner_esc {
                                inner_esc = false;
                                out.push(' ');
                            } else if ic == '\\' {
                                inner_esc = true;
                                out.push(' ');
                            } else if ic == iq {
                                inner_quote = None;
                                out.push(' ');
                            } else {
                                out.push(' ');
                            }
                            i += 1;
                            continue;
                        }
                        if ic == '\'' || ic == '"' || ic == '`' {
                            inner_quote = Some(ic);
                            out.push(' ');
                            i += 1;
                            continue;
                        }
                        if ic == '{' {
                            depth += 1;
                            out.push(ic);
                        } else if ic == '}' {
                            depth -= 1;
                            if depth == 0 {
                                out.push(' ');
                                i += 1;
                                break;
                            }
                            out.push(ic);
                        } else {
                            out.extend(ic.to_lowercase());
                        }
                        i += 1;
                    }
                    continue;
                }
                out.push(' ');
                i += 1;
            }
            continue;
        }
        out.extend(ch.to_lowercase());
        i += 1;
    }
    out
}

fn find_bounded_ident(code: &str, ident: &str, require_call: bool) -> bool {
    let mut rest = code;
    while let Some(pos) = rest.find(ident) {
        let before = rest[..pos].chars().next_back();
        let after_index = pos + ident.len();
        let after = rest[after_index..].chars().next();
        if !is_ident_char(before) && !is_ident_char(after) {
            if !require_call || rest[after_index..].trim_start().starts_with('(') {
                return true;
            }
        }
        rest = &rest[after_index..];
    }
    false
}

fn contains_identifier_call(code: &str, ident: &str) -> bool {
    find_bounded_ident(code, ident, true)
}

fn contains_identifier(code: &str, ident: &str) -> bool {
    find_bounded_ident(code, ident, false)
}

fn contains_member_path(code: &str, object: &str, member: &str) -> bool {
    contains_identifier(code, &format!("{object}.{member}"))
}

/// Match `call('target…` / `call("target…` only in code positions — never
/// inside string or template literal contents (residual 7nl/ah3 false positives
/// when README bodies quoted `require('fs')` or fenced JS samples).
fn contains_require_or_import_target(code: &str, call: &str, target: &str) -> bool {
    let chars: Vec<char> = code.chars().map(|c| c.to_ascii_lowercase()).collect();
    let call_chars: Vec<char> = call.chars().map(|c| c.to_ascii_lowercase()).collect();
    let target_chars: Vec<char> = target.chars().map(|c| c.to_ascii_lowercase()).collect();
    let call_len = call_chars.len();
    let target_len = target_chars.len();
    if call_len == 0 || target_len == 0 {
        return false;
    }
    let n = chars.len();
    let mut i = 0;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    while i < n {
        let ch = chars[i];
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == q {
                quote = None;
            } else if q == '`' && ch == '$' && chars.get(i + 1) == Some(&'{') {
                // Enter interpolation as code: leave template quote mode for
                // the `${…}` span by scanning brace-balanced with nested quotes.
                i += 2;
                let mut depth = 1;
                let mut iq: Option<char> = None;
                let mut ie = false;
                while i < n && depth > 0 {
                    let c = chars[i];
                    if let Some(q2) = iq {
                        if ie {
                            ie = false;
                        } else if c == '\\' {
                            ie = true;
                        } else if c == q2 {
                            iq = None;
                        }
                        i += 1;
                        continue;
                    }
                    if c == '\'' || c == '"' || c == '`' {
                        iq = Some(c);
                        i += 1;
                        continue;
                    }
                    if c == '{' {
                        depth += 1;
                    } else if c == '}' {
                        depth -= 1;
                        if depth == 0 {
                            i += 1;
                            break;
                        }
                    }
                    // Still inside `${…}`: attempt call match at this code pos.
                    if depth > 0 && match_call_target_at(&chars, i, &call_chars, &target_chars) {
                        return true;
                    }
                    i += 1;
                }
                continue;
            }
            i += 1;
            continue;
        }
        if ch == '\'' || ch == '"' || ch == '`' {
            quote = Some(ch);
            i += 1;
            continue;
        }
        if match_call_target_at(&chars, i, &call_chars, &target_chars) {
            return true;
        }
        i += 1;
    }
    false
}

fn match_call_target_at(
    chars: &[char],
    i: usize,
    call_chars: &[char],
    target_chars: &[char],
) -> bool {
    let call_len = call_chars.len();
    let target_len = target_chars.len();
    let n = chars.len();
    if i + call_len > n {
        return false;
    }
    if chars[i..i + call_len] != call_chars[..] {
        return false;
    }
    let before = if i == 0 { None } else { Some(chars[i - 1]) };
    let after = chars.get(i + call_len).copied();
    if is_ident_char(before) || is_ident_char(after) {
        return false;
    }
    let mut j = i + call_len;
    while j < n && chars[j].is_whitespace() {
        j += 1;
    }
    if j >= n || chars[j] != '(' {
        return false;
    }
    j += 1;
    while j < n && chars[j].is_whitespace() {
        j += 1;
    }
    if j >= n {
        return false;
    }
    let q = chars[j];
    if q != '\'' && q != '"' {
        return false;
    }
    j += 1;
    if j + target_len > n {
        return false;
    }
    chars[j..j + target_len] == target_chars[..]
}

fn is_ident_char(ch: Option<char>) -> bool {
    ch.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}
