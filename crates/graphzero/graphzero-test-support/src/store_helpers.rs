use std::path::Path;

pub fn publish_token(store_root: &Path) {
    graphzero_store::install_capability_token(store_root, "test-publish-token")
        .expect("failed to install capability token in fixture store");
}

pub fn evidence_for_file(repo_root: &Path, rel: &str, start: u32, end: u32) -> String {
    let content = std::fs::read(repo_root.join(rel))
        .unwrap_or_else(|err| panic!("failed to read fixture evidence file {rel}: {err}"));
    let hash = graphzero_store::ContentHash::of(&content);
    graphzero_store::store::refs::blob_span_ref(&hash.to_hex(), start, end)
}

pub fn minimal_batch(edges_json: &str) -> String {
    format!(
        r#"{{"schema_version":"publish/v1","publisher":"ci.flake-detector","edges":[{edges_json}]}}"#
    )
}
