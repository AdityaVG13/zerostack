#![forbid(unsafe_code)]
// Engine modules use `use super::*` / crate-level imports after the split.
#![allow(unused_imports)]

//! Transport-neutral TokenZero domain engine and typed dispatcher.
//!
//! # Dependency direction (tokenzero-irx9.2)
//!
//! This crate must not depend on FastMCP, MCP JSON-RPC framing, or the CodeMode
//! sandbox. Transport adapters (`tokenzero-mcp`, CLI) depend inward and call
//! [`dispatch_operation`] exactly once per domain op.
//!
//! `zero_abi::TokenEngine` (`ZeroTokenEngine`) is owned by `tokenzero-kernel`.
//! This crate does not re-export it. CLI talks to recovery through the
//! operator facades in [`cache_maintenance`], not `tokenzero-recovery` internals.

pub mod action_cache_key;
pub mod admission;
pub mod binary_resolve;
pub mod cache_crossover;
pub mod cache_maintenance;
pub mod cache_meter;
mod cache_pack;
pub mod cachezero;
pub mod codemode_catalog;
pub mod codemode_wire;
mod collect;
pub mod config;
mod diff;
mod dispatcher;
mod domain;
mod engine_common;
mod engine_edit;
mod engine_expand;
mod engine_fetch;
mod engine_find;
mod engine_ingest;
mod engine_misc;
mod engine_read;
mod engine_search;
mod engine_session;
mod engine_shell;
pub mod eviction_scheduler;
pub mod expand_params;
pub mod exposure;
mod fetch_cache;
mod fetch_guard;
pub mod frontier;
pub mod ledger;
pub mod metrics;
pub mod paths;
/// Profiling-only measurement hooks (`TOKENZERO_PERF_PROFILE`). Not a product surface.
pub mod perf_profile;
pub mod prefix_probe;
pub mod racc_gauge;
pub mod raw_worker;
mod recall;
pub mod render;
mod report_tool;
pub mod session;
pub mod session_persist;
pub mod shell_hooks;
pub mod surface_handshake;
pub mod surface_health;
mod text_aliases;
pub mod usage_telemetry;
pub mod wall;
pub mod warmkeeper;
pub mod workspace;
pub mod write_ladder;

pub use action_cache_key::{
    ACTIONCACHE_KEY_SCHEMA, ActionCacheKeyInput, ConsistencyClass, action_cache_envelope,
    action_cache_key,
};
pub use admission::{
    ADMISSION_BYTES_PER_TOKEN, ADMISSION_SCHEMA, AdmissionDecision, AdmissionEstimator,
    AdmissionPolicy, AdmissionReason,
};
pub use binary_resolve::{
    BinaryResolution, ResolveError, ResolvedBinary, TOKENZERO_BIN_ENV, TOKENZERO_CURL_PATH_ENV,
    TOKENZERO_RG_PATH_ENV, engine_binaries_json, resolve_all_engine_binaries, resolve_curl_binary,
    resolve_rg_binary, resolve_tokenzero_binary,
};
pub use cache_crossover::{
    CACHE_CROSSOVER_SCHEMA, CacheContentClass, CacheCrossoverAction, CacheCrossoverError,
    CacheCrossoverInput, CacheCrossoverReason, CacheCrossoverReceipt, EmissionCrossoverConfig,
    TOKEN_COST_PPM_SCALE, decide_cache_crossover,
};
pub use cache_maintenance::{
    OperatorMigrationOutcome, cache_maintenance, cache_maintenance_coalesced,
    cache_migrate_cleanup, cache_migrate_refs, cache_migrate_rollback, cache_migrate_verify,
    cachezero_stats_json, prune_stale_cache, recovery_blob_status_json, recovery_migration_state,
    session_pack, shell_spill_dir,
};
pub use collect::{find_rg_in_path, parse_rg_line};
pub use dispatcher::{
    DispatchOutcome, DispatchProfile, DispatchSurface, dispatch_cli, dispatch_codemode_method,
    dispatch_count, dispatch_mcp_tool, dispatch_operation, dispatch_raw_worker,
    last_dispatch_profile, tool_response_to_domain,
};
pub use domain::{
    DomainDispatchError, EmbeddedDispatchError, all_domain_operations, batch_response,
    domain_fastmcp_ops, execute_domain_op, execute_embedded_value, is_domain_operation,
    operation_is_domain,
};
pub use fetch_cache::{load_fetch_index, record_fetch};
pub use racc_gauge::{
    ChargeReceiptFragment, CompressionRoute, SessionRaccGauge, charge_from_accounting,
    classify_compression, lexical_tokenizer_identity, seal_with_labeled_evidence,
};
pub use raw_worker::{
    RawWorkerError, RawWorkerRequest, RawWorkerResponse, RawWorkerServeOptions,
    execute_raw_worker_frame, execute_raw_worker_json, maybe_run_raw_worker_from_args,
    parse_raw_worker_argv, raw_worker_print_handshake, response_from_outcome, run_raw_worker_once,
    run_raw_worker_serve,
};
pub use render::{
    cli_json, exact_ref_token_count, prune_dead_refs, render_text, request_full_cli_envelope,
    slim_envelope_enabled,
};
pub use surface_handshake::{
    CompressionOwner, HandshakeSurface, PlannerOwner, RAW_WORKER_PROTOCOL_VERSION,
    SURFACE_CAPABILITY_SCHEMA, SurfaceCapability, SurfaceLimits, build_surface_capability,
    check_contract_compatibility, composition_trace, surface_capability_json,
};

