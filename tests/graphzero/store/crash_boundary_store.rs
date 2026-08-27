use std::fs;
#[cfg(feature = "crash-injection")]
use std::process::Command;

use graphzero_store::Snapshot;
use graphzero_store::store::blob_store::BlobStore;
use graphzero_store::store::compaction::{append_entries, compact};
use graphzero_store::store::delta_log::{DeltaEntry, DeltaLog, entry_type};
use graphzero_store::store::format::{HEADER_LEN, SECTION_COUNT, ShardHeader};
#[cfg(feature = "crash-injection")]
use graphzero_store::store::indexer::authorize_crash_point;
use graphzero_store::store::indexer::index_repo;
use graphzero_store::store::manifest::Manifest;
use graphzero_store::store::query::encode_symbol;
use graphzero_store::store::shard::ShardReader;

#[test]
fn partial_shard_header_is_rejected_before_mmap() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("partial.gzsh");
    std::fs::write(&path, b"GZSH\x01").unwrap();

    let err = match ShardReader::open(&path) {
        Ok(_) => panic!("partial shard unexpectedly opened"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("file too small for GZSH header"));
}

#[test]
fn impossible_section_offset_is_rejected_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad-offset.gzsh");
    let mut offsets = [HEADER_LEN as u64; SECTION_COUNT];
    offsets[1] = 64 * 1024;
    let header = ShardHeader::new(offsets);
    std::fs::write(&path, bytemuck::bytes_of(&header)).unwrap();

    let err = match ShardReader::open(&path) {
        Ok(_) => panic!("corrupt shard unexpectedly opened"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("beyond file end"));
}

#[test]
fn crash_boundary_corpus_lists_store_recovery_cases() {
    let corpus = include_str!("../../benchmarks/crash-boundary/cases.jsonl");
    assert!(corpus.contains("\"component\":\"shard_write\""));
    assert!(corpus.contains("partial_shard_header_is_rejected_before_mmap"));
}

/// INV-DUR-1 / z6h7.3: `put_nosync` must queue files; `sync_all` must drain
/// them via per-file `sync_data` (not directory sync alone) before a reopen
/// can treat blob bytes as durable.
#[test]
fn put_nosync_sync_all_fsyncs_blob_file_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(".graphzero");
    let store = BlobStore::open(&root).unwrap();

    let payloads: [&[u8]; 3] = [b"blob-a-payload", b"blob-b-payload", b"blob-c-payload"];
    let mut hashes = Vec::new();
    for data in payloads {
        hashes.push(store.put_nosync(data).unwrap());
    }
    // Each put_nosync writes a single cas-local fan-out object (graphzero-56s1t).
    assert_eq!(
        store.pending_unsynced_count(),
        3,
        "put_nosync must leave one cas-local path per new blob pending the durability barrier"
    );

    store.sync_all().unwrap();
    assert_eq!(
        store.pending_unsynced_count(),
        0,
        "sync_all must fsync pending blob files then clear the queue"
    );

    // Simulated post-barrier crash: reopen store and read exact bytes.
    let reopened = BlobStore::open(&root).unwrap();
    for (hash, data) in hashes.iter().zip(payloads.iter()) {
        let got = reopened
            .get(hash)
            .unwrap()
            .unwrap_or_else(|| panic!("missing blob after sync_all: {}", hash.to_hex()));
        assert_eq!(got.as_slice(), *data);
    }
}

#[cfg(feature = "crash-injection")]
fn crash_capability(label: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    format!("graphzero-crash-v1-{label}-{}-{now}", std::process::id())
}

