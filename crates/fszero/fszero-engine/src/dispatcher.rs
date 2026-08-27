//! In-process typed domain dispatcher (fszero-ncib.2).
//!
//! FastMCP, CodeMode, CLI, and the private raw worker share this path for
//! canonical operations. Transport adapters serialize only at the boundary;
//! they must not call each other or re-enter via JSON-RPC.
//!
//! This module lives in `core` and MUST NOT import FastMCP, MCP JSON-RPC,
//! CodeMode sandbox, or surface packaging modules.

use super::batch_evidence::{EVIDENCE_KEY, batch_evidence};
use super::memory::{memory_put_wire, memory_rename_wire};
use super::operation_abi::{DomainError, DomainResult, Mutability, operation_by_id, resolve_alias};
use super::session::{FSZeroSession, OpCode, parse_exec_opcode};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use zero_cert::{
    CompletenessWitness, EvidenceCertificate, ObjectId, OperatorLock, Provenance, Query, Resolver,
    SpanRef,
};

static NEXT_BATCH_ID: AtomicU64 = AtomicU64::new(1);

fn next_batch_id() -> String {
    let nanos = super::unix_epoch_nanos();
    let seq = NEXT_BATCH_ID.fetch_add(1, Ordering::Relaxed);
    format!("{nanos}-{seq}")
}

#[inline]
fn inv_arg(msg: impl Into<String>) -> DomainError {
    DomainError::invalid_argument(msg)
}

/// Which external adapter invoked the dispatcher (telemetry / profiling only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchSurface {
    Cli,
    Mcp,
    CodeMode,
    RawWorker,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Serialize)]
pub enum EvidenceRoute {
    Certified,
    RawFallback,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct InlineEvidence {
    pub route: EvidenceRoute,
    pub certificate: Option<Value>,
}

impl InlineEvidence {
    pub fn raw_fallback() -> Self {
        Self {
            route: EvidenceRoute::RawFallback,
            certificate: None,
        }
    }
}

impl DispatchSurface {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Mcp => "mcp",
            Self::CodeMode => "codemode",
            Self::RawWorker => "raw_worker",
        }
    }
}

/// Full dispatcher outcome: domain result plus wire-adjacent recovery metadata.
///
/// Adapters may wrap this into MCP tool results or CodeMode `FsStep` without
/// re-running authorization, mutation, or ref minting.
#[derive(Debug, Clone, PartialEq)]
pub struct DispatchOutcome {
    pub result: DomainResult,
    /// Optional proof-carrying evidence for adapter-inline serialization.
    pub inline_evidence: Option<InlineEvidence>,
    /// Primary recovery key for the op (when one was minted/updated).
    /// Opcode/static keys use `Cow::Borrowed` (no alloc); batch ids use Owned.
    pub recovery_key: Option<Cow<'static, str>>,
    /// Kernel detail string (legacy execute third element).
    pub detail: Option<String>,
    /// CLI opcode when the op maps to one.
    pub opcode: Option<char>,
    /// Pure dispatcher bookkeeping cost (nanoseconds), excluding kernel work.
    pub dispatcher_overhead_ns: u64,
    /// End-to-end dispatch wall time including kernel (nanoseconds).
    pub wall_ns: u64,
    pub surface: DispatchSurface,
}

impl DispatchOutcome {
    /// Legacy `(ack, ok, detail)` triple used by CLI and older adapters.
    pub fn into_execute_tuple(self) -> (String, bool, Option<String>) {
        let ack = self.result.ack.clone().unwrap_or_else(|| {
            if self.result.ok {
                "ok".into()
            } else {
                "X0".into()
            }
        });
        (ack, self.result.ok, self.detail)
    }

    /// Structured invalid-argument failure (no kernel work).
    pub fn invalid(
        surface: DispatchSurface,
        op_id: &str,
        err: DomainError,
        wall_start: Instant,
    ) -> Self {
        let wall_ns = wall_start.elapsed().as_nanos() as u64;
        record_profile(surface, wall_ns, wall_ns, 0);
        Self {
            result: DomainResult::failure(op_id, err.clone()),
            inline_evidence: None,
            recovery_key: None,
            detail: Some(err.message),
            opcode: None,
            dispatcher_overhead_ns: wall_ns,
            wall_ns,
            surface,
        }
    }
}

/// Last recorded dispatcher profile sample (for benchmark subtraction).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DispatchProfile {
    pub dispatcher_overhead_ns: u64,
    pub wall_ns: u64,
    pub kernel_ns: u64,
    pub surface: u8,
}

