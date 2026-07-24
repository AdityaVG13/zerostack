//! Golden JSON Schema validation (embedded at compile time).

use anyhow::{Result, bail};
use jsonschema::Validator;
use serde_json::Value;
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaName {
    CapabilityManifest,
    Telemetry,
    Error,
    ExecutionRecord,
    LimitsEcho,
}

impl SchemaName {
    pub fn file_stem(self) -> &'static str {
        match self {
            Self::CapabilityManifest => "capability_manifest",
            Self::Telemetry => "telemetry",
            Self::Error => "error",
            Self::ExecutionRecord => "execution_record",
            Self::LimitsEcho => "limits_echo",
        }
    }

    fn schema_json(self) -> &'static str {
        match self {
            Self::CapabilityManifest => include_str!("../schemas/capability_manifest.json"),
            Self::Telemetry => include_str!("../schemas/telemetry.json"),
            Self::Error => include_str!("../schemas/error.json"),
            Self::ExecutionRecord => include_str!("../schemas/execution_record.json"),
            Self::LimitsEcho => include_str!("../schemas/limits_echo.json"),
        }
    }
}

fn validator(name: SchemaName) -> &'static Validator {
    static CAP: LazyLock<Validator> = LazyLock::new(|| compile(SchemaName::CapabilityManifest));
    static TEL: LazyLock<Validator> = LazyLock::new(|| compile(SchemaName::Telemetry));
    static ERR: LazyLock<Validator> = LazyLock::new(|| compile(SchemaName::Error));
    static EXE: LazyLock<Validator> = LazyLock::new(|| compile(SchemaName::ExecutionRecord));
    static LIM: LazyLock<Validator> = LazyLock::new(|| compile(SchemaName::LimitsEcho));

    match name {
        SchemaName::CapabilityManifest => &CAP,
        SchemaName::Telemetry => &TEL,
        SchemaName::Error => &ERR,
        SchemaName::ExecutionRecord => &EXE,
        SchemaName::LimitsEcho => &LIM,
    }
}

fn compile(name: SchemaName) -> Validator {
    let raw: Value = serde_json::from_str(name.schema_json())
        .unwrap_or_else(|e| panic!("invalid embedded schema {}: {e}", name.file_stem()));
    Validator::new(&raw).unwrap_or_else(|e| panic!("validator {}: {e}", name.file_stem()))
}

pub fn validate_document(name: SchemaName, doc: &Value) -> Result<()> {
    let v = validator(name);
    let errors: Vec<String> = v
        .iter_errors(doc)
        .map(|e| e.to_string())
        .take(8)
        .collect();
    if errors.is_empty() {
        return Ok(());
    }
    bail!(
        "schema {} failed: {}",
        name.file_stem(),
        errors.join("; ")
    );
}

/// Alias used by integration tests.
pub fn validate_against_schema(name: SchemaName, doc: &Value) -> Result<()> {
    validate_document(name, doc)
}

pub fn telemetry_has_no_raw_leak(doc: &Value) -> Result<()> {
    if doc.as_object().and_then(|o| o.get("raw_leak")).is_some() {
        bail!("telemetry must not contain raw_leak");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn golden_telemetry_validates_and_rejects_raw_leak_field() {
        let ok = json!({
            "kind": "codemode.execute",
            "status": "ok",
            "logical_ops": 1,
            "physical_ops": 1,
            "batched_ops": 0,
            "internal_actions": 1,
            "cache_hits": 0,
            "cache_misses": 0,
            "store_writes": 0,
            "wall_ms": 1,
            "bytes_materialized": 4
        });
        validate_document(SchemaName::Telemetry, &ok).expect("valid telemetry");
        telemetry_has_no_raw_leak(&ok).expect("no raw_leak");

        let with_leak = json!({
            "kind": "x",
            "status": "ok",
            "logical_ops": 0,
            "physical_ops": 0,
            "batched_ops": 0,
            "internal_actions": 0,
            "cache_hits": 0,
            "cache_misses": 0,
            "store_writes": 0,
            "wall_ms": 0,
            "bytes_materialized": 0,
            "raw_leak": false
        });
        assert!(validate_document(SchemaName::Telemetry, &with_leak).is_err());
    }

    #[test]
    fn error_taxonomy_requires_kind_message_retryable() {
        validate_document(
            SchemaName::Error,
            &json!({
                "kind": "sandbox",
                "message": "fetch denied",
                "retryable": false
            }),
        )
        .expect("valid error");
        assert!(validate_document(
            SchemaName::Error,
            &json!({ "kind": "sandbox", "message": "x" })
        )
        .is_err());
    }
}