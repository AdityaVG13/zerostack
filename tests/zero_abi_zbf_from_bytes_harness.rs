//! CI crash+roundtrip harness for `ZbfObject::from_bytes`.
//! Bounded garbage campaign + round-trip identity. Reuses public parser only.
//! Bead: zerostack-zbf-from-bytes-ci-harness-8ok8

use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence};
use zero_abi::{
    ArtifactOwner, DurableProfile, DurableProfileId, Sha256Digest, ZbfArtifactKind, ZbfObject,
};

// ---------------------------------------------------------------------------
// Config - bounded, CI-friendly. Same WithSource style as abi_proptest.rs
// ---------------------------------------------------------------------------
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------
fn arb_kind() -> impl Strategy<Value = ZbfArtifactKind> {
    prop_oneof![
        Just(ZbfArtifactKind::AssemblyManifest),
        Just(ZbfArtifactKind::FsPack),
        Just(ZbfArtifactKind::GraphPack),
        Just(ZbfArtifactKind::TokenPack),
        Just(ZbfArtifactKind::Plan),
        Just(ZbfArtifactKind::Receipt),
        Just(ZbfArtifactKind::Witness),
        Just(ZbfArtifactKind::Effect),
        Just(ZbfArtifactKind::Snapshot),
    ]
}

fn arb_owner() -> impl Strategy<Value = ArtifactOwner> {
    prop_oneof![
        Just(ArtifactOwner::ZeroStack),
        Just(ArtifactOwner::FsZero),
        Just(ArtifactOwner::GraphZero),
        Just(ArtifactOwner::TokenZero),
        Just(ArtifactOwner::PiZeroStack),
    ]
}

fn arb_payload_bytes() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..512)
}

/// Generate a leaf object. Caller provides profile + digests.
fn arb_leaf(
    profile: DurableProfile,
    assembly: Sha256Digest,
    source_root: Sha256Digest,
    producer: Sha256Digest,
) -> impl Strategy<Value = ZbfObject> {
    (arb_kind(), arb_owner(), arb_payload_bytes()).prop_map(move |(kind, owner, payload)| {
        ZbfObject::new_leaf(
            kind,
            owner,
            assembly,
            profile,
            source_root,
            producer,
            payload,
        )
        .expect("bounded leaf")
    })
}

/// Recursive container/leaf generator with bounded depth/children.
fn arb_object_inner(
    profile: DurableProfile,
    assembly: Sha256Digest,
    source_root: Sha256Digest,
    producer: Sha256Digest,
    depth: u16,
) -> impl Strategy<Value = ZbfObject> {
    let leaf = arb_leaf(profile, assembly, source_root, producer);
    if depth == 0 {
        return leaf.boxed();
    }
    // At depth>0, produce either leaf or container of leaves/containers one level deeper.
    let child_depth = depth - 1;
    // Bound children to 0..4 to keep payload small and CI fast.
    prop_oneof![
        arb_leaf(profile, assembly, source_root, producer).boxed(),
        prop::collection::vec(
            arb_object_inner(profile, assembly, source_root, producer, child_depth),
            0..4
        )
        .prop_map(move |children| {
            // new_container with fixed kind/owner for determinism; children already bounded
            ZbfObject::new_container(
                ZbfArtifactKind::Snapshot,
                ArtifactOwner::ZeroStack,
                assembly,
                profile,
                source_root,
                producer,
                children,
            )
            .expect("bounded container")
        })
        .boxed(),
    ]
    .boxed()
}

fn arb_object() -> impl Strategy<Value = ZbfObject> {
    // Fixed digests/profile for determinism so round-trip can reuse same expected values.
    let profile = DurableProfile::portable_strict();
    let assembly = Sha256Digest::from_bytes([0x11; 32]);
    let source_root = Sha256Digest::from_bytes([0x22; 32]);
    let producer = Sha256Digest::from_bytes([0x33; 32]);
    // Depth 0..3 keeps recursion bounded while still exercising nesting.
    (0u16..4).prop_flat_map(move |d| arb_object_inner(profile, assembly, source_root, producer, d))
}

// Single valid fixture for E2E smoke.
fn valid_leaf_fixture() -> (ZbfObject, DurableProfile, Sha256Digest, Vec<u8>) {
    let profile = DurableProfile::portable_strict();
    let assembly = Sha256Digest::from_bytes([0xAA; 32]);
    let source_root = Sha256Digest::from_bytes([0xBB; 32]);
    let producer = Sha256Digest::from_bytes([0xCC; 32]);
    let obj = ZbfObject::new_leaf(
        ZbfArtifactKind::Plan,
        ArtifactOwner::GraphZero,
        assembly,
        profile,
        source_root,
        producer,
        b"hello zbf".to_vec(),
    )
    .unwrap();
    let bytes = obj.to_bytes(profile).unwrap();
    (obj, profile, assembly, bytes)
}

