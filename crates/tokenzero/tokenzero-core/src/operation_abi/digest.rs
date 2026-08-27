//! Deterministic semantic contract digest.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use zero_abi::{
    ApprovalRequirement, CANONICAL_DISPATCH_VERSION, CanonicalOperation, CanonicalRegistry,
    CanonicalResource, DispatchErrorClass, EffectClass, EffectPolicy, PermitRequirement,
    RegistryEngine,
};

use super::registry::all_operations;
use super::schema::{normalize_schema, schema_fingerprint_hex};
use super::types::{DomainErrorKind, Mutability, Operation, SEMANTIC_CONTRACT_VERSION};

fn effect_policy(mutability: Mutability) -> EffectPolicy {
    match mutability {
        Mutability::ReadOnly => EffectPolicy {
            effect_class: EffectClass::ReadOnly,
            permit: PermitRequirement::NotRequired,
            approval: ApprovalRequirement::NotRequired,
        },
        Mutability::WorkspaceMutating | Mutability::StoreOnly => EffectPolicy {
            effect_class: EffectClass::ReversibleMutation,
            permit: PermitRequirement::Required,
            approval: ApprovalRequirement::NotRequired,
        },
    }
}

fn dispatch_error(kind: DomainErrorKind) -> DispatchErrorClass {
    match kind {
        DomainErrorKind::Validation
        | DomainErrorKind::InvalidPattern
        | DomainErrorKind::InvalidRef
        | DomainErrorKind::InvalidUrl
        | DomainErrorKind::HunkNotFound
        | DomainErrorKind::AmbiguousHunk
        | DomainErrorKind::NoOpHunk => DispatchErrorClass::InvalidArguments,
        DomainErrorKind::Policy | DomainErrorKind::Unauthorized => {
            DispatchErrorClass::UnauthorizedEffect
        }
        DomainErrorKind::Approval => DispatchErrorClass::ApprovalRequired,
        DomainErrorKind::Sandbox => DispatchErrorClass::PermitRequired,
        DomainErrorKind::Runtime
        | DomainErrorKind::Substrate
        | DomainErrorKind::Busy
        | DomainErrorKind::Cancelled
        | DomainErrorKind::DeadlineExceeded
        | DomainErrorKind::NotFound => DispatchErrorClass::DispatchFailed,
    }
}

fn hub_registry(operations: &[Operation]) -> CanonicalRegistry {
    CanonicalRegistry {
        version: CANONICAL_DISPATCH_VERSION.to_string(),
        engine: RegistryEngine::TokenZero,
        operations: operations
            .iter()
            .map(|operation| CanonicalOperation {
                canonical_id: operation.name.to_string(),
                description: operation.description.to_string(),
                aliases: operation
                    .aliases
                    .iter()
                    .map(|alias| (*alias).to_string())
                    .collect(),
                args_schema: operation.args.schema.clone(),
                output_schema: Some(operation.results.schema.clone()),
                mcp_tool_name: operation
                    .exposure
                    .fastmcp_tool
                    .then(|| operation.name.to_string()),
                effect_policy: effect_policy(operation.mutability),
                errors: operation
                    .error_kinds
                    .iter()
                    .copied()
                    .map(dispatch_error)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            })
            .collect(),
        resources: operations
            .iter()
            .filter_map(|operation| {
                operation
                    .exposure
                    .resource_uri
                    .map(|uri| CanonicalResource {
                        uri: uri.to_string(),
                        name: operation.name.to_string(),
                        description: operation.description.to_string(),
                        mime_type: Some("application/json".to_string()),
                    })
            })
            .collect(),
    }
}

fn normalized_string_set(values: &[&str], label: &str) -> Result<Vec<String>, String> {
    if values.iter().any(|value| {
        value.is_empty() || value.trim() != *value || value.chars().any(char::is_whitespace)
    }) {
        return Err(format!("{label} contains an invalid member"));
    }
    let mut normalized = values
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
    normalized.sort();
    if normalized.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(format!("{label} contains duplicates"));
    }
    Ok(normalized)
}

