use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value as JsonValue;
use zero_store::SharedCas;

#[cfg(feature = "quickjs")]
use crate::wrap_plan;
use crate::{HostLimits, LimitError, PlanError};

static RUNTIME_CREATIONS: AtomicU64 = AtomicU64::new(0);

pub fn runtime_creation_count() -> u64 {
    RUNTIME_CREATIONS.load(Ordering::Relaxed)
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CapabilityDescriptor {
    pub surface: String,
    pub method: String,
}

impl CapabilityDescriptor {
    pub fn new(surface: impl Into<String>, method: impl Into<String>) -> Self {
        Self {
            surface: surface.into(),
            method: method.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlobalRegistration {
    pub root: String,
    pub capabilities: Vec<CapabilityDescriptor>,
}

impl GlobalRegistration {
    pub fn zero(capabilities: Vec<CapabilityDescriptor>) -> Self {
        Self {
            root: "zero".to_owned(),
            capabilities,
        }
    }

    pub fn validate(&self) -> Result<(), RegistrationError> {
        validate_identifier(&self.root)
            .map_err(|_| RegistrationError::InvalidGlobal(self.root.clone()))?;
        let mut seen = BTreeSet::new();
        for capability in &self.capabilities {
            if validate_identifier(&capability.surface).is_err()
                || validate_identifier(&capability.method).is_err()
            {
                return Err(RegistrationError::InvalidCapability(capability.clone()));
            }
            if !seen.insert(capability.clone()) {
                return Err(RegistrationError::DuplicateCapability(capability.clone()));
            }
        }
        Ok(())
    }
}

fn validate_identifier(value: &str) -> Result<(), ()> {
    if matches!(value, "__proto__" | "prototype" | "constructor") {
        return Err(());
    }
    let mut chars = value.chars();
    let first = chars.next().ok_or(())?;
    if !(first == '_' || first == '$' || first.is_ascii_alphabetic()) {
        return Err(());
    }
    if chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        Err(())
    }
}

/// Deadline and serialization budget supplied to a connector dispatch.
///
/// Connectors must cooperatively enforce this context for a complete timeout:
/// Rust callbacks cannot be safely preempted while they are executing.
#[derive(Clone, Copy, Debug)]
pub struct DispatchContext {
    pub deadline: Instant,
    pub max_json_bytes: usize,
}

impl DispatchContext {
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.deadline
    }
}

pub trait Connector {
    fn call(
        &self,
        capability: &CapabilityDescriptor,
        args_json: &str,
        context: DispatchContext,
    ) -> Result<String, ConnectorError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectorError {
    message: String,
}

impl ConnectorError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ConnectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ConnectorError {}

/// Schema tag of the envelope returned in place of an oversized result.
pub const RESULT_SPILL_SCHEMA: &str = "zerostack.codemode.result_spill.v1";

/// Upper bound on the inline preview carried beside a spilled result ref.
pub const RESULT_SPILL_PREVIEW_BYTES: usize = 256;

/// Conservative byte ceiling for aggregate values shown directly to a model.
/// Receipts label bytes only; tokenizer-specific visible-token certification
/// remains a separate TokenZero boundary.
pub const DEFAULT_MAX_VISIBLE_RESULT_BYTES: usize = 1_024;

/// Hard ceiling for the serialized spill envelope itself, including its
/// exact-byte receipt. Crossing this bound fails typed instead of leaking text.
pub const MAX_RESULT_SPILL_ENVELOPE_BYTES: usize = 2_000;

/// Bound for typed error text emitted by a model-facing adapter.
pub const MAX_VISIBLE_ERROR_BYTES: usize = 1_024;

/// Bound untrusted error text without splitting UTF-8. The typed error code
/// remains the authority; the human text is diagnostic only.
pub fn finalize_visible_error(value: &str) -> String {
    if value.len() <= MAX_VISIBLE_ERROR_BYTES {
        return value.to_owned();
    }
    const SUFFIX: &str = "... [truncated]";
    let mut end = MAX_VISIBLE_ERROR_BYTES.saturating_sub(SUFFIX.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &value[..end], SUFFIX)
}

/// Fields every capability result exposes for inline output and for ref-ed
/// output, whichever route produced the result.
pub const CANONICAL_RESULT_FIELDS: &[&str] = &["text", "ref"];

/// Source fields, in precedence order, that a connector may use for inline
/// output. The first one present as a string is mirrored onto `text`.
pub const CANONICAL_TEXT_ALIASES: &[&str] = &["text", "visible", "result", "stdout"];

/// Source fields, in precedence order, that a connector may use for ref-ed
/// output. The first one present as a string is mirrored onto `ref`.
pub const CANONICAL_REF_ALIASES: &[&str] = &["ref", "stdout_ref", "combined_ref"];

#[derive(Clone, Debug)]
pub struct Host {
    limits: HostLimits,
    registration: GlobalRegistration,
    spill_root: Option<PathBuf>,
    max_visible_result_bytes: usize,
}

impl Host {
    pub fn new(limits: HostLimits, registration: GlobalRegistration) -> Result<Self, HostError> {
        limits.validate().map_err(HostError::Limits)?;
        registration.validate().map_err(HostError::Registration)?;
        #[cfg(not(feature = "quickjs"))]
        {
            let _ = registration;
            Err(HostError::QuickJsDisabled)
        }
        #[cfg(feature = "quickjs")]
        {
            Ok(Self {
                max_visible_result_bytes: limits.max_json_bytes,
                limits,
                registration,
                spill_root: None,
            })
        }
    }

    /// Publish results larger than `max_json_bytes` into the content-addressed
    /// store rooted at `cas_root` and return a ref plus a bounded preview,
    /// instead of failing with [HostError::ResultTooLarge].
    pub fn with_result_spill(mut self, cas_root: impl Into<PathBuf>) -> Self {
        self.spill_root = Some(cas_root.into());
        self
    }

    /// Set the finalized result byte budget independently from connector frame
    /// bounds. A zero budget is rejected loudly.
    pub fn with_visible_result_budget(mut self, max_bytes: usize) -> Result<Self, HostError> {
        if max_bytes == 0 {
            return Err(HostError::Limits(LimitError::Zero(
                "max_visible_result_bytes",
            )));
        }
        self.max_visible_result_bytes = max_bytes;
        Ok(self)
    }

    pub fn limits(&self) -> HostLimits {
        self.limits
    }

    pub fn registration(&self) -> &GlobalRegistration {
        &self.registration
    }

    #[cfg(feature = "quickjs")]
    pub fn execute(
        &self,
        plan: &str,
        connector: Rc<dyn Connector>,
    ) -> Result<JsonValue, HostError> {
        self.execute_with_cancel(plan, connector, Arc::new(AtomicBool::new(false)))
    }

    #[cfg(feature = "quickjs")]
    pub fn execute_with_cancel(
        &self,
        plan: &str,
        connector: Rc<dyn Connector>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<JsonValue, HostError> {
        quickjs::execute(self, plan, connector, cancelled, self.limits.wall_timeout)
    }

    #[cfg(feature = "quickjs")]
    pub fn execute_with_cancel_timeout(
        &self,
        plan: &str,
        connector: Rc<dyn Connector>,
        cancelled: Arc<AtomicBool>,
        timeout: Duration,
    ) -> Result<JsonValue, HostError> {
        quickjs::execute(
            self,
            plan,
            connector,
            cancelled,
            timeout.min(self.limits.wall_timeout),
        )
    }

    #[cfg(not(feature = "quickjs"))]
    pub fn execute(
        &self,
        _plan: &str,
        _connector: Rc<dyn Connector>,
    ) -> Result<JsonValue, HostError> {
        Err(HostError::QuickJsDisabled)
    }

    #[cfg(not(feature = "quickjs"))]
    pub fn execute_with_cancel_timeout(
        &self,
        _plan: &str,
        _connector: Rc<dyn Connector>,
        _cancelled: Arc<AtomicBool>,
        _timeout: Duration,
    ) -> Result<JsonValue, HostError> {
        Err(HostError::QuickJsDisabled)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistrationError {
    InvalidGlobal(String),
    InvalidCapability(CapabilityDescriptor),
    DuplicateCapability(CapabilityDescriptor),
}

impl fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGlobal(name) => write!(f, "invalid global name: {name}"),
            Self::InvalidCapability(cap) => {
                write!(f, "invalid capability: {}.{}", cap.surface, cap.method)
            }
            Self::DuplicateCapability(cap) => {
                write!(f, "duplicate capability: {}.{}", cap.surface, cap.method)
            }
        }
    }
}

impl std::error::Error for RegistrationError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostError {
    QuickJsDisabled,
    Limits(LimitError),
    Registration(RegistrationError),
    Plan(PlanError),
    Runtime(String),
    JavaScript(String),
    MethodNotFound(String),
    SurfaceNotFound(String),
    Connector(String),
    Json(String),
    ResultTooLarge { actual: usize, maximum: usize },
    ResultSpill(String),
    MicrotaskLimit,
    DeadlineExceeded,
    FuelExhausted,
    Cancelled,
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QuickJsDisabled => f.write_str("QuickJS support is disabled"),
            Self::Limits(error) => write!(f, "invalid limits: {error}"),
            Self::Registration(error) => write!(f, "invalid registration: {error}"),
            Self::Plan(error) => write!(f, "invalid plan: {error}"),
            Self::Runtime(message) => write!(f, "runtime error: {message}"),
            Self::JavaScript(message)
            | Self::MethodNotFound(message)
            | Self::SurfaceNotFound(message) => write!(f, "JavaScript exception: {message}"),
            Self::Connector(message) => write!(f, "connector error: {message}"),
            Self::Json(message) => write!(f, "JSON error: {message}"),
            Self::ResultTooLarge { actual, maximum } => {
                write!(f, "result is {actual} bytes; maximum is {maximum}")
            }
            Self::ResultSpill(message) => write!(f, "result spill failed: {message}"),
            Self::MicrotaskLimit => f.write_str("microtask ceiling exceeded"),
            Self::DeadlineExceeded => f.write_str("wall-clock deadline exceeded"),
            Self::FuelExhausted => f.write_str("instruction budget exhausted"),
            Self::Cancelled => f.write_str("execution cancelled"),
        }
    }
}

impl std::error::Error for HostError {}

/// Publish an oversized encoded result into the CAS and describe it with a ref
/// plus a bounded preview, so a large final value degrades to a fetchable
/// reference instead of a hard framing error.
fn spill_result(cas_root: &Path, encoded: &str) -> Result<JsonValue, HostError> {
    let cas = SharedCas::open_labeled(cas_root, "codemode-result-spill");
    let hash = cas
        .put(encoded.as_bytes())
        .map_err(|error| HostError::ResultSpill(error.to_string()))?;
    let reference = format!("tz://blob/{hash}");
    let preview = "[exact result omitted; expand ref]";
    debug_assert!(preview.len() <= RESULT_SPILL_PREVIEW_BYTES);
    let raw_bytes = encoded.len();
    let mut envelope = serde_json::json!({
        "schema": RESULT_SPILL_SCHEMA,
        "spilled": true,
        "ref": reference,
        "sha256": hash,
        "bytes": raw_bytes,
        "preview": preview,
        "previewBytes": preview.len(),
        "previewTruncated": true,
        "receipt": {
            "schema": "zerostack.codemode.result_finalization_receipt.v1",
            "rawResultJsonBytes": raw_bytes,
            "inlineResultBytes": 0,
            "omittedBehindExactRefBytes": raw_bytes,
            "typedFailureBytes": 0,
            "finalizedValueJsonBytes": 0,
            "visibleTokenCount": JsonValue::Null,
            "visibleTokenCountStatus": "requires_tokenzero_certification",
            "savingsBytes": 0,
            "integrity": "sha256-cas",
        },
    });
    for _ in 0..16 {
        let visible_bytes = serde_json::to_vec(&envelope)
            .map_err(|error| HostError::ResultSpill(error.to_string()))?
            .len();
        let savings_bytes = raw_bytes.saturating_sub(visible_bytes);
        let receipt = envelope
            .get_mut("receipt")
            .and_then(JsonValue::as_object_mut)
            .ok_or_else(|| HostError::ResultSpill("missing finalization receipt".into()))?;
        let prior_visible = receipt
            .get("finalizedValueJsonBytes")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0) as usize;
        let prior_savings = receipt
            .get("savingsBytes")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0) as usize;
        if prior_visible == visible_bytes && prior_savings == savings_bytes {
            if visible_bytes > MAX_RESULT_SPILL_ENVELOPE_BYTES {
                return Err(HostError::ResultSpill(format!(
                    "finalized spill envelope is {visible_bytes} bytes; maximum is {MAX_RESULT_SPILL_ENVELOPE_BYTES}"
                )));
            }
            return Ok(envelope);
        }
        receipt.insert("finalizedValueJsonBytes".into(), visible_bytes.into());
        receipt.insert("savingsBytes".into(), savings_bytes.into());
    }
    Err(HostError::ResultSpill(
        "finalized spill receipt length did not converge".into(),
    ))
}