pub use report_tool::{build_tool_issue_report, is_reportable_tool_name, record_tool_issue};
pub use shell_hooks::{ProcessHooks, install as install_process_hooks};
pub use workspace::{
    SHARED_STORE_OPT_IN_ENVS, STORE_ROOT_ENVS, StoreResolutionReport, allowed_roots_for_workspace,
    default_allowed_roots, default_recovery_cache_path, resolve_recovery_cache_path,
    resolve_recovery_cache_path_with_env, resolve_store_root_with_env,
    shared_store_opt_in_from_env, store_is_under_project_root, store_resolution_json,
    store_resolution_report, store_resolution_report_with_env, tokenzero_work_root,
};
pub use write_ladder::{
    WRITE_ESCAPE_ENV, WRITE_RECOVERY_LADDER, annotate_write_failure, write_escape_ack_active,
};

use cache_pack::{
    cache_pack_manifest_path, cache_pack_sources, previous_cache_digest, read_line_range_from_file,
};
use collect::*;
use engine_common::*;
use fetch_cache::{epoch_secs, fetch_index_path};
use fetch_guard::{FETCH_META_MARKER, split_fetch_meta, validate_fetch_target};
use globset::{GlobBuilder, GlobMatcher};
use paths::*;
use render::*;
use serde_json::{Value, json};
use session::{DiffTelemetry, SeenState, ServeKey, ServedRecord, SessionMemory, SessionSummary};
use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, SystemTime};
use tokenzero_core::{
    Accounting, CLI_SCHEMA_VERSION, ContentType, Mode, ShellRenderInput, ToolResponse,
    count_tokens, detect_content_type, make_capsule, make_capsule_with_raw_tokens, ref_record,
    render_shell, sha256_hex, shell_combined_output, shell_raw_tokens,
};
use tokenzero_filters::rewrite_command;
use tokenzero_recovery::{ExpansionResult, RecoveryStore, StoredPayload};
use tokenzero_runtime::{
    RunOutputPolicy, StreamCapture, contains_platform_shell_syntax, run_command_with_policy,
    split_command_string,
};

pub const DEFAULT_SHELL_TIMEOUT_SECS: u64 = 60;
pub const MAX_SHELL_TIMEOUT_SECS: u64 = 3600;
/// Idle exit is disabled by default: agent sessions can sit idle for hours
/// between tool calls, and an idle-exited server reads as a disconnect to MCP
/// clients. Stale-process cleanup relies on stdin EOF when the client goes
/// away; idle exit remains available as an explicit opt-in.
pub const DEFAULT_MCP_IDLE_TIMEOUT_SECS: u64 = 0;
pub const MAX_MCP_IDLE_TIMEOUT_SECS: u64 = 24 * 60 * 60;
/// Portable one-token response when requested bytes are already prompt-resident.
/// Surfaces compare against this instead of depending on `tokenzero-recovery`.
pub const ALREADY_RESIDENT_ATOM: &str = tokenzero_recovery::working_set::ALREADY_RESIDENT_ATOM;

