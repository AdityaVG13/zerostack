//! P5.5 install latency smoke bench (NFR-001 walking skeleton).

use std::hint::black_box;
use std::time::Instant;

use criterion::{Criterion, criterion_group, criterion_main};
use graphzero_pack::{PackSignKey, build_fixture_pack, install_pack};
use tempfile::tempdir;

fn bench_install(c: &mut Criterion) {
    c.bench_function("pack_install_fixture", |b| {
        b.iter(|| {
            let dir = tempdir().unwrap();
            let pack_dir = dir.path().join("pack");
            let store = dir.path().join("store");
            std::fs::create_dir_all(&store).unwrap();
            let key = PackSignKey::fixture();
            let manifest_path = build_fixture_pack(&pack_dir, &key).unwrap();
            let start = Instant::now();
            install_pack(&store, &manifest_path, &key.public()).unwrap();
            black_box(start.elapsed());
        });
    });
}

criterion_group!(benches, bench_install);
criterion_main!(benches);
