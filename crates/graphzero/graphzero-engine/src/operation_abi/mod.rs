//! Canonical GraphZero operation ABI and semantic contract (graphzero-o2uq.1).
//!
//! One versioned registry is the source of truth for operation names, aliases,
//! input schemas, mutability, capability, cost class, ref ownership, error
//! taxonomy, and cancellation. FastMCP lean tools and CodeMode bindings must
//! agree with this registry (name set equality **and** full structural
//! input/output schema parity — types, requiredness, nested constraints).
//!
//! Dispatch wiring is graphzero-o2uq.2; this module defines the contract only.

mod catalog;
mod digest;
mod registry;
mod schema;
mod types;
mod vectors;

pub use catalog::{
    codemode_binding_names, codemode_discovery_hits, codemode_meta_tools_from_registry,
    lean_fastmcp_tool_names, lean_fastmcp_tools_from_registry, orient_surface_names,
    resolve_operation, schema_property_keys, schema_required_keys,
};
pub use digest::{contract_digest, contract_digest_hex, contract_manifest};
pub use registry::{all_operations, operation_by_name};
pub use schema::{
    assert_tool_schema_parity, canonical_schema_json, schema_diff, schema_fingerprint_hex,
    schemas_structurally_equal,
};
pub use types::{
    CancellationSemantics, CapabilityRequirement, CostClass, DomainError, DomainErrorKind,
    DomainResult, MigrationStatus, Mutability, Operation, OperationArgs, OperationId,
    OperationResults, RefOwnership, SEMANTIC_CONTRACT_VERSION, SurfaceExposure,
};
pub use vectors::{GoldenVector, golden_vectors};
