    use super::*;
    use serde_json::{Map, json};

    fn root(name: &str) -> CacheRootV1 {
        CacheRootV1::new(name).unwrap()
    }

    fn witness() -> CompletenessWitnessV1 {
        CompletenessWitnessV1::new(
            root("proof"),
            vec![
                root("scope"),
                root("dep-a"),
                root("dep-b"),
                root("env"),
                root("toolchain"),
            ],
        )
        .unwrap()
    }

    fn positive_key(parameters: Value) -> CacheKeyV1 {
        CacheKeyV1::new(
            OperatorIdentityV1::new("graph.snap", "2").unwrap(),
            parameters,
            vec![root("dep-b"), root("dep-a")],
            vec![root("env")],
            vec![root("toolchain")],
            witness(),
        )
        .unwrap()
    }

    #[test]
    fn round_trip_positive_and_negative_entries() {
        let positive = CacheEntryV1::positive(
            positive_key(json!({"query": "needle"})),
            root("output"),
            Some(VerifierReceiptV1::new("graph.verifier", root("receipt")).unwrap()),
        )
        .unwrap();
        let negative = CacheEntryV1::negative(
            CacheKeyV1::with_scope_roots(
                OperatorIdentityV1::new("fs.search", "1").unwrap(),
                json!({"query": "absent"}),
                vec![root("dep-a")],
                vec![root("env")],
                vec![root("toolchain")],
                witness(),
                vec![root("scope")],
            )
            .unwrap(),
        )
        .unwrap();

        for entry in [positive, negative] {
            let encoded = serde_json::to_string(&entry).unwrap();
            let decoded: CacheEntryV1 = serde_json::from_str(&encoded).unwrap();
            assert_eq!(entry, decoded);
        }
    }

    #[test]
    fn canonical_hash_is_stable_for_semantically_equal_keys() {
        let mut params_a = Map::new();
        params_a.insert("z".to_owned(), json!(1));
        params_a.insert("a".to_owned(), json!(2));
        let mut params_b = Map::new();
        params_b.insert("a".to_owned(), json!(2));
        params_b.insert("z".to_owned(), json!(1));
        let a = positive_key(Value::Object(params_a));
        let b = CacheKeyV1::new(
            OperatorIdentityV1::new("graph.snap", "2").unwrap(),
            Value::Object(params_b),
            vec![root("dep-a"), root("dep-b")],
            vec![root("env")],
            vec![root("toolchain")],
            CompletenessWitnessV1::new(
                root("proof"),
                vec![
                    root("dep-a"),
                    root("dep-b"),
                    root("scope"),
                    root("env"),
                    root("toolchain"),
                ],
            )
            .unwrap(),
        )
        .unwrap();

        assert_eq!(a.canonical_key_json(), b.canonical_key_json());
        assert_eq!(a.key_hash_hex(), b.key_hash_hex());
    }

    #[test]
    fn negative_entry_requires_and_carries_scope_roots() {
        let key = CacheKeyV1::with_scope_roots(
            OperatorIdentityV1::new("fs.search", "1").unwrap(),
            json!({"query": "absent"}),
            vec![],
            vec![root("env")],
            vec![root("toolchain")],
            witness(),
            vec![root("scope")],
        )
        .unwrap();
        let entry = CacheEntryV1::negative(key).unwrap();
        assert!(matches!(entry.value(), CacheValueV1::NoMatches));
        assert_eq!(entry.key().scope_roots(), &[root("scope")]);
        assert!(CacheEntryV1::negative(positive_key(json!({}))).is_err());
    }

    #[test]
    fn missing_completeness_witness_is_rejected() {
        let entry = CacheEntryV1::positive(positive_key(json!({})), root("output"), None).unwrap();
        let mut wire = serde_json::to_value(entry).unwrap();
        wire["key"]
            .as_object_mut()
            .unwrap()
            .remove("completeness_witness");
        assert!(serde_json::from_value::<CacheEntryV1>(wire).is_err());
    }

    #[test]
    fn mutation_without_witness_update_is_rejected() {
        let entry = CacheEntryV1::positive(positive_key(json!({})), root("output"), None).unwrap();
        let mut wire = serde_json::to_value(entry).unwrap();
        wire["key"]["minimum_dependency_roots"][0] = json!("dep-mutated");
        assert!(serde_json::from_value::<CacheEntryV1>(wire).is_err());
    }
