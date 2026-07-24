//! Canonical ZeroRef v1 content-addressed store layout and publish protocol.
//!
//! Layout: <cas_root>/blobs/sha256/<first-two-hex>/<64-lowercase-hex>,
//! immutable complete objects only. Engine facts, indexes, provenance, and
//! mutable metadata never live in this namespace.
//!
//! The publish protocol is crash-safe and concurrency-safe: unique sibling
//! temp file, sync, atomic rename, directory sync. Identical concurrent
//! writers converge on one valid object; a preexisting object with different
//! bytes is a loud corruption error and is never overwritten.

mod cas;
mod fs_replace;

pub use cas::{
    CAS_LAYOUT, CAS_LAYOUT_VERSION, CAS_MAX_OBJECT_BYTES, CAS_TEMP_REAP_AGE, CasError, SharedCas,
};
pub use fs_replace::{atomic_write_file, replace_file};
