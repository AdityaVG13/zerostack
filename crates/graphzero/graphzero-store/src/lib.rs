//! Content-addressed, mmap-native snapshot storage for GraphZero.
//! Stores perfect-hash symbols, CSR edges, trigram postings, coverage bitmaps,
//! append-only deltas, bare domain refs, and portable `z://blob` evidence.

use std::fmt;

pub mod store;

pub use store::absence::{AbsenceAnswer, AbsenceCertificate, AbsenceConfig, AnswerClass, absence};
pub use store::blob_store::BlobStore;
pub use store::claim::{
    ClaimCertificate, ClaimKind, ClaimVerifyConfig, ClaimVerifyResult, SurvivingSpan,
    append_verify_evidence_graph, supported_claim_kinds_csv, verify_claim,
};
pub use store::coverage::CoverageBitmap;
pub use store::csr::{CsrAdjacency, CsrBuilder, EdgeKind, ReverseIndex};
pub use store::delta_log::{DeltaEntry, DeltaLog, SEGMENT_MAX_SIZE};
pub use store::durability_receipt::{
    CanonicalSurfaceBytes, DURABILITY_RECEIPT_SCHEMA_VERSION, DurabilityEvidenceInput,
    DurabilityMetadata, DurabilityReceipt, DurabilityReceiptAdapter, DurabilityReceiptExpectation,
    ManifestIdentity, PreparedDurabilityEvidence, REQUIRED_FEEDER_IDS, ReceiptStatus,
};
pub use store::entity::{
    DEDUP_LEDGER_REL, DEDUP_LEDGER_SCHEMA, DEFAULT_ENTITY_REGISTRY_MAX, ENTITY_KEY_VERSION,
    ENTITY_REGISTRY_MAX_ENV, ENTITY_SIDECAR_SCHEMA, EncounterCost, EntityDedupLedger, EntityId,
    EntityKey, EntityKind, EntityNovelty, EntityRecord, EntityRegistry, EntityView, EntityViewKind,
    PublishedEntityIndex, REPEAT_ENCOUNTER_PCT, SymbolSpanMint, clear_entity_registry,
    clear_process_dedup_ledger, dedup_ledger_path, defining_content_digest, entities_file_name,
    entity_for_view, entity_registry_len, entity_registry_max, hydrate_published_entities,
    link_emitted_symbol_view, link_emitted_view, link_view, link_view_from_anchor, lookup_entity,
    lookup_entity_with_store, mint_symbol_span_entity, mint_symbol_spans, process_dedup_ledger,
    read_dedup_ledger, record_dedup_ledger, record_process_dedup_encounter,
    record_process_destination_hit, register_entity_records, repeat_bill, repeat_encounter_pct,
    slice_defining_bytes, try_load_published_entities, write_dedup_ledger,
    write_published_entities,
};
pub use store::entity_novelty_fusion::{
    DEFAULT_SHARED_ENTITY_NOVELTY_MAX, ENTITY_NOVELTY_RECORD_TYPE, ENTITY_NOVELTY_REL_DIR,
    ENTITY_NOVELTY_SCHEMA_VERSION, SHARED_ENTITY_NOVELTY_MAX_ENV, SharedEntityNoveltyRecord,
    flush_entity_novelty, fusion_store_root_from_env, hydrate_entity_novelty,
    merge_shared_entity_novelty, novelty_scope_digest, read_shared_entity_novelty, scope_key_for,
    shared_entity_novelty_max, shared_entity_novelty_path, write_shared_entity_novelty,
};
pub use store::expand::{
    BlobRequest, ExpandError, ExpandErrorKind, ExpandResolver, ExternalResolveError, ExternalStore,
    LocalBlobStoreAdapter, Resolution,
};
pub use store::format::{FORMAT_VERSION, SHARD_MAGIC};
pub use store::frecency::{
    FRECENCY_SCHEMA, FrecencyLedger, PathFrecency, ai_mode as frecency_ai_mode,
    as_of_from_snapshot_nanos, blob_hash_from_ref, combined_entry, load as load_frecency,
    merge_last_commits, now_unix as frecency_now, path_from_evidence_ref, score as frecency_score,
    score_path, touch_evidence, touch_path,
};
pub use store::gc_roots::{
    GC_ENGINE, GC_SCHEMA_VERSION, PinRecord, ReachabilitySnapshot, enumerate_live_blob_hashes,
    pin_record_path, project_id, publish_live_roots, publish_pin_record,
    publish_reachability_snapshot, reachability_snapshot_path, read_reachability_snapshot,
    read_reachability_snapshot_at,
};
pub use store::indexer::{
    GenerationBlast, GenerationBlastDiff, IncrementalCollect, IncrementalCollectStats,
    IncrementalIndex, SpeculativeFileOverlay, collect_changed_paths, collect_with_content_overlays,
    diff_blast_sets_between_generations, index_changed_paths, overlay_from_fszero_edit_cert,
    speculative_blast_from_overlays,
};
pub use store::intent_parse::{IntentParse, parse_intent};
pub use store::manifest::{Manifest, SnapshotEntry};
pub use store::mmr::{InclusionProof, TransparencyLog};
pub use store::perf_profile::{
    PERF_PROFILE_ENV, PERF_PROFILE_SCHEMA, perf_profile_enabled, perf_profile_hypothesis_evaluated,
    perf_profile_run_complete, perf_profile_run_start, perf_profile_sample_collected,
    perf_profile_span_summary, reset_perf_profile_for_tests,
};
pub use store::provenance::{
    ByteSpan, DERIVED_KIND_OUTLINE_SPAN, DERIVED_KIND_QUERY_CAPSULE, DERIVED_KIND_SEMANTIC_CHUNK,
    LineSpan, OrphanedDerivation, PROVENANCE_OPT_IN_ENVS, PROVENANCE_SCHEMA_VERSION,
    ProvenanceDoctorReport, ProvenanceRecord, RECORD_TYPE_DERIVATION, TRANSFORM_CAPSULE_BUILD,
    TRANSFORM_INDEXER_SHARD_EDGES, TRANSFORM_OUTLINE_EXTRACT_SPANS,
    TRANSFORM_OVERLAY_EXTRACT_EDGES, TRANSFORM_SEMANTIC_EXTRACT_CHUNKS,
    attach_capsule_build_provenance, attach_def_span_provenance,
    attach_indexer_shard_edge_provenance, attach_outline_span_provenance,
    attach_overlay_edge_provenance, attach_semantic_chunk_provenance, find_orphaned_derivations,
    list_provenance_records, lookup_by_derived_ref, provenance_doctor_report, provenance_enabled,
    read_provenance_record, why_for_evidence_ref, write_provenance_record,
};
pub use store::publish::{
    MAX_BATCH_BYTES, MAX_EDGES, PUBLISH_SCHEMA_VERSION, PublishAck, PublishError, PublishOptions,
    capability_ok, confidence_to_u8, install_capability_token, map_publish_kind, publish_batch,
    publish_schema_json, validate_batch_json,
};
pub use store::query::{
    BudgetLedger, DestinationRef, FileTargetHit, LocateCapsule, LocateKind, NAME_BIGRAM_MAGIC,
    NAME_BIGRAM_VERSION, NameBigramIndex, QueryCapsule, QueryEngine, SEARCH_BIGRAM_ENV, SnapRoute,
    Snapshot, TARGET_CONTEXT_LINES, TARGET_INLINE_TOP_HITS, canonical_ref_for_loc,
    encode_edge_with_meta, file_target_for_evidence, locate, locate_shell, name_bigram_file_name,
    render_target, savings_tokens_after_expand, search_bigram_enabled, snap, span_range,
};
pub use store::refs::GzRef;
pub use store::schema_version::{
    AdmitOutcome, GRAPHZERO_STORE_SCHEMA_MAJOR, GRAPHZERO_STORE_SCHEMA_MINOR, SNAPSHOT_SCHEMA_FILE,
    SchemaVersionError, SchemaVersionRefuseReason, SchemaVersionStamp, SnapshotSchemaSegment,
    StoreSegmentKind, admit_current, admit_fingerprint_stamp, admit_read,
    admit_snapshot_schema_stamp, current_store_stamp, store_writer_version,
    write_snapshot_schema_stamp,
};
pub use store::session::{
    DEFAULT_SESSION_NOVELTY_MAX, DEFAULT_SESSION_SCOPES_MAX, DEFAULT_SESSION_SEEN_MAX,
    DEFAULT_SESSION_TRACE_EVENTS_MAX, EntityAwareSeenProvider, LocalSeenProvider,
    LocalTraceProvider, SESSION_NOVELTY_MAX_ENV, SESSION_SCOPES_MAX_ENV, SESSION_SEEN_MAX_ENV,
    SESSION_TRACE_EVENTS_MAX_ENV, SOURCE_TRACE_INGEST, SeenKey, SeenProvider, SeenScope,
    SeenStatus, SessionDedupStats, SessionLedger, SessionLedgerStats, TRACE_SCHEMA, TraceEvent,
    TraceProvider, apply_seen_to_destinations, clear_session_state, default_seen_provider,
    default_trace_provider, ingest_traces_and_reindex, ingest_traces_into_index,
    session_novelty_max, session_scopes_max, session_seen_max, session_trace_events_max,
};
pub use store::shard::{ShardBuilder, ShardReader};
pub use store::shared_cas::{CAS_MAX_OBJECT_BYTES, CAS_TEMP_REAP_AGE, SharedCas};
pub use store::stage_hist::{
    STAGE_HISTOGRAM_ENV, StageHistSummary, record_dispatch_phases, record_index_phases,
    record_op_stages, record_open_phases, record_stage_ms, reset_stage_histograms,
    stage_hist_snapshot, stage_histogram_enabled,
};
pub use store::symbol_table::SymbolTable;