fn valid_container_fixture() -> (ZbfObject, DurableProfile, Sha256Digest, Vec<u8>) {
    let profile = DurableProfile::portable_strict();
    let assembly = Sha256Digest::from_bytes([0xAA; 32]);
    let source_root = Sha256Digest::from_bytes([0xBB; 32]);
    let producer = Sha256Digest::from_bytes([0xCC; 32]);
    let leaf1 = ZbfObject::new_leaf(
        ZbfArtifactKind::Receipt,
        ArtifactOwner::ZeroStack,
        assembly,
        profile,
        source_root,
        producer,
        b"leaf1".to_vec(),
    )
    .unwrap();
    let leaf2 = ZbfObject::new_leaf(
        ZbfArtifactKind::Witness,
        ArtifactOwner::FsZero,
        assembly,
        profile,
        source_root,
        producer,
        b"leaf2".to_vec(),
    )
    .unwrap();
    let container = ZbfObject::new_container(
        ZbfArtifactKind::Snapshot,
        ArtifactOwner::ZeroStack,
        assembly,
        profile,
        source_root,
        producer,
        vec![leaf1, leaf2],
    )
    .unwrap();
    let bytes = container.to_bytes(profile).unwrap();
    (container, profile, assembly, bytes)
}

// ---------------------------------------------------------------------------
// Crash oracle: garbage bytes never panic
// ---------------------------------------------------------------------------
proptest! {
    #![proptest_config(config())]

    #[test]
    fn garbage_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        let profile = DurableProfile::portable_strict();
        let assembly = Sha256Digest::from_bytes([0x11; 32]);
        // Must not panic; result is intentionally ignored.
        let _ = ZbfObject::from_bytes(&bytes, assembly, profile);
    }

    #[test]
    fn truncated_valid_never_panic(len in 0usize..4096) {
        let (_, profile, assembly, bytes) = valid_leaf_fixture();
        let truncated = if len < bytes.len() { &bytes[..len] } else { &bytes[..] };
        let _ = ZbfObject::from_bytes(truncated, assembly, profile);
    }

    #[test]
    fn bitflip_valid_never_panic(idx in 0usize..512, bit in 0u8..8) {
        let (_, profile, assembly, mut bytes) = valid_leaf_fixture();
        if !bytes.is_empty() {
            let i = idx % bytes.len();
            bytes[i] ^= 1u8 << (bit % 8);
        }
        let _ = ZbfObject::from_bytes(&bytes, assembly, profile);
    }

    #[test]
    fn roundtrip_identity_holds(obj in arb_object()) {
        let profile = DurableProfile::portable_strict();
        let assembly = Sha256Digest::from_bytes([0x11; 32]);
        let bytes = obj.to_bytes(profile).expect("to_bytes must succeed for generated valid object");
        let decoded = ZbfObject::from_bytes(&bytes, assembly, profile)
            .expect("from_bytes must succeed on own to_bytes");
        prop_assert_eq!(&decoded, &obj);
        // Byte-level identity: re-encode must match original encoding.
        let reencoded = decoded.to_bytes(profile).expect("re-encode");
        prop_assert_eq!(reencoded, bytes);
    }
}

// ---------------------------------------------------------------------------
// Deterministic E2E smoke: one valid fixture + one truncated buffer
// ---------------------------------------------------------------------------
#[test]
fn e2e_valid_leaf_roundtrip() {
    let (obj, profile, assembly, bytes) = valid_leaf_fixture();
    let decoded = ZbfObject::from_bytes(&bytes, assembly, profile).expect("valid leaf");
    assert_eq!(decoded, obj);
    assert_eq!(decoded.to_bytes(profile).unwrap(), bytes);
}

#[test]
fn e2e_valid_container_roundtrip() {
    let (obj, profile, assembly, bytes) = valid_container_fixture();
    let decoded = ZbfObject::from_bytes(&bytes, assembly, profile).expect("valid container");
    assert_eq!(decoded, obj);
    assert_eq!(decoded.to_bytes(profile).unwrap(), bytes);
}

#[test]
fn e2e_truncated_buffer_is_error_not_panic() {
    let (_, profile, assembly, bytes) = valid_leaf_fixture();
    // Truncate to header-1 (should be UnexpectedEof, not panic)
    let truncated = &bytes[..10];
    let err = ZbfObject::from_bytes(truncated, assembly, profile).unwrap_err();
    // Just assert it's an error; code mapping is not the focus here.
    let _ = err.code();
}

#[test]
fn e2e_empty_and_bad_magic_are_errors() {
    let profile = DurableProfile::portable_strict();
    let assembly = Sha256Digest::from_bytes([1; 32]);
    for seed in [b"".as_slice(), b"NOTZBF!!", &[0u8; 16], &[0xffu8; 64]] {
        let res = ZbfObject::from_bytes(seed, assembly, profile);
        assert!(
            res.is_err(),
            "seed {:?} must be rejected, got {:?}",
            seed,
            res
        );
    }
}

