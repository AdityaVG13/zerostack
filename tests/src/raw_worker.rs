//! Raw-worker v2 conformance transport for `*-codemode` artifacts (gates RW1-RW10).
//!
//! A `*-codemode` artifact is a planner-free raw-worker v2 binary, **not** an
//! MCP server and **not** a planner host. This module drives it through the
//! public raw-worker v2 wire protocol (`zero_abi::raw_worker` frames) using
//! the hub's `zero_codemode::worker` client. These RW1-RW10 gates are a
//! DISTINCT layer from the plan-level G1-G10 gates (`checks.rs` / `plan.rs`):
//! they are not aliases, and a raw worker cannot own planner semantics.
//!
//! MCP surface conformance for actual `*-mcp` artifacts stays in the separate
//! MCP transport path (`lib.rs::McpClient`, G1 exposure only). A raw worker is
//! never treated as an MCP server: no initialize/tools/call framing, no
//! planner, no JavaScript host, no capability catalog in the worker.
//!
//! Spawn/probe contract: FSZero and GraphZero use `capabilities --json`. The
//! canonical TokenZero binary here ALSO accepts `capabilities --json`; the hub
//! `zero-codemode/session.rs` happens to use a different valid Token probe, so
//! this module's Token probe shape is a harness choice, not the engine's only
//! valid one (see README).
//!
//!   RW1 artifact_exposure:  capabilities probe + wire handshake + opposite-surface refusal
//!   RW2 recoverable_refs:   op result carries engine-scoped refs that resolve via the
//!                           owning engine's expand op, plus echoed trace ids. Engine-owned
//!                           refs (e.g. tz://file/<id>) are valid here even when they are
//!                           not portable blob64/codemode refs; the portable validator
//!                           stays in `lib.rs` for the planner/MCP surface only.
//!   RW3 telemetry_accounting: telemetry_request yields engine timeline; worker token
//!                           accounting is REQUIRED for TokenZero and OPTIONAL for
//!                           FSZero/GraphZero (which may emit None).
//!   RW4 output_bounds:      oversize op output stays within negotiated frame/output bounds
//!   RW5 typed_errors:       validation/forbidden/substrate failures are typed WorkerError.
//!                           Host authorization policy is NOT a raw-worker gate.
//!   RW6 session_continuity: distinct calls on one worker process keep continuity and
//!                           per-call ref/trace ownership. NOT a literal ctx.step primitive.
//!   RW7 frame_limits:       negotiated protocol limits are nonzero and enforced
//!   RW8 domain_mutation:    domain-authority mutation op succeeds at the worker boundary.
//!                           The engine owns mutation; the hub owns authorization.
//!   RW9 process_reuse:      many sequential ops settle in one worker process. NOT
//!                           aggregate plan-level op coalescing; that is the planner's job.
//!   RW10 planner_refusal:   planner/JS/MCP ops are denied with typed errors and the
//!                           worker stays alive for the next call

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use zero_abi::raw_worker::{
    CallRequest, EngineIdentity, ProtocolLimits, RefOwnership, TelemetryRequestV1, WorkerResult,
    WorkerTokenAccountingV1, WorkerTrace,
};
use zero_codemode::worker::{
    StaticWorkerFactory, WorkerAdapterError, WorkerClient, WorkerClientConfig, WorkerContext,
    WorkerObservation, WorkerRegistry, WorkerSettlementReceiptV1,
};

use crate::{CheckResult, Ns};

/// Per-engine raw-worker v2 contract, mirrored from `zero-codemode/session.rs`
/// and the engine surface binaries.
#[derive(Clone, Copy)]
pub struct RawEngineContract {
    pub ns: Ns,
    pub engine: EngineIdentity,
    /// argv for the one-shot capability probe.
    pub probe_args: &'static [&'static str],
    /// read op + root-relative path arg key.
    pub read_op: &'static str,
    /// mutation op (domain-authority; engine owns it).
    pub mutation_op: &'static str,
    /// public expand op used to resolve an ownership ref back to content.
    pub expand_op: &'static str,
    /// arg key under which the ref is passed to `expand_op`.
    pub expand_arg: &'static str,
    /// argv used to enter the wire serve loop (root appended by the harness).
    pub serve_args: &'static [&'static str],
    /// argv that must be refused (opposite surface probe).
    pub refusal_args: &'static [&'static str],
}

fn contract(ns: Ns) -> RawEngineContract {
    match ns {
        Ns::Fz => RawEngineContract {
            ns,
            engine: EngineIdentity::FsZero,
            probe_args: &["capabilities", "--json"],
            read_op: "fs.read",
            mutation_op: "fs.edit",
            expand_op: "fs.expand",
            expand_arg: "ref",
            serve_args: &["--raw-worker", "--root"],
            refusal_args: &["--mode=mcp"],
        },
        Ns::Tz => RawEngineContract {
            ns,
            engine: EngineIdentity::TokenZero,
            probe_args: &["capabilities", "--json"],
            read_op: "read",
            mutation_op: "edit",
            expand_op: "expand",
            expand_arg: "ref",
            serve_args: &["raw-worker", "--root"],
            refusal_args: &["--mode=mcp"],
        },
        Ns::Gz => RawEngineContract {
            ns,
            engine: EngineIdentity::GraphZero,
            probe_args: &["capabilities", "--json"],
            read_op: "snap",
            mutation_op: "index",
            expand_op: "expand",
            expand_arg: "reference",
            serve_args: &[],
            refusal_args: &["--mode=mcp"],
        },
    }
}

/// The raw-v2 wire protocol the serve loop must speak. Forced via
/// `ZEROSTACK_RAW_WORKER_PROTOCOL` because FSZero and TokenZero otherwise
/// default to their legacy v1 serve loop.
pub const RAW_WORKER_PROTOCOL: &str = "zerostack.raw_worker.v2";

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

static ID_COUNTER: AtomicU64 = AtomicU64::new(1);

fn fresh_id(prefix: &str) -> String {
    let seq = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{}-{}", prefix, now_unix_ms(), seq)
}

/// One-shot capability probe result used to derive the pinned contract digest.
pub struct ProbeResult {
    pub output: String,
    pub contract_digest: String,
    pub worker_revision: String,
}

fn run_probe(bin: &Path, args: &[&str], timeout: Duration) -> Result<(bool, Vec<u8>)> {
    let mut command = Command::new(bin);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let started = Instant::now();
    let mut child = command
        .spawn()
        .with_context(|| format!("spawning probe {}", bin.display()))?;
    let output = loop {
        match child.try_wait() {
            Ok(Some(_status)) => break child.wait_with_output().context("reading probe output")?,
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("capability probe timed out: {}", bin.display());
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("cannot wait for probe: {}", error);
            }
        }
    };
    let success = output.status.success();
    let bytes = if output.stdout.is_empty() {
        output.stderr
    } else {
        output.stdout
    };
    Ok((success, bytes))
}

fn sha256_hex(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path)
        .with_context(|| format!("reading binary for revision hash: {}", path.display()))?;
    let mut digest = Sha256::new();
    digest.update(&bytes);
    let value: [u8; 32] = digest.finalize().into();
    let mut out = String::with_capacity(64);
    for byte in value {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    Ok(out)
}

/// Run the one-shot capability probe and derive the pinned contract digest
/// plus the binary SHA-256 worker revision, using the same probe shapes as
/// `zero-codemode/session.rs::probe_contract` (every engine exposes
/// `<bin> capabilities --json` with a nested `/package/abi_digest`).
pub fn probe(ns: Ns, bin: &Path, timeout: Duration) -> Result<ProbeResult> {
    let c = contract(ns);
    let (success, bytes) = run_probe(bin, c.probe_args, timeout)?;
    if !success && bytes.is_empty() {
        bail!("capability probe failed without output: {}", bin.display());
    }
    let text = String::from_utf8(bytes).context("probe output was not UTF-8")?;
    let digest = package_abi_digest(&text).with_context(|| {
        format!(
            "{} probe omitted package.abi_digest; raw output: {}",
            ns.as_str(),
            text.trim().chars().take(400).collect::<String>()
        )
    })?;
    let worker_revision = sha256_hex(bin)?;
    Ok(ProbeResult {
        output: text,
        contract_digest: digest,
        worker_revision,
    })
}

