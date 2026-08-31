//! Exact identity, TokenPage limits, and expansion through the public API.
//! Dual-store fragment expansion is not a TokenPage or ModelCapsule round trip.

use tokenzero_core::model_artifacts::{
    ExactTokenMap, ExactTokenizerAdapter, ExactTokenizerIdentity, MAX_CAPSULE_EVIDENCE_REFS,
    MAX_CAPSULE_RENDER_BYTES, MAX_CAPSULE_TOKEN_PAGES, MAX_TOKEN_PAGE_BYTES, MAX_TOKEN_PAGE_TOKENS,
    ModelArtifactError, ModelCapsule, TokenPage,
};
use tokenzero_core::{
    TokenizerIdPreflightError, UNLABELED_ESTIMATE_TOKENIZER_PREFIX, preflight_tokenizer_id,
    sha256_hex,
};
use zero_gauge::ProviderLock;

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
    let manifest = b"gauntlet-fake-tokenizer-revision";
    let digest = sha256_hex(std::str::from_utf8(manifest).expect("ascii manifest"));
    let identity = ExactTokenizerIdentity::new(
        ProviderLock {
            provider: "gauntlet".to_string(),
            model: "fake-tokenizer".to_string(),
            tokenizer_revision_digest: digest,
        },
        manifest,
    )
    .expect("identity");
    ByteAdapter { identity }
}

fn blob_anchor(map: &ExactTokenMap) -> String {
    format!("z://blob/{}", map.source_digest().to_hex())
}

#[test]
fn tokenizer_id_preflight_refuses_unlabeled_estimate_and_q99_as_exact() {
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
}

#[test]
fn exact_tokenizer_identity_rejects_revision_digest_mismatch() {
    let expected = sha256_hex("gauntlet-expected-manifest");
    let err = ExactTokenizerIdentity::new(
        ProviderLock {
            provider: "gauntlet".to_string(),
            model: "fake-tokenizer".to_string(),
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
    assert_eq!(MAX_TOKEN_PAGE_TOKENS, 4_096);
    assert_eq!(MAX_TOKEN_PAGE_BYTES, 1_048_576);
    assert_eq!(MAX_CAPSULE_EVIDENCE_REFS, 4_096);
    assert_eq!(MAX_CAPSULE_TOKEN_PAGES, 4_096);
    assert_eq!(MAX_CAPSULE_RENDER_BYTES, 16 * 1_048_576);
}

#[test]
fn empty_token_page_rejects_empty_range() {
    let adapter = adapter();
    let map = ExactTokenMap::tokenize(&adapter, b"abc").expect("map");
    let anchor = blob_anchor(&map);
    assert!(matches!(
        TokenPage::new(&map, &anchor, 0..0),
        Err(ModelArtifactError::EmptyTokenPage)
    ));
}

#[test]
fn token_page_expands_exact_source_bytes() {
    let adapter = adapter();
    let source = b"abc";
    let map = ExactTokenMap::tokenize(&adapter, source).expect("map");
    let anchor = blob_anchor(&map);
    let page = TokenPage::new(&map, &anchor, 0..3).expect("page");
    assert_eq!(page.expand(), source);
}

#[test]
fn token_page_subrange_expand_equals_source_slice() {
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
