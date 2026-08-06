//! Canonical JSON Schema compare for operation ABI parity.
//!
//! Catalogs and bindings must match an engine registry on the full structural
//! schema (types, requiredness, nested constraints), not merely property-name
//! sets. Description/title text is ignored so prose edits do not mask real
//! drift.

use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

use crate::digest::sha256_hex;

/// Keys treated as non-structural documentation (ignored in parity compare).
const DOC_KEYS: &[&str] = &["description", "title", "$comment"];

/// Schema keywords whose values are arbitrary JSON rather than nested schemas.
const OPAQUE_PAYLOAD_KEYS: &[&str] = &["default", "const", "examples"];

/// Deep structural equality of two JSON Schema fragments.
pub fn schemas_structurally_equal(a: &Value, b: &Value) -> bool {
    canonical_schema_json(a) == canonical_schema_json(b)
}

/// Human-readable first structural divergence (for kill-test assertions).
pub fn schema_diff(a: &Value, b: &Value) -> Option<String> {
    diff_normalized(&normalize_schema(a), &normalize_schema(b), "$")
}

/// Canonical JSON string of a schema (sorted keys, sorted required/enum where
/// order is not semantically significant). Used by the contract digest.
pub fn canonical_schema_json(schema: &Value) -> String {
    canonical_json(&normalize_schema(schema))
}

/// Deterministic JSON encoding with sorted object keys (recursive).
pub fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into()),
        Value::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(canonical_json).collect();
            format!("[{}]", parts.join(","))
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    let key = serde_json::to_string(k).unwrap_or_else(|_| "\"\"".into());
                    format!("{key}:{}", canonical_json(&map[k]))
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
    }
}

/// Fingerprint of a schema for digest embedding.
pub fn schema_fingerprint_hex(schema: &Value) -> String {
    sha256_hex(canonical_schema_json(schema).as_bytes())
}

/// Normalize a schema for structural compare / digest:
/// - drop documentation keys
/// - sort object keys (via canonical serialization path)
/// - sort required arrays
/// - sort string-only enum arrays (non-string enums preserve order)
/// - recurse into properties / items / additionalProperties / *Of
pub fn normalize_schema(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                if DOC_KEYS.contains(&k.as_str()) {
                    continue;
                }
                let normalized = match k.as_str() {
                    key if OPAQUE_PAYLOAD_KEYS.contains(&key) => v.clone(),
                    "required" => normalize_required(v),
                    "type" if v.is_array() => normalize_required(v),
                    "dependentRequired" => normalize_dependent_required(v),
                    "enum" => normalize_enum(v),
                    "properties" | "patternProperties" | "definitions" | "$defs" => {
                        normalize_schema_map(v)
                    }
                    "items" => {
                        if v.is_array() {
                            Value::Array(
                                v.as_array().unwrap().iter().map(normalize_schema).collect(),
                            )
                        } else {
                            normalize_schema(v)
                        }
                    }
                    "additionalProperties"
                    | "additionalItems"
                    | "not"
                    | "contains"
                    | "propertyNames" => {
                        if v.is_boolean() {
                            v.clone()
                        } else {
                            normalize_schema(v)
                        }
                    }
                    "allOf" | "anyOf" | "oneOf" | "prefixItems" => {
                        if let Some(schemas) = v.as_array() {
                            Value::Array(schemas.iter().map(normalize_schema).collect())
                        } else {
                            v.clone()
                        }
                    }
                    _ => normalize_schema(v),
                };
                out.insert(k.clone(), normalized);
            }
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(normalize_schema).collect()),
        other => other.clone(),
    }
}

fn normalize_schema_map(v: &Value) -> Value {
    let Some(obj) = v.as_object() else {
        return normalize_schema(v);
    };
    let mut out = Map::new();
    for (k, child) in obj {
        out.insert(k.clone(), normalize_schema(child));
    }
    Value::Object(out)
}

fn normalize_required(v: &Value) -> Value {
    let Some(required) = v.as_array() else {
        return v.clone();
    };
    if !required.iter().all(Value::is_string) {
        return v.clone();
    }

    let mut keys: Vec<&str> = required.iter().filter_map(Value::as_str).collect();
    keys.sort();
    json!(keys)
}