fn normalized_arg_aliases(value: &Value, operation: &str) -> Result<Value, String> {
    let aliases = value
        .as_object()
        .ok_or_else(|| format!("{operation} arg_aliases must be an object"))?;
    let mut normalized = serde_json::Map::new();
    for (argument, values) in aliases {
        if argument.is_empty()
            || argument.trim() != argument
            || argument.chars().any(char::is_whitespace)
        {
            return Err(format!(
                "{operation} arg_aliases contains an invalid argument"
            ));
        }
        let values = values
            .as_array()
            .ok_or_else(|| format!("{operation} arg_aliases.{argument} must be a string array"))?;
        let strings = values
            .iter()
            .map(|value| {
                value.as_str().ok_or_else(|| {
                    format!("{operation} arg_aliases.{argument} must contain only strings")
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        normalized.insert(
            argument.clone(),
            json!(normalized_string_set(
                &strings,
                &format!("{operation} arg_aliases.{argument}")
            )?),
        );
    }
    Ok(Value::Object(normalized))
}

fn semantic_operation_manifest(operation: &Operation) -> Result<Value, String> {
    let error_kinds = normalized_string_set(
        &operation
            .error_kinds
            .iter()
            .map(|kind| kind.as_str())
            .collect::<Vec<_>>(),
        &format!("{} error_kinds", operation.name),
    )?;
    Ok(json!({
        "name": operation.name,
        "aliases": normalized_string_set(operation.aliases, &format!("{} aliases", operation.name))?,
        "cluster": operation.cluster,
        "capabilities": normalized_string_set(
            operation.capabilities,
            &format!("{} capabilities", operation.name),
        )?,
        "mutability": operation.mutability.as_str(),
        "capability": operation.capability.as_str(),
        "cost_class": operation.cost_class.as_str(),
        "ref_ownership": operation.ref_ownership.as_str(),
        "cancellation": operation.cancellation.as_str(),
        "migration": operation.migration.as_str(),
        "fastmcp_tool": operation.exposure.fastmcp_tool,
        "codemode_mcp_tool": operation.exposure.codemode_mcp_tool,
        "codemode_binding": operation.exposure.codemode_binding,
        "resource_uri": operation.exposure.resource_uri,
        "input_schema": normalize_schema(&operation.args.schema),
        "output_schema": normalize_schema(&operation.results.schema),
        "input_schema_fingerprint": schema_fingerprint_hex(&operation.args.schema),
        "output_schema_fingerprint": schema_fingerprint_hex(&operation.results.schema),
        "error_kinds": error_kinds,
        "arg_aliases": normalized_arg_aliases(&operation.arg_aliases, operation.name)?,
    }))
}

pub fn contract_manifest_for(operations: &[Operation]) -> Result<Value, String> {
    let registry = hub_registry(operations);
    let canonical = registry
        .canonical_manifest()
        .map_err(|error| error.to_string())?;
    let dispatch_digest = zero_abi::contract_digest_hex(&canonical);
    let by_name = operations
        .iter()
        .map(|operation| (operation.name, operation))
        .collect::<BTreeMap<_, _>>();
    let operation_order = canonical["operations"]
        .as_array()
        .ok_or_else(|| "hub canonical registry omitted operations".to_string())?;
    let semantic_operations = operation_order
        .iter()
        .map(|operation| {
            let name = operation["canonical_id"]
                .as_str()
                .ok_or_else(|| "hub canonical operation omitted canonical_id".to_string())?;
            semantic_operation_manifest(
                by_name
                    .get(name)
                    .copied()
                    .ok_or_else(|| format!("hub canonical operation {name:?} is unknown"))?,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(json!({
        "semantic_contract_version": SEMANTIC_CONTRACT_VERSION,
        "engine": "tokenzero",
        "schema_parity": "structural_io",
        "canonical_dispatch_version": CANONICAL_DISPATCH_VERSION,
        "canonical_dispatch_digest": dispatch_digest,
        "operations": semantic_operations,
    }))
}

/// Full contract manifest used as digest input.
///
/// Embeds **complete** normalized input and output schemas (not property-name
/// sets alone) so type/required/nested-constraint drift changes the digest.
pub fn contract_manifest() -> Value {
    contract_manifest_for(all_operations())
        .unwrap_or_else(|error| panic!("invalid TokenZero canonical registry: {error}"))
}

/// Raw digest bytes (SHA-256 over canonical JSON).
///
/// Memoized process-wide: the registry is static, so recomputing the full
/// manifest hash on every handshake/SBOM/doctor call was pure overhead
/// (tokenzero-irx9.9 hot-path).
pub fn contract_digest() -> [u8; 32] {
    static DIGEST: std::sync::OnceLock<[u8; 32]> = std::sync::OnceLock::new();
    *DIGEST.get_or_init(|| zero_abi::contract_digest(&contract_manifest()))
}

/// Lowercase hex digest (64 chars). Deterministic across builds for the same registry.
pub fn contract_digest_hex() -> String {
    static HEX: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HEX.get_or_init(|| {
        CONTRACT_DIGEST_HEX_INITIALIZATIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        zero_abi::contract_digest_hex(&contract_manifest())
    })
    .clone()
}

static CONTRACT_DIGEST_HEX_INITIALIZATIONS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

#[doc(hidden)]
pub fn contract_digest_hex_initializations() -> usize {
    CONTRACT_DIGEST_HEX_INITIALIZATIONS.load(std::sync::atomic::Ordering::Relaxed)
}
