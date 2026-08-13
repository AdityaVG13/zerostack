//! Conformance suite for the fresh-work accounting vector and eta_action.
//!
//! Normative prose: conformance/contracts/fresh-work-vector-v1.md.

use proptest::prelude::*;
use zero_ledger::{
    ActionFreshWork, Digest, FreshWorkComponent, FreshWorkVector, LedgerConfig, LedgerError,
    PPM_ONE, ResourceGauge, SessionFreshWork, TokenCharge, TokenizerIdentity,
};

fn tokenizer() -> TokenizerIdentity {
    TokenizerIdentity::new("cl100k_base", Digest([7; 32]))
}

fn gauge() -> ResourceGauge {
    ResourceGauge::new(LedgerConfig::new(tokenizer()))
}

fn vector(fresh: u64, replayed: u64, recovery: u64, overhead: u64) -> FreshWorkVector {
    FreshWorkVector::new(fresh, replayed, recovery, overhead).expect("components fit in u64")
}

fn action(id: &str, v: FreshWorkVector) -> ActionFreshWork {
    ActionFreshWork::new(id, v).expect("nonempty action id")
}

// --- component-sum invariant ----------------------------------------------

#[test]
fn components_sum_to_total() {
    let v = vector(40, 100, 10, 50);
    assert_eq!(v.total_tokens(), 200);
    assert_eq!(v.component_sum().unwrap(), v.total_tokens());

    let mut walked = 0u64;
    for component in FreshWorkComponent::ALL {
        walked += v.component_tokens(component);
    }
    assert_eq!(walked, v.total_tokens());
}

#[test]
fn component_set_is_exhaustive_and_distinct() {
    let mut names: Vec<&str> = FreshWorkComponent::ALL
        .iter()
        .map(|c| c.field_name())
        .collect();
    let declared = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(
        names.len(),
        declared,
        "component field names must be unique"
    );
    assert_eq!(
        names,
        vec![
            "fresh_work_tokens",
            "overhead_tokens",
            "recovery_tokens",
            "replayed_tokens"
        ]
    );
}

#[test]
fn overflowing_components_are_a_typed_error() {
    assert_eq!(
        FreshWorkVector::new(u64::MAX, 1, 0, 0),
        Err(LedgerError::CounterOverflow {
            counter: "replayed_tokens"
        })
    );
}

// --- eta bounds ------------------------------------------------------------

#[test]
fn eta_action_is_zero_when_no_work_is_novel() {
    let v = vector(0, 300, 100, 100);
    assert_eq!(v.eta_action_ppm().unwrap().ppm(), 0);
}

#[test]
fn eta_action_is_one_when_all_work_is_novel() {
    let v = FreshWorkVector::all_fresh(512);
    assert_eq!(v.total_tokens(), 512);
    assert_eq!(v.eta_action_ppm().unwrap().ppm(), PPM_ONE);
}

#[test]
fn eta_action_is_undefined_for_an_undeclared_vector() {
    let v = FreshWorkVector::default();
    assert!(!v.is_declared());
    assert!(v.eta_action_ppm().is_none());
}

#[test]
fn eta_action_floors_a_partial_ratio() {
    // 1 fresh token out of 3: 333_333.33... ppm floors to 333_333.
    let v = vector(1, 1, 1, 0);
    assert_eq!(v.eta_action_ppm().unwrap().ppm(), 333_333);
}

proptest! {
    #[test]
    fn eta_action_stays_within_unit_bounds(
        fresh in 0u64..1_000_000,
        replayed in 0u64..1_000_000,
        recovery in 0u64..1_000_000,
        overhead in 0u64..1_000_000,
    ) {
        let v = vector(fresh, replayed, recovery, overhead);
        prop_assert_eq!(v.component_sum().unwrap(), v.total_tokens());
        match v.eta_action_ppm() {
            None => prop_assert_eq!(v.total_tokens(), 0),
            Some(eta) => prop_assert!(eta.ppm() <= PPM_ONE),
        }
    }

    #[test]
    fn eta_action_is_monotone_in_fresh_work(
        fresh in 0u64..500_000,
        extra in 1u64..500_000,
        replayed in 0u64..1_000_000,
    ) {
        // Swapping replayed tokens into fresh work cannot lower eta.
        let base = vector(fresh, replayed + extra, 0, 0);
        let shifted = vector(fresh + extra, replayed, 0, 0);
        prop_assert_eq!(base.total_tokens(), shifted.total_tokens());
        prop_assert!(
            shifted.eta_action_ppm().unwrap().ppm() >= base.eta_action_ppm().unwrap().ppm()
        );
    }
}

// --- serde round trip ------------------------------------------------------

#[test]
fn vector_round_trips_through_json() {
    let v = vector(7, 21, 3, 9);
    let json = serde_json::to_string(&v).unwrap();
    let decoded: FreshWorkVector = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, v);
    assert_eq!(serde_json::to_string(&decoded).unwrap(), json);
    assert!(
        json.contains("\"total_tokens\":40"),
        "unexpected encoding: {json}"
    );
}

#[test]
fn wire_total_that_disagrees_with_the_components_is_rejected() {
    let json = serde_json::to_string(&vector(7, 21, 3, 9)).unwrap();
    let forged = json.replace("\"total_tokens\":40", "\"total_tokens\":400");
    assert_ne!(forged, json);
    let err = serde_json::from_str::<FreshWorkVector>(&forged).unwrap_err();
    assert!(
        err.to_string().contains("fresh-work components sum to 40"),
        "unexpected error: {err}"
    );
}