static LAST_DISPATCH_OVERHEAD_NS: AtomicU64 = AtomicU64::new(0);
static LAST_DISPATCH_WALL_NS: AtomicU64 = AtomicU64::new(0);
static LAST_DISPATCH_KERNEL_NS: AtomicU64 = AtomicU64::new(0);
static LAST_DISPATCH_SURFACE: AtomicU64 = AtomicU64::new(0);
static DISPATCH_COUNT: AtomicU64 = AtomicU64::new(0);
// A process-global delta is polluted by unrelated parallel libtest cases.
// Kept outside `#[cfg(test)]` so dependent crates' test suites (fs-zero) can
// read per-thread dispatch deltas against the engine.
std::thread_local! {
    static TEST_THREAD_DISPATCH_COUNT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn surface_code(s: DispatchSurface) -> u64 {
    match s {
        DispatchSurface::Cli => 1,
        DispatchSurface::Mcp => 2,
        DispatchSurface::CodeMode => 3,
        DispatchSurface::RawWorker => 4,
    }
}

fn record_profile(surface: DispatchSurface, overhead_ns: u64, wall_ns: u64, kernel_ns: u64) {
    LAST_DISPATCH_OVERHEAD_NS.store(overhead_ns, Ordering::Relaxed);
    LAST_DISPATCH_WALL_NS.store(wall_ns, Ordering::Relaxed);
    LAST_DISPATCH_KERNEL_NS.store(kernel_ns, Ordering::Relaxed);
    LAST_DISPATCH_SURFACE.store(surface_code(surface), Ordering::Relaxed);
    DISPATCH_COUNT.fetch_add(1, Ordering::Relaxed);
    TEST_THREAD_DISPATCH_COUNT.with(|count| count.set(count.get().saturating_add(1)));
}

/// Profiling sample for the most recent dispatch (benchmark subtraction).
pub fn last_dispatch_profile() -> DispatchProfile {
    let surface = match LAST_DISPATCH_SURFACE.load(Ordering::Relaxed) {
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 4,
        _ => 0,
    };
    DispatchProfile {
        dispatcher_overhead_ns: LAST_DISPATCH_OVERHEAD_NS.load(Ordering::Relaxed),
        wall_ns: LAST_DISPATCH_WALL_NS.load(Ordering::Relaxed),
        kernel_ns: LAST_DISPATCH_KERNEL_NS.load(Ordering::Relaxed),
        surface,
    }
}

pub fn dispatch_count() -> u64 {
    DISPATCH_COUNT.load(Ordering::Relaxed)
}

/// Per-test dispatch counter for this thread (measured, read-only). Kept
/// outside `#[cfg(test)]` so dependent crates' test suites (fs-zero) can
/// assert dispatch-count deltas against the engine.
pub fn test_thread_dispatch_count() -> u64 {
    TEST_THREAD_DISPATCH_COUNT.with(std::cell::Cell::get)
}

/// Default recovery key for a CLI opcode after a successful kernel op.
pub fn recovery_key_for_opcode(code: char) -> Option<&'static str> {
    OpCode::from_char(code).map(OpCode::recovery_key)
}

pub fn opcode_for_operation(op_id: &str) -> Option<char> {
    operation_by_id(op_id).and_then(|op| op.cli_opcodes.first().copied())
}

/// Opcode → domain op id without scanning the full registry (hot dispatch).
pub fn operation_for_opcode(code: char) -> Option<&'static str> {
    OpCode::from_char(code).map(OpCode::operation_id)
}

fn op_mutated(op_id: &str, ok: bool) -> bool {
    if !ok {
        return false;
    }
    // Mixed treated as write (conservative: adapter may refine).
    match operation_by_id(op_id).map(|o| o.mutability) {
        Some(Mutability::Write | Mutability::Mixed) => true,
        _ => false,
    }
}

fn build_outcome(
    surface: DispatchSurface,
    operation: &str,
    ack: String,
    ok: bool,
    detail: Option<String>,
    opcode: Option<char>,
    recovery_key: Option<Cow<'static, str>>,
    wall_ns: u64,
    kernel_ns: u64,
    overhead_ns: u64,
) -> DispatchOutcome {
    record_profile(surface, overhead_ns, wall_ns, kernel_ns);
    let error = if ok {
        None
    } else {
        Some(DomainError::from_detail(
            detail.as_deref().unwrap_or("operation failed"),
        ))
    };
    // DomainResult.refs stays Vec<String> (public wire shape); one owned copy.
    let refs: Vec<String> = recovery_key
        .iter()
        .map(|k| k.as_ref().to_string())
        .collect();
    let result = if ok {
        // Prefer String leaf over object tree when adapters only need detail text.
        // Wire shape remains {"detail": "..."} for object consumers via map.
        let value = detail.as_ref().map(|d| json!({"detail": d}));
        DomainResult::success(
            operation,
            Some(ack.clone()),
            value,
            refs,
            op_mutated(operation, true),
        )
    } else {
        let mut r = DomainResult::failure(
            operation,
            error.unwrap_or_else(|| DomainError::internal("operation failed")),
        );
        r.ack = Some(ack.clone());
        r.refs = recovery_key
            .iter()
            .map(|k| k.as_ref().to_string())
            .collect();
        r
    };
    DispatchOutcome {
        result,
        inline_evidence: None,
        recovery_key,
        detail,
        opcode,
        dispatcher_overhead_ns: overhead_ns,
        wall_ns,
        surface,
    }
}

#[inline]
fn static_key(s: &'static str) -> Option<Cow<'static, str>> {
    Some(Cow::Borrowed(s))
}

#[inline]
fn owned_key(s: String) -> Option<Cow<'static, str>> {
    Some(Cow::Owned(s))
}

/// Wall+kernel time a kernel quadruple (ack, ok, detail, recovery_key).
fn timed_build_key(
    surface: DispatchSurface,
    operation: &str,
    opcode: Option<char>,
    f: impl FnOnce() -> (String, bool, Option<String>, Option<Cow<'static, str>>),
) -> DispatchOutcome {
    let wall_start = Instant::now();
    let kernel_start = Instant::now();
    let (ack, ok, detail, recovery_key) = f();
    let kernel_ns = kernel_start.elapsed().as_nanos() as u64;
    let wall_ns = wall_start.elapsed().as_nanos() as u64;
    build_outcome(
        surface,
        operation,
        ack,
        ok,
        detail,
        opcode,
        recovery_key,
        wall_ns,
        kernel_ns,
        wall_ns.saturating_sub(kernel_ns),
    )
}

/// Wall+kernel time a kernel triple with a fixed recovery key.
fn timed_build(
    surface: DispatchSurface,
    operation: &str,
    opcode: Option<char>,
    recovery_key: Option<Cow<'static, str>>,
    f: impl FnOnce() -> (String, bool, Option<String>),
) -> DispatchOutcome {
    timed_build_key(surface, operation, opcode, || {
        let (ack, ok, detail) = f();
        (ack, ok, detail, recovery_key)
    })
}

const RACC_PARSER_ID: &str = "fszero.read.parser";
const RACC_PARSER_VERSION: &str = "1";
const RACC_INDEX_ID: &str = "fszero.read.index";
const RACC_INDEX_VERSION: &str = "1";
const RACC_OPERATOR_ID: &str = "fszero.read";
const RACC_OPERATOR_VERSION: &str = "1";

struct CurrentReadResolver<'a> {
    bytes: &'a [u8],
    object_id: ObjectId,
}

