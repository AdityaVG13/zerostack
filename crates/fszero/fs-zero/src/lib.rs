// FSZero: token-efficient filesystem layer for agents.
//
// Process exclusivity (fszero-ncib.3): at most one of surface-mcp / surface-codemode
// may be enabled. Dual feature selection fails at compile time in packaging/.
pub mod codemode;
pub mod core;
pub mod mcp_protocol;
pub mod mcp_rpc;
pub mod packaging;
#[cfg(feature = "dev-harness")]
pub mod surface_bench;
pub mod surfaces;

pub use codemode::{
    ERROR_REF, FsConnector, ParallelBranch, ParallelOnError, PlanStep, RESULT_REF, STEPS_REF,
    TELEMETRY_REF, TransactionJournal, TransactionMode, ack_with_refs, classify_error,
    codemode_tool_refs_for_describe, codemode_tool_refs_for_plan, codemode_tool_refs_for_search,
    describe_signature as codemode_describe_signature,
    discovery_describe as codemode_discovery_describe,
    discovery_search as codemode_discovery_search, execute_plan as codemode_execute_plan,
    finish_error, parse_program, program_has_mutations, try_recipe_with_session, validate_program,
};
pub use core::MAX_TRANSIENT_PAYLOADS;
pub use core::{
    CANDIDATE_DELTA_PROTOCOL, CANDIDATE_MAX_CHAIN, CANDIDATE_OPERATOR_ID,
    CANDIDATE_OPERATOR_VERSION, CANDIDATE_SCHEMA, CAPABILITY_STORE_KEY, CancellationSemantics,
    CandidateCost, CandidateDelta, CandidateDiagnostic, CandidateError, CandidateRecord,
    CandidateStore, CapabilityRequirement, CasError, CasPutOutcome, CasStore, CostClass,
    DOCTOR_SCHEMA, DeltaOp, DispatchOutcome, DispatchProfile, DispatchSurface, DoctorDiagnostic,
    DoctorReport, DoctorSeverity, DomainError, DomainResult, FAIL_OPEN_AFTER,
    FILESYSTEM_CONTRACT_JSON, FILESYSTEM_CONTRACT_MAJOR, FILESYSTEM_CONTRACT_MINOR,
    FILESYSTEM_CONTRACT_NAME, FILESYSTEM_CONTRACT_STORE_KEY, FILESYSTEM_CONTRACT_VERSION,
    FSZeroSession, FilesystemContractError, FsZeroStore, GC_ENGINE_FSZERO,
    GC_RECORD_TYPE_REACHABILITY, GC_SCHEMA_VERSION, GcRootsPublish, GlobalStoreMigrationReport,
    HandshakeAck, HandshakeRequest, IndexBuildReport, IndexRefreshReport, JOURNAL_DELTA_VERSION,
    JournalByteRange,
    JournalDelta, JournalDeltaOp, LOCAL_COUNTERS_SCHEMA, LineSpan, LocalTokenCounters,
    LockWaitSnapshot, MIGRATION_MANIFEST_KEY, MaterializeOutcome, MemoEntry, MemoError, MemoOp,
    MemoOutcome, MemoRequest, MigrationReport, Mutability, OP_MEMO_KEY_VERSION, OP_MEMO_SCHEMA,
    OPCODE_MAP_HINT, OPERATION_ABI_DIGEST_ALGORITHM, OPERATION_ABI_NAME,
    OPERATION_ABI_SCHEMAS_JSON, OPERATION_ABI_SCHEMAS_NAME, OPERATION_ABI_SCHEMAS_VERSION,
    OPERATION_ABI_STORE_KEY, OPERATION_ABI_VERSION, OPERATION_REGISTRY, OpCode, OpMemoStore,
    Operation, OperationArgs, Ownership, PrivateRawWorker, RAW_WORKER_VERSION, REF_SCHEME,
    REF_VERSION, RecoveryStore, RefOwnership, RuntimeHealth, SEMANTIC_CONTRACT_NAME,
    STDERR_RING_BYTES, SURFACE_MANIFEST_SCHEMA, SelectedSurface, Side as CandidateSide,
    SnapshotGcEntry, StoreAdoptionReport, StoreGcPlan, SubstrateChildConfig, SubstrateDown,
    SupervisedChild, SurfaceCapability, TELEMETRY_ENV, TELEMETRY_EXPORTER, TELEMETRY_SCHEMA,
    TelemetryInspection, TelemetryPayload, ToolIdentity, WatchEvent, WatchReconcileState,
    WatchStats, WorkerRequestFrame, WorkerResponseFrame, WorkerTrace, adopt_superseded_store,
    candidate_render_diagnostics, classify_detail_to_error_class,
    client_native_raw_worker_capability, codemode_method_schema_entry, codemode_tool_schema_entry,
    contract_digest_hex, dispatch_batch, dispatch_codemode_method, dispatch_count, dispatch_doctor,
    dispatch_edit_parts, dispatch_mcp_tool, dispatch_migrate_cas, dispatch_opcode,
    dispatch_operation, dispatch_raw_worker, dispatch_world_query, doctor_diagnostics,
    domain_operation_schemas, duplicate_serialization_detected, error_class_retryable,
    estimate_visible_tokens, exact_schema_parity, export_shareable_telemetry,
    filesystem_contract_descriptor, filesystem_contract_operation_names, fszero_store_sqlite_path,
    gc_project_id, global_metadata_fallback_disabled, inspect_telemetry, inspection_json,
    integrate_journal_deltas, last_dispatch_profile, last_serialize_bytes,
    live_codemode_matches_registry, live_mcp_matches_registry, load_telemetry_config,
    local_capability, lock_metrics_for_test, lock_wait_snapshot, materialize_codemode_tools,
    materialize_mcp_tools, mcp_tool_schema_entry, migrate_legacy_global_store,
    negotiate_filesystem_contract, negotiate_shared_interop, normalize_schema,
    opcode_for_operation, operation_abi_descriptor, operation_abi_digest,
    operation_abi_schemas_digest, operation_abi_schemas_document, operation_by_id,
    operation_for_opcode, operation_ids, operation_is_dispatchable, operation_name_map,
    outer_router_raw_worker_capability, parse_exec_opcode, private_worker_dispatch_checked,
    private_worker_source_forbids_sandbox, process_start_count, publish_fszero_gc_roots,
    record_durable_open_busy_wait, record_index_lock_wait, record_local_tokens,
    record_process_start, record_serialization, recovery_key_for_opcode, registry_cli_opcodes,
    registry_codemode_aliases, registry_mcp_aliases, registry_mcp_input_keys, repo_metadata_dir,
    repo_store_sid, reset_runtime_metrics, resolve_alias, resolve_telemetry, serialization_count,
    shareable_payload_from_counters, snapshot_retention_budget, store_gc_apply, store_gc_plan,
    store_id_for_db_path, store_id_for_path, store_is_global_host, take_lock_wait_snapshot,
    take_process_starts, take_serializations, telemetry_env_enabled, telemetry_from_config_value,
    telemetry_store_root, validate_codemode_method_schemas, validate_filesystem_contract_document,
    validate_handshake, validate_live_codemode_tool_catalog, validate_live_mcp_catalog,
    validate_mcp_tool_schema, validate_operation_abi, validate_operation_abi_schemas,
    validate_peer_descriptor, wire_arg_for_operation, write_local_counters,
    zerostack_store_or_detect,
};
#[cfg(feature = "dev-harness")]
pub use surface_bench::{
    BenchSurface, BenchWorkload, TrialResult, WIRE_RATCHET_MIN_SAMPLES, WIRE_RATCHET_MULTIPLIER,
    WIRE_RATCHET_SCHEMA, bench_provenance, codemode_in_process_scope_diagnostic,
    evaluate_absolute_thresholds, evaluate_wire_ratchet, evidence_document, run_surface_trials,
    wire_evidence_document,
};

