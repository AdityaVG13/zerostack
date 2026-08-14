//! Unit tests for the session Q99/residency gate (V6-R4).
//!
//! Covers: per-tier observations from measured accounting only, labeled
//! denominators, empty-window unavailability, report rejection on missing
//! demand weights, demand dedup, and the eviction slack guard (consulted
//! through [`super::SessionResidencyGate::guard_eviction`]).

use super::{SessionResidencyGate, tier_of_engine};
use zero_abi::raw_worker::{
    EngineIdentity, WorkerTokenAccountingV1, WorkerTokenCountKind,
};
use zero_abi::DigestV1;
use zero_gate::residency::{CacheLayerTierV1, ResidencyErrorV1, Q99_CENTRAL_CHANGE_FRACTION_PPM};

fn digest(byte: u8) -> DigestV1 {
    DigestV1::from_bytes([byte; 32])
}

fn exact_accounting(raw_tokens: u64, cached_tokens: u64) -> WorkerTokenAccountingV1 {
    WorkerTokenAccountingV1 {
        tokenizer_id: "test-tokenizer-v1".into(),
        tokenizer_version_digest: Some("0".repeat(64)),
        count_kind: WorkerTokenCountKind::Exact,
        raw_tokens,
        visible_tokens: raw_tokens.saturating_sub(cached_tokens),
        recovery_tokens: 0,
        billed_tokens: raw_tokens,
        cached_tokens,
        exact_ref_tokens: Some(0),
    }
}

#[test]
fn tier_of_engine_maps_the_three_slots() {
    assert_eq!(tier_of_engine(EngineIdentity::FsZero), CacheLayerTierV1::L1);
    assert_eq!(tier_of_engine(EngineIdentity::GraphZero), CacheLayerTierV1::L2);
    assert_eq!(tier_of_engine(EngineIdentity::TokenZero), CacheLayerTierV1::L3);
}

#[test]
fn central_change_over_one_percent_reports_unavailable_with_labeled_denominator() {
    let mut gate = SessionResidencyGate::new("g1-r1");
    gate.observe_dispatch(CacheLayerTierV1::L3, &exact_accounting(1000, 100))
        .expect("observe");
    let report = gate.report(0).expect("report");
    let tier = report
        .tiers
        .iter()
        .find(|tier| tier.tier == CacheLayerTierV1::L3)
        .expect("l3 tier present");
    assert_eq!(tier.window.demanded_mass, 1000);
    assert_eq!(tier.window.hit_mass, 100);
    assert!(tier.window.recompute_ppm > Q99_CENTRAL_CHANGE_FRACTION_PPM);
    assert_eq!(tier.denominator_label, "q99_demanded_mass:1000");
    assert!(tier.window.unavailable);
    assert!(report.reasons.iter().any(|reason| reason.starts_with("l3:central_change_exceeds_one_percent")));
    assert!(report.unavailable);
    assert_eq!(report.resident_mass, 100);
    assert_eq!(report.demanded_mass, 1000);
    assert_eq!(report.eviction_floor_mass, 990);
    // slack = resident_ppm - 990000ppm = 100000 - 990000 = -890000
    assert_eq!(report.eviction_slack_ppm, Some(-890_000));
}

#[test]
fn empty_window_reports_unavailable_never_vacuous() {
    let gate = SessionResidencyGate::new("g1-r0");
    let report = gate.report(0).expect("report");
    assert!(report.unavailable);
    assert!(report.reasons.iter().any(|reason| reason.ends_with("no_demand_observations")));
    assert_eq!(report.demand_ledger.objects.len(), 0);
    assert_eq!(report.demanded_mass, 0);
    assert_eq!(report.eviction_slack_ppm, None);
    assert_eq!(report.tiers.len(), 3);
}

#[test]
fn zero_hit_dispatch_is_a_full_recompute_not_a_hit() {
    let mut gate = SessionResidencyGate::new("g1-r1");
    gate.observe_dispatch(CacheLayerTierV1::L1, &exact_accounting(64, 0))
        .expect("observe");
    let report = gate.report(0).expect("report");
    let tier = report
        .tiers
        .iter()
        .find(|tier| tier.tier == CacheLayerTierV1::L1)
        .expect("l1 tier present");
    assert_eq!(tier.window.hit_mass, 0);
    assert_eq!(tier.window.recompute_ppm, 1_000_000);
    assert!(tier.window.unavailable);
}