impl Resolver for CurrentReadResolver<'_> {
    fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]> {
        (*object_id == self.object_id).then_some(self.bytes)
    }
    fn trusted_operator_version<'a>(&'a self, id: &str) -> Option<&'a str> {
        (id == RACC_OPERATOR_ID).then_some(RACC_OPERATOR_VERSION)
    }
    fn trusted_parser_version<'a>(&'a self, id: &str) -> Option<&'a str> {
        (id == RACC_PARSER_ID).then_some(RACC_PARSER_VERSION)
    }
    fn trusted_index_version<'a>(&'a self, id: &str) -> Option<&'a str> {
        (id == RACC_INDEX_ID).then_some(RACC_INDEX_VERSION)
    }
}

fn decode_blob_digest(reference: &str) -> Option<[u8; 32]> {
    let hex = reference.strip_prefix("fz://blob/")?;
    if hex.len() != 64 {
        return None;
    }
    let mut digest = [0u8; 32];
    for (index, pair) in hex.as_bytes().chunks_exact(2).enumerate() {
        let high = decode_hex_nibble(pair[0])?;
        let low = decode_hex_nibble(pair[1])?;
        digest[index] = (high << 4) | low;
    }
    Some(digest)
}

fn decode_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn read_inline_evidence(session: &FSZeroSession, arg: Option<&str>) -> InlineEvidence {
    let parsed = super::read_ops::parse_read_arg(arg.unwrap_or("src/main.rs"));
    if !matches!(parsed, Ok((_, None))) {
        return InlineEvidence::raw_fallback();
    }
    let Some((bytes, content_ref)) = session.last_stable_complete_read() else {
        return InlineEvidence::raw_fallback();
    };
    let Some(digest) = decode_blob_digest(content_ref.as_ref()) else {
        return InlineEvidence::raw_fallback();
    };
    let Ok(byte_len) = u64::try_from(bytes.len()) else {
        return InlineEvidence::raw_fallback();
    };
    let object_id = ObjectId(digest);
    let span = SpanRef {
        object_id,
        byte_start: 0,
        byte_len,
        object_digest: digest,
        span_digest: digest,
    };
    let provenance = Provenance {
        parser_id: RACC_PARSER_ID.into(),
        parser_version: RACC_PARSER_VERSION.into(),
        index_id: RACC_INDEX_ID.into(),
        index_version: RACC_INDEX_VERSION.into(),
        operator_id: RACC_OPERATOR_ID.into(),
        operator_version: RACC_OPERATOR_VERSION.into(),
    };
    let certificate = EvidenceCertificate {
        query: Query::ReadSpan(span.clone()),
        spans: vec![span],
        payload: Cow::Owned(bytes.as_ref().clone()),
        provenance,
        completeness: CompletenessWitness::ReadSpan {
            operator: OperatorLock {
                operator_id: RACC_OPERATOR_ID.into(),
                operator_version: RACC_OPERATOR_VERSION.into(),
            },
        },
        input_token_cost: 0,
        backend_work_units: 1,
    };
    let resolver = CurrentReadResolver {
        bytes: bytes.as_slice(),
        object_id,
    };
    if zero_cert::verify(&certificate, &resolver).is_err() {
        return InlineEvidence::raw_fallback();
    }
    // FSZero emits a zero-cert-verified candidate certificate; policy and
    // native-durability promotion are decided by the hub alone (zero-gate).
    // The verified certificate is reported as Certified inline evidence.
    match serde_json::to_value(certificate) {
        Ok(certificate) => InlineEvidence {
            route: EvidenceRoute::Certified,
            certificate: Some(certificate),
        },
        Err(_) => InlineEvidence::raw_fallback(),
    }
}

/// CLI / opcode path — shared by `FSZeroSession::execute` and raw workers.
///
/// Shared policy that must not differ by surface:
/// - one bounded retry on retryable search/grep faults (was CodeMode-only)
/// - recovery key selection for compound/memory
pub fn dispatch_opcode(
    session: &mut FSZeroSession,
    surface: DispatchSurface,
    code: char,
    arg: Option<&str>,
) -> DispatchOutcome {
    let wall_start = Instant::now();
    let pre = Instant::now();
    let operation = operation_for_opcode(code).unwrap_or("unknown");
    let overhead_pre = pre.elapsed().as_nanos() as u64;

    let kernel_start = Instant::now();
    let (mut ack, mut ok, mut detail) = session.execute_kernel(code, arg);
    // Domain-shared search retry (fszero-szw): one bounded retry on retryable
    // substrate/store faults — identical for FastMCP, CodeMode, and raw worker.
    if code == 'S' && !ok {
        if let Some(msg) = detail.as_deref() {
            if DomainError::from_detail(msg).retryable {
                let (a2, o2, d2) = session.execute_kernel(code, arg);
                ack = a2;
                ok = o2;
                detail = d2;
            }
        }
    }
    let kernel_ns = kernel_start.elapsed().as_nanos() as u64;

    let post = Instant::now();
    let recovery_key = recovery_key_for_dispatch(code, arg, ok);
    let overhead_ns = overhead_pre + post.elapsed().as_nanos() as u64;
    let wall_ns = wall_start.elapsed().as_nanos() as u64;
    let mut outcome = build_outcome(
        surface,
        operation,
        ack,
        ok,
        detail,
        Some(code),
        recovery_key,
        wall_ns,
        kernel_ns,
        overhead_ns,
    );
    if code == 'R' && ok {
        outcome.inline_evidence = Some(read_inline_evidence(session, arg));
    }
    outcome
}

fn recovery_key_for_dispatch(code: char, arg: Option<&str>, ok: bool) -> Option<Cow<'static, str>> {
    match (code, ok) {
        ('C', true) => static_key("compound"),
        ('C', false) => static_key("compound:err"),
        ('M', _) => static_key(if arg.is_some_and(|a| a.starts_with("ls:")) {
            "memory/ls"
        } else {
            "memory"
        }),
        _ => recovery_key_for_opcode(code).map(Cow::Borrowed),
    }
}