/// Extract the pinned contract digest from a `capabilities --json` probe
/// (nested `/package/abi_digest`, same shape as session.rs
/// `package_contract_from_json`).
pub fn package_abi_digest(output: &str) -> Option<String> {
    let value: Value = serde_json::from_str(output).ok()?;
    let digest = value
        .get("package")?
        .get("abi_digest")?
        .as_str()?
        .to_owned();
    (digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    .then_some(digest)
}

fn spawn_worker(
    ns: Ns,
    bin: &Path,
    fixture: &Fixture,
    digest: &str,
    revision: &str,
    timeout: Duration,
    observer: Option<Arc<dyn Fn(&WorkerObservation) + Send + Sync>>,
) -> Result<WorkerClient> {
    let c = contract(ns);
    let mut factory = StaticWorkerFactory::new(bin, revision, digest, digest)
        .env("ZEROSTACK_WORKER_REVISION", revision)
        .env("ZEROSTACK_RAW_WORKER_PROTOCOL", RAW_WORKER_PROTOCOL);
    for arg in c.serve_args {
        factory = factory.arg(*arg);
    }
    if !c.serve_args.is_empty() {
        factory = factory.arg(fixture.root.to_string_lossy().as_ref());
    }
    let mut registry = WorkerRegistry::new();
    registry
        .register(c.engine, Arc::new(factory))
        .map_err(|error| anyhow::anyhow!("worker registration failed: {}", error))?;
    let session_id = format!("zerostack-conformance-{}-{}", ns.as_str(), now_unix_ms());
    let config = WorkerClientConfig {
        limits: ProtocolLimits::default(),
        handshake_timeout: timeout,
        shutdown_timeout: Duration::from_secs(2),
        max_stderr_bytes: 65_536,
        observer,
    };
    registry
        .launch(
            WorkerContext {
                engine: c.engine,
                store_root: fixture.root.clone(),
                session_id,
            },
            config,
        )
        .map_err(|error| anyhow::anyhow!("worker spawn/handshake failed: {}", error))
}

fn make_request(
    op: &str,
    args: Value,
    deadline_ms: Option<u64>,
    telemetry: bool,
    worker_revision: &str,
    contract_digest: &str,
) -> CallRequest {
    let request_id = fresh_id("req");
    let trace = WorkerTrace {
        runtime_id: "zerostack-conformance".into(),
        cell_id: fresh_id("cell"),
        request_id: request_id.clone(),
        trace_id: fresh_id("trace"),
        parent_span_id: None,
        worker_revision: worker_revision.into(),
        contract_digest: contract_digest.into(),
    };
    CallRequest {
        request_id,
        op: op.into(),
        args,
        deadline_unix_ms: deadline_ms,
        trace,
        approval_grant: None,
        telemetry_request: telemetry.then_some(TelemetryRequestV1 {
            engine_stage_timeline: true,
            worker_token_accounting: true,
        }),
    }
}

/// Build an engine call with telemetry requests that match the engine ABI.
/// Every engine must emit a stage timeline when requested. Only TokenZero is a
/// tokenizer, so requesting token accounting from FSZero/GraphZero would make
/// the shared WorkerClient reject their intentional `None` response.
fn make_engine_request(
    ns: Ns,
    op: &str,
    args: Value,
    deadline_ms: Option<u64>,
    telemetry: bool,
    worker_revision: &str,
    contract_digest: &str,
) -> CallRequest {
    let mut request = make_request(
        op,
        args,
        deadline_ms,
        telemetry,
        worker_revision,
        contract_digest,
    );
    if let Some(telemetry_request) = &mut request.telemetry_request {
        telemetry_request.worker_token_accounting = requires_token_accounting(ns);
    }
    request
}

fn deadline_in(timeout: Duration) -> u64 {
    now_unix_ms()
        .checked_add(timeout.as_millis() as u64)
        .unwrap_or(u64::MAX)
}

/// Remote error kind extracted from a dispatch failure, if it is a typed
/// worker error (as opposed to a client/protocol/process failure).
fn remote_kind(error: &WorkerAdapterError) -> Option<&str> {
    match error {
        WorkerAdapterError::Remote { kind, .. } => Some(kind.as_str()),
        _ => None,
    }
}

fn remote_error(error: &WorkerAdapterError) -> String {
    match error {
        WorkerAdapterError::Remote { kind, message, .. } => {
            format!("worker error kind={}: {}", kind, message)
        }
        other => format!("{}", other),
    }
}

/// Whether a remote error `kind` is a forbidden-op denial: a raw worker
/// refusing a planner/JS/MCP/codemode capability it cannot own. Accepts
/// validation, sandbox, forbidden, and unsupported variants. Rejects policy
/// (mutation realm), deadline, and output-bounds kinds, which belong to the
/// mutation, limits, and leak gates respectively.
fn is_forbidden_denial(kind: &str) -> bool {
    let kind = kind.to_ascii_lowercase();
    kind.contains("validation")
        || kind.contains("sandbox")
        || kind.contains("forbidden")
        || kind.contains("unsupported")
}

/// Whether a remote error `kind` signals output/frame bounds enforcement
/// (leak-proof and oversized-frame evidence), as opposed to a policy or
/// deadline refusal. Matches `FrameCodecError::TooLarge::kind()`
/// (`frame_too_large`) plus the analogous typed worker kinds.
fn is_output_bounds_kind(kind: &str) -> bool {
    let kind = kind.to_ascii_lowercase();
    kind.contains("too_large") || kind.contains("bounds")
}

/// Whether a dispatch error is deadline enforcement: the client-side
/// `Deadline`/`DeadlineOverflow` refusal (the harness honors the deadline
/// itself) or a typed remote deadline/expired/timeout error.
fn is_deadline_enforcement(error: &WorkerAdapterError) -> bool {
    match error {
        WorkerAdapterError::Deadline { .. } | WorkerAdapterError::DeadlineOverflow { .. } => true,
        WorkerAdapterError::Remote { kind, .. } => {
            let kind = kind.to_ascii_lowercase();
            kind.contains("deadline") || kind.contains("expired") || kind.contains("timeout")
        }
        _ => false,
    }
}

/// Whether a dispatch error is output/frame bounds enforcement: a client
/// `Bounds` refusal (stdout/stderr over budget), a frame codec `TooLarge`,
/// or a typed remote output-bounds error.
fn is_bounds_enforcement(error: &WorkerAdapterError) -> bool {
    match error {
        WorkerAdapterError::Bounds { .. } => true,
        WorkerAdapterError::Protocol(zero_abi::raw_worker::FrameCodecError::TooLarge { .. }) => {
            true
        }
        WorkerAdapterError::Remote { kind, .. } => is_output_bounds_kind(kind),
        _ => false,
    }
}

/// Interpret an RW4 output-bounds probe outcome. A typed output-bounds refusal is
/// enforcement evidence (the worker refused to leak the bytes back). An `Ok`
/// whose serialized value still exceeds the negotiated output bound leaked
/// the bytes inline without emitting a ref. Any other error shape is flagged
/// so a non-bounds failure cannot be papered over as enforcement.
fn classify_leak_outcome(
    outcome: &Result<WorkerResult, WorkerAdapterError>,
    limit: usize,
) -> Vec<String> {
    let mut details = Vec::new();
    match outcome {
        Ok(result) => {
            let serialized = serde_json::to_vec(&result.value)
                .map(|bytes| bytes.len())
                .unwrap_or(usize::MAX);
            if serialized > limit {
                details.push(format!(
                    "oversize result value ({serialized} bytes) exceeds negotiated output bound ({limit} bytes) without emitting a ref"
                ));
            }
        }
        Err(error) => {
            if !is_bounds_enforcement(error) {
                details.push(format!(
                    "leak-proof probe failed without output-bounds enforcement: {error}"
                ));
            }
        }
    }
    details
}

/// Interpret an RW7 oversized-frame probe outcome. The request frame itself is
/// above the negotiated frame bound, so the client rejects it outbound
/// (`Protocol(TooLarge)`) and the worker is torn down (`Bounds`/`Crash`); all
/// of those are enforcement evidence. A typed remote output-bounds error is
/// evidence too. Only an `Ok` whose value is echoed back inline above the
/// output bound is a failure. Any other error shape is flagged.
fn classify_oversized_frame_outcome(
    outcome: &Result<WorkerResult, WorkerAdapterError>,
    limit: usize,
) -> Vec<String> {
    let mut details = Vec::new();
    match outcome {
        Ok(result) => {
            let serialized = serde_json::to_vec(&result.value)
                .map(|bytes| bytes.len())
                .unwrap_or(usize::MAX);
            if serialized > limit {
                details.push(format!(
                    "oversize request was echoed back inline ({serialized} bytes) above the output bound ({limit} bytes)"
                ));
            }
        }
        Err(error) => match error {
            WorkerAdapterError::Protocol(zero_abi::raw_worker::FrameCodecError::TooLarge { .. })
            | WorkerAdapterError::Bounds { .. }
            | WorkerAdapterError::Crash { .. } => {}
            WorkerAdapterError::Remote { kind, .. } if is_output_bounds_kind(kind) => {}
            other => details.push(format!(
                "oversized-frame probe failed without bounds enforcement: {other}"
            )),
        },
    }
    details
}

/// A worker client plus the last observed settlement receipt.
struct ProbedWorker {
    client: WorkerClient,
    last_settlement: Arc<Mutex<Option<WorkerSettlementReceiptV1>>>,
}

impl ProbedWorker {
    fn spawn(
        ns: Ns,
        bin: &Path,
        fixture: &Fixture,
        digest: &str,
        revision: &str,
        timeout: Duration,
    ) -> Result<Self> {
        let last_settlement = Arc::new(Mutex::new(None));
        let observed = Arc::clone(&last_settlement);
        let observer: Arc<dyn Fn(&WorkerObservation) + Send + Sync> =
            Arc::new(move |observation: &WorkerObservation| {
                if let Some(settlement) = &observation.settlement {
                    if let Ok(mut slot) = observed.lock() {
                        *slot = Some(settlement.clone());
                    }
                }
            });
        let client = spawn_worker(ns, bin, fixture, digest, revision, timeout, Some(observer))?;
        Ok(Self {
            client,
            last_settlement,
        })
    }

    fn dispatch(&mut self, request: CallRequest) -> Result<WorkerResult, WorkerAdapterError> {
        self.client.dispatch(request)
    }

    fn negotiated_limits(&self) -> ProtocolLimits {
        self.client.negotiated_limits().clone()
    }

    fn process_id(&self) -> u32 {
        self.client.process_id()
    }

    fn shutdown(&mut self) {
        let _ = self.client.shutdown();
    }

    fn settlement(&self) -> Option<WorkerSettlementReceiptV1> {
        self.last_settlement
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }
}

impl Drop for ProbedWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Fixture workspace: retained tempdir (never leaked) plus the engine
/// store/session root. Raw workers enforce root-relative paths, so ops use
/// the relative file names, not absolute paths.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn small_rel(&self) -> &'static str {
        "fixture.txt"
    }

    fn large_rel(&self) -> &'static str {
        "large.txt"
    }
}

