//! Adjudicated corpus for the K0 Snap-to-File gate (`zerostack-xbg3`).
//!
//! Every acceptance criterion is measured here:
//! - safe one-expansion path with no model-visible discovery (the expand
//!   ledger contains only the exact known classes; every lookup is ledged
//!   `backend_work`);
//! - S0 exact object/file root: a single-atom projection is returned
//!   root-exact and flagged primary-file orientation, never sold as the
//!   complete multi-file demand (the completeness claim stays bound to the
//!   certified S3 envelope);
//! - Unknown coverage escapes to the frozen native baseline with the
//!   strategy preserved (request/scope/index roots stay in the packet);
//! - Unsafe demands refuse with typed reasons and no guessed subset;
//! - zero false-complete by construction (a completeness claim exists only
//!   on `Snapped`, and only for the certified envelope; the full closure
//!   covers the adjudicated ground truth);
//! - read-only authority (no edit/transaction/commit/write field exists on
//!   the packet or the permit wire form);
//! - the same packet is byte-stable through two harness adapters (route
//!   construction and wire round-trip) and across two route instances;
//! - the decision view certifies Proved only with every evidence class the
//!   route holds, and degrades to Unknown otherwise.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use zero_abi::{CompletenessGrade, LiveExpandState, SafetyVerdict, Sha256Digest, sha256};
use zero_gate::project_image::{
    CausalGraphRef, DemandScenario, ExactObject, PerObjectLayers, ProofGraphRef,
    ProjectImageManifest, ShadowResourceLedger,
};
use zero_gate::{
    DemandRequest, IncrementalDeltaRequest, NativeBaseline, ProtectedScope, SnapMetrics,
    SnapOutcome, SnapOutcomeKind, SnapPacket, SnapToFileRoute, adjudicate,
};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn digest(seed: u8) -> Sha256Digest {
    Sha256Digest::from_bytes(sha256(&[seed; 32]))
}

fn sorted(atoms: &[Sha256Digest]) -> Vec<Sha256Digest> {
    let mut v = atoms.to_vec();
    v.sort();
    v.dedup();
    v
}

fn obj(seed: u8, byte_len: u64) -> ExactObject {
    ExactObject::new(digest(seed), byte_len).unwrap()
}

fn layer(atom: Sha256Digest, l2: Option<bool>, l3: Option<bool>) -> PerObjectLayers {
    PerObjectLayers {
        object_root: atom,
        l1_provider_cached: Some(true),
        l2_logically_valid: l2,
        l3_physically_resident: l3,
        l2_needs_refetch: false,
        unknown_reason: if l2.is_none() && l3.is_none() {
            Some("unknown layers".into())
        } else {
            None
        },
    }
}

fn scenario(id: &str, atoms: &[Sha256Digest], weight: u64) -> DemandScenario {
    DemandScenario {
        scenario_id: id.to_owned(),
        demanded_object_roots: atoms.to_vec(),
        demand_weight: weight,
        window_id: None,
        unknown_reason: None,
    }
}

fn manifest(
    objects: Vec<ExactObject>,
    layers: Vec<PerObjectLayers>,
    scenarios: Vec<DemandScenario>,
) -> ProjectImageManifest {
    ProjectImageManifest::new(
        digest(0x7f),
        objects,
        CausalGraphRef::present(digest(0x21)).unwrap(),
        ProofGraphRef::present(digest(0x22)).unwrap(),
        vec![],
        layers,
        scenarios,
        ShadowResourceLedger::empty(),
    )
    .unwrap()
}

fn request(id: &str, projection: &[Sha256Digest]) -> DemandRequest {
    DemandRequest::new(id.to_owned(), projection.to_vec()).unwrap()
}

fn scope(atoms: &[Sha256Digest]) -> ProtectedScope {
    ProtectedScope::new("test-scope".to_owned(), atoms.to_vec()).unwrap()
}

fn universe(pairs: &[(Sha256Digest, Option<bool>)]) -> Vec<zero_gate::CoverageAtom> {
    let mut v: Vec<zero_gate::CoverageAtom> = pairs
        .iter()
        .map(|(atom, covered)| zero_gate::CoverageAtom {
            atom_root: *atom,
            covered: *covered,
        })
        .collect();
    v.sort_by_key(|atom| atom.atom_root);
    v
}

