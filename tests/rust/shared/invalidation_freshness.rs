//! Frozen Z3 vector replay and schema/ABI correspondence.

use serde::Deserialize;
use serde_json::{Value, json};
use zero_abi::{
    CertifiedInfluenceClosure, DependencyEdgeKindV1, DependencyEdgeV1, DigestV1,
    EssentialDependencyCertificate, FreshnessFailureCodeV1, FreshnessHeadV1, FreshnessStatusV1,
    IndexedThroughCertificate, ProducerDomainV1, canonical_json, decide_freshness_v1, sha256_hex,
};
use zerostack_shared_tests::racc::{RACC_INVALIDATION_FRESHNESS_SCHEMA, validate_racc_schema};

const VECTORS: &str = include_str!("../fixtures/invalidation-freshness-v1.json");

#[derive(Deserialize)]
struct Vectors {
    assembly_manifest_digest: String,
    required_closure: CertifiedInfluenceClosure,
    canonical_fresh_certificate: IndexedThroughCertificate,
    canonical_fresh_bytes: String,
    canonical_fresh_bytes_sha256: String,
    cases: Vec<Case>,
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

fn edge(producer: &str, consumer: &str) -> DependencyEdgeV1 {
    DependencyEdgeV1::new(producer, consumer, DependencyEdgeKindV1::Derives).unwrap()
}
fn closure(
    assembly: &str,
    head: &str,
    extra: bool,
    omit_last_edge: bool,
) -> CertifiedInfluenceClosure {
    let first = edge("fs:file", "graph:symbol");
    let essential = EssentialDependencyCertificate::new(
        first.clone(),
        vec!["fs:file".into(), "graph:symbol".into()],
    )
    .unwrap();
    let mut scope = vec!["fs:file".into(), "graph:symbol".into()];
    let mut edges = vec![first];
    let mut domains = vec![
        ProducerDomainV1::FilesystemIndex,
        ProducerDomainV1::GraphIndex,
    ];
    if extra {
        scope.push("token:cache".into());
        if !omit_last_edge {
            edges.push(edge("graph:symbol", "token:cache"));
        }
        domains.push(ProducerDomainV1::TokenCache);
    }
    CertifiedInfluenceClosure::new(
        DigestV1::from_hex(assembly).unwrap(),
        vec![FreshnessHeadV1::new("ZeroStack", head).unwrap()],
        domains,
        scope,
        edges,
        vec![essential],
    )
    .unwrap()
}
fn indexed(vectors: &Vectors, case: &Case) -> IndexedThroughCertificate {
    let influence = closure(
        &vectors.assembly_manifest_digest,
        &case.indexed_head,
        case.indexed_extra_scope,
        case.indexed_omit_last_edge,
    );
    let mut certificate =
        IndexedThroughCertificate::new("graph-index", case.index_generation, influence).unwrap();
    if case.mutate_replay {
        certificate.replay_identity = DigestV1::from_bytes([0x55; 32]);
    }
    certificate
}

#[test]
fn freshness_vectors_are_canonical_and_byte_exact() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    let bytes = vectors
        .canonical_fresh_certificate
        .canonical_bytes()
        .unwrap();
    assert_eq!(
        String::from_utf8(bytes.clone()).unwrap(),
        vectors.canonical_fresh_bytes
    );
    assert_eq!(sha256_hex(&bytes), vectors.canonical_fresh_bytes_sha256);
    let reparsed: Value = serde_json::from_str(&vectors.canonical_fresh_bytes).unwrap();
    assert_eq!(canonical_json(&reparsed), vectors.canonical_fresh_bytes);
}

#[test]
fn freshness_schema_accepts_exact_fresh_document() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    let decision = decide_freshness_v1(
        &vectors.required_closure,
        &vectors.canonical_fresh_certificate,
        7,
    );
    let document = json!({
        "schema_version": 1,
        "model_version": "zerostack.invalidation-freshness.v1",
        "certificate": vectors.canonical_fresh_certificate,
        "result": decision,
    });
    validate_racc_schema(RACC_INVALIDATION_FRESHNESS_SCHEMA, &document).unwrap();
}

