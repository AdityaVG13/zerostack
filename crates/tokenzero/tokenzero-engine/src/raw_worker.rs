//! Private raw worker framed protocol (tokenzero-irx9.4).
//!
//! Trusted local composition path: invokes the typed domain dispatcher once
//! per frame. Does **not** open FastMCP catalogs, parse JavaScript, plan,
//! compact again, or rewrite envelopes. It is the planner-free backend for
//! ZeroStack composition, not a user-facing host.
//!
//! Production entry: [`run_raw_worker_serve`] / [`run_raw_worker_once`] for
//! canonical `tokenzero-codemode` worker binary.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use crate::TokenZeroEngine;
use crate::config::EngineConfig;
use crate::dispatcher::{DispatchOutcome, dispatch_raw_worker};
use crate::surface_handshake::{
    CompressionOwner, HandshakeSurface, PlannerOwner, RAW_WORKER_PROTOCOL_VERSION,
    SurfaceCapability, build_surface_capability, check_contract_compatibility, composition_trace,
    surface_capability_json,
};

/// Framed request: one domain op, optional peer contract for fail-closed handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawWorkerRequest {
    /// Protocol marker — must be [`RAW_WORKER_PROTOCOL_VERSION`] or omitted (default).
    #[serde(default)]
    pub protocol: Option<String>,
    /// Canonical op name or alias (`tz_read`, `zero.read`, `read`, …).
    #[serde(default)]
    pub op: String,
    /// Domain args object.
    #[serde(default)]
    pub args: Value,
    /// Optional peer semantic contract digest (handshake).
    #[serde(default)]
    pub peer_contract_digest: Option<String>,
    /// Optional peer semantic contract version.
    #[serde(default)]
    pub peer_contract_version: Option<String>,
    /// Control verbs: `handshake`, `ping`, `shutdown` (OMP control plane).
    #[serde(default)]
    pub control: Option<String>,
}

/// Framed response with composition ownership trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawWorkerResponse {
    pub ok: bool,
    pub protocol: String,
    pub op: String,
    pub surface: String,
    /// Normalized domain / tool outcome when dispatch succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RawWorkerError>,
    /// Composition ownership + boundary accounting (AC: planner/compression owners).
    pub trace: Value,
    /// Catalog-free capability snapshot used for this call.
    pub capability: SurfaceCapability,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawWorkerError {
    pub kind: String,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
}

/// Execute one framed raw-worker request through the shared dispatcher.
///
/// Guarantees:
/// - Exactly one domain dispatch boundary (`boundary_count=1` on success/tool-error
///   after handshake; handshake failures have `boundary_count=0`).
/// - Ordinary `ToolResponse` failures (missing path, policy, deadline, cancel, …)
///   return `ok=false`, `result=null`, and typed `error` with `retryable` preserved.
/// - No CodeMode sandbox / JS runtime is created.
/// - Peer digest/version mismatches fail before domain execution.
pub fn execute_raw_worker_frame(
    engine: &TokenZeroEngine,
    request: &RawWorkerRequest,
) -> RawWorkerResponse {
    let capability = build_surface_capability(HandshakeSurface::RawWorker);
    let protocol = request
        .protocol
        .as_deref()
        .unwrap_or(RAW_WORKER_PROTOCOL_VERSION)
        .to_string();

    if protocol != RAW_WORKER_PROTOCOL_VERSION {
        return fail_response(
            &request.op,
            capability,
            "protocol_mismatch",
            format!(
                "raw worker protocol mismatch: local={RAW_WORKER_PROTOCOL_VERSION} peer={protocol}"
            ),
            false,
            0,
        );
    }

    // Control plane: handshake / ping / shutdown (no domain dispatch).
    if let Some(control) = request.control.as_deref() {
        return handle_control(control, capability);
    }

    if request.op.is_empty() {
        return fail_response(
            "",
            capability,
            "validation",
            "raw worker frame requires non-empty op (or control=handshake|ping|shutdown)".into(),
            false,
            0,
        );
    }

    if let Err(msg) = check_contract_compatibility(
        &capability,
        request.peer_contract_digest.as_deref(),
        request.peer_contract_version.as_deref(),
    ) {
        return fail_response(&request.op, capability, "contract_mismatch", msg, false, 0);
    }

    let args = if request.args.is_null() {
        json!({})
    } else {
        request.args.clone()
    };

    let outcome = dispatch_raw_worker(engine, &request.op, &args);
    response_from_outcome(&request.op, capability, outcome)
}

