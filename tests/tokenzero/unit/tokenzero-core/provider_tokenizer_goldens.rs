//! SPEC-TZ-GOLD-001 / F-TZ-001: provider tokenizer golden honesty contract.
//!
//! Restored as a live driver in Phase 1. `d8c0844` deleted the previous
//! `tests/engine/provider_tokenizer_goldens.rs` target but left the fixture.
//! Unverified entries carry no numeric count. Approximate counts are never
//! presented as exact. Subject ≠ ProviderTokenizer oracle.

use serde::Deserialize;
use tokenzero_core::{TokenizerFamily, count_tokens_for_model, tokenizer_metadata};
use tokenzero_test_support::{GauntletIdentityPair, GauntletOracle};

fn stamp_gauntlet_subject_ne_oracle() {
    GauntletIdentityPair::new(GauntletOracle::ProviderTokenizer).assert_distinct();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CountClass {
    Exact,
    Approximate,
}

#[derive(Debug, Deserialize)]
struct GoldenEntry {
    id: String,
    provider: String,
    tokenizer_identity: String,
    tokenizer_revision: String,
    model_id: String,
    prompt_text: String,
    expected_count: Option<u64>,
    count_class: CountClass,
    unverified: bool,
    source: String,
}

#[derive(Debug, Deserialize)]
struct GoldenFixture {
    schema: String,
    entries: Vec<GoldenEntry>,
}

fn fixture() -> GoldenFixture {
    stamp_gauntlet_subject_ne_oracle();
    serde_json::from_str(include_str!(
        "../../engine/fixtures/provider-tokenizer-goldens.json"
    ))
    .expect("provider-tokenizer-goldens.json must parse")
}

fn expected_family(identity: &str) -> Option<TokenizerFamily> {
    match identity {
        "cl100k_base" => Some(TokenizerFamily::Cl100k),
        "o200k_base" => Some(TokenizerFamily::O200k),
        "llama_sentencepiece_v3" => Some(TokenizerFamily::SentencePiece),
        _ => None,
    }
}

impl GoldenEntry {
    fn expected_family(&self) -> Option<TokenizerFamily> {
        expected_family(&self.tokenizer_identity)
    }
}

#[test]
fn gauntlet_subject_is_not_provider_tokenizer_oracle() {
    stamp_gauntlet_subject_ne_oracle();
}

#[test]
fn fixture_schema_is_tokenizer_goldens() {
    assert_eq!(fixture().schema, "tokenzero.tokenizer-goldens.v1");
    assert!(!fixture().entries.is_empty());
}

#[test]
fn tokenizer_identity_mismatch_does_not_resolve_to_a_family() {
    stamp_gauntlet_subject_ne_oracle();
    assert_ne!(
        expected_family("cl100k_base"),
        expected_family("o200k_base")
    );
    assert_ne!(
        expected_family("cl100k_base"),
        expected_family("llama_sentencepiece_v3")
    );
    for identity in [
        "EngineIdentity::TokenZero",
        "RegistryEngine::TokenZero",
        "unknown_tokenizer_family",
        "",
    ] {
        assert_eq!(expected_family(identity), None, "{identity}");
    }
}

#[test]
fn every_entry_has_a_source_and_unverified_entries_carry_no_count() {
    for entry in &fixture().entries {
        assert!(!entry.source.is_empty(), "{}", entry.id);
        assert!(!entry.provider.is_empty(), "{}", entry.id);
        assert!(!entry.tokenizer_identity.is_empty(), "{}", entry.id);
        assert!(!entry.tokenizer_revision.is_empty(), "{}", entry.id);
        assert_eq!(
            entry.unverified,
            entry.expected_count.is_none(),
            "{}",
            entry.id
        );
        if entry.unverified {
            assert_eq!(entry.count_class, CountClass::Exact, "{}", entry.id);
        }
    }
}

#[test]
fn verified_entries_match_count_tokens_for_model() {
    for entry in &fixture().entries {
        if entry.unverified {
            continue;
        }
        let got = count_tokens_for_model(&entry.prompt_text, Some(&entry.model_id));
        let expected = entry
            .expected_count
            .expect("verified entry must carry a count");
        assert_eq!(
            u64::try_from(got).expect("token counts fit u64"),
            expected,
            "{}",
            entry.id
        );
    }
}

#[test]
fn verified_exact_entries_are_provable_without_a_vocabulary() {
    for entry in &fixture().entries {
        if entry.unverified || entry.count_class != CountClass::Exact {
            continue;
        }
        let expected = entry
            .expected_count
            .expect("verified entry must carry a count");
        let trivial = (entry.prompt_text.is_empty() && expected == 0)
            || (entry.prompt_text.chars().count() == 1 && expected == 1);
        assert!(trivial, "{}", entry.id);
    }
}

#[test]
fn approximate_entries_are_recomputed_from_the_disclosed_heuristic() {
    for entry in &fixture().entries {
        if entry.unverified || entry.count_class != CountClass::Approximate {
            continue;
        }
        let metadata = tokenizer_metadata(&entry.model_id)
            .unwrap_or_else(|| panic!("{}: model {} must resolve", entry.id, entry.model_id));
        let chars = entry.prompt_text.chars().count() as u64;
        let expected = chars
            .saturating_mul(1_000)
            .div_ceil(metadata.chars_per_token_milli as u64);
        assert_eq!(entry.expected_count, Some(expected), "{}", entry.id);
    }
}

#[test]
fn approximate_counts_are_never_presented_as_exact() {
    for entry in &fixture().entries {
        let Some(metadata) = tokenizer_metadata(&entry.model_id) else {
            panic!("{}: model {} must resolve", entry.id, entry.model_id);
        };
        assert!(
            metadata.approximate,
            "{}: family {} must report approximate=true while no vocabulary is linked",
            entry.id,
            metadata.family.name()
        );
        assert_eq!(
            entry.expected_family(),
            Some(metadata.family),
            "{}",
            entry.id
        );
    }
}
