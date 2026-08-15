//! Parse/display identity plus the swlm/i4t3 differential:
//! `ZeroRefV1::parse` and `ZeroResultV1::reference` must agree on Ok/Err.
//!
//! Spark / CI: `cargo test -p zero-ref --test property_identity`
//! cargo-fuzz (if the pruned `fuzz/` crate is restored):
//! `cargo fuzz run zeroref_envelope_differential`
use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use zero_abi::ZeroResultV1;
use zero_ref::ZeroRefV1;

const MAX_FUZZ_CHARS: usize = 4096;

/// Seed corpus required by i4t3 (also mirrored under `corpus/`).
const SEED_CORPUS: &[(&str, &str)] = &[
    ("empty", ""),
    ("short_tz", "tz://blob/abc"),
    (
        "legal_64hex",
        "fz://blob/abababababababababababababababababababababababababababababababab",
    ),
    (
        "byte_span",
        "fz://blob/abababababababababababababababababababababababababababababababab#B0-4",
    ),
    (
        "line_span",
        "fz://blob/abababababababababababababababababababababababababababababababab#L1-2",
    ),
    (
        "garbage_fragment",
        "fz://blob/abababababababababababababababababababababababababababababababab#garbage",
    ),
    ("gz_node", "gz://node/x"),
];

fn parsers_agree(input: &str) {
    if input.chars().count() > MAX_FUZZ_CHARS {
        return;
    }
    let parse_ok = ZeroRefV1::parse(input).is_ok();
    let result_ok = ZeroResultV1::reference("0", input, None).is_ok();
    assert_eq!(
        parse_ok, result_ok,
        "ZeroRef::parse and ZeroResult::reference split on {input:?}: parse={parse_ok} result={result_ok}"
    );
}

fn canonical_ref(
    scheme: &str,
    identity: &[u8; 32],
    fragment_kind: u8,
    first: u16,
    width: u16,
) -> String {
    let hash = identity
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let base = format!("{scheme}://blob/{hash}");

    match fragment_kind {
        0 => base,
        1 => {
            let start = u64::from(first);
            let end = start + u64::from(width);
            format!("{base}#B{start}-{end}")
        }
        2 => {
            let start = u64::from(first) + 1;
            let end = start + u64::from(width);
            format!("{base}#L{start}-{end}")
        }
        _ => unreachable!("fragment kind is selected from the canonical cases"),
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(0x5eed_2e10),
        ..ProptestConfig::default()
    })]

    #[test]
    fn parse_display_identity_for_canonical_refs(
        identity in any::<[u8; 32]>(),
        first in any::<u16>(),
        width in any::<u16>(),
    ) {
        for scheme in ["fz", "gz", "tz"] {
            for fragment_kind in 0..3 {
                let canonical =
                    canonical_ref(scheme, &identity, fragment_kind, first, width);
                let parsed = ZeroRefV1::parse(&canonical).unwrap_or_else(|error| {
                    panic!("failed to parse generated canonical ref {canonical:?}: {error}")
                });
                let displayed = parsed.to_string();

                prop_assert_eq!(
                    &displayed,
                    &canonical,
                    "Display changed canonical ref; parsed value: {:?}",
                    parsed
                );

                let reparsed = ZeroRefV1::parse(&displayed).unwrap_or_else(|error| {
                    panic!("failed to reparse Display output {displayed:?}: {error}")
                });
                prop_assert_eq!(
                    &reparsed,
                    &parsed,
                    "Display output reparsed to a different value for {:?}",
                    canonical
                );
                parsers_agree(&canonical);
                prop_assert_eq!(
                    reparsed.to_string(),
                    canonical,
                    "reparsed value did not preserve the canonical string"
                );
            }
        }
    }

    #[test]
    fn unstructured_inputs_parsers_agree(input in ".{0,256}") {
        parsers_agree(&input);
    }

    #[test]
    fn structured_scheme_hash_fragment_parsers_agree(
        scheme in prop_oneof![
            Just("fz".to_string()),
            Just("gz".to_string()),
            Just("tz".to_string()),
            "[a-z]{0,6}",
        ],
        hash in prop_oneof![
            Just("ab".repeat(32)),
            "[a-f0-9]{0,80}",
            "[A-F0-9]{64}",
            Just("abc".to_string()),
        ],
        fragment in prop_oneof![
            Just("".to_string()),
            Just("#B0-4".to_string()),
            Just("#L1-2".to_string()),
            Just("#B0+4".to_string()),
            Just("#garbage".to_string()),
            Just("#".to_string()),
            "#[A-Za-z0-9+\\-]{0,16}",
        ],
    ) {
        let input = if scheme.is_empty() {
            format!("{hash}{fragment}")
        } else {
            format!("{scheme}://blob/{hash}{fragment}")
        };
        parsers_agree(&input);
    }
}

#[test]
fn seed_corpus_parsers_agree() {
    assert!(
        SEED_CORPUS.len() >= 5,
        "i4t3 requires at least five seed entries"
    );
    for (name, seed) in SEED_CORPUS {
        parsers_agree(seed);
        let on_disk = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/rust/zero-ref/corpus/zeroref_envelope_differential")
            .join(name);
        let disk = std::fs::read_to_string(&on_disk).unwrap_or_else(|error| {
            panic!("missing seed corpus file {}: {error}", on_disk.display())
        });
        assert_eq!(
            disk, *seed,
            "seed file {} drifted from the in-test corpus",
            on_disk.display()
        );
    }
}