/// Structured edit without path:old|new grammar.
pub fn dispatch_edit_parts(
    session: &mut FSZeroSession,
    surface: DispatchSurface,
    path: &str,
    find: &str,
    replace: &str,
) -> DispatchOutcome {
    timed_build(
        surface,
        "fs.edit",
        Some('E'),
        static_key("last_cert"),
        || session.execute_edit_parts_kernel(path, find, replace),
    )
}

/// Structured edit constrained to one canonical discovery target window.
pub fn dispatch_edit_parts_window(
    session: &mut FSZeroSession,
    surface: DispatchSurface,
    path: &str,
    find: &str,
    replace: &str,
    window: super::target_ref::LineWindow,
) -> DispatchOutcome {
    timed_build(
        surface,
        "fs.edit",
        Some('E'),
        static_key("last_cert"),
        || session.execute_edit_parts_window_kernel(path, find, replace, window),
    )
}

/// Fused search selection plus target-window mutation as one fs.edit dispatch.
pub fn dispatch_snap_edit(
    session: &mut FSZeroSession,
    surface: DispatchSurface,
    query: &str,
    scope: &str,
    preimage: &str,
    replacement: &str,
) -> DispatchOutcome {
    timed_build(
        surface,
        "fs.edit",
        Some('E'),
        static_key("last_cert"),
        || session.execute_snap_edit_kernel(query, scope, preimage, replacement),
    )
}

/// Primary JSON field(s) for simple single-arg ops (first hit wins).
fn wire_primary_fields(
    op_id: &str,
) -> Option<(char, &'static [&'static str], Option<&'static str>)> {
    // (opcode, field candidates, required-field name for error text)
    match op_id {
        "fs.ls" => Some(('L', &["arg", "path"], None)),
        "fs.read" => Some(('R', &["path", "arg"], Some("path"))),
        "fs.search" => Some(('S', &["query", "arg"], Some("query"))),
        "fs.compound" => Some(('C', &["intent", "arg"], Some("intent"))),
        "fs.expand" => Some(('X', &["ref", "arg"], Some("ref"))),
        "fs.stat" => Some(('T', &["path", "arg"], Some("path"))),
        "fs.history" => Some(('H', &["arg", "path"], None)),
        "fs.undo" => Some(('U', &["arg", "path"], Some("path"))),
        _ => None,
    }
}

pub fn structured_world_arg(args: &Value) -> Result<Option<String>, DomainError> {
    let Some(map) = args.as_object() else {
        return Ok(None);
    };
    let get = |key: &str| map.get(key).and_then(Value::as_str);
    let Some(action) = get("action") else {
        return Ok(None);
    };
    let required = |keys: &[&str], label: &str| {
        keys.iter().find_map(|key| get(key)).ok_or_else(|| {
            DomainError::typed(
                "invalid_argument",
                format!("fs.world action {action} requires {label}"),
            )
        })
    };
    let edit_spec = || -> Result<String, DomainError> {
        let path = required(&["path"], "path")?;
        let find = required(&["find", "old"], "find/old")?;
        let replace = required(&["replace", "new"], "replace/new")?;
        let escaped_find = find.replace('\\', "\\\\").replace('|', "\\|");
        Ok(format!("{path}:{escaped_find}|{replace}"))
    };
    let world_id = || required(&["world", "wid", "id"], "world/wid/id");
    let spec = match action {
        "fork" => "fork".to_string(),
        "new" => format!("new:{}", edit_spec()?),
        "edit" => format!("edit:{}:{}", world_id()?, edit_spec()?),
        "commit" => format!(
            "commit:{}{}",
            world_id()?,
            if map.get("git").and_then(Value::as_bool) == Some(true) {
                ":git"
            } else {
                ""
            }
        ),
        "drop" | "preview" | "rebase" | "conflicts" => {
            format!("{action}:{}", world_id()?)
        }
        other => {
            return Err(DomainError::typed(
                "invalid_argument",
                format!(
                    "unknown fs.world action {other}; expected fork, new, edit, commit, drop, preview, rebase, or conflicts"
                ),
            ));
        }
    };
    Ok(Some(spec))
}

/// Map structured domain args to a CLI opcode wire argument.
pub fn wire_arg_for_operation(
    op_id: &str,
    args: &Value,
) -> Result<(char, Option<String>), DomainError> {
    let map = args.as_object();
    let get = |k: &str| map.and_then(|m| m.get(k)).and_then(Value::as_str);
    if let Some((code, fields, required)) = wire_primary_fields(op_id) {
        let mut val = None;
        for f in fields {
            if let Some(v) = get(f) {
                val = Some(v.to_string());
                break;
            }
        }
        if val.is_none() {
            if let Some(name) = required {
                return Err(inv_arg(format!("missing {name}")));
            }
        }
        return Ok((code, val));
    }
    match op_id {
        "fs.edit" => {
            if let Some(spec) = get("spec").or_else(|| get("arg")) {
                return Ok(('E', Some(spec.to_string())));
            }
            // Object form handled by dispatch_operation separately.
            Err(inv_arg(
                "fs.edit requires spec or {path,find/old,replace/new}",
            ))
        }
        "fs.write" => {
            let path = get("path").ok_or_else(|| inv_arg("missing path"))?;
            let content = get("content").unwrap_or("");
            if let Some(arg) = get("arg") {
                return Ok(('P', Some(arg.to_string())));
            }
            Ok(('P', Some(format!("{path}|{content}"))))
        }
        "fs.world" => {
            if let Some(q) = get("query") {
                return Err(inv_arg(format!("use dispatch_world_query for query={q}")));
            }
            if let Some(arg) = get("arg") {
                return Ok(('W', Some(arg.to_string())));
            }
            let arg = structured_world_arg(args)?
                .ok_or_else(|| inv_arg("fs.world requires arg, query, or a typed action"))?;
            Ok(('W', Some(arg)))
        }
        "fs.resolve" => {
            let intent = get("intent").ok_or_else(|| inv_arg("missing intent"))?;
            // Kernel resolve accepts free-form intent; engine/limit are advisory
            // for semantic resolve and passed as JSON-ish only when present.
            if map.is_some_and(|m| m.contains_key("engine") || m.contains_key("limit")) {
                let mut payload = Map::new();
                payload.insert("intent".into(), json!(intent));
                if let Some(e) = get("engine") {
                    payload.insert("engine".into(), json!(e));
                }
                if let Some(lim) = map.and_then(|m| m.get("limit")) {
                    payload.insert("limit".into(), lim.clone());
                }
                return Ok((
                    'V',
                    Some(
                        serde_json::to_string(&Value::Object(payload))
                            .unwrap_or_else(|_| intent.to_string()),
                    ),
                ));
            }
            Ok(('V', Some(intent.to_string())))
        }
        "fs.memory" => Ok(('M', Some(memory_args_to_wire(args)?))),
        // Multi-item / session-level ops use dedicated dispatch_* helpers, not a
        // single CLI opcode. Callers of wire_arg_for_operation for these get a
        // structured marker so dispatch_operation can route them.
        "fs.multiRead" | "fs.multiSearch" | "fs.multiStat" | "fs.multiList"
        | "fs.multiAstSearch" | "fs.transact" | "doctor" | "migrate-cas" => Err(inv_arg(format!(
            "use dispatch_operation for multi/session op {op_id}"
        ))),
        other => Err(inv_arg(format!("unknown operation {other}"))),
    }
}

