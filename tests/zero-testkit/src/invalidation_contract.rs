//! Cross-engine KATs for the hub-owned freshness and invalidation schema.

/// Frozen canonical vectors consumed by engine adoption suites without peer imports.
pub const INVALIDATION_FRESHNESS_VECTORS_V1: &str =
    include_str!("../conformance/invalidation-freshness/v1/vectors.json");

#[cfg(test)]
mod tests {
    use super::INVALIDATION_FRESHNESS_VECTORS_V1;
    use serde::Deserialize;
    use std::{collections::BTreeMap, fs, path::PathBuf, process::Command};
    use zero_abi::{
        CertifiedInfluenceClosure, DependencyEdgeKindV1, DependencyEdgeV1, DigestV1,
        EssentialDependencyCertificate, FreshnessFailureCodeV1, FreshnessHeadV1, FreshnessStatusV1,
        IndexedThroughCertificate, ProducerDomainV1, decide_freshness_v1, sha256_hex,
    };

    #[derive(Deserialize)]
    struct Vectors {
        assembly_manifest_digest: String,
        required_closure: CertifiedInfluenceClosure,
        canonical_fresh_certificate: IndexedThroughCertificate,
        canonical_fresh_bytes: String,
        canonical_fresh_bytes_sha256: String,
        cases: Vec<Case>,
        clock_skew_vector: ClockSkew,
    }

    #[derive(Deserialize)]
    struct Case {
        case_id: String,
        indexed_head: String,
        indexed_extra_scope: bool,
        #[serde(default)]
        required_extra_scope: bool,
        #[serde(default)]
        indexed_omit_last_edge: bool,
        index_generation: u64,
        minimum_generation: u64,
        mutate_replay: bool,
        expected_status: String,
        expected_failure_code: Option<String>,
    }

    #[derive(Deserialize)]
    struct ArchiveIndex {
        schema_version: u32,
        vector_set: String,
        evidence_scope: String,
        files: BTreeMap<String, String>,
        immutable: bool,
    }

    #[derive(Deserialize)]
    struct ClockSkew {
        wall_clock_delta_seconds: i64,
        expected_effect: String,
    }

    fn digest(value: &str) -> DigestV1 {
        DigestV1::from_hex(value).unwrap()
    }

    fn edge(producer: &str, consumer: &str) -> DependencyEdgeV1 {
        DependencyEdgeV1::new(producer, consumer, DependencyEdgeKindV1::Derives).unwrap()
    }

    fn closure(
        assembly: &str,
        head: &str,
        scope: &[&str],
        edges: Vec<DependencyEdgeV1>,
        essential: Vec<EssentialDependencyCertificate>,
    ) -> CertifiedInfluenceClosure {
        let mut domains = vec![
            ProducerDomainV1::FilesystemIndex,
            ProducerDomainV1::GraphIndex,
        ];
        if scope.contains(&"token:cache") {
            domains.push(ProducerDomainV1::TokenCache);
        }
        CertifiedInfluenceClosure::new(
            digest(assembly),
            vec![FreshnessHeadV1::new("ZeroStack", head).unwrap()],
            domains,
            scope.iter().map(|value| (*value).into()).collect(),
            edges,
            essential,
        )
        .unwrap()
    }

    fn indexed_for(vectors: &Vectors, case: &Case) -> IndexedThroughCertificate {
        let first = edge("fs:file", "graph:symbol");
        let essential = EssentialDependencyCertificate::new(
            first.clone(),
            vec!["fs:file".into(), "graph:symbol".into()],
        )
        .unwrap();
        let (scope, edges) = if case.indexed_extra_scope {
            (
                vec!["fs:file", "graph:symbol", "token:cache"],
                if case.indexed_omit_last_edge {
                    vec![first]
                } else {
                    vec![first, edge("graph:symbol", "token:cache")]
                },
            )
        } else {
            (vec!["fs:file", "graph:symbol"], vec![first])
        };
        let influence = closure(
            &vectors.assembly_manifest_digest,
            &case.indexed_head,
            &scope,
            edges,
            vec![essential],
        );
        let mut indexed =
            IndexedThroughCertificate::new("graph-index", case.index_generation, influence)
                .unwrap();
        if case.mutate_replay {
            indexed.replay_identity = DigestV1::from_bytes([0x55; 32]);
        }
        indexed
    }

