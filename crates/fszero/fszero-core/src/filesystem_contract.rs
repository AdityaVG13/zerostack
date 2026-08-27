//! Versioned FSZero filesystem semantics.
//!
//! The canonical contract is the checked-in JSON document. CLI doctor,
//! embedded callers, MCP root reports, and CodeMode all expose this exact
//! parsed value; no surface maintains a second copy.

use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::sync::OnceLock;

pub const FILESYSTEM_CONTRACT_NAME: &str = "fszero-filesystem";
pub const FILESYSTEM_CONTRACT_MAJOR: u64 = 1;
pub const FILESYSTEM_CONTRACT_MINOR: u64 = 0;
/// Patch bumps are additive clarifications only (ABI advertisement, golden vectors).
pub const FILESYSTEM_CONTRACT_VERSION: &str = "1.0.4";
pub const FILESYSTEM_CONTRACT_STORE_KEY: &str = "filesystem_contract";
pub const FILESYSTEM_CONTRACT_JSON: &str =
    include_str!("../../../../contracts/fszero/filesystem-v1.json");

static FILESYSTEM_CONTRACT: OnceLock<Value> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesystemContractError {
    pub class: &'static str,
    pub message: String,
}

impl std::fmt::Display for FilesystemContractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.class, self.message)
    }
}

impl std::error::Error for FilesystemContractError {}

fn incompatible(message: impl Into<String>) -> FilesystemContractError {
    FilesystemContractError {
        class: "incompatible_contract",
        message: message.into(),
    }
}

#[inline]
fn obj_get<'a>(v: &'a Value, key: &str, err: &str) -> Result<&'a Map<String, Value>, String> {
    v.get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| err.to_string())
}

#[inline]
fn str_get<'a>(m: &'a Map<String, Value>, key: &str, err: &str) -> Result<&'a str, String> {
    m.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| err.to_string())
}

/// Shared name/major/minor identity extract for document validate + peer negotiate.
fn contract_identity(contract: &Map<String, Value>) -> Result<(&str, u64), &'static str> {
    let name = contract.get("name").and_then(Value::as_str).ok_or("name")?;
    let major = contract
        .get("major")
        .and_then(Value::as_u64)
        .ok_or("major")?;
    if contract.get("minor").and_then(Value::as_u64).is_none() {
        return Err("minor");
    }
    Ok((name, major))
}

/// Exact canonical descriptor exposed by every public surface.
pub fn filesystem_contract_descriptor() -> &'static Value {
    FILESYSTEM_CONTRACT.get_or_init(|| {
        serde_json::from_str(FILESYSTEM_CONTRACT_JSON)
            .expect("checked-in filesystem contract must be valid JSON")
    })
}