fn input(
    index_version: &str,
    pairs: &[(Sha256Digest, Option<bool>)],
    attempt_count: u64,
) -> zero_gate::GraphZeroCompletenessInput {
    zero_gate::GraphZeroCompletenessInput::new(
        digest(0x31),
        index_version.to_owned(),
        "task-main".to_owned(),
        universe(pairs),
        attempt_count,
    )
    .unwrap()
}

fn route() -> SnapToFileRoute {
    SnapToFileRoute::new(
        [0x42; 32],
        "tenant-a".to_owned(),
        7,
        digest(0x31),
        "index-v1".to_owned(),
    )
    .unwrap()
}

fn live_for(route: &SnapToFileRoute, handle: &zero_abi::SafeExpandHandle) -> LiveExpandState {
    route
        .current_live_state(handle, SafetyVerdict::Safe, false)
        .unwrap()
}

fn assert_reason_contains(packet: &SnapPacket, needle: &str) {
    let all = &packet.reasons;
    assert!(
        all.iter().any(|reason| reason.contains(needle)),
        "packet reasons {all:?} must contain {needle:?}"
    );
}

fn assert_snapped(outcome: SnapOutcome) -> (SnapPacket, zero_abi::DecisionView, zero_gate::FirstExpansion, zero_abi::SafeExpandHandle) {
    match outcome {
        SnapOutcome::Snapped {
            packet,
            view,
            expansion,
            handle,
        } => (packet, view, expansion, handle),
        other => panic!("expected Snapped, got {:?}", other.outcome_kind()),
    }
}

fn assert_escaped(outcome: SnapOutcome) -> (SnapPacket, zero_abi::DecisionView) {
    match outcome {
        SnapOutcome::Escaped { packet, view } => (packet, view),
        other => panic!("expected Escaped, got {:?}", other.outcome_kind()),
    }
}

fn assert_refused(outcome: SnapOutcome) -> (SnapPacket, zero_abi::DecisionView) {
    match outcome {
        SnapOutcome::Refused { packet, view } => (packet, view),
        other => panic!("expected Refused, got {:?}", other.outcome_kind()),
    }
}

/// The exact ledger class set of one first expansion: the route performs no
/// model-visible discovery, so no `ls`/grep/probe row can exist.
const FIRST_EXPANSION_LEDGER_CLASSES: [&str; 9] = [
    "visible_bytes",
    "backend_work",
    "retry_count",
    "first_try_sufficiency",
    "false_complete",
    "certified_atoms",
    "expanded_atoms",
    "native_baseline_bytes",
    "native_baseline_probes",
];

fn assert_no_discovery_rows(expansion: &zero_gate::FirstExpansion) {
    let classes: BTreeSet<&str> = expansion
        .ledger
        .rows
        .iter()
        .map(|row| row.class.as_str())
        .collect();
    let expected: BTreeSet<&str> = FIRST_EXPANSION_LEDGER_CLASSES.into_iter().collect();
    assert_eq!(
        classes, expected,
        "the expand ledger must contain exactly the known classes"
    );
}

// ---------------------------------------------------------------------------
// Safe path: one expansion, no discovery, exact evidence, native comparison
// ---------------------------------------------------------------------------