    fn archive_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/invalidation-freshness/v1")
    }

    #[test]
    fn invalidation_contract_archive_is_hash_bound_and_cross_language() {
        let root = archive_dir();
        let index: ArchiveIndex =
            serde_json::from_slice(&fs::read(root.join("index.json")).unwrap()).unwrap();
        assert_eq!(index.schema_version, 1);
        assert_eq!(index.vector_set, "zerostack.invalidation-freshness-kat.v1");
        assert_eq!(index.evidence_scope, "cross_language_kat_only");
        assert!(index.immutable);
        for (path, expected) in index.files {
            assert_eq!(
                sha256_hex(&fs::read(root.join(&path)).unwrap()),
                expected,
                "{path}"
            );
        }
        let output = Command::new("python3")
            .arg(root.join("runners/python/verify_v1.py"))
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Python runner failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            "invalidation_freshness_kat:python:v1:passed"
        );
    }

    #[test]
    fn invalidation_contract_vectors_are_exact_and_canonical() {
        let vectors: Vectors = serde_json::from_str(INVALIDATION_FRESHNESS_VECTORS_V1).unwrap();
        vectors.required_closure.validate().unwrap();
        vectors.canonical_fresh_certificate.validate().unwrap();
        let bytes = vectors
            .canonical_fresh_certificate
            .canonical_bytes()
            .unwrap();
        assert_eq!(
            String::from_utf8(bytes.clone()).unwrap(),
            vectors.canonical_fresh_bytes
        );
        assert_eq!(sha256_hex(&bytes), vectors.canonical_fresh_bytes_sha256);
    }

    #[test]
    fn invalidation_contract_stale_race_and_replay_vectors_fail_closed() {
        let vectors: Vectors = serde_json::from_str(INVALIDATION_FRESHNESS_VECTORS_V1).unwrap();
        for case in &vectors.cases {
            let indexed = indexed_for(&vectors, case);
            let required = if case.required_extra_scope {
                let first = edge("fs:file", "graph:symbol");
                closure(
                    &vectors.assembly_manifest_digest,
                    &vectors.required_closure.source_repository_heads[0].head,
                    &["fs:file", "graph:symbol", "token:cache"],
                    vec![first.clone(), edge("graph:symbol", "token:cache")],
                    vec![
                        EssentialDependencyCertificate::new(
                            first,
                            vec!["fs:file".into(), "graph:symbol".into()],
                        )
                        .unwrap(),
                    ],
                )
            } else {
                vectors.required_closure.clone()
            };
            let decision = decide_freshness_v1(&required, &indexed, case.minimum_generation);
            let value = serde_json::to_value(&decision).unwrap();
            assert_eq!(
                value["status"], case.expected_status,
                "{} status",
                case.case_id
            );
            assert_eq!(
                value["failure_code"],
                case.expected_failure_code
                    .as_ref()
                    .map_or(serde_json::Value::Null, |code| serde_json::Value::String(
                        code.clone()
                    )),
                "{} failure code",
                case.case_id
            );
            assert_eq!(
                decision.trusted,
                case.expected_status == "fresh",
                "{} trust",
                case.case_id
            );
            if !decision.trusted {
                assert_eq!(decision.indexed_certificate_digest, None);
            }
        }
    }

    #[test]
    fn invalidation_contract_missing_edge_returns_index_behind() {
        let vectors: Vectors = serde_json::from_str(INVALIDATION_FRESHNESS_VECTORS_V1).unwrap();
        let first = edge("fs:file", "graph:symbol");
        let second = edge("graph:symbol", "token:cache");
        let required = closure(
            &vectors.assembly_manifest_digest,
            &vectors.required_closure.source_repository_heads[0].head,
            &["fs:file", "graph:symbol", "token:cache"],
            vec![first.clone(), second],
            vec![
                EssentialDependencyCertificate::new(
                    first.clone(),
                    vec!["fs:file".into(), "graph:symbol".into()],
                )
                .unwrap(),
            ],
        );
        let incomplete = closure(
            &vectors.assembly_manifest_digest,
            &vectors.required_closure.source_repository_heads[0].head,
            &["fs:file", "graph:symbol", "token:cache"],
            vec![first.clone()],
            vec![
                EssentialDependencyCertificate::new(
                    first,
                    vec!["fs:file".into(), "graph:symbol".into()],
                )
                .unwrap(),
            ],
        );
        let indexed = IndexedThroughCertificate::new("graph-index", 7, incomplete).unwrap();
        let decision = decide_freshness_v1(&required, &indexed, 7);
        assert_eq!(decision.status, FreshnessStatusV1::IndexBehind);
        assert_eq!(
            decision.failure_code,
            Some(FreshnessFailureCodeV1::MissingEdge)
        );
        assert!(!decision.trusted);
    }

    #[test]
    fn invalidation_contract_missing_scope_is_never_certified() {
        let vectors: Vectors = serde_json::from_str(INVALIDATION_FRESHNESS_VECTORS_V1).unwrap();
        let first = edge("fs:file", "graph:symbol");
        let required = closure(
            &vectors.assembly_manifest_digest,
            &vectors.required_closure.source_repository_heads[0].head,
            &["fs:file", "graph:symbol", "token:cache"],
            vec![first.clone(), edge("graph:symbol", "token:cache")],
            vec![
                EssentialDependencyCertificate::new(
                    first.clone(),
                    vec!["fs:file".into(), "graph:symbol".into()],
                )
                .unwrap(),
            ],
        );
        let incomplete = closure(
            &vectors.assembly_manifest_digest,
            &vectors.required_closure.source_repository_heads[0].head,
            &["fs:file", "graph:symbol"],
            vec![first.clone()],
            vec![
                EssentialDependencyCertificate::new(
                    first,
                    vec!["fs:file".into(), "graph:symbol".into()],
                )
                .unwrap(),
            ],
        );
        let indexed = IndexedThroughCertificate::new("graph-index", 7, incomplete).unwrap();
        let decision = decide_freshness_v1(&required, &indexed, 7);
        assert!(matches!(
            decision.status,
            FreshnessStatusV1::IndexBehind | FreshnessStatusV1::Unknown
        ));
        assert!(!decision.trusted);
    }

    #[test]
    fn invalidation_contract_clock_skew_cannot_change_identity_result() {
        let vectors: Vectors = serde_json::from_str(INVALIDATION_FRESHNESS_VECTORS_V1).unwrap();
        assert!(vectors.clock_skew_vector.wall_clock_delta_seconds.abs() > 31_000_000);
        assert_eq!(vectors.clock_skew_vector.expected_effect, "none");
        let decision = decide_freshness_v1(
            &vectors.required_closure,
            &vectors.canonical_fresh_certificate,
            7,
        );
        assert_eq!(decision.status, FreshnessStatusV1::Fresh);
    }

    #[test]
    fn invalidation_contract_unknown_fields_are_rejected() {
        let vectors: Vectors = serde_json::from_str(INVALIDATION_FRESHNESS_VECTORS_V1).unwrap();
        let mut value = serde_json::to_value(vectors.canonical_fresh_certificate).unwrap();
        value["collected_at"] = serde_json::json!("2099-01-01T00:00:00Z");
        assert!(serde_json::from_value::<IndexedThroughCertificate>(value).is_err());
    }
}
