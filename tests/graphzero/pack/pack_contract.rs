use graphzero_pack::{
    MANIFEST_SCHEMA_VERSION, PackManifest, PackProvenance, PackSemanticSidecarEntry,
    PackShardEntry, PackSignKey, build_fixture_pack, build_pack_from_sources, install_pack,
    list_installed, query_pack_symbol_in_version, reproducible_manifest_digest,
    verify_pack_artifacts,
};
use tempfile::tempdir;

#[test]
fn unsigned_manifest_payload_preserves_schema_with_empty_signature() {
    let manifest = PackManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        pack_id: "fixture-deps".into(),
        version: "0.1.0".into(),
        tier_a_coverage: 1.0,
        shards: vec![PackShardEntry {
            file_name: "a.gzp".into(),
            content_sha256: "a".repeat(64),
            blob_count: 7,
            file_hash64: 42,
        }],
        semantic_sidecars: vec![PackSemanticSidecarEntry {
            gzsh_file_name: "a.gzsh".into(),
            file_name: "a.rs".into(),
            content_sha256: "b".repeat(64),
            record_count: 3,
        }],
        provenance: PackProvenance {
            lockfile_sha256: "c".repeat(64),
            toolchain: "rustc-test".into(),
            built_at_unix_nanos: 123,
        },
        signature_hex: "already-signed".into(),
    };

    let unsigned = manifest.canonical_unsigned_bytes().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&unsigned).unwrap();
    assert_eq!(json["schema_version"], MANIFEST_SCHEMA_VERSION);
    assert_eq!(json["signature_hex"], "");
    assert_eq!(json["shards"][0]["file_name"], "a.gzp");
    assert_eq!(json["semantic_sidecars"][0]["gzsh_file_name"], "a.gzsh");
    assert_eq!(json["provenance"]["toolchain"], "rustc-test");
}

#[test]
fn pack_manifest_roundtrip_and_signature() {
    let dir = tempdir().unwrap();
    let key = PackSignKey::fixture();
    let manifest_path = build_fixture_pack(dir.path(), &key).unwrap();
    let m = PackManifest::read_json(&manifest_path).unwrap();
    assert_eq!(m.schema_version, MANIFEST_SCHEMA_VERSION);
    assert!(!m.signature_hex.is_empty());
    verify_pack_artifacts(dir.path(), &m).unwrap();
    assert_eq!(reproducible_manifest_digest(&m).unwrap().len(), 64);
}

#[test]
fn pack_artifact_count_must_exactly_match_manifest_entries() {
    let dir = tempdir().unwrap();
    let key = PackSignKey::fixture();
    let manifest_path = build_fixture_pack(dir.path(), &key).unwrap();
    let manifest = PackManifest::read_json(&manifest_path).unwrap();

    let mut missing_entry = manifest.clone();
    missing_entry.shards.clear();
    let err = verify_pack_artifacts(dir.path(), &missing_entry).unwrap_err();
    assert!(err.to_string().contains("manifest shard count"));

    let shard = dir
        .path()
        .join("shards")
        .join(&manifest.shards[0].file_name);
    std::fs::copy(&shard, dir.path().join("shards/extra.gzsh")).unwrap();
    let err = verify_pack_artifacts(dir.path(), &manifest).unwrap_err();
    assert!(err.to_string().contains("manifest shard count"));
}

#[test]
fn tampered_signature_refuses_install() {
    let pack_dir = tempdir().unwrap();
    let store_root = tempdir().unwrap().path().join(".graphzero");
    std::fs::create_dir_all(&store_root).unwrap();
    let key = PackSignKey::fixture();
    let manifest_path = build_fixture_pack(pack_dir.path(), &key).unwrap();
    let mut m = PackManifest::read_json(&manifest_path).unwrap();
    m.signature_hex = "00".repeat(128);
    m.write_json(&manifest_path).unwrap();
    let err = install_pack(&store_root, &manifest_path, &key.public()).unwrap_err();
    assert!(err.to_string().contains("signature"));
}

#[test]
fn install_allows_same_pack_id_with_distinct_versions() {
    let store = tempdir().unwrap();
    let store_root = store.path().join(".graphzero");
    std::fs::create_dir_all(&store_root).unwrap();
    let key = PackSignKey::fixture();
    let sources = vec![(
        "dep_alpha.rs".to_string(),
        b"fn alpha() -> u64 { 1 }\n".to_vec(),
    )];

    let pack = tempdir().unwrap();
    let manifest =
        build_pack_from_sources(pack.path(), "fixture-deps", "0.1.0", &sources, &key).unwrap();
    let pack = tempdir().unwrap();
    let manifest =
        build_pack_from_sources(pack.path(), "fixture-deps", "0.2.0", &sources, &key).unwrap();

    install_pack(&store_root, &manifest, &key.public()).unwrap();
    install_pack(&store_root, &manifest, &key.public()).unwrap();

    let mut installed = list_installed(&store_root).unwrap();
    installed.sort_by(|a, b| a.version.cmp(&b.version));
    assert_eq!(installed.len(), 2);
    assert!(installed.iter().all(|pack| pack.pack_id == "fixture-deps"));
    assert_eq!(installed[0].version, "0.1.0");
    assert_eq!(installed[1].version, "0.2.0");
    assert!(
        installed[0]
            .manifest_path
            .contains("fixture-deps/0.1.0/manifest.json")
    );
    assert!(
        installed[1]
            .manifest_path
            .contains("fixture-deps/0.2.0/manifest.json")
    );

    let v1_hit = query_pack_symbol_in_version(&store_root, "fixture-deps", "0.1.0", "alpha")
        .unwrap()
        .expect("v1 symbol should be queryable");
    let v2_hit = query_pack_symbol_in_version(&store_root, "fixture-deps", "0.2.0", "alpha")
        .unwrap()
        .expect("v2 symbol should be queryable");
    assert_eq!(v1_hit.symbol, "alpha");
    assert_eq!(v2_hit.symbol, "alpha");
}

#[test]
fn install_rejects_same_pack_id_and_version() {
    let store = tempdir().unwrap();
    let store_root = store.path().join(".graphzero");
    std::fs::create_dir_all(&store_root).unwrap();
    let key = PackSignKey::fixture();
    let pack_dir = tempdir().unwrap();
    let manifest = build_fixture_pack(pack_dir.path(), &key).unwrap();

    install_pack(&store_root, &manifest, &key.public()).unwrap();
    let err = install_pack(&store_root, &manifest, &key.public()).unwrap_err();
    assert!(
        err.to_string()
            .contains("fixture-deps@0.1.0 already installed")
    );
}

#[test]
fn install_reports_stale_destination_cleanup_failure() {
    let store = tempdir().unwrap();
    let store_root = store.path().join(".graphzero");
    std::fs::create_dir_all(&store_root).unwrap();
    let key = PackSignKey::fixture();
    let pack_dir = tempdir().unwrap();
    let manifest = build_fixture_pack(pack_dir.path(), &key).unwrap();

    let stale_dest = graphzero_store::store::pack_registry::packs_root(&store_root)
        .join("fixture-deps")
        .join("0.1.0");
    std::fs::create_dir_all(stale_dest.parent().unwrap()).unwrap();
    std::fs::write(&stale_dest, b"not a directory").unwrap();

    let err = install_pack(&store_root, &manifest, &key.public()).unwrap_err();
    assert!(
        err.to_string()
            .contains("remove existing pack install directory"),
        "unexpected error: {err:#}"
    );
}