#[cfg(feature = "quickjs")]
mod quickjs {
    use std::collections::BTreeMap;
    use std::sync::atomic::AtomicU64;

    use rquickjs::function::Rest;
    use rquickjs::promise::PromiseState;
    use rquickjs::{Array, Context, Ctx, Function, Object, Persistent, Promise, Runtime, Value};

    use super::*;

    pub(super) fn execute(
        host: &Host,
        plan: &str,
        connector: Rc<dyn Connector>,
        cancelled: Arc<AtomicBool>,
        wall_timeout: Duration,
    ) -> Result<JsonValue, HostError> {
        let wrapped = wrap_plan(plan, host.limits.max_plan_bytes).map_err(HostError::Plan)?;
        let runtime = Runtime::new().map_err(runtime_error)?;
        RUNTIME_CREATIONS.fetch_add(1, Ordering::Relaxed);
        runtime.set_memory_limit(host.limits.memory_bytes);
        runtime.set_max_stack_size(host.limits.stack_bytes);

        let deadline = Instant::now() + wall_timeout;
        let fuel = Arc::new(AtomicU64::new(host.limits.instruction_budget));
        let timed_out = Arc::new(AtomicBool::new(false));
        let dispatch_expired = Arc::new(AtomicBool::new(false));
        let exhausted = Arc::new(AtomicBool::new(false));
        let interrupt_fuel = Arc::clone(&fuel);
        let interrupt_timeout = Arc::clone(&timed_out);
        let interrupt_exhausted = Arc::clone(&exhausted);
        let interrupt_cancelled = Arc::clone(&cancelled);
        runtime.set_interrupt_handler(Some(Box::new(move || {
            if interrupt_cancelled.load(Ordering::Relaxed) {
                return true;
            }
            if Instant::now() >= deadline {
                interrupt_timeout.store(true, Ordering::Relaxed);
                return true;
            }
            if interrupt_fuel
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    value.checked_sub(1)
                })
                .is_err()
            {
                interrupt_exhausted.store(true, Ordering::Relaxed);
                return true;
            }
            false
        })));

        let context = Context::full(&runtime).map_err(runtime_error)?;
        let persistent: Result<Persistent<Promise<'static>>, HostError> = context.with(|ctx| {
            install_globals(
                ctx.clone(),
                &host.registration,
                connector,
                DispatchContext {
                    deadline,
                    max_json_bytes: host.limits.max_json_bytes,
                },
                Arc::clone(&dispatch_expired),
            )?;
            let promise: Promise<'_> = ctx
                .eval(wrapped.as_str())
                .map_err(|error| normalized_js_error(&ctx, error))?;
            Ok(Persistent::save(&ctx, promise))
        });

        check_limits(
            &timed_out,
            &dispatch_expired,
            &exhausted,
            &cancelled,
            deadline,
        )?;
        let persistent = persistent?;

        let mut executed_jobs = 0;
        loop {
            let state = context.with(|ctx| {
                persistent
                    .clone()
                    .restore(&ctx)
                    .map(|promise| promise.state())
                    .map_err(js_error)
            })?;
            if state != PromiseState::Pending {
                break;
            }
            if executed_jobs >= host.limits.microtask_ceiling {
                return Err(HostError::MicrotaskLimit);
            }
            check_limits(
                &timed_out,
                &dispatch_expired,
                &exhausted,
                &cancelled,
                deadline,
            )?;
            match runtime.execute_pending_job() {
                Ok(true) => executed_jobs += 1,
                Ok(false) => {
                    return Err(HostError::JavaScript(
                        "plan promise did not settle".to_owned(),
                    ));
                }
                Err(error) => {
                    check_limits(
                        &timed_out,
                        &dispatch_expired,
                        &exhausted,
                        &cancelled,
                        deadline,
                    )?;
                    return Err(HostError::JavaScript(error.to_string()));
                }
            }
            check_limits(
                &timed_out,
                &dispatch_expired,
                &exhausted,
                &cancelled,
                deadline,
            )?;
        }

        check_limits(
            &timed_out,
            &dispatch_expired,
            &exhausted,
            &cancelled,
            deadline,
        )?;
        let encoded = context.with(move |ctx| {
            let promise = persistent.restore(&ctx).map_err(js_error)?;
            match promise.result::<String>() {
                Some(Ok(encoded)) => Ok(encoded),
                Some(Err(error)) => Err(normalized_js_error(&ctx, error)),
                None => Err(HostError::JavaScript(
                    "plan promise did not settle".to_owned(),
                )),
            }
        })?;
        if encoded.len() > host.max_visible_result_bytes {
            return match host.spill_root.as_deref() {
                Some(root) => spill_result(root, &encoded),
                None => Err(HostError::ResultTooLarge {
                    actual: encoded.len(),
                    maximum: host.max_visible_result_bytes,
                }),
            };
        }
        serde_json::from_str(&encoded).map_err(|error| HostError::Json(error.to_string()))
    }

    fn check_limits(
        timed_out: &AtomicBool,
        dispatch_expired: &AtomicBool,
        exhausted: &AtomicBool,
        cancelled: &AtomicBool,
        deadline: Instant,
    ) -> Result<(), HostError> {
        if cancelled.load(Ordering::Relaxed) {
            return Err(HostError::Cancelled);
        }
        if timed_out.load(Ordering::Relaxed)
            || dispatch_expired.load(Ordering::Relaxed)
            || Instant::now() >= deadline
        {
            return Err(HostError::DeadlineExceeded);
        }
        if exhausted.load(Ordering::Relaxed) {
            return Err(HostError::FuelExhausted);
        }
        Ok(())
    }

    fn null_object<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>, HostError> {
        ctx.eval::<Object<'js>, _>("Object.create(null)")
            .map_err(|error| normalized_js_error(ctx, error))
    }

    /// Wraps every capability result so reading a property the host never
    /// returned throws instead of yielding undefined. A mistyped field name is a
    /// caller bug worth one error, not a silent empty value.
    const STRICT_RESULT_WRAPPER: &str = r#"(() => {