fn normalize_dependent_required(v: &Value) -> Value {
    let Some(dependencies) = v.as_object() else {
        return v.clone();
    };
    let mut out = Map::new();
    for (property, required) in dependencies {
        out.insert(property.clone(), normalize_required(required));
    }
    Value::Object(out)
}

fn normalize_enum(v: &Value) -> Value {
    let Some(arr) = v.as_array() else {
        return v.clone();
    };
    if arr.iter().all(|x| x.is_string()) {
        let mut s: Vec<String> = arr
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect();
        s.sort();
        return json!(s);
    }
    v.clone()
}

fn diff_normalized(a: &Value, b: &Value, path: &str) -> Option<String> {
    if a == b {
        return None;
    }
    match (a, b) {
        (Value::Object(am), Value::Object(bm)) => {
            let ak: BTreeSet<_> = am.keys().collect();
            let bk: BTreeSet<_> = bm.keys().collect();
            if let Some(k) = ak.difference(&bk).next() {
                return Some(format!("{path}: missing key in right: {k}"));
            }
            if let Some(k) = bk.difference(&ak).next() {
                return Some(format!("{path}: extra key in right: {k}"));
            }
            for k in &ak {
                if let Some(d) = diff_normalized(&am[*k], &bm[*k], &format!("{path}.{k}")) {
                    return Some(d);
                }
            }
            Some(format!("{path}: object values differ"))
        }
        (Value::Array(aa), Value::Array(ba)) => {
            if aa.len() != ba.len() {
                return Some(format!("{path}: array length {} != {}", aa.len(), ba.len()));
            }
            for (i, (l, r)) in aa.iter().zip(ba.iter()).enumerate() {
                if let Some(d) = diff_normalized(l, r, &format!("{path}[{i}]")) {
                    return Some(d);
                }
            }
            Some(format!("{path}: arrays differ"))
        }
        _ => Some(format!("{path}: {a} != {b}")),
    }
}

