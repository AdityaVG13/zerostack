//! SPEC-TZ-TOK-001 / CAP-001 / CAP-002: exact identity + TokenPage MAX_* + expand.
//!
//! Minimal public-API driver. Does not treat dual-store fragment expand as
//! TokenPage/ModelCapsule round-trip.

use tokenzero_core::model_artifacts::{
    ExactTokenMap, ExactTokenizerAdapter, ExactTokenizerIdentity, MAX_CAPSULE_EVIDENCE_REFS,
    MAX_CAPSULE_RENDER_BYTES, MAX_CAPSULE_TOKEN_PAGES, MAX_TOKEN_PAGE_BYTES, MAX_TOKEN_PAGE_TOKENS,
    ModelArtifactError, ModelCapsule, TokenPage,
};
use tokenzero_core::{
    Accounting, FORBIDDEN_MCP_ENGINE_IDENTITY, FORBIDDEN_MCP_REGISTRY_ENGINE, LEXICAL_ESTIMATOR_ID,
    TokenizerIdPreflightError, UNLABELED_ESTIMATE_TOKENIZER_PREFIX, count_tokens_tokenizer_id,
    is_forbidden_mcp_tokenizer_identity, preflight_tokenizer_id, sha256_hex,
};
use tokenzero_test_support::{
    ExecutionEnvelope, GauntletIdentityPair, GauntletOracle, ScenarioAgreement, scenario,
};
use zero_gauge::ProviderLock;

fn stamp_subject_ne_oracle() {
    GauntletIdentityPair::new(GauntletOracle::Spec).assert_distinct();
}

struct ByteAdapter {
    identity: ExactTokenizerIdentity,
}

impl ExactTokenizerAdapter for ByteAdapter {
    fn identity(&self) -> &ExactTokenizerIdentity {
        &self.identity
    }

    fn encode(&self, source: &[u8]) -> Result<Vec<u32>, String> {
        Ok(source.iter().copied().map(u32::from).collect())
    }

    fn token_bytes(&self, token_id: u32) -> Result<Vec<u8>, String> {
        let byte = u8::try_from(token_id).map_err(|err| err.to_string())?;
        Ok(vec![byte])
    }
}

fn adapter() -> ByteAdapter {
    let manifest = b"gauntlet-phase2-fake-tokenizer-rev";
    let digest = sha256_hex(std::str::from_utf8(manifest).expect("ascii manifest"));
    let identity = ExactTokenizerIdentity::new(
        ProviderLock {
            provider: "gauntlet".to_string(),
            model: "phase2".to_string(),
            tokenizer_revision_digest: digest,
        },
        manifest,
    )
    .expect("identity");
    ByteAdapter { identity }
}

fn blob_anchor(map: &ExactTokenMap) -> String {
    format!("tz://blob/{}", map.source_digest().to_hex())
}

#[test]
fn tokenizer_id_preflight_refuses_unlabeled_estimate_and_q99_as_exact() {
    stamp_subject_ne_oracle();
    assert_eq!(UNLABELED_ESTIMATE_TOKENIZER_PREFIX, "estimate:");
    assert_eq!(
        preflight_tokenizer_id("estimate:tokenzero-lexical"),
        Err(TokenizerIdPreflightError::UnlabeledEstimateAlias)
    );
    assert_eq!(
        preflight_tokenizer_id("estimator:tokenzero-lexical"),
        Ok(())
    );
    assert_eq!(
        preflight_tokenizer_id("Q99"),
        Err(TokenizerIdPreflightError::Q99IsNotExact)
    );
    assert_eq!(
        preflight_tokenizer_id("tiktoken:Q99"),
        Err(TokenizerIdPreflightError::Q99IsNotExact)
    );
    assert_eq!(
        preflight_tokenizer_id("exact"),
        Err(TokenizerIdPreflightError::ExactLabelIsNotATokenizerId)
    );
    assert_eq!(preflight_tokenizer_id("tiktoken:o200k_base"), Ok(()));
    assert_eq!(
        preflight_tokenizer_id(FORBIDDEN_MCP_ENGINE_IDENTITY),
        Err(TokenizerIdPreflightError::McpRegistryIdentity)
    );
    assert_eq!(
        preflight_tokenizer_id(FORBIDDEN_MCP_REGISTRY_ENGINE),
        Err(TokenizerIdPreflightError::McpRegistryIdentity)
    );
    assert!(is_forbidden_mcp_tokenizer_identity(
        FORBIDDEN_MCP_ENGINE_IDENTITY
    ));
}

