#![forbid(unsafe_code)]

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use zero_ledger::{Digest, LedgerConfig, ResourceGauge, TokenCharge, TokenizerIdentity};

fn benchmark_charge(c: &mut Criterion) {
    let tokenizer = TokenizerIdentity::new("cl100k_base", Digest([7; 32]));
    let mut gauge = ResourceGauge::new(LedgerConfig::new(tokenizer.clone()));
    let charge = TokenCharge {
        raw_input_tokens: 1_024,
        input_tokens: 256,
        billed_tokens: 256,
        model_calls: 1,
        ..TokenCharge::default()
    };

    c.bench_function("zero_ledger_charge", |b| {
        b.iter(|| {
            gauge
                .charge(black_box(&tokenizer), black_box(&charge))
                .expect("representative charge uses the locked tokenizer");
            black_box(gauge.charge_count())
        });
    });
}

criterion_group!(benches, benchmark_charge);
criterion_main!(benches);
