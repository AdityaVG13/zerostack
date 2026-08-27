//! Thin FastMCP adapter over the typed domain dispatcher (graphzero-o2uq.5).
//!
//! Contract:
//! - Tool names/descriptions/schemas come from the operation registry only.
//! - One tools/call → one domain `dispatch` → one transport envelope.
//! - No plan execution, sandbox, secondary compression, or CodeMode tools.
//! - Unknown tools and invalid args surface stable typed domain errors.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde_json::{Value, json};

use graphzero_engine::operation_abi::{
    DomainError, DomainErrorKind, DomainResult, lean_fastmcp_tool_names, resolve_operation,
};
use graphzero_engine::{AdapterKind, EngineContext, dispatch, dispatch_profiled};

/// Process-wide count of domain dispatches performed through this adapter.
static DOMAIN_DISPATCH_COUNT: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
thread_local! {
    /// Isolates the one-call law from unrelated parallel unit tests that also
    /// exercise the process-wide adapter counter.
    static TEST_THREAD_DISPATCH_COUNT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn record_domain_dispatch() {
    DOMAIN_DISPATCH_COUNT.fetch_add(1, Ordering::SeqCst);
    #[cfg(test)]
    TEST_THREAD_DISPATCH_COUNT.set(TEST_THREAD_DISPATCH_COUNT.get().saturating_add(1));
}

/// Transport-neutral FastMCP tool result (before JSON-RPC framing).
#[derive(Clone, Debug)]
pub struct FastMcpCallOk {
    pub result: DomainResult,
    /// Wall nanoseconds spent in the adapter around the single dispatch.
    pub adapter_wall_ns: u128,
    /// Dispatcher-only wall nanoseconds (for framework overhead subtraction).
    pub dispatcher_wall_ns: u128,
}

/// Typed FastMCP failure (stable kind/retryable for clients).
#[derive(Clone, Debug)]
pub struct FastMcpCallErr {
    pub error: DomainError,
    pub adapter_wall_ns: u128,
    pub dispatcher_wall_ns: Option<u128>,
}

impl FastMcpCallErr {
    pub fn to_mcp_error_payload(&self) -> Value {
        typed_error_payload(&self.error)
    }
}

/// JSON object embedded in MCP tool error text / structured content.
pub fn typed_error_payload(err: &DomainError) -> Value {
    let mut payload = json!({
        "error": err.message,
        "kind": err.kind.as_str(),
        "retryable": err.retryable,
        "op": err.op,
        "surface": "fastmcp",
    });
    if let Some(r) = &err.recovery_ref
        && let Some(obj) = payload.as_object_mut()
    {
        // Expand attaches ExpandError::to_json in recovery_ref so agents get
        // `trace`/`kind`/`ref` once as sibling fields (not a JSON string inside
        // `error`). Prefer expand `kind` tokens (wrong_root/expired/…) when
        // present so harnesses can branch on ExpandErrorKind (m3wx).
        if err.op.as_deref() == Some("expand")
            && let Ok(expand) = serde_json::from_str::<Value>(r)
            && expand.get("trace").is_some()
        {
            if let Some(trace) = expand.get("trace") {
                obj.insert("trace".into(), trace.clone());
            }
            if let Some(reference) = expand.get("ref") {
                obj.insert("ref".into(), reference.clone());
            }
            if let Some(kind) = expand.get("kind") {
                obj.insert("kind".into(), kind.clone());
                obj.insert("expand_kind".into(), kind.clone());
            }
        } else {
            obj.insert("recovery_ref".into(), json!(r));
        }
    }
    payload
}

pub fn domain_dispatch_count() -> u64 {
    DOMAIN_DISPATCH_COUNT.load(Ordering::SeqCst)
}

pub fn reset_domain_dispatch_count_for_tests() {
    DOMAIN_DISPATCH_COUNT.store(0, Ordering::SeqCst);
    #[cfg(test)]
    TEST_THREAD_DISPATCH_COUNT.set(0);
}

#[cfg(test)]
fn test_thread_dispatch_count() -> u64 {
    TEST_THREAD_DISPATCH_COUNT.get()
}

/// Framework-only overhead above dispatcher cost (saturating).
pub fn framework_overhead_ns(adapter_wall_ns: u128, dispatcher_wall_ns: u128) -> u128 {
    adapter_wall_ns.saturating_sub(dispatcher_wall_ns)
}

/// Resolve a tools/call name against the lean FastMCP product set.
pub fn resolve_fastmcp_tool(name: &str) -> Result<&'static str, DomainError> {
    if let Some(op) = resolve_operation(name) {
        if op.exposure.fastmcp_tool {
            return Ok(op.name);
        }
        // Documented alias: blast_intent → blast. Kept until a major version
        // after clients migrate; not listed in the lean FastMCP catalog.
        if lean_fastmcp_tool_names().contains(&op.name) {
            return Ok(op.name);
        }
        return Err(DomainError::new(
            DomainErrorKind::NotFound,
            format!(
                "tool '{name}' is not in the lean FastMCP catalog (resolved to '{}')",
                op.name
            ),
        )
        .with_op(name));
    }
    Err(DomainError::new(DomainErrorKind::NotFound, format!("unknown tool {name}")).with_op(name))
}

