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
mod fs_replace;
mod gc_lock;
mod metadata;
mod store_root;
mod zbf;

pub use cas::{
    CasError, PutOutcome, SharedCas, CAS_LAYOUT, CAS_LAYOUT_VERSION, CAS_MAX_OBJECT_BYTES,
    CAS_QUARANTINE_DIR, CAS_TEMP_REAP_AGE,
};
pub use fs_replace::{atomic_write_file, replace_file};
pub use gc_lock::{
    coordinator_lock_path, LockMode, StoreLock, COORDINATOR_LOCK, GC_DIR, LOCK_DEADLINE,
};
pub use metadata::ObservationMetadata;
pub use zbf::{
    zbf_contract_digest_v1, zbf_contract_manifest_v1, DurableProfileIdV1, DurableProfileV1,
    ZbfArtifactKindV1, ZbfErrorV1, ZbfFailureCodeV1, ZbfHeaderV1, ZbfObjectV1, ZbfPayloadV1,
    ZBF_CONTAINER_FLAG_V1, ZBF_CONTRACT_VERSION_V1, ZBF_HEADER_LEN_V1, ZBF_MAGIC_V1,
    ZBF_MAX_CHILDREN_V1, ZBF_MAX_DEPTH_V1, ZBF_MAX_OBJECT_BYTES_V1, ZBF_SCHEMA_MAJOR_V1,
    ZBF_SCHEMA_MINOR_V1,
};

pub use store_root::{
    absolutize, ensure_layout, project_key, store_is_under_project_root, Engine, ResolvedStore,
    StoreEnv, StoreMode, StoreResolutionReport, BLOBS_DIR, LOCAL_STORE_DIR, PROJECTS_DIR,
    PROJECT_KEY_HEX_LEN, SHARED_STORE_OPT_IN_ENV, STORE_RESOLUTION_SCHEMA, STORE_ROOT_ENVS,
};