#[test]
fn estimate_accounting_never_feeds_the_window() {
    let mut gate = SessionResidencyGate::new("g1-r1");
    let mut estimate = exact_accounting(1000, 900);
    estimate.count_kind = WorkerTokenCountKind::Estimate;
    gate.observe_dispatch(CacheLayerTierV1::L3, &estimate)
        .expect("observe");
    let report = gate.report(0).expect("report");
    let tier = report
        .tiers
        .iter()
        .find(|tier| tier.tier == CacheLayerTierV1::L3)
        .expect("l3 tier present");
    assert_eq!(tier.window.demanded_mass, 0);
    assert!(tier.window.unavailable);
    assert!(tier
        .window
        .reasons
        .iter()
        .any(|reason| reason == "no_demand_observations"));
}

#[test]
fn report_rejects_when_demand_weight_is_absent() {
    let mut gate = SessionResidencyGate::new("g1-r1");
    // A zero-byte object is demanded but carries no representable nonzero
    // demand weight; the report fails closed instead of omitting it.
    gate.record_demand(digest(1), 0, CacheLayerTierV1::L1)
        .expect("record zero-weight demand as unweighted");
    let error = gate.report(0).expect_err("report must reject");
    match error {
        ResidencyErrorV1::InvalidDemandLedger(detail) => {
            assert!(detail.contains("demand weight absent"), "{detail}");
        }
        other => panic!("unexpected rejection: {other}"),
    }
}

#[test]
fn record_demand_rejects_zero_weight_declaration_at_record_time() {
    // W4 contract: `DemandWeightedObjectV1` itself rejects a zero weight; the
    // gate routes zero-byte objects to the unweighted list first, so a direct
    // zero-weight declaration is impossible.
    let mut gate = SessionResidencyGate::new("g1-r1");
    assert!(gate
        .record_demand(digest(2), 0, CacheLayerTierV1::L2)
        .is_ok());
}

#[test]
fn demand_ledger_deduplicates_objects_across_dispatches_in_one_window() {
    let mut gate = SessionResidencyGate::new("g1-r1");
    let root = digest(7);
    gate.record_demand(root, 30, CacheLayerTierV1::L1).expect("first dispatch");
    gate.record_demand(root, 30, CacheLayerTierV1::L1).expect("second dispatch");
    let report = gate.report(0).expect("report");
    assert_eq!(report.demand_ledger.objects.len(), 1);
    let object = &report.demand_ledger.objects[0];
    assert_eq!(object.object_root, root);
    assert_eq!(object.demand_weight, 60);
    assert_eq!(object.tier, CacheLayerTierV1::L1);
    assert_eq!(object.window_id, "g1-r1");
}

#[test]
fn eviction_slack_guard_is_consulted_and_rejects_below_the_ninety_nine_percent_floor() {
    // Fixture-like cache: 1000 demanded, 995 resident (hit). Evicting 5 keeps
    // resident at exactly the 99% floor; evicting 6 pushes below it.
    let mut gate = SessionResidencyGate::new("g1-r1");
    gate.observe_dispatch(CacheLayerTierV1::L3, &exact_accounting(1000, 995))
        .expect("observe");
    gate.guard_eviction(5).expect("eviction at the floor is allowed");
    match gate.guard_eviction(6) {
        Err(ResidencyErrorV1::SlackExceeded {
            resident_mass,
            demanded_mass,
            slack,
        }) => {
            assert_eq!(resident_mass, 995);
            assert_eq!(demanded_mass, 1000);
            // W4 slack is the pre-eviction headroom above the 99% floor:
            // resident 995000ppm - floor 990000ppm = 5000ppm. The guard
            // rejects because evicting 6 leaves 989 < floor 990.
            assert_eq!(slack, 5_000);
        }
        other => panic!("expected SlackExceeded, got {other:?}"),
    }
}

#[test]
fn eviction_slack_guard_fails_closed_without_demand() {
    let gate = SessionResidencyGate::new("g1-r1");
    match gate.guard_eviction(1) {
        Err(ResidencyErrorV1::InvalidDemandLedger(_)) => {}
        other => panic!("expected InvalidDemandLedger, got {other:?}"),
    }
}