#[test]
fn corpus_safe_multi_file_snap_single_expansion_no_discovery() {
    let a = digest(1);
    let b = digest(2);
    let c = digest(3);
    let all = [a, b, c];
    let m = manifest(
        vec![obj(1, 100), obj(2, 200), obj(3, 300)],
        vec![
            layer(a, Some(true), Some(true)),
            layer(b, Some(true), Some(true)),
            layer(c, Some(true), Some(true)),
        ],
        vec![scenario("task-main", &all, 42)],
    );
    let req = request("task-main", &all);
    let scp = scope(&[]);
    let inp = input(
        "index-v1",
        &[(a, Some(true)), (b, Some(true)), (c, Some(true))],
        1,
    );
    let native = NativeBaseline::new(3000, 9);
    let mut gate = route();
    let (packet, view, expansion, _handle) =
        assert_snapped(gate.snap(&m, &req, &scp, &inp, &native).unwrap());

    // One exact expansion, root/projection exact, no model-visible
    // discovery: the ledger carries only the known classes.
    let returned: Vec<Sha256Digest> = expansion.atoms.iter().map(|atom| atom.atom_root).collect();
    assert_eq!(returned, sorted(&all));
    assert_eq!(expansion.projection_root, expansion.permit.projection_root());
    assert_eq!(expansion.projection_root, zero_gate::projection_root_of(&returned));
    assert!(expansion.first_try_sufficiency);
    assert!(expansion.session.terminal());
    assert_no_discovery_rows(&expansion);

    // Adapter-stable packet: Snapped with the full certified claim.
    assert_eq!(packet.outcome, SnapOutcomeKind::Snapped);
    assert_eq!(packet.family, "exact_scenario_closure");
    assert_eq!(packet.proved_levels, zero_gate::snap_to_file::proved_levels());
    assert_eq!(packet.proved_levels, vec!["s0", "s3"]);
    assert_eq!(
        packet
            .unproved_levels
            .iter()
            .map(|level| level.level.as_str())
            .collect::<Vec<_>>(),
        vec!["s1", "s2", "s4"]
    );
    assert_eq!(packet.plan_root.as_deref(), Some(expansion.plan.plan_root.to_hex().as_str()));
    assert_eq!(packet.projection_root.as_deref(), Some(expansion.plan.projection_root.to_hex().as_str()));
    assert!(packet.certificate_root.is_some());
    assert_eq!(packet.checker_identity.as_deref(), Some("zerostack.w9e.completeness.total"));
    assert_eq!(packet.checker_version.as_deref(), Some("1.0.0"));
    assert!(packet.handle_id.is_some());
    assert_eq!(packet.evidence_refs, vec![packet.certificate_root.clone().unwrap()]);
    assert!(!packet.baseline_escape);
    assert!(!packet.primary_file_orientation);
    assert!(packet.reasons.is_empty());
    assert_eq!(
        packet.obligations,
        vec![
            "verify_projection_atoms",
            "no_edit_authority",
            "revalidate_before_continuation",
        ]
    );
    assert_eq!(packet.atoms.len(), 3);
    assert!(packet.atoms.contains(&zero_gate::snap_to_file::PacketAtom {
        atom_root: a.to_hex(),
        byte_len: 100,
    }));
    packet.validate().unwrap();
    packet.verify_root(&packet.packet_root()).unwrap();

    // Adjudicated native-comparison metrics: exact visible bytes, ledged
    // backend work, zero retries, native baseline comparison fields.
    let metrics = packet.metrics.unwrap();
    assert_eq!(
        metrics,
        SnapMetrics {
            visible_bytes: 600,
            backend_work: 20,
            retry_count: 0,
            first_try_sufficiency: true,
            false_complete: false,
            certified_atoms: 3,
            expanded_atoms: 3,
            native_baseline_bytes: 3000,
            native_baseline_probes: 9,
            native_savings_bytes: 2400,
        }
    );
    assert_eq!(expansion.ledger.total("visible_bytes"), 600);
    assert_eq!(expansion.ledger.total("backend_work"), 20);
    assert_eq!(expansion.ledger.total("retry_count"), 0);

    // Decision view: Proved, bound to the certificate and the handle, with
    // the packet carrying the view root.
    assert_eq!(view.completeness_grade(), CompletenessGrade::Proved);
    assert_eq!(view.supported_decisions(), &["snap_to_file".to_owned()]);
    assert_eq!(view.project_root(), digest(0x7f).to_hex().as_str());
    assert_eq!(view.causal_lens_root(), digest(0x31).to_hex().as_str());
    assert_eq!(
        view.task_contract_root(),
        expansion.plan.plan_root.to_hex().as_str()
    );
    assert_eq!(view.evidence_refs(), &[packet.certificate_root.clone().unwrap()]);
    assert_eq!(view.expansion_handles(), &[packet.handle_id.clone().unwrap()]);
    assert!(!view.baseline_escape());
    assert!(view.unresolved_question().is_none());
    assert!(view.omitted_classes().is_empty());
    assert_eq!(packet.decision_view_root, view.root());
    view.verify_root(&packet.decision_view_root).unwrap();
    let present: BTreeSet<String> = zero_gate::snap_to_file::snap_evidence_classes();
    assert_eq!(
        view.certificate(&zero_gate::snap_to_file::snap_evidence_classes(), &present).unwrap(),
        CompletenessGrade::Proved
    );

    // Zero false-complete against the adjudicated ground truth.
    let ground_truth: BTreeSet<Sha256Digest> = [a, b, c].into_iter().collect();
    let adjudicated = adjudicate(&expansion, &ground_truth);
    assert!(!adjudicated.false_complete);
    assert!(adjudicated.first_try_sufficiency);
    assert_eq!(adjudicated.native_savings_bytes, 2400);
}

