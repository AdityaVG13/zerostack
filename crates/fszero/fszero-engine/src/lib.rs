//! FSZero kernel — session, ops, recovery, search.

#![allow(unused_imports)]

use std::fs;
use std::io::{Read as _, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::time::{Instant, SystemTime};

pub mod access_world_ops;
mod already_read;
mod asof_snapshot;
mod ast;
mod ast_sgrep;
mod batch_cse;
mod batch_evidence;
mod batch_ops;
mod budget;
pub mod candidate_store;
#[cfg(feature = "fszero-ast-sgrep")]
mod multi_ast_search;
pub use fszero_core::{MutationOutcome, MutationState};
pub use fszero_core::{
    canonicalize, edit_spec, filesystem_contract, hashline, hexutil, line_class, mutation_outcome,
    operation_abi, operation_schemas, raw_worker_protocol, target_ref, zeroref,
};
pub use fszero_store::{
    access_log, ast_store, cas, cdc, journal_delta, memory, path, recovery, runtime_metrics,
    store_migration, store_pack, store_schema_version, zerostack_store,
};
pub mod capability;
mod compound_ops;
mod cone_eviction;
pub mod dispatcher;
pub mod doctor;
mod effect_capture;
mod embedded_store;
mod external_edit;
mod frecency;
mod fs_ops;
mod fuzzy_fallback;
mod journal_bookmark;
mod journal_freshness;
mod list_ops;
mod multi_list;
mod negative_cache;
mod okf_memory;
mod op_budgets;
pub mod op_memo;
mod op_result;

pub mod racc;
pub mod raw_worker;
mod read_ops;
mod read_pagination;
mod readonly_ops;
pub mod residency_probe;
mod search_cursor;
mod session_world;
pub mod surface_handshake;
mod temporal_recall;
mod virtual_overlay;
mod would_have_hit;

mod ref_resolver;
pub mod resolve;
mod resolve_ident;
pub mod runtime_health;
mod search;
/// Bigram/memmem prefilter (fszero-9yq/9ot/up8/kbo; default-on after kbo).
pub mod search_prefilter_eval;
#[cfg(feature = "fszero-semantic-local")]
pub mod semantic_local;
mod session;
pub mod substrate_child;
mod subsystems;
#[cfg(test)]
mod target_ref_proof;
pub mod telemetry;
pub mod usage_telemetry;
mod verified_edit;
mod watch;
mod world;
pub mod zero_kernel;
pub mod zeroref_fixture;

pub use candidate_store::{
    CANDIDATE_DELTA_PROTOCOL, CANDIDATE_MAX_CHAIN, CANDIDATE_OPERATOR_ID,
    CANDIDATE_OPERATOR_VERSION, CANDIDATE_SCHEMA, CandidateCost, CandidateDelta,
    CandidateDiagnostic, CandidateError, CandidateRecord, CandidateStore, DeltaOp, LineSpan,
    MaterializeOutcome, Side, render_diagnostics as candidate_render_diagnostics,
};
pub use capability::{CAPABILITY_STORE_KEY, negotiate_shared_interop, validate_peer_descriptor};
pub use cas::{
    CAS_DIR_NAME, CAS_LAYOUT_VERSION, CasError, CasPutOutcome, CasStore, GC_ENGINE_FSZERO,
    GC_RECORD_TYPE_REACHABILITY, GC_SCHEMA_VERSION, GcRootsPublish, cas_layout, gc_project_id,
    gc_roots_current_path, hub_shared_cas, publish_fszero_gc_roots,
    store_root_has_unexpanded_tilde,
};
pub use dispatcher::structured_world_arg;
pub use dispatcher::{
    DispatchOutcome, DispatchProfile, DispatchSurface, dispatch_batch, dispatch_codemode_method,
    dispatch_count, dispatch_doctor, dispatch_edit_parts, dispatch_mcp_tool, dispatch_migrate_cas,
    dispatch_opcode, dispatch_operation, dispatch_raw_worker, dispatch_world_query,
    last_dispatch_profile, opcode_for_operation, operation_for_opcode, operation_is_dispatchable,
    recovery_key_for_opcode, wire_arg_for_operation,
};
pub use doctor::{
    DOCTOR_SCHEMA, DoctorDiagnostic, DoctorReport, DoctorSeverity, doctor_diagnostics,
};
pub use embedded_store::FsZeroStore;
pub use filesystem_contract::{
    FILESYSTEM_CONTRACT_JSON, FILESYSTEM_CONTRACT_MAJOR, FILESYSTEM_CONTRACT_MINOR,
    FILESYSTEM_CONTRACT_NAME, FILESYSTEM_CONTRACT_STORE_KEY, FILESYSTEM_CONTRACT_VERSION,
    FilesystemContractError, filesystem_contract_descriptor, filesystem_contract_operation_names,
    negotiate_filesystem_contract, validate_filesystem_contract_document,
};
pub use journal_delta::{
    JOURNAL_DELTA_VERSION, JournalByteRange, JournalDelta, JournalDeltaOp, integrate_journal_deltas,
};
pub use memory::{decode_wire_path, encode_wire_path, memory_put_wire, memory_rename_wire};
pub use memory::{delete_memory, get_memory, put_memory};
pub use op_memo::{
    MemoEntry, MemoError, MemoOp, MemoOutcome, MemoRequest, OP_MEMO_KEY_VERSION, OP_MEMO_SCHEMA,
    OpMemoStore, ToolIdentity,
};
pub use op_result::visible_ack;
pub use operation_abi::{
    CancellationSemantics, CapabilityRequirement, CostClass, DomainError, DomainResult, Mutability,
    OPERATION_ABI_DIGEST_ALGORITHM, OPERATION_ABI_NAME, OPERATION_ABI_STORE_KEY,
    OPERATION_ABI_VERSION, OPERATION_REGISTRY, Operation, OperationArgs, RefOwnership,
    classify_detail_to_error_class, error_class_retryable, live_codemode_matches_registry,
    live_mcp_matches_registry, operation_abi_descriptor, operation_abi_digest, operation_by_id,
    operation_ids, operation_name_map, registry_cli_opcodes, registry_codemode_aliases,
    registry_mcp_aliases, registry_mcp_input_keys, resolve_alias, validate_mcp_tool_schema,
    validate_operation_abi,
};
pub use operation_schemas::{
    JSON_SCHEMA_2020_12, OPERATION_ABI_SCHEMAS_JSON, OPERATION_ABI_SCHEMAS_NAME,
    OPERATION_ABI_SCHEMAS_VERSION, codemode_method_schema_entry, codemode_tool_schema_entry,
    domain_operation_schemas, exact_schema_parity, materialize_codemode_tools,
    materialize_mcp_tools, mcp_tool_schema_entry, normalize_schema, operation_abi_schemas_digest,
    operation_abi_schemas_document, validate_codemode_method_schemas,
    validate_live_codemode_tool_catalog, validate_live_mcp_catalog, validate_operation_abi_schemas,
};
pub use raw_worker::{RawWorker, resolve_worker_revision};
pub use raw_worker_protocol::{
    DEFAULT_MAX_FRAME_BYTES as RAW_WORKER_MAX_FRAME_BYTES, RAW_WORKER_PROTOCOL_VERSION,
};
pub use readonly_ops::is_parallel_branch_safe;
pub use readonly_ops::{
    ParallelBranchWork, ParallelReadContext, execute_parallel_branch, execute_search_branch,
};
pub use recovery::{
    CacheMissCause, CacheMissCauseSnapshot, ChunkIndexReport, ChunkInvalidation, HotSqlEntry,
    MAX_TRANSIENT_PAYLOADS, MIGRATION_MANIFEST_KEY, MigrationReport, PreparedCacheMetrics,
    RecoveryStore, SnapshotGcEntry, SqlExplainCapture, SqlProfileRow, StoreGcPlan, StoredChunk,
    capture_hot_sql_explains, ensure_prepared_cache_profile, hot_sql_catalog,
    maybe_capture_sql_explains, prepared_cache_metrics, prepared_cache_metrics_json,
    prepared_cache_profile_env_enabled, reset_sql_profile, snapshot_retention_budget,
    sql_explain_env_enabled, sql_explain_status_json, sql_profile_env_enabled, sql_profile_json,
    sql_profile_top, store_gc_apply, store_gc_plan, write_sql_explain_artifacts,
};
pub use recovery::{unix_epoch_millis, unix_epoch_nanos, unix_epoch_secs};
pub use runtime_health::{FAIL_OPEN_AFTER, RuntimeHealth};
pub use runtime_metrics::{
    LockWaitSnapshot, duplicate_serialization_detected, last_serialize_bytes,
    lock_metrics_for_test, lock_wait_snapshot, process_start_count, record_durable_open_busy_wait,
    record_index_lock_wait, record_process_start, record_serialization, reset_runtime_metrics,
    serialization_count, take_lock_wait_snapshot, take_process_starts, take_serializations,
};
pub use session::{
    FSZeroSession, OPCODE_MAP_HINT, OpCode, estimate_visible_tokens, parse_exec_opcode,
};
pub use session::{ReadCacheEntry, ReadViewMeta};
pub use store_migration::{
    GlobalStoreMigrationReport, StoreAdoptionReport, adopt_superseded_store,
    global_metadata_fallback_disabled, migrate_legacy_global_store,
};
pub use substrate_child::{
    STDERR_RING_BYTES, SubstrateChildConfig, SubstrateDown, SupervisedChild,
};
pub use subsystems::{IndexBuildReport, IndexRefreshReport};
pub use surface_handshake::{
    HandshakeAck, HandshakeRequest, Ownership, PrivateRawWorker, RAW_WORKER_VERSION, REF_SCHEME,
    REF_VERSION, SEMANTIC_CONTRACT_NAME, SURFACE_MANIFEST_SCHEMA, SelectedSurface,
    SurfaceCapability, WorkerRequestFrame, WorkerResponseFrame, WorkerTrace,
    client_native_raw_worker_capability, contract_digest_hex, local_capability,
    outer_router_raw_worker_capability, private_worker_dispatch_checked,
    private_worker_source_forbids_sandbox, validate_handshake,
};
pub use telemetry::{
    LOCAL_COUNTERS_SCHEMA, LocalTokenCounters, TELEMETRY_ENV, TELEMETRY_EXPORTER, TELEMETRY_SCHEMA,
    TelemetryInspection, TelemetryPayload, export_shareable_telemetry, inspect_telemetry,
    inspection_json, load_telemetry_config, record_local_tokens, resolve_telemetry,
    shareable_payload_from_counters, telemetry_env_enabled, telemetry_from_config_value,
    telemetry_store_root, write_local_counters,
};
pub use usage_telemetry::{
    ExecutionPath, UsageRecord, UsageTelemetryError, UsageTelemetryInspection,
    inspect_usage_telemetry, record_codemode_accounting, record_mcp_accounting,
    record_opt_in_visible_accounting, record_usage, usage_telemetry_enabled,
    usage_telemetry_path_for_cache,
};
pub use watch::{WatchEvent, WatchReconcileState, WatchStats};
pub use world::{world_arg_creates, world_arg_is_staging, world_arg_mutates};
pub use zeroref_fixture::{
    ExpandResult, FIXTURE_SCHEMA, FixtureError, ZEROREF_EXIT_CLASSES, ZerorefFixtureAction,
    binary_identity, error_diag, exit_code_dictionary, exit_code_for_class, parse_args,
    run_descriptor, run_expand, run_put, run_put_bytes,
};
pub use zerostack_store::{
    effective_root_mode, fszero_store_sqlite_path, repo_metadata_dir, repo_store_sid,
    store_id_for_db_path, store_id_for_path, store_is_global_host, store_root_from_db_path,
    zerostack_store_or_detect,
};

pub use budget::env_usize;
pub use path::guard_write_target_parent;
pub use path::{
    atomic_write, canonicalize_root, ensure_path_under_root, file_meta_snapshot, mtime_ns_of,
    resolve_existing_path, restore_xattrs, revalidate_path_under_root_canon, set_mode,
    set_mtime_ns, validate_rollback_path, xattrs_of,
};
pub use search::parse_budget_message;
pub use search_prefilter_eval::literal_physical_scan_count;

pub fn kernel_visible_error(res_str: &str, op: OpCode) -> String {
    op_result::classify_op_result(res_str).visible_error(op)
}

pub use racc::{
    AtomicPublication, CrashPoint, DEFAULT_FILE_MODE, DeoptRestoreError, DeoptRestoreReceipt,
    DurabilityCase, DurabilityCaseResult, DurabilityMatrixReport, EffectMutation, EvidencePage,
    EvidencePageError, ExactRange, ExactSnapshot, FileRecord, JournalRecord, NonsemanticExclusion,
    Overlay, OverlayError, PublicationStage, RawBaselineSafepoint, RefFate, SafepointError,
    SnapshotEntry, SnapshotError, SuccessorMap, SuccessorMapError, ToolchainContract,
    realize_effects, record_path_move, rehydrate_from_safepoint, run_durability_matrix,
    safepoint_for_snapshot, snapshot_from_files, snapshot_root_digest, toolchain_contract_digest,
};

pub use temporal_recall::{TemporalHit, TemporalQuery, is_zero_token_recall, recall_mutations};

pub use journal_freshness::{
    CERT_SCHEMA, FreshnessError, JournalFreshnessCertificate, JournalMutation, verify_freshness,
};

pub use canonicalize::{
    ArtifactClass, CanonicalizeError, canonicalize, canonicalize_orient_pack,
    canonicalize_repo_map, canonicalize_search_results, digest_hex as canonicalize_digest_hex,
};

pub use asof_snapshot::{AsofError, AsofJournal, AsofMutation, blob_ref as asof_blob_ref};

pub use session_world::{
    SessionDiff, SessionSnapshot, SessionWorld, digest_hex as session_digest_hex,
};

pub use virtual_overlay::{VirtualOverlay, VirtualOverlayError};

pub use already_read::AlreadyReadTracker;
pub use negative_cache::{NegativeCache, NegativeEntry};
pub use read_pagination::{ReadPage, page_bytes, page_lines};
pub use store_schema_version::{STORE_SCHEMA_VERSION, SchemaSkew, check_schema_skew};

pub use cone_eviction::{ConeMemoEntry, ConeMemoStore};
pub use hashline::{StaleEditError, check_line_anchors, file_line_hashes, line_hash};
pub use journal_bookmark::{JournalBookmark, JournalBookmarks};
pub use line_class::{LineClass, classify_line};

pub use store_pack::{PACK_SCHEMA, StorePackError, export_cas_pack, import_cas_pack};

pub use fuzzy_fallback::{FuzzyHit, edit_distance, fuzzy_fallback, is_weak_match};

pub use okf_memory::{
    OkfDocument, content_hash as okf_content_hash, is_stale as okf_is_stale, parse_okf, wiki_links,
};
pub use search_cursor::{SearchCursorStore, SearchPage};

pub use effect_capture::{EffectAction, EffectPath, EffectRecord, EffectScope};
pub use external_edit::{
    ExternalEditDetector, ExternalEffectDisposition, ExternalEffectReceipt,
    FileSig as ExternalFileSig,
};
pub use frecency::{FrecencySignals, frecency_score, rank_paths};
pub use op_budgets::{BudgetHit, BudgetTracker, OpBudgets, truncate_bytes};
pub use would_have_hit::{WouldHaveHit, WouldHaveHitLedger};

pub use batch_cse::{
    ExecShape, FusionPlan, choose_exec_shape, dedupe_execute, plan_search_ast_fusion,
};

pub use zero_kernel::ZeroFileEngine;
