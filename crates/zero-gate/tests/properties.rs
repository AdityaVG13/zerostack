use proptest::prelude::*;
use zero_abi::raw_worker::EffectClass;
use zero_gate::{DecisionGate, GateInput, GateState, check_t10_bound, decide};

fn input(effect_class: EffectClass, required_budget: u128) -> GateInput<'static, 'static> {
    GateInput {
        effect_class,
        required_budget,
        verified_evidence: None,
        task_receipt: None,
    }
}

proptest! {
    #[test]
    fn bounded_sequences_never_panic_or_commit_without_proof(
        b0 in 1u128..=1_000_000,
        demands in proptest::collection::vec(0u128..=1_000_000_000, 0..64),
        irreversible in proptest::collection::vec(any::<bool>(), 0..64),
        q in 0u128..=1000,
    ) {
        let mut state = GateState::new(b0).unwrap();
        let mut high = b0;
        for (index, demand) in demands.into_iter().enumerate() {
            high = high.max(demand);
            let effect = if irreversible.get(index).copied().unwrap_or(false) { EffectClass::Irreversible } else { EffectClass::ReadOnly };
            match decide(state, input(effect, demand)) {
                Ok((next, DecisionGate::Expand(_))) => state = next,
                Ok((_, DecisionGate::RawFallback)) => break,
                Ok((_, DecisionGate::Certified(_) | DecisionGate::TaskVerified(_))) => prop_assert!(false, "unproven compressed gate"),
                Err(_) => break,
            }
        }
        prop_assert!(check_t10_bound(state, high.max(1), q).map(|b| b.holds).unwrap_or(false));
    }
}
