//! Exact-arithmetic fixtures for the W8 exact Q99 + child-image repair shadow
//! reporter (`zerostack-e7dz`): pass, deny, repair, zero-weight, missing
//! evidence, and provider-hit-only cases, with L1/L2/L3 never aliased.

use zero_abi::{Sha256Digest, sha256};
use zero_gate::{
    ActionGuardOutcome, ActionGuardSimulation, CausalGraphRef, DeclaredAddObject, DeclaredChange,
    DemandMassClass, DemandScenario, ExactObject, ExactRational, PerObjectLayers, PrewarmLedgerRow,
    ProjectImageManifest, ProofGraphRef, ProposedAction, ShadowResourceLedger,
    child_warm_swap_report, compute_demand_coverage, compute_q99_slack, demand_coverage,
    hypothetical_child, layer_ledger_from_manifest, simulate_action_guard,
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
        unknown_reason: reason.map(|s| s.to_owned()),
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
        Sha256Digest::from_bytes(sha256(b"test-root")),
        objects,
        CausalGraphRef {
            digest: None,
            unknown_reason: Some("test".into()),
        },
        ProofGraphRef {
            digest: None,
            unknown_reason: Some("test".into()),
        },
        vec![],
        layers,
        scenarios,
        ShadowResourceLedger {
            rows: vec![],
            unknown_reason: None,
        },
    )
    .expect("fixture manifest should satisfy the project-image contract")
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

