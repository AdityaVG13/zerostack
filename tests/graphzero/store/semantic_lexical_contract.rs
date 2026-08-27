mod common;

use graphzero_store::Snapshot;
use graphzero_store::store::indexer;
use graphzero_store::store::query::snap;

fn build_semantic_fixture() -> common::Fixture {
    let fx = common::make_repo();
    std::fs::write(
        fx.repo_root.join("src/checksum.rs"),
        "/// Computes a rolling checksum over a byte buffer.\n\
         /// Used by the delta encoder to detect changes.\n\
         pub fn rolling_checksum(data: &[u8]) -> u64 {\n\
         \x20   let mut hash: u64 = 0;\n\
         \x20   for &b in data {\n\
         \x20       hash = hash.wrapping_mul(31).wrapping_add(b as u64);\n\
         \x20   }\n\
         \x20   hash\n\
         }\n",
    )
    .unwrap();
    std::fs::write(
        fx.repo_root.join("src/manifest.rs"),
        "/// Parses a JSON manifest into a typed structure.\n\
         pub fn parse_manifest(text: &str) -> Manifest {\n\
         \x20   serde_json::from_str(text).unwrap_or_default()\n\
         }\n\
         \n\
         #[derive(Default)]\n\
         pub struct Manifest {\n\
         \x20   pub name: String,\n\
         }\n",
    )
    .unwrap();
    std::fs::write(
        fx.repo_root.join("src/worker.rs"),
        "/// Spawns a background worker thread.\n\
         pub fn spawn_worker(pool: &Pool) {\n\
         \x20   pool.spawn(|| { run_tasks(); });\n\
         }\n",
    )
    .unwrap();
    indexer::index_repo(&fx.repo_root, &fx.store_root).unwrap();
    fx
}

#[test]
fn semantic_route_returns_destinations_for_natural_language() {
    let fx = build_semantic_fixture();
    let snapshot = Snapshot::open(&fx.store_root, Some(&fx.repo_root)).unwrap();
    let capsule = snap(
        &snapshot,
        "compute rolling checksum digest",
        4096,
        None,
        false,
    )
    .unwrap();
    assert_eq!(
        capsule.route,
        graphzero_store::store::query::SnapRoute::Semantic
    );
    assert!(
        !capsule.destinations.is_empty(),
        "semantic route must return destinations"
    );
    let top = &capsule.destinations[0];
    assert!(
        top.label.contains("checksum"),
        "top destination should be rolling_checksum, got: {}",
        top.label
    );
    assert!(
        top.evidence_ref.starts_with("gz://blob/"),
        "evidence_ref must be a blob ref: {}",
        top.evidence_ref
    );
    assert!(
        !capsule
            .diagnostics
            .notes
            .iter()
            .any(|n| n == "semantic_degraded"),
        "semantic_degraded must be removed when tier served"
    );
    assert!(
        capsule
            .diagnostics
            .notes
            .iter()
            .any(|n| n == "lexical_semantic_served"),
        "should note lexical_semantic_served"
    );
}

#[test]
fn semantic_destinations_round_trip_byte_exact() {
    use graphzero_store::store::expand::ExpandResolver;
    use graphzero_store::store::refs::GzRef;

    let fx = build_semantic_fixture();
    let snapshot = Snapshot::open(&fx.store_root, Some(&fx.repo_root)).unwrap();
    let resolver = ExpandResolver::new(&fx.store_root, Some(&fx.repo_root)).unwrap();

    let capsule = snap(&snapshot, "parse manifest json", 4096, None, false).unwrap();
    assert!(!capsule.destinations.is_empty());

    let dest = &capsule.destinations[0];
    let gz_ref = GzRef::parse(&dest.evidence_ref).unwrap();
    let expanded = resolver.resolve(&gz_ref, &dest.evidence_ref).unwrap();
    let bytes = &expanded.bytes;
    assert!(
        bytes
            .windows(8)
            .any(|w| w.eq_ignore_ascii_case(b"manifest")),
        "expanded bytes must contain the queried concept, got: {}",
        String::from_utf8_lossy(bytes).as_ref()
    );
}

#[test]
fn semantic_coverage_percent_is_honest() {
    let fx = build_semantic_fixture();
    let snapshot = Snapshot::open(&fx.store_root, Some(&fx.repo_root)).unwrap();
    let pct = snapshot.semantic_tier_percent();
    assert!(
        pct > 0.0,
        "semantic_tier_percent must be > 0 when sidecar exists, got {pct}"
    );
    assert!(
        pct <= 100.0,
        "semantic_tier_percent must be <= 100, got {pct}"
    );
}

#[test]
fn semantic_route_falls_back_when_no_match() {
    let fx = build_semantic_fixture();
    let snapshot = Snapshot::open(&fx.store_root, Some(&fx.repo_root)).unwrap();
    let capsule = snap(
        &snapshot,
        "zzzqqqxxxyz nonexistent gibberish",
        4096,
        None,
        false,
    )
    .unwrap();
    assert!(capsule.destinations.is_empty());
    assert!(
        capsule
            .diagnostics
            .notes
            .iter()
            .any(|n| n == "semantic_degraded"),
        "should retain semantic_degraded when no hits"
    );
}

#[test]
fn publish_time_sidecar_exists_after_index() {
    use graphzero_store::store::query::lexical_semantic_file_name;
    let fx = build_semantic_fixture();
    let snapshot = Snapshot::open(&fx.store_root, Some(&fx.repo_root)).unwrap();
    let sidecar = fx
        .store_root
        .join("shards")
        .join(lexical_semantic_file_name(snapshot.entry.snapshot_id));
    assert!(sidecar.is_file(), "GZLX sidecar must exist after index");
}