/// Build fail/success envelope from a dispatcher outcome.
///
/// Tool-level errors (`status != "ok"`) take the same fail envelope as
/// `domain_error` paths: `ok=false`, `result=null`, typed error + retryable.
pub fn response_from_outcome(
    op: &str,
    capability: SurfaceCapability,
    outcome: DispatchOutcome,
) -> RawWorkerResponse {
    if let Some(err) = outcome.tool_domain_error() {
        return fail_response(
            op,
            capability,
            err.kind.as_str(),
            err.message,
            err.retryable,
            1,
        );
    }
    if let Some(err) = &outcome.domain_error {
        return fail_response(
            op,
            capability,
            err.kind.as_str(),
            err.message.clone(),
            err.retryable,
            1,
        );
    }
    // Explicit ToolResponse status check (belt-and-suspenders).
    if let Some(resp) = &outcome.tool_response
        && resp.status != "ok"
    {
        let code = resp
            .error
            .as_ref()
            .map(|e| e.code.as_str())
            .unwrap_or("runtime");
        let message = resp
            .error
            .as_ref()
            .map(|e| e.message.clone())
            .unwrap_or_else(|| format!("{} failed with status {}", op, resp.status));
        let domain = outcome.tool_domain_error();
        let retryable = domain.as_ref().map(|d| d.retryable).unwrap_or(false);
        let kind = domain
            .as_ref()
            .map(|d| d.kind.as_str().to_string())
            .unwrap_or_else(|| code.to_string());
        return fail_response(op, capability, &kind, message, retryable, 1);
    }
    success_response(op, capability, outcome)
}

fn handle_control(control: &str, capability: SurfaceCapability) -> RawWorkerResponse {
    match control {
        "handshake" | "hello" | "capabilities" => RawWorkerResponse {
            ok: true,
            protocol: RAW_WORKER_PROTOCOL_VERSION.into(),
            op: "control.handshake".into(),
            surface: HandshakeSurface::RawWorker.as_str().into(),
            result: Some(surface_capability_json(HandshakeSurface::RawWorker)),
            error: None,
            trace: composition_trace(
                HandshakeSurface::RawWorker,
                PlannerOwner::Client,
                CompressionOwner::Engine,
                0,
            ),
            capability,
        },
        "ping" => RawWorkerResponse {
            ok: true,
            protocol: RAW_WORKER_PROTOCOL_VERSION.into(),
            op: "control.ping".into(),
            surface: HandshakeSurface::RawWorker.as_str().into(),
            result: Some(json!({"pong": true})),
            error: None,
            trace: composition_trace(
                HandshakeSurface::RawWorker,
                PlannerOwner::Client,
                CompressionOwner::Engine,
                0,
            ),
            capability,
        },
        "shutdown" => RawWorkerResponse {
            ok: true,
            protocol: RAW_WORKER_PROTOCOL_VERSION.into(),
            op: "control.shutdown".into(),
            surface: HandshakeSurface::RawWorker.as_str().into(),
            result: Some(json!({"shutdown": true})),
            error: None,
            trace: composition_trace(
                HandshakeSurface::RawWorker,
                PlannerOwner::Client,
                CompressionOwner::Engine,
                0,
            ),
            capability,
        },
        other => fail_response(
            "control",
            capability,
            "validation",
            format!("unknown control verb {other:?}; expected handshake|ping|shutdown"),
            false,
            0,
        ),
    }
}

