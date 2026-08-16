//! Tiny seed-corpus regression for untrusted parse boundaries that still
//! lack a live cargo-fuzz campaign (zerostack-zbf-from-bytes-ci-harness-8ok8,
//! zerostack-raw-worker-unstructured-bytes-ci-c4j6). Must not panic.

use zero_abi::{
    decode_request_frame, decode_response_frame, DigestV1, DurableProfileV1, ZbfObjectV1,
    DEFAULT_MAX_FRAME_BYTES,
};

const FRAME_SEEDS: &[&[u8]] = &[
    b"",
    b"\n",
    b"\r\n",
    b"{",
    b"null",
    b"[]",
    b"{\"kind\":\"\"}",
    b"{\"kind\":\"handshake\"}",
    br#"{"kind":"handshake","request":{"protocol_version":"zerostack.raw_worker.v2","root":"/repo","session_id":"s1","expected_engine":"fszero","expected_worker_revision":"r1","expected_contract_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","expected_registry_digest":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}}"#,
    b"\x00\x01\x02\xff",
];

const ZBF_SEEDS: &[&[u8]] = &[
    b"",
    b"ZBFv0001",
    b"not-zbf",
    &[0u8; 16],
    &[0xffu8; 64],
];

#[test]
fn untrusted_frame_and_zbf_seeds_do_not_panic() {
    let profile = DurableProfileV1::portable_strict();
    let assembly = DigestV1::from_bytes([1; 32]);
    for seed in FRAME_SEEDS {
        let _ = decode_request_frame(seed, DEFAULT_MAX_FRAME_BYTES);
        let _ = decode_response_frame(seed, DEFAULT_MAX_FRAME_BYTES);
        let _ = decode_request_frame(seed, 8);
        let _ = decode_response_frame(seed, 8);
    }
    for seed in ZBF_SEEDS {
        let _ = ZbfObjectV1::from_bytes(seed, assembly, profile);
    }
}