"use strict";
const TEXT_ALIASES = __TEXT_ALIASES__;
const REF_ALIASES = __REF_ALIASES__;
const OPTIONAL_RESULT_FIELDS = new Set(REF_ALIASES);
const firstString = (target, names) => {
    for (const name of names) {
        if (Object.prototype.hasOwnProperty.call(target, name) && typeof target[name] === "string") {
            return target[name];
        }
    }
    return undefined;
};
// Mirrors whichever alias the producing route used onto the canonical field,
// so a plan reads output identically whichever route served the call.
const normalize = (value) => {
    if (value === null || typeof value !== "object" || Array.isArray(value)) {
        return value;
    }
    const text = firstString(value, TEXT_ALIASES);
    if (text !== undefined && !Object.prototype.hasOwnProperty.call(value, "text")) {
        value.text = text;
    }
    const reference = firstString(value, REF_ALIASES);
    if (reference !== undefined && !Object.prototype.hasOwnProperty.call(value, "ref")) {
        value.ref = reference;
    }
    return value;
};
const guard = (value, label) => {
    if (value === null || typeof value !== "object" || Array.isArray(value)) {
        return value;
    }
    return new Proxy(value, {
        get(target, property) {
            if (typeof property === "symbol") {
                return Reflect.get(target, property);
            }
            if (Object.prototype.hasOwnProperty.call(target, property)) {
                return guard(target[property], label + "." + property);
            }
            if (
                property === "then" ||
                property === "toJSON" ||
                OPTIONAL_RESULT_FIELDS.has(property)
            ) {
                return undefined;
            }
            const keys = Object.keys(target);
            throw new TypeError(
                "unknown property '" + property + "' on " + label +
                "; available properties: " + (keys.length ? keys.join(", ") : "(none)")
            );
        },
    });
};
// Async so every capability call returns a real Promise: plans can use
// Promise.all/.then over calls. The dispatch itself still runs eagerly at
// invocation, so await semantics are unchanged.
return (call, label) => async (...args) => guard(normalize(call(...args)), label);
})()"#;

    fn strict_result_wrapper<'js>(ctx: &Ctx<'js>) -> Result<Function<'js>, HostError> {
        let source = STRICT_RESULT_WRAPPER
            .replace("__TEXT_ALIASES__", &alias_literal(CANONICAL_TEXT_ALIASES))
            .replace("__REF_ALIASES__", &alias_literal(CANONICAL_REF_ALIASES));
        ctx.eval::<Function<'js>, _>(source)
            .map_err(|error| normalized_js_error(ctx, error))
    }

    /// Guards the aggregate capability tree itself. An unknown surface or
    /// method is a caller bug, never a reason to degrade to catalog search.
    const STRICT_CAPABILITY_WRAPPER: &str = r#"(() => {
