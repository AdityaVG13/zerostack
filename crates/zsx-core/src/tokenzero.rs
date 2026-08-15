//! In-process TokenZero domain adapter over the immutable engine revision.
//!
//! [`TokenZeroAdapter`] implements the [`DomainAdapter`] contract for the
//! TokenZero engine (pinned revision [`TOKENZERO_ENGINE_REVISION`],
//! `tokenzero-engine` 1.4.0) by dispatching through the engine's embedded
//! value entry (`tokenzero_engine::execute_embedded_value`) first and falling
//! back to the canonical typed dispatcher (`tokenzero_engine::dispatch_operation`
//! — the same entries the raw-worker-v2 path funnels through), converting the
//! typed `DomainResult` / `DomainError` outcomes into the [`WorkerResult`]
//! envelope the aggregate connector validates. No raw-worker framing crosses
//! this boundary: the module contains no `Command::spawn`, no NDJSON codec,
//! no socket, no MCP transport, and no CodeMode runtime.
//!
//! Identity mirrors the TokenZero raw-worker-v2 binding
//! (`raw_worker_v2_impl::revision` + `surface_handshake::build_surface_capability`):
//!
//! - engine [`EngineIdentity::TokenZero`], ref scheme `tz://`;
//! - `worker_revision` from `ZEROSTACK_WORKER_REVISION` with the pinned
//!   `tokenzero-engine` crate version fallback;
//! - `semantic_contract_version` from the registry
//!   [`SEMANTIC_CONTRACT_VERSION`];
//! - both digests from [`contract_digest_hex`] (the v2 worker binds the
//!   operation-registry digest to the same contract digest).
//!
//! Outcome conversion mirrors the v2 worker frame mapping
//! (`raw_worker_v2_impl::dispatch_call`): shell/compact/ingest operations
//! report [`EffectClass::Irreversible`], everything else
//! [`EffectClass::ReadOnly`]; approvals and revert are never claimed; `tz://`
//! refs are collected recursively from the result value (job polls never
//! contribute ownership, exactly like the v2 `refs` scan); the response
//! echoes `request.trace` verbatim, as the connector requires.
//!
//! Dispatch runs `tokenzero_engine::execute_embedded_value` first — the
//! engine's own embedded seam for `job` polls and background `shell`, the
//! same entry the raw-worker-v2 dispatcher uses — and falls back to
//! `dispatch_operation` for every registry operation. No job or
//! background-shell execution or parsing is duplicated in this module.
//!
//! Refs: the in-process core verifies every `://blob/` ref against the hub
//! shared CAS under the session root and retains it for GC reachability, so
//! this adapter publishes the exact payload of every portable `tz://blob/…`
//! ref it returns into that CAS (resolved raw from the engine's recovery
//! store, reloading once on a stale handle). Non-portable refs (`tz://s/…`,
//! `tz://seq/…`) and foreign-scheme blob refs are skipped, exactly as the
//! core's reachability scan skips them.
//!
//! Cancellation and deadline are checked before dispatch; the connector
//! re-checks both at every boundary. The remaining deadline is installed as
//! the engine's cooperative wall deadline for the dispatch so hot loops stop
//! inside the declared bound, and a post-dispatch cancellation check mirrors
//! the v2 worker.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tokenzero_core::operation_abi::{
    DomainError, DomainErrorKind, SEMANTIC_CONTRACT_VERSION, contract_digest_hex,
};
use tokenzero_engine::wall::{WallDeadline, with_host_wall_deadline_and_cancel};
use tokenzero_engine::{
    DispatchSurface, EmbeddedDispatchError, EngineConfig, TokenZeroEngine, dispatch_operation,
    execute_embedded_value,
};
use tokenzero_recovery::RecoveryStore;
use zero_abi::{
    ApprovalMetadata, ApprovalState, CallRequest, EffectClass, EngineIdentity, EngineStageSpanV1,
    EngineStageTimelineV1, RefOwnership, RevertMetadata, TOKEN_JOB_OPERATION_V1, WorkerResult,
    WorkerResultMetadata, WorkerTokenAccountingV1, WorkerTokenCountKind,
};
use zero_ref::{ZeroRefV1, ZeroScheme};
use zero_store::{Engine as StoreEngine, ResolvedStore, SharedCas};

use crate::adapter::{
    AdapterBinding, AdapterCall, AdapterContractError, AdapterError, AdapterResponse, DomainAdapter,
};

