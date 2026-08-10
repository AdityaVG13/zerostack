#![forbid(unsafe_code)]

//! Canonical ZeroRef v1 content-addressed store layout, publish protocol,
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

mod cas;
mod durable_journal;
mod fs_replace;
mod gc;
mod gc_lock;
mod metadata;
mod store_root;
mod zbf;

pub use cas::{
    CAS_LAYOUT, CAS_LAYOUT_VERSION, CAS_MAX_OBJECT_BYTES, CAS_QUARANTINE_DIR, CAS_TEMP_REAP_AGE,
    CasError, PutOutcome, SharedCas,
};
pub use durable_journal::{
    AbortReasonV1, ContinuationCartridgeV1, DURABLE_BINDING_SCHEMA_VERSION_V1,
    DURABLE_JOURNAL_MAX_RECORD_BYTES_V1, DURABLE_JOURNAL_SCHEMA_VERSION_V2,
    DURABLE_RECEIPT_SCHEMA_VERSION_V1, DurableJournalV2, FaultPlanV1, JournalBindingV1,
    JournalBoundaryV1, JournalErrorV1, JournalFailureCodeV1, JournalPathsV1, JournalRecordV1,
    JournalStateV1, OwnerDeathReceiptV1, PublishedRootV1, RecoveryOutcomeV1, RecoveryReceiptV1,
    RootPublicationReceipt, abort_journal_v1, abort_journal_with_fault_v1, commit_journal_v1,
    commit_journal_with_fault_v1, durable_journal_contract_v1, initialize_published_root_v1,
    initialize_published_root_with_fault_v1, prepare_journal_v1, prepare_journal_with_fault_v1,
    read_continuation_cartridge_v1, read_journal_record_v1, read_published_root_v1,
    record_owner_death_v1, record_owner_death_with_fault_v1, recover_journal_v1,
    recover_journal_with_fault_v1,
};
pub use fs_replace::{atomic_write_file, replace_file};
pub use gc::{
    BeforeUnlinkHook, DEFAULT_GC_REPORT_LIMIT, DryRunReport, GC_MAX_BLOB_HASHES,
    GC_MAX_EVIDENCE_ITEMS, GC_MAX_OWNER_HOST_BYTES, GC_MAX_PRODUCER_ID_BYTES,
    GC_MAX_PRODUCER_NAMESPACES, GC_MAX_RECORD_BYTES, GC_MAX_REPORT_OBJECTS, GC_MIN_GRACE_SECONDS,
    GC_RECORD_TYPE_DRY_RUN, GC_RECORD_TYPE_LEASE, GC_RECORD_TYPE_PIN, GC_RECORD_TYPE_REACHABILITY,
    GC_RECORD_TYPE_REPAIR, GC_RECORD_TYPE_SWEEP_PROGRESS, GC_SCHEMA_VERSION, GC_SCHEMA_VERSION_V1,
    GcCandidate, GcConfig, GcError, GcRunReceipt, GcRunState, GcVerdict, LeaseOwner, LeaseRecord,
    PinRecord, ReachabilitySnapshot, RepairReceipt, current_reachability_snapshot,
    gc_contract_digest_hex, gc_contract_manifest, gc_repair_receipt_digest_hex,
    gc_report_digest_hex, project_id as gc_project_id, publish_lease_record, publish_pin_record,
    publish_reachability_snapshot, remove_lease_record, remove_pin_record, repair_object,
    repair_object_receipted, run_gc, validate_dry_run_report, validate_repair_receipt,
};
pub use gc_lock::{
    COORDINATOR_LOCK, GC_DIR, LOCK_DEADLINE, LockMode, StoreLock, coordinator_lock_path,
};
pub use metadata::ObservationMetadata;
pub use zbf::{
    DurableProfileIdV1, DurableProfileV1, ZBF_CONTAINER_FLAG_V1, ZBF_CONTRACT_VERSION_V1,
    ZBF_HEADER_LEN_V1, ZBF_MAGIC_V1, ZBF_MAX_CHILDREN_V1, ZBF_MAX_DEPTH_V1,
    ZBF_MAX_OBJECT_BYTES_V1, ZBF_SCHEMA_MAJOR_V1, ZBF_SCHEMA_MINOR_V1, ZbfArtifactKindV1,
    ZbfErrorV1, ZbfFailureCodeV1, ZbfHeaderV1, ZbfObjectV1, ZbfPayloadV1, zbf_contract_digest_v1,
    zbf_contract_manifest_v1,
};

pub use store_root::{
    BLOBS_DIR, Engine, LOCAL_STORE_DIR, PROJECT_KEY_HEX_LEN, PROJECTS_DIR, ResolvedStore,
    SHARED_STORE_OPT_IN_ENV, STORE_RESOLUTION_SCHEMA, STORE_ROOT_ENVS, StoreEnv, StoreMode,
    StoreResolutionReport, absolutize, ensure_layout, project_key, store_is_under_project_root,
};