// ---------------------------------------------------------------------------
// S0: exact object/file root, never sold as the complete multi-file demand
// ---------------------------------------------------------------------------

#[test]
fn corpus_s0_exact_file_root_not_sold_as_multi_file() {
    let a = digest(1);
    let b = digest(2);
    let c = digest(3);
    let all = [a, b, c];
    let m = manifest(
        vec![obj(1, 100), obj(2, 200), obj(3, 300)],
        vec![
            layer(a, Some(true), Some(true)),
            layer(b, Some(true), Some(true)),
            layer(c, Some(true), Some(true)),
        ],
        vec![scenario("task-main", &all, 42)],
    );
    // The request already names an exact object/file root: projection is
    // the single atom `a`.
    let req = request("task-main", &[a]);
    let scp = scope(&[]);
    let inp = input(
        "index-v1",
        &[(a, Some(true)), (b, Some(true)), (c, Some(true))],
        1,
    );
    let native = NativeBaseline::new(3000, 9);
    let mut gate = route();
    let (packet, view, expansion, handle) =
        assert_snapped(gate.snap(&m, &req, &scp, &inp, &native).unwrap());

    // S0 exact: the one returned atom is exactly the requested root, with
    // its exact byte length.
    assert_eq!(expansion.atoms.len(), 1);
    assert_eq!(expansion.atoms[0].atom_root, a);
    assert_eq!(expansion.atoms[0].byte_len, 100);
    assert_eq!(expansion.visible_bytes, 100);
    assert!(expansion.session.delta_seq() == 0);
    assert!(!expansion.session.terminal());

    // The packet flags primary-file orientation and carries the
    // orientation-only obligation: the snap is never sold as the complete
    // multi-file demand.
    assert!(packet.primary_file_orientation);
    assert!(packet
        .obligations
        .contains(&"primary_file_orientation_only".to_owned()));
    assert!(packet
        .obligations
        .contains(&"expand_remaining_closure".to_owned()));
    let metrics = packet.metrics.unwrap();
    assert_eq!(metrics.expanded_atoms, 1);
    assert_eq!(metrics.certified_atoms, 3);
    assert_eq!(metrics.visible_bytes, 100);
    // The completeness claim is the certified envelope, not the projection:
    // plan root and certificate root are carried, and the packet itself
    // proves 1-of-3 expanded with the remaining-closure obligation.
    assert!(packet.plan_root.is_some());
    assert!(packet.certificate_root.is_some());
    assert_eq!(packet.proved_levels, vec!["s0", "s3"]);
    packet.validate().unwrap();

    // The decision view is Proved for the certified envelope (S3), with the
    // handle bound for continuation.
    assert_eq!(view.completeness_grade(), CompletenessGrade::Proved);
    let present: BTreeSet<String> = zero_gate::snap_to_file::snap_evidence_classes();
    assert_eq!(
        view.certificate(&zero_gate::snap_to_file::snap_evidence_classes(), &present).unwrap(),
        CompletenessGrade::Proved
    );

    // Zero false-complete: the route's claim is the certified envelope, and
    // the full closure (first expansion + continuation deltas) covers the
    // adjudicated ground truth exactly.
    let ground_truth: BTreeSet<Sha256Digest> = [a, b, c].into_iter().collect();
    let mut expanded: BTreeSet<Sha256Digest> =
        expansion.atoms.iter().map(|atom| atom.atom_root).collect();
    let live = live_for(&gate, &handle);
    let delta = gate
        .expand_delta(
            &expansion.session,
            &IncrementalDeltaRequest::new(vec![b, c]).unwrap(),
            &live,
        )
        .unwrap();
    for atom in &delta.atoms {
        expanded.insert(atom.atom_root);
    }
    assert!(delta.terminal);
    assert!(ground_truth.is_subset(&expanded), "closure must cover ground truth");
    // Nothing in the packet claims the 1-atom view is the full closure:
    // the expanded/certified split plus the obligations make it explicit.
    assert!(metrics.expanded_atoms < metrics.certified_atoms);
}

// ---------------------------------------------------------------------------
// Unknown -> native escape; Unsafe -> refusal; both with zero false-complete
// ---------------------------------------------------------------------------

