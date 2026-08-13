    use super::*;
    fn digest(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }
    fn closure(head: u8, extra_scope: bool) -> CertifiedInfluenceClosure {
        let mut scope = vec!["fs:file".into(), "graph:symbol".into()];
        let mut edges = vec![
            DependencyEdgeV1::new("fs:file", "graph:symbol", DependencyEdgeKindV1::Derives)
                .unwrap(),
        ];
        if extra_scope {
            scope.push("token:cache".into());
            edges.push(
                DependencyEdgeV1::new("graph:symbol", "token:cache", DependencyEdgeKindV1::Derives)
                    .unwrap(),
            );
        }
        let essential = EssentialDependencyCertificate::new(
            edges[0].clone(),
            vec!["fs:file".into(), "graph:symbol".into()],
        )
        .unwrap();
        influence_closure_v1(
            digest(9),
            vec![FreshnessHeadV1::new("ZeroStack", format!("head-{head}")).unwrap()],
            vec![
                ProducerDomainV1::FilesystemIndex,
                ProducerDomainV1::GraphIndex,
            ],
            scope,
            edges,
            vec![essential],
        )
        .unwrap()
    }
    #[test]
    fn freshness_exact_closure_passes() {
        let required = closure(1, false);
        let indexed = IndexedThroughCertificate::new("graph-index", 7, required.clone()).unwrap();
        let decision = decide_freshness_v1(&required, &indexed, 7);
        assert_eq!(decision.status, FreshnessStatusV1::Fresh);
        assert!(decision.trusted);
        assert_eq!(decision.failure_code, None);
    }
    #[test]
    fn freshness_stale_head_is_never_fresh() {
        let required = closure(2, false);
        let indexed = IndexedThroughCertificate::new("graph-index", 8, closure(1, false)).unwrap();
        let decision = decide_freshness_v1(&required, &indexed, 7);
        assert_eq!(decision.status, FreshnessStatusV1::IndexBehind);
        assert_eq!(
            decision.failure_code,
            Some(FreshnessFailureCodeV1::SourceHeadMismatch)
        );
        assert!(!decision.trusted);
        assert_eq!(decision.indexed_certificate_digest, None);
    }
    #[test]
    fn freshness_generation_rollback_is_typed() {
        let required = closure(1, false);
        let indexed = IndexedThroughCertificate::new("graph-index", 6, required.clone()).unwrap();
        let decision = decide_freshness_v1(&required, &indexed, 7);
        assert_eq!(
            decision.failure_code,
            Some(FreshnessFailureCodeV1::GenerationRollback)
        );
    }
    #[test]
    fn freshness_scope_inflation_is_unknown() {
        let required = closure(1, false);
        let indexed = IndexedThroughCertificate::new("graph-index", 7, closure(1, true)).unwrap();
        let decision = decide_freshness_v1(&required, &indexed, 7);
        assert_eq!(decision.status, FreshnessStatusV1::Unknown);
        assert_eq!(
            decision.failure_code,
            Some(FreshnessFailureCodeV1::ScopeInflation)
        );
    }
    #[test]
    fn freshness_missing_edge_is_not_fresh() {
        let required = closure(1, true);
        let indexed = IndexedThroughCertificate::new("graph-index", 7, closure(1, false)).unwrap();
        let decision = decide_freshness_v1(&required, &indexed, 7);
        assert_ne!(decision.status, FreshnessStatusV1::Fresh);
        assert!(!decision.trusted);
    }
    #[test]
    fn freshness_replay_identity_mutation_is_rejected() {
        let required = closure(1, false);
        let mut indexed =
            IndexedThroughCertificate::new("graph-index", 7, required.clone()).unwrap();
        indexed.replay_identity = digest(55);
        let decision = decide_freshness_v1(&required, &indexed, 7);
        assert_eq!(
            decision.failure_code,
            Some(FreshnessFailureCodeV1::ReplayIdentityMismatch)
        );
        assert!(!decision.trusted);
    }
    #[test]
    fn freshness_canonical_bytes_and_contract_digest_are_stable() {
        let required = closure(1, false);
        assert_eq!(
            required.canonical_bytes().unwrap(),
            required.canonical_bytes().unwrap()
        );
        assert_ne!(freshness_contract_digest_v1(), DigestV1::ZERO);
        assert_eq!(
            freshness_contract_manifest_v1()["wall_clock_is_authority"],
            false
        );
    }
    #[test]
    fn freshness_duplicate_repository_identity_is_rejected() {
        let mut value = closure(1, false);
        value
            .source_repository_heads
            .push(FreshnessHeadV1::new("ZeroStack", "head-2").unwrap());
        value.source_repository_heads.sort();
        assert_eq!(
            value.validate().unwrap_err().code,
            FreshnessFailureCodeV1::DuplicateIdentity
        );
    }

    #[test]
    fn freshness_old_schema_version_has_typed_outcome() {
        let mut indexed =
            IndexedThroughCertificate::new("graph-index", 7, closure(1, false)).unwrap();
        indexed.schema_version = 0;
        assert_eq!(
            indexed.validate().unwrap_err().code,
            FreshnessFailureCodeV1::UnsupportedSchemaVersion
        );
    }

    #[test]
    fn freshness_wire_shape_rejects_unknown_fields() {
        let required = closure(1, false);
        let mut value = serde_json::to_value(required).unwrap();
        value["timestamp"] = json!("2099-01-01T00:00:00Z");
        assert!(serde_json::from_value::<CertifiedInfluenceClosure>(value).is_err());
    }