/// Validate the checked-in document's internal references.
///
/// This is intentionally dependency-free rather than a partial JSON-Schema
/// implementation: it checks every invariant FSZero consumes at runtime.
pub fn validate_filesystem_contract_document(value: &Value) -> Result<(), String> {
    let contract = obj_get(value, "contract", "contract must be an object")?;
    let (name, major) = match contract_identity(contract) {
        Ok(v) => v,
        Err("name") => return Err("contract.name must be a string".into()),
        Err("major") => return Err("contract.major must be numeric".into()),
        Err(_) => return Err("contract.minor must be numeric".into()),
    };
    if name != FILESYSTEM_CONTRACT_NAME {
        return Err(format!("contract.name must be {FILESYSTEM_CONTRACT_NAME}"));
    }
    if major != FILESYSTEM_CONTRACT_MAJOR {
        return Err(format!(
            "contract.major must be {FILESYSTEM_CONTRACT_MAJOR}"
        ));
    }

    let guarantees = obj_get(value, "guarantees", "guarantees must be an object")?;
    let errors = obj_get(value, "error_classes", "error_classes must be an object")?;
    let operations = obj_get(value, "operations", "operations must be an object")?;
    if operations.is_empty() {
        return Err("operations must not be empty".to_string());
    }
    for (operation, mapping) in operations {
        let clauses = mapping
            .get("clauses")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{operation}.clauses must be an array"))?;
        for clause in clauses {
            let clause = clause
                .as_str()
                .ok_or_else(|| format!("{operation}.clauses entries must be strings"))?;
            if !guarantees.contains_key(clause) && clause != "privacy" && clause != "platforms" {
                return Err(format!("{operation} references unknown clause {clause}"));
            }
        }
        let mapped_errors = mapping
            .get("errors")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{operation}.errors must be an array"))?;
        for class in mapped_errors {
            let class = class
                .as_str()
                .ok_or_else(|| format!("{operation}.errors entries must be strings"))?;
            if !errors.contains_key(class) {
                return Err(format!("{operation} references unknown error {class}"));
            }
        }
    }

    let aliases = obj_get(value, "aliases", "aliases must be an object")?;
    for (surface, entries) in aliases {
        let entries = entries
            .as_object()
            .ok_or_else(|| format!("aliases.{surface} must be an object"))?;
        for (alias, target) in entries {
            let target = target
                .as_str()
                .ok_or_else(|| format!("aliases.{surface}.{alias} must be a string"))?;
            if target != "surface_dispatch" && !operations.contains_key(target) {
                return Err(format!(
                    "aliases.{surface}.{alias} targets unknown {target}"
                ));
            }
        }
    }

    // Optional additive ABI advertisement (fszero-ncib.1). When present it must
    // name the operation ABI and pin a digest algorithm.
    if let Some(abi) = value.get("abi") {
        let abi = abi
            .as_object()
            .ok_or_else(|| "abi must be an object".to_string())?;
        let name = str_get(abi, "name", "abi.name must be a string")?;
        if name != "fszero-operation-abi" {
            return Err(format!("abi.name must be fszero-operation-abi, got {name}"));
        }
        if abi.get("version").and_then(Value::as_str).is_none() {
            return Err("abi.version must be a string".to_string());
        }
        let algo = str_get(
            abi,
            "digest_algorithm",
            "abi.digest_algorithm must be a string",
        )?;
        if algo != "sha256" {
            return Err(format!("abi.digest_algorithm must be sha256, got {algo}"));
        }
    }

    if let Some(vectors) = value.get("golden_vectors").and_then(Value::as_object) {
        if let Some(abi_domain) = vectors.get("abi_domain") {
            let rows = abi_domain
                .as_array()
                .ok_or_else(|| "golden_vectors.abi_domain must be an array".to_string())?;
            if rows.is_empty() {
                return Err("golden_vectors.abi_domain must not be empty".to_string());
            }
            for row in rows {
                let id = row
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "abi_domain vector missing id".to_string())?;
                let op = row
                    .get("operation")
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("abi_domain {id} missing operation"))?;
                if !operations.contains_key(op) {
                    return Err(format!("abi_domain {id} references unknown op {op}"));
                }
                if let Some(err) = row.get("error") {
                    if !err.is_null() {
                        let class = err
                            .as_str()
                            .ok_or_else(|| format!("abi_domain {id} error must be string|null"))?;
                        if !errors.contains_key(class) {
                            return Err(format!("abi_domain {id} unknown error {class}"));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Negotiate a peer descriptor. Additive minor fields are forward compatible;
/// name and major define the protocol boundary.
pub fn negotiate_filesystem_contract(peer: &Value) -> Result<(), FilesystemContractError> {
    let contract = peer
        .get("contract")
        .and_then(Value::as_object)
        .ok_or_else(|| incompatible("missing contract object"))?;
    let (name, major) = match contract_identity(contract) {
        Ok(v) => v,
        Err("name") => return Err(incompatible("missing string contract.name")),
        Err("major") => return Err(incompatible("missing numeric contract.major")),
        Err(_) => return Err(incompatible("missing numeric contract.minor")),
    };
    if name != FILESYSTEM_CONTRACT_NAME {
        return Err(incompatible(format!(
            "peer contract {name:?} differs from {FILESYSTEM_CONTRACT_NAME:?}"
        )));
    }
    if major != FILESYSTEM_CONTRACT_MAJOR {
        return Err(incompatible(format!(
            "peer major {major} differs from supported major {FILESYSTEM_CONTRACT_MAJOR}"
        )));
    }
    Ok(())
}

pub fn filesystem_contract_operation_names() -> BTreeSet<String> {
    filesystem_contract_descriptor()["operations"]
        .as_object()
        .expect("validated filesystem operations")
        .keys()
        .cloned()
        .collect()
}
