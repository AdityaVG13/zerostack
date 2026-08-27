//! Fuzz the WAL delta-codec decoders (graphzero-la0n).
//!
//! Invariant: whenever a decode succeeds, re-encoding the decoded semantic
//! tuple must succeed and decode back to the identical tuple. Arbitrary
//! trailing bytes are not asserted to be canonical; only semantic
//! round-trip is required.

#![no_main]

use libfuzzer_sys::fuzz_target;

use graphzero_store::store::query::{
    decode_edge, decode_symbol, encode_edge_with_meta, encode_symbol,
};

fuzz_target!(|data: &[u8]| {
    if let Some((name, (kind, tier, start, end))) = decode_symbol(data) {
        let encoded =
            encode_symbol(&name, kind, tier, start, end).expect("re-encode decoded symbol");
        let (round_name, (round_kind, round_tier, round_start, round_end)) =
            decode_symbol(&encoded).expect("decode re-encoded symbol");
        assert_eq!(round_name, name);
        assert_eq!(
            (round_kind, round_tier, round_start, round_end),
            (kind, tier, start, end)
        );
    }

    if let Some((src, dst, kind, conf, start, end, source)) = decode_edge(data) {
        let encoded = encode_edge_with_meta(&src, &dst, kind, conf, start, end, source.as_deref())
            .expect("re-encode decoded edge");
        let (round_src, round_dst, round_kind, round_conf, round_start, round_end, round_source) =
            decode_edge(&encoded).expect("decode re-encoded edge");
        assert_eq!(round_src, src);
        assert_eq!(round_dst, dst);
        assert_eq!(round_kind, kind);
        assert_eq!(round_conf, conf);
        assert_eq!(round_start, start);
        assert_eq!(round_end, end);
        assert_eq!(round_source, source);
    }
});