/// True when `text` is the already-resident working-set atom (trim-insensitive).
pub fn is_already_resident_text(text: &str) -> bool {
    text.trim() == ALREADY_RESIDENT_ATOM
}

const SEARCH_VISIT_MULTIPLIER: usize = 500;
const MIN_SEARCH_VISITED_FILES: usize = 1_000;
const MAX_SEARCH_VISITED_FILES: usize = 50_000;
pub const SEARCH_BACKEND_ENV: &str = "TOKENZERO_SEARCH_BACKEND";
pub const RG_PATH_ENV: &str = "TOKENZERO_RG_PATH";
pub const SESSION_DEDUP_ENV: &str = "TOKENZERO_MCP_DEDUP";
pub const DIFF_READS_ENV: &str = "TOKENZERO_MCP_DIFF_READS";
/// Diff-aware re-reads skip diffing when either side exceeds these bounds;
/// oversized payloads get a full serve instead (docs/codemode.md §5b).
const DIFF_MAX_BYTES: usize = 2 * 1024 * 1024;
const DIFF_MAX_LINES: usize = 50_000;

pub use cache_meter::{
    ANTHROPIC_CACHE_DIAGNOSIS_BETA, AnthropicCacheDiagnosisRequest, CacheMeter, CacheMeterError,
    CacheObservation, CachePricing, CacheProvider, CacheSessionReport, CacheSloConfig,
    CacheSloDashboard, ProviderCacheEligibility, ProviderCacheEligibilityStatus,
    ProviderCacheTelemetry, ProviderUsage, cache_miss_attribution, parse_provider_usage,
    parse_provider_usage_observation,
};
pub use config::{
    CAPSULE_EXACT_REF_THRESHOLD_ENV, CORRIDOR_ENV, CorridorEstimates,
    DEFAULT_CAPSULE_EXACT_REF_THRESHOLD_BYTES, DEFAULT_SHELL_INLINE_BUDGET, EngineConfig,
    FETCH_ALLOW_ENV, FETCH_DENY_ENV, FETCH_ENABLED_ENV, RATC_ENV, RATC_STATUS_ADVISORY,
    RatcWeights, SHELL_INLINE_BUDGET_ENV, SearchBackend, TELEMETRY_ENV,
    capsule_exact_ref_threshold, capsule_exact_ref_threshold_from_env, default_mcp_idle_timeout,
    default_shell_timeout, mcp_idle_timeout_from_secs, mcp_tool_surface_from_env,
    parse_corridor_estimates, parse_ratc_weights, resolve_telemetry, shell_inline_budget_from_env,
    shell_timeout_from_millis, shell_timeout_from_secs, telemetry_env_enabled,
};
pub use eviction_scheduler::{
    CacheBreakpoint, EvictionBatch, EvictionCandidate, EvictionDecision, EvictionDecisionKind,
    EvictionReplayItem, EvictionReplayReport, EvictionSavingsLedger, EvictionSchedule,
    OPENAI_MAX_RETENTION_SECONDS, PrefixTier, provider_breakpoints, schedule_evictions,
    simulate_eviction_replay, ttl_from_gaps,
};
pub use frontier::{
    FRONTIER_OPTIMIZER_NAME, FRONTIER_PLAN_SCHEMA, FrontierBudgets, FrontierPlan,
    FrontierPlanObject, plan_frontier_resident_set,
};
pub use ledger::{CountMethodVersion, UNSTAMPED_LEGACY, current_count_method_version};
pub use prefix_probe::{
    ArmTrial, HistoryChunk, ProbeArm, ProbeFixture, ProbeReport, QualitySlot, replay_prefix_probe,
};
pub use usage_telemetry::{
    AmplificationRecord, DirectionTokens, ExecutionPath, OperationClass, TA_REGISTRY,
    TaClassReport, TaCostLockViolation, TelemetryInspection, UsageRecord, enforce_ta_cost_locks,
    inspect_usage_telemetry, record_codemode_accounting, record_mcp_accounting,
    record_operation_amplification, replay_ta_table, usage_telemetry_enabled,
    usage_telemetry_path_for_cache,
};
pub use warmkeeper::{
    HotPlacement, PrefetchTarget, ResumeRewarmKind, WARM_PING_OUTPUT_TOKENS, WarmDecision,
    WarmDecisionKind, WarmLane, WarmLaneTier, WarmReplayLane, WarmSimulationReport,
    ZeroOutputTouch, resume_rewarm_kind, schedule_rewarms, select_prefetch_targets,
    simulate_warmkeeper,
};

