//! Shared, test-only fixtures for ZeroStack integration targets.
//!
//! Production crates must not depend on this package. Keep only fixtures with
//! multiple real consumers; one-off builders belong beside their test.

#![forbid(unsafe_code)]

use std::fmt::Write as _;

use zero_abi::{
    CapsuleEventRoots, KernelLedger, ZERO_KERNEL_PROTOCOL, ZeroHandle, ZeroKernelEvent,
    ZeroKernelOutcome,
};
/// Valid lowercase 64-hex root: repeats `hex` 64 times.
pub fn root64(hex: char) -> String {
    assert!(
        hex.is_ascii_hexdigit() && !hex.is_ascii_uppercase(),
        "root64 expects lowercase hex digit, got {hex}"
    );
    std::iter::repeat_n(hex, 64).collect()
}

/// Independent SHA-256 hex oracle shared by storage tests.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = <sha2::Sha256 as sha2::Digest>::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut hex, "{byte:02x}").unwrap();
    }
    hex
}

/// Canonical [`CapsuleEventRoots`] fixture used across zero-kernel and
/// zero-store event tests.
pub fn capsule_roots() -> CapsuleEventRoots {
    CapsuleEventRoots {
        capsule_root: root64('1'),
        capsule_object: ZeroHandle::from_digest(&root64('a')).unwrap(),
        provider_root: root64('2'),
        cache_root: root64('3'),
        speculation_root: root64('4'),
        effect_root: root64('5'),
        quality_root: root64('6'),
        occurrence_root: root64('7'),
    }
}

/// Completed, capsule-rooted [`ZeroKernelEvent`] over `visible` bytes.
/// `model_visible_digest` is `blake3(visible)` hex, matching publish oracles.
pub fn capsule_event(visible: &[u8]) -> ZeroKernelEvent {
    ZeroKernelEvent {
        protocol: ZERO_KERNEL_PROTOCOL.into(),
        session_id: "session".into(),
        cell_id: "cell".into(),
        source_digest: "source".into(),
        contract_digest: "contract".into(),
        policy_digest: "policy".into(),
        state_root_before: None,
        state_root_after: None,
        input_handles: vec![],
        output_handles: vec![],
        outcome: ZeroKernelOutcome::Completed,
        ledger: KernelLedger::default(),
        model_visible_digest: blake3::hash(visible).to_hex().to_string(),
        turn: None,
        capsule: Some(capsule_roots()),
    }
}
