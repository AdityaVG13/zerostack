use std::time::Duration;

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use zero_ref::ZeroRefV1;

const PARSE_CASES: &[(&str, &str)] = &[
    (
        "zero_ref_v1_parse_whole_fz",
        "fz://blob/e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    ),
    (
        "zero_ref_v1_parse_byte_span_gz",
        "gz://blob/e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855#B0-4096",
    ),
    (
        "zero_ref_v1_parse_line_span_tz",
        "tz://blob/e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855#L1-128",
    ),
];

fn parse_valid_refs(c: &mut Criterion) {
    for &(id, input) in PARSE_CASES {
        c.bench_function(id, |b| {
            b.iter(|| {
                let parsed = ZeroRefV1::parse(black_box(input))
                    .expect("benchmark inputs must remain valid ZeroRef v1 refs");
                black_box(parsed)
            });
        });
    }
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3))
        .sample_size(50);
    targets = parse_valid_refs
}
criterion_main!(benches);