/// One find/replace hunk for [`TokenZeroEngine::edit`]. `find` must match the
/// evolving file text exactly once unless `replace_all` is set.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct EditHunk {
    pub find: String,
    pub replace: String,
    #[serde(default)]
    pub replace_all: bool,
}

/// Per-call serving options for read/find/grep. Existing positional methods
/// delegate here with defaults so their signatures stay stable.
#[derive(Debug, Clone, Copy, Default)]
pub struct ServeOptions {
    /// Bypass the session redundancy layer for this call: always serve the
    /// full render. The serve is still recorded so later calls can dedup.
    pub fresh: bool,
}

#[derive(Debug)]
pub struct TokenZeroEngine {
    pub config: EngineConfig,
    /// Resolved rg binary, looked up once per engine instance.
    rg_binary: OnceLock<Option<PathBuf>>,
    /// Session-lifetime seen-set for the redundancy layer (docs/codemode.md
    /// §5). Loaded from `session-memory.json` when dedup is enabled.
    // None until a tool actually needs the persisted working set. Session boot must not
    // deserialize session-memory.json on the compatible manifest+delta path.
    /// Visible to integration tests in tokenzero-mcp (same-process harness).
    pub session: Mutex<Option<SessionMemory>>,
    /// Prompt-resident spans; bodies page to durable refs under budget pressure.
    /// Visible to integration tests in tokenzero-mcp.
    pub working_set: Mutex<tokenzero_recovery::working_set::WorkingSet>,
    /// Reusable RecoveryStore slot for long-lived MCP/CodeMode engines.
    /// A request checks the store out instead of holding this mutex while it
    /// performs filesystem work, so unrelated concurrent requests do not serialize.
    /// SAFETY: occupancy only. `recovery_store()` drops this mutex before
    /// `RecoveryStore::new(Some(path))` snapshot/journal I/O (sibling of
    /// session cold-load-off-mutex). Persist lives inside RecoveryStore.
    recovery_store: Option<Mutex<Option<tokenzero_recovery::RecoveryStore>>>,
    /// Single-flight gate: ServeKeys currently being served, with a condvar
    /// to wake waiters. Two pipelined identical reads on the 4-worker pool
    /// would otherwise both miss the seen-set (the first has not recorded its
    /// serve yet) and both serve full — the dedup race behind the
    /// unreproducible repeat-read benchmark. A second request for a key in
    /// flight waits for the first to record, then dedups.
    in_flight: (Mutex<HashSet<ServeKey>>, Condvar),
    /// Stable id for Pulse attribution of every call this engine serves
    /// (one engine per MCP session or CLI command).
    session_id: String,
    /// Per-tool call observability; session counters plus a cross-session
    /// sidecar next to the recovery cache.
    metrics: metrics::ToolMetrics,
    /// Disk-backed seen-set; `None` when session dedup is off.
    session_persist: Option<session_persist::SessionPersistence>,
    /// Expand/read surface health + crash-only recovery unlock (wqw.9).
    /// Shared with CodeMode plan engines so expand outcomes update the same gate.
    /// Lazily opened on first `session_boot_snapshot` so cheap CLI tools do not
    /// pay boot I/O when they never ask for the capsule.
    session_boot: OnceLock<Option<tokenzero_recovery::boot::SessionBoot>>,
    surface_health: std::sync::Arc<surface_health::SurfaceHealth>,
    /// vz89.10 session exposure ledger, shared process-wide per session scope
    /// so per-call CodeMode engines never re-inline bytes the session holds.
    exposure: std::sync::Arc<Mutex<exposure::SessionExposureLedger>>,
    /// Fail-closed append-only response accounting beside the recovery cache.
    ledger: Option<ledger::LedgerWriter>,
    /// Per-connection MCP initialize lifecycle (stdio session / engine).
    pub lifecycle: Mutex<InitializeState>,
}

/// MCP initialize lifecycle for one engine/connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InitializeState {
    #[default]
    Uninitialized,
    /// `initialize` succeeded; waiting for `notifications/initialized`.
    Negotiated,
    /// Client completed initialize + initialized; tools/list and peers allowed.
    Ready,
}