/// JSON convenience entry: parse request object, return response JSON.
pub fn execute_raw_worker_json(engine: &TokenZeroEngine, request: &Value) -> Value {
    let parsed: Result<RawWorkerRequest, _> = serde_json::from_value(request.clone());
    match parsed {
        Ok(req) => serde_json::to_value(execute_raw_worker_frame(engine, &req))
            .expect("RawWorkerResponse serializes"),
        Err(e) => {
            let capability = build_surface_capability(HandshakeSurface::RawWorker);
            serde_json::to_value(fail_response(
                "",
                capability,
                "invalid_frame",
                format!("invalid raw worker request: {e}"),
                false,
                0,
            ))
            .expect("serialize")
        }
    }
}

fn success_response(
    op: &str,
    capability: SurfaceCapability,
    outcome: DispatchOutcome,
) -> RawWorkerResponse {
    let mut result = outcome.result.value.clone();
    if let Value::Object(ref mut map) = result {
        map.insert("refs".into(), json!(outcome.result.refs));
        map.insert("op".into(), json!(outcome.op));
    }
    if let Some(resp) = &outcome.tool_response
        && let Ok(v) = serde_json::to_value(resp)
        && let Value::Object(ref mut map) = result
    {
        map.insert("tool_response".into(), v);
    }
    RawWorkerResponse {
        ok: true,
        protocol: RAW_WORKER_PROTOCOL_VERSION.into(),
        op: op.into(),
        surface: HandshakeSurface::RawWorker.as_str().into(),
        result: Some(result),
        error: None,
        trace: composition_trace(
            HandshakeSurface::RawWorker,
            PlannerOwner::Client,
            CompressionOwner::Engine,
            1,
        ),
        capability,
    }
}

fn fail_response(
    op: &str,
    capability: SurfaceCapability,
    kind: &str,
    message: String,
    retryable: bool,
    boundary_count: u32,
) -> RawWorkerResponse {
    RawWorkerResponse {
        ok: false,
        protocol: RAW_WORKER_PROTOCOL_VERSION.into(),
        op: op.into(),
        surface: HandshakeSurface::RawWorker.as_str().into(),
        result: None,
        error: Some(RawWorkerError {
            kind: kind.into(),
            message,
            retryable,
        }),
        trace: composition_trace(
            HandshakeSurface::RawWorker,
            PlannerOwner::Client,
            CompressionOwner::Engine,
            boundary_count,
        ),
        capability,
    }
}

// ---------------------------------------------------------------------------
// Production process entry (OMP / router consumable)
// ---------------------------------------------------------------------------

/// Options for the shipped raw-worker process entry.
#[derive(Debug, Clone)]
pub struct RawWorkerServeOptions {
    pub root: PathBuf,
    pub cache_path: Option<PathBuf>,
    /// When true, print handshake capability JSON then exit 0 (no serve loop).
    pub handshake_only: bool,
    /// Single JSON request string; execute once and exit (no serve loop).
    pub once_json: Option<String>,
}

impl Default for RawWorkerServeOptions {
    fn default() -> Self {
        Self {
            root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            cache_path: None,
            handshake_only: false,
            once_json: None,
        }
    }
}

fn engine_from_options(opts: &RawWorkerServeOptions) -> TokenZeroEngine {
    let mut cfg = EngineConfig::for_root(&opts.root);
    if let Some(cache) = &opts.cache_path {
        cfg.cache_path = cache.clone();
    }
    cfg.session_dedup = false;
    TokenZeroEngine::new(cfg)
}

/// One-shot: print capability handshake JSON and exit.
pub fn raw_worker_print_handshake() -> i32 {
    let cap = surface_capability_json(HandshakeSurface::RawWorker);
    println!("{}", serde_json::to_string(&cap).expect("serialize cap"));
    0
}

