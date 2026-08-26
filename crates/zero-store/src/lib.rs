#![deny(unsafe_code)]

//! Canonical ZeroRef content-addressed store layout, publish protocol,
//! store-root resolution, and collection coordination.
//!
//! Layout: <store_root>/blobs/sha256/<first-two-hex>/<64-lowercase-hex>,
//! immutable complete objects only. Engine facts, indexes, provenance, and
//! mutable metadata never live in this namespace.
//!
//! The publish protocol is crash-safe and concurrency-safe: unique sibling
//! temp file, sync, atomic rename, directory sync. Identical concurrent
//! writers converge on one valid object; a preexisting object with different
//! bytes is a loud corruption error and is never overwritten.
//!
//! Publishing additionally holds the shared store coordination lock, and
//! removal requires the exclusive one, so a collector's liveness recheck and
//! its unlink cannot be split by a concurrent publisher.

mod attempt_journal;
mod cas;
mod durable_journal;
mod event_log;
mod fs_replace;
mod gc;
mod gc_lock;
mod idle_gate;
mod metadata;
mod migrations;
mod scrub;
mod session_wal;
mod snapshot;
mod store_root;
mod zbf;
mod zero_cas;
mod zero_migration;

pub use attempt_journal::{
    ATTEMPT_BINDING_SCHEMA_VERSION, ATTEMPT_JOURNAL_MAX_ENTRIES, ATTEMPT_JOURNAL_MAX_RECORD_BYTES,
    ATTEMPT_JOURNAL_SCHEMA_VERSION, ATTEMPT_RECEIPT_SCHEMA_VERSION, AttemptAbortReason,
    AttemptBinding, AttemptBoundary, AttemptEntry, AttemptEvidence, AttemptFailureCode,
    AttemptFaultPlan, AttemptJournalError, AttemptJournalPaths, AttemptRecoveryOutcome,
    AttemptRecoveryReceipt, AttemptState, abort_attempt, abort_attempt_with_fault,
    attempt_journal_contract, mark_dispatch_crossed, mark_dispatch_crossed_with_fault, mark_failed,
    mark_failed_with_fault, mark_indeterminate, mark_indeterminate_with_fault, mark_succeeded,
    mark_succeeded_with_fault, prepare_attempt, prepare_attempt_with_fault, read_attempt_entry,
    read_current_attempt, recover_attempt, recover_attempt_with_fault,
};
pub use cas::{
    CAS_LAYOUT, CAS_LAYOUT_VERSION, CAS_MAX_OBJECT_BYTES, CAS_QUARANTINE_DIR, CAS_TEMP_REAP_AGE,
    CasError, CasReadGate, PutOutcome, SharedCas,
};
pub use durable_journal::{
    AbortReason, BindingLease, ContinuationCartridge, ContinuationLeaseCartridge,
    DURABLE_BINDING_SCHEMA_VERSION, DURABLE_JOURNAL_MAX_RECORD_BYTES,
    DURABLE_JOURNAL_SCHEMA_VERSION, DURABLE_LEASE_BINDING_SCHEMA_VERSION,
    DURABLE_LEASE_JOURNAL_SCHEMA_VERSION, DURABLE_LEASE_SCHEMA_VERSION,
    DURABLE_RECEIPT_SCHEMA_VERSION, DurableJournal, DurableLeaseJournal, FaultPlan, JournalBinding,
    JournalBindingLike, JournalBoundary, JournalError, JournalFailureCode, JournalLeaseBinding,
    JournalPaths, JournalRecord, JournalState, OwnerDeathReceipt, PublishedRoot, RecoveryOutcome,
    RecoveryReceipt, RootPublicationReceipt, abort_journal, abort_journal_with_fault,
    abort_lease_journal, abort_lease_journal_with_fault, commit_journal, commit_journal_with_fault,
    commit_lease_journal, commit_lease_journal_with_fault, durable_journal_contract,
    initialize_published_root, initialize_published_root_with_fault, prepare_journal,
    prepare_journal_with_fault, prepare_lease_journal, prepare_lease_journal_with_fault,
    read_continuation_cartridge, read_journal_record, read_lease_continuation_cartridge,
    read_lease_journal_record, read_published_root, record_lease_owner_death,
    record_lease_owner_death_with_fault, record_owner_death, record_owner_death_with_fault,
    recover_journal, recover_journal_with_fault, recover_lease_journal,
    recover_lease_journal_with_fault, verify_committed_lease_binding,
};
pub use fs_replace::{
    SyncPolicy, atomic_write_file, atomic_write_file_with_sync, replace_file, sync_unsupported,
    tolerate_unsupported_sync,
};
pub use gc::{
    BeforeUnlinkHook, DEFAULT_GC_REPORT_LIMIT, DryRunReport, GC_MAX_BLOB_HASHES,
    GC_MAX_EVIDENCE_ITEMS, GC_MAX_OWNER_HOST_BYTES, GC_MAX_PRODUCER_ID_BYTES,
    GC_MAX_PRODUCER_NAMESPACES, GC_MAX_RECORD_BYTES, GC_MAX_REPORT_OBJECTS, GC_MIN_GRACE_SECONDS,
    GC_RECORD_TYPE_DRY_RUN, GC_RECORD_TYPE_LEASE, GC_RECORD_TYPE_PIN, GC_RECORD_TYPE_REACHABILITY,
    GC_RECORD_TYPE_REPAIR, GC_RECORD_TYPE_SWEEP_PROGRESS, GC_REFS_FORMAT, GC_SCHEMA_VERSION,
    GC_SCHEMA_VERSION_LEGACY, GcCandidate, GcConfig, GcError, GcRunReceipt, GcRunState, GcVerdict,
    LeaseOwner, LeaseRecord, PinRecord, ReachabilitySnapshot, RepairReceipt,
    current_reachability_snapshot, gc_contract_digest_hex, gc_contract_manifest,
    gc_repair_receipt_digest_hex, gc_report_digest_hex, project_id as gc_project_id,
    publish_lease_record, publish_pin_record, publish_reachability_snapshot,
    refs_from_verified_bytes, remove_lease_record, remove_pin_record, repair_object,
    repair_object_receipted, run_gc, validate_dry_run_report, validate_repair_receipt,
};
pub use gc_lock::{
    COORDINATOR_LOCK, GC_DIR, LOCK_DEADLINE, LockMode, StoreLock, coordinator_lock_path,
};
pub use idle_gate::{
    DEFAULT_IDLE_MAX_CPU_FRACTION_PPB, DEFAULT_IDLE_MAX_RSS_BYTES, IDLE_GATE_ABI_VERSION,
    IDLE_GATE_DOMAIN, IDLE_GATE_SCHEMA_VERSION, IdleBudgets, IdleGateError, IdleGateReceipt,
    IdleGateRefusal, IdleGateRefusalReason, IdleSample, IdleSampler, IdleWindowEvidence,
    evaluate_idle_release_gate, idle_gate_contract, measure_idle_window,
};
pub use metadata::ObservationMetadata;
pub use migrations::{
    MIGRATION_MARKER_DOMAIN, MIGRATION_RECEIPT_DOMAIN, MIGRATION_RECEIPT_SCHEMA_VERSION,
    MIGRATION_STEP_DOMAIN, MigrationError, MigrationMarker, MigrationReceipt, MigrationStep,
    MigrationStepOutcome, MigrationTransform, STORE_FORMAT_MAX_KNOWN_VERSION,
    STORE_FORMAT_SCHEMA_VERSION, STORE_FORMAT_VERSION_CURRENT, STORE_FORMAT_VERSION_FILENAME,
    StoreFormatVersion, detect_store_format_version, ensure_format_supported,
    production_migration_steps, run_store_migrations,
};
pub use scrub::{
    SCRUB_MAX_OBJECT_BYTES, SCRUB_MAX_OBJECTS_PER_PASS, SCRUB_SCHEMA_VERSION, ScrubConfig,
    ScrubError, ScrubFinding, ScrubFindingKind, ScrubReceipt, read_scrub_receipt, run_scrub,
};
pub use session_wal::{
    AppendOutcome, FileIdentity, Replay, SESSION_WAL_DEFAULT_MAX_REPLAY_BYTES,
    SESSION_WAL_DEFAULT_MAX_SEALED_SEGMENTS, SESSION_WAL_MAX_RECORD_BYTES,
    SESSION_WAL_MIN_SEGMENT_BYTES, SESSION_WAL_SCHEMA_VERSION, SessionWal, SessionWalConfig,
    SessionWalError, session_wal_contract,
};
pub use snapshot::{
    SNAPSHOT_ABI_VERSION, SNAPSHOT_SCHEMA_VERSION, SNAPSHOT_STALENESS_DOMAIN,
    SnapshotReadResolution, SnapshotStalenessReceipt, SnapshotView, resolve_snapshot_read,
    snapshot_isolation_contract, take_root_snapshot,
};
pub use zbf::{
    DurableProfile, DurableProfileId, ZBF_CONTAINER_FLAG, ZBF_CONTRACT_VERSION, ZBF_HEADER_LEN,
    ZBF_MAGIC, ZBF_MAX_CHILDREN, ZBF_MAX_DEPTH, ZBF_MAX_OBJECT_BYTES, ZBF_SCHEMA_MAJOR,
    ZBF_SCHEMA_MINOR, ZbfArtifactKind, ZbfError, ZbfFailureCode, ZbfHeader, ZbfObject, ZbfPayload,
    zbf_contract_digest, zbf_contract_manifest,
};

