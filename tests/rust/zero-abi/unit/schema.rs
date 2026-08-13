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
