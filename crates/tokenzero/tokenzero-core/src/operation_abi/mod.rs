//! Canonical TokenZero operation ABI and semantic contract (tokenzero-irx9.1).
//!
//! One versioned registry is the source of truth for operation names, aliases,
//! input/output schemas, mutability, capability, cost class, ref ownership,
//! error taxonomy, and cancellation. FastMCP tools and CodeMode bindings must
//! agree with this registry (name set equality **and** full structural
//! input/output schema parity — types, requiredness, nested constraints).
//!
//! Dispatch wiring is tokenzero-irx9.2; this module defines the contract only.

mod catalog;
pub mod digest;
mod registry;
mod schema;
mod schemas;
mod types;
mod vectors;

pub use catalog::{
    codemode_binding_names, codemode_mcp_tool_names, fastmcp_tool_names, input_schema_for,
    output_schema_for, resolve_operation, resource_uris,
};
pub use digest::{contract_digest, contract_digest_hex, contract_manifest};
pub use registry::{all_operations, operation_by_name};
pub use schema::{
    assert_tool_schema_parity, canonical_json, canonical_schema_json, normalize_schema,
    schema_diff, schema_fingerprint_hex, schema_property_keys, schema_required_keys,
    schemas_structurally_equal,
};
pub use schemas::{
    batch_schema, cache_pack_schema, codemode_describe_schema, codemode_search_schema, edit_schema,
    execute_code_schema, expand_schema, fetch_schema, glob_schema, no_args_schema, read_schema,
    recall_schema, report_tool_issue_schema, rewrite_schema, search_schema, shell_schema,
    text_schema, tree_schema,
};
pub use types::{
    ABI_DEFAULT_SHELL_TIMEOUT_SECS, ABI_HARD_MAX_WALL_MS, CancellationSemantics,
    CapabilityRequirement, CostClass, DomainError, DomainErrorKind, DomainResult, MigrationStatus,
    Mutability, Operation, OperationArgs, OperationId, OperationResults, RefOwnership,
    SEMANTIC_CONTRACT_VERSION, SurfaceExposure,
};
pub use vectors::{GoldenVector, golden_vectors};
