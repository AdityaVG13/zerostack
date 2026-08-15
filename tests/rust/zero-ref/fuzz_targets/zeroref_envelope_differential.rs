//! Differential fuzz target for envelope refs (i4t3).
//!
//! Spark / CI runs the same oracle via proptest:
//! `cargo test -p zero-ref --test property_identity -- differential`
//!
//! This file is the cargo-fuzz shape used by TokenZero's
//! `expand_fragment_differential` target. The live ZeroStack fuzz crate was
//! pruned (`archive/pruned-20260730/fuzz`); do not stand up a second harness
//! crate. Restore this file as `fuzz/fuzz_targets/zeroref_envelope_differential.rs`
//! if that crate comes back, then `cargo fuzz run zeroref_envelope_differential`.
//!
//! The production `zero-abi` / `zero-ref` trees stay free of libfuzzer /
//! arbitrary deps -- those belong only in a fuzz crate.

const MAX_FUZZ_BYTES: usize = 4096;

/// Crash + differential oracle: both parsers accept or both reject.
pub fn parsers_agree(input: &str) {
    if input.len() > MAX_FUZZ_BYTES {
        return;
    }
    let parse_ok = zero_ref::ZeroRefV1::parse(input).is_ok();
    let result_ok = zero_abi::ZeroResultV1::reference("0", input, None).is_ok();
    assert_eq!(
        parse_ok, result_ok,
        "ZeroRef::parse and ZeroResult::reference split on {input:?}"
    );
}

#[cfg(test)]
mod tests {
    use super::parsers_agree;

    #[test]
    fn documented_seeds_do_not_split() {
        for seed in [
            "",
            "tz://blob/abc",
            "fz://blob/abababababababababababababababababababababababababababababababab",
            "fz://blob/abababababababababababababababababababababababababababababababab#B0-4",
            "gz://node/x",
        ] {
            parsers_agree(seed);
        }
    }
}
