use std::fs;

use tempfile::tempdir;
use zero_abi::ExpandOptions;
use zero_store::{SelectionIndex, ZERO_CAS_LAYOUT, ZeroCas, ZeroCasError, ZeroObjectMetadata};

#[test]
fn put_get_and_map_use_blake3_layout() {
    let root = tempdir().unwrap();
    let cas = ZeroCas::open(root.path());
    let bytes = b"alpha\nbeta\ngamma\n";
    let handle = cas.put(bytes).unwrap();
    assert_eq!(handle.digest(), blake3::hash(bytes).to_hex().as_str());
    assert_eq!(ZERO_CAS_LAYOUT, "blobs/blake3/<hh>/<digest>");
    assert!(
        cas.object_path(&handle)
            .to_string_lossy()
            .contains("blobs/blake3")
    );
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