/// Doctor smoke: list + read against `root`.
///
/// Prefer CodeMode JS when `surface-codemode` is compiled; otherwise use
/// domain opcodes so `fszero-mcp` doctor never requires the hub interpreter.
/// Healthy binary / healthy session returns `Ok(ack)`; failures return `Err`.
pub fn doctor_smoke_plan(root: &std::path::Path) -> Result<String, String> {
    let mut sess = FSZeroSession::with_repo_store(root);
    #[cfg(feature = "surface-codemode")]
    {
        // Explicit fs.ls + fs.read — Acceptance: "doctor smoke list/read plan".
        let plan = r#"
        const listing = await fs.ls({});
        if (!listing || listing.ok === false) throw new Error('list failed');
        const read = await fs.read({ path: 'CHANGELOG.md' });
        if (!read || read.ok === false) {
          const alt = await fs.read({ path: 'Cargo.toml' });
          if (!alt || alt.ok === false) throw new Error('read failed');
          return { listing, read: alt }; }
        return { listing, read };
    "#;
        let out = codemode_execute_plan(&mut sess, plan);
        if out == "C" {
            return Ok(out);
        }
        if out == "X0" || out.starts_with('X') {
            let detail = sess
                .expand("codemode/error")
                .or_else(|| sess.expand(ERROR_REF))
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_else(|| out.clone());
            return Err(format!("doctor smoke failed: {detail}"));
        }
        return Err(format!("doctor smoke unexpected ack: {out}"));
    }
    #[cfg(not(feature = "surface-codemode"))]
    {
        let (ack_l, ok_l, detail_l) = sess.execute('L', Some("."));
        if !ok_l {
            return Err(format!("doctor smoke list failed: {ack_l} {detail_l:?}"));
        }
        let path = if root.join("CHANGELOG.md").is_file() {
            "CHANGELOG.md"
        } else {
            "Cargo.toml"
        };
        let (ack_r, ok_r, detail_r) = sess.execute('R', Some(path));
        if !ok_r {
            return Err(format!("doctor smoke read failed: {ack_r} {detail_r:?}"));
        }
        Ok(format!("{ack_l}+{ack_r}"))
    }
}

