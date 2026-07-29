use stats_alloc::{Region, StatsAlloc, INSTRUMENTED_SYSTEM};
use std::alloc::System;
use zero_abi::raw_worker::EffectClass;
use zero_gate::{decide, DecisionGate, GateInput, GateState};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

#[test]
fn successful_decide_allocates_nothing() {
    let region = Region::new(GLOBAL);
    let (_, gate) = decide(
        GateState::new(8).unwrap(),
        GateInput {
            effect_class: EffectClass::ReadOnly,
            required_budget: 9,
            verified_evidence: None,
            task_receipt: None,
        },
    )
    .unwrap();
    assert!(matches!(gate, DecisionGate::Expand(_)));
    assert_eq!(region.change().allocations, 0);
}