/// Pinned immutable TokenZero engine revision this adapter is written against
/// (conformance `program-aggregate-2026-08-11.json` `sourceHead`).
pub const TOKENZERO_ENGINE_REVISION: &str = "d1a8ebbb6a88c61b6f56f6d5e7a72d2a0a00268b";
/// `tokenzero-engine` crate version at the pinned revision (workspace 1.4.0).
pub const TOKENZERO_ENGINE_VERSION: &str = "1.4.0";

/// Advertised and enforced output cap, byte-identical to the raw worker (9lwo):
/// the serialized result value of any call must fit within this many bytes.
const MAX_OUTPUT_BYTES: usize = 65_536;
/// Default per-call deadline when the request carries none (matches the
/// raw-worker handshake advertisement).
const DEFAULT_DEADLINE_MS: u64 = 30_000;
/// Engine-stage span name for the in-process boundary.
const ENGINE_STAGE: &str = "tokenzero.in_process_call";

/// In-process TokenZero domain adapter.
///
/// Owns one [`TokenZeroEngine`] built exactly like the raw-worker process
/// entry (`engine_from_options`: `EngineConfig::for_root`, session dedup off).
/// The connector serializes per-engine calls, so the adapter itself is
/// stateless between calls apart from the lazily opened recovery-store handle
/// used for exact ref publishing.
pub struct TokenZeroAdapter {
    engine: TokenZeroEngine,
    binding: AdapterBinding,
    root: PathBuf,
    session_id: String,
    /// Read-only recovery-store handle for publishing exact ref payloads into
    /// the hub CAS; opened lazily on the first ref-bearing call.
    recovery: OnceLock<Mutex<RecoveryStore>>,
}

impl TokenZeroAdapter {
    /// Build the adapter for `root` (the session root the hub CAS lives
    /// under) and the session id the connector will validate ownership
    /// against.
    pub fn new(
        root: impl Into<PathBuf>,
        session_id: impl Into<String>,
    ) -> Result<Self, AdapterContractError> {
        let root = root.into();
        let cache_path = ResolvedStore::resolve_from_process(&root, StoreEngine::TokenZero, &[])
            .engine_dir()
            .join("recovery-cache.json");
        Self::build(root.clone(), root, cache_path, session_id)
    }

    /// Build over `workspace_root` while keeping TokenZero recovery and CAS
    /// state below the explicit session `state_root`.
    pub fn new_with_state_root(
        workspace_root: impl Into<PathBuf>,
        state_root: impl Into<PathBuf>,
        session_id: impl Into<String>,
    ) -> Result<Self, AdapterContractError> {
        let workspace_root = workspace_root.into();
        let state_root = state_root.into();
        let cache_path = state_root.join("tokenzero").join("recovery-cache.json");
        Self::build(workspace_root, state_root, cache_path, session_id)
    }

    fn build(
        workspace_root: PathBuf,
        state_root: PathBuf,
        cache_path: PathBuf,
        session_id: impl Into<String>,
    ) -> Result<Self, AdapterContractError> {
        let _ = std::fs::create_dir_all(cache_path.parent().unwrap_or(&state_root));
        let mut config = EngineConfig::for_root(&workspace_root);
        config.cache_path = cache_path;
        // Mirror the raw-worker entry (`engine_from_options`): the seen-set
        // redundancy layer stays off for the composition path.
        config.session_dedup = false;
        let engine = TokenZeroEngine::new(config);
        let digest = contract_digest_hex();
        let binding = AdapterBinding::new(
            EngineIdentity::TokenZero,
            worker_revision(),
            SEMANTIC_CONTRACT_VERSION,
            digest.clone(),
            digest,
            "tz://",
        )?;
        Ok(Self {
            engine,
            binding,
            root: state_root,
            session_id: session_id.into(),
            recovery: OnceLock::new(),
        })
    }