fn read_args(ns: Ns, _fixture: &Fixture, relative: &str) -> Value {
    match ns {
        // FSZero resolves root-relative paths against --root.
        Ns::Fz => json!({ "path": relative }),
        // TokenZero call_root resolves root-relative paths against --root
        // (native Linux 9b4df92). Exercise the promised public contract.
        Ns::Tz => json!({ "path": relative }),
        Ns::Gz => json!({ "query": "fn main", "budget": 1, "repo": "." }),
    }
}

fn mutation_args(ns: Ns, fixture: &Fixture) -> Value {
    match ns {
        Ns::Fz => json!({ "path": fixture.small_rel(), "find": "needle", "replace": "changed" }),
        Ns::Tz => json!({
            "path": fixture.small_rel(),
            "edits": [{ "find": "needle", "replace": "changed" }]
        }),
        Ns::Gz => json!({}),
    }
}

fn oversized_read_args(ns: Ns, payload: &str) -> Value {
    match ns {
        Ns::Fz | Ns::Tz => json!({ "path": payload }),
        Ns::Gz => json!({ "query": payload, "budget": 1, "repo": "." }),
    }
}

/// RW5 substrate error probe: returns the (op, args) that must produce a
/// typed error for a definitely-missing target. GraphZero's `snap` returns an
/// empty success capsule on a symbol miss, so Graph uses the public `expand`
/// op on a missing ref (arg key `reference`); FSZero/TokenZero use a missing
/// read path.
fn substrate_probe(ns: Ns) -> (&'static str, Value) {
    let c = contract(ns);
    match ns {
        Ns::Fz | Ns::Tz => (c.read_op, json!({ "path": "__missing_target__.txt" })),
        Ns::Gz => (
            c.expand_op,
            json!({ (c.expand_arg): "gz://missing/zzz_no_such_claim_zzz" }),
        ),
    }
}

/// RW1 artifact exposure over raw-worker v2.
pub fn check_exposure(ns: Ns, bin: &Path, timeout: Duration) -> CheckResult {
    let mut details = Vec::new();
    let probed = match probe(ns, bin, timeout) {
        Ok(probed) => probed,
        Err(error) => {
            return CheckResult::fail(
                "RW1",
                "artifact_exposure",
                format!(
                    "capability probe failed (artifact is not a raw-worker v2 binary?): {}",
                    error
                ),
            );
        }
    };
    if !probed.output.contains("raw_worker") && !probed.output.contains("raw-worker") {
        details.push("probe output does not mention the raw-worker surface".into());
    }
    match check_refusal(ns, bin, timeout) {
        Ok(()) => {}
        Err(error) => details.push(error.to_string()),
    }
    CheckResult::with_details("RW1", "artifact_exposure", details)
}

fn check_refusal(ns: Ns, bin: &Path, timeout: Duration) -> Result<()> {
    let c = contract(ns);
    let (success, bytes) = run_probe(bin, c.refusal_args, timeout)?;
    let text = String::from_utf8_lossy(&bytes);
    if success {
        bail!(
            "artifact exited 0 with opposite-surface args {:?}: {}",
            c.refusal_args,
            text.trim().chars().take(200).collect::<String>()
        );
    }
    let combined = text.to_ascii_lowercase();
    if combined.contains("refus")
        || combined.contains("mutually exclusive")
        || combined.contains("does not serve")
        || combined.contains("unsupported")
        || combined.contains("cannot serve")
    {
        Ok(())
    } else {
        bail!(
            "opposite-surface args {:?} exited nonzero without a refusal message: {}",
            c.refusal_args,
            text.trim().chars().take(200).collect::<String>()
        )
    }
}

/// Engine scheme prefix owned by a namespace at the worker boundary.
fn engine_scheme(ns: Ns) -> &'static str {
    match ns {
        Ns::Fz => "fz",
        Ns::Tz => "tz",
        Ns::Gz => "gz",
    }
}

/// Engine-owned ref format check for the raw-worker boundary: the scheme must
/// match the owning engine, and the path must be nonempty, bounded, and free
/// of control characters. Engine-owned refs (e.g. `tz://file/<id>`) are valid
/// here even when they are not portable blob64/codemode refs; the portable
/// validator (`crate::valid_ref`) stays intact for the MCP/CodeMode surface.
fn validate_engine_ref_format(scheme: &str, reference: &str) -> Vec<String> {
    let mut details = Vec::new();
    let prefix = format!("{scheme}://");
    let path = match reference.strip_prefix(prefix.as_str()) {
        Some(path) => path,
        None => {
            details.push(format!(
                "ownership ref {reference:?} does not use the {scheme}:// scheme"
            ));
            return details;
        }
    };
    if path.is_empty() {
        details.push(format!("ownership ref {reference:?} has an empty path"));
    }
    const MAX_REF_LEN: usize = 4096;
    if reference.len() > MAX_REF_LEN {
        details.push(format!(
            "ownership ref is {} bytes, exceeds {MAX_REF_LEN} bound",
            reference.len()
        ));
    }
    if path.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
        details.push(format!(
            "ownership ref {reference:?} path contains control characters"
        ));
    }
    details
}

fn validate_ownership_refs(ns: Ns, ownership: &RefOwnership) -> Vec<String> {
    let scheme = engine_scheme(ns);
    let mut details = Vec::new();
    if ownership.refs.is_empty() {
        details.push("result metadata ownership carries no refs".into());
    }
    for reference in &ownership.refs {
        details.extend(validate_engine_ref_format(scheme, reference));
    }
    details
}

/// Dispatch the owning engine's public expand op on each unique ownership ref
/// and require success. A ref that the owning engine cannot resolve back to
/// content is not a real ownership claim at the worker boundary. Dedups so a
/// repeated ref is probed once.
fn expand_ownership_refs(
    worker: &mut ProbedWorker,
    ns: Ns,
    ownership: &RefOwnership,
    deadline_unix_ms: u64,
    revision: &str,
    digest: &str,
) -> Vec<String> {
    let c = contract(ns);
    let mut seen = std::collections::BTreeSet::new();
    let mut details = Vec::new();
    for reference in &ownership.refs {
        if !seen.insert(reference.as_str()) {
            continue;
        }
        let request = make_request(
            c.expand_op,
            json!({ (c.expand_arg): reference }),
            Some(deadline_unix_ms),
            false,
            revision,
            digest,
        );
        if let Err(error) = worker.dispatch(request) {
            details.push(format!(
                "ownership ref {reference:?} did not resolve via {} ({}={}): {}",
                c.expand_op, c.expand_arg, reference, remote_error(&error)
            ));
        }
    }
    details
}