pub use store::telemetry::{
    LOCAL_COUNTERS_REL, LOCAL_COUNTERS_SCHEMA, LocalTokenCounters, TELEMETRY_CONFIG_KEY,
    TELEMETRY_ENV, TELEMETRY_EXPORTER, TELEMETRY_SCHEMA, TelemetryInspection, TelemetryPayload,
    export_shareable_telemetry, inspect_telemetry, inspection_json, load_telemetry_config,
    local_counters_path, read_local_counters, record_local_tokens, resolve_telemetry,
    shareable_payload_from_counters, telemetry_env_enabled, telemetry_from_config_value,
    write_local_counters,
};
pub use store::tier_b;
pub use store::usage_telemetry::{
    ExecutionPath, USAGE_TELEMETRY_REL, UsageRecord, UsageTelemetryError, UsageTelemetryInspection,
    inspect_usage_telemetry, record_usage, usage_telemetry_enabled, usage_telemetry_path_for_store,
};
pub use store::zeroref::{ZeroFragment, ZeroRef, ZeroRefError, ZeroRefErrorClass, ZeroScheme};

pub use store::memory::{
    MemoryExport, MemoryFact, MemoryHint, MemoryIndex, MemoryKind, RememberInput,
    attach_memory_to_skeleton, export_memory, format_recall_budget_one, import_memory, load_fact,
    mem_dir, mem_ref, remember_fact,
};
/// Content-addressed blob identifier (sha256 hex digest).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobId(pub String);

impl BlobId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl AsRef<str> for BlobId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

pub use graphzero_types::ContentHash;
pub use graphzero_types::{fast_hex, fast_hex_32};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlobHashIndexError {
    pub blob_idx: u32,
    pub blob_hash_count: usize,
}

impl fmt::Display for BlobHashIndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "blob_idx {} out of range for {} blob hashes",
            self.blob_idx, self.blob_hash_count
        )
    }
}

impl std::error::Error for BlobHashIndexError {}

pub fn hex_blob_hash(blob_hashes: &[[u8; 32]], idx: u32) -> Result<String, BlobHashIndexError> {
    let hash = blob_hashes.get(idx as usize).ok_or(BlobHashIndexError {
        blob_idx: idx,
        blob_hash_count: blob_hashes.len(),
    })?;
    Ok(fast_hex_32(hash))
}

/// Tier enumeration used across GraphZero.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    A = 0, // tree-sitter syntactic
    B = 1, // SCIP / LSP semantic
    C = 2, // git empirical
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tier::A => write!(f, "A"),
            Tier::B => write!(f, "B"),
            Tier::C => write!(f, "C"),
        }
    }
}
