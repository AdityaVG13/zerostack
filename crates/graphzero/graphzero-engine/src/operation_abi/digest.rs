//! Deterministic semantic contract digest.

use serde_json::{Value, json};

use super::registry::all_operations;
use super::schema::{normalize_schema, schema_fingerprint_hex};
use super::types::SEMANTIC_CONTRACT_VERSION;

/// Full contract manifest used as digest input.
///
/// Embeds **complete** normalized input and output schemas (not property-name
/// sets alone) so type/required/nested-constraint drift changes the digest.
pub fn contract_manifest() -> Value {
    let mut ops = Vec::new();
    for op in all_operations() {
        let mut aliases: Vec<&str> = op.aliases.to_vec();
        aliases.sort_unstable();
        let mut error_kinds: Vec<&str> = op.error_kinds.iter().map(|k| k.as_str()).collect();
        error_kinds.sort_unstable();
        let input_schema = normalize_schema(&op.args.schema);
        let output_schema = normalize_schema(&op.results.schema);
        ops.push(json!({
            "name": op.name,
            "aliases": aliases,
            "mutability": mutability_str(op.mutability),
            "capability": capability_str(op.capability),
            "cost_class": cost_str(op.cost_class),
            "ref_ownership": ref_str(op.ref_ownership),
            "cancellation": cancel_str(op.cancellation),
            "migration": migration_str(op.migration),
            "fastmcp_tool": op.exposure.fastmcp_tool,
            "codemode_binding": op.exposure.codemode_binding,
            "codemode_meta": op.exposure.codemode_meta,
            "input_schema": input_schema,
            "output_schema": output_schema,
            "input_schema_fingerprint": schema_fingerprint_hex(&op.args.schema),
            "output_schema_fingerprint": schema_fingerprint_hex(&op.results.schema),
            "error_kinds": error_kinds,
            "is_orient_router": op.is_orient_router,
        }));
    }
    // Ops already sorted by name in registry build.
    json!({
        "semantic_contract_version": SEMANTIC_CONTRACT_VERSION,
        "engine": "graphzero",
        "schema_parity": "structural_io",
        "operations": ops,
    })
}

/// Raw digest bytes (SHA-256 over canonical JSON).
pub fn contract_digest() -> [u8; 32] {
    zero_abi::contract_digest(&contract_manifest())
}

/// Lowercase hex digest (64 chars). Deterministic across builds for the same registry.
pub fn contract_digest_hex() -> String {
    zero_abi::contract_digest_hex(&contract_manifest())
}

fn mutability_str(m: super::types::Mutability) -> &'static str {
    match m {
        super::types::Mutability::ReadOnly => "read_only",
        super::types::Mutability::StoreOnly => "store_only",
    }
}

fn capability_str(c: super::types::CapabilityRequirement) -> &'static str {
    match c {
        super::types::CapabilityRequirement::Public => "public",
        super::types::CapabilityRequirement::PrivateWorker => "private_worker",
    }
}

fn cost_str(c: super::types::CostClass) -> &'static str {
    match c {
        super::types::CostClass::Cheap => "cheap",
        super::types::CostClass::Medium => "medium",
        super::types::CostClass::Heavy => "heavy",
    }
}

fn ref_str(r: super::types::RefOwnership) -> &'static str {
    match r {
        super::types::RefOwnership::None => "none",
        super::types::RefOwnership::Query => "query",
        super::types::RefOwnership::Blob => "blob",
        super::types::RefOwnership::Mem => "mem",
        super::types::RefOwnership::Multi => "multi",
        super::types::RefOwnership::Execution => "execution",
    }
}

fn cancel_str(c: super::types::CancellationSemantics) -> &'static str {
    match c {
        super::types::CancellationSemantics::None => "none",
        super::types::CancellationSemantics::Cooperative => "cooperative",
        super::types::CancellationSemantics::Deadline => "deadline",
    }
}

fn migration_str(m: super::types::MigrationStatus) -> &'static str {
    match m {
        super::types::MigrationStatus::Canonical => "canonical",
        super::types::MigrationStatus::LegacyAlias => "legacy_alias",
        super::types::MigrationStatus::OrientSubSurface => "orient_sub_surface",
    }
}