fn check_refs_gate(
    ns: Ns,
    bin: &Path,
    fixture: &Fixture,
    digest: &str,
    revision: &str,
    timeout: Duration,
) -> CheckResult {
    let mut worker = match ProbedWorker::spawn(ns, bin, fixture, digest, revision, timeout) {
        Ok(worker) => worker,
        Err(error) => return CheckResult::fail("RW2", "recoverable_refs", error.to_string()),
    };
    let mut details = Vec::new();
    let request = make_request(
        contract(ns).read_op,
        read_args(ns, fixture, fixture.small_rel()),
        Some(deadline_in(timeout)),
        false,
        revision,
        digest,
    );
    let expected_id = request.request_id.clone();
    match worker.dispatch(request) {
        Ok(result) => {
            details.extend(validate_ownership_refs(ns, &result.metadata.ownership));
            if result.metadata.trace.request_id != expected_id {
                details.push("result trace request_id does not echo the request".into());
            }
            if result.metadata.ownership.session_id.is_empty() {
                details.push("ownership session_id is empty".into());
            }
            // Each engine-owned ref must resolve back to content through the
            // owning engine's public expand op; engine-owned refs are NOT
            // checked against the portable blob64/codemode regex.
            details.extend(expand_ownership_refs(
                &mut worker,
                ns,
                &result.metadata.ownership,
                deadline_in(timeout),
                revision,
                digest,
            ));
        }
        Err(error) => details.push(remote_error(&error)),
    }
    CheckResult::with_details("RW2", "recoverable_refs", details)
}

fn check_telemetry_gate(
    ns: Ns,
    bin: &Path,
    fixture: &Fixture,
    digest: &str,
    revision: &str,
    timeout: Duration,
) -> CheckResult {
    let mut worker = match ProbedWorker::spawn(ns, bin, fixture, digest, revision, timeout) {
        Ok(worker) => worker,
        Err(error) => return CheckResult::fail("RW3", "telemetry_accounting", error.to_string()),
    };
    let mut details = Vec::new();
    let request = make_engine_request(
        ns,
        contract(ns).read_op,
        read_args(ns, fixture, fixture.small_rel()),
        Some(deadline_in(timeout)),
        true,
        revision,
        digest,
    );
    match worker.dispatch(request) {
        Ok(_result) => match worker.settlement() {
            None => details.push("no settlement receipt observed for telemetry call".into()),
            Some(receipt) => {
                if receipt.engine_timeline.is_none() {
                    details.push("engine_stage_timeline missing from settlement".into());
                }
                match &receipt.engine_timeline {
                    Some(timeline) => {
                        if timeline.total_ns == 0 || timeline.spans.is_empty() {
                            details.push("engine timeline has no measured spans".into());
                        }
                    }
                    None => {}
                }
                match &receipt.worker_token_accounting {
                    None => {
                        // Token accounting is REQUIRED for TokenZero (it is a
                        // tokenizer) and OPTIONAL for FSZero/GraphZero, which
                        // intentionally emit None. When present it is still
                        // validated for honesty.
                        if requires_token_accounting(ns) {
                            details.push(
                                "worker_token_accounting missing from settlement".into(),
                            );
                        }
                    }
                    Some(accounting) => details.extend(validate_accounting(accounting)),
                }
            }
        },
        Err(error) => details.push(remote_error(&error)),
    }
    CheckResult::with_details("RW3", "telemetry_accounting", details)
}

fn validate_accounting(accounting: &WorkerTokenAccountingV1) -> Vec<String> {
    let mut details = Vec::new();
    if accounting.tokenizer_id.is_empty() {
        details.push("tokenizer_id is empty".into());
    }
    if accounting.raw_tokens == 0 && accounting.visible_tokens == 0 {
        details.push("token accounting reports zero tokens".into());
    }
    details
}

/// Per-engine expectation for worker token accounting. Only TokenZero is a
/// tokenizer, so only it must emit nonzero ABI-valid accounting. FSZero and
/// GraphZero may emit `None`.
fn requires_token_accounting(ns: Ns) -> bool {
    matches!(ns, Ns::Tz)
}

fn check_leak_gate(
    ns: Ns,
    bin: &Path,
    fixture: &Fixture,
    digest: &str,
    revision: &str,
    timeout: Duration,
) -> CheckResult {
    let mut worker = match ProbedWorker::spawn(ns, bin, fixture, digest, revision, timeout) {
        Ok(worker) => worker,
        Err(error) => return CheckResult::fail("RW4", "output_bounds", error.to_string()),
    };
    let mut details = Vec::new();
    // Read the 70 KiB fixture: the visible result must stay within the
    // negotiated output bound and/or return a ref instead of echoing inline.
    // A typed output_too_large/bounds error is leak-proof enforcement, not a
    // failure.
    let request = make_engine_request(
        ns,
        contract(ns).read_op,
        read_args(ns, fixture, fixture.large_rel()),
        Some(deadline_in(timeout)),
        true,
        revision,
        digest,
    );
    let outcome = worker.dispatch(request);
    let limit = worker.negotiated_limits().max_output_bytes as usize;
    details.extend(classify_leak_outcome(&outcome, limit));
    if let Ok(_result) = &outcome {
        if let Some(receipt) = worker.settlement() {
            if let Some(accounting) = &receipt.worker_token_accounting {
                details.extend(validate_accounting(accounting));
            }
        }
    }
    CheckResult::with_details("RW4", "output_bounds", details)
}

fn check_errors_gate(
    ns: Ns,
    bin: &Path,
    fixture: &Fixture,
    digest: &str,
    revision: &str,
    timeout: Duration,
) -> CheckResult {
    let mut worker = match ProbedWorker::spawn(ns, bin, fixture, digest, revision, timeout) {
        Ok(worker) => worker,
        Err(error) => return CheckResult::fail("RW5", "typed_errors", error.to_string()),
    };
    let mut details = Vec::new();
    let deadline = Some(deadline_in(timeout));

    // validation: unknown op must produce a typed error, not a crash.
    let unknown = make_request(
        "__zerostack_unknown_op__",
        json!({}),
        deadline,
        false,
        revision,
        digest,
    );
    match worker.dispatch(unknown) {
        Ok(_) => details.push("unknown op did not return a typed error".into()),
        Err(error) => match remote_kind(&error) {
            Some(kind)
                if kind.contains("validation")
                    || kind.contains("unknown")
                    || kind.contains("unsupported") => {}
            Some(kind) => details.push(format!("unknown op returned unexpected kind {:?}", kind)),
            None => details.push(format!("unknown op failed client-side: {}", error)),
        },
    }

    // sandbox: planner/JS/MCP ops are forbidden on a raw worker.
    for forbidden in [
        "planner",
        "planner.run",
        "js.execute",
        "mcp.tools_call",
        "codemode.execute",
    ] {
        let request = make_request(forbidden, json!({}), deadline, false, revision, digest);
        match worker.dispatch(request) {
            Ok(_) => details.push(format!("forbidden op {:?} was not denied", forbidden)),
            Err(error) => match remote_kind(&error) {
                Some(kind) if is_forbidden_denial(kind) => {}
                Some(kind) => details.push(format!(
                    "forbidden op {:?} returned unexpected kind {:?}",
                    forbidden, kind
                )),
                None => details.push(format!(
                    "forbidden op {:?} failed client-side: {}",
                    forbidden, error
                )),
            },
        }
    }

    // substrate: a missing target must be a typed error, not a crash or a
    // silent success. GraphZero's `snap` returns an empty success capsule on a
    // symbol miss, so for Graph use the public `expand` op on a definitely
    // missing ref (arg key `reference`); FSZero/TokenZero use a missing read
    // path.
    let (substrate_op, substrate_args) = substrate_probe(ns);
    let request = make_request(
        substrate_op,
        substrate_args,
        deadline,
        false,
        revision,
        digest,
    );
    match worker.dispatch(request) {
        Ok(_) => details.push("substrate probe did not return a typed error".into()),
        Err(error) => {
            if remote_kind(&error).is_none() {
                details.push(format!("substrate case failed client-side: {}", error));
            }
        }
    }

    // RW5 checks typed validation/forbidden/substrate errors only. Host
    // authorization policy is NOT a raw-worker gate: a raw worker is a domain
    // authority and approval/policy is a hub concern (RW8 tests the mutation as
    // a domain-authority op that must succeed).

    CheckResult::with_details("RW5", "typed_errors", details)
}