/// Whether `op_id` is a first-class registry op that has a typed dispatcher path.
/// Every registry op is dispatchable; table kept in OPERATION_REGISTRY only.
pub fn operation_is_dispatchable(op_id: &str) -> bool {
    operation_by_id(op_id).is_some()
}

/// Batch field + per-item opcode for multi-item ops.
fn batch_spec(op_id: &str) -> Option<(&'static str, char, &'static str)> {
    match op_id {
        "fs.multiRead" => Some(("paths", 'R', "fs.read")),
        "fs.multiSearch" => Some(("queries", 'S', "fs.search")),
        "fs.multiStat" => Some(("paths", 'T', "fs.stat")),
        "fs.multiList" => Some(("items", 'L', "fs.ls")),
        "fs.multiAstSearch" => Some(("items", 'S', "fs.search")),
        _ => None,
    }
}

/// Domain-owned multi-item dispatch (formerly CodeMode `batch_strings`).
///
/// Executes one fused kernel batch, preserving ordered per-item outcomes,
/// and publishes the batch JSON payload under
/// `codemode/batch/{id}` (stable key family for existing expand clients).
pub fn dispatch_batch(
    session: &mut FSZeroSession,
    surface: DispatchSurface,
    op_id: &str,
    args: &Value,
) -> DispatchOutcome {
    let wall_start = Instant::now();
    let Some((field, _code, item_op)) = batch_spec(op_id) else {
        return DispatchOutcome::invalid(
            surface,
            op_id,
            inv_arg(format!("not a batch op: {op_id}")),
            wall_start,
        );
    };
    let Some(values) = args.get(field).and_then(Value::as_array) else {
        return DispatchOutcome::invalid(
            surface,
            op_id,
            inv_arg(format!("missing required arg: {field}")),
            wall_start,
        );
    };

    let batch_id = next_batch_id();
    let kernel_start = Instant::now();
    let batch = session.execute_batch_kernel(op_id, values, args);
    let kernel_ns = kernel_start.elapsed().as_nanos() as u64;
    let mut rows = Vec::with_capacity(batch.rows.len());
    for (idx, item) in batch.rows.into_iter().enumerate() {
        // Row aliases are resolved from the durable batch envelope on demand.
        // Persisting every alias duplicated the payload and made multiRead O(n)
        // SQLite commits on the warm path.
        let payload_ref = format!("codemode/batch/{batch_id}/{idx}");
        let mut row = item.fields;
        row.insert("ok".into(), json!(item.ok));
        row.insert("ack".into(), json!(item.ack));
        row.insert("ref".into(), json!(payload_ref));
        row.insert("operation".into(), json!(item_op));
        row.insert("payload_len".into(), json!(item.payload.len()));
        if let Some(source_ref) = item.source_ref {
            row.insert("source_ref".into(), json!(source_ref));
        }
        if let Some(detail) = item.detail {
            row.insert("detail".into(), json!(detail));
        }
        if let Some(error) = item.error {
            row.insert("error".into(), error.to_json());
        }
        rows.push(Value::Object(row));
    }
    let payload = serde_json::to_vec(&rows)
        .unwrap_or_else(|e| format!("serialization failed: {e}").into_bytes());
    let batch_ref = format!("codemode/batch/{batch_id}");
    session.recovery.put_key(&batch_ref, &payload);
    session.recovery.put_key("codemode/batch", &payload);
    let wall_ns = wall_start.elapsed().as_nanos() as u64;
    let overhead_ns = wall_ns.saturating_sub(kernel_ns);
    let detail = format!(
        "batch {op_id} count={} physical_passes={} unique_inputs={} visited_files={}",
        values.len(),
        batch.physical_passes,
        batch.unique_inputs,
        batch.visited_files
    );
    let mut outcome = build_outcome(
        surface,
        op_id,
        "B".into(),
        true,
        Some(detail),
        None,
        owned_key(batch_ref),
        wall_ns,
        kernel_ns,
        overhead_ns,
    );
    let mut value = json!({
        "count": values.len(), "physical_passes": batch.physical_passes,
        "unique_inputs": batch.unique_inputs, "visited_files": batch.visited_files,
        "exec_shape": batch.exec_shape,
    });
    value[EVIDENCE_KEY] = batch_evidence(op_id, args, &rows, payload.len(), kernel_ns / 1_000);
    outcome.result.value = Some(value);
    outcome
}

