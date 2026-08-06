//! Golden-vector conformance: every vector in the shared fixture must parse,
//! canonicalize, and select exactly as specified. The fixture is shared
//! verbatim across the three engines; changing it is a reviewed cross-repo
//! decision that bumps fixture_version everywhere atomically.

use serde_json::Value;
use zero_ref::{LineEndPolicy, ZeroRefV1, content_hash_hex};

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
    assert_eq!(f["fixture_version"], 2);
    assert_eq!(f["selection_policy"]["byte"], "strict");
    assert_eq!(f["selection_policy"]["line_start"], "strict");
    assert_eq!(f["selection_policy"]["line_end"], "clamp");
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
    assert_eq!(
        classes, ours,
        "error classes must match the fixture verbatim"
    );
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
fn canonical_default_clamps_line_end_at_eof() {
    let f = fixture();
    let v = f["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "lines_end_past_count")
        .unwrap();
    let bytes = hex_decode(
        f["blobs"][v["blob"].as_str().unwrap()]["bytes_hex"]
            .as_str()
            .unwrap(),
    );
    let r = ZeroRefV1::parse(v["input"].as_str().unwrap()).unwrap();
    let selected = r.verify_and_select(&bytes).unwrap();
    assert_eq!(
        selected,
        &hex_decode(v["selected_hex"].as_str().unwrap())[..]
    );
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
                    let bytes = hex_decode(blobs[blob_name]["bytes_hex"].as_str().unwrap());
                    let policy = match v.get("line_end_policy").and_then(|p| p.as_str()) {
                        None | Some("canonical") => LineEndPolicy::ClampEnd,
                        Some("strict") => LineEndPolicy::Strict,
                        Some(other) => {
                            panic!("vector '{name}' has unknown line_end_policy '{other}'")
                        }
                    };
                    match r.verify_and_select_with_policy(&bytes, policy) {
                        Ok(selected) => {
                            let expected = hex_decode(v["selected_hex"].as_str().unwrap());
                            assert_eq!(selected, &expected[..], "vector '{name}' selection");
                        }
                        Err(e) => {
                            let expected_class =
                                v["selection_error"].as_str().unwrap_or_else(|| {
                                    panic!("vector '{name}' selected unexpectedly failed: {e}")
                                });
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
                let e = parsed
                    .err()
                    .unwrap_or_else(|| panic!("vector '{name}' must fail"));
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

const ZERO_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn line_ref(scheme: &str, start: u64, end: u64) -> ZeroRefV1 {
    ZeroRefV1::parse(&format!("{scheme}://blob/{ZERO_HASH}#L{start}-{end}"))
        .expect("test line ref parses")
}

#[test]
fn clamp_end_is_monotonic_across_starts_and_past_eof() {
    let bytes = b"alpha\nbeta\ngamma";
    let cases: &[(u64, u64, &[u8])] = &[
        (1, 1, b"alpha\n"),
        (1, 2, b"alpha\nbeta\n"),
        (1, 3, b"alpha\nbeta\ngamma"),
        (1, 4, b"alpha\nbeta\ngamma"),
        (1, 100, b"alpha\nbeta\ngamma"),
        (2, 2, b"beta\n"),
        (2, 3, b"beta\ngamma"),
        (2, 4, b"beta\ngamma"),
        (2, 100, b"beta\ngamma"),
        (3, 3, b"gamma"),
        (3, 4, b"gamma"),
        (3, 100, b"gamma"),
    ];

    for &(start, end, expected) in cases {
        let selected = line_ref("fz", start, end)
            .select_with_policy(bytes, LineEndPolicy::ClampEnd)
            .unwrap();
        assert_eq!(selected, expected, "canonical #L{start}-{end} bytes");
    }
}

#[test]
fn adversarial_spelling_and_line_endings_have_stable_semantics() {
    for scheme in ["FZ", "Gz", "tZ"] {
        let input = format!("{scheme}://blob/{ZERO_HASH}");
        let error = ZeroRefV1::parse(&input).unwrap_err();
        assert_eq!(error.class.as_str(), "unsupported", "scheme {scheme}");
    }

    let lowercase_byte_fragment = format!("fz://blob/{ZERO_HASH}#b0-1");
    let error = ZeroRefV1::parse(&lowercase_byte_fragment).unwrap_err();
    assert_eq!(error.class.as_str(), "malformed", "lowercase #b spelling");

    let pure_cr = b"left\rright\r";
    assert_eq!(
        line_ref("gz", 1, 99)
            .select_with_policy(pure_cr, LineEndPolicy::ClampEnd)
            .unwrap(),
        b"left\rright\r"
    );
    let error = line_ref("gz", 2, 2)
        .select_with_policy(pure_cr, LineEndPolicy::ClampEnd)
        .unwrap_err();
    assert_eq!(error.class.as_str(), "range_out_of_bounds");

    let sole_lf = b"\n";
    assert_eq!(
        line_ref("tz", 1, 1)
            .select_with_policy(sole_lf, LineEndPolicy::ClampEnd)
            .unwrap(),
        b"\n"
    );
    assert_eq!(
        line_ref("tz", 1, 99)
            .select_with_policy(sole_lf, LineEndPolicy::ClampEnd)
            .unwrap(),
        b"\n"
    );
    let error = line_ref("tz", 2, 2)
        .select_with_policy(sole_lf, LineEndPolicy::ClampEnd)
        .unwrap_err();
    assert_eq!(error.class.as_str(), "range_out_of_bounds");
}