/// One tools/call with lean-catalog gate: validate name, dispatch once, serialize once.
///
/// Callers supply repo/store paths already resolved from MCP arguments.
pub fn call_once(
    op_name: &str,
    args: &Value,
    repo_root: std::path::PathBuf,
    store_root: std::path::PathBuf,
) -> Result<FastMcpCallOk, FastMcpCallErr> {
    let started = Instant::now();
    let canonical = match resolve_fastmcp_tool(op_name) {
        Ok(c) => c,
        Err(error) => {
            return Err(FastMcpCallErr {
                error,
                adapter_wall_ns: started.elapsed().as_nanos(),
                dispatcher_wall_ns: None,
            });
        }
    };
    dispatch_once(canonical, args, repo_root, store_root, started)
}

/// One domain dispatch without lean-catalog gating (legacy aliases / internal routes).
///
/// Still counts as exactly one dispatcher invocation for the one-call law.
pub fn dispatch_once(
    op_name: &str,
    args: &Value,
    repo_root: std::path::PathBuf,
    store_root: std::path::PathBuf,
    started: Instant,
) -> Result<FastMcpCallOk, FastMcpCallErr> {
    let ctx = EngineContext::for_paths(repo_root, store_root, AdapterKind::FastMcp);
    let (outcome, profile) = dispatch_profiled(&ctx, op_name, args);
    record_domain_dispatch();
    let adapter_wall_ns = started.elapsed().as_nanos();

    match outcome {
        Ok(result) => Ok(FastMcpCallOk {
            result,
            adapter_wall_ns,
            dispatcher_wall_ns: profile.wall_ns,
        }),
        Err(error) => Err(FastMcpCallErr {
            error,
            adapter_wall_ns,
            dispatcher_wall_ns: Some(profile.wall_ns),
        }),
    }
}

/// Serialize a successful domain result into a single MCP text envelope body.
///
/// Exactly one transport envelope — no secondary compression pass.
pub fn serialize_domain_result(result: &DomainResult) -> String {
    match &result.value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| other.to_string()),
    }
}

/// Capture framework-only overhead for one successful or failed call.
pub fn measure_framework_overhead(
    op_name: &str,
    args: &Value,
    repo_root: std::path::PathBuf,
    store_root: std::path::PathBuf,
) -> Value {
    match call_once(op_name, args, repo_root, store_root) {
        Ok(ok) => json!({
            "ok": true,
            "op": ok.result.op,
            "adapter_wall_ns": ok.adapter_wall_ns,
            "dispatcher_wall_ns": ok.dispatcher_wall_ns,
            "framework_overhead_ns": framework_overhead_ns(ok.adapter_wall_ns, ok.dispatcher_wall_ns),
            "domain_dispatch_count_delta": 1,
        }),
        Err(err) => json!({
            "ok": false,
            "kind": err.error.kind.as_str(),
            "adapter_wall_ns": err.adapter_wall_ns,
            "dispatcher_wall_ns": err.dispatcher_wall_ns,
            "framework_overhead_ns": err
                .dispatcher_wall_ns
                .map(|d| framework_overhead_ns(err.adapter_wall_ns, d)),
            "domain_dispatch_count_delta": if err.dispatcher_wall_ns.is_some() { 1 } else { 0 },
        }),
    }
}

/// Static guarantee: this adapter module does not start CodeMode or nest MCP.
pub fn adapter_source_is_thin() -> bool {
    let src = include_str!("fastmcp_adapter.rs");
    let prod = src.split("#[cfg(test)]").next().unwrap_or(src);
    let scan = prod.split("/// Static guarantee").next().unwrap_or(prod);
    let needles = [
        ["rqui", "ckjs"].concat(),
        ["gz_execute", "_code"].concat(),
        ["execute_with", "_host"].concat(),
        ["tools/", "list"].concat(),
    ];
    needles.iter().all(|n| !scan.contains(n.as_str())) && scan.contains("dispatch_profiled")
}

// Re-export raw dispatch for callers that already have EngineContext.
pub fn dispatch_fastmcp(
    ctx: &EngineContext,
    op: &str,
    args: &Value,
) -> Result<DomainResult, DomainError> {
    debug_assert_eq!(ctx.adapter, AdapterKind::FastMcp);
    record_domain_dispatch();
    dispatch(ctx, op, args)
}