/// Doctor root diagnosis: workspace root (FS ops) vs durable store root.
/// Used by `fszero doctor` so multi-project misconfig is visible without a debugger.
pub fn doctor_root_report(root: &std::path::Path) -> serde_json::Value {
    let sess = FSZeroSession::with_repo_store(root);
    sess.root_report()
}
#[cfg(feature = "mcp-http")]
pub use mcp_protocol::HttpMcpServer;
pub use mcp_protocol::{
    McpHandler, PROTOCOL_2025, PROTOCOL_LEGACY, PROTOCOL_RC, SUPPORTED_VERSIONS, SurfaceKind,
    TransportProfile, assert_server_surface_boundary, raw_worker_call_once, raw_worker_requested,
    resolve_codemode_response, run_fastmcp_server, run_raw_worker_stdio, run_stdio_server,
    supports_handshake_and_call_frames, tools_list_for_surface,
};
pub use mcp_rpc::{
    JSON_SCHEMA_2020_12, TOOLS_LIST_TTL_MS, install_explicit_root, parse_root_flag,
    resolve_cli_root, resolve_root,
};
#[cfg(feature = "dev-harness")]
pub use packaging::release_smoke::{
    ReleaseSmokeReport, dual_surface_temp_prefix_smoke, ensure_surface_bin,
    ensure_surface_bin_increments_process_starts, smoke_one_surface,
};
pub use packaging::surface_bin::{run_batch_command, run_surface_bin};
pub use packaging::waiver::{
    Waiver, WaiverError, load_and_validate_waivers, parse_active_waivers,
    validate_waivers_not_expired,
};
pub use packaging::{
    ALLOW_BARE_SERVER_ENV, ARTIFACT_CODEMODE, ARTIFACT_MCP, ARTIFACT_SHIM, COMMON_FLAGS,
    COMPLETION_SHELLS, ClientConfig, InstallState, PackageSurface, SHIM_COMMANDS, args_has,
    assert_surface_compiled, baked_package_surface, bare_server_opt_in, capabilities_document,
    client_config_for, compile_time_surfaces, completion_script, current_platform,
    default_install_prefix, did_you_mean_suffix, die1, die2, dual_surface_diagnostic,
    edit_distance_at_most_one, exit_install_result, exit_uninstall_from_args,
    exit_uninstall_result, explicit_server_request, install_surface, is_bare_invocation,
    layout_document, load_install_state, modes_from_args, nearest_names, package_identity,
    parse_binary_flag, parse_prefix_flag, parse_surface_flag, print_install_ok, print_version_line,
    reject_dual_env_selection, resolve_selected_binary, resolve_startup_surface,
    resolve_surface_binary, robot_docs_guide, robot_triage_document, sbom_document,
    semantic_contract_digest, shim_should_start_server, surface_compiled_in, uninstall_report,
    uninstall_surface, version_flag_requested, write_client_config,
};

