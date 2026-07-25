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
mod store_root;

pub use cas::{
    CasError, PutOutcome, SharedCas, CAS_LAYOUT, CAS_LAYOUT_VERSION, CAS_MAX_OBJECT_BYTES,
    CAS_QUARANTINE_DIR, CAS_TEMP_REAP_AGE,
};
pub use fs_replace::{atomic_write_file, replace_file};
pub use gc_lock::{
    coordinator_lock_path, LockMode, StoreLock, COORDINATOR_LOCK, GC_DIR, LOCK_DEADLINE,
};
pub use store_root::{
    absolutize, ensure_layout, project_key, store_is_under_project_root, Engine, ResolvedStore,
    StoreEnv, StoreMode, StoreResolutionReport, BLOBS_DIR, LOCAL_STORE_DIR, PROJECTS_DIR,
    PROJECT_KEY_HEX_LEN, SHARED_STORE_OPT_IN_ENV, STORE_RESOLUTION_SCHEMA, STORE_ROOT_ENVS,
};
