use super::*;
use crate::conformal::ParityScorecard;
use crate::parity_taxonomy::{FeatureUniverse, truncate_score};

fn card(point: f64, lower: f64) -> ParityScorecard {
    ParityScorecard {
        observation_count: 1.0,
        global_point: truncate_score(point),
        global_lower: truncate_score(lower),
        global_upper: 1.0,
        ..ParityScorecard::default()
    }
}

fn covering_waiver(previous: f64, current_lower: f64) -> RatchetWaiver {
    RatchetWaiver {
        id: "WV-TZ-001".into(),
        applies_to_bound_kind: "global_lower".into(),
        old_bound: previous,
        new_bound: current_lower,
        expired: false,
    }
}

#[test]
fn allow_when_new_lower_meets_or_exceeds_previous() {
    let hold = card(0.50, 0.40);
    assert_eq!(apply_ratchet(0.40, &hold), RatchetVerdict::Allow);
    let raised = card(0.55, 0.41);
    assert_eq!(apply_ratchet(0.40, &raised), RatchetVerdict::Allow);
    assert_eq!(RatchetVerdict::Allow.to_string(), "Allow");
}

#[test]
fn block_when_point_exceeds_previous_but_lower_drops() {
    // Drop is inside the 0.005 quarantine band; point still above the old
    // high-water. Point estimate cannot raise (or hold) the bound.
    let sc = card(0.410, 0.396);
    assert!(
        truncate_score(0.400) - sc.global_lower <= truncate_score(CATEGORY_QUARANTINE_THRESHOLD)
    );
    assert!(sc.global_point > truncate_score(0.400));
    assert!(sc.global_lower < truncate_score(0.400));
    assert_eq!(apply_ratchet(0.400, &sc), RatchetVerdict::Block);
    assert_eq!(RatchetVerdict::Block.to_string(), "Block");
}

#[test]
fn quarantine_when_honest_drop_is_within_threshold() {
    let sc = card(0.399, 0.396);
    assert!(sc.global_point <= truncate_score(0.400));
    assert!(
        truncate_score(0.400) - sc.global_lower <= truncate_score(CATEGORY_QUARANTINE_THRESHOLD)
    );
    assert_eq!(apply_ratchet(0.400, &sc), RatchetVerdict::Quarantine);
    assert_eq!(RatchetVerdict::Quarantine.to_string(), "Quarantine");
}

#[test]
fn waiver_covers_honest_downgrade() {
    let sc = card(0.35, 0.30);
    assert_eq!(apply_ratchet(0.40, &sc), RatchetVerdict::Block);
    let w = covering_waiver(0.40, 0.30);
    assert_eq!(
        apply_ratchet_with_waiver(0.40, &sc, Some(&w)),
        RatchetVerdict::Waiver
    );
    assert_eq!(RatchetVerdict::Waiver.to_string(), "Waiver");
}

#[test]
fn expired_or_mismatched_waiver_does_not_cover() {
    let sc = card(0.35, 0.30);
    let mut w = covering_waiver(0.40, 0.30);
    w.expired = true;
    assert_eq!(
        apply_ratchet_with_waiver(0.40, &sc, Some(&w)),
        RatchetVerdict::Block
    );
    w.expired = false;
    w.new_bound = 0.31;
    assert_eq!(
        apply_ratchet_with_waiver(0.40, &sc, Some(&w)),
        RatchetVerdict::Block
    );
}

#[test]
fn truncate_score_on_previous_before_compare() {
    // 0.4000009 truncates toward zero to 0.400000; hold is Allow.
    let sc = card(0.50, 0.400000);
    assert_eq!(apply_ratchet(0.4000009, &sc), RatchetVerdict::Allow);
    let below = card(0.50, 0.399999);
    assert_eq!(apply_ratchet(0.4000009, &below), RatchetVerdict::Block);
}

#[test]
fn no_evidence_and_non_finite_fail_closed() {
    let empty = ParityScorecard::default();
    assert_eq!(apply_ratchet(0.0, &empty), RatchetVerdict::Block);
    let nan_prev = card(0.5, 0.4);
    assert_eq!(apply_ratchet(f64::NAN, &nan_prev), RatchetVerdict::Block);
    let inverted = card(0.3, 0.4);
    assert_eq!(apply_ratchet(0.2, &inverted), RatchetVerdict::Block);
}