#[test]
fn action_and_session_records_round_trip() {
    let actions = vec![
        action("read:src/lib.rs", vector(10, 90, 0, 0)),
        action("edit:src/lib.rs", vector(30, 20, 10, 40)),
    ];
    let session = SessionFreshWork::from_actions(actions.iter()).unwrap();

    let action_json = serde_json::to_string(&actions).unwrap();
    let decoded: Vec<ActionFreshWork> = serde_json::from_str(&action_json).unwrap();
    assert_eq!(decoded, actions);

    let session_json = serde_json::to_string(&session).unwrap();
    let decoded_session: SessionFreshWork = serde_json::from_str(&session_json).unwrap();
    assert_eq!(decoded_session, session);
}

#[test]
fn action_record_requires_an_identity() {
    assert_eq!(
        ActionFreshWork::new("", FreshWorkVector::all_fresh(1)),
        Err(LedgerError::EmptyActionId)
    );
    let err = serde_json::from_str::<ActionFreshWork>(
        "{\"action_id\":\"\",\"vector\":{\"fresh_work_tokens\":1,\"replayed_tokens\":0,\"recovery_tokens\":0,\"overhead_tokens\":0,\"total_tokens\":1}}",
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("nonempty action id"),
        "unexpected error: {err}"
    );
}

// --- aggregation across actions --------------------------------------------

#[test]
fn session_aggregate_is_the_component_wise_sum() {
    let actions = [
        action("a", vector(10, 90, 0, 0)),
        action("b", vector(30, 20, 10, 40)),
        action("c", vector(0, 0, 5, 5)),
    ];
    let session = SessionFreshWork::from_actions(actions.iter()).unwrap();

    assert_eq!(session.actions(), 3);
    let agg = session.aggregate();
    assert_eq!(agg.fresh_work_tokens(), 40);
    assert_eq!(agg.replayed_tokens(), 110);
    assert_eq!(agg.recovery_tokens(), 15);
    assert_eq!(agg.overhead_tokens(), 45);
    assert_eq!(agg.total_tokens(), 210);
    assert_eq!(agg.component_sum().unwrap(), agg.total_tokens());
    // 40 / 210 = 190_476.19... ppm
    assert_eq!(session.eta_session_ppm().unwrap().ppm(), 190_476);
}

#[test]
fn session_aggregation_is_order_independent() {
    let a = action("a", vector(10, 90, 0, 0));
    let b = action("b", vector(30, 20, 10, 40));
    let forward = SessionFreshWork::from_actions([&a, &b]).unwrap();
    let reverse = SessionFreshWork::from_actions([&b, &a]).unwrap();
    assert_eq!(forward, reverse);
}

#[test]
fn empty_session_has_no_eta() {
    let session = SessionFreshWork::default();
    assert_eq!(session.actions(), 0);
    assert!(session.eta_session_ppm().is_none());
}

// --- ledger integration ----------------------------------------------------

fn charge_with(input: u64, v: FreshWorkVector) -> TokenCharge {
    TokenCharge {
        raw_input_tokens: input * 4,
        input_tokens: input,
        billed_tokens: input,
        model_calls: 1,
        fresh_work: v,
        ..TokenCharge::default()
    }
}

#[test]
fn charges_accumulate_the_fresh_work_vector_into_the_ledger() {
    let mut gauge = gauge();
    gauge
        .charge(&tokenizer(), &charge_with(100, vector(10, 90, 0, 0)))
        .unwrap();
    gauge
        .charge(&tokenizer(), &charge_with(100, vector(30, 20, 10, 40)))
        .unwrap();

    let ledger = gauge.ledger();
    assert_eq!(ledger.fresh_work_actions, 2);
    assert_eq!(ledger.fresh_work.total_tokens(), 200);
    assert_eq!(ledger.fresh_work.fresh_work_tokens(), 40);
    assert_eq!(ledger.fresh_work.eta_action_ppm().unwrap().ppm(), 200_000);
    assert_eq!(ledger.check_accounting_complete().unwrap(), 200);
}

#[test]
fn an_undeclared_vector_leaves_the_session_accounting_untouched() {
    let mut gauge = gauge();
    gauge
        .charge(&tokenizer(), &charge_with(64, FreshWorkVector::default()))
        .unwrap();

    let ledger = gauge.ledger();
    assert_eq!(ledger.fresh_work_actions, 0);
    assert!(!ledger.fresh_work.is_declared());
    assert!(ledger.fresh_work.eta_action_ppm().is_none());
}

#[test]
fn a_vector_that_does_not_decompose_the_declared_input_is_rejected() {
    let mut gauge = gauge();
    let err = gauge
        .charge(&tokenizer(), &charge_with(100, vector(10, 10, 10, 10)))
        .unwrap_err();
    assert_eq!(
        err,
        LedgerError::FreshWorkTotalMismatch {
            declared: 100,
            decomposed: 40,
        }
    );
    assert_eq!(
        gauge.charge_count(),
        0,
        "rejected charge must not mutate history"
    );
}

#[test]
fn ledger_fresh_work_survives_a_json_round_trip() {
    let mut gauge = gauge();
    gauge
        .charge(&tokenizer(), &charge_with(100, vector(25, 50, 15, 10)))
        .unwrap();
    let json = serde_json::to_string(gauge.ledger()).unwrap();
    let decoded: zero_ledger::TokenLedger = serde_json::from_str(&json).unwrap();
    assert_eq!(&decoded, gauge.ledger());
    assert_eq!(decoded.fresh_work.eta_action_ppm().unwrap().ppm(), 250_000);
}