    /// The session root the hub CAS is published under.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The session id this adapter claims ownership for.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Dispatch one validated call. Embedded raw-worker seams (`job` polls
    /// and background `shell`) run through `execute_embedded_value` first,
    /// exactly like the raw-worker v2 dispatcher; everything else falls back
    /// to the canonical typed dispatcher. Returns the raw domain value plus
    /// the domain operation's declared ref list on success, or a typed
    /// registry error. The ref list is the canonical `DomainResult.refs` the
    /// v2 worker echoes into its envelope's `refs` key; ownership is carried
    /// here in `metadata.ownership.refs` instead of an envelope key.
    fn dispatch(&self, request: &CallRequest) -> Result<(Value, Vec<String>), DomainError> {
        match execute_embedded_value(&self.engine, &request.op, &request.args) {
            // Embedded seams return bare values and never declare refs: job
            // tails are content, not minted refs, and background launches
            // carry none.
            Some(Ok(value)) => Ok((value, Vec::new())),
            Some(Err(error)) => Err(embedded_error(error, request)),
            None => {
                let outcome = dispatch_operation(
                    &self.engine,
                    DispatchSurface::RawWorker,
                    &request.op,
                    &request.args,
                );
                if let Some(error) = outcome.tool_domain_error() {
                    return Err(error);
                }
                if let Some(error) = outcome.domain_error {
                    return Err(error);
                }
                Ok((outcome.result.value, outcome.result.refs.clone()))
            }
        }
    }

    /// Bind one dispatched outcome into the validated in-process envelope.
    fn bind_outcome(
        &self,
        request: &CallRequest,
        outcome: Result<(Value, Vec<String>), DomainError>,
        elapsed: Duration,
    ) -> Result<AdapterResponse, AdapterError> {
        let (mut value, domain_refs) = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                return Err(AdapterError::new(
                    error.kind.as_str(),
                    error.message,
                    error.retryable,
                    Some(request.trace.clone()),
                ));
            }
        };
        bind_terminal_exact_expansion(request, &mut value);
        // 9lwo: advertised limits must be effective; an oversized value
        // becomes a typed error naming the limit, never a truncated result.
        let output_bytes = serde_json::to_vec(&value).map_or(0, |bytes| bytes.len());
        if output_bytes > MAX_OUTPUT_BYTES {
            let range_hint = request
                .args
                .get("ref")
                .and_then(Value::as_str)
                .filter(|reference| !reference.contains('#'))
                .map(|reference| {
                    format!(
                        "; retry with {reference}#B0-32768 and continue with later byte ranges"
                    )
                })
                .unwrap_or_default();
            return Err(AdapterError::new(
                "output_too_large",
                format!(
                    "operation result is {output_bytes} bytes; the advertised max_output_bytes limit is {MAX_OUTPUT_BYTES}{range_hint}"
                ),
                false,
                Some(request.trace.clone()),
            ));
        }
        let telemetry = request.telemetry_request.as_ref();
        let worker_token_accounting = if telemetry.is_some_and(|t| t.worker_token_accounting) {
            match worker_token_accounting(&request.op, &request.args, &value) {
                Ok(accounting) => Some(accounting),
                Err(message) => {
                    return Err(AdapterError::new(
                        "invalid_token_accounting",
                        message,
                        false,
                        Some(request.trace.clone()),
                    ));
                }
            }
        } else {
            None
        };
        // Job tails are arbitrary shell bytes; a line beginning with `tz://`
        // is content, not a minted ref, so job results never contribute
        // ownership (mirrors the v2 worker). The v2 worker's ownership scan
        // runs over its envelope value, whose `refs` key carries exactly
        // `DomainResult.refs`; the in-process envelope carries the same list
        // in `metadata.ownership.refs`, so the declared refs are merged with
        // any standalone `tz://` strings found in the value (deduplicated,
        // order-preserving).
        let mut refs = Vec::new();
        if request.op != TOKEN_JOB_OPERATION_V1 {
            collect_refs(&value, &mut refs);
            for reference in &domain_refs {
                if !refs.iter().any(|existing| existing == reference) {
                    refs.push(reference.clone());
                }
            }
        }
        self.publish_refs(&refs)?;
        let result = WorkerResult {
            value,
            metadata: WorkerResultMetadata {
                effect: effect_class(&request.op),
                approval: ApprovalMetadata {
                    state: ApprovalState::NotRequired,
                    approval_id: None,
                    policy: None,
                },
                revert: RevertMetadata {
                    supported: false,
                    journal_id: None,
                    rollback_op: None,
                },
                ownership: RefOwnership {
                    engine: EngineIdentity::TokenZero,
                    session_id: self.session_id.clone(),
                    refs,
                    snapshot: None,
                },
                trace: request.trace.clone(),
            },
        };
        let engine_timeline = telemetry.is_some_and(|t| t.engine_stage_timeline).then(|| {
            let total_ns = elapsed.as_nanos().min(u128::from(u64::MAX)) as u64;
            let total_ns = total_ns.max(1);
            EngineStageTimelineV1 {
                total_ns,
                spans: vec![EngineStageSpanV1 {
                    stage: ENGINE_STAGE.into(),
                    start_ns: 0,
                    duration_ns: total_ns,
                }],
            }
        });
        Ok(AdapterResponse {
            result,
            engine_timeline,
            worker_token_accounting,
        })
    }

    /// Publish the exact payload of every portable `tz://blob/…` ref into the
    /// hub CAS under the session root, so the connector's reachability
    /// verification and GC retention can resolve it. Foreign-scheme blob refs
    /// route to their owning engine and are skipped; non-portable refs do not
    /// contain `://blob/` and are skipped by the core anyway.
    fn publish_refs(&self, refs: &[String]) -> Result<(), AdapterError> {
        let cas = SharedCas::open(&self.root);
        for reference in refs {
            if !reference.contains("://blob/") {
                continue;
            }
            let parsed = ZeroRefV1::parse(reference).map_err(|error| {
                AdapterError::new(
                    "invalid_ref",
                    format!("token adapter ref {reference:?} is not portable v1: {error}"),
                    false,
                    None,
                )
            })?;
            if parsed.scheme != ZeroScheme::Tz {
                continue;
            }
            // Presence is not identity: a tampered dest still `contains()`.
            if cas.get_verified(&parsed.hash).is_ok() {
                continue;
            }
            let payload = self.resolve_payload(&parsed.hash)?;
            let published = cas.put(&payload).map_err(|error| {
                AdapterError::new(
                    "internal_contract",
                    format!(
                        "cannot publish token blob {} to hub CAS: {error}",
                        parsed.hash
                    ),
                    false,
                    None,
                )
            })?;
            if published != parsed.hash {
                return Err(AdapterError::new(
                    "internal_contract",
                    format!(
                        "token blob digest mismatch: ref {parsed_hash} published {published}",
                        parsed_hash = parsed.hash
                    ),
                    false,
                    None,
                ));
            }
        }
        Ok(())
    }

    /// Resolve the exact payload bytes for `tz://blob/<hash>` from the
    /// engine's recovery store (selector `raw`, the same exact-bytes path the
    /// engine's store adapter uses). A stale handle is reloaded once: the
    /// engine persists its store before returning, so a miss on the first
    /// read usually means our handle predates the write.
    fn resolve_payload(&self, hash: &str) -> Result<Vec<u8>, AdapterError> {
        // Modern TokenZero blob refs are published to the engine's shared CAS,
        // which is derived from the configured recovery-cache path. The legacy
        // RecoveryStore lookup below is still needed for non-CAS compatibility
        // records, but cannot resolve a full-hash blob by itself.
        if let Some(cas) = tokenzero_recovery::shared_cas::SharedCas::detect_from_cache_path(
            &self.engine.config.cache_path,
        ) && let Ok(payload) = cas.resolve(hash)
        {
            return Ok(payload);
        }
        let store = self.recovery.get_or_init(|| {
            Mutex::new(RecoveryStore::new(Some(
                self.engine.config.cache_path.clone(),
            )))
        });
        let reference = format!("tz://blob/{hash}");
        for attempt in 0..2 {
            let result = store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .expand(&reference, Some("raw"), None, None, None, None);
            if result.found {
                return Ok(result.content.into_bytes());
            }
            if attempt == 0 {
                let mut guard = store
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *guard = RecoveryStore::new(Some(self.engine.config.cache_path.clone()));
            }
        }
        Err(AdapterError::new(
            "invalid_ref",
            format!("token blob {reference} is not resolvable from the engine recovery store"),
            false,
            None,
        ))
    }
}

