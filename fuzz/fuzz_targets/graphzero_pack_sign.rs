//! Fuzz pack-manifest parse + fixture-key sign/verify (graphzero-la0n).
//!
//! Invariants:
//! - Malformed JSON or unsupported schema is a normal parse failure.
//! - The input's own signature or key fields are never trusted; the fixture
//!   key is the only authority.
//! - Once a manifest is fixture-signed, fixture-key verification must
//!   succeed, and flipping its first signature hex nibble must fail.

#![no_main]

use libfuzzer_sys::fuzz_target;

use graphzero_pack::{PackManifest, PackSignKey, sign_manifest, verify_manifest_signature};

fuzz_target!(|data: &[u8]| {
    let manifest: PackManifest = match serde_json::from_slice(data) {
        Ok(manifest) => manifest,
        Err(_) => return, // malformed JSON is a normal parse failure
    };
    if manifest.validate_schema().is_err() {
        return; // legacy or unsupported schema: not signable
    }

    let key = PackSignKey::fixture();
    let mut signed = manifest;
    if sign_manifest(&mut signed, &key).is_err() {
        return; // canonical serialization failed; nothing signable
    }
    if signed.signature_hex.is_empty() {
        return;
    }
    let public = key.public();
    verify_manifest_signature(&signed, &public)
        .expect("fixture-signed manifest must verify with the fixture key");

    // Mutate exactly one signature hex nibble; verification must fail.
    let mut tampered = signed;
    let mut bytes = tampered.signature_hex.clone().into_bytes();
    let replacement = if bytes[0] == b'0' { b'1' } else { b'0' };
    bytes[0] = replacement;
    tampered.signature_hex = String::from_utf8(bytes).expect("signature hex is ASCII");
    let result = verify_manifest_signature(&tampered, &public);
    assert!(result.is_err(), "tampered signature must fail verification");
});