/// One-shot framed op from a JSON request string.
pub fn run_raw_worker_once(opts: &RawWorkerServeOptions, request_json: &str) -> i32 {
    let engine = engine_from_options(opts);
    let value: Value = match serde_json::from_str(request_json) {
        Ok(v) => v,
        Err(e) => {
            let capability = build_surface_capability(HandshakeSurface::RawWorker);
            let resp = fail_response(
                "",
                capability,
                "invalid_frame",
                format!("invalid raw worker request JSON: {e}"),
                false,
                0,
            );
            println!("{}", serde_json::to_string(&resp).expect("serialize"));
            return 2;
        }
    };
    let resp = execute_raw_worker_json(&engine, &value);
    println!("{}", serde_json::to_string(&resp).expect("serialize"));
    if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        0
    } else {
        1
    }
}

/// NDJSON serve loop: one request object per stdin line → one response line.
///
/// Restart behavior: if the process is respawned by the hub, a fresh handshake
/// (`{"control":"handshake"}`) re-advertises capability; no in-memory planner
/// state is retained across process restarts (stateless frames).
///
/// Control `shutdown` ends the loop with exit 0.
pub fn run_raw_worker_serve(opts: &RawWorkerServeOptions) -> i32 {
    if std::env::var("ZEROSTACK_RAW_WORKER_PROTOCOL")
        .is_ok_and(|value| value == raw_worker_protocol::RAW_WORKER_PROTOCOL_VERSION)
    {
        return run_raw_worker_protocol_serve(opts);
    }
    if opts.handshake_only {
        return raw_worker_print_handshake();
    }
    if let Some(once) = &opts.once_json {
        return run_raw_worker_once(opts, once);
    }

    let engine = engine_from_options(opts);
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let lines = stdin.lock().lines();

    // Emit ready banner for OMP/router (single line JSON).
    let ready = json!({
        "ok": true,
        "protocol": RAW_WORKER_PROTOCOL_VERSION,
        "surface": "raw_worker",
        "event": "ready",
        "capability": surface_capability_json(HandshakeSurface::RawWorker),
    });
    if writeln!(stdout, "{}", ready).is_err() {
        return 2;
    }
    let _ = stdout.flush();

    for line in lines {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                let capability = build_surface_capability(HandshakeSurface::RawWorker);
                let resp = fail_response(
                    "",
                    capability,
                    "io",
                    format!("stdin read error: {e}"),
                    false,
                    0,
                );
                let _ = writeln!(
                    stdout,
                    "{}",
                    serde_json::to_string(&resp).unwrap_or_default()
                );
                let _ = stdout.flush();
                return 2;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                let capability = build_surface_capability(HandshakeSurface::RawWorker);
                let resp = fail_response(
                    "",
                    capability,
                    "invalid_frame",
                    format!("invalid raw worker request JSON: {e}"),
                    false,
                    0,
                );
                if writeln!(
                    stdout,
                    "{}",
                    serde_json::to_string(&resp).unwrap_or_default()
                )
                .is_err()
                {
                    return 2;
                }
                let _ = stdout.flush();
                continue;
            }
        };
        let resp = execute_raw_worker_json(&engine, &value);
        if writeln!(
            stdout,
            "{}",
            serde_json::to_string(&resp).unwrap_or_default()
        )
        .is_err()
        {
            return 2;
        }
        let _ = stdout.flush();
        if value
            .get("control")
            .and_then(|c| c.as_str())
            .is_some_and(|c| c == "shutdown")
        {
            return 0;
        }
    }
    // EOF: clean exit so hub can restart (restart = new process + new ready banner).
    0
}

