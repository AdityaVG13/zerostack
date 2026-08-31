use fszero_store::*;
use zero_abi::WorkCapsule;

fn capsule(epoch: u64) -> WorkCapsule {
    let digest = |byte: char| std::iter::repeat_n(byte, 64).collect();
    WorkCapsule {
        version: 1,
        roots: zero_abi::CapsuleRoots {
            project: digest('a'),
            task: digest('b'),
            protected_scope: digest('c'),
            obligations: digest('d'),
            evidence: digest('e'),
            policy: digest('f'),
            execution: digest('1'),
            verifier: digest('2'),
            fallback: digest('3'),
            ledger: digest('4'),
        },
        state: zero_abi::CapsuleState::Draft,
        epoch,
        provider_usage_budget: 10,
        complete_work_budget: 20,
    }
}

#[test]
fn incomplete_support_graph_blocks_gc() {
    assert!(matches!(
        plan_gc_dry_run(
            ["capsule".into()],
            ["capsule".into()],
            [GcSupportEdge {
                from: "capsule".into(),
                to: "missing".into(),
            }],
            true,
        ),
        GcDryRun::Unknown { .. }
    ));
}

#[test]
fn complete_support_graph_proves_transitive_reclamation() {
    assert_eq!(
        plan_gc_dry_run(
            ["capsule".into(), "evidence".into(), "garbage".into()],
            ["capsule".into()],
            [GcSupportEdge {
                from: "capsule".into(),
                to: "evidence".into(),
            }],
            true,
        ),
        GcDryRun::Complete {
            retained: vec!["capsule".into(), "evidence".into()],
            reclaimable: vec!["garbage".into()],
        }
    );
}

#[test]
fn capsule_and_projection_round_trip_through_cas() {
    let root = tempfile::tempdir().unwrap();
    let blobs = root.path().join("blobs");
    let store = CapsuleObjectStore::new(CasStore::at_blobs_root(&blobs));
    let capsule = capsule(1);
    let receipt = store.put(&capsule).unwrap();
    assert_eq!(store.get(&receipt.object_hash).unwrap(), capsule);

    let source = b"abcdefghi";
    let projection = store
        .put_projection(
            &DirectProjectionManifest {
                source_hash: fszero_store::access_log::content_hash_bytes(source),
                projection_kind: "two_ranges".into(),
                ranges: vec![
                    ProjectionRange { start: 0, end: 3 },
                    ProjectionRange { start: 6, end: 9 },
                ],
            },
            source,
        )
        .unwrap();
    assert_eq!(
        CasStore::at_blobs_root(blobs)
            .get(&projection.projection_hash)
            .unwrap(),
        b"abcghi"
    );
}

#[test]
fn capsule_put_is_idempotent_and_exact() {
    let root = tempfile::tempdir().unwrap();
    let store = CapsuleObjectStore::new(CasStore::at_blobs_root(root.path().join("blobs")));
    let capsule = capsule(1);
    let first = store.put(&capsule).unwrap();
    assert!(first.created);
    let second = store.put(&capsule).unwrap();
    assert!(!second.created, "identical bytes must not be re-created");
    assert_eq!(second.object_hash, first.object_hash);
    assert_eq!(second.capsule_root, first.capsule_root);
    assert_eq!(second.capsule_root, capsule.root().unwrap());
    assert_eq!(store.get(&first.object_hash).unwrap(), capsule);
    assert_eq!(
        store
            .get_expected(&first.object_hash, &first.capsule_root)
            .unwrap(),
        capsule,
    );
}

#[test]
fn get_expected_refuses_wrong_capsule_root() {
    let root = tempfile::tempdir().unwrap();
    let store = CapsuleObjectStore::new(CasStore::at_blobs_root(root.path().join("blobs")));
    let receipt = store.put(&capsule(1)).unwrap();
    let other = capsule(2).root().unwrap();
    assert_ne!(receipt.capsule_root, other);
    let error = store
        .get_expected(&receipt.object_hash, &other)
        .expect_err("wrong expected root must be refused");
    assert!(matches!(
        error,
        CapsuleStoreError::ExactRootMismatch { expected, actual }
            if expected == other && actual == receipt.capsule_root
    ));
}

#[test]
fn get_expected_refuses_tampered_object() {
    let root = tempfile::tempdir().unwrap();
    let blobs = root.path().join("blobs");
    let store = CapsuleObjectStore::new(CasStore::at_blobs_root(&blobs));
    let receipt = store.put(&capsule(1)).unwrap();
    let object = CasStore::at_blobs_root(&blobs)
        .object_path(&receipt.object_hash)
        .unwrap();
    std::fs::write(&object, b"tampered").unwrap();
    let error = store
        .get_expected(&receipt.object_hash, &receipt.capsule_root)
        .expect_err("tampered object must be refused");
    assert!(matches!(
        error,
        CapsuleStoreError::Cas(CasError::Corrupt { .. })
    ));
}

#[test]
fn get_expected_refuses_envelope_root_mismatch() {
    let root = tempfile::tempdir().unwrap();
    let blobs = root.path().join("blobs");
    let store = CapsuleObjectStore::new(CasStore::at_blobs_root(&blobs));
    store.put(&capsule(1)).unwrap();
    // A hash-consistent envelope that declares a root different from the
    // manifest's own root must never be served.
    let forged = serde_json::json!({
        "capsuleRoot": std::iter::repeat_n('f', 64).collect::<String>(),
        "manifest": serde_json::to_value(capsule(1)).unwrap(),
    });
    let forged_bytes = zero_abi::canonical_json(&forged).into_bytes();
    let forged_hash = CasStore::at_blobs_root(&blobs)
        .put(&forged_bytes)
        .unwrap()
        .hash;
    let error = store
        .get_expected(
            &forged_hash,
            &std::iter::repeat_n('f', 64).collect::<String>(),
        )
        .expect_err("envelope root mismatch must be refused");
    assert!(matches!(error, CapsuleStoreError::RootMismatch { .. }));
}

#[test]
fn equal_exact_pack_tiers_do_not_dominate_each_other() {
    let tier = |name: &str, exact| PackTierResources {
        tier: name.into(),
        exact,
        resident_bytes: 10,
        metadata_bytes: 1,
        expected_read_bytes: 10,
        expected_decode_work: 1,
    };
    let frontier =
        nondominated_pack_tiers(&[tier("a", true), tier("b", true), tier("lossy", false)]);
    assert_eq!(
        frontier
            .iter()
            .map(|candidate| candidate.tier.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
}