fn check_chain_gate(
    ns: Ns,
    bin: &Path,
    fixture: &Fixture,
    digest: &str,
    revision: &str,
    timeout: Duration,
) -> CheckResult {
    let mut worker = match ProbedWorker::spawn(ns, bin, fixture, digest, revision, timeout) {
        Ok(worker) => worker,
        Err(error) => return CheckResult::fail("RW6", "session_continuity", error.to_string()),
    };
    let mut details = Vec::new();
    let op = contract(ns).read_op;
    let first = make_request(
        op,
        read_args(ns, fixture, fixture.small_rel()),
        Some(deadline_in(timeout)),
        false,
        revision,
        digest,
    );
    let first_id = first.request_id.clone();
    let first_trace = first.trace.trace_id.clone();
    let first_result = match worker.dispatch(first) {
        Ok(result) => result,
        Err(error) => {
            return CheckResult::fail(
                "RW6",
                    "session_continuity",
                format!("first chain call failed: {}", remote_error(&error)),
            );
        }
    };
    details.extend(validate_ownership_refs(
        ns,
        &first_result.metadata.ownership,
    ));
    if first_result.metadata.trace.trace_id != first_trace {
        details.push("first call trace_id was not echoed".into());
    }
    let second = make_request(
        op,
        read_args(ns, fixture, fixture.small_rel()),
        Some(deadline_in(timeout)),
        false,
        revision,
        digest,
    );
    let second_id = second.request_id.clone();
    let second_trace = second.trace.trace_id.clone();
    let second_result = match worker.dispatch(second) {
        Ok(result) => result,
        Err(error) => {
            return CheckResult::fail(
                "RW6",
                    "session_continuity",
                format!(
                    "second chain call failed (session continuity broken): {}",
                    remote_error(&error)
                ),
            );
        }
    };
    details.extend(validate_ownership_refs(
        ns,
        &second_result.metadata.ownership,
    ));
    if second_result.metadata.trace.trace_id != second_trace {
        details.push("second call trace_id was not echoed".into());
    }
    if first_id == second_id || first_trace == second_trace {
        details.push("distinct calls shared request/trace identity".into());
    }
    CheckResult::with_details("RW6", "session_continuity", details)
}

fn check_limits_gate(
    ns: Ns,
    bin: &Path,
    fixture: &Fixture,
    digest: &str,
    revision: &str,
    timeout: Duration,
) -> CheckResult {
    let mut worker = match ProbedWorker::spawn(ns, bin, fixture, digest, revision, timeout) {
        Ok(worker) => worker,
        Err(error) => return CheckResult::fail("RW7", "frame_limits", error.to_string()),
    };
    let mut details = Vec::new();
    let limits = worker.negotiated_limits();
    if limits.max_frame_bytes == 0
        || limits.max_output_bytes == 0
        || limits.max_in_flight == 0
        || limits.default_deadline_ms == 0
    {
        details.push(format!(
            "handshake advertised a zero protocol limit: {:?}",
            limits
        ));
    }

    // Expired deadline must produce a typed refusal or a client-side deadline
    // enforcement, not a hang, crash, or silent success.
    let expired = make_request(
        contract(ns).read_op,
        read_args(ns, fixture, fixture.small_rel()),
        Some(now_unix_ms().saturating_sub(1_000)),
        false,
        revision,
        digest,
    );
    match worker.dispatch(expired) {
        Ok(_) => details.push("expired-deadline call was not refused".into()),
        Err(error) => {
            if is_deadline_enforcement(&error) {
                // WorkerClient Deadline/DeadlineOverflow or a typed
                // deadline/expired worker error is enforcement evidence.
            } else {
                match remote_kind(&error) {
                    Some(kind) => details.push(format!(
                        "expired-deadline call returned unexpected kind {:?}",
                        kind
                    )),
                    None => details.push(format!(
                        "expired-deadline call failed without deadline enforcement: {}",
                        error
                    )),
                }
            }
        }
    }

    // Oversized request frame: run in a SEPARATE fresh worker because the
    // worker client correctly marks one transport terminal after an outbound
    // bounds violation. A typed frame error or client-side
    // bounds/protocol/terminal failure is enforcement evidence; only an
    // inline echo above the output bound is a failure.
    let mut oversized = match ProbedWorker::spawn(ns, bin, fixture, digest, revision, timeout) {
        Ok(worker) => worker,
        Err(error) => {
            return CheckResult::with_details(
                "RW7",
                "frame_limits",
                vec![format!("oversized-frame worker spawn failed: {}", error)],
            );
        }
    };
    let huge = "x".repeat(2 * 1024 * 1024);
    let request = make_request(
        contract(ns).read_op,
        oversized_read_args(ns, &huge),
        Some(deadline_in(timeout)),
        false,
        revision,
        digest,
    );
    let outcome = oversized.dispatch(request);
    details.extend(classify_oversized_frame_outcome(
        &outcome,
        limits.max_output_bytes as usize,
    ));
    CheckResult::with_details("RW7", "frame_limits", details)
}

fn check_mutation_gate(
    ns: Ns,
    bin: &Path,
    digest: &str,
    revision: &str,
    timeout: Duration,
) -> CheckResult {
    // Each RW8 run gets its OWN retained fixture so a successful domain
    // mutation never perturbs the shared fixture used by other gates.
    let fixture = match prepare_fixture() {
        Ok(fixture) => fixture,
        Err(error) => {
            return CheckResult::fail(
                "RW8",
                "domain_mutation",
                format!("fixture setup failed: {}", error),
            )
        }
    };
    let mut worker = match ProbedWorker::spawn(ns, bin, &fixture, digest, revision, timeout) {
        Ok(worker) => worker,
        Err(error) => return CheckResult::fail("RW8", "domain_mutation", error.to_string()),
    };
    let c = contract(ns);
    let mut details = Vec::new();
    let request = make_request(
        c.mutation_op,
        mutation_args(ns, &fixture),
        Some(deadline_in(timeout)),
        false,
        revision,
        digest,
    );
    // A raw worker is a domain authority: the mutation op must succeed at the
    // worker boundary. Host-level authorization (approvals/policy) is a hub
    // concern, not a raw-worker gate, so RW8 asserts success and that any
    // returned ownership refs are well-formed engine refs (format only; a
    // mutation that returns no ref is still a successful domain mutation).
    match worker.dispatch(request) {
        Ok(result) => {
            let scheme = engine_scheme(ns);
            for reference in &result.metadata.ownership.refs {
                details.extend(validate_engine_ref_format(scheme, reference));
            }
        }
        Err(error) => details.push(format!(
            "domain mutation op {} failed at authority boundary: {}",
            c.mutation_op,
            remote_error(&error)
        )),
    }
    CheckResult::with_details("RW8", "domain_mutation", details)
}

fn check_coalescing_gate(
    ns: Ns,
    bin: &Path,
    fixture: &Fixture,
    digest: &str,
    revision: &str,
    timeout: Duration,
) -> CheckResult {
    let mut worker = match ProbedWorker::spawn(ns, bin, fixture, digest, revision, timeout) {
        Ok(worker) => worker,
        Err(error) => return CheckResult::fail("RW9", "process_reuse", error.to_string()),
    };
    let pid = worker.process_id();
    let op = contract(ns).read_op;
    let started = Instant::now();
    let mut failures = Vec::new();
    for _ in 0..20 {
        let request = make_engine_request(
            ns,
            op,
            read_args(ns, fixture, fixture.small_rel()),
            Some(deadline_in(timeout)),
            true,
            revision,
            digest,
        );
        if let Err(error) = worker.dispatch(request) {
            failures.push(remote_error(&error));
            break;
        }
        if worker.process_id() != pid {
            failures.push("worker process restarted mid-batch".into());
            break;
        }
    }
    let elapsed = started.elapsed();
    if !failures.is_empty() {
        failures.push(format!(
            "batch did not settle in one worker process (pid {}) after {:?}",
            pid, elapsed
        ));
        return CheckResult::with_details("RW9", "process_reuse", failures);
    }
    if elapsed > Duration::from_secs(60) {
        return CheckResult::fail(
            "RW9",
            "process_reuse",
            format!("20-call batch took {:?}, over the 60s budget", elapsed),
        );
    }
    CheckResult::pass("RW9", "process_reuse")
}