impl DomainAdapter for TokenZeroAdapter {
    fn engine(&self) -> EngineIdentity {
        EngineIdentity::TokenZero
    }

    fn binding(&self) -> AdapterBinding {
        self.binding.clone()
    }

    fn call(&self, call: AdapterCall<'_>) -> Result<AdapterResponse, AdapterError> {
        let request = call.request;
        if call.cancellation.is_cancelled() {
            return Err(AdapterError::new(
                "cancelled",
                "token adapter cancelled before dispatch",
                false,
                Some(request.trace.clone()),
            ));
        }
        if request.deadline_expired(now_ms()) {
            return Err(AdapterError::new(
                "deadline",
                "token adapter deadline expired before dispatch",
                false,
                Some(request.trace.clone()),
            ));
        }
        if forbidden_operation(&request.op) {
            return Err(AdapterError::new(
                "unsupported_operation",
                "planner, JavaScript, and MCP operations are forbidden",
                false,
                Some(request.trace.clone()),
            ));
        }
        // Install the remaining wall budget as the engine's cooperative
        // deadline so find/expand/shell hot loops checkpoint inside the
        // declared bound; the domain kernel maps an overrun to
        // DeadlineExceeded exactly like the raw worker.
        let remaining_ms = request
            .deadline_unix_ms
            .map(|deadline| deadline.saturating_sub(now_ms()))
            .unwrap_or(DEFAULT_DEADLINE_MS)
            .max(1);
        let started = Instant::now();
        let outcome = with_host_wall_deadline_and_cancel(
            WallDeadline::from_elapsed_ms(0, remaining_ms),
            call.cancellation.as_atomic(),
            || self.dispatch(request),
        );
        if call.cancellation.is_cancelled() {
            return Err(AdapterError::new(
                "cancelled",
                "token adapter cancelled during dispatch",
                false,
                Some(request.trace.clone()),
            ));
        }
        self.bind_outcome(request, outcome, started.elapsed())
    }
}

