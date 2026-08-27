//! Extraction benchmark suite (NFR-001, NFR-002, G-007).

use criterion::{Criterion, Throughput, criterion_group, criterion_main};

use graphzero_extract::BlobInput;
use graphzero_extract::engine::{extract_batch, extract_tier_a};
use graphzero_extract::queries::QuerySet;

fn make_rust_blob(size_kb: usize) -> Vec<u8> {
    let mut s = String::new();
    while s.len() < size_kb * 1024 {
        let i = s.len() / 80;
        s.push_str(&format!("fn function_{i:06}(x: u64, y: u64) -> u64 {{\n    x.wrapping_add(y).wrapping_mul({i})\n}}\n"));
    }
    s.into_bytes()
}

fn single_blob_100kb(c: &mut Criterion) {
    let qs = QuerySet::new();
    let blob = make_rust_blob(100);
    let mut group = c.benchmark_group("single_blob_100kb");
    group.throughput(Throughput::Bytes(blob.len() as u64));
    group.bench_function("rust", |b| {
        b.iter(|| {
            let input = BlobInput::new(Some("bench.rs"), &blob);
            extract_tier_a(&input, &qs)
        })
    });
    group.finish();
}

fn throughput_1000x50kb(c: &mut Criterion) {
    let qs = QuerySet::new();
    let blob = make_rust_blob(50);
    let inputs: Vec<BlobInput> = (0..1000)
        .map(|_| BlobInput::new(Some("bench.rs"), &blob))
        .collect();

    let mut group = c.benchmark_group("throughput_1000x50kb");
    group.bench_function("rayon_batch", |b| b.iter(|| extract_batch(&inputs, &qs)));
    group.finish();
}

criterion_group!(benches, single_blob_100kb, throughput_1000x50kb);
criterion_main!(benches);