"use strict";
const ignored = new Set(["then", "toJSON", "toString"]);
const distance = (left, right) => {
    let row = Array.from({length: right.length + 1}, (_, index) => index);
    for (let leftIndex = 0; leftIndex < left.length; leftIndex += 1) {
        const next = [leftIndex + 1];
        for (let rightIndex = 0; rightIndex < right.length; rightIndex += 1) {
            next.push(Math.min(
                next[rightIndex] + 1,
                row[rightIndex + 1] + 1,
                row[rightIndex] + (left[leftIndex] === right[rightIndex] ? 0 : 1),
            ));
        }
        row = next;
    }
    return row[right.length];
};
return (target, label, kind) => new Proxy(target, {
    get(target, property) {
        if (typeof property === "symbol") {
            return Reflect.get(target, property);
        }
        if (Object.prototype.hasOwnProperty.call(target, property)) {
            return target[property];
        }
        if (ignored.has(property)) {
            return undefined;
        }
        const closest = Object.keys(target)
            .map(name => [distance(property, name), name])
            .sort((left, right) => left[0] - right[0] || (left[1] < right[1] ? -1 : left[1] > right[1] ? 1 : 0))
            .slice(0, 3)
            .map(entry => entry[1]);
        const plural = kind === "method" ? "methods" : "surfaces";
        throw new TypeError(
            kind + "_not_found: unknown " + kind + " '" + property + "' on " + label +
            "; closest " + plural + ": " + (closest.length ? closest.join(", ") : "(none)")
        );
    },
});
})()"#;

    fn strict_capability_wrapper<'js>(ctx: &Ctx<'js>) -> Result<Function<'js>, HostError> {
        ctx.eval::<Function<'js>, _>(STRICT_CAPABILITY_WRAPPER)
            .map_err(|error| normalized_js_error(ctx, error))
    }

    fn alias_literal(aliases: &[&str]) -> String {
        serde_json::to_string(aliases).expect("alias names serialize")
    }

    /// Encodes a capability call's argument list for the connector. A single
    /// argument keeps its own shape so existing one-argument capabilities are
    /// unchanged; extra arguments (an `opts` bag, for example) are forwarded as
    /// a JSON array instead of being dropped before dispatch.
    fn call_arguments<'js>(
        ctx: &Ctx<'js>,
        mut args: Vec<Value<'js>>,
    ) -> Result<Value<'js>, rquickjs::Error> {
        if args.len() == 1 {
            return Ok(args.remove(0));
        }
        let array = Array::new(ctx.clone())?;
        for (index, value) in args.into_iter().enumerate() {
            array.set(index, value)?;
        }
        Ok(array.into_value())
    }

    fn install_globals<'js>(
        ctx: Ctx<'js>,
        registration: &GlobalRegistration,
        connector: Rc<dyn Connector>,
        dispatch_context: DispatchContext,
        dispatch_expired: Arc<AtomicBool>,
    ) -> Result<(), HostError> {
        let root = null_object(&ctx)?;
        let strict_result = strict_result_wrapper(&ctx)?;
        let strict_capability = strict_capability_wrapper(&ctx)?;
        let mut surfaces: BTreeMap<String, Object<'js>> = BTreeMap::new();

        for capability in &registration.capabilities {
            let surface = match surfaces.entry(capability.surface.clone()) {
                std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(null_object(&ctx)?)
                }
            }
            .clone();
            let descriptor = capability.clone();
            let connector = Rc::clone(&connector);
            let expired = Arc::clone(&dispatch_expired);
            let function =
                Function::new(ctx.clone(), move |ctx: Ctx<'js>, args: Rest<Value<'js>>| {
                    let args = call_arguments(&ctx, args.0)?;
                    let Some(json) = ctx.json_stringify(args)? else {
                        return Err(rquickjs::Error::new_from_js_message(
                            "value",
                            "JSON",
                            "arguments are not JSON-serializable",
                        ));
                    };
                    let encoded = json.to_string()?;
                    if encoded.len() > dispatch_context.max_json_bytes {
                        return Err(rquickjs::Error::new_from_js_message(
                            "JSON",
                            "connector",
                            "arguments exceed JSON limit",
                        ));
                    }
                    if dispatch_context.is_expired() {
                        expired.store(true, Ordering::Relaxed);
                        return Err(rquickjs::Error::new_from_js_message(
                            "deadline",
                            "connector",
                            "wall-clock deadline exceeded",
                        ));
                    }
                    let result = connector.call(&descriptor, &encoded, dispatch_context);
                    if dispatch_context.is_expired() {
                        expired.store(true, Ordering::Relaxed);
                        return Err(rquickjs::Error::new_from_js_message(
                            "deadline",
                            "connector",
                            "wall-clock deadline exceeded",
                        ));
                    }
                    let encoded = result.map_err(|error| {
                        rquickjs::Error::new_from_js_message(
                            "connector",
                            "JavaScript",
                            error.to_string(),
                        )
                    })?;
                    if encoded.len() > dispatch_context.max_json_bytes {
                        return Err(rquickjs::Error::new_from_js_message(
                            "connector",
                            "JSON",
                            "result exceeds JSON limit",
                        ));
                    }
                    ctx.json_parse(encoded)
                })
                .map_err(js_error)?;
            let label = format!("{}.{} result", capability.surface, capability.method);
            let guarded: Function<'js> = strict_result
                .call((function, label))
                .map_err(|error| normalized_js_error(&ctx, error))?;
            surface
                .set(capability.method.as_str(), guarded)
                .map_err(js_error)?;
        }

        for (name, surface) in surfaces {
            let label = format!("{}.{}", registration.root, name);
            let guarded: Object<'js> = strict_capability
                .call((surface, label, "method"))
                .map_err(|error| normalized_js_error(&ctx, error))?;
            root.set(name, guarded).map_err(js_error)?;
        }
        let guarded_root: Object<'js> = strict_capability
            .call((root, registration.root.clone(), "surface"))
            .map_err(|error| normalized_js_error(&ctx, error))?;
        ctx.globals()
            .set(registration.root.as_str(), guarded_root)
            .map_err(js_error)
    }

    fn runtime_error(error: rquickjs::Error) -> HostError {
        HostError::Runtime(error.to_string())
    }

    fn js_error(error: rquickjs::Error) -> HostError {
        HostError::JavaScript(error.to_string())
    }

    fn classified_js_error(message: String) -> HostError {
        if message.starts_with("method_not_found:") {
            HostError::MethodNotFound(message)
        } else if message.starts_with("surface_not_found:") {
            HostError::SurfaceNotFound(message)
        } else {
            HostError::JavaScript(message)
        }
    }

    fn normalized_js_error(ctx: &Ctx<'_>, error: rquickjs::Error) -> HostError {
        if matches!(error, rquickjs::Error::Exception) {
            let caught = ctx.catch();
            if let Some(object) = caught.as_object() {
                if let Ok(message) = object.get::<_, String>("message") {
                    return classified_js_error(message);
                }
            }
            if let Some(string) = caught.as_string() {
                if let Ok(message) = string.to_string() {
                    return classified_js_error(message);
                }
            }
        }
        js_error(error)
    }
}