#[test]
fn corpus_unknown_coverage_escapes_to_native_preserving_strategy() {
    let a = digest(1);
    let b = digest(2);
    let c = digest(3);
    let all = [a, b, c];
    let m = manifest(
        vec![obj(1, 100), obj(2, 200), obj(3, 300)],
        vec![
            layer(a, Some(true), Some(true)),
            layer(b, Some(true), Some(true)),
            layer(c, Some(true), Some(true)),
        ],
        vec![scenario("task-main", &all, 1)],
    );
    let req = request("task-main", &all);
    let scp = scope(&[]);
    // Coverage of `b` is unknown: the checker evaluated it but cannot
    // establish coverage.
    let inp = input(
        "index-v1",
        &[(a, Some(true)), (b, None), (c, Some(true))],
        1,
    );
    let native = NativeBaseline::new(3000, 9);
    let mut gate = route();
    let (packet, view) = assert_escaped(gate.snap(&m, &req, &scp, &inp, &native).unwrap());

    // Escape: the native baseline is open and nothing was issued.
    assert_eq!(packet.outcome, SnapOutcomeKind::Escaped);
    assert!(packet.baseline_escape);
    assert_reason_contains(&packet, "coverage_unknown:");
    assert!(packet.handle_id.is_none());
    assert!(packet.certificate_root.is_none());
    assert!(packet.plan_root.is_none());
    assert!(packet.projection_root.is_none());
    assert!(packet.atoms.is_empty());
    assert!(packet.metrics.is_none());
    assert!(packet.proved_levels.is_empty());
    assert!(packet.evidence_refs.is_empty());
    assert_eq!(
        packet.obligations,
        vec!["native_escape", "no_completeness_claim"]
    );
    assert!(!packet.primary_file_orientation);
    packet.validate().unwrap();

    // Strategy preserved: the request, scope, and index bindings stay in
    // the packet, so the native run proceeds on the same demand.
    assert_eq!(packet.request_root, req.request_root.to_hex());
    assert_eq!(packet.project_root, m.root.to_hex());
    assert_eq!(packet.scope_root, scp.scope_root.to_hex());
    assert_eq!(packet.index_root, digest(0x31).to_hex());
    assert_eq!(packet.index_version, "index-v1");

    // Decision view: claimed grade Unknown, and the certificate degrades to
    // Unknown because the coverage class is missing -- never a guessed
    // subset labeled complete.
    assert_eq!(view.completeness_grade(), CompletenessGrade::Unknown);
    assert!(view.baseline_escape());
    assert!(view.evidence_refs().is_empty());
    assert!(view.expansion_handles().is_empty());
    let mut present = zero_gate::snap_to_file::snap_evidence_classes();
    present.remove("coverage");
    assert_eq!(
        view.certificate(&zero_gate::snap_to_file::snap_evidence_classes(), &present).unwrap(),
        CompletenessGrade::Unknown
    );
}

#[test]
fn corpus_unsafe_demand_refuses_without_guessed_subset() {
    let a = digest(1);
    let b = digest(2);
    let c = digest(3);
    let all = [a, b, c];
    let m = manifest(
        vec![obj(1, 100), obj(2, 200), obj(3, 300)],
        vec![
            layer(a, Some(true), Some(true)),
            layer(b, Some(true), Some(true)),
            layer(c, Some(true), Some(true)),
        ],
        vec![scenario("task-main", &all, 1)],
    );
    let req = request("task-main", &all);
    // `b` is protected: the demand must refuse.
    let scp = scope(&[b]);
    let inp = input(
        "index-v1",
        &[(a, Some(true)), (b, Some(true)), (c, Some(true))],
        1,
    );
    let native = NativeBaseline::new(3000, 9);
    let mut gate = route();
    let (packet, view) = assert_refused(gate.snap(&m, &req, &scp, &inp, &native).unwrap());

    assert_eq!(packet.outcome, SnapOutcomeKind::Refused);
    assert!(!packet.baseline_escape);
    assert_reason_contains(&packet, "protected_atom_demanded:");
    assert!(packet.handle_id.is_none());
    assert!(packet.certificate_root.is_none());
    assert!(packet.atoms.is_empty());
    assert!(packet.metrics.is_none());
    assert!(packet.proved_levels.is_empty());
    assert_eq!(
        packet.obligations,
        vec!["demand_refused", "no_completeness_claim"]
    );
    packet.validate().unwrap();
    assert_eq!(view.completeness_grade(), CompletenessGrade::Unknown);
    assert!(!view.baseline_escape());
}

