use protobuf::Message;
use scip::types as t;
use std::path::PathBuf;

fn main() {
    let mut doc = t::Document {
        relative_path: "src/lib.rs".into(),
        ..Default::default()
    };
    doc.symbols.push(t::SymbolInformation {
        symbol: "sym alpha".into(),
        display_name: "alpha".into(),
        ..Default::default()
    });
    doc.symbols.push(t::SymbolInformation {
        symbol: "sym beta".into(),
        display_name: "beta".into(),
        ..Default::default()
    });
    doc.occurrences.push(t::Occurrence {
        symbol: "sym alpha".into(),
        range: vec![0, 3, 0, 8],
        ..Default::default()
    });
    doc.symbols[0].relationships.push(t::Relationship {
        symbol: "sym beta".into(),
        is_reference: true,
        ..Default::default()
    });
    let mut index = t::Index::new();
    index.documents.push(doc);
    let buf = index.write_to_bytes().unwrap();
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/sample.scip");
    std::fs::write(&out, &buf).unwrap();
    let golden = serde_json::json!({
        "symbol_count": 2,
        "relationship_count": 2,
        "document_count": 1
    });
    std::fs::write(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/sample.golden.json"),
        serde_json::to_string_pretty(&golden).unwrap(),
    )
    .unwrap();
}