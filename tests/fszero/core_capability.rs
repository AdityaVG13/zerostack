//! ZeroRef capability grammar on `fszero-core`. Session descriptor and shared-CAS
//! state live on the engine. This target drives the shipped parser and selection algebra against
//! the advertised core constants (hash length, schemes, portable kinds, fragment rules).

use fszero_core::zeroref::{
    EMITTED_SCHEME, HASH_HEX_LEN, PORTABLE_KINDS, ZeroFragment, ZeroRef, ZeroRefErrorClass,
    ZeroScheme, select_fragment,
};

fn blob(hash: &str) -> String {
    format!("z://blob/{hash}")
}

#[test]
fn parser_accepts_advertised_read_schemes() {
    let hash = "a".repeat(HASH_HEX_LEN);
    for scheme in ZeroScheme::ALL {
        let r = format!("{}://blob/{hash}", scheme.as_str());
        assert!(
            ZeroRef::parse(&r).is_ok(),
            "advertised scheme rejected: {r}"
        );
    }
    let alien = format!("qq://blob/{hash}");
    assert_eq!(
        ZeroRef::parse(&alien).unwrap_err().class,
        ZeroRefErrorClass::Unsupported
    );
}

#[test]
fn only_advertised_portable_kinds_parse() {
    let hash = "a".repeat(HASH_HEX_LEN);
    assert!(!PORTABLE_KINDS.contains(&"node"));
    assert_eq!(
        ZeroRef::parse(&format!("z://node/{hash}"))
            .unwrap_err()
            .class,
        ZeroRefErrorClass::Unsupported
    );
}

#[test]
fn hash_is_lowercase_exact_length() {
    assert_eq!(
        ZeroRef::parse(&format!("z://blob/{}", "A".repeat(HASH_HEX_LEN)))
            .unwrap_err()
            .class,
        ZeroRefErrorClass::Malformed
    );
    assert_eq!(
        ZeroRef::parse(&format!("z://blob/{}", "a".repeat(HASH_HEX_LEN - 1)))
            .unwrap_err()
            .class,
        ZeroRefErrorClass::Malformed
    );
}

#[test]
fn byte_and_line_fragments_match_core_algebra() {
    let hash = "a".repeat(HASH_HEX_LEN);
    let empty = ZeroRef::parse(&format!("{}#B0-0", blob(&hash))).unwrap();
    assert_eq!(empty.fragment, ZeroFragment::Bytes { start: 0, end: 0 });
    assert_eq!(
        ZeroRef::parse(&format!("{}#B5-2", blob(&hash)))
            .unwrap_err()
            .class,
        ZeroRefErrorClass::Malformed
    );
    assert_eq!(
        ZeroRef::parse(&format!("{}#L0-1", blob(&hash)))
            .unwrap_err()
            .class,
        ZeroRefErrorClass::Malformed
    );
    let lines = ZeroRef::parse(&format!("{}#L1-1", blob(&hash))).unwrap();
    assert_eq!(
        select_fragment(b"a\nb\n", &lines.fragment, "test").unwrap(),
        b"a\n"
    );
}

#[test]
fn minted_scheme_is_z() {
    assert_eq!(EMITTED_SCHEME, ZeroScheme::Z);
}