#[test]
fn e2e_curated_corpus_never_panics() {
    // Mirrors the tiny corpus in fuzz_corpus_untrusted_bytes_20260815.rs plus ZBF-specific edges.
    let profile = DurableProfile::portable_strict();
    let assembly = Sha256Digest::from_bytes([1; 32]);
    let (_, _, _, valid) = valid_leaf_fixture();
    let mut seeds: Vec<Vec<u8>> = vec![
        vec![],
        b"ZBFv0001".to_vec(),
        b"ZEROZBF1".to_vec(),
        vec![0u8; 192],
        vec![0xffu8; 192],
        vec![0u8; 191],
        vec![0u8; 193],
        {
            let mut b = valid.clone();
            b.extend_from_slice(b"trailing");
            b
        },
        {
            let mut b = valid.clone();
            if b.len() > 20 {
                b[15] = 0xFF; // unknown flags
            }
            b
        },
        {
            let mut b = valid.clone();
            if b.len() > 190 {
                b[185] = 0x01; // reserved nonzero
            }
            b
        },
    ];
    // payload_len overflow: set payload_len to u64::MAX in header
    {
        let mut b = valid.clone();
        if b.len() >= 24 {
            b[16..24].copy_from_slice(&u64::MAX.to_be_bytes());
        }
        seeds.push(b);
    }
    for s in &seeds {
        let _ = ZbfObject::from_bytes(s, assembly, profile);
    }
}

#[test]
fn e2e_all_profiles_roundtrip_leaf() {
    let assembly = Sha256Digest::from_bytes([0xAA; 32]);
    let source_root = Sha256Digest::from_bytes([0xBB; 32]);
    let producer = Sha256Digest::from_bytes([0xCC; 32]);
    for id in [
        DurableProfileId::PortableStrict,
        DurableProfileId::ApfsStrict,
        DurableProfileId::Ext4XfsStrict,
        DurableProfileId::NtfsStrict,
    ] {
        let profile = DurableProfile::new(id);
        let obj = ZbfObject::new_leaf(
            ZbfArtifactKind::Effect,
            ArtifactOwner::TokenZero,
            assembly,
            profile,
            source_root,
            producer,
            b"profile check".to_vec(),
        )
        .unwrap();
        let bytes = obj.to_bytes(profile).unwrap();
        let decoded = ZbfObject::from_bytes(&bytes, assembly, profile).unwrap();
        assert_eq!(decoded, obj, "profile {id:?} round-trip");
    }
}

#[test]
fn e2e_wrong_assembly_is_error() {
    let (obj, profile, _, bytes) = valid_leaf_fixture();
    let wrong = Sha256Digest::from_bytes([0xFF; 32]);
    let err = ZbfObject::from_bytes(&bytes, wrong, profile).unwrap_err();
    match err {
        zero_abi::ZbfError::AssemblyMismatch { expected, actual } => {
            assert_eq!(expected, wrong);
            assert_eq!(actual, obj.header.assembly_manifest_digest);
        }
        other => panic!("expected AssemblyMismatch, got {:?}", other),
    }
}

#[test]
fn e2e_max_depth_boundary() {
    let profile = DurableProfile::portable_strict();
    let assembly = Sha256Digest::from_bytes([0x11; 32]);
    let source_root = Sha256Digest::from_bytes([0x22; 32]);
    let producer = Sha256Digest::from_bytes([0x33; 32]);
    let mut leaf = ZbfObject::new_leaf(
        ZbfArtifactKind::Plan,
        ArtifactOwner::ZeroStack,
        assembly,
        profile,
        source_root,
        producer,
        b"deep".to_vec(),
    )
    .unwrap();
    for _ in 0..zero_abi::ZBF_MAX_DEPTH {
        leaf = ZbfObject::new_container(
            ZbfArtifactKind::Snapshot,
            ArtifactOwner::ZeroStack,
            assembly,
            profile,
            source_root,
            producer,
            vec![leaf],
        )
        .unwrap();
    }
    let bytes = leaf.to_bytes(profile).unwrap();
    let decoded = ZbfObject::from_bytes(&bytes, assembly, profile).unwrap();
    assert_eq!(decoded, leaf);
    let too_deep = ZbfObject::new_container(
        ZbfArtifactKind::Snapshot,
        ArtifactOwner::ZeroStack,
        assembly,
        profile,
        source_root,
        producer,
        vec![leaf],
    );
    let err = too_deep.unwrap_err();
    match err {
        zero_abi::ZbfError::DepthExceeded { .. } => {}
        other => panic!("expected DepthExceeded, got {:?}", other),
    }
}