#[test]
fn manifest_layer_ledger_preserves_only_valid_l2_state() {
    let valid = d(1);
    let refetch = d(2);
    let invalid = d(3);
    let manifest = manifest(
        vec![obj(1), obj(2), obj(3)],
        vec![
            layer(valid, Some(true), Some(true), Some(true), false, None),
            layer(refetch, Some(true), Some(true), Some(false), true, None),
            layer(invalid, Some(true), Some(false), Some(true), false, None),
        ],
        vec![scenario("layers", &[valid, refetch, invalid], 1)],
    );

    let ledger = layer_ledger_from_manifest(&manifest);
    let valid_entry = ledger.entry(valid).expect("valid L2 entry");
    assert!(valid_entry.l2_valid);
    assert!(!valid_entry.l2_needs_refetch);
    assert!(!valid_entry.l1_valid && !valid_entry.l3_valid);

    let refetch_entry = ledger.entry(refetch).expect("refetch-pending L2 entry");
    assert!(refetch_entry.l2_valid);
    assert!(refetch_entry.l2_needs_refetch);
    assert!(!refetch_entry.l1_valid && !refetch_entry.l3_valid);

    assert!(
        ledger.entry(invalid).is_none(),
        "manifest entries without L2 validity must not be promoted"
    );
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
    // independently calculated boundary: 100% valid => coverage exactly 1
    let expected_coverage = ExactRational::new(1, 1).expect("1/1");
    assert_eq!(
        coverage.coverage.expect("coverage rational"),
        expected_coverage
    );
    assert_eq!(
        coverage.coverage.expect("coverage").to_ppm().expect("ppm"),
        1_000_000
    );
    assert!(!coverage.has_authority());
    assert!(coverage.coverage_unknown_reason.is_none());

    let slack = compute_q99_slack(&m, &m.demand_scenarios).expect("slack");
    // independent slack holds: resident_valid *100 >= demanded*99
    let independent_holds =
        slack.resident_valid_mass as u128 * 100 >= slack.demanded_mass as u128 * 99;
    assert_eq!(slack.slack_holds, independent_holds);
    assert!(slack.slack_holds, "Q99 slack must hold at 100%");
    assert!(slack.unavailable_reason.is_none());
    assert!(!slack.has_authority());

    let sim = simulate_action_guard(&m, &action("noop", vec![], vec![], true)).expect("guard");
    let independent_pass =
        sim.valid_after_mass as u128 * 100 >= sim.next_demanded_mass as u128 * 99;
    assert!(independent_pass, "noop must hold Q99 independently");
    assert_eq!(sim.outcome, ActionGuardOutcome::Pass);
    assert_eq!(sim.shortfall_to_hold_q99, 0);
    assert_eq!(
        sim.shadow_note.as_deref(),
        Some("shadow-only action guard; no production gate enforced")
    );
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
    // missing L2 evidence contributes no valid mass
    assert_eq!(sim.added_mass, 10);
    assert_eq!(
        sim.added_valid_mass, 0,
        "missing evidence addition contributes no valid mass even with L1 hit"
    );
    assert_eq!(sim.valid_after_mass, 90);
    assert_eq!(sim.next_demanded_mass, 100);
    assert!(!sim.obligation_holds, "90 < 0.99*100 must not hold");
    // independent g_min = ceil((100*(B+A_valid)-W_next)/100)
    let expected_g_min = {
        let baseline_plus_valid = sim.baseline_valid_mass as i128; // 90
        let num = 100_i128 * baseline_plus_valid - sim.next_demanded_mass as i128;
        if num > 0 {
            (num as u128).div_ceil(100) as u64
        } else {
            0
        }
    };
    assert_eq!(sim.g_min, expected_g_min);
    assert_eq!(expected_g_min, 89);
    assert_eq!(sim.shortfall_to_hold_q99, 9);
    assert!(sim.repair_restores_q99, "90 + 89 >= 0.99*100");
    assert_eq!(
        sim.outcome,
        ActionGuardOutcome::RepairRequired { g_min: 89 }
    );
    assert!(!sim.has_authority());
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

    let sim = simulate_action_guard(&m, &action("big-invalidation", vec![q1], vec![], true))
        .expect("guard");
    assert_eq!(sim.next_demanded_mass, 90, "10 of 100 invalidated");
    assert_eq!(sim.valid_after_mass, 0, "max(0, 10 - 10)");
    // independent boundary: g_min 10 cannot reach 99% of 90 (needs 89)
    let expected_g_min = 10;
    let expected_shortfall = 90;
    assert_eq!(sim.g_min, expected_g_min);
    assert_eq!(sim.shortfall_to_hold_q99, expected_shortfall);
    assert!(
        !sim.repair_restores_q99,
        "10 mass of repair cannot reach 99% of 90"
    );
    let expected_reason = format!(
        "minimum_repair_insufficient:g_min={expected_g_min},shortfall={expected_shortfall}"
    );
    assert_eq!(
        sim.outcome,
        ActionGuardOutcome::Deny {
            reason: expected_reason
        }
    );
    assert!(!sim.has_authority());
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
    let sim = simulate_action_guard(&m, &action("no-replenish", vec![], vec![add], false))
        .expect("guard");
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
    assert_eq!(
        slack.unavailable_reason.as_deref(),
        Some("zero_weight_envelope")
    );

    let sim =
        simulate_action_guard(&m, &action("zero-action", vec![z1], vec![], true)).expect("guard");
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
        vec![scenario("s1", &[a, b, c, e], 50), scenario("s2", &[a], 30)],
    );

    // demand multiplicity weighted correctly: 50*4 +30 =230
    let coverage = compute_demand_coverage(&m, &m.demand_scenarios).expect("coverage");
    let independent_demanded = 50 * 4 + 30;
    assert_eq!(coverage.demanded_mass, independent_demanded);
    assert_eq!(coverage.demanded_mass, 230);
    // L2 alone determines valid/invalid mass
    assert_eq!(coverage.valid_mass, 180, "a50+b50+e50+a30");
    assert_eq!(coverage.invalid_mass, 50, "c50");
    assert_eq!(coverage.unknown_mass, 0);
    // L1/L3/refetch remain unaliased
    assert_eq!(coverage.l1_hit_mass, 180, "a50+b50+e50+a30");
    assert_eq!(coverage.l3_resident_mass, 180, "a50+c50+e50+a30");
    assert_eq!(coverage.l2_refetch_mass, 50, "e50: valid but L3 copy lost");
    // exact coverage reduces to 18/23
    let expected_rational = ExactRational::new(18, 23).expect("18/23");
    assert_eq!(
        coverage.coverage.expect("coverage rational"),
        expected_rational
    );
    assert_eq!(
        coverage.coverage.expect("coverage").to_ppm().expect("ppm"),
        782_608
    );
    assert!(!coverage.has_authority());
    // resident-valid slack uses documented residency rule: a50+e50+a30 =130
    let slack = compute_q99_slack(&m, &m.demand_scenarios).expect("slack");
    assert_eq!(slack.resident_valid_mass, 130, "a50+e50+a30");
    let independent_holds = 130_u128 * 100 >= 230_u128 * 99;
    assert_eq!(slack.slack_holds, independent_holds);
    assert!(!slack.slack_holds);
    assert!(!slack.has_authority());

    // invalidating b leads to correct repair decision
    let sim = simulate_action_guard(&m, &action("mixed", vec![b], vec![], true)).expect("guard");
    assert_eq!(sim.next_demanded_mass, 180);
    assert_eq!(sim.valid_after_mass, 130);
    // independent g_min and shortfall
    let expected_g_min = 179;
    let expected_shortfall = 49;
    assert_eq!(sim.g_min, expected_g_min);
    assert_eq!(sim.shortfall_to_hold_q99, expected_shortfall);
    assert!(sim.repair_restores_q99);
    assert_eq!(
        sim.outcome,
        ActionGuardOutcome::RepairRequired { g_min: 179 }
    );
    // L2 class for c is Invalid and distinct from L3 residency
    let c_row = coverage
        .rows
        .iter()
        .find(|row| row.object_root == c)
        .expect("row for c");
    assert_eq!(c_row.mass_class, DemandMassClass::Invalid);
    assert_eq!(c_row.l3_resident, Some(true), "L3 residency stays distinct");
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
    let parent_root_before = parent.root;
    let change = DeclaredChange::new(vec![z], vec!["c1".to_owned()]);
    // independently known child identity without using hypothetical_child as oracle
    let expected_child_root = {
        const DOMAIN: &[u8] = b"zerostack.project_image.shadow\0";
        let mut changed_sorted = vec![z];
        changed_sorted.sort();
        let mut claims_sorted = vec!["c1".to_owned()];
        claims_sorted.sort();
        let mut preimage = Vec::new();
        preimage.extend_from_slice(DOMAIN);
        preimage.extend_from_slice(parent_root_before.as_bytes());
        for d in &changed_sorted {
            preimage.extend_from_slice(d.as_bytes());
        }
        for c in &claims_sorted {
            preimage.extend_from_slice(c.as_bytes());
            preimage.push(0);
        }
        Sha256Digest::from_bytes(sha256(&preimage))
    };
    let child_envelope = vec![scenario("warm", &[x, y, z], 100)];
    let prewarm = vec![
        PrewarmLedgerRow {
            child_root: expected_child_root,
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
    let report = child_warm_swap_report(&parent, &change, &child_envelope, prewarm.clone())
        .expect("warm swap");
    assert_eq!(report.child_root, expected_child_root);
    assert_eq!(report.child_manifest.root, expected_child_root);
    assert_eq!(report.preserved_old_root, parent_root_before);
    // determinism across repeated runs
    let report2 =
        child_warm_swap_report(&parent, &change, &child_envelope, prewarm).expect("second");
    assert_eq!(report, report2, "warm swap must be deterministic");
    // parent not mutated
    assert_eq!(parent.root, parent_root_before);
    assert_eq!(
        parent.digest().expect("parent digest after"),
        parent_digest_before
    );
    assert!(!report.has_authority());
    // changed demand without evidence becomes unknown
    assert_eq!(report.coverage.demanded_mass, 300);
    assert_eq!(report.coverage.valid_mass, 200, "x + y stay L2-valid");
    assert_eq!(
        report.coverage.unknown_mass, 100,
        "z is unknown in the child"
    );
    assert_eq!(
        report.coverage.coverage.expect("coverage rational"),
        ExactRational::new(200, 300).expect("2/3")
    );
    // independent Q99 repair requirement
    let expected_repair = {
        let shortfall = 99_i128 * 300 - 100_i128 * 200;
        if shortfall > 0 {
            (shortfall as u128).div_ceil(100) as u64
        } else {
            0
        }
    };
    assert_eq!(report.child_repair_to_hold_q99, expected_repair);
    assert_eq!(report.child_repair_to_hold_q99, 97);
    assert!(!report.warm_swap_holds_q99, "200 < 0.99*300");
    assert_eq!(report.total_prewarm_mass, 80);
    assert_eq!(
        report.unselected_prewarm_mass, 30,
        "unselected work is ledged"
    );
    assert_eq!(report.prewarm_rows.len(), 2);
    assert_eq!(
        report.schema_version,
        "zerostack.project_image.shadow.q99.v1"
    );
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

    // empty action_id
    let err = simulate_action_guard(&m, &action("", vec![], vec![], true)).unwrap_err();
    assert_eq!(
        err,
        zero_gate::project_image::ProjectImageError::InvalidDemand("action_id is empty".into()),
        "empty action_id must be InvalidDemand"
    );

    // zero invalidation digest
    let err = simulate_action_guard(&m, &action("z", vec![Sha256Digest::ZERO], vec![], true))
        .unwrap_err();
    assert_eq!(
        err,
        zero_gate::project_image::ProjectImageError::InvalidDemand(
            "proposed action invalidates the zero digest".into()
        ),
        "zero digest must be InvalidDemand"
    );

    // duplicate added roots
    let err = simulate_action_guard(
        &m,
        &action(
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
        ),
    )
    .unwrap_err();
    assert_eq!(
        err,
        zero_gate::project_image::ProjectImageError::InvalidDemand(format!(
            "proposed action adds {} twice",
            x.to_hex()
        )),
        "duplicate add must be InvalidDemand"
    );

    // duplicate scenario ids at coverage boundary
    let dup_scenarios = manifest(
        vec![obj(1)],
        vec![layer(x, Some(true), Some(true), Some(true), false, None)],
        vec![scenario("dup", &[x], 1), scenario("dup", &[x], 1)],
    );
    let err = compute_demand_coverage(&dup_scenarios, &dup_scenarios.demand_scenarios).unwrap_err();
    assert_eq!(
        err,
        zero_gate::project_image::ProjectImageError::InvalidDemand(
            "duplicate scenario_id dup in envelope".into()
        ),
        "duplicate scenario coverage must fail"
    );
    let err = compute_q99_slack(&dup_scenarios, &dup_scenarios.demand_scenarios).unwrap_err();
    assert_eq!(
        err,
        zero_gate::project_image::ProjectImageError::InvalidDemand(
            "duplicate scenario_id dup in envelope".into()
        ),
        "duplicate scenario slack must fail"
    );

    // zero rational denominator
    let err = ExactRational::new(1, 0).unwrap_err();
    assert_eq!(
        err,
        zero_gate::project_image::ProjectImageError::InvalidValidity(
            "exact rational denominator is zero".into()
        ),
        "zero denominator must be InvalidValidity"
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

    // canonical determinism across semantically equivalent input permutations
    let m_permuted = manifest(
        vec![obj(2), obj(1)],
        vec![
            layer(b, None, Some(false), None, false, None),
            layer(a, Some(true), Some(true), Some(true), false, None),
        ],
        vec![scenario("s1", &[b, a], 40)],
    );
    let first = compute_demand_coverage(&m, &m.demand_scenarios).expect("coverage");
    let permuted = compute_demand_coverage(&m_permuted, &m_permuted.demand_scenarios)
        .expect("permuted coverage");
    assert_eq!(
        first, permuted,
        "coverage must be deterministic across permutations"
    );

    // serialization round-trip determinism for guard
    let guard = simulate_action_guard(&m, &action("a", vec![], vec![], true)).expect("guard");
    let json = serde_json::to_string(&guard).expect("serialize");
    let roundtripped: ActionGuardSimulation = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(
        guard, roundtripped,
        "guard must round-trip deterministically"
    );

    // authority-free
    assert!(!first.has_authority(), "coverage grants no authority");
    let slack = compute_q99_slack(&m, &m.demand_scenarios).expect("slack");
    assert!(!slack.has_authority(), "slack grants no authority");
    assert_eq!(
        guard.shadow_note.as_deref(),
        Some("shadow-only action guard; no production gate enforced"),
        "guard is shadow"
    );

    // absent-root invalidation changes neither invalidated mass nor demanded envelope
    let absent =
        simulate_action_guard(&m, &action("absent", vec![d(9)], vec![], true)).expect("guard");
    assert_eq!(absent.invalidated_mass, 0);
    assert_eq!(absent.next_demanded_mass, absent.current_demanded_mass);
    assert_eq!(
        absent.next_demanded_mass, 80,
        "absent root must not change envelope"
    );
}
