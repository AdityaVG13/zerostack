//! Golden-vector conformance: every vector in the shared fixture must parse,
//! canonicalize, and select exactly as specified. The fixture is shared
//! verbatim across the three engines; changing it is a reviewed cross-repo
//! decision that bumps fixture_version everywhere atomically.

use serde_json::Value;
use zero_ref::{ZeroRefV1, content_hash_hex};

fn fixture() -> Value {
    let raw = include_str!("../fixtures/zeroref_v1_vectors.json");
    serde_json::from_str(raw).expect("fixture parses")
}

fn hex_decode(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
        .collect()
}

#[test]
fn fixture_metadata_matches_crate() {
    let f = fixture();
    assert_eq!(f["zeroref_version"], zero_ref::ZEROREF_VERSION);
    assert_eq!(f["fixture_version"], 1);
    let classes: Vec<&str> = f["error_classes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let ours: Vec<&str> = zero_ref::ZeroRefErrorClass::ALL
        .iter()
        .map(|c| c.as_str())
        .collect();
    assert_eq!(classes, ours, "error classes must match the fixture verbatim");
}

#[test]
fn fixture_blobs_hash_to_their_identities() {
    let f = fixture();
    for (name, blob) in f["blobs"].as_object().unwrap() {
        let bytes = hex_decode(blob["bytes_hex"].as_str().unwrap());
        assert_eq!(
            content_hash_hex(&bytes),
            blob["sha256"].as_str().unwrap(),
            "blob '{name}' identity"
        );
    }
}

#[test]
fn all_vectors_conform() {
    let f = fixture();
    let blobs = f["blobs"].as_object().unwrap();
    for v in f["vectors"].as_array().unwrap() {
        let name = v["name"].as_str().unwrap();
        let input = v["input"].as_str().unwrap();
        let parsed = ZeroRefV1::parse(input);
        match v["parse"].as_str().unwrap() {
            "ok" => {
                let r = parsed.unwrap_or_else(|e| panic!("vector '{name}' must parse: {e}"));
                assert_eq!(
                    r.to_string(),
                    v["canonical"].as_str().unwrap(),
                    "vector '{name}' canonical form"
                );
                if let Some(blob_name) = v.get("blob").and_then(|b| b.as_str()) {
                    let bytes =
                        hex_decode(blobs[blob_name]["bytes_hex"].as_str().unwrap());
                    match r.verify_and_select(&bytes) {
                        Ok(selected) => {
                            let expected =
                                hex_decode(v["selected_hex"].as_str().unwrap());
                            assert_eq!(selected, &expected[..], "vector '{name}' selection");
                        }
                        Err(e) => {
                            let expected_class = v["selection_error"]
                                .as_str()
                                .unwrap_or_else(|| panic!("vector '{name}' selected unexpectedly failed: {e}"));
                            assert_eq!(
                                e.class.as_str(),
                                expected_class,
                                "vector '{name}' selection error class"
                            );
                        }
                    }
                }
            }
            "error" => {
                let e = parsed.err().unwrap_or_else(|| panic!("vector '{name}' must fail"));
                assert_eq!(
                    e.class.as_str(),
                    v["error_class"].as_str().unwrap(),
                    "vector '{name}' error class"
                );
            }
            other => panic!("vector '{name}' has unknown parse kind '{other}'"),
        }
    }
}