pub use store_root::{
    BLOBS_DIR, Engine, LOCAL_STORE_DIR, PROJECT_KEY_HEX_LEN, PROJECTS_DIR, ResolvedStore,
    SHARED_STORE_OPT_IN_ENV, STORE_RESOLUTION_SCHEMA, STORE_ROOT_ENVS, StoreEnv, StoreMode,
    StoreResolutionReport, absolutize, ensure_layout, project_key, store_is_under_project_root,
};

pub use zero_cas::{
    ExpandedBlob, MappedBlob, SelectionIndex, SymbolSelection, ZERO_CAS_INDEX_BYTE_LIMIT,
    ZERO_CAS_LAYOUT, ZERO_CAS_OBJECT_BYTE_LIMIT, ZeroCas, ZeroCasError, ZeroObjectMetadata,
};

pub use event_log::{
    EVENT_LOG_BYTE_LIMIT, EVENT_LOG_DIR, EVENT_RECORD_BYTE_LIMIT, EventLog, EventLogError,
    EventLogRecord, EventPublication, ProviderUsageLogRecord, ProviderUsagePublication,
    USAGE_LOG_DIR,
};

pub use zero_migration::{
    LEGACY_OBJECT_LAYOUT, MIGRATION_MANIFEST_BYTE_LIMIT, ZeroMigrationEntry, ZeroMigrationError,
    ZeroMigrationManifest, import_legacy_store, read_and_verify_manifest,
};