/// Session doctor report as a domain operation (no CLI process spawn).
pub fn dispatch_doctor(
    session: &mut FSZeroSession,
    surface: DispatchSurface,
    args: &Value,
) -> DispatchOutcome {
    timed_build(surface, "doctor", None, static_key("doctor"), || {
        let mut report = session.root_report();
        // Optional lightweight smoke: list root via opcode when requested.
        if args.get("smoke").and_then(Value::as_bool) == Some(true) {
            let smoke = dispatch_opcode(session, surface, 'L', Some("."));
            report["smoke"] =
                json!({ "ok": smoke.result.ok, "ack": smoke.result.ack, "detail": smoke.detail, });
        }
        let body = report.to_string();
        session.recovery.put_key("doctor", body.as_bytes());
        ("D".into(), true, Some(body))
    })
}

/// Physical residency probe as a domain operation (`fs.residency`).
///
/// `args` carries `{"refs": ["fz://blob/<sha256>", "<recovery-key>", ...]}`.
/// The report (counts + byte totals, everything measured) is returned as the
/// op value and parked under the `residency` recovery key (V6-F6 /
/// ZS-BENCH-011, ZS-BENCH-012).
pub fn dispatch_residency(
    session: &mut FSZeroSession,
    surface: DispatchSurface,
    args: &Value,
) -> DispatchOutcome {
    timed_build(
        surface,
        "fs.residency",
        None,
        static_key("residency"),
        || {
            let refs: Vec<String> = args
                .get("refs")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let report = super::residency_probe::probe_residency(session, &refs);
            let body = report.to_string();
            session.recovery.put_key("residency", body.as_bytes());
            ("C".into(), true, Some(body))
        },
    )
}

/// Legacy → canonical CAS migration as a domain operation.
pub fn dispatch_migrate_cas(
    session: &mut FSZeroSession,
    surface: DispatchSurface,
    _args: &Value,
) -> DispatchOutcome {
    timed_build_key(surface, "migrate-cas", None, || {
        match session.migrate_blobs_to_cas() {
            Ok(report) => {
                let text = report.counts_json().to_string();
                session.recovery.put_key("migrate-cas", text.as_bytes());
                ("M1".into(), true, Some(text), static_key("migrate-cas"))
            }
            // Failures never advertise a recovery key (nothing was parked).
            Err(e) => ("X0".into(), false, Some(e), None),
        }
    })
}