#[test]
fn ratchet_state_advances_only_on_allow() {
    let prev = card(0.50, 0.40);
    let mut prev = prev;
    prev.categories.insert(
        "only".into(),
        crate::conformal::CategoryScore {
            category: "only".into(),
            weight: 1.0,
            successes: 1.0,
            failures: 1.0,
            trials: 2.0,
            alpha: 2.0,
            beta: 2.0,
            raw_rate: 0.5,
            point: truncate_score(0.50),
            lower: truncate_score(0.40),
            upper: 1.0,
        },
    );
    let state = RatchetState::from_scorecard(&prev);
    assert_eq!(state.schema_version, RATCHET_STATE_SCHEMA);
    assert_eq!(state.global_lower, truncate_score(0.40));

    let mut raised = card(0.55, 0.41);
    raised.categories = prev.categories.clone();
    raised.categories.get_mut("only").unwrap().lower = truncate_score(0.41);
    raised.categories.get_mut("only").unwrap().point = truncate_score(0.55);
    let (v, next) = state.apply(&raised);
    assert_eq!(v, RatchetVerdict::Allow);
    let next = next.expect("Allow persists");
    assert_eq!(next.global_lower, truncate_score(0.41));
    assert_eq!(next.previous_bound, Some(truncate_score(0.40)));
    assert!(next.global_lower >= state.global_lower);

    let dropped = card(0.35, 0.30);
    let (v, next) = state.apply(&dropped);
    assert_eq!(v, RatchetVerdict::Block);
    assert!(next.is_none());

    let honest = card(0.399, 0.396);
    let w = covering_waiver(0.40, 0.396);
    let (v, next) = state.apply_with_waiver(&honest, Some(&w));
    assert_eq!(v, RatchetVerdict::Waiver);
    assert!(next.is_none(), "waiver must not move the high-water mark");
}

#[test]
fn persisted_point_estimate_cannot_be_the_high_water() {
    let u = FeatureUniverse::load_embedded().expect("embedded matrix");
    let sc = u.conformal_scorecard();
    assert!(sc.global_lower < sc.global_point);
    assert_eq!(apply_ratchet(sc.global_lower, &sc), RatchetVerdict::Allow);
    // Seeding the ratchet with the point estimate is the lie: next apply Blocks.
    assert_eq!(apply_ratchet(sc.global_point, &sc), RatchetVerdict::Block);
    assert!(!crate::conformal::release_pass_on_point_estimate(
        sc.global_point,
        sc.global_lower
    ));
}

#[test]
fn live_universe_seeds_monotone_state() {
    let u = FeatureUniverse::load_embedded().expect("embedded matrix");
    let sc = u.conformal_scorecard();
    let state = RatchetState::from_scorecard(&sc);
    assert_eq!(state.global_lower, sc.global_lower);
    assert_eq!(state.per_category_bounds.len(), sc.categories.len());
    let (v, next) = state.apply(&sc);
    assert_eq!(v, RatchetVerdict::Allow);
    let next = next.expect("hold is Allow");
    assert_eq!(next.global_lower, sc.global_lower);
    for (cat, lo) in &next.per_category_bounds {
        let prev = state.per_category_bounds.get(cat).copied().unwrap();
        assert!(*lo >= prev);
    }
    let json = serde_json::to_string(&state).expect("ratchet json");
    assert!(json.contains(RATCHET_STATE_SCHEMA));
    assert!(json.contains("global_lower"));
    let round: RatchetState = serde_json::from_str(&json).expect("roundtrip");
    assert_eq!(round, state);
    if std::env::var("TOKENZERO_DUMP_RATCHET").as_deref() == Ok("1") {
        eprintln!("{}", serde_json::to_string_pretty(&state).expect("pretty"));
    }
}

#[test]
fn four_outcomes_are_distinct_and_complete() {
    let outcomes = [
        apply_ratchet(0.40, &card(0.50, 0.40)),
        apply_ratchet(0.40, &card(0.41, 0.396)),
        apply_ratchet(0.40, &card(0.399, 0.396)),
        apply_ratchet_with_waiver(0.40, &card(0.35, 0.30), Some(&covering_waiver(0.40, 0.30))),
    ];
    assert_eq!(
        outcomes,
        [
            RatchetVerdict::Allow,
            RatchetVerdict::Block,
            RatchetVerdict::Quarantine,
            RatchetVerdict::Waiver,
        ]
    );
}
