//! Differentially checks `ZbfObject::from_bytes` against `refs_from_verified_bytes`.
//! The bounded garbage campaign requires the refs walker to avoid panics.

use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence};
use zero_abi::{ArtifactOwner, DurableProfile, Sha256Digest, ZbfArtifactKind, ZbfObject};
use zero_store::refs_from_verified_bytes;
use zerostack_test_support::sha256_hex;

fn config() -> Config {
    Config {
        cases: if cfg!(miri) { 8 } else { 128 },
        max_shrink_iters: if cfg!(miri) { 4 } else { 32 },
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

fn valid_leaf_bytes() -> Vec<u8> {
    let profile = DurableProfile::portable_strict();
    let assembly = Sha256Digest::from_bytes([0xAA; 32]);
    let obj = ZbfObject::new_leaf(
        ZbfArtifactKind::Plan,
        ArtifactOwner::ZeroStack,
        assembly,
        profile,
        Sha256Digest::from_bytes([0xBB; 32]),
        Sha256Digest::from_bytes([0xCC; 32]),
        b"leaf".to_vec(),
    )
    .unwrap();
    obj.to_bytes(profile).unwrap()
}

fn valid_container_bytes() -> Vec<u8> {
    let profile = DurableProfile::portable_strict();
    let assembly = Sha256Digest::from_bytes([0xAA; 32]);
    let s = Sha256Digest::from_bytes([0xBB; 32]);
    let p = Sha256Digest::from_bytes([0xCC; 32]);
    let leaf = ZbfObject::new_leaf(
        ZbfArtifactKind::Receipt,
        ArtifactOwner::ZeroStack,
        assembly,
        profile,
        s,
        p,
        b"child".to_vec(),
    )
    .unwrap();
    let container = ZbfObject::new_container(
        ZbfArtifactKind::Snapshot,
        ArtifactOwner::ZeroStack,
        assembly,
        profile,
        s,
        p,
        vec![leaf.clone(), leaf],
    )
    .unwrap();
    container.to_bytes(profile).unwrap()
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn refs_walker_never_panics_on_garbage(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        let _ = refs_from_verified_bytes(&bytes);
        // Also ensure from_bytes on same bytes never panics (differential corpus).
        let profile = DurableProfile::portable_strict();
        let assembly = Sha256Digest::from_bytes([0x11; 32]);
        let _ = ZbfObject::from_bytes(&bytes, assembly, profile);
    }



    #[test]
    fn valid_bytes_both_parsers_do_not_panic_and_agree_on_success(
        payload in prop::collection::vec(any::<u8>(), 0..256)
    ) {
        let profile = DurableProfile::portable_strict();
        let assembly = Sha256Digest::from_bytes([0xAA; 32]);
        let obj = ZbfObject::new_leaf(
            ZbfArtifactKind::Plan,
            ArtifactOwner::ZeroStack,
            assembly,
            profile,
            Sha256Digest::from_bytes([0xBB; 32]),
            Sha256Digest::from_bytes([0xCC; 32]),
            payload,
        )
        .unwrap();
        let bytes = obj.to_bytes(profile).unwrap();
        // from_bytes must succeed
        let decoded = ZbfObject::from_bytes(&bytes, assembly, profile).expect("from_bytes");
        prop_assert_eq!(decoded, obj);
        // refs walker must succeed; leaf has no refs
        let refs = refs_from_verified_bytes(&bytes).expect("refs on valid leaf");
        prop_assert!(refs.is_empty());
    }

    #[test]
    fn container_refs_match_children(
        n in 0usize..4,
        payload_len in 0usize..64
    ) {
        let profile = DurableProfile::portable_strict();
        let assembly = Sha256Digest::from_bytes([0xAA; 32]);
        let s = Sha256Digest::from_bytes([0xBB; 32]);
        let p = Sha256Digest::from_bytes([0xCC; 32]);
        let children: Vec<ZbfObject> = (0..n)
            .map(|i| {
                ZbfObject::new_leaf(
                    ZbfArtifactKind::Receipt,
                    ArtifactOwner::ZeroStack,
                    assembly,
                    profile,
                    s,
                    p,
                    vec![i as u8; payload_len],
                )
                .unwrap()
            })
            .collect();
        let container = ZbfObject::new_container(
            ZbfArtifactKind::Snapshot,
            ArtifactOwner::ZeroStack,
            assembly,
            profile,
            s,
            p,
            children.clone(),
        )
        .unwrap();
        let bytes = container.to_bytes(profile).unwrap();
        let decoded = ZbfObject::from_bytes(&bytes, assembly, profile).unwrap();
        prop_assert_eq!(decoded, container);
        // refs are content hashes of each child (deduped)
        let refs = refs_from_verified_bytes(&bytes).expect("refs on valid container");
        // Compute expected hashes independently from the zero-store walker.
        let mut expected: Vec<String> = children
            .iter()
            .map(|child| sha256_hex(&child.to_bytes(profile).unwrap()))
            .collect();
        expected.sort();
        expected.dedup();
        let mut got = refs.clone();
        got.sort();
        got.dedup();
        prop_assert_eq!(got, expected);
    }
}

#[test]
fn e2e_refs_on_non_zbf_is_empty() {
    // Non-magic bytes are leaves with no refs, not an error.
    for seed in [b"".as_slice(), b"hello", b"\x00\x01\x02"] {
        let refs = refs_from_verified_bytes(seed).unwrap();
        assert!(refs.is_empty(), "non-ZBF should be empty refs");
    }
}

#[test]
fn e2e_refs_on_truncated_zbf_is_error_not_panic() {
    let valid = valid_leaf_bytes();
    let truncated = &valid[..valid.len() / 2];
    let err = refs_from_verified_bytes(truncated).unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn e2e_refs_on_valid_leaf_empty_and_container_nonempty() {
    let leaf_bytes = valid_leaf_bytes();
    assert!(refs_from_verified_bytes(&leaf_bytes).unwrap().is_empty());

    // Independently compute the expected child reference for the container's
    // duplicated child and assert exact identity, not merely count.
    let profile = DurableProfile::portable_strict();
    let assembly = Sha256Digest::from_bytes([0xAA; 32]);
    let s = Sha256Digest::from_bytes([0xBB; 32]);
    let p = Sha256Digest::from_bytes([0xCC; 32]);
    let leaf = ZbfObject::new_leaf(
        ZbfArtifactKind::Receipt,
        ArtifactOwner::ZeroStack,
        assembly,
        profile,
        s,
        p,
        b"child".to_vec(),
    )
    .unwrap();
    let child_bytes = leaf.to_bytes(profile).unwrap();
    let expected = sha256_hex(&child_bytes);

    let container_bytes = valid_container_bytes();
    let refs = refs_from_verified_bytes(&container_bytes).unwrap();
    assert_eq!(
        refs,
        vec![expected],
        "container must expose exactly the deduplicated child digest"
    );
}