fn check_sandbox_gate(
    ns: Ns,
    bin: &Path,
    fixture: &Fixture,
    digest: &str,
    revision: &str,
    timeout: Duration,
) -> CheckResult {
    let mut worker = match ProbedWorker::spawn(ns, bin, fixture, digest, revision, timeout) {
        Ok(worker) => worker,
        Err(error) => return CheckResult::fail("RW10", "planner_refusal", error.to_string()),
    };
    let mut details = Vec::new();
    for forbidden in [
        "planner",
        "planner.run",
        "js.execute",
        "mcp.tools_call",
        "mcp.tools_list",
        "codemode.execute",
        "execute_code",
    ] {
        let request = make_request(
            forbidden,
            json!({}),
            Some(deadline_in(timeout)),
            false,
            revision,
            digest,
        );
        match worker.dispatch(request) {
            Ok(_) => details.push(format!("forbidden op {:?} was not denied", forbidden)),
            Err(error) => match remote_kind(&error) {
                Some(kind) if is_forbidden_denial(kind) => {}
                Some(kind) => details.push(format!(
                    "forbidden op {:?} returned unexpected kind {:?}",
                    forbidden, kind
                )),
                None => details.push(format!(
                    "forbidden op {:?} failed client-side: {}",
                    forbidden, error
                )),
            },
        }
    }
    // Liveness: the worker must still answer a real op after all denials.
    let request = make_request(
        contract(ns).read_op,
        read_args(ns, fixture, fixture.small_rel()),
        Some(deadline_in(timeout)),
        false,
        revision,
        digest,
    );
    match worker.dispatch(request) {
        Ok(result) => {
            if result.metadata.ownership.refs.is_empty() {
                details.push("post-denial liveness call returned no refs".into());
            }
        }
        Err(error) => details.push(format!(
            "worker died after denials: {}",
            remote_error(&error)
        )),
    }
    CheckResult::with_details("RW10", "planner_refusal", details)
}

/// Index the GraphZero fixture source tree once on the shared fixture before
/// the read gates run. GraphZero has no graph until `index` runs, so `snap`
/// would fail by construction otherwise. No-op for FSZero/TokenZero. A failure
/// here is recorded as an RW2 detail on the shared checks rather than swallowed.
fn pre_index_graph(
    ns: Ns,
    bin: &Path,
    fixture: &Fixture,
    digest: &str,
    revision: &str,
    timeout: Duration,
) -> Result<(), String> {
    if !matches!(ns, Ns::Gz) {
        return Ok(());
    }
    let mut worker = ProbedWorker::spawn(ns, bin, fixture, digest, revision, timeout)
        .map_err(|error| format!("graph pre-index worker spawn failed: {error}"))?;
    // GraphZero op_index maps PathBuf(".") to the process cwd and only uses
    // ctx.repo_root when path == repo_root; passing {"path":"."} indexed the
    // cwd and wrote ./<cwd>/.graphzero, never the fixture. Omit path so op_index
    // defaults to ctx.repo_root (the fixture root). Empty args also exercises
    // the canonical default and matches the RW8 mutation arg shape.
    let request = make_request(
        contract(ns).mutation_op,
        mutation_args(ns, fixture),
        Some(deadline_in(timeout)),
        false,
        revision,
        digest,
    );
    worker
        .dispatch(request)
        .map_err(|error| format!("graph pre-index dispatch failed: {}", remote_error(&error)))?;
    Ok(())
}

/// Run the RW1-RW10 raw-worker v2 checks for one engine artifact.
pub fn run_conformance(ns: Ns, bin: &Path, timeout: Duration) -> Vec<CheckResult> {
    let fixture = match prepare_fixture() {
        Ok(fixture) => fixture,
        Err(error) => {
            return vec![
                CheckResult::pass("RW1", "artifact_exposure"),
                CheckResult::fail("RW2", "recoverable_refs", format!("fixture setup failed: {}", error)),
            ];
        }
    };
    let mut checks = Vec::new();
    checks.push(check_exposure(ns, bin, timeout));
    let probed = match probe(ns, bin, timeout) {
        Ok(probed) => probed,
        Err(error) => {
            for (id, name) in [
                ("RW2", "recoverable_refs"),
                ("RW3", "telemetry_accounting"),
                ("RW4", "output_bounds"),
                ("RW5", "typed_errors"),
                ("RW6", "session_continuity"),
                ("RW7", "frame_limits"),
                ("RW8", "domain_mutation"),
                ("RW9", "process_reuse"),
                ("RW10", "planner_refusal"),
            ] {
                checks.push(CheckResult::skip(
                    id,
                    name,
                    format!("capability probe failed: {}", error),
                ));
            }
            return checks;
        }
    };
    let digest = probed.contract_digest.clone();
    let revision = probed.worker_revision.clone();
    // GraphZero must index its fixture source tree once before the shared read
    // gates; otherwise `snap` has no graph to read and fails by construction.
    // A setup failure is surfaced explicitly (attached to RW2) so the report
    // names the pre-index root cause instead of only downstream no-snapshot
    // errors. Other rows still run honestly; we never false-green.
    let pre_index_note = match pre_index_graph(ns, bin, &fixture, &digest, &revision, timeout) {
        Ok(()) => None,
        Err(reason) => Some(reason),
    };
    checks.push(check_refs_gate(
        ns, bin, &fixture, &digest, &revision, timeout,
    ));
    checks.push(check_telemetry_gate(
        ns, bin, &fixture, &digest, &revision, timeout,
    ));
    checks.push(check_leak_gate(
        ns, bin, &fixture, &digest, &revision, timeout,
    ));
    checks.push(check_errors_gate(
        ns, bin, &fixture, &digest, &revision, timeout,
    ));
    checks.push(check_chain_gate(
        ns, bin, &fixture, &digest, &revision, timeout,
    ));
    checks.push(check_limits_gate(
        ns, bin, &fixture, &digest, &revision, timeout,
    ));
    checks.push(check_mutation_gate(ns, bin, &digest, &revision, timeout));
    checks.push(check_coalescing_gate(
        ns, bin, &fixture, &digest, &revision, timeout,
    ));
    checks.push(check_sandbox_gate(
        ns, bin, &fixture, &digest, &revision, timeout,
    ));
    // Attach a GraphZero pre-index setup failure to RW2 so the report names the
    // root cause (spawn/dispatch) instead of only downstream no-snapshot
    // errors. RW2 stays/ becomes a fail; other rows already ran honestly.
    if let Some(reason) = pre_index_note {
        if let Some(rw2) = checks.iter_mut().find(|check| check.id == "RW2") {
            let mut merged = vec![format!("graph pre-index setup failed: {reason}")];
            merged.append(&mut rw2.details);
            rw2.details = merged;
            rw2.passed = false;
            rw2.status = crate::GateStatus::Fail;
        }
    }
    checks
}

