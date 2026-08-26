//! Parse/Display identity. Seed contract: WithSource + committed regressions.

use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence};
use zero_ref::ZeroRef;

fn hex64() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[0-9a-f]{64}").unwrap()
}

fn scheme() -> impl Strategy<Value = &'static str> {
    prop_oneof![Just("fz"), Just("gz"), Just("tz")]
}

fn config() -> Config {
    Config {
        cases: if cfg!(miri) { 8 } else { 64 },
        // File persistence calls getcwd; Miri isolation rejects that.
        failure_persistence: if cfg!(miri) {
            None
        } else {
            Some(Box::new(FileFailurePersistence::WithSource(
                "proptest-regressions",
            )))
        },
        ..Config::default()
    }
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn parse_display_is_identity_for_canonical_refs(
        scheme in scheme(),
        hash in hex64(),
        start in 0u64..4096,
        span in 0u64..4096,
        line_start in 1u64..64,
        line_span in 0u64..64,
        kind in 0u8..3,
    ) {
        let input = match kind {
            0 => format!("{scheme}://blob/{hash}"),
            1 => format!("{scheme}://blob/{hash}#B{start}-{}", start + span),
            _ => format!(
                "{scheme}://blob/{hash}#L{line_start}-{}",
                line_start + line_span
            ),
        };
        let parsed = ZeroRef::parse(&input).expect("canonical grammar");
        let via_fromstr: ZeroRef = input.parse().expect("FromStr");
        prop_assert_eq!(&via_fromstr, &parsed);
        let rendered = parsed.to_string();
        prop_assert_eq!(&rendered, &input);
        let again = ZeroRef::parse(&rendered).expect("reparse");
        prop_assert_eq!(again, parsed);
    }

    #[test]
    fn legacy_plus_form_normalizes_to_hyphen_display(
        scheme in scheme(),
        hash in hex64(),
        start in 0u64..2048,
        len in 0u64..2048,
    ) {
        let input = format!("{scheme}://blob/{hash}#B{start}+{len}");
        let parsed = ZeroRef::parse(&input).expect("legacy alias");
        let rendered = parsed.to_string();
        let expected = format!("{scheme}://blob/{hash}#B{start}-{}", start + len);
        prop_assert_eq!(&rendered, &expected);
        prop_assert_eq!(ZeroRef::parse(&rendered).unwrap(), parsed);
    }
}