#[test]
fn corpus_under_declared_envelope_refuses_false_complete() {
    // Adjudicated ground truth {A, B, C}; the scenario declares only {A, B}
    // while the published coverage positively establishes C. Selling {A, B}
    // as complete would be false-complete, so the gate refuses.
    let a = digest(1);
    let b = digest(2);
    let c = digest(3);
    let m = manifest(
        vec![obj(1, 100), obj(2, 200), obj(3, 300)],
        vec![
            layer(a, Some(true), Some(true)),
            layer(b, Some(true), Some(true)),
            layer(c, Some(true), Some(true)),
        ],
        vec![scenario("task-main", &[a, b], 5)],
    );
    let req = request("task-main", &[a, b]);
    let scp = scope(&[]);
    let inp = input(
        "index-v1",
        &[(a, Some(true)), (b, Some(true)), (c, Some(true))],
        1,
    );
    let native = NativeBaseline::new(3000, 9);
    let mut gate = route();
    let (packet, _view) = assert_refused(gate.snap(&m, &req, &scp, &inp, &native).unwrap());

    assert_reason_contains(&packet, "coverage_exceeds_demand:");
    assert!(packet.atoms.is_empty());
    assert!(packet.handle_id.is_none());
    packet.validate().unwrap();
}

#[test]
fn corpus_projection_atom_missing_from_image_escapes() {
    // The projection atom is inside the declared envelope but has no image
    // record: no exact evidence exists, so the gate escapes.
    let a = digest(1);
    let b = digest(2);
    let ghost = digest(9);
    let m = manifest(
        vec![obj(1, 100), obj(2, 200)],
        vec![
            layer(a, Some(true), Some(true)),
            layer(b, Some(true), Some(true)),
        ],
        vec![scenario("task-main", &[a, b, ghost], 3)],
    );
    let req = request("task-main", &[a, b, ghost]);
    let scp = scope(&[]);
    let inp = input(
        "index-v1",
        &[
            (a, Some(true)),
            (b, Some(true)),
            (ghost, Some(true)),
        ],
        1,
    );
    let native = NativeBaseline::new(3000, 9);
    let mut gate = route();
    let (packet, _view) = assert_escaped(gate.snap(&m, &req, &scp, &inp, &native).unwrap());

    assert!(packet.baseline_escape);
    assert_reason_contains(&packet, "demanded_atom_missing_from_image:");
    assert!(packet.atoms.is_empty());
    packet.validate().unwrap();
}

// ---------------------------------------------------------------------------
// Adapter stability: the same packet through two harness adapters
// ---------------------------------------------------------------------------

#[test]
fn corpus_packet_stable_through_two_harness_adapters() {
    let a = digest(1);
    let b = digest(2);
    let c = digest(3);
    let all = [a, b, c];
    let m = manifest(
        vec![obj(1, 100), obj(2, 200), obj(3, 300)],
        vec![
            layer(a, Some(true), Some(true)),
            layer(b, Some(true), Some(true)),
            layer(c, Some(true), Some(true)),
        ],
        vec![scenario("task-main", &all, 42)],
    );
    let req = request("task-main", &all);
    let scp = scope(&[]);
    let inp = input(
        "index-v1",
        &[(a, Some(true)), (b, Some(true)), (c, Some(true))],
        1,
    );
    let native = NativeBaseline::new(3000, 9);

    // Adapter A: the hub route constructs the packet.
    let mut gate_a = route();
    let (packet_a, _, _, _) = assert_snapped(gate_a.snap(&m, &req, &scp, &inp, &native).unwrap());
    let canonical_a = packet_a.canonical_render_json();
    let root_a = packet_a.packet_root();

    // Adapter B: the wire round-trip -- deserialize the canonical rendering
    // into the packet type and re-render. Bytes must be identical.
    let parsed: SnapPacket = serde_json::from_str(&canonical_a)
        .expect("canonical packet must parse into the packet type");
    parsed.validate().unwrap();
    assert_eq!(parsed.canonical_render_json(), canonical_a);
    assert_eq!(parsed.packet_root(), root_a);
    parsed.verify_root(&root_a).unwrap();

    // A second fresh route instance (same secret and bindings) must produce
    // the byte-identical packet: no harness adapter can change the packet.
    let mut gate_b = route();
    let (packet_b, _, _, _) = assert_snapped(gate_b.snap(&m, &req, &scp, &inp, &native).unwrap());
    assert_eq!(packet_b.canonical_render_json(), canonical_a);
    assert_eq!(packet_b.packet_root(), root_a);
    assert_eq!(packet_b.handle_id, packet_a.handle_id);
    assert_eq!(packet_b.decision_view_root, packet_a.decision_view_root);
}

