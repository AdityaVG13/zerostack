//! Engine-agnostic operation ABI contract machinery shared by ZeroStack engines.
//!
//! Each engine (TokenZero, FSZero, GraphZero) keeps its own operation
//! registry, enums, and catalogs. This crate owns the parts that must never
//! drift between engines:
//!
//! - canonical JSON encoding with deterministic key order
//! - JSON Schema normalization and structural comparison
//! - schema fingerprints and the contract digest hash
//!
//! Engines wrap these primitives with their own registry types and parity
//! assertions, so adopting this crate changes no digests and no behavior.

pub mod digest;
pub mod schema;

pub use digest::{contract_digest, contract_digest_hex, sha256, sha256_hex};
pub use schema::{
    canonical_json, canonical_schema_json, normalize_schema, schema_diff, schema_fingerprint_hex,
    schema_property_keys, schema_required_keys, schemas_structurally_equal,
};