/// Typed domain dispatch by canonical operation id + JSON args.
pub fn dispatch_operation(
    session: &mut FSZeroSession,
    surface: DispatchSurface,
    op_id: &str,
    args: &Value,
) -> DispatchOutcome {
    let wall_start = Instant::now();
    // Multi-item and session-level ops — full domain ownership (fszero-ncib.2).
    if batch_spec(op_id).is_some() {
        return dispatch_batch(session, surface, op_id, args);
    }
    if op_id == "doctor" {
        return dispatch_doctor(session, surface, args);
    }
    if op_id == "fs.residency" {
        return dispatch_residency(session, surface, args);
    }
    // All-or-nothing multi-step mutation: gate every step, apply in order,
    // roll back applied steps through the journaled undo path on any failure.
    if op_id == "fs.transact" {
        let Some(steps) = args.get("steps").and_then(Value::as_array) else {
            return DispatchOutcome::invalid(
                surface,
                op_id,
                inv_arg("missing required arg: steps"),
                wall_start,
            );
        };
        let steps = steps.clone();
        return timed_build(surface, "fs.transact", None, static_key("transact"), || {
            session.execute_transact_kernel(&steps)
        });
    }
    if op_id == "migrate-cas" {
        return dispatch_migrate_cas(session, surface, args);
    }
    // CAS base gate: `base: null` (must-not-exist create) or
    // `base: "fz://blob/<sha256>"` (content compare-and-swap). Absent base
    // keeps the historical unconditional paths below.
    let base_gate = match parse_base_gate(args) {
        Ok(gate) => gate,
        Err(err) => return DispatchOutcome::invalid(surface, op_id, err, wall_start),
    };
    if op_id == "fs.write" {
        if let Some(gate) = base_gate.as_ref() {
            let map = args.as_object();
            let Some(path) = map
                .and_then(|m| m.get("path"))
                .and_then(Value::as_str)
                .map(str::to_owned)
            else {
                return DispatchOutcome::invalid(
                    surface,
                    op_id,
                    inv_arg("missing path"),
                    wall_start,
                );
            };
            let content = map
                .and_then(|m| m.get("content"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            return timed_build(
                surface,
                "fs.write",
                Some('P'),
                recovery_key_for_opcode('P').map(Cow::Borrowed),
                || session.execute_write_cas_kernel(&path, &content, gate),
            );
        }
    }
    // Structured edit object form.
    if op_id == "fs.edit" {
        if let Some(gate) = base_gate.as_ref() {
            if matches!(gate, super::fs_ops::WriteBaseGate::MustNotExist) {
                return DispatchOutcome::invalid(
                    surface,
                    op_id,
                    inv_arg("fs.edit base: null is not meaningful; edits require an existing file"),
                    wall_start,
                );
            }
            let Some(path) = args.get("path").and_then(Value::as_str).map(str::to_owned) else {
                return DispatchOutcome::invalid(
                    surface,
                    op_id,
                    inv_arg("fs.edit with base requires path"),
                    wall_start,
                );
            };
            if let Some(detail) = session.base_gate_violation(&path, gate) {
                return timed_build(surface, "fs.edit", Some('E'), None, || {
                    ("E0".to_string(), false, Some(format!("edit:0 ({detail})")))
                });
            }
        }
        let map = args.as_object();
        if let Some(query) = map.and_then(|m| m.get("query")).and_then(Value::as_str) {
            let scope = map.and_then(|m| m.get("scope")).and_then(Value::as_str);
            let preimage = map.and_then(|m| m.get("preimage")).and_then(Value::as_str);
            let replacement = map
                .and_then(|m| m.get("replacement"))
                .and_then(Value::as_str);
            let unique = map.and_then(|m| m.get("uniqueness")).is_some_and(|value| {
                value
                    .as_str()
                    .is_some_and(|value| matches!(value, "exactly_one" | "unique" | "1"))
                    || value.as_bool() == Some(true)
                    || value.as_u64() == Some(1)
            });
            if !unique {
                return DispatchOutcome::invalid(
                    surface,
                    op_id,
                    inv_arg(
                        "fused fs.edit requires uniqueness=exactly_one (also accepts true or 1)",
                    ),
                    wall_start,
                );
            }
            let (Some(scope), Some(preimage), Some(replacement)) = (scope, preimage, replacement)
            else {
                return DispatchOutcome::invalid(
                    surface,
                    op_id,
                    inv_arg(
                        "fused fs.edit requires query, scope, uniqueness, preimage, and replacement",
                    ),
                    wall_start,
                );
            };
            return dispatch_snap_edit(session, surface, query, scope, preimage, replacement);
        }
        if map
            .and_then(|m| m.get("spec"))
            .and_then(Value::as_str)
            .is_none()
            && map
                .and_then(|m| m.get("arg"))
                .and_then(Value::as_str)
                .is_none()
        {
            let path = map.and_then(|m| m.get("path")).and_then(Value::as_str);
            let find = map
                .and_then(|m| m.get("find").or_else(|| m.get("old")))
                .and_then(Value::as_str);
            let replace = map
                .and_then(|m| m.get("replace").or_else(|| m.get("new")))
                .and_then(Value::as_str);
            if let (Some(p), Some(f), Some(r)) = (path, find, replace) {
                let start = map
                    .and_then(|m| m.get("start_line").or_else(|| m.get("startLine")))
                    .and_then(Value::as_u64);
                let end = map
                    .and_then(|m| m.get("end_line").or_else(|| m.get("endLine")))
                    .and_then(Value::as_u64);
                match (start, end) {
                    (None, None) => return dispatch_edit_parts(session, surface, p, f, r),
                    (Some(start), Some(end))
                        if start > 0 && end >= start && end <= usize::MAX as u64 =>
                    {
                        let window = super::target_ref::LineWindow {
                            start: start as usize,
                            end: end as usize,
                        };
                        return dispatch_edit_parts_window(session, surface, p, f, r, window);
                    }
                    _ => {
                        return DispatchOutcome::invalid(
                            surface,
                            op_id,
                            inv_arg(
                                "fs.edit start_line/end_line must both be positive integers in ascending order",
                            ),
                            wall_start,
                        );
                    }
                }
            }
        }
    }
    // World access-ledger query.
    if op_id == "fs.world" {
        if let Some(q) = args.get("query").and_then(Value::as_str) {
            if super::access_world_ops::parse_world_access_query(q).is_some() {
                return dispatch_world_query(session, surface, args);
            }
        }
    }

    match wire_arg_for_operation(op_id, args) {
        Ok((code, arg)) => {
            let owned = arg;
            dispatch_opcode(session, surface, code, owned.as_deref())
        }
        Err(err) => {
            let wall_ns = wall_start.elapsed().as_nanos() as u64;
            record_profile(surface, wall_ns, wall_ns, 0);
            DispatchOutcome {
                result: DomainResult::failure(op_id, err.clone()),
                inline_evidence: None,
                recovery_key: None,
                detail: Some(err.message),
                opcode: opcode_for_operation(op_id),
                dispatcher_overhead_ns: wall_ns,
                wall_ns,
                surface,
            }
        }
    }
}

/// Access-ledger world query (hot/recent/coaccess).
pub fn dispatch_world_query(
    session: &mut FSZeroSession,
    surface: DispatchSurface,
    args: &Value,
) -> DispatchOutcome {
    timed_build(
        surface,
        "fs.world",
        Some('W'),
        static_key("world/access"),
        || {
            let detail = session.do_world_access_query(args);
            let ok = detail.starts_with("world:1");
            (if ok { "W1".into() } else { "X0".into() }, ok, Some(detail))
        },
    )
}

/// Private raw worker entry: typed op id + args, no transport framing.
pub fn dispatch_raw_worker(
    session: &mut FSZeroSession,
    op_id: &str,
    args: &Value,
) -> DispatchOutcome {
    dispatch_operation(session, DispatchSurface::RawWorker, op_id, args)
}

/// Resolve an MCP tool name + args into a domain dispatch (no MCP framing).
pub fn dispatch_mcp_tool(
    session: &mut FSZeroSession,
    name: &str,
    args: &Value,
) -> Result<DispatchOutcome, DomainError> {
    if name == "fszero.world_query" {
        return Ok(dispatch_world_query(session, DispatchSurface::Mcp, args));
    }
    if let Some(spec) = memory_tool_wire(name, args)? {
        return Ok(dispatch_opcode(
            session,
            DispatchSurface::Mcp,
            'M',
            Some(&spec),
        ));
    }
    if name == "fszero.exec" {
        let raw = args
            .get("code")
            .and_then(Value::as_str)
            .ok_or_else(|| inv_arg("fszero.exec requires code"))?;
        let code = parse_exec_opcode(raw).map_err(inv_arg)?;
        let arg = args.get("arg").and_then(Value::as_str);
        return Ok(dispatch_opcode(session, DispatchSurface::Mcp, code, arg));
    }
    let op_id = resolve_alias("mcp", name).ok_or_else(|| {
        inv_arg(format!(
            "unknown mcp tool: {name}{}",
            suggest_aliases("mcp", name)
        ))
    })?;
    // Prefer structured fields; fall back to arg/path (includes fszero.resolve).
    Ok(dispatch_operation(
        session,
        DispatchSurface::Mcp,
        op_id,
        args,
    ))
}

fn memory_tool_wire(name: &str, args: &Value) -> Result<Option<String>, DomainError> {
    let op = match name {
        "fszero.memory_put" => "put",
        "fszero.memory_get" => "get",
        "fszero.memory_ls" => "ls",
        "fszero.memory_delete" => "delete",
        "fszero.memory_rename" => "rename",
        _ => return Ok(None),
    };
    let mut m = args.as_object().cloned().unwrap_or_default();
    m.insert("op".into(), json!(op));
    Ok(Some(memory_args_to_wire(&Value::Object(m))?))
}

/// Build kernel memory wire string from structured args (explicit `op` or inferred).
fn memory_args_to_wire(args: &Value) -> Result<String, DomainError> {
    let get = |k: &str| args.get(k).and_then(Value::as_str);
    let req =
        |k: &str, ctx: &str| get(k).ok_or_else(|| inv_arg(format!("memory {ctx} missing {k}")));
    if let Some(op) = get("op") {
        return match op {
            "put" => Ok(memory_put_wire(
                req("path", "put")?,
                get("content").unwrap_or(""),
            )),
            "get" => Ok(format!("get:{}", req("path", "get")?)),
            "ls" => Ok(format!("ls:{}", get("prefix").unwrap_or(""))),
            "delete" => Ok(format!("delete:{}", req("path", "delete")?)),
            "rename" => Ok(memory_rename_wire(
                req("from", "rename")?,
                req("to", "rename")?,
            )),
            other => Err(inv_arg(format!("unknown memory op {other}"))),
        };
    }
    if get("content").is_some() {
        return Ok(memory_put_wire(
            req("path", "put")?,
            get("content").unwrap_or(""),
        ));
    }
    if get("from").is_some() {
        return Ok(memory_rename_wire(
            get("from").unwrap(),
            req("to", "rename")?,
        ));
    }
    if get("path").is_some() && args.get("delete").is_some() {
        return Ok(format!("delete:{}", get("path").unwrap()));
    }
    if get("prefix").is_some() || get("path").is_none() {
        return Ok(format!("ls:{}", get("prefix").unwrap_or("")));
    }
    Ok(format!("get:{}", get("path").unwrap()))
}

/// CodeMode method path → domain dispatch (no sandbox / plan runtime).
pub fn dispatch_codemode_method(
    session: &mut FSZeroSession,
    method: &str,
    args: &Value,
) -> Result<DispatchOutcome, DomainError> {
    // Memory sub-methods map to fs.memory with op field.
    let (op_id, args_owned): (&str, Value) = if let Some(rest) = method.strip_prefix("fs.memory.") {
        let mut m = args.as_object().cloned().unwrap_or_default();
        m.insert("op".into(), json!(rest));
        ("fs.memory", Value::Object(m))
    } else {
        let op = resolve_alias("codemode", method)
            .or_else(|| operation_by_id(method).map(|op| op.id))
            .ok_or_else(|| {
                inv_arg(format!(
                    "unknown method: {method}{}",
                    suggest_aliases("codemode", method)
                ))
            })?;
        (op, args.clone())
    };
    Ok(dispatch_operation(
        session,
        DispatchSurface::CodeMode,
        op_id,
        &args_owned,
    ))
}

/// Parse the optional CAS `base` argument shared by `fs.write` and `fs.edit`.
///
/// `base: null` gates on must-not-exist; `base: "fz://blob/<sha256>"` gates
/// on exact current content. Absent `base` returns `None` (unconditional).
fn parse_base_gate(args: &Value) -> Result<Option<super::fs_ops::WriteBaseGate>, DomainError> {
    let Some(map) = args.as_object() else {
        return Ok(None);
    };
    match map.get("base") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(super::fs_ops::WriteBaseGate::MustNotExist)),
        Some(Value::String(reference)) => {
            let digest = decode_blob_digest(reference).ok_or_else(|| {
                inv_arg(format!(
                    "base must be null or an fz://blob/<sha256> ref; got {reference}"
                ))
            })?;
            let _ = digest;
            let hex = reference
                .strip_prefix("fz://blob/")
                .expect("decode_blob_digest verified the prefix")
                .to_ascii_lowercase();
            Ok(Some(super::fs_ops::WriteBaseGate::MustMatch(hex)))
        }
        Some(other) => Err(inv_arg(format!(
            "base must be null or an fz://blob/<sha256> ref; got {other}"
        ))),
    }
}