/// Worker revision advertised in the binding and echoed into traces.
/// Mirrors the v2 worker: `ZEROSTACK_WORKER_REVISION` wins, the pinned
/// `tokenzero-engine` crate version is the fallback.
fn worker_revision() -> String {
    std::env::var("ZEROSTACK_WORKER_REVISION")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| TOKENZERO_ENGINE_VERSION.to_string())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn domain_error(
    kind: DomainErrorKind,
    message: impl Into<String>,
    request: &CallRequest,
) -> DomainError {
    DomainError::new(kind, message.into()).with_op(request.op.clone())
}

/// Map the embedded-seam typed error into the registry error envelope.
/// `validation` and `not_found` keep their meaning; `invalid_result` means
/// the engine's own seam emitted an untypeable payload — a runtime failure
/// of the seam, not a caller error.
fn embedded_error(error: EmbeddedDispatchError, request: &CallRequest) -> DomainError {
    let kind = match error.kind {
        "validation" => DomainErrorKind::Validation,
        "not_found" => DomainErrorKind::NotFound,
        _ => DomainErrorKind::Runtime,
    };
    domain_error(kind, error.message, request)
}

/// Effect classification, byte-identical to the v2 worker's `effect_class`.
fn effect_class(op: &str) -> EffectClass {
    match op {
        "shell" | "tz_shell" | "zero.shell" | "compact" | "tz_compact" | "zero.compact"
        | "ingest" | "tz_ingest" | "zero.ingest" => EffectClass::Irreversible,
        _ => EffectClass::ReadOnly,
    }
}

/// Recursively collect every `tz://` string in `value`, byte-identical to the
/// v2 worker's `refs` scan.
fn collect_refs(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::String(reference) if reference.starts_with("tz://") => {
            output.push(reference.clone());
        }
        Value::Array(values) => values.iter().for_each(|value| collect_refs(value, output)),
        Value::Object(values) => values
            .values()
            .for_each(|value| collect_refs(value, output)),
        _ => {}
    }
}

/// Forbidden-operation mask, byte-identical to the v2 worker's `forbidden`.
fn forbidden_operation(op: &str) -> bool {
    let op = op.to_ascii_lowercase();
    matches!(
        op.as_str(),
        "plan"
            | "planner"
            | "js"
            | "javascript"
            | "mcp"
            | "execute_code"
            | "tz_execute_code"
            | "codemode_search"
            | "tz_codemode_search"
            | "codemode_describe"
            | "tz_codemode_describe"
            | "tools/call"
            | "tools/list"
    ) || op.starts_with("planner.")
        || op.starts_with("javascript.")
        || op.starts_with("mcp.")
}

fn encoded_len(field: &str, value: &Value) -> Result<u64, String> {
    serde_json::to_vec(value)
        .map_err(|error| format!("cannot encode {field} for token accounting: {error}"))
        .and_then(|bytes| checked_u64_count(field, bytes.len()))
}

/// Returns (recovery bytes, refs_omitted). A successful domain result that
/// omits `refs` no longer fails the call: engine repos evolve independently
/// of this adapter, and hard-failing every read/shell on a contract-shape
/// drift is worse than accounting recovery as zero. The omission stays loud:
/// the caller downgrades `count_kind` to `Estimate` because the byte total
/// can undercount recovery and is no longer a proven upper bound.
fn declared_recovery_bytes(value: &Value, allow_missing: bool) -> Result<(u64, bool), String> {
    let Some(refs) = value.get("refs") else {
        return Ok((0, !allow_missing));
    };
    let refs = refs
        .as_array()
        .ok_or_else(|| "successful domain result refs must be an array".to_string())?;
    let total = refs.iter().try_fold(0_u64, |total, record| {
        let bytes = record
            .get("bytes")
            .and_then(Value::as_u64)
            .ok_or_else(|| "domain ref omitted a valid bytes count".to_string())?;
        total
            .checked_add(bytes)
            .ok_or_else(|| "declared recovery bytes overflowed".to_string())
    })?;
    Ok((total, false))
}

