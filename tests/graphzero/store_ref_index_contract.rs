use std::collections::HashSet;
use std::fs;
use std::sync::Arc;
use std::thread;

use graphzero_store::store::blob_store::BlobStore;
use graphzero_store::store::expand::ExpandResolver;
use graphzero_store::store::query::persist_query_json;
use graphzero_store::store::ref_index;
use graphzero_store::store::refs::GzRef;
use serial_test::serial;
use zerostack_test_support::ScopedEnvVars;

/// Bind the ref-index env for a test. The returned guard holds the shared env
/// lock and restores both variables on drop (panic-safe).
fn set_index(path: &std::path::Path) -> ScopedEnvVars {
    let mut env = ScopedEnvVars::new();
    env.set("GRAPHZERO_REF_INDEX_PATH", path);
    env.remove("GRAPHZERO_REF_INDEX");
    env
}

/// Bind the ref-index env in disabled mode. Restore on drop is panic-safe.
fn disable_index(path: &std::path::Path) -> ScopedEnvVars {
    let mut env = ScopedEnvVars::new();
    env.set("GRAPHZERO_REF_INDEX_PATH", path);
    env.set("GRAPHZERO_REF_INDEX", "0");
    env
}

#[test]
#[serial]
fn cross_root_expands_blob_and_compact_query_refs() {
    let tmp = tempfile::tempdir().unwrap();
    let index = tmp.path().join("idx");
    let _env = set_index(&index);
    let store_a = tmp.path().join("a/.graphzero");
    let store_b = tmp.path().join("b/.graphzero");

    let blob_ref = {
        let store = BlobStore::open(&store_a).unwrap();
        let hash = store.put(b"cross-root blob").unwrap();
        format!("z://blob/{}", hash.to_hex())
    };
    let query_id =
        persist_query_json(&store_a, r#"{"kind":"query","ok":true}"#).expect("persist query");
    let compact_query_ref = format!("q:{query_id}");

    let resolver = ExpandResolver::new(&store_b, None).unwrap();
    let blob = resolver
        .resolve(&GzRef::parse(&blob_ref).unwrap(), &blob_ref)
        .unwrap();
    assert_eq!(blob.bytes, b"cross-root blob");
    assert_eq!(blob.source, "ref-index");

    let query = resolver
        .resolve(
            &GzRef::parse(&compact_query_ref).unwrap(),
            &compact_query_ref,
        )
        .unwrap();
    assert_eq!(query.bytes, br#"{"kind":"query","ok":true}"#);
    assert_eq!(query.source, "ref-index");
}

#[test]
#[serial]
fn unauthorized_indexed_root_emits_wrong_root() {
    use graphzero_store::store::expand::ExpandErrorKind;

    let tmp = tempfile::tempdir().unwrap();
    let index = tmp.path().join("idx");
    let _env = set_index(&index);
    let store_a = tmp.path().join("a/.graphzero");
    let store_b = tmp.path().join("b/.graphzero");
    let query_id = persist_query_json(&store_a, r#"{"kind":"query","acl":true}"#).unwrap();
    let reference = format!("query/{query_id}");

    let resolver = ExpandResolver::new(&store_b, None)
        .unwrap()
        .with_authorized_roots(vec![store_b.clone()]);
    let err = resolver
        .resolve(&GzRef::parse(&reference).unwrap(), &reference)
        .unwrap_err();
    assert_eq!(err.kind, ExpandErrorKind::WrongRoot, "got {}", err.reason);
    assert!(err.to_json().contains("\"kind\":\"wrong_root\""));
}

#[test]
#[serial]
fn env_disable_disables_mint_and_expand_index_tier() {
    let tmp = tempfile::tempdir().unwrap();
    let index = tmp.path().join("idx");
    let _env = disable_index(&index);
    let store_a = tmp.path().join("a/.graphzero");
    let store_b = tmp.path().join("b/.graphzero");
    let query_id = persist_query_json(&store_a, r#"{"disabled":true}"#).expect("persist query");
    let compact_query_ref = format!("q:{query_id}");

    let resolver = ExpandResolver::new(&store_b, None).unwrap();
    let err = resolver
        .resolve(
            &GzRef::parse(&compact_query_ref).unwrap(),
            &compact_query_ref,
        )
        .unwrap_err();
    assert!(err.to_json().contains("ref-index"), "{}", err.to_json());
    assert!(!index.exists(), "disabled mint side must not create index");
}

#[test]
#[serial]
fn stale_index_entries_are_pruned_lazily() {
    let tmp = tempfile::tempdir().unwrap();
    let index = tmp.path().join("idx");
    let _env = set_index(&index);
    fs::create_dir_all(&index).unwrap();
    let shard = index.join("ab.ndjson");
    fs::write(
        &shard,
        "{\"ref_id\":\"q:abcdef\",\"store_path\":\"/definitely/missing/graphzero/store\",\"ts\":1}\n",
    )
    .unwrap();

    assert!(ref_index::lookup_store("q:abcdef").is_none());
    let pruned = fs::read_to_string(&shard).unwrap_or_default();
    assert!(!pruned.contains("/definitely/missing/graphzero/store"));
}

#[test]
#[serial]
fn shard_compaction_keeps_newest_entry_per_ref() {
    let tmp = tempfile::tempdir().unwrap();
    let index = tmp.path().join("idx");
    let _env = set_index(&index);
    let store = tmp.path().join("store/.graphzero");
    fs::create_dir_all(&store).unwrap();
    let ref_id = format!("q:ab{}", "x".repeat(240));

    for _ in 0..5000 {
        ref_index::record_ref(&ref_id, &store).unwrap();
    }
    ref_index::compact_all_for_tests();
    let shard = index.join("ab.ndjson");
    assert!(fs::metadata(&shard).unwrap().len() < 1024 * 1024);
    let lines: Vec<_> = fs::read_to_string(&shard)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "compaction should keep only newest duplicate"
    );
    assert!(lines[0].contains(&ref_id));
}

#[test]
#[serial]
fn ref_index_write_failure_is_reported_with_context() {
    let tmp = tempfile::tempdir().unwrap();
    let blocked_index_path = tmp.path().join("not-a-directory");
    fs::write(&blocked_index_path, b"blocks directory creation").unwrap();
    let store = tmp.path().join("store/.graphzero");
    fs::create_dir_all(&store).unwrap();
    let _env = set_index(&blocked_index_path);

    let err = ref_index::record_ref("q:ab-write-fails", &store).unwrap_err();
    let message = format!("{err:#}");
    assert!(
        message.contains("record ref-index entry for q:ab-write-fails"),
        "{message}"
    );
    assert!(
        message.contains("create ref-index dir") || message.contains("open ref-index shard"),
        "{message}"
    );
}

#[test]
#[serial]
fn blob_put_fails_when_ref_index_cannot_be_recorded() {
    let tmp = tempfile::tempdir().unwrap();
    let blocked_index_path = tmp.path().join("not-a-directory");
    fs::write(&blocked_index_path, b"blocks directory creation").unwrap();
    let store = tmp.path().join("store/.graphzero");
    let _env = set_index(&blocked_index_path);

    let err = BlobStore::open(&store)
        .unwrap()
        .put(b"must not silently drop ref")
        .unwrap_err();
    let message = format!("{err:#}");
    assert!(
        message.contains("record ref-index entry for z://blob/"),
        "{message}"
    );
}

#[test]
#[serial]
fn concurrent_appends_do_not_corrupt_shard() {
    let tmp = tempfile::tempdir().unwrap();
    let index = tmp.path().join("idx");
    let _env = set_index(&index);
    let store = Arc::new(tmp.path().join("store/.graphzero"));
    fs::create_dir_all(store.as_ref()).unwrap();
    let expected_store = fs::canonicalize(store.as_ref()).unwrap();

    let mut handles = Vec::new();
    for thread_id in 0..8 {
        let store = Arc::clone(&store);
        handles.push(thread::spawn(move || {
            for i in 0..100 {
                ref_index::record_ref(&format!("q:aa{thread_id:02}{i:03}"), store.as_ref())
                    .unwrap();
            }
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }

    let shard = index.join("aa.ndjson");
    let contents = fs::read_to_string(shard).unwrap();
    let mut refs = HashSet::new();
    for line in contents.lines() {
        let value = serde_json::from_str::<serde_json::Value>(line).unwrap();
        refs.insert(
            value
                .get("ref_id")
                .and_then(|v| v.as_str())
                .expect("ref_id")
                .to_owned(),
        );
    }
    assert_eq!(
        refs.len(),
        800,
        "concurrent index append should preserve every unique ref"
    );
    for thread_id in 0..8 {
        for i in 0..100 {
            let ref_id = format!("q:aa{thread_id:02}{i:03}");
            assert!(refs.contains(&ref_id), "missing ref {ref_id}");
            assert_eq!(
                ref_index::lookup_store(&ref_id).as_deref(),
                Some(expected_store.as_path())
            );
        }
    }
}

/// the index must work when its directory path contains spaces
/// and non-ASCII characters (Windows user profiles commonly do).
#[test]
#[serial]
fn index_works_under_unicode_and_spaced_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let index = tmp.path().join("réf índex dir/with späces");
    let _env = set_index(&index);
    let store = tmp.path().join("ünï code store/.graphzero");
    let blob_store = BlobStore::open(&store).unwrap();
    let hash = blob_store.put(b"unicode path payload").unwrap().to_hex();

    let found = ref_index::lookup_store(&format!("z://blob/{hash}"));
    assert_eq!(
        found.as_deref().and_then(|p| p.canonicalize().ok()),
        store.canonicalize().ok(),
        "lookup must resolve through a unicode/spaced index dir"
    );
}

/// An orphaned compaction temp cannot shadow or corrupt the shard.
/// Readers see the old index, and the next compaction publishes a complete file.
#[test]
#[serial]
fn interrupted_replace_leaves_a_valid_index() {
    let tmp = tempfile::tempdir().unwrap();
    let index = tmp.path().join("idx");
    let _env = set_index(&index);
    let store = tmp.path().join("a/.graphzero");
    let blob_store = BlobStore::open(&store).unwrap();
    let hash = blob_store
        .put(b"interrupted replace payload")
        .unwrap()
        .to_hex();
    let ref_id = format!("z://blob/{hash}");

    // Simulate a compactor that died mid-write: a torn temp file next to the
    // shard, using the same naming scheme.
    let shard = fs::read_dir(&index)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().and_then(|s| s.to_str()) == Some("ndjson"))
        .expect("shard exists");
    let torn = shard.with_extension("ndjson.tmp-99999-0");
    fs::write(&torn, b"{\"ref_id\":\"z://blob/tor").unwrap();

    // Readers are unaffected: the destination shard is complete and valid.
    assert!(ref_index::lookup_store(&ref_id).is_some());

    // Compaction publishes a complete replacement; the shard never contains
    // a torn line afterwards.
    ref_index::compact_all_for_tests();
    let text = fs::read_to_string(&shard).unwrap();
    assert!(
        text.lines()
            .all(|l| serde_json::from_str::<serde_json::Value>(l).is_ok()),
        "every published line parses; no truncated destination"
    );
    assert!(text.contains(&hash), "newest entry survives the replace");
    // The young torn temp is never deleted (bounded cleanup only reaps old temps).
    assert!(
        torn.exists(),
        "possibly active temp files are never deleted"
    );
}