#[test]
fn freshness_stale_replay_generation_and_inflation_mutants_are_typed() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    for case in &vectors.cases {
        let required = if case.required_extra_scope {
            closure(
                &vectors.assembly_manifest_digest,
                &vectors.required_closure.source_repository_heads[0].head,
                true,
                false,
            )
        } else {
            vectors.required_closure.clone()
        };
        let decision =
            decide_freshness_v1(&required, &indexed(&vectors, case), case.minimum_generation);
        let value = serde_json::to_value(&decision).unwrap();
        assert_eq!(value["status"], case.expected_status, "{}", case.case_id);
        assert_eq!(
            value["failure_code"],
            case.expected_failure_code
                .as_ref()
                .map_or(Value::Null, |value| Value::String(value.clone())),
            "{}",
            case.case_id
        );
        if decision.status != FreshnessStatusV1::Fresh {
            assert!(!decision.trusted, "{}", case.case_id);
            assert_eq!(
                decision.indexed_certificate_digest, None,
                "{}",
                case.case_id
            );
        }
    }
}

#[test]
fn freshness_missing_edge_is_loud_and_non_promotable() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    let head = &vectors.required_closure.source_repository_heads[0].head;
    let required_first = edge("fs:file", "graph:symbol");
    let required = CertifiedInfluenceClosure::new(
        DigestV1::from_hex(&vectors.assembly_manifest_digest).unwrap(),
        vec![FreshnessHeadV1::new("ZeroStack", head).unwrap()],
        vec![
            ProducerDomainV1::FilesystemIndex,
            ProducerDomainV1::GraphIndex,
            ProducerDomainV1::TokenCache,
        ],
        vec![
            "fs:file".into(),
            "graph:symbol".into(),
            "token:cache".into(),
        ],
        vec![required_first.clone(), edge("graph:symbol", "token:cache")],
        vec![
            EssentialDependencyCertificate::new(
                required_first.clone(),
                vec!["fs:file".into(), "graph:symbol".into()],
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let incomplete = CertifiedInfluenceClosure::new(
        DigestV1::from_hex(&vectors.assembly_manifest_digest).unwrap(),
        vec![FreshnessHeadV1::new("ZeroStack", head).unwrap()],
        vec![
            ProducerDomainV1::FilesystemIndex,
            ProducerDomainV1::GraphIndex,
            ProducerDomainV1::TokenCache,
        ],
        vec![
            "fs:file".into(),
            "graph:symbol".into(),
            "token:cache".into(),
        ],
        vec![required_first.clone()],
        vec![
            EssentialDependencyCertificate::new(
                required_first,
                vec!["fs:file".into(), "graph:symbol".into()],
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let certificate = IndexedThroughCertificate::new("graph-index", 7, incomplete).unwrap();
    let decision = decide_freshness_v1(&required, &certificate, 7);
    assert_eq!(decision.status, FreshnessStatusV1::IndexBehind);
    assert_eq!(
        decision.failure_code,
        Some(FreshnessFailureCodeV1::MissingEdge)
    );
    assert!(!decision.trusted);
}

#[test]
fn freshness_schema_rejects_partial_trust_and_shape_drift() {
    let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
    let bad = json!({
        "schema_version": 1,
        "model_version": "zerostack.invalidation-freshness.v1",
        "certificate": vectors.canonical_fresh_certificate,
        "result": {
            "schema_version": 1,
            "status": "unknown",
            "trusted": true,
            "failure_code": "INCOMPARABLE_SCOPE",
            "detail": "mutant",
            "indexed_certificate_digest": null
        },
        "timestamp": "2099-01-01T00:00:00Z"
    });
    assert!(validate_racc_schema(RACC_INVALIDATION_FRESHNESS_SCHEMA, &bad).is_err());
}