/// Parse argv for production raw-worker entry on surface binaries.
///
/// Recognized:
/// - `raw-worker` / `raw_worker` → serve loop
/// - `raw-worker --handshake` → print capability and exit
/// - `raw-worker --once '{...}'` → single frame
/// - `raw-worker --root DIR --cache-path PATH`
pub fn parse_raw_worker_argv(args: &[String]) -> Result<Option<RawWorkerServeOptions>, String> {
    if !args
        .get(1)
        .is_some_and(|arg| arg == "raw-worker" || arg == "raw_worker")
    {
        return Ok(None);
    }

    fn value<'a>(option: &str, value: Option<&'a String>) -> Result<&'a str, String> {
        let value = value
            .map(String::as_str)
            .filter(|value| !value.is_empty() && !value.starts_with("--"))
            .ok_or_else(|| format!("{option} requires a value and it must be non-empty"))?;
        Ok(value)
    }

    let rest = &args[2..];
    let mut opts = RawWorkerServeOptions::default();
    let mut seen_handshake = false;
    let mut seen_once = false;
    let mut seen_root = false;
    let mut seen_cache_path = false;
    let mut i = 0;
    while i < rest.len() {
        let argument = rest[i].as_str();
        match argument {
            "--handshake" | "handshake" => {
                if seen_handshake {
                    return Err("duplicate raw-worker handshake mode".into());
                }
                seen_handshake = true;
                opts.handshake_only = true;
            }
            "--once" => {
                if seen_once {
                    return Err("duplicate raw-worker --once option".into());
                }
                seen_once = true;
                opts.once_json = Some(value("--once", rest.get(i + 1))?.to_string());
                i += 1;
            }
            "--root" => {
                if seen_root {
                    return Err("duplicate raw-worker --root option".into());
                }
                seen_root = true;
                opts.root = PathBuf::from(value("--root", rest.get(i + 1))?);
                i += 1;
            }
            "--cache-path" => {
                if seen_cache_path {
                    return Err("duplicate raw-worker --cache-path option".into());
                }
                seen_cache_path = true;
                opts.cache_path = Some(PathBuf::from(value("--cache-path", rest.get(i + 1))?));
                i += 1;
            }
            argument if argument.starts_with("--once=") => {
                if seen_once {
                    return Err("duplicate raw-worker --once option".into());
                }
                seen_once = true;
                opts.once_json = Some(
                    argument
                        .strip_prefix("--once=")
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| "--once requires a non-empty value".to_string())?
                        .to_string(),
                );
            }
            argument if argument.starts_with("--root=") => {
                if seen_root {
                    return Err("duplicate raw-worker --root option".into());
                }
                seen_root = true;
                opts.root = PathBuf::from(
                    argument
                        .strip_prefix("--root=")
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| "--root requires a non-empty value".to_string())?,
                );
            }
            argument if argument.starts_with("--cache-path=") => {
                if seen_cache_path {
                    return Err("duplicate raw-worker --cache-path option".into());
                }
                seen_cache_path = true;
                opts.cache_path = Some(PathBuf::from(
                    argument
                        .strip_prefix("--cache-path=")
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| "--cache-path requires a non-empty value".to_string())?,
                ));
            }
            _ => return Err(format!("unknown raw-worker argument: {argument}")),
        }
        i += 1;
    }
    if seen_handshake && seen_once {
        return Err("raw-worker --handshake and --once are incompatible".into());
    }
    Ok(Some(opts))
}

/// Entry for surface binaries: if argv contains raw-worker, run and never return.
pub fn maybe_run_raw_worker_from_args(args: &[String]) -> Result<Option<i32>, String> {
    let Some(opts) = parse_raw_worker_argv(args)? else {
        return Ok(None);
    };
    Ok(Some(run_raw_worker_serve(&opts)))
}

#[path = "raw_worker_impl.rs"]
mod raw_worker_impl;
#[path = "raw_worker_protocol.rs"]
pub mod raw_worker_protocol;
pub use raw_worker_impl::{
    RawWorkerSession, execute_raw_worker_frame as execute_raw_worker_protocol_frame,
    run_raw_worker_protocol_serve,
};