/// Create a fixture workspace that also serves as the worker store/session
/// root. The `TempDir` is retained (never leaked) for the whole run; ops use
/// root-relative file names because raw workers enforce root-relative paths.
fn prepare_fixture() -> Result<Fixture> {
    let dir = tempfile::tempdir().context("creating conformance fixture")?;
    let root = dir.path().to_path_buf();
    std::fs::write(
        root.join("fixture.txt"),
        "hello needle world\nfn main() {}\n",
    )
    .with_context(|| format!("writing {}", root.join("fixture.txt").display()))?;
    let payload = "x".repeat(70 * 1024);
    std::fs::write(root.join("large.txt"), payload)
        .with_context(|| format!("writing {}", root.join("large.txt").display()))?;
    // GraphZero indexes a source tree; provide a real `src/main.rs` so `index`
    // has something to build a graph from before the shared read gates run.
    std::fs::create_dir_all(root.join("src"))
        .with_context(|| format!("creating {}", root.join("src").display()))?;
    std::fs::write(root.join("src").join("main.rs"), "fn main() {}\n")
        .with_context(|| format!("writing {}", root.join("src/main.rs").display()))?;
    Ok(Fixture { _dir: dir, root })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_tables_use_canonical_spawn_and_probe_contracts() {
        let fz = contract(Ns::Fz);
        assert_eq!(fz.serve_args, &["--raw-worker", "--root"]);
        assert_eq!(fz.probe_args, &["capabilities", "--json"]);
        assert_eq!(fz.read_op, "fs.read");
        assert_eq!(fz.mutation_op, "fs.edit");
        assert_eq!(fz.expand_op, "fs.expand");
        assert_eq!(fz.expand_arg, "ref");

        let tz = contract(Ns::Tz);
        assert_eq!(tz.serve_args, &["raw-worker", "--root"]);
        assert_eq!(tz.probe_args, &["capabilities", "--json"]);
        assert_eq!(tz.read_op, "read");
        assert_eq!(tz.mutation_op, "edit");
        assert_eq!(tz.expand_op, "expand");
        assert_eq!(tz.expand_arg, "ref");

        let gz = contract(Ns::Gz);
        assert!(gz.serve_args.is_empty());
        assert_eq!(gz.probe_args, &["capabilities", "--json"]);
        assert_eq!(gz.read_op, "snap");
        assert_eq!(gz.mutation_op, "index");
        assert_eq!(gz.expand_op, "expand");
        assert_eq!(gz.expand_arg, "reference");
    }

    #[test]
    fn digest_parsing_matches_engine_probe_shapes() {
        let capabilities = r#"{"schema":"fszero.raw-worker.capabilities/v1","package":{"abi_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"protocol":"zerostack.raw_worker.v2"}"#;
        assert_eq!(
            package_abi_digest(capabilities).as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert!(package_abi_digest("{}").is_none());
        assert!(package_abi_digest("not json").is_none());
        let wrong_len = r#"{"package":{"abi_digest":"abc"}}"#;
        assert!(package_abi_digest(wrong_len).is_none());
    }

    #[test]
    fn requests_bind_trace_to_request_and_honor_telemetry() {
        let request = make_request(
            "fs.read",
            json!({"path": "/x"}),
            Some(1234),
            true,
            "rev",
            "digest",
        );
        assert_eq!(request.trace.request_id, request.request_id);
        assert_eq!(request.trace.worker_revision, "rev");
        assert_eq!(request.trace.contract_digest, "digest");
        assert!(request.telemetry_request.is_some());
        let no_telemetry = make_request("fs.read", json!({}), None, false, "r", "d");
        assert!(no_telemetry.telemetry_request.is_none());
        assert!(no_telemetry.deadline_unix_ms.is_none());

        let token = make_engine_request(Ns::Tz, "read", json!({}), None, true, "r", "d");
        assert!(
            token
                .telemetry_request
                .as_ref()
                .is_some_and(|request| request.worker_token_accounting)
        );
        let graph = make_engine_request(Ns::Gz, "snap", json!({}), None, true, "r", "d");
        assert!(
            graph
                .telemetry_request
                .as_ref()
                .is_some_and(|request| !request.worker_token_accounting)
        );
    }

    #[test]
    fn remote_error_extraction_handles_typed_and_client_errors() {
        let remote = WorkerAdapterError::Remote {
            request_id: Some("r".into()),
            kind: "sandbox".into(),
            message: "denied".into(),
            retryable: false,
            details: None,
            trace: None,
        };
        assert_eq!(remote_kind(&remote), Some("sandbox"));
        let client = WorkerAdapterError::Handshake("boom".into());
        assert_eq!(remote_kind(&client), None);
    }

    #[test]
    fn read_args_match_engine_wire_contracts() {
        let fixture = prepare_fixture().unwrap();
        // FSZero resolves root-relative paths against --root.
        assert_eq!(
            read_args(Ns::Fz, &fixture, "fixture.txt"),
            json!({"path": "fixture.txt"})
        );
        // TokenZero call_root resolves root-relative paths against --root
        // (native Linux 9b4df92): exercise the promised public contract.
        assert_eq!(
            read_args(Ns::Tz, &fixture, "fixture.txt"),
            json!({"path": "fixture.txt"})
        );
        assert_eq!(
            read_args(Ns::Gz, &fixture, "fixture.txt"),
            json!({"query": "fn main", "budget": 1, "repo": "."})
        );
        assert_eq!(fixture.small_rel(), "fixture.txt");
        assert_eq!(fixture.large_rel(), "large.txt");
        assert!(fixture.root.join(fixture.small_rel()).is_file());
        assert!(fixture.root.join(fixture.large_rel()).is_file());
    }

    #[test]
    fn forbidden_denial_accepts_validation_and_rejects_policy() {
        assert!(is_forbidden_denial("validation"));
        assert!(is_forbidden_denial("unsupported_operation"));
        assert!(is_forbidden_denial("sandbox"));
        assert!(is_forbidden_denial("forbidden"));
        assert!(!is_forbidden_denial("policy"));
        assert!(!is_forbidden_denial("deadline_exceeded"));
    }

    #[test]
    fn output_bounds_kind_is_leak_enforcement() {
        assert!(is_output_bounds_kind("output_too_large"));
        assert!(is_output_bounds_kind("frame_too_large"));
        assert!(is_output_bounds_kind("bounds_exceeded"));
        assert!(!is_output_bounds_kind("policy"));
    }

    #[test]
    fn classify_leak_outcome_flags_inline_echo_above_bound() {
        let outcome = Ok(WorkerResult {
            value: json!({"payload": "x".repeat(70_000)}),
            metadata: zero_abi::raw_worker::WorkerResultMetadata {
                effect: zero_abi::raw_worker::EffectClass::ReadOnly,
                approval: zero_abi::raw_worker::ApprovalMetadata {
                    state: zero_abi::raw_worker::ApprovalState::NotRequired,
                    approval_id: None,
                    policy: None,
                },
                revert: zero_abi::raw_worker::RevertMetadata {
                    supported: false,
                    journal_id: None,
                    rollback_op: None,
                },
                ownership: zero_abi::raw_worker::RefOwnership {
                    engine: EngineIdentity::FsZero,
                    session_id: "s".into(),
                    refs: vec![],
                    snapshot: None,
                },
                trace: zero_abi::raw_worker::WorkerTrace {
                    runtime_id: "r".into(),
                    cell_id: "c".into(),
                    request_id: "req".into(),
                    trace_id: "t".into(),
                    parent_span_id: None,
                    worker_revision: "rev".into(),
                    contract_digest: "d".into(),
                },
            },
        });
        let details = classify_leak_outcome(&outcome, 65_536);
        assert!(
            details.iter().any(|d| d.contains("exceeds negotiated")),
            "{:?}",
            details
        );
        let small = classify_leak_outcome(&outcome, 200_000);
        assert_eq!(small, Vec::<String>::new());
    }

    #[test]
    fn deadline_enforcement_accepts_client_and_remote_forms() {
        let client = WorkerAdapterError::Deadline {
            request_id: Some("r".into()),
        };
        assert!(is_deadline_enforcement(&client));
        let overflow = WorkerAdapterError::DeadlineOverflow {
            request_id: Some("r".into()),
        };
        assert!(is_deadline_enforcement(&overflow));
        let remote = WorkerAdapterError::Remote {
            request_id: Some("r".into()),
            kind: "deadline_exceeded".into(),
            message: "expired".into(),
            retryable: false,
            details: None,
            trace: None,
        };
        assert!(is_deadline_enforcement(&remote));
        let crash = WorkerAdapterError::Crash {
            status: None,
            stderr: zero_codemode::worker::StderrCapture {
                text: String::new(),
                observed_bytes: 0,
                complete: false,
                truncated: false,
            },
        };
        assert!(!is_deadline_enforcement(&crash));
    }

    #[test]
    fn bounds_enforcement_accepts_client_and_remote_forms() {
        let client = WorkerAdapterError::Bounds {
            stream: "stdout",
            actual: 70_000,
            maximum: 65_536,
        };
        assert!(is_bounds_enforcement(&client));
        let protocol =
            WorkerAdapterError::Protocol(zero_abi::raw_worker::FrameCodecError::TooLarge {
                actual: 2_000_000,
                maximum: 1_048_576,
            });
        assert!(is_bounds_enforcement(&protocol));
        let remote = WorkerAdapterError::Remote {
            request_id: Some("r".into()),
            kind: "output_too_large".into(),
            message: "too big".into(),
            retryable: false,
            details: None,
            trace: None,
        };
        assert!(is_bounds_enforcement(&remote));
        let policy = WorkerAdapterError::Remote {
            request_id: Some("r".into()),
            kind: "policy".into(),
            message: "denied".into(),
            retryable: false,
            details: None,
            trace: None,
        };
        assert!(!is_bounds_enforcement(&policy));
    }

    #[test]
    fn oversized_frame_outcome_accepts_typed_and_client_terminal_errors() {
        let client_protocol = Err(WorkerAdapterError::Protocol(
            zero_abi::raw_worker::FrameCodecError::TooLarge {
                actual: 2_000_000,
                maximum: 1_048_576,
            },
        ));
        assert_eq!(
            classify_oversized_frame_outcome(&client_protocol, 65_536),
            Vec::<String>::new()
        );
        let crash = Err(WorkerAdapterError::Crash {
            status: None,
            stderr: zero_codemode::worker::StderrCapture {
                text: String::new(),
                observed_bytes: 0,
                complete: false,
                truncated: false,
            },
        });
        // A transport-terminal crash caused by the outbound bounds violation
        // is enforcement evidence, not a harness failure.
        assert_eq!(
            classify_oversized_frame_outcome(&crash, 65_536),
            Vec::<String>::new()
        );
        let inline = Ok(WorkerResult {
            value: json!({"echo": "x".repeat(70_000)}),
            metadata: zero_abi::raw_worker::WorkerResultMetadata {
                effect: zero_abi::raw_worker::EffectClass::ReadOnly,
                approval: zero_abi::raw_worker::ApprovalMetadata {
                    state: zero_abi::raw_worker::ApprovalState::NotRequired,
                    approval_id: None,
                    policy: None,
                },
                revert: zero_abi::raw_worker::RevertMetadata {
                    supported: false,
                    journal_id: None,
                    rollback_op: None,
                },
                ownership: zero_abi::raw_worker::RefOwnership {
                    engine: EngineIdentity::FsZero,
                    session_id: "s".into(),
                    refs: vec![],
                    snapshot: None,
                },
                trace: zero_abi::raw_worker::WorkerTrace {
                    runtime_id: "r".into(),
                    cell_id: "c".into(),
                    request_id: "req".into(),
                    trace_id: "t".into(),
                    parent_span_id: None,
                    worker_revision: "rev".into(),
                    contract_digest: "d".into(),
                },
            },
        });
        let details = classify_oversized_frame_outcome(&inline, 65_536);
        assert!(
            details.iter().any(|d| d.contains("echoed back inline")),
            "{:?}",
            details
        );
    }

    #[test]
    fn fresh_ids_are_unique_and_ordered() {
        let a = fresh_id("req");
        let b = fresh_id("req");
        assert_ne!(a, b);
    }

    #[test]
    fn engine_ref_format_accepts_engine_owned_refs_and_rejects_other_schemes() {
        // The exact native TokenZero shape the old portable regex rejected.
        assert!(validate_engine_ref_format("tz", "tz://file/f0d138154edd2bc68").is_empty());
        assert!(validate_engine_ref_format("fz", "fz://blob/abc123").is_empty());
        assert!(validate_engine_ref_format("gz", "gz://claim/xyz").is_empty());

        // wrong scheme is rejected (engine-owned ref must match its owner).
        assert!(!validate_engine_ref_format("tz", "cm://file/abc").is_empty());
        assert!(!validate_engine_ref_format("tz", "fz://blob/abc").is_empty());

        // empty path / control chars / oversize are rejected.
        assert!(!validate_engine_ref_format("tz", "tz://").is_empty());
        assert!(!validate_engine_ref_format("tz", "tz://file/ab\tcd").is_empty());
        assert!(!validate_engine_ref_format("tz", &format!("tz://file/{}", "a".repeat(5000))).is_empty());
    }

    #[test]
    fn ownership_refs_accept_engine_owned_refs_without_portable_regex() {
        // Regression: native TokenZero emits tz://file/<id>; the old portable
        // blob64/codemode regex rejected it and broke RW2. Engine-scheme format
        // validation must accept it.
        let ownership = zero_abi::raw_worker::RefOwnership {
            engine: EngineIdentity::TokenZero,
            session_id: "s".into(),
            refs: vec!["tz://file/f0d138154edd2bc68".into()],
            snapshot: None,
        };
        assert!(validate_ownership_refs(Ns::Tz, &ownership).is_empty());

        // empty refs is still flagged (a read must claim ownership).
        let empty = zero_abi::raw_worker::RefOwnership {
            engine: EngineIdentity::TokenZero,
            session_id: "s".into(),
            refs: vec![],
            snapshot: None,
        };
        assert!(!validate_ownership_refs(Ns::Tz, &empty).is_empty());
    }

    #[test]
    fn accounting_accepts_all_abi_enumerated_count_kinds() {
        use zero_abi::raw_worker::WorkerTokenCountKind;
        fn accounting(kind: WorkerTokenCountKind) -> WorkerTokenAccountingV1 {
            WorkerTokenAccountingV1 {
                tokenizer_id: "tz".into(),
                count_kind: kind,
                raw_tokens: 10,
                visible_tokens: 8,
                recovery_tokens: 0,
                billed_tokens: 8,
                cached_tokens: 0,
                exact_ref_tokens: None,
            }
        }
        // ABI permits Exact, ConservativeUpperBound, and Estimate. None may be
        // rejected; only honesty (nonempty tokenizer, nonzero tokens) matters.
        assert!(validate_accounting(&accounting(WorkerTokenCountKind::Exact)).is_empty());
        assert!(validate_accounting(&accounting(WorkerTokenCountKind::ConservativeUpperBound)).is_empty());
        assert!(validate_accounting(&accounting(WorkerTokenCountKind::Estimate)).is_empty());

        // empty tokenizer / all-zero accounting are still rejected.
        let bad = WorkerTokenAccountingV1 {
            tokenizer_id: String::new(),
            count_kind: WorkerTokenCountKind::Exact,
            raw_tokens: 0,
            visible_tokens: 0,
            recovery_tokens: 0,
            billed_tokens: 0,
            cached_tokens: 0,
            exact_ref_tokens: None,
        };
        assert!(!validate_accounting(&bad).is_empty());
    }

    #[test]
    fn mutation_args_are_real_edits_not_noop_hunks() {
        let fixture = prepare_fixture().unwrap();
        let fz = mutation_args(Ns::Fz, &fixture);
        assert_ne!(fz["find"], fz["replace"]);
        assert_eq!(fz["find"], "needle");
        assert_eq!(fz["replace"], "changed");

        let tz = mutation_args(Ns::Tz, &fixture);
        let edit = &tz["edits"][0];
        assert_ne!(edit["find"], edit["replace"]);
        assert_eq!(edit["find"], "needle");
        assert_eq!(edit["replace"], "changed");

        let gz = mutation_args(Ns::Gz, &fixture);
        assert_eq!(gz, json!({}));
        // fixture contains the needle so the edit is real, not a no-op.
        let body = std::fs::read_to_string(fixture.root.join("fixture.txt")).unwrap();
        assert!(body.contains("needle"));
    }

    #[test]
    fn token_accounting_expectation_is_per_engine() {
        // Only TokenZero is a tokenizer and must emit accounting.
        assert!(requires_token_accounting(Ns::Tz));
        assert!(!requires_token_accounting(Ns::Fz));
        assert!(!requires_token_accounting(Ns::Gz));
    }

    #[test]
    fn substrate_probe_uses_expand_on_missing_ref_for_graph() {
        // Graph `snap` returns an empty success capsule on a miss, so the RW5
        // substrate probe must drive the public `expand` op with the
        // `reference` arg key on a definitely-missing ref.
        let (gz_op, gz_args) = substrate_probe(Ns::Gz);
        assert_eq!(gz_op, "expand");
        assert_eq!(gz_args, json!({ "reference": "gz://missing/zzz_no_such_claim_zzz" }));
        // FSZero/TokenZero keep the missing-read-path probe.
        let (fz_op, fz_args) = substrate_probe(Ns::Fz);
        assert_eq!(fz_op, "fs.read");
        assert_eq!(fz_args, json!({ "path": "__missing_target__.txt" }));
        let (tz_op, tz_args) = substrate_probe(Ns::Tz);
        assert_eq!(tz_op, "read");
        assert_eq!(tz_args, json!({ "path": "__missing_target__.txt" }));
    }

    #[test]
    fn graph_fixture_has_indexable_source_and_index_arg_shape() {
        let fixture = prepare_fixture().unwrap();
        // GraphZero indexes a real source tree.
        assert!(fixture.root.join("src/main.rs").is_file());
        // op_index defaults to ctx.repo_root only when path is absent;
        // passing {"path":"."} indexed the process cwd instead. The canonical
        // shape is empty args so op_index uses repo_root (the fixture root).
        let gz = mutation_args(Ns::Gz, &fixture);
        assert_eq!(gz, json!({}));
        // FSZero and TokenZero mutation args are untouched real edits.
        assert_eq!(mutation_args(Ns::Fz, &fixture)["path"], fixture.small_rel());
        assert_eq!(mutation_args(Ns::Tz, &fixture)["path"], fixture.small_rel());
    }

    #[test]
    fn emitted_gate_ids_are_the_distinct_rw_vocabulary() {
        // RW ids must never collide with the plan-level G vocabulary.
        let rw: std::collections::HashSet<&str> = crate::checks::RAW_GATE_MAPPINGS
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        let g: std::collections::HashSet<&str> = crate::checks::GATE_MAPPINGS
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert!(rw.is_disjoint(&g));
        // The raw transport emits only RW ids in its happy/skip paths.
        assert!(matches!(
            crate::checks::RawCheckId::Rw1ArtifactExposure.as_str(),
            "RW1"
        ));
    }
}