/// Property names from an object schema's properties map (sorted).
pub fn schema_property_keys(schema: &Value) -> Vec<String> {
    let mut keys: Vec<String> = schema
        .get("properties")
        .and_then(|p| p.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    keys.sort();
    keys
}

/// Required keys from an object schema (sorted).
pub fn schema_required_keys(schema: &Value) -> Vec<String> {
    let mut keys: Vec<String> = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    keys.sort();
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn type_change_detected_when_property_names_match() {
        let a = json!({
            "type": "object",
            "properties": { "budget": { "type": "integer", "default": 1 } },
            "required": []
        });
        let b = json!({
            "type": "object",
            "properties": { "budget": { "type": "string", "default": 1 } },
            "required": []
        });
        assert!(!schemas_structurally_equal(&a, &b));
    }

    #[test]
    fn requiredness_change_detected() {
        let a = json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"]
        });
        let b = json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": []
        });
        assert!(!schemas_structurally_equal(&a, &b));
    }

    #[test]
    fn nested_constraint_change_detected() {
        let a = json!({
            "type": "object",
            "properties": { "plan": { "type": "string", "maxLength": 65536 } }
        });
        let b = json!({
            "type": "object",
            "properties": { "plan": { "type": "string", "maxLength": 1024 } }
        });
        assert!(!schemas_structurally_equal(&a, &b));
    }

    #[test]
    fn doc_keys_ignored() {
        let a = json!({ "type": "string", "description": "old words" });
        let b = json!({ "type": "string", "description": "new words" });
        assert!(schemas_structurally_equal(&a, &b));
    }

    #[test]
    fn required_order_ignored() {
        let a = json!({ "type": "object", "required": ["b", "a"] });
        let b = json!({ "type": "object", "required": ["a", "b"] });
        assert!(schemas_structurally_equal(&a, &b));
    }

    #[test]
    fn abi_hardening_type_array_order_is_set_like() {
        let a = json!({ "type": ["null", "string"] });
        let b = json!({ "type": ["string", "null"] });
        assert!(schemas_structurally_equal(&a, &b));
    }

    #[test]
    fn abi_hardening_dependent_required_order_is_set_like() {
        let a = json!({ "dependentRequired": { "credit_card": ["billing", "name"] } });
        let b = json!({ "dependentRequired": { "credit_card": ["name", "billing"] } });
        assert!(schemas_structurally_equal(&a, &b));
    }

    #[test]
    fn abi_hardening_ordinary_array_order_remains_significant() {
        let a = json!({ "default": ["a", "b"] });
        let b = json!({ "default": ["b", "a"] });
        assert!(!schemas_structurally_equal(&a, &b));
    }

    #[test]
    fn malformed_required_values_remain_distinguishable() {
        assert!(!schemas_structurally_equal(
            &json!({ "required": "query" }),
            &json!({ "required": [] })
        ));
        assert!(!schemas_structurally_equal(
            &json!({ "required": ["query", 1] }),
            &json!({ "required": ["query"] })
        ));
    }

    #[test]
    fn malformed_schema_array_keywords_remain_distinguishable() {
        for keyword in ["allOf", "anyOf", "oneOf", "prefixItems"] {
            let malformed = json!({ keyword: { "type": "string" } });
            assert!(!schemas_structurally_equal(
                &malformed,
                &json!({ keyword: [] })
            ));
            assert!(!schemas_structurally_equal(
                &malformed,
                &json!({ keyword: "malformed" })
            ));
        }
    }

    #[test]
    fn string_enum_order_ignored_mixed_enum_order_significant() {
        let a = json!({ "enum": ["b", "a"] });
        let b = json!({ "enum": ["a", "b"] });
        assert!(schemas_structurally_equal(&a, &b));
        let c = json!({ "enum": [1, "a"] });
        let d = json!({ "enum": ["a", 1] });
        assert!(!schemas_structurally_equal(&c, &d));
    }

    #[test]
    fn payload_keywords_are_opaque_while_schema_docs_are_stripped() {
        let schema = json!({
            "description": "schema documentation",
            "properties": {
                "value": {
                    "title": "property documentation",
                    "type": "object",
                    "default": {
                        "description": "payload description",
                        "title": "payload title",
                        "nested": { "description": "still payload" }
                    },
                    "const": { "title": "constant payload", "value": 1.0 },
                    "examples": [
                        { "description": "example payload", "title": "preserved" }
                    ]
                }
            }
        });

        let normalized = normalize_schema(&schema);
        assert!(normalized.get("description").is_none());
        assert!(normalized["properties"]["value"].get("title").is_none());
        assert_eq!(
            canonical_json(&normalized["properties"]["value"]["default"]),
            canonical_json(&schema["properties"]["value"]["default"])
        );
        assert_eq!(
            canonical_json(&normalized["properties"]["value"]["const"]),
            canonical_json(&schema["properties"]["value"]["const"])
        );
        assert_eq!(
            canonical_json(&normalized["properties"]["value"]["examples"]),
            canonical_json(&schema["properties"]["value"]["examples"])
        );
    }

    #[test]
    fn structural_equality_uses_canonical_number_semantics() {
        let integer: Value = serde_json::from_str(r#"{"default":1}"#).unwrap();
        let decimal: Value = serde_json::from_str(r#"{"default":1.0}"#).unwrap();

        assert_ne!(
            canonical_schema_json(&integer),
            canonical_schema_json(&decimal)
        );
        assert_ne!(
            schema_fingerprint_hex(&integer),
            schema_fingerprint_hex(&decimal)
        );
        assert!(!schemas_structurally_equal(&integer, &decimal));
    }

    #[test]
    fn equality_and_fingerprint_agree_after_key_order_normalization() {
        let a: Value = serde_json::from_str(
            r#"{"type":"object","properties":{"b":{"type":"string"},"a":{"type":"integer"}}}"#,
        )
        .unwrap();
        let b: Value = serde_json::from_str(
            r#"{"properties":{"a":{"type":"integer"},"b":{"type":"string"}},"type":"object"}"#,
        )
        .unwrap();

        assert!(schemas_structurally_equal(&a, &b));
        assert_eq!(schema_fingerprint_hex(&a), schema_fingerprint_hex(&b));
    }

    #[test]
    fn canonical_json_sorts_keys() {
        let v = json!({ "b": 1, "a": { "d": 2, "c": 3 } });
        assert_eq!(canonical_json(&v), "{\"a\":{\"c\":3,\"d\":2},\"b\":1}");
    }
}
