use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use zero_ref::ZeroRefV1;

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
                prop_assert_eq!(
                    reparsed.to_string(),
                    canonical,
                    "reparsed value did not preserve the canonical string"
                );
            }
        }
    }
}
