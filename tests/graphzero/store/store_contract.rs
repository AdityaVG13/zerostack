mod common;

use std::thread;
use std::time::{Duration, Instant};

use graphzero_store::store::blob_store::BlobStore;
#[cfg(unix)]
use graphzero_store::store::daemon::{disable_daemon, socket_path, spawn_stem_for_test};
use graphzero_store::store::path_safety::read_queries_file;
use graphzero_store::store::query::QueryEngine;
use graphzero_store::{ExpandResolver, GzRef};

#[test]
fn path_traversal_rejected() {
    for input in [
        "gz://query/../../etc/passwd",
        "gz://snap/../publish_tokens",
        "gz://node/foo/bar",
    ] {
        assert!(GzRef::parse(input).is_err(), "{input}");
    }
    let dir = tempfile::tempdir().unwrap();
    let store = BlobStore::open(dir.path()).unwrap();
    assert!(store.get_hex("../../publish_tokens").is_err());
    let store_root = dir.path().join(".graphzero");
    std::fs::create_dir_all(store_root.join("queries")).unwrap();
    std::fs::write(store_root.join("queries/safe123.json"), b"{}").unwrap();
    assert_eq!(
        read_queries_file(&store_root, "safe123.json").unwrap(),
        b"{}"
    );
    assert!(read_queries_file(&store_root, "../outside.json").is_err());
}

#[cfg(unix)]
#[test]
fn daemon_stem_blocks_idle() {
    let fx = common::indexed_repo_no_git();
    let handle = spawn_stem_for_test(fx.store_root.clone(), fx.repo_root.clone());
    for _ in 0..100 {
        if socket_path(&fx.store_root).exists() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(socket_path(&fx.store_root).exists());
    let start = Instant::now();
    thread::sleep(Duration::from_millis(200));
    assert!(start.elapsed() >= Duration::from_millis(150));
    disable_daemon(&fx.store_root).unwrap();
    drop(handle);
}

#[test]
fn expand_resolves_indexed_blob() {
    let fx = common::indexed_repo();
    let resolver = ExpandResolver::new(&fx.store_root, Some(&fx.repo_root)).unwrap();
    let snap = graphzero_store::Snapshot::open(&fx.store_root, Some(&fx.repo_root)).unwrap();
    let json = QueryEngine::warm(&snap, "alpha", 800)
        .unwrap()
        .to_json(Some(&fx.store_root));
    let reference = json
        .split("gz://blob/")
        .nth(1)
        .map(|s| {
            format!(
                "gz://blob/{}",
                s.chars().take_while(|c| *c != '"').collect::<String>()
            )
        })
        .expect("blob ref in snap json");
    let gz = GzRef::parse(&reference).unwrap();
    assert!(!resolver.resolve(&gz, &reference).unwrap().bytes.is_empty());
}
