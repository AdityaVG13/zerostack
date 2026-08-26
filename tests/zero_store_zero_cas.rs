use std::fs;
use std::path::PathBuf;

use tempfile::tempdir;
use zero_abi::ExpandOptions;
use zero_store::{SelectionIndex, ZeroCas, ZeroCasError, ZeroObjectMetadata};

#[test]
fn put_get_and_map_use_blake3_layout() {
    let root = tempdir().unwrap();
    let cas = ZeroCas::open(root.path());
    let bytes = b"alpha\nbeta\ngamma\n";
    let expected_digest = blake3::hash(bytes).to_hex().to_string();
    let handle = cas.put(bytes).unwrap();
    assert_eq!(handle.digest(), expected_digest);
    // Exact documented layout: blobs/blake3/<hh>/<digest> where <hh> are first two digest chars.
    let expected_rel = PathBuf::from("blobs")
        .join("blake3")
        .join(&expected_digest[..2])
        .join(&expected_digest);
    assert_eq!(
        cas.object_path(&handle).strip_prefix(root.path()).unwrap(),
        expected_rel
    );
    assert_eq!(cas.object_path(&handle), root.path().join(&expected_rel));
    assert_eq!(cas.get(&handle).unwrap(), bytes);
    assert_eq!(cas.map(&handle).unwrap().bytes(), bytes);
}

#[test]
fn line_and_symbol_expansion_use_published_metadata() {
    let root = tempdir().unwrap();
    let cas = ZeroCas::open(root.path());
    let text = "alpha\nbeta\ngamma\n";
    let handle = cas.put(text.as_bytes()).unwrap();
    let index = SelectionIndex::from_utf8(text)
        .with_symbol("middle", 6, 10)
        .unwrap();
    cas.publish_metadata(&ZeroObjectMetadata {
        handle: handle.clone(),
        byte_len: text.len() as u64,
        media_type: "text/plain".into(),
        producer: "test".into(),
        contract_digest: "contract".into(),
        selection: Some(index),
    })
    .unwrap();

    let lines = cas
        .expand(
            &handle,
            &ExpandOptions {
                line_start: Some(2),
                line_end: Some(2),
                ..ExpandOptions::default()
            },
        )
        .unwrap();
    assert_eq!(lines, b"beta\n");

    let symbol = cas
        .expand(
            &handle,
            &ExpandOptions {
                symbol: Some("middle".into()),
                ..ExpandOptions::default()
            },
        )
        .unwrap();
    assert_eq!(symbol, b"beta");
}

#[test]
fn line_expansion_derives_offsets_for_plain_exact_handles() {
    let root = tempdir().unwrap();
    let cas = ZeroCas::open(root.path());
    let handle = cas.put(b"alpha\nbeta\ngamma\n").unwrap();

    let lines = cas
        .expand(
            &handle,
            &ExpandOptions {
                line_start: Some(2),
                line_end: Some(2),
                ..ExpandOptions::default()
            },
        )
        .unwrap();

    assert_eq!(lines, b"beta\n");
}

#[test]
fn corruption_is_refused_before_bytes_escape() {
    let root = tempdir().unwrap();
    let cas = ZeroCas::open(root.path());
    let handle = cas.put(b"trusted").unwrap();
    fs::write(cas.object_path(&handle), b"tampered").unwrap();
    assert!(matches!(
        cas.get(&handle),
        Err(ZeroCasError::Corrupt { .. })
    ));
    assert!(matches!(
        cas.map(&handle),
        Err(ZeroCasError::Corrupt { .. })
    ));
}
