//! Exact-arithmetic fixtures for the W8 exact Q99 + child-image repair shadow
//! reporter (`zerostack-e7dz`): pass, deny, repair, zero-weight, missing
//! evidence, and provider-hit-only cases, with L1/L2/L3 never aliased.

use zero_abi::{Sha256Digest, sha256};
use zero_gate::{
    ActionGuardOutcome, CausalGraphRef, DeclaredAddObject, DeclaredChange, DemandMassClass,
    DemandScenario, ExactObject, ExactRational, PerObjectLayers, PrewarmLedgerRow,
    ProjectImageManifest, ProofGraphRef, ProposedAction, ShadowResourceLedger,
    child_warm_swap_report, compute_demand_coverage, compute_q99_slack, demand_coverage,
    hypothetical_child, simulate_action_guard,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn d(tag: u8) -> Sha256Digest {
    Sha256Digest::from_bytes(sha256(&[tag]))
}

fn layer(
    root: Sha256Digest,
    l1: Option<bool>,
    l2: Option<bool>,
    l3: Option<bool>,
    refetch: bool,
    reason: Option<&str>,
) -> PerObjectLayers {
    PerObjectLayers {
        object_root: root,
        l1_provider_cached: l1,
        l2_logically_valid: l2,
        l3_physically_resident: l3,
        l2_needs_refetch: refetch,
        unknown_reason: reason.map(str::to_owned),
    }
}

fn scenario(id: &str, roots: &[Sha256Digest], weight: u64) -> DemandScenario {
    DemandScenario {
        scenario_id: id.to_owned(),
        demanded_object_roots: roots.to_vec(),
        demand_weight: weight,
        window_id: None,
        unknown_reason: None,
    }
}

fn obj(tag: u8) -> ExactObject {
    ExactObject::new(d(tag), 16).expect("exact object")
}

fn manifest(
    objects: Vec<ExactObject>,
    layers: Vec<PerObjectLayers>,
    scenarios: Vec<DemandScenario>,
) -> ProjectImageManifest {
    ProjectImageManifest::new(
        d(0xEE),
        objects,
        CausalGraphRef::present(d(0xCA)).expect("causal graph"),
        ProofGraphRef::present(d(0xCF)).expect("proof graph"),
        Vec::new(),
        layers,
        scenarios,
        ShadowResourceLedger::empty(),
    )
    .expect("manifest")
}

fn action(
    id: &str,
    invalidate: Vec<Sha256Digest>,
    add: Vec<DeclaredAddObject>,
    simulate_replenish: bool,
) -> ProposedAction {
    ProposedAction {
        action_id: id.to_owned(),
        invalidate_object_roots: invalidate,
        add_objects: add,
        simulate_replenish,
    }
}

// ---------------------------------------------------------------------------
// Pass: valid mass already covers >= 99/100 of next demand
// ---------------------------------------------------------------------------

#[test]
fn pass_action_holds_q99_exact() {
    let x = d(1);
    let m = manifest(
        vec![obj(1)],
        vec![layer(x, Some(true), Some(true), Some(true), false, None)],
        vec![scenario("pass", &[x], 100)],
    );

    let coverage = demand_coverage(&m).expect("coverage");
    assert_eq!(coverage.demanded_mass, 100);
    assert_eq!(coverage.valid_mass, 100);
    assert_eq!(coverage.invalid_mass, 0);
    assert_eq!(coverage.unknown_mass, 0);
    assert_eq!(coverage.l1_hit_mass, 100, "L1 hit is reported, never aliased");
    assert_eq!(coverage.l3_resident_mass, 100);
    assert_eq!(coverage.l2_refetch_mass, 0);
    let c = coverage.coverage.expect("coverage rational");
    assert_eq!(c, ExactRational::new(100, 100).expect("1/1"), "exact 100/100");
    assert_eq!(c.to_ppm().expect("ppm"), 1_000_000);
    assert_eq!(coverage.denominator_label, "q99_demanded_mass:100");
    assert!(coverage.coverage_unknown_reason.is_none());
    assert!(!coverage.has_authority());

    let slack = compute_q99_slack(&m, &m.demand_scenarios).expect("slack");
    assert_eq!(slack.resident_valid_mass, 100);
    assert_eq!(slack.slack_numerator_100, 100, "100*100 - 99*100");
    assert!(slack.slack_holds);
    assert!(slack.unavailable_reason.is_none());

    let sim = simulate_action_guard(&m, &action("noop", vec![], vec![], true)).expect("guard");
    assert_eq!(sim.current_demanded_mass, 100);
    assert_eq!(sim.next_demanded_mass, 100);
    assert_eq!(sim.baseline_valid_mass, 100);
    assert_eq!(sim.valid_after_mass, 100);
    assert_eq!(sim.obligation_numerator_100, 100);
    assert!(sim.obligation_holds);
    assert_eq!(sim.g_min_numerator_100, 9900, "100*(100+0) - 100");
    assert_eq!(sim.g_min, 99);
    assert!(sim.repair_restores_q99);
    assert_eq!(sim.shortfall_to_hold_q99, 0);
    assert_eq!(sim.outcome, ActionGuardOutcome::Pass);
    assert!(!sim.has_authority());
}

// ---------------------------------------------------------------------------
// Repair: g_min per the W8 design; missing-evidence additions never add
// valid mass
// ---------------------------------------------------------------------------

#[test]
fn repair_required_with_g_min_and_missing_evidence_add() {
    let y = d(2);
    let m = manifest(
        vec![obj(2)],
        vec![layer(y, Some(true), Some(true), Some(true), false, None)],
        vec![scenario("repair", &[y], 90)],
    );

    // Add Z with declared weight 10 but NO L2 evidence: missing evidence must
    // never count as valid mass (provider hits never repair L2).
    let z = d(3);
    let add = DeclaredAddObject {
        object_root: z,
        demand_weight: 10,
        l2_valid: None,
        l1_provider_hit: Some(true),
        unknown_reason: Some("missing evidence for addition".to_owned()),
    };
    let sim = simulate_action_guard(&m, &action("repair-action", vec![], vec![add], true))
        .expect("guard");
    assert_eq!(sim.current_demanded_mass, 90);
    assert_eq!(sim.next_demanded_mass, 100, "W_next = 90 + 10");
    assert_eq!(sim.baseline_valid_mass, 90);
    assert_eq!(sim.added_mass, 10);
    assert_eq!(
        sim.added_valid_mass, 0,
        "missing evidence addition contributes no valid mass"
    );
    assert_eq!(sim.valid_after_mass, 90);
    assert_eq!(sim.obligation_numerator_100, -900, "100*90 - 99*100");
    assert!(!sim.obligation_holds);
    assert_eq!(sim.g_min_numerator_100, 8900, "100*(90+0) - 100");
    assert_eq!(sim.g_min, 89, "ceil(8900/100)");
    assert_eq!(sim.shortfall_to_hold_q99, 9, "ceil(900/100)");
    assert!(sim.repair_restores_q99, "90 + 89 >= 0.99*100");
    assert_eq!(
        sim.outcome,
        ActionGuardOutcome::RepairRequired { g_min: 89 }
    );
}

// ---------------------------------------------------------------------------
// Deny: provider-hit-only demand is never valid mass, so no repair exists
// ---------------------------------------------------------------------------

#[test]
fn deny_provider_hit_only_never_valid() {
    let p = d(4);
    let m = manifest(
        vec![obj(4)],
        vec![layer(
            p,
            Some(true),
            None,
            None,
            false,
            Some("provider hit only; no L2 evidence"),
        )],
        vec![scenario("hit", &[p], 100)],
    );

    let coverage = demand_coverage(&m).expect("coverage");
    assert_eq!(coverage.valid_mass, 0, "L1 hit never becomes valid mass");
    assert_eq!(coverage.unknown_mass, 100);
    assert_eq!(coverage.l1_hit_mass, 100);
    assert_eq!(
        coverage.coverage.expect("coverage rational"),
        ExactRational::new(0, 100).expect("0/1"),
        "exact 0/100 reduced to 0/1"
    );
    assert_eq!(coverage.rows.len(), 1);
    assert_eq!(coverage.rows[0].mass_class, DemandMassClass::Unknown);
    assert_eq!(
        coverage.rows[0].l1_provider_hit,
        Some(true),
        "row keeps L1 distinct"
    );

    let slack = compute_q99_slack(&m, &m.demand_scenarios).expect("slack");
    assert_eq!(slack.resident_valid_mass, 0);
    assert_eq!(slack.slack_numerator_100, -9900);
    assert!(!slack.slack_holds);

    let sim = simulate_action_guard(&m, &action("hit-only", vec![], vec![], true)).expect("guard");
    assert_eq!(sim.baseline_valid_mass, 0);
    assert_eq!(sim.next_demanded_mass, 100);
    assert_eq!(sim.g_min, 0, "no L2 evidence => zero minimum repair");
    assert_eq!(
        sim.outcome,
        ActionGuardOutcome::Deny {
            reason: "minimum_repair_is_zero; action cannot hold Q99".into()
        }
    );
}

// ---------------------------------------------------------------------------
// Deny: repair that cannot restore the obligation fails closed
// ---------------------------------------------------------------------------

#[test]
fn deny_insufficient_repair() {
    let q1 = d(5);
    let q2 = d(6);
    let m = manifest(
        vec![obj(5), obj(6)],
        vec![
            layer(q1, Some(true), Some(true), Some(true), false, None),
            layer(q2, None, Some(false), None, false, None),
        ],
        vec![
            scenario("s-valid", &[q1], 10),
            scenario("s-invalid", &[q2], 90),
        ],
    );

    let sim =
        simulate_action_guard(&m, &action("big-invalidation", vec![q1], vec![], true)).expect("guard");
    assert_eq!(sim.current_demanded_mass, 100);
    assert_eq!(sim.next_demanded_mass, 90, "10 of 100 invalidated");
    assert_eq!(sim.baseline_valid_mass, 10);
    assert_eq!(sim.invalidated_mass, 10);
    assert_eq!(sim.valid_after_mass, 0, "max(0, 10 - 10)");
    assert_eq!(sim.g_min_numerator_100, 910, "100*(10+0) - 90");
    assert_eq!(sim.g_min, 10, "ceil(910/100)");
    assert_eq!(sim.shortfall_to_hold_q99, 90, "ceil(8910/100)");
    assert!(!sim.repair_restores_q99, "10 mass of repair cannot reach 99% of 90");
    assert!(matches!(
        &sim.outcome,
        ActionGuardOutcome::Deny { reason } if reason.starts_with("minimum_repair_insufficient:")
    ));
}

// ---------------------------------------------------------------------------
// Deny: replenish branch not simulated
// ---------------------------------------------------------------------------

#[test]
fn deny_when_replenish_not_simulated() {
    let y = d(2);
    let m = manifest(
        vec![obj(2)],
        vec![layer(y, Some(true), Some(true), Some(true), false, None)],
        vec![scenario("repair", &[y], 90)],
    );
    let z = d(3);
    let add = DeclaredAddObject {
        object_root: z,
        demand_weight: 10,
        l2_valid: None,
        l1_provider_hit: None,
        unknown_reason: Some("missing evidence".to_owned()),
    };
    let sim =
        simulate_action_guard(&m, &action("no-replenish", vec![], vec![add], false)).expect("guard");
    assert_eq!(sim.g_min, 89, "repair exists but is not simulated");
    assert_eq!(
        sim.outcome,
        ActionGuardOutcome::Deny {
            reason: "replenish_not_simulated".into()
        }
    );
}

// ---------------------------------------------------------------------------
// Zero-weight: impossibility reported, never a fake number
// ---------------------------------------------------------------------------

#[test]
fn zero_weight_envelope_unavailable_never_fake() {
    let z1 = d(7);
    let m = manifest(
        vec![obj(7)],
        vec![layer(z1, Some(true), Some(true), Some(true), false, None)],
        vec![scenario("zero", &[z1], 0)],
    );

    let coverage = demand_coverage(&m).expect("coverage");
    assert_eq!(coverage.demanded_mass, 0);
    assert!(coverage.coverage.is_none(), "no fake 0% coverage");
    assert_eq!(
        coverage.coverage_unknown_reason.as_deref(),
        Some("zero_weight_envelope")
    );
    assert_eq!(coverage.rows.len(), 1);
    assert_eq!(coverage.rows[0].demand_weight, 0);
    assert_eq!(coverage.denominator_label, "q99_demanded_mass:0");

    let slack = compute_q99_slack(&m, &m.demand_scenarios).expect("slack");
    assert!(!slack.slack_holds, "no vacuous hold on zero demand");
    assert_eq!(slack.unavailable_reason.as_deref(), Some("zero_weight_envelope"));

    let sim = simulate_action_guard(&m, &action("zero-action", vec![z1], vec![], true)).expect("guard");
    assert_eq!(sim.next_demanded_mass, 0);
    assert_eq!(
        sim.outcome,
        ActionGuardOutcome::Unavailable {
            reason: "zero_weight_next_envelope".into()
        }
    );
}

// ---------------------------------------------------------------------------
// Exact-rational coverage + slack on a mixed envelope, L1/L2/L3 distinct
// ---------------------------------------------------------------------------

#[test]
fn coverage_and_slack_exact_rational_no_aliasing() {
    let a = d(1);
    let b = d(2);
    let c = d(3);
    let e = d(4);
    let m = manifest(
        vec![obj(1), obj(2), obj(3), obj(4)],
        vec![
            layer(a, Some(true), Some(true), Some(true), false, None),
            layer(b, Some(true), Some(true), Some(false), false, None),
            layer(c, None, Some(false), Some(true), false, None),
            layer(e, Some(true), Some(true), Some(true), true, None),
        ],
        vec![
            scenario("s1", &[a, b, c, e], 50),
            scenario("s2", &[a], 30),
        ],
    );

    let coverage = compute_demand_coverage(&m, &m.demand_scenarios).expect("coverage");
    assert_eq!(coverage.demanded_mass, 230, "50*4 + 30");
    assert_eq!(coverage.valid_mass, 180, "a50+b50+e50+a30");
    assert_eq!(coverage.invalid_mass, 50, "c50");
    assert_eq!(coverage.unknown_mass, 0);
    assert_eq!(coverage.l1_hit_mass, 180, "a50+b50+e50+a30");
    assert_eq!(coverage.l3_resident_mass, 180, "a50+c50+e50+a30");
    assert_eq!(coverage.l2_refetch_mass, 50, "e50: valid but L3 copy lost");
    let c_rat = coverage.coverage.expect("coverage rational");
    assert_eq!(c_rat, ExactRational::new(180, 230).expect("18/23"));
    assert_eq!(c_rat.to_ppm().expect("ppm"), 782_608);
    assert_eq!(coverage.denominator_label, "q99_demanded_mass:230");
    assert_eq!(coverage.envelope_scenario_ids, vec!["s1", "s2"]);

    // Rows are sorted by (scenario_id, object_root); mass class is derived
    // from L2 only (never from L1/L3), checked without assuming digest order.
    assert_eq!(coverage.rows.len(), 5);
    assert_eq!(
        coverage
            .rows
            .iter()
            .filter(|row| row.mass_class == DemandMassClass::Valid)
            .count(),
        4,
        "a50, b50, e50, a30 are Valid"
    );
    assert_eq!(
        coverage
            .rows
            .iter()
            .filter(|row| row.mass_class == DemandMassClass::Invalid)
            .count(),
        1,
        "c50 is Invalid"
    );
    let c_row = coverage
        .rows
        .iter()
        .find(|row| row.object_root == c)
        .expect("row for c");
    assert_eq!(c_row.mass_class, DemandMassClass::Invalid);
    assert_eq!(c_row.l3_resident, Some(true), "L3 residency stays distinct");
    let s2_rows: Vec<_> = coverage
        .rows
        .iter()
        .filter(|row| row.scenario_id == "s2")
        .collect();
    assert_eq!(s2_rows.len(), 1);
    assert_eq!(s2_rows[0].demand_weight, 30);
    assert_eq!(s2_rows[0].mass_class, DemandMassClass::Valid);

    let slack = compute_q99_slack(&m, &m.demand_scenarios).expect("slack");
    assert_eq!(slack.demanded_mass, 230);
    assert_eq!(slack.resident_valid_mass, 130, "a50+e50+a30");
    assert_eq!(slack.slack_numerator_100, -9_770, "100*130 - 99*230");
    assert!(!slack.slack_holds);

    // Guard over the same envelope: invalidate b (50) -> repair path.
    let sim = simulate_action_guard(&m, &action("mixed", vec![b], vec![], true)).expect("guard");
    assert_eq!(sim.next_demanded_mass, 180);
    assert_eq!(sim.valid_after_mass, 130);
    assert_eq!(sim.obligation_numerator_100, -4_820, "100*130 - 99*180");
    assert_eq!(sim.g_min_numerator_100, 17_820, "100*(180+0) - 180");
    assert_eq!(sim.g_min, 179, "ceil(17820/100)");
    assert_eq!(sim.shortfall_to_hold_q99, 49, "ceil(4820/100)");
    assert!(sim.repair_restores_q99);
    assert_eq!(sim.outcome, ActionGuardOutcome::RepairRequired { g_min: 179 });
}

// ---------------------------------------------------------------------------
// Warm-swap: deterministic child, preserved old root, prewarm ledger
// ---------------------------------------------------------------------------

#[test]
fn warm_swap_report_deterministic_child_and_prewarm_ledger() {
    let x = d(1);
    let y = d(2);
    let z = d(3);
    let parent = manifest(
        vec![obj(1), obj(2), obj(3)],
        vec![
            layer(x, Some(true), Some(true), Some(true), false, None),
            layer(y, Some(true), Some(true), Some(true), false, None),
            layer(z, None, Some(false), Some(true), false, None),
        ],
        vec![scenario("warm", &[x, y, z], 100)],
    );
    let parent_digest_before = parent.digest().expect("parent digest");

    let change = DeclaredChange::new(vec![z], vec!["c1".to_owned()]);
    let fork = hypothetical_child(&parent, &change).expect("fork");
    assert_ne!(fork.child_root, parent.root, "child root differs from parent");
    assert_eq!(fork.preserved_old_root, parent.root);

    // Child envelope: same demand as the parent (the change did not alter it).
    let child_envelope = vec![scenario("warm", &[x, y, z], 100)];
    let prewarm = vec![
        PrewarmLedgerRow {
            child_root: fork.child_root,
            selected: true,
            declared_work_mass: 50,
            note: Some("warmed selected branch".to_owned()),
        },
        PrewarmLedgerRow {
            child_root: d(9),
            selected: false,
            declared_work_mass: 30,
            note: Some("unselected branch work ledged".to_owned()),
        },
    ];
    let report =
        child_warm_swap_report(&parent, &change, &child_envelope, prewarm).expect("warm swap");

    assert_eq!(report.schema_version, "zerostack.project_image.shadow.q99.v1");
    assert_eq!(report.parent_root, parent.root);
    assert_eq!(report.child_root, fork.child_root);
    assert_eq!(report.preserved_old_root, parent.root);
    assert_eq!(report.child_manifest.root, fork.child_root);
    assert!(!report.has_authority());

    // Child coverage: changed root Z became unknown (missing evidence).
    assert_eq!(report.coverage.demanded_mass, 300);
    assert_eq!(report.coverage.valid_mass, 200, "x + y stay L2-valid");
    assert_eq!(report.coverage.unknown_mass, 100, "z is unknown in the child");
    assert_eq!(
        report.coverage.coverage.expect("coverage rational"),
        ExactRational::new(200, 300).expect("2/3")
    );
    assert_eq!(
        report.slack.slack_numerator_100,
        -9_700,
        "100*200 - 99*300"
    );
    assert!(!report.warm_swap_holds_q99, "200 < 0.99*300");
    assert_eq!(report.child_repair_to_hold_q99, 97, "ceil(9700/100)");

    assert_eq!(report.total_prewarm_mass, 80);
    assert_eq!(report.unselected_prewarm_mass, 30, "unselected work is ledged");
    assert_eq!(report.prewarm_rows.len(), 2);

    // The hypothetical action cannot mutate or publish roots: the parent is
    // byte-identical after the report.
    assert_eq!(parent.digest().expect("parent digest after"), parent_digest_before);
}

// ---------------------------------------------------------------------------
// Warm-swap validation: exactly one selected branch matching the child root
// ---------------------------------------------------------------------------

#[test]
fn warm_swap_rejects_invalid_prewarm_rows() {
    let x = d(1);
    let y = d(2);
    let parent = manifest(
        vec![obj(1), obj(2)],
        vec![
            layer(x, Some(true), Some(true), Some(true), false, None),
            layer(y, Some(true), Some(true), Some(true), false, None),
        ],
        vec![scenario("warm", &[x, y], 100)],
    );
    let change = DeclaredChange::new(vec![y], vec!["c1".to_owned()]);
    let fork = hypothetical_child(&parent, &change).expect("fork");
    let envelope = vec![scenario("warm", &[x, y], 100)];

    // Zero selected rows.
    let err = child_warm_swap_report(
        &parent,
        &change,
        &envelope,
        vec![PrewarmLedgerRow {
            child_root: d(9),
            selected: false,
            declared_work_mass: 1,
            note: None,
        }],
    )
    .expect_err("zero selected rows must fail");
    assert!(err.to_string().contains("exactly one selected"), "{err}");

    // Two selected rows.
    let err = child_warm_swap_report(
        &parent,
        &change,
        &envelope,
        vec![
            PrewarmLedgerRow {
                child_root: fork.child_root,
                selected: true,
                declared_work_mass: 1,
                note: None,
            },
            PrewarmLedgerRow {
                child_root: fork.child_root,
                selected: true,
                declared_work_mass: 1,
                note: None,
            },
        ],
    )
    .expect_err("two selected rows must fail");
    assert!(err.to_string().contains("exactly one selected"), "{err}");

    // Selected branch that is not the child root.
    let err = child_warm_swap_report(
        &parent,
        &change,
        &envelope,
        vec![PrewarmLedgerRow {
            child_root: d(9),
            selected: true,
            declared_work_mass: 1,
            note: None,
        }],
    )
    .expect_err("selected branch must be the child root");
    assert!(err.to_string().contains("is not the child root"), "{err}");
}

// ---------------------------------------------------------------------------
// Input validation and determinism
// ---------------------------------------------------------------------------

#[test]
fn action_guard_rejects_invalid_actions() {
    let x = d(1);
    let m = manifest(
        vec![obj(1)],
        vec![layer(x, Some(true), Some(true), Some(true), false, None)],
        vec![scenario("s", &[x], 100)],
    );

    let empty_id = action("", vec![], vec![], true);
    assert!(
        simulate_action_guard(&m, &empty_id).is_err(),
        "empty action_id must fail"
    );

    let zero_invalidate = action("z", vec![Sha256Digest::ZERO], vec![], true);
    assert!(
        simulate_action_guard(&m, &zero_invalidate).is_err(),
        "zero digest invalidation must fail"
    );

    let dup_add = action(
        "dup",
        vec![],
        vec![
            DeclaredAddObject {
                object_root: x,
                demand_weight: 1,
                l2_valid: Some(true),
                l1_provider_hit: None,
                unknown_reason: None,
            },
            DeclaredAddObject {
                object_root: x,
                demand_weight: 1,
                l2_valid: Some(true),
                l1_provider_hit: None,
                unknown_reason: None,
            },
        ],
        true,
    );
    assert!(
        simulate_action_guard(&m, &dup_add).is_err(),
        "duplicate add must fail"
    );

    let dup_scenarios = manifest(
        vec![obj(1)],
        vec![layer(x, Some(true), Some(true), Some(true), false, None)],
        vec![scenario("dup", &[x], 1), scenario("dup", &[x], 1)],
    );
    assert!(
        compute_demand_coverage(&dup_scenarios, &dup_scenarios.demand_scenarios).is_err(),
        "duplicate scenario ids must fail"
    );
    assert!(
        compute_q99_slack(&dup_scenarios, &dup_scenarios.demand_scenarios).is_err(),
        "duplicate scenario ids must fail"
    );

    assert!(
        ExactRational::new(1, 0).is_err(),
        "zero denominator must fail"
    );
}

#[test]
fn reports_are_deterministic_and_authority_free() {
    let a = d(1);
    let b = d(2);
    let m = manifest(
        vec![obj(1), obj(2)],
        vec![
            layer(a, Some(true), Some(true), Some(true), false, None),
            layer(b, None, Some(false), None, false, None),
        ],
        vec![scenario("s1", &[a, b], 40)],
    );

    let first = compute_demand_coverage(&m, &m.demand_scenarios).expect("coverage");
    let second = compute_demand_coverage(&m, &m.demand_scenarios).expect("coverage again");
    assert_eq!(first, second, "coverage is deterministic");

    let sim1 = simulate_action_guard(&m, &action("a", vec![], vec![], true)).expect("guard");
    let sim2 = simulate_action_guard(&m, &action("a", vec![], vec![], true)).expect("guard again");
    assert_eq!(sim1, sim2, "guard is deterministic");
    assert!(!sim1.has_authority(), "guard grants no authority");
    assert!(
        sim1.shadow_note.as_deref().unwrap_or_default().contains("shadow"),
        "guard is explicitly shadow"
    );

    let slack1 = compute_q99_slack(&m, &m.demand_scenarios).expect("slack");
    let slack2 = compute_q99_slack(&m, &m.demand_scenarios).expect("slack again");
    assert_eq!(slack1, slack2, "slack is deterministic");
    assert!(!slack1.has_authority(), "slack grants no authority");

    // Invalidating an absent root contributes zero mass, exactly.
    let absent = simulate_action_guard(&m, &action("absent", vec![d(9)], vec![], true)).expect("guard");
    assert_eq!(absent.invalidated_mass, 0);
    assert_eq!(absent.next_demanded_mass, absent.current_demanded_mass);
}