// RACC-R exact-bytes surfaces (fszero-vk1q chain).
pub use crate::core::{
    AtomicPublication, CAS_DIR_NAME, CAS_LAYOUT_VERSION, CrashPoint, DEFAULT_FILE_MODE,
    DeoptRestoreReceipt, DurabilityCaseResult, DurabilityMatrixReport, EffectMutation,
    EvidencePage, ExactRange, ExactSnapshot, NonsemanticExclusion, Overlay, PublicationStage,
    RawBaselineSafepoint, RefFate, SnapshotEntry, SuccessorMap, ToolchainContract, cas_layout,
    hub_shared_cas, realize_effects, rehydrate_from_safepoint, run_durability_matrix,
    snapshot_root_digest, store_root_has_unexpanded_tilde, toolchain_contract_digest,
};

pub use crate::core::{TemporalHit, TemporalQuery, is_zero_token_recall, recall_mutations};

pub use crate::core::{
    CERT_SCHEMA, FreshnessError, JournalFreshnessCertificate, JournalMutation, verify_freshness,
};

pub use crate::core::{
    ArtifactClass, CanonicalizeError, canonicalize, canonicalize_orient_pack,
    canonicalize_repo_map, canonicalize_search_results,
};

pub use crate::core::{AsofError, AsofJournal, AsofMutation};

pub use crate::core::{SessionDiff, SessionSnapshot, SessionWorld};

pub use crate::core::{VirtualOverlay, VirtualOverlayError};

pub use crate::core::{
    AlreadyReadTracker, NegativeCache, NegativeEntry, ReadPage, STORE_SCHEMA_VERSION, SchemaSkew,
    check_schema_skew, page_bytes, page_lines,
};

pub use crate::core::{
    ConeMemoEntry, ConeMemoStore, JournalBookmark, JournalBookmarks, LineClass, StaleEditError,
    check_line_anchors, classify_line, line_hash,
};

pub use crate::core::{StorePackError, export_cas_pack, import_cas_pack};

pub use crate::core::snapshot_from_files;

pub use crate::core::safepoint_for_snapshot;

pub use crate::core::{FuzzyHit, fuzzy_fallback, is_weak_match};

pub use crate::core::{OkfDocument, SearchCursorStore, SearchPage, parse_okf, wiki_links};

pub use crate::core::{
    BudgetHit, BudgetTracker, ExternalEditDetector, ExternalEffectDisposition,
    ExternalEffectReceipt, FrecencySignals, OpBudgets, WouldHaveHit, WouldHaveHitLedger,
    frecency_score, truncate_bytes,
};

pub use crate::core::{
    ExecShape, FusionPlan, choose_exec_shape, dedupe_execute, plan_search_ast_fusion,
};