/// Tokenizer-independent upper-bound accounting mirrored by TokenZero's raw
/// worker. UTF-8/JSON bytes cannot undercount tokens because every token
/// consumes at least one source byte. Exact ref-token totals remain unknown.
fn worker_token_accounting(
    op: &str,
    args: &Value,
    value: &Value,
) -> Result<WorkerTokenAccountingV1, String> {
    let is_job = op == TOKEN_JOB_OPERATION_V1;
    let is_background_shell =
        matches!(op, "shell" | "tz_shell" | "zero.shell") && args["background"] == true;
    let accounting_optional = is_job || is_background_shell;
    let accounting = value
        .get("accounting")
        .map(|accounting| {
            serde_json::from_value::<tokenzero_core::Accounting>(accounting.clone())
                .map_err(|error| format!("invalid domain accounting: {error}"))
        })
        .transpose()?;
    if accounting.is_none() && !accounting_optional {
        return Err("successful domain result omitted accounting".to_string());
    }
    if accounting
        .as_ref()
        .is_some_and(|accounting| accounting.cached_tokens > accounting.billed_tokens)
    {
        return Err("worker token accounting cached_tokens exceeds billed_tokens".to_string());
    }
    let input_bytes = encoded_len("request args", args)?;
    let output_bytes = encoded_len("domain result", value)?;
    let (recovery_bytes, refs_omitted) = declared_recovery_bytes(value, accounting_optional)?;
    let raw_tokens = input_bytes
        .checked_add(output_bytes)
        .and_then(|value| value.checked_add(recovery_bytes))
        .ok_or_else(|| "raw token upper bound overflowed".to_string())?;
    let domain_billed = accounting
        .as_ref()
        .map(|accounting| checked_u64_count("billed_tokens", accounting.billed_tokens))
        .transpose()?
        .unwrap_or(output_bytes);
    let cached_tokens = accounting
        .as_ref()
        .map(|accounting| checked_u64_count("cached_tokens", accounting.cached_tokens))
        .transpose()?
        .unwrap_or(0);
    let worker = WorkerTokenAccountingV1 {
        // Conservative upper-bound accounting has no bound tokenizer
        // version; it is never charged into the resource ledger.
        tokenizer_version_digest: None,
        tokenizer_id: "conservative:utf8-json-bytes-v1".to_string(),
        // A refs-omitting success can undercount recovery bytes, so its
        // accounting is loudly an estimate, not a proven upper bound.
        count_kind: if refs_omitted {
            WorkerTokenCountKind::Estimate
        } else {
            WorkerTokenCountKind::ConservativeUpperBound
        },
        raw_tokens,
        visible_tokens: output_bytes,
        recovery_tokens: recovery_bytes,
        billed_tokens: domain_billed.max(output_bytes),
        cached_tokens,
        exact_ref_tokens: None,
    };
    Ok(worker)
}

fn checked_u64_count(field: &str, value: usize) -> Result<u64, String> {
    u64::try_from(value).map_err(|_| format!("{field} exceeds the raw-worker accounting range"))
}

fn bind_terminal_exact_expansion(request: &CallRequest, value: &mut Value) {
    if request.op != "expand"
        || value.get("status").and_then(Value::as_str) != Some("ok")
        || value.get("mode").and_then(Value::as_str) != Some("exact")
    {
        return;
    }
    let Some(visible) = value
        .get("visible")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return;
    };
    let Some(object) = value.as_object_mut() else {
        return;
    };
    object.insert("op".into(), Value::String("tz_expand".into()));
    object.insert(
        "tool_response".into(),
        serde_json::json!({
            "tool": "expand",
            "status": "ok",
            "mode": "exact",
            "visible": {"kind": "capsule", "text": visible},
            "recovery": {
                "do_not_recompact": true,
                "exact_bytes": true,
                "terminal": true
            }
        }),
    );
}

#[cfg(test)]
#[path = "../../../tests/rust/zsx-core/unit/tokenzero.rs"]
mod tests;
