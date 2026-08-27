//! ZeroRef v1 #B/#L fragment conformance through the real expansion surface
//! (ExpandResolver, the engine under CLI/MCP/CodeMode expand), plus local
//! table cases the shared golden fixture does not carry (Unicode content,
//! byte-boundary spans, usize/u64 overflow spellings).
//!
//! Golden fixture: docs/contracts/zeroref-v1-fixtures.json, fixture_version 2
//! (canonical line_end=clamp; byte bounds and line starts remain strict).

use std::collections::BTreeMap;
use std::path::PathBuf;

use graphzero_store::store::blob_store::BlobStore;
use graphzero_store::store::refs::GzRef;
use graphzero_store::store::zeroref::{ZeroRef, select_fragment};
use graphzero_store::{ContentHash, ExpandResolver};
use serde::Deserialize;
use tempfile::tempdir;

#[derive(Deserialize)]
struct Fixture {
    blobs: BTreeMap<String, FixtureBlob>,
    vectors: Vec<Vector>,
}

#[derive(Deserialize)]
struct FixtureBlob {
    bytes_hex: String,
}

#[derive(Deserialize)]
struct Vector {
    name: String,
    input: String,
    parse: String,
    #[serde(default)]
    blob: Option<String>,
    #[serde(default)]
    selected_hex: Option<String>,
    #[serde(default)]
    selection_error: Option<String>,
    /// Absent or "canonical" → ExpandResolver path (ClampEnd). "strict" is
    /// only exercised by the standalone ZeroRef selector with an explicit
    /// policy and is not part of the engine expand surface.
    #[serde(default)]
    line_end_policy: Option<String>,
}

fn load_fixture() -> Fixture {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/contracts/zeroref-v1-fixtures.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("read fixture"))
        .expect("parse fixture")
}

fn decode_hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
        .collect()
}

/// Every gz-scheme golden vector with a blob must behave identically through
/// the ExpandResolver chain: same bytes on success, same error class token in
/// the resolver's reason on failure. This pins CLI/MCP behavior to the
/// fixture, not just the standalone parser.
#[test]
fn resolver_expansion_matches_golden_vectors() {
    let fixture = load_fixture();
    let dir = tempdir().unwrap();
    let store = BlobStore::open(dir.path()).unwrap();
    for blob in fixture.blobs.values() {
        store.put(&decode_hex(&blob.bytes_hex)).unwrap();
    }
    let resolver = ExpandResolver::new(dir.path(), None).unwrap();

    let mut exercised = 0;
    for v in &fixture.vectors {
        if v.parse != "ok" || !v.input.starts_with("gz://") {
            continue;
        }
        // ExpandResolver always uses the canonical ClampEnd policy; skip
        // explicit strict-override vectors (hub zero-ref golden_vectors).
        match v.line_end_policy.as_deref() {
            None | Some("canonical") => {}
            Some("strict") => continue,
            Some(other) => panic!("vector '{}' has unknown line_end_policy '{other}'", v.name),
        }
        let Some(blob_key) = v.blob.as_deref() else {
            continue;
        };
        // Digest-mismatch vectors pair a ref with a different blob's bytes;
        // the resolver chain always serves the ref's true object, so only
        // selection behavior is comparable here.
        let ref_hash_matches_blob = {
            let bytes = decode_hex(&fixture.blobs[blob_key].bytes_hex);
            v.input.contains(&ContentHash::of(&bytes).to_hex())
        };
        if !ref_hash_matches_blob {
            continue;
        }
        exercised += 1;
        let gz = GzRef::parse(&v.input).expect("engine grammar accepts canonical v1 blob refs");
        match (&v.selected_hex, &v.selection_error) {
            (Some(expected_hex), None) => {
                let hit = resolver
                    .resolve(&gz, &v.input)
                    .unwrap_or_else(|e| panic!("vector '{}' failed via resolver: {e}", v.name));
                assert_eq!(
                    hit.bytes,
                    decode_hex(expected_hex),
                    "vector '{}' resolver bytes mismatch",
                    v.name
                );
            }
            (None, Some(class)) => {
                let err = resolver
                    .resolve(&gz, &v.input)
                    .expect_err(&format!("vector '{}' must fail via resolver", v.name));
                assert!(
                    err.reason.starts_with(class.as_str()),
                    "vector '{}': reason '{}' lacks class '{class}'",
                    v.name,
                    err.reason
                );
            }
            _ => {}
        }
    }
    assert!(
        exercised >= 20,
        "expected to exercise most golden vectors, got {exercised}"
    );
}

struct LocalCase {
    name: &'static str,
    content: &'static [u8],
    fragment: &'static str,
    expect: Result<&'static [u8], &'static str>,
}