#[test]
fn mcp_accounting_json_stamps_kernel_measure_tokenizer_id() {
    stamp_subject_ne_oracle();
    let accounting = Accounting::measured(10, 4, 6, 4, 0, Some(2));
    assert_eq!(accounting.tokenizer_id, count_tokens_tokenizer_id());
    assert_eq!(accounting.tokenizer_id, LEXICAL_ESTIMATOR_ID);
    preflight_tokenizer_id(&accounting.tokenizer_id).expect("kernel estimator must pass preflight");
    assert!(
        !accounting.tokenizer_id.contains('@'),
        "MCP accounting must not invent ExactTokenizerIdentity"
    );
    assert!(!accounting.certified);
    assert_eq!(accounting.spent_tokens(), 10);
    assert_eq!(accounting.recovered_tokens(), 6);
    let json = serde_json::to_value(&accounting).expect("serialize");
    assert_eq!(json["tokenizer_id"], LEXICAL_ESTIMATOR_ID);
    assert_eq!(json["certified"], false);
    assert_eq!(json["spent_tokens"], 10);
    assert_eq!(json["recovered_tokens"], 6);
    assert_eq!(json["raw_tokens"], 10);
    assert_eq!(json["visible_tokens"], 4);
    assert!(
        json["tokenizer_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("estimator:")),
        "{json}"
    );
}

#[test]
fn mcp_accounting_replaces_unlabeled_estimate_and_never_certifies_estimator() {
    stamp_subject_ne_oracle();
    let mut accounting = Accounting::measured(3, 3, 0, 3, 0, None);
    accounting.tokenizer_id = "estimate:tokenzero-lexical".into();
    accounting.certified = true;
    accounting.stamp_tokenizer();
    assert_eq!(accounting.tokenizer_id, LEXICAL_ESTIMATOR_ID);
    assert!(!accounting.certified);
    let unlabeled: Accounting = serde_json::from_value(
        serde_json::json!({"raw_tokens": 1, "visible_tokens": 1, "recovery_tokens": 0}),
    )
    .expect("legacy accounting without tokenizer_id");
    assert_eq!(unlabeled.tokenizer_id, LEXICAL_ESTIMATOR_ID);
}

#[test]
fn mcp_accounting_does_not_launder_engine_identity_into_estimator() {
    stamp_subject_ne_oracle();
    let mut accounting = Accounting::measured(3, 3, 0, 3, 0, None);
    accounting.tokenizer_id = FORBIDDEN_MCP_ENGINE_IDENTITY.into();
    accounting.certified = true;
    accounting.stamp_tokenizer();
    assert_eq!(accounting.tokenizer_id, FORBIDDEN_MCP_ENGINE_IDENTITY);
    assert!(!accounting.certified);
    assert_eq!(
        preflight_tokenizer_id(&accounting.tokenizer_id),
        Err(TokenizerIdPreflightError::McpRegistryIdentity)
    );

    accounting.tokenizer_id = FORBIDDEN_MCP_REGISTRY_ENGINE.into();
    accounting.stamp_tokenizer();
    assert_eq!(accounting.tokenizer_id, FORBIDDEN_MCP_REGISTRY_ENGINE);
}

#[test]
fn exact_tokenizer_identity_rejects_revision_digest_mismatch() {
    stamp_subject_ne_oracle();
    let expected = sha256_hex("gauntlet-expected-manifest");
    let err = ExactTokenizerIdentity::new(
        ProviderLock {
            provider: "gauntlet".to_string(),
            model: "phase2".to_string(),
            tokenizer_revision_digest: expected.clone(),
        },
        b"different-manifest-bytes",
    )
    .expect_err("mismatch must fail loud");
    match err {
        ModelArtifactError::TokenizerRevisionDigestMismatch {
            expected: got_expected,
            actual,
        } => {
            assert_eq!(got_expected, expected);
            assert_ne!(actual, expected);
        }
        other => panic!("expected digest mismatch, got {other:?}"),
    }
}

#[test]
fn token_page_and_capsule_max_contracts() {
    stamp_subject_ne_oracle();
    assert_eq!(MAX_TOKEN_PAGE_TOKENS, 4_096);
    assert_eq!(MAX_TOKEN_PAGE_BYTES, 1_048_576);
    assert_eq!(MAX_CAPSULE_EVIDENCE_REFS, 4_096);
    assert_eq!(MAX_CAPSULE_TOKEN_PAGES, 4_096);
    assert_eq!(MAX_CAPSULE_RENDER_BYTES, 16 * 1_048_576);
}

fn spec_empty_range_is_illegal(start: usize, end: usize) -> Result<(), ModelArtifactError> {
    if start >= end {
        Err(ModelArtifactError::EmptyTokenPage)
    } else {
        Ok(())
    }
}

#[test]
fn empty_token_page_both_error_is_spec_agreement() {
    stamp_subject_ne_oracle();
    let pair = GauntletIdentityPair::new(GauntletOracle::Spec);
    let envelope = ExecutionEnvelope::from_pair("empty-token-page", 1, pair, vec!["0..0".into()]);
    envelope.assert_engine_identities(pair);
    let adapter = adapter();
    let map = ExactTokenMap::tokenize(&adapter, b"abc").expect("map");
    let anchor = blob_anchor(&map);
    match scenario(
        "empty-token-page",
        pair,
        || TokenPage::new(&map, &anchor, 0..0).map(|page| page.expand()),
        || spec_empty_range_is_illegal(0, 0),
    ) {
        ScenarioAgreement::BothErr { subject, oracle } => {
            assert_eq!(subject, ModelArtifactError::EmptyTokenPage);
            assert_eq!(oracle, ModelArtifactError::EmptyTokenPage);
        }
        ScenarioAgreement::BothOk(_) => panic!("empty page must be both-error agreement, not Ok"),
    }
}

#[test]
fn token_page_expand_through_spec_scenario() {
    stamp_subject_ne_oracle();
    let pair = GauntletIdentityPair::new(GauntletOracle::Spec);
    let envelope = ExecutionEnvelope::from_pair("token-page-expand", 2, pair, vec!["0..3".into()]);
    envelope.assert_engine_identities(pair);
    let adapter = adapter();
    let source = b"abc";
    let map = ExactTokenMap::tokenize(&adapter, source).expect("map");
    let anchor = blob_anchor(&map);
    match scenario(
        "token-page-expand",
        pair,
        || TokenPage::new(&map, &anchor, 0..3).map(|page| page.expand()),
        || spec_empty_range_is_illegal(0, 3),
    ) {
        ScenarioAgreement::BothOk(bytes) => assert_eq!(bytes, source),
        ScenarioAgreement::BothErr { subject, oracle } => {
            panic!("in-range page must be BothOk, got subject={subject:?} oracle={oracle:?}")
        }
    }
}

#[test]
fn token_page_subrange_expand_equals_source_slice() {
    stamp_subject_ne_oracle();
    let adapter = adapter();
    let source = b"abcdef";
    let map = ExactTokenMap::tokenize(&adapter, source).expect("map");
    let anchor = blob_anchor(&map);
    let page = TokenPage::new(&map, &anchor, 2..5).expect("page");
    assert_eq!(page.expand(), &source[2..5]);
    assert_eq!(page.expand(), map.reconstruct()[2..5]);
}

#[test]
fn model_capsule_render_is_prefix_concat_tail() {
    stamp_subject_ne_oracle();
    let adapter = adapter();
    let source = b"hello world";
    let full = ExactTokenMap::tokenize(&adapter, source).expect("full");
    let prefix = ExactTokenMap::tokenize(&adapter, b"hello").expect("prefix");
    let tail = ExactTokenMap::tokenize(&adapter, b" world").expect("tail");
    let anchor = blob_anchor(&full);
    let page = TokenPage::new(&full, &anchor, 0..source.len()).expect("page");
    let capsule = ModelCapsule::new(
        full.source_digest(),
        ModelCapsule::absent_model_profile_digest(),
        adapter.identity(),
        Vec::new(),
        &[page],
        &prefix,
        &tail,
    )
    .expect("capsule");
    assert_eq!(capsule.render(), source);
    assert_eq!(capsule.stable_prefix(), b"hello");
    assert_eq!(capsule.dynamic_tail(), b" world");
}
