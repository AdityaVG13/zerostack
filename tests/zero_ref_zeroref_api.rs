//! FromStr, Display-form serde, and error-class reachability.

use std::str::FromStr;

use zero_ref::{ZeroRef, ZeroRefErrorClass};

const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn canonical_blob() -> String {
    format!("z://blob/{HASH}")
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
    let input = format!("z://blob/{HASH}#B0-4");
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
        "z://blob/ZZZ",
        "z://blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa#BAD",
        "z://blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "z://blob/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
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
fn plus_length_byte_fragment_is_rejected() {
    let input = format!("z://blob/{HASH}#B6+4");
    let err = ZeroRef::parse(&input).expect_err("plus-length alias must fail closed");
    assert_eq!(err.class, ZeroRefErrorClass::Malformed);
}

#[test]
fn retired_product_schemes_fail_closed() {
    for retired in [
        format!("fz://blob/{HASH}"),
        format!("gz://blob/{HASH}"),
        format!("tz://blob/{HASH}"),
        format!("tz://blob/{HASH}#B0-4"),
    ] {
        let err = ZeroRef::parse(&retired).expect_err("retired scheme must fail closed");
        assert_eq!(
            err.class,
            ZeroRefErrorClass::Unsupported,
            "retired {retired} must be rejected as a nonportable scheme"
        );
    }
    let live = ZeroRef::parse(&format!("z://blob/{HASH}")).expect("z://blob is the live family");
    assert_eq!(live.to_string(), format!("z://blob/{HASH}"));
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
    let oob = ZeroRef::parse(&format!("z://blob/{hash}#B0-99")).unwrap();
    assert_eq!(
        oob.verify_and_select(bytes).unwrap_err().class,
        ZeroRefErrorClass::RangeOutOfBounds
    );
    let lines = ZeroRef::parse(&format!("z://blob/{hash}#L1-1")).unwrap();
    assert_eq!(
        lines.verify_and_select(b"\xff").unwrap_err().class,
        ZeroRefErrorClass::DigestMismatch
    );
    let utf = ZeroRef::parse(&format!(
        "z://blob/{}#L1-1",
        zero_ref::content_hash_hex(b"\xff")
    ))
    .unwrap();
    assert_eq!(
        utf.verify_and_select(b"\xff").unwrap_err().class,
        ZeroRefErrorClass::NotUtf8
    );
}
