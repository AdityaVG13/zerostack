//! FromStr, Display-form serde, negotiate, and error-class reachability.

use std::str::FromStr;

use zero_ref::{ZEROREF_MAJOR, ZEROREF_MINOR, ZeroRef, ZeroRefErrorClass, negotiate};

const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn canonical_blob() -> String {
    format!("fz://blob/{HASH}")
}

#[test]
fn fromstr_equals_parse() {
    let input = canonical_blob();
    let via_parse = ZeroRef::parse(&input).expect("parse");
    let via_fromstr = ZeroRef::from_str(&input).expect("FromStr");
    let via_parse_method: ZeroRef = input.parse().expect("str::parse");
    assert_eq!(via_fromstr, via_parse);
    assert_eq!(via_parse_method, via_parse);
}

#[test]
fn serde_roundtrips_display_form() {
    let input = format!("tz://blob/{HASH}#B0-4");
    let parsed = ZeroRef::parse(&input).expect("parse");
    let json = serde_json::to_string(&parsed).expect("serialize");
    assert_eq!(json, serde_json::to_string(&input).expect("string json"));
    let back: ZeroRef = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, parsed);
    assert_eq!(back.to_string(), input);
}

#[test]
fn serde_rejects_malformed_display_form() {
    // Wire contract: any string that is not a canonical ZeroRef display form
    // must be rejected. Typed oracle is ZeroRefErrorClass::Malformed via the
    // direct parser; serde layer must also reject without relying on message prose.
    for bad in [
        "not-a-ref",
        "fz://blob/ZZZ",
        "tz://blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa#BAD",
        "fz://blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "fz://blob/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    ] {
        let parse_err = ZeroRef::parse(bad).unwrap_err();
        assert_eq!(
            parse_err.class,
            ZeroRefErrorClass::Malformed,
            "expected Malformed for {bad:?}, got {:?}",
            parse_err.class
        );
        let json = serde_json::to_string(&bad).expect("json string");
        assert!(
            serde_json::from_str::<ZeroRef>(&json).is_err(),
            "serde must reject {bad:?}"
        );
    }
}

#[test]
fn negotiate_accepts_same_major_any_minor() {
    // Minor bumps are additive: every peer minor with the same major is accepted.
    // Exercise boundaries: zero, the local minor, an arbitrary higher value, and
    // the type maximum, without pinning an implementation sentinel.
    for minor in [0u64, 1, ZEROREF_MINOR, 99, u64::MAX] {
        let result = negotiate(ZEROREF_MAJOR, minor);
        assert!(
            result.is_ok(),
            "negotiate({}, {}) must accept same-major any-minor, got {result:?}",
            ZEROREF_MAJOR,
            minor
        );
    }
}
#[test]
fn negotiate_rejects_other_major() {
    let err = negotiate(ZEROREF_MAJOR + 1, 0).expect_err("major mismatch");
    assert_eq!(err.class, ZeroRefErrorClass::IncompatibleVersion);
    assert!(err.message.contains("incompatible"), "{}", err.message);
}

#[test]
fn parser_never_emits_reserved_resolution_classes() {
    let reserved = ZeroRefErrorClass::RESERVED_FOR_RESOLUTION;
    let samples = [
        "not-a-ref",
        "g:compact",
        "xx://blob/aa",
        "fz://seq/nope",
        &format!("fz://blob/{}", &HASH[..8]),
        &format!("fz://blob/{HASH}#B9-1"),
        &format!("fz://blob/{HASH}#L0-1"),
        &format!("fz://blob/{HASH}#Z1-2"),
    ];
    for sample in samples {
        let class = ZeroRef::parse(sample).expect_err(sample).class;
        assert!(
            !reserved.contains(&class),
            "parser emitted reserved {class:?} for {sample}"
        );
        assert!(
            ZeroRefErrorClass::PARSER_AND_SELECTOR.contains(&class)
                || class == ZeroRefErrorClass::Unsupported,
            "unexpected parser class {class:?} for {sample}"
        );
    }
}

#[test]
fn selector_emits_range_and_utf8_and_digest() {
    let bytes = b"hello";
    let hash = zero_ref::content_hash_hex(bytes);
    let oob = ZeroRef::parse(&format!("fz://blob/{hash}#B0-99")).unwrap();
    assert_eq!(
        oob.verify_and_select(bytes).unwrap_err().class,
        ZeroRefErrorClass::RangeOutOfBounds
    );
    let lines = ZeroRef::parse(&format!("fz://blob/{hash}#L1-1")).unwrap();
    assert_eq!(
        lines.verify_and_select(b"\xff").unwrap_err().class,
        ZeroRefErrorClass::DigestMismatch
    );
    let utf = ZeroRef::parse(&format!(
        "fz://blob/{}#L1-1",
        zero_ref::content_hash_hex(b"\xff")
    ))
    .unwrap();
    assert_eq!(
        utf.verify_and_select(b"\xff").unwrap_err().class,
        ZeroRefErrorClass::NotUtf8
    );
}
