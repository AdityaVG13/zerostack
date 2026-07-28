use criterion::{black_box, criterion_group, criterion_main, Criterion};
use zero_abi::raw_worker::EffectClass;
use zero_gate::{decide, GateInput, GateState};

fn bench_decide(c: &mut Criterion) {
    c.bench_function("zero_gate_decide_expand", |b| b.iter(|| {
        decide(black_box(GateState::new(8).unwrap()), black_box(GateInput {
            effect_class: EffectClass::ReadOnly,
            required_budget: 9,
            verified_evidence: None,
            task_receipt: None,
        })).unwrap()
    }));
}
criterion_group!(benches, bench_decide);
criterion_main!(benches);