/// Cases beyond the shared fixture: Unicode multi-byte content, first/last
/// byte boundaries, and overflow spellings. Each runs through BOTH the
/// standalone v1 selector and the resolver so the surfaces cannot drift.
///
/// Line-end past EOF uses the canonical ClampEnd policy (fixture_version 2);
/// byte bounds and line starts remain strict. No process-global policy state.
#[test]
fn local_case_matrix_covers_unicode_boundaries_and_overflow() {
    const UNICODE: &[u8] = "héllo\nwörld\n".as_bytes(); // 13 bytes: h,é(2),l,l,o,\n,w,ö(2),r,l,d,\n
    const SINGLE: &[u8] = b"x";
    let cases = [
        LocalCase {
            name: "unicode_whole_first_line",
            content: UNICODE,
            fragment: "#L1-1",
            expect: Ok("héllo\n".as_bytes()),
        },
        LocalCase {
            name: "unicode_last_line",
            content: UNICODE,
            fragment: "#L2-2",
            expect: Ok("wörld\n".as_bytes()),
        },
        LocalCase {
            name: "unicode_mid_codepoint_byte_slice_is_byte_exact",
            content: UNICODE,
            fragment: "#B1-2",
            expect: Ok(&[0xc3]),
        },
        LocalCase {
            name: "first_byte",
            content: SINGLE,
            fragment: "#B0-1",
            expect: Ok(b"x"),
        },
        LocalCase {
            name: "empty_slice_at_last_byte",
            content: SINGLE,
            fragment: "#B1-1",
            expect: Ok(b""),
        },
        LocalCase {
            name: "byte_past_end_never_clamps",
            content: SINGLE,
            fragment: "#B0-2",
            expect: Err("range_out_of_bounds"),
        },
        LocalCase {
            // UNICODE has 2 lines; #L1-3 clamps end to 2 under canonical policy.
            name: "line_end_past_eof_clamps_to_last",
            content: UNICODE,
            fragment: "#L1-3",
            expect: Ok(UNICODE),
        },
        LocalCase {
            name: "line_start_past_count_never_clamps",
            content: UNICODE,
            fragment: "#L3-3",
            expect: Err("range_out_of_bounds"),
        },
        LocalCase {
            name: "u64_max_span_out_of_bounds",
            content: SINGLE,
            fragment: "#B0-18446744073709551615",
            expect: Err("range_out_of_bounds"),
        },
        LocalCase {
            name: "u64_overflow_digits_malformed",
            content: SINGLE,
            fragment: "#B0-18446744073709551616",
            expect: Err("malformed"),
        },
        LocalCase {
            name: "legacy_plus_len_overflow_malformed",
            content: SINGLE,
            fragment: "#B18446744073709551615+1",
            expect: Err("malformed"),
        },
        LocalCase {
            name: "legacy_plus_len_normalizes",
            content: UNICODE,
            fragment: "#B0+7",
            expect: Ok("héllo\n".as_bytes()),
        },
    ];

    let dir = tempdir().unwrap();
    let store = BlobStore::open(dir.path()).unwrap();
    let resolver = ExpandResolver::new(dir.path(), None).unwrap();

    for case in &cases {
        let hash = store.put(case.content).unwrap().to_hex();
        let reference = format!("gz://blob/{hash}{}", case.fragment);

        // Surface 1: standalone ZeroRef v1 parse + verified selection.
        let v1_outcome = ZeroRef::parse(&reference)
            .and_then(|r| r.verify_and_select(case.content).map(<[u8]>::to_vec));
        // Surface 2: engine resolver (CLI/MCP path).
        let resolver_outcome = GzRef::parse(&reference)
            .map_err(|e| e.to_string())
            .and_then(|gz| {
                resolver
                    .resolve(&gz, &reference)
                    .map(|hit| hit.bytes)
                    .map_err(|e| e.reason.clone())
            });

        match case.expect {
            Ok(expected) => {
                let v1 = v1_outcome.unwrap_or_else(|e| panic!("case '{}' v1: {e}", case.name));
                let res = resolver_outcome
                    .unwrap_or_else(|e| panic!("case '{}' resolver: {e}", case.name));
                assert_eq!(v1, expected, "case '{}' v1 bytes", case.name);
                assert_eq!(res, expected, "case '{}' resolver bytes", case.name);
            }
            Err(class) => {
                let v1_err = v1_outcome.expect_err(&format!("case '{}' v1 must fail", case.name));
                assert_eq!(
                    v1_err.class.as_str(),
                    class,
                    "case '{}' v1 class",
                    case.name
                );
                let res_err = resolver_outcome
                    .expect_err(&format!("case '{}' resolver must fail", case.name));
                // The engine grammar rejects overflow spellings at parse time
                // with its own wording; any other failure must carry the
                // stable class token.
                let engine_parse_rejection =
                    res_err.contains("invalid number") || res_err.contains("overflows");
                assert!(
                    res_err.contains(class) || (class == "malformed" && engine_parse_rejection),
                    "case '{}': resolver error '{res_err}' lacks class '{class}'",
                    case.name
                );
            }
        }
    }
}

/// A valid fragment window must never hide corruption elsewhere in the
/// object: whole-object verification precedes selection on every surface.
#[test]
fn valid_window_cannot_hide_corruption_outside_it() {
    let dir = tempdir().unwrap();
    let store = BlobStore::open(dir.path()).unwrap();
    let hash = store.put(b"good prefix | poisoned tail").unwrap().to_hex();
    // Corrupt only the tail; the requested window covers the intact prefix.
    std::fs::write(
        dir.path().join("blobs").join(&hash),
        b"good prefix | CORRUPTED!!!!",
    )
    .unwrap();
    let resolver = ExpandResolver::new(dir.path(), None).unwrap();
    let reference = format!("gz://blob/{hash}#B0-11");
    let gz = GzRef::parse(&reference).unwrap();
    let err = resolver
        .resolve(&gz, &reference)
        .expect_err("corrupt object must fail even for an intact window");
    assert!(
        err.reason.starts_with("digest_mismatch"),
        "reason: {}",
        err.reason
    );
}

/// Direct selector checks for degenerate inputs that no ref spelling reaches.
#[test]
fn selector_rejects_reversed_ranges_constructed_directly() {
    use graphzero_store::store::zeroref::ZeroFragment;
    let err = select_fragment(b"abc", &ZeroFragment::Bytes { start: 2, end: 1 }, "direct")
        .expect_err("reversed byte span");
    assert_eq!(err.class.as_str(), "malformed");
    let err = select_fragment(b"abc", &ZeroFragment::Lines { start: 0, end: 1 }, "direct")
        .expect_err("zero line start");
    assert_eq!(err.class.as_str(), "malformed");
}