// ---------------------------------------------------------------------------
// Read-only authority surface
// ---------------------------------------------------------------------------

fn collect_keys(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                out.push(key.clone());
                collect_keys(value, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_keys(item, out);
            }
        }
        _ => {}
    }
}

#[test]
fn corpus_packet_and_permit_have_read_only_authority_surface() {
    let a = digest(1);
    let b = digest(2);
    let c = digest(3);
    let all = [a, b, c];
    let m = manifest(
        vec![obj(1, 100), obj(2, 200), obj(3, 300)],
        vec![
            layer(a, Some(true), Some(true)),
            layer(b, Some(true), Some(true)),
            layer(c, Some(true), Some(true)),
        ],
        vec![scenario("task-main", &all, 42)],
    );
    let req = request("task-main", &all);
    let scp = scope(&[]);
    let inp = input(
        "index-v1",
        &[(a, Some(true)), (b, Some(true)), (c, Some(true))],
        1,
    );
    let native = NativeBaseline::new(3000, 9);
    let mut gate = route();
    let (packet, _, expansion, _handle) =
        assert_snapped(gate.snap(&m, &req, &scp, &inp, &native).unwrap());

    let mut packet_keys = Vec::new();
    collect_keys(&json!(packet), &mut packet_keys);
    for key in &packet_keys {
        for forbidden in ["edit", "transaction", "commit", "write", "mutat", "effect"] {
            assert!(
                !key.to_lowercase().contains(forbidden),
                "packet key {key:?} must not carry {forbidden:?} authority"
            );
        }
    }

    let mut permit_keys = Vec::new();
    collect_keys(&json!(expansion.permit), &mut permit_keys);
    assert_eq!(
        permit_keys,
        [
            "demand_plan_root",
            "epoch",
            "handle_id",
            "index_root",
            "index_version",
            "project_root",
            "projection_root",
            "protected_scope_root",
            "renderer_contract",
            "request_root",
            "tenant",
        ]
    );
    for key in &permit_keys {
        for forbidden in ["edit", "transaction", "commit", "write", "mutat", "effect"] {
            assert!(
                !key.to_lowercase().contains(forbidden),
                "permit key {key:?} must not carry {forbidden:?} authority"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Exactly one first expansion per snap call; decision view certificate laws
// ---------------------------------------------------------------------------

#[test]
fn corpus_snap_issues_fresh_handle_and_expands_exactly_once_per_call() {
    let a = digest(1);
    let b = digest(2);
    let c = digest(3);
    let all = [a, b, c];
    let m = manifest(
        vec![obj(1, 100), obj(2, 200), obj(3, 300)],
        vec![
            layer(a, Some(true), Some(true)),
            layer(b, Some(true), Some(true)),
            layer(c, Some(true), Some(true)),
        ],
        vec![scenario("task-main", &all, 42)],
    );
    let req = request("task-main", &all);
    let scp = scope(&[]);
    let inp = input(
        "index-v1",
        &[(a, Some(true)), (b, Some(true)), (c, Some(true))],
        1,
    );
    let native = NativeBaseline::new(3000, 9);
    let mut gate = route();

    let (packet_one, _, expansion_one, handle_one) =
        assert_snapped(gate.snap(&m, &req, &scp, &inp, &native).unwrap());
    let (packet_two, _, expansion_two, handle_two) =
        assert_snapped(gate.snap(&m, &req, &scp, &inp, &native).unwrap());

    // Each snap call issues a fresh read-only handle and performs exactly
    // one first expansion (delta sequence 0, projection exact).
    assert_ne!(packet_one.handle_id, packet_two.handle_id);
    assert_ne!(handle_one.handle_id(), handle_two.handle_id());
    assert_eq!(expansion_one.session.delta_seq(), 0);
    assert_eq!(expansion_two.session.delta_seq(), 0);
    assert_eq!(expansion_one.atoms.len(), 3);
    assert_eq!(expansion_two.atoms.len(), 3);
    assert_eq!(expansion_one.visible_bytes, expansion_two.visible_bytes);
}

#[test]
fn corpus_decision_view_certificate_laws_hold() {
    let a = digest(1);
    let b = digest(2);
    let c = digest(3);
    let all = [a, b, c];
    let m = manifest(
        vec![obj(1, 100), obj(2, 200), obj(3, 300)],
        vec![
            layer(a, Some(true), Some(true)),
            layer(b, Some(true), Some(true)),
            layer(c, Some(true), Some(true)),
        ],
        vec![scenario("task-main", &all, 42)],
    );
    let req = request("task-main", &all);
    let scp = scope(&[]);
    let native = NativeBaseline::new(3000, 9);
    let needed = zero_gate::snap_to_file::snap_evidence_classes();

    // Safe: every needed class present -> Proved stands.
    let mut gate = route();
    let inp_safe = input(
        "index-v1",
        &[(a, Some(true)), (b, Some(true)), (c, Some(true))],
        1,
    );
    let (_, view, _, _) = assert_snapped(gate.snap(&m, &req, &scp, &inp_safe, &native).unwrap());
    let present_safe: BTreeSet<String> = needed.clone();
    assert_eq!(
        view.certificate(&needed, &present_safe).unwrap(),
        CompletenessGrade::Proved
    );
    // A Proved claim with a missing needed class fails closed.
    let mut present_no_coverage = needed.clone();
    present_no_coverage.remove("coverage");
    let error = view
        .certificate(&needed, &present_no_coverage)
        .expect_err("Proved claim with missing coverage must fail");
    assert!(error.to_string().contains("coverage"));

    // Unknown: a missing needed class degrades the claim to Unknown --
    // never to a guessed subset.
    let mut gate = route();
    let inp_unknown = input(
        "index-v1",
        &[(a, Some(true)), (b, None), (c, Some(true))],
        1,
    );
    let (_, view) = assert_escaped(gate.snap(&m, &req, &scp, &inp_unknown, &native).unwrap());
    let mut present_unknown: BTreeSet<String> = needed.clone();
    present_unknown.remove("coverage");
    assert_eq!(
        view.certificate(&needed, &present_unknown).unwrap(),
        CompletenessGrade::Unknown
    );

    // Deterministic roots: the view root is the canonical render digest and
    // the packet binds it.
    let mut gate = route();
    let (packet, view, _, _) = assert_snapped(gate.snap(&m, &req, &scp, &inp_safe, &native).unwrap());
    assert_eq!(packet.decision_view_root, view.root());
    assert_eq!(
        view.root(),
        zero_abi::sha256_hex(view.canonical_render_json().as_bytes())
    );
}

#[test]
fn corpus_empty_projection_request_fails_closed() {
    // The W9-E grammar requires a nonempty projection; a malformed request
    // can never reach the route (one grammar, fail-closed).
    let error = DemandRequest::new("task-main".to_owned(), vec![]).unwrap_err();
    assert!(matches!(error, zero_gate::DemandError::EmptyProjection));
}

#[test]
fn corpus_ledger_measurement_sources_are_honest() {
    let a = digest(1);
    let b = digest(2);
    let c = digest(3);
    let all = [a, b, c];
    let m = manifest(
        vec![obj(1, 100), obj(2, 200), obj(3, 300)],
        vec![
            layer(a, Some(true), Some(true)),
            layer(b, Some(true), Some(true)),
            layer(c, Some(true), Some(true)),
        ],
        vec![scenario("task-main", &all, 42)],
    );
    let req = request("task-main", &all);
    let scp = scope(&[]);
    let inp = input(
        "index-v1",
        &[(a, Some(true)), (b, Some(true)), (c, Some(true))],
        1,
    );
    let native = NativeBaseline::new(3000, 9);
    let mut gate = route();
    let (_, _, expansion, _) = assert_snapped(gate.snap(&m, &req, &scp, &inp, &native).unwrap());

    // Exact rows are exact; native baseline rows are declared estimates;
    // nothing estimated is ever reported as exact.
    let by_class: BTreeMap<&str, &str> = expansion
        .ledger
        .rows
        .iter()
        .map(|row| (row.class.as_str(), row.measurement_source.as_str()))
        .collect();
    for class in [
        "visible_bytes",
        "backend_work",
        "retry_count",
        "first_try_sufficiency",
        "false_complete",
        "certified_atoms",
        "expanded_atoms",
    ] {
        assert_eq!(by_class.get(class), Some(&"exact"), "{class} must be exact");
    }
    assert_eq!(
        by_class.get("native_baseline_bytes"),
        Some(&"estimate"),
        "native baseline must stay an estimate"
    );
    assert_eq!(
        by_class.get("native_baseline_probes"),
        Some(&"estimate"),
        "native baseline must stay an estimate"
    );
}