/// Did-you-mean suffix for unknown MCP tools / CodeMode methods (R-FE-003).
fn suggest_aliases(surface: &str, needle: &str) -> String {
    let candidates: Vec<&str> = super::operation_abi::OPERATION_REGISTRY
        .iter()
        .flat_map(|op| match surface {
            "mcp" => op.mcp_aliases.iter().copied(),
            "codemode" => op.codemode_aliases.iter().copied(),
            _ => [].iter().copied(),
        })
        .collect();
    let hits = nearest_alias_names(needle, &candidates, 3);
    if hits.is_empty() {
        return String::new();
    }
    format!("; did you mean: {}?", hits.join(", "))
}

fn nearest_alias_names<'a>(needle: &str, candidates: &[&'a str], limit: usize) -> Vec<&'a str> {
    let needle = needle.to_ascii_lowercase();
    let mut out = Vec::new();
    for &cand in candidates {
        if edit_distance_at_most_one(&needle, &cand.to_ascii_lowercase()) {
            if cand != needle.as_str() && !out.contains(&cand) {
                out.push(cand);
                if out.len() >= limit {
                    break;
                }
            }
        }
    }
    out
}

/// ASCII Damerau–Levenshtein distance ≤ 1 (insert/delete/substitute/transpose).
fn edit_distance_at_most_one(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    let (la, lb) = (a.len(), b.len());
    if la.abs_diff(lb) > 1 {
        return false;
    }
    if la == lb {
        let mut diffs = 0usize;
        let mut i = 0usize;
        while i < la {
            if a[i] == b[i] {
                i += 1;
                continue;
            }
            if i + 1 < la && a[i] == b[i + 1] && a[i + 1] == b[i] {
                diffs += 1;
                if diffs > 1 {
                    return false;
                }
                i += 2;
                continue;
            }
            diffs += 1;
            if diffs > 1 {
                return false;
            }
            i += 1;
        }
        return true;
    }
    let (short, long) = if la < lb { (a, b) } else { (b, a) };
    let mut i = 0usize;
    let mut j = 0usize;
    let mut skipped = false;
    while i < short.len() && j < long.len() {
        if short[i] == long[j] {
            i += 1;
            j += 1;
        } else if !skipped {
            skipped = true;
            j += 1;
        } else {
            return false;
        }
    }
    true
}
