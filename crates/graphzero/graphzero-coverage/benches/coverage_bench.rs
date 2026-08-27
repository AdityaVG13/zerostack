use criterion::{Criterion, criterion_group, criterion_main};
use graphzero_coverage::{
    Bitmap, BlobId, CoverageIndex, MockCoverageIndex, QueryResultBuilder, Tier,
};
use graphzero_store::ContentHash;
use std::hint::black_box;

fn bitmap_query_1000(c: &mut Criterion) {
    let mut idx = MockCoverageIndex::new();
    for i in 0..1000 {
        let b = BlobId::new(format!("blob_{:04}", i));
        let mut bm = Bitmap::new();
        bm.mark_indexed(&b, Tier::A);
        idx.write_coverage(b, bm).unwrap();
    }

    c.bench_function("bitmap_query_1000", |bench| {
        bench.iter(|| {
            let mut count = 0usize;
            for blob in idx.all_blob_ids() {
                if let Some(bm) = idx.read_coverage(&blob)
                    && bm.is_indexed(&blob, Tier::A)
                {
                    count += 1;
                }
            }
            black_box(count);
        });
    });
}

fn certificate_gen_1000(c: &mut Criterion) {
    let mut idx = MockCoverageIndex::new();
    for i in 0..1000 {
        let b = BlobId::new(format!("blob_{:04}", i));
        let mut bm = Bitmap::new();
        bm.mark_indexed(&b, Tier::A);
        idx.write_coverage(b.clone(), bm).unwrap();
        idx.write_freshness(b, ContentHash::from_bytes([0u8; 32]))
            .unwrap();
    }

    struct EmptyProvider;
    impl graphzero_coverage::LiveBytesProvider for EmptyProvider {}

    c.bench_function("certificate_gen_1000", |bench| {
        bench.iter(|| {
            let builder = QueryResultBuilder::new(&idx, Tier::A).not_found();
            let result = builder.build(&EmptyProvider);
            black_box(result);
        });
    });
}

fn bitmap_overhead(c: &mut Criterion) {
    let total_blobs = 10000usize;
    let mut bm = Bitmap::with_capacity(total_blobs);
    for i in 0..total_blobs {
        let b = BlobId::new(format!("blob_{:05}", i));
        bm.mark_indexed(&b, Tier::A);
    }

    // Rough estimate: 10k blobs * 3 u64 = 240k bytes
    let bitmap_bytes = std::mem::size_of::<u64>() * 3 * total_blobs;
    // simulate a 24MB index (1000x the bitmap)
    let total_index_bytes = bitmap_bytes * 1000;
    let overhead = bitmap_bytes as f64 / total_index_bytes as f64;
    assert!(overhead < 0.01, "bitmap overhead {} >= 1%", overhead);

    c.bench_function("bitmap_overhead", |bench| {
        bench.iter(|| {
            let pct = bm.coverage_pct(Tier::A);
            black_box(pct);
        });
    });
}

fn freshness_check_cold(c: &mut Criterion) {
    let bytes = vec![0u8; 1024 * 1024]; // 1 MB blob
    let hash = graphzero_coverage::freshness::compute_hash(&bytes);

    c.bench_function("freshness_check_cold", |bench| {
        bench.iter(|| {
            let ok = graphzero_coverage::freshness_check(Some(&hash), &bytes).unwrap();
            black_box(ok);
        });
    });
}

criterion_group!(
    benches,
    bitmap_query_1000,
    certificate_gen_1000,
    bitmap_overhead,
    freshness_check_cold
);
criterion_main!(benches);