/// INV-DUR-1 / r0qv.8: crash between `put_nosync` and `sync_all` must not
/// publish a manifest that references those blobs.
#[cfg(feature = "crash-injection")]
#[test]
fn crash_before_blob_sync_does_not_publish_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let store = repo.join(".graphzero");
    fs::create_dir_all(repo.join("src")).unwrap();
    git2::Repository::init(&repo).unwrap();
    // Content not in git → indexer takes put_nosync path.
    fs::write(
        repo.join("src/lib.rs"),
        "fn crash_window_probe() { let _x = 1; }\n",
    )
    .unwrap();

    let exe = std::env::current_exe().expect("current_exe");
    let capability = crash_capability("before-blob-sync");
    let status = Command::new(&exe)
        .env("GRAPHZERO_CRASH_POINT", "before_blob_sync")
        .env("GRAPHZERO_CRASH_CAPABILITY", &capability)
        .env("GRAPHZERO_CRASH_CHILD", "1")
        .env("GRAPHZERO_CRASH_REPO", &repo)
        .env("GRAPHZERO_CRASH_STORE", &store)
        .arg("--exact")
        .arg("crash_child_index_at_before_blob_sync")
        .status()
        .expect("spawn crash child");

    // Child aborts at before_blob_sync → non-zero / signal.
    assert!(
        !status.success(),
        "child must abort at before_blob_sync, got {status}"
    );

    assert!(
        !store.join(".manifest").exists(),
        "manifest must not publish when crash hits before_blob_sync"
    );
    // Shard/global artifacts are written only after blob sync; none expected.
    let shards = store.join("shards");
    if shards.is_dir() {
        let entries: Vec<_> = fs::read_dir(&shards)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("shard_"))
            .collect();
        assert!(
            entries.is_empty(),
            "no shard artifacts after pre-sync crash: {entries:?}"
        );
    }
}

/// Child entry for [`crash_before_blob_sync_does_not_publish_manifest`].
/// Invoked only when `GRAPHZERO_CRASH_CHILD=1`.
#[cfg(feature = "crash-injection")]
#[test]
fn crash_child_index_at_before_blob_sync() {
    if std::env::var_os("GRAPHZERO_CRASH_CHILD").is_none() {
        return;
    }
    let repo = std::env::var("GRAPHZERO_CRASH_REPO").expect("GRAPHZERO_CRASH_REPO");
    let store = std::env::var("GRAPHZERO_CRASH_STORE").expect("GRAPHZERO_CRASH_STORE");
    let capability =
        std::env::var("GRAPHZERO_CRASH_CAPABILITY").expect("GRAPHZERO_CRASH_CAPABILITY");
    let _authorization =
        authorize_crash_point("before_blob_sync", &capability).expect("authorize before_blob_sync");
    let _ = index_repo(std::path::Path::new(&repo), std::path::Path::new(&store));
    panic!("expected abort at before_blob_sync");
}

#[cfg(feature = "crash-injection")]
#[test]
fn remaining_index_crash_points_recover_without_partial_publication() {
    for point in ["after_blob_sync", "after_shards", "before_rename"] {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let store = repo.join(".graphzero");
        let source = format!(
            "pub fn recovered_{}() -> u8 {{ 1 }}\n",
            point.replace('_', "")
        );
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src/lib.rs"), source.as_bytes()).unwrap();
        let expected_blob = graphzero_store::ContentHash::of(source.as_bytes());
        let capability = crash_capability(point);

        let status = Command::new(std::env::current_exe().unwrap())
            .env("GRAPHZERO_CRASH_POINT", point)
            .env("GRAPHZERO_CRASH_CAPABILITY", &capability)
            .env("GRAPHZERO_CRASH_MATRIX_CHILD", "1")
            .env("GRAPHZERO_CRASH_REPO", &repo)
            .env("GRAPHZERO_CRASH_STORE", &store)
            .arg("--exact")
            .arg("crash_child_index_at_matrix_point")
            .status()
            .unwrap();
        assert!(!status.success(), "child must abort at {point}: {status}");
        assert!(
            !store.join(".manifest").exists(),
            "{point} published a manifest before the atomic boundary"
        );

        let durable_blob = BlobStore::open(&store)
            .unwrap()
            .get(&expected_blob)
            .unwrap();
        assert_eq!(
            durable_blob.as_deref(),
            Some(source.as_bytes()),
            "{point} did not retain its post-sync blob bytes"
        );

        if point != "after_blob_sync" {
            let shard_paths: Vec<_> = fs::read_dir(store.join("shards"))
                .unwrap()
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with("shard_"))
                })
                .collect();
            assert!(!shard_paths.is_empty(), "{point} wrote no shard artifact");
            for path in shard_paths {
                ShardReader::open(&path).unwrap_or_else(|error| {
                    panic!("{point} left invalid shard {}: {error}", path.display())
                });
            }
        }

        let recovered = index_repo(&repo, &store)
            .unwrap_or_else(|error| panic!("reindex after {point} failed: {error:#}"));
        assert_eq!(recovered.snapshot_id, 1);
        assert!(store.join(".manifest").exists());
        let reopened = Snapshot::open(&store, Some(&repo)).unwrap();
        assert!(reopened.symbol_count().unwrap() >= 1);
    }
}

