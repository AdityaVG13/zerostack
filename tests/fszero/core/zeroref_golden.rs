//! ZeroRef v1 golden fixture executed against parse + engine expand.
//!
//! The fixture is the tie-breaker (annex). Every vector is loaded and run:
//! parse error class, canonical emission, digest-verified selection, and
//! `RecoveryStore::expand_zeroref` for matching-hash expand cases.

#[path = "../common/mod.rs"]
mod common;

use common::TestRoot;
use fs_zero::FSZeroSession;
use fs_zero::core::zeroref::{LineEndPolicy, ZeroRef, ZeroRefError, ZeroRefErrorClass};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const FIXTURE: &str = include_str!("../fixtures/zeroref_v1_vectors.json");

#[derive(Deserialize)]
struct Fixture {
    schema: String,
    zeroref_version: String,
    fixture_version: u32,
    error_classes: Vec<String>,
    blobs: BTreeMap<String, FixtureBlob>,
    vectors: Vec<Vector>,
}

#[derive(Deserialize)]
struct FixtureBlob {
    bytes_hex: String,
    sha256: String,
}

#[derive(Deserialize)]
struct Vector {
    name: String,
    input: String,
    parse: String,
    #[serde(default)]
    canonical: Option<String>,
    #[serde(default)]
    blob: Option<String>,
    #[serde(default)]
    selected_hex: Option<String>,
    #[serde(default)]
    error_class: Option<String>,
    #[serde(default)]
    selection_error: Option<String>,
}

fn decode_hex(s: &str) -> Vec<u8> {
    assert!(s.len() % 2 == 0, "odd hex length");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
            use std::fmt::Write;
            let _ = write!(out, "{b:02x}");
            out
        })
}

fn sha256_hex(bytes: &[u8]) -> String {
    encode_hex(&Sha256::digest(bytes))
}

fn class_of(err: &ZeroRefError) -> &str {
    err.class.as_str()
}

#[test]
fn zeroref_v1_golden_vectors_parse_expand_and_error_class() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("parse zeroref v1 fixture");
    assert_eq!(fixture.schema, "zeroref-golden-vectors");
    assert_eq!(fixture.zeroref_version, "v1");
    assert_eq!(fixture.fixture_version, 1);
    assert_eq!(
        fixture.error_classes,
        ZeroRefErrorClass::ALL
            .iter()
            .map(|c| c.as_str().to_string())
            .collect::<Vec<_>>()
    );
    assert!(!fixture.vectors.is_empty());

    let decoded: BTreeMap<String, Vec<u8>> = fixture
        .blobs
        .iter()
        .map(|(name, blob)| {
            let bytes = decode_hex(&blob.bytes_hex);
            assert_eq!(sha256_hex(&bytes), blob.sha256, "blob '{name}' digest");
            (name.clone(), bytes)
        })
        .collect();

    let root = TestRoot::new("zeroref_golden");
    std::fs::create_dir_all(root.join(".zerostack/blobs")).unwrap();
    let mut sess = FSZeroSession::with_repo_store(root.path());
    for bytes in decoded.values() {
        let minted = sess.recovery.put_content_ref(bytes);
        assert!(
            minted.starts_with("fz://blob/"),
            "mint must be a portable blob ref: {minted}"
        );
    }

    let mut executed = 0usize;
    for v in &fixture.vectors {
        executed += 1;
        match v.parse.as_str() {
            "error" => {
                let class = v.error_class.as_deref().unwrap_or_else(|| {
                    panic!("vector '{}' parse=error missing error_class", v.name)
                });
                let parse_err = ZeroRef::parse(&v.input)
                    .expect_err(&format!("vector '{}' must fail parse", v.name));
                assert_eq!(
                    class_of(&parse_err),
                    class,
                    "vector '{}' parse class",
                    v.name
                );

                let expand_err = sess
                    .recovery
                    .expand_zeroref(&v.input)
                    .expect_err(&format!("vector '{}' must fail expand", v.name));
                assert_eq!(
                    class_of(&expand_err),
                    class,
                    "vector '{}' expand class",
                    v.name
                );
            }
            "ok" => {
                let parsed = ZeroRef::parse(&v.input)
                    .unwrap_or_else(|e| panic!("vector '{}' parse failed: {e}", v.name));
                let canonical = v
                    .canonical
                    .as_deref()
                    .unwrap_or_else(|| panic!("vector '{}' parse=ok missing canonical", v.name));
                assert_eq!(
                    parsed.to_string(),
                    canonical,
                    "vector '{}' canonical",
                    v.name
                );

                let blob_name = v
                    .blob
                    .as_deref()
                    .unwrap_or_else(|| panic!("vector '{}' parse=ok missing blob", v.name));
                let bytes = decoded
                    .get(blob_name)
                    .unwrap_or_else(|| panic!("vector '{}' unknown blob '{blob_name}'", v.name));
                let hash_matches = fixture.blobs[blob_name].sha256 == parsed.hash;

                match (v.selected_hex.as_deref(), v.selection_error.as_deref()) {
                    (Some(expected_hex), None) => {
                        let selected = parsed
                            .verify_and_select_with_policy(bytes, LineEndPolicy::Strict)
                            .unwrap_or_else(|e| panic!("vector '{}' select failed: {e}", v.name));
                        assert_eq!(
                            encode_hex(selected),
                            expected_hex,
                            "vector '{}' selected bytes",
                            v.name
                        );
                        assert!(
                            hash_matches,
                            "vector '{}' success case must pair matching blob digest",
                            v.name
                        );
                        let expanded = sess
                            .recovery
                            .expand_zeroref(&v.input)
                            .unwrap_or_else(|e| panic!("vector '{}' expand failed: {e}", v.name));
                        assert_eq!(
                            encode_hex(&expanded),
                            expected_hex,
                            "vector '{}' expand bytes",
                            v.name
                        );
                    }
                    (None, Some(class)) => {
                        let select_err = parsed
                            .verify_and_select_with_policy(bytes, LineEndPolicy::Strict)
                            .expect_err(&format!("vector '{}' select must fail", v.name));
                        assert_eq!(
                            class_of(&select_err),
                            class,
                            "vector '{}' select class",
                            v.name
                        );
                        if hash_matches {
                            let expand_err = sess
                                .recovery
                                .expand_zeroref(&v.input)
                                .expect_err(&format!("vector '{}' expand must fail", v.name));
                            assert_eq!(
                                class_of(&expand_err),
                                class,
                                "vector '{}' expand class",
                                v.name
                            );
                        } else {
                            assert_eq!(
                                class, "digest_mismatch",
                                "vector '{}' mismatched blob is only valid for digest_mismatch",
                                v.name
                            );
                        }
                    }
                    other => panic!(
                        "vector '{}' needs selected_hex xor selection_error, got {other:?}",
                        v.name
                    ),
                }
            }
            other => panic!("vector '{}' unknown parse '{other}'", v.name),
        }
    }
    assert_eq!(executed, fixture.vectors.len());
    assert_eq!(executed, 56, "fixture_version 1 vector count");
}
