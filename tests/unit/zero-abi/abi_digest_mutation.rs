//! SPEC-HUB-005 -- ABI digest bumps on Wire/version pins (C-23/24/26).
//!
//! Mutation test: changing a C-23/24/26 pin constant inside the test
//! must change `assembly_abi_contract_digest()`. Fails if the digest is
//! unchanged. C-25 semantic-mutation (ProtocolLimits defaults etc.) is
//! deliberately not asserted here; see `crates/zero-abi/src/digest.rs` and
//! `docs/spec/SPEC-TAGS.md` Notes -- C-25 stays Ambiguous.

use serde_json::Value;
use zero_abi::{
    ASSEMBLY_ABI_CONTRACT_VERSION, CWIR_CONTRACT_VERSION, MAX_ASSEMBLY_MANIFEST_BYTES,
    Sha256Digest, assembly_abi_contract_digest, assembly_abi_contract_manifest, canonical_json,
    sha256,
};

const ASSEMBLY_SRC: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/assembly.rs"));

fn digest_of(manifest: &Value) -> Sha256Digest {
    Sha256Digest::from_bytes(sha256(canonical_json(manifest).as_bytes()))
}

#[test]
fn unmutated_manifest_digest_equals_published() {
    let manifest = assembly_abi_contract_manifest();
    let expected = assembly_abi_contract_digest();
    let computed = digest_of(&manifest);
    assert_eq!(
        computed, expected,
        "canonical manifest digest must equal published assembly_abi_contract_digest"
    );
}

/// Pin constants, not JSON literals, must feed the hashed manifest.
/// Hardcoding `1` next to `const X: u16 = 1` would let a const bump leave the digest stuck.
#[test]
fn hashed_manifest_binds_c23_c24_c26_pin_constants() {
    let manifest = assembly_abi_contract_manifest();
    assert_eq!(
        manifest["contract_version"].as_u64(),
        Some(u64::from(ASSEMBLY_ABI_CONTRACT_VERSION)),
        "C-23: hashed contract_version must equal ASSEMBLY_ABI_CONTRACT_VERSION"
    );
    assert_eq!(
        manifest["linked_contracts"]["cwir_contract_version"].as_u64(),
        Some(u64::from(CWIR_CONTRACT_VERSION)),
        "C-24: hashed cwir_contract_version must equal CWIR_CONTRACT_VERSION"
    );
    assert_eq!(
        manifest["bounds"]["max_manifest_bytes"].as_u64(),
        Some(MAX_ASSEMBLY_MANIFEST_BYTES as u64),
        "C-26: hashed max_manifest_bytes must equal MAX_ASSEMBLY_MANIFEST_BYTES"
    );
    assert!(
        ASSEMBLY_SRC.contains("\"contract_version\": ASSEMBLY_ABI_CONTRACT_VERSION"),
        "C-23: assembly.rs must interpolate ASSEMBLY_ABI_CONTRACT_VERSION, not a numeric literal"
    );
    assert!(
        ASSEMBLY_SRC.contains("\"cwir_contract_version\": CWIR_CONTRACT_VERSION"),
        "C-24: assembly.rs must interpolate CWIR_CONTRACT_VERSION, not a numeric literal"
    );
    assert!(
        ASSEMBLY_SRC.contains("\"max_manifest_bytes\": MAX_ASSEMBLY_MANIFEST_BYTES"),
        "C-26: assembly.rs must interpolate MAX_ASSEMBLY_MANIFEST_BYTES, not a numeric literal"
    );
}

#[test]
fn contract_version_pin_mutation_bumps_digest() {
    // C-23 / Wire version pin: top-level contract_version (ASSEMBLY_ABI_CONTRACT_VERSION).
    let mut manifest = assembly_abi_contract_manifest();
    let baseline = digest_of(&manifest);
    // Mutate inside test; production const stays 1.
    manifest["contract_version"] = Value::from(999_u64);
    let mutated = digest_of(&manifest);
    assert_ne!(
        baseline, mutated,
        "SPEC-HUB-005: mutating C-23 contract_version must bump digest; got unchanged {}",
        baseline
    );
}

#[test]
fn cwir_linked_contract_version_pin_mutation_bumps_digest() {
    // C-24 / linked contract version pin.
    let mut manifest = assembly_abi_contract_manifest();
    let baseline = digest_of(&manifest);
    let linked = manifest
        .get_mut("linked_contracts")
        .and_then(Value::as_object_mut)
        .expect("linked_contracts object");
    let original = linked["cwir_contract_version"].as_u64().unwrap();
    linked["cwir_contract_version"] = Value::from(original + 100);
    let mutated = digest_of(&manifest);
    assert_ne!(
        baseline, mutated,
        "SPEC-HUB-005: mutating C-24 cwir_contract_version must bump digest"
    );
}

#[test]
fn max_manifest_bytes_bound_pin_mutation_bumps_digest() {
    // C-26 / bound pin: max_manifest_bytes (MAX_ASSEMBLY_MANIFEST_BYTES).
    let mut manifest = assembly_abi_contract_manifest();
    let baseline = digest_of(&manifest);
    let bounds = manifest
        .get_mut("bounds")
        .and_then(Value::as_object_mut)
        .expect("bounds object");
    let original = bounds["max_manifest_bytes"].as_u64().unwrap();
    bounds["max_manifest_bytes"] = Value::from(original + 1);
    let mutated = digest_of(&manifest);
    assert_ne!(
        baseline, mutated,
        "SPEC-HUB-005: mutating C-26 max_manifest_bytes must bump digest"
    );
}

#[test]
fn leaving_pins_untouched_keeps_digest_stable() {
    // Control: without mutation, digest is deterministic and stable across calls.
    let d1 = assembly_abi_contract_digest();
    let d2 = assembly_abi_contract_digest();
    assert_eq!(d1, d2, "digest must be deterministic across calls");
    let m1 = assembly_abi_contract_manifest();
    let m2 = assembly_abi_contract_manifest();
    assert_eq!(m1, m2, "manifest must be deterministic");
    assert_eq!(digest_of(&m1), d1);
}