#[cfg(feature = "crash-injection")]
#[test]
fn crash_child_index_at_matrix_point() {
    if std::env::var_os("GRAPHZERO_CRASH_MATRIX_CHILD").is_none() {
        return;
    }
    let point = std::env::var("GRAPHZERO_CRASH_POINT").expect("GRAPHZERO_CRASH_POINT");
    let capability =
        std::env::var("GRAPHZERO_CRASH_CAPABILITY").expect("GRAPHZERO_CRASH_CAPABILITY");
    let repo = std::env::var("GRAPHZERO_CRASH_REPO").expect("GRAPHZERO_CRASH_REPO");
    let store = std::env::var("GRAPHZERO_CRASH_STORE").expect("GRAPHZERO_CRASH_STORE");
    let _authorization =
        authorize_crash_point(&point, &capability).expect("authorize matrix point");
    let _ = index_repo(std::path::Path::new(&repo), std::path::Path::new(&store));
    panic!("expected abort at {point}");
}

fn append_wal_symbol(store: &std::path::Path, byte: u8, name: &str) {
    let blob_hash = [byte; 32];
    append_entries(
        store,
        vec![
            DeltaEntry {
                entry_type: entry_type::COVERAGE,
                blob_hash,
                payload: vec![0b001],
            },
            DeltaEntry {
                entry_type: entry_type::SYMBOL,
                blob_hash,
                payload: encode_symbol(name, 0, 0, 0, 1).expect("test symbol encodes"),
            },
        ],
    )
    .unwrap();
}

#[cfg(feature = "crash-injection")]
#[test]
fn wal_open_compaction_crash_after_publish_never_replays_folded_segments() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let store = repo.join(".graphzero");
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(
        repo.join("src/lib.rs"),
        "pub fn base_symbol() {}
",
    )
    .unwrap();
    index_repo(&repo, &store).unwrap();
    let baseline = Snapshot::open(&store, Some(&repo))
        .unwrap()
        .symbol_count()
        .unwrap();
    append_wal_symbol(&store, 201, "first_wal_symbol");
    let first_segments = DeltaLog::segment_ids(&store.join("wal")).unwrap();

    let capability = crash_capability("compact-after-publish");
    let status = Command::new(std::env::current_exe().unwrap())
        .env("GRAPHZERO_CRASH_POINT", "after_publish")
        .env("GRAPHZERO_CRASH_CAPABILITY", &capability)
        .env("GRAPHZERO_COMPACT_CRASH_CHILD", "1")
        .env("GRAPHZERO_CRASH_STORE", &store)
        .arg("--exact")
        .arg("crash_child_compact_at_after_publish")
        .status()
        .unwrap();
    assert!(!status.success(), "child must abort after manifest publish");

    let published = Manifest::load(&store).unwrap();
    let published_ids = &published.latest().unwrap().segment_ids;
    assert!(first_segments.iter().all(|id| published_ids.contains(id)));
    assert_eq!(
        Snapshot::open(&store, Some(&repo))
            .unwrap()
            .symbol_count()
            .unwrap(),
        baseline + 1
    );

    append_wal_symbol(&store, 202, "second_wal_symbol");
    let all_segments = DeltaLog::segment_ids(&store.join("wal")).unwrap();
    compact(&store).unwrap();
    let republished = Manifest::load(&store).unwrap();
    let republished_ids = &republished.latest().unwrap().segment_ids;
    assert!(all_segments.iter().all(|id| republished_ids.contains(id)));
    assert_eq!(
        Snapshot::open(&store, Some(&repo))
            .unwrap()
            .symbol_count()
            .unwrap(),
        baseline + 2
    );
    assert!(
        DeltaLog::segment_ids(&store.join("wal"))
            .unwrap()
            .is_empty()
    );
}

#[cfg(feature = "crash-injection")]
#[test]
fn crash_child_compact_at_after_publish() {
    if std::env::var_os("GRAPHZERO_COMPACT_CRASH_CHILD").is_none() {
        return;
    }
    let store = std::env::var("GRAPHZERO_CRASH_STORE").expect("GRAPHZERO_CRASH_STORE");
    let capability =
        std::env::var("GRAPHZERO_CRASH_CAPABILITY").expect("GRAPHZERO_CRASH_CAPABILITY");
    let _authorization =
        authorize_crash_point("after_publish", &capability).expect("authorize after_publish");
    let _ = compact(std::path::Path::new(&store));
    panic!("expected abort at after_publish");
}
