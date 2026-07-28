mod common;
use std::borrow::Cow;
use common::fixture;
use zero_cert::{verify, VerificationError};

#[test]
fn fixed_seed_valid_and_single_bit_tampering_property() {
    let mut state = 0x4d59_5df4_d0f3_3173_u64;
    for len in 1..=128usize {
        let mut bytes = vec![0; len];
        for byte in &mut bytes { state ^= state << 13; state ^= state >> 7; state ^= state << 17; *byte = state as u8; }
        let (certificate, resident) = fixture(&bytes);
        assert!(verify(&certificate, &resident).is_ok());
        let mut payload = certificate.clone(); let mut changed = bytes.clone(); changed[len / 2] ^= 1; payload.payload = Cow::Owned(changed);
        assert!(matches!(verify(&payload, &resident), Err(VerificationError::PayloadMismatch { .. })));
        let mut digest = certificate.clone(); digest.spans[0].span_digest[(state as usize) & 31] ^= 1;
        assert!(matches!(verify(&digest, &resident), Err(VerificationError::SpanDigestMismatch { .. })));
    }
}
