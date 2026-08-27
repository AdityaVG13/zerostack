use criterion::{BatchSize, Criterion, black_box};
use tokenzero_core::ContentType;
use tokenzero_recovery::{RecoveryStore, segment_store::SegmentStore};
const TOTAL: usize = 8 * 1024 * 1024;
const CHUNK: usize = 64 * 1024;
fn corpus() -> Vec<String> {
    (0..TOTAL / CHUNK)
        .map(|n| {
            (0..CHUNK)
                .map(|i| char::from(b' ' + ((i + n * 31) % 95) as u8))
                .collect()
        })
        .collect()
}
pub fn segment_store_8mib(c: &mut Criterion) {
    let data = corpus();
    let byte_data: Vec<&[u8]> = data.iter().map(String::as_bytes).collect();
    c.bench_function("legacy_8mib_persist", |b| {
        b.iter_batched(
            || tempfile::tempdir().unwrap(),
            |d| {
                let mut s = RecoveryStore::new(Some(d.path().join("recovery-cache.json")));
                for x in &data {
                    s.store_payload_deferred(x, ContentType::Unknown, None, None, None);
                }
                s.persist_pending().unwrap();
            },
            BatchSize::LargeInput,
        )
    });
    let ld = tempfile::tempdir().unwrap();
    let lp = ld.path().join("recovery-cache.json");
    {
        let mut s = RecoveryStore::new(Some(lp.clone()));
        for x in &data {
            s.store_payload_deferred(x, ContentType::Unknown, None, None, None);
        }
        s.persist_pending().unwrap();
    }
    c.bench_function("legacy_8mib_load", |b| {
        b.iter(|| black_box(RecoveryStore::new(Some(lp.clone()))))
    });
    c.bench_function("segment_8mib_persist", |b| {
        b.iter_batched(
            || tempfile::tempdir().unwrap(),
            |d| {
                let mut s = SegmentStore::create_shadow(d.path().join("recovery-cache.json"), None)
                    .unwrap();
                s.activate().unwrap();
                for (n, x) in byte_data.iter().enumerate() {
                    s.put(&format!("tz://blob/{n:064x}"), x, u64::MAX).unwrap();
                }
            },
            BatchSize::LargeInput,
        )
    });
    let sd = tempfile::tempdir().unwrap();
    let sp = sd.path().join("recovery-cache.json");
    {
        let mut s = SegmentStore::create_shadow(sp.clone(), None).unwrap();
        s.activate().unwrap();
        for (n, x) in byte_data.iter().enumerate() {
            s.put(&format!("tz://blob/{n:064x}"), x, u64::MAX).unwrap();
        }
    }
    c.bench_function("segment_8mib_hot_load", |b| {
        b.iter(|| black_box(SegmentStore::open(sp.clone(), None).unwrap()))
    });
}
