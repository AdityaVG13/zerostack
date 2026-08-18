//! Adjudicated corpus for the W9-E exact-scenario-closure demand family
//! (`zerostack-rybb`).
//!
//! Every acceptance criterion is measured here:
//! - false-complete (release blocker): a result is false-complete iff the
//!   route claims a complete expansion whose returned atoms do not cover the
//!   adjudicated ground truth;
//! - first-try sufficiency: the first expansion covers the projection on the
//!   first attempt, with no hidden retry;
//! - visible bytes, backend (index) work, retry count, and native baseline
//!   cost, all from the bounded expand ledger;
//! - root/projection-exact first expansion;
//! - continuation-bound incremental deltas that append only new atoms and
//!   revalidate the live handle first;
//! - Unknown coverage stays Unknown; nothing missing is ever labeled
//!   complete.

use std::collections::BTreeSet;

use zero_abi::{LiveExpandState, SafetyVerdict, Sha256Digest, SafeExpandHandle, sha256};
use zero_gate::project_image::{
    CausalGraphRef, DemandScenario, ExactObject, PerObjectLayers, ProofGraphRef,
    ProjectImageManifest, ShadowResourceLedger,
};
use zero_gate::{
    CoverageAtom, DemandError, DemandRequest, GraphZeroCompletenessInput,
    IncrementalDeltaRequest, NativeBaseline, ProtectedScope, RouteOutcome, W9eRoute,
    adjudicate, projection_root_of,
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

fn scenario_without_envelope(id: &str) -> DemandScenario {
    DemandScenario {
        scenario_id: id.to_owned(),
        demanded_object_roots: vec![],
        demand_weight: 0,
        window_id: None,
        unknown_reason: Some("envelope not declared".into()),
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

fn universe(pairs: &[(Sha256Digest, Option<bool>)]) -> Vec<CoverageAtom> {
    let mut v: Vec<CoverageAtom> = pairs
        .iter()
        .map(|(atom, covered)| CoverageAtom {
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
) -> GraphZeroCompletenessInput {
    GraphZeroCompletenessInput::new(
        digest(0x31),
        index_version.to_owned(),
        "task-main".to_owned(),
        universe(pairs),
        attempt_count,
    )
    .unwrap()
}

fn route() -> W9eRoute {
    W9eRoute::new(
        [0x42; 32],
        "tenant-a".to_owned(),
        7,
        digest(0x31),
        "index-v1".to_owned(),
    )
    .unwrap()
}

fn live_for(route: &W9eRoute, handle: &SafeExpandHandle) -> LiveExpandState {
    route
        .current_live_state(handle, SafetyVerdict::Safe, false)
        .unwrap()
}

fn atom_set(expanded: &[zero_gate::ExpandedAtom]) -> BTreeSet<Sha256Digest> {
    expanded.iter().map(|atom| atom.atom_root).collect()
}

fn reasons(verdict: &SafetyVerdict) -> Vec<&str> {
    match verdict {
        SafetyVerdict::Safe => vec![],
        SafetyVerdict::Unsafe { reasons } | SafetyVerdict::Unknown { reasons } => {
            reasons.iter().map(String::as_str).collect()
        }
    }
}

fn assert_reason_contains(verdict: &SafetyVerdict, needle: &str) {
    let all = reasons(verdict);
    assert!(
        all.iter().any(|reason| reason.contains(needle)),
        "verdict reasons {all:?} must contain {needle:?}"
    );
}

fn assert_unsafe_reason(error: &DemandError, needle: &str) {
    match error {
        DemandError::RevalidationUnsafe { reasons } => assert!(
            reasons.iter().any(|reason| reason.contains(needle)),
            "unsafe reasons {reasons:?} must contain {needle:?}"
        ),
        other => panic!("expected RevalidationUnsafe, got {other:?}"),
    }
}

fn assert_refused(outcome: RouteOutcome) -> SafetyVerdict {
    match outcome {
        RouteOutcome::Refused { verdict } => verdict,
        RouteOutcome::Issued { .. } => panic!("route must refuse, got an issued handle"),
    }
}

// ---------------------------------------------------------------------------
// Positive route: exact first expansion + adjudicated metrics
// ---------------------------------------------------------------------------

#[test]
fn corpus_complete_multi_file_first_expansion_is_root_projection_exact() {
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
    let mut route = route();
    let outcome = route.compile_and_check(&m, &req, &scp, &inp).unwrap();
    let RouteOutcome::Issued {
        handle,
        plan,
        certificate_root,
        checker_identity,
        checker_version,
    } = outcome
    else {
        panic!("complete multi-file demand must issue");
    };
    assert!(certificate_root != Sha256Digest::ZERO);
    assert_eq!(checker_identity, "zerostack.w9e.completeness.total");
    assert_eq!(checker_version, "1.0.0");

    // The certified closure is the full multi-file envelope -- never a
    // primary-file subset.
    assert_eq!(plan.demanded_atoms, sorted(&all));
    assert_eq!(plan.demand_weight, 42);

    let live = live_for(&route, &handle);
    let native = NativeBaseline::new(3000, 9);
    let expansion = route
        .expand_first(&handle, &live, &native)
        .expect("first expansion must succeed");

    // Root/projection exact: returned set roots to the permit projection.
    let returned: Vec<Sha256Digest> = expansion.atoms.iter().map(|atom| atom.atom_root).collect();
    assert_eq!(expansion.projection_root, projection_root_of(&returned));
    assert_eq!(expansion.projection_root, expansion.permit.projection_root());
    assert_eq!(expansion.projection_root, plan.projection_root);
    assert_eq!(returned, sorted(&all));
    let mut expected_atoms = vec![
        zero_gate::ExpandedAtom {
            atom_root: a,
            byte_len: 100,
        },
        zero_gate::ExpandedAtom {
            atom_root: b,
            byte_len: 200,
        },
        zero_gate::ExpandedAtom {
            atom_root: c,
            byte_len: 300,
        },
    ];
    expected_atoms.sort_by_key(|atom| atom.atom_root);
    assert_eq!(expansion.atoms, expected_atoms);
    expansion.validate().unwrap();

    // Metrics: exact visible bytes, ledged backend work, zero retries,
    // first-try sufficiency, native baseline comparison fields.
    assert_eq!(expansion.visible_bytes, 600);
    assert_eq!(expansion.ledger.total("visible_bytes"), 600);
    assert_eq!(expansion.ledger.total("backend_work"), 20);
    assert_eq!(expansion.ledger.total("retry_count"), 0);
    assert_eq!(expansion.ledger.total("first_try_sufficiency"), 1);
    assert_eq!(expansion.ledger.total("false_complete"), 0);
    assert_eq!(expansion.ledger.total("certified_atoms"), 3);
    assert_eq!(expansion.ledger.total("expanded_atoms"), 3);
    assert_eq!(expansion.ledger.total("native_baseline_bytes"), 3000);
    assert_eq!(expansion.ledger.total("native_baseline_probes"), 9);
    assert_eq!(expansion.certified_atoms, 3);
    assert!(expansion.first_try_sufficiency);
    assert_eq!(expansion.native_baseline, native);
    // Projection covers the whole envelope: continuation is immediately
    // terminal.
    assert!(expansion.session.terminal());

    // Adjudication against the ground-truth closure: no false-complete,
    // first try sufficed, and the W9-E route is cheaper than the declared
    // native baseline.
    let ground_truth: BTreeSet<Sha256Digest> = [a, b, c].into_iter().collect();
    let metrics = adjudicate(&expansion, &ground_truth);
    assert!(!metrics.false_complete);
    assert!(metrics.first_try_sufficiency);
    assert_eq!(metrics.visible_bytes, 600);
    assert_eq!(metrics.backend_work, 20);
    assert_eq!(metrics.retry_count, 0);
    assert_eq!(metrics.native_baseline_bytes, 3000);
    assert_eq!(metrics.certified_atoms, 3);
    assert_eq!(metrics.expanded_atoms, 3);
    assert_eq!(metrics.native_savings_bytes, 2400);
}

// ---------------------------------------------------------------------------
// Continuation-bound incremental deltas: new atoms only, live revalidation
// ---------------------------------------------------------------------------

#[test]
fn corpus_partial_projection_deltas_append_only_new_atoms() {
    let a = digest(1);
    let b = digest(2);
    let c = digest(3);
    let d = digest(4);
    let envelope = [a, b, c, d];
    let m = manifest(
        vec![obj(1, 100), obj(2, 200), obj(3, 300), obj(4, 400)],
        vec![
            layer(a, Some(true), Some(true)),
            layer(b, Some(true), Some(true)),
            layer(c, Some(true), Some(true)),
            layer(d, Some(true), Some(true)),
        ],
        vec![scenario("task-main", &envelope, 7)],
    );
    let req = request("task-main", &[a, b]);
    let scp = scope(&[]);
    let inp = input(
        "index-v1",
        &[
            (a, Some(true)),
            (b, Some(true)),
            (c, Some(true)),
            (d, Some(true)),
        ],
        1,
    );
    let mut route = route();
    let outcome = route.compile_and_check(&m, &req, &scp, &inp).unwrap();
    let RouteOutcome::Issued { handle, .. } = outcome else {
        panic!("must issue");
    };
    let live = live_for(&route, &handle);

    let expansion = route
        .expand_first(&handle, &live, &NativeBaseline::new(4000, 12))
        .unwrap();
    assert_eq!(
        expansion.atoms.iter().map(|atom| atom.atom_root).collect::<Vec<_>>(),
        sorted(&[a, b])
    );
    assert_eq!(expansion.visible_bytes, 300);
    assert_eq!(expansion.ledger.total("backend_work"), 24);
    assert!(!expansion.session.terminal());
    let mut session = expansion.session.clone();

    // Delta 1: atom C -- new, certified, live handle revalidated.
    let delta = route
        .expand_delta(&session, &IncrementalDeltaRequest::new(vec![c]).unwrap(), &live)
        .unwrap();
    assert_eq!(delta.delta_seq, 1);
    assert_eq!(delta.new_atoms, 1);
    assert_eq!(
        delta.atoms.iter().map(|atom| atom.atom_root).collect::<Vec<_>>(),
        vec![c]
    );
    assert_eq!(delta.visible_bytes_delta, 300);
    assert_eq!(delta.visible_bytes_total, 600);
    assert_eq!(delta.ledger.total("backend_work"), 26);
    assert_eq!(delta.ledger.total("retry_count"), 0);
    assert_eq!(delta.ledger.total("false_complete"), 0);
    assert_eq!(delta.ledger.total("new_atoms"), 1);
    assert_eq!(delta.ledger.total("expanded_atoms"), 3);
    assert_eq!(delta.ledger.total("terminal"), 0);
    assert!(!delta.terminal);
    delta.validate().unwrap();
    session = delta.session;

    // Delta 2: atom D -- now the envelope is exhausted.
    // Re-expanding an already-expanded atom is refused.
    match route.expand_delta(&session, &IncrementalDeltaRequest::new(vec![c]).unwrap(), &live) {
        Err(DemandError::DeltaAtomAlreadyExpanded { atom_root }) => assert_eq!(atom_root, c),
        other => panic!("expected DeltaAtomAlreadyExpanded, got {other:?}"),
    }

    // An atom outside the certified envelope is refused.
    match route.expand_delta(
        &session,
        &IncrementalDeltaRequest::new(vec![digest(9)]).unwrap(),
        &live,
    ) {
        Err(DemandError::DeltaAtomNotCertified { .. }) => {}
        other => panic!("expected DeltaAtomNotCertified, got {other:?}"),
    }
    let delta = route
        .expand_delta(&session, &IncrementalDeltaRequest::new(vec![d]).unwrap(), &live)
        .unwrap();
    assert_eq!(delta.delta_seq, 2);
    assert_eq!(delta.new_atoms, 1);
    assert_eq!(delta.visible_bytes_delta, 400);
    assert_eq!(delta.visible_bytes_total, 1000);
    assert_eq!(delta.ledger.total("backend_work"), 28);
    assert!(delta.terminal);
    session = delta.session;

    // Every atom was expanded exactly once (new atoms only).
    let mut expanded = atom_set(&expansion.atoms);
    expanded.insert(c);
    expanded.insert(d);
    assert_eq!(expanded, envelope.into_iter().collect());


    // A stale continuation (replayed pre-delta token) is refused.
    let stale = expansion.session.clone();
    match route.expand_delta(&stale, &IncrementalDeltaRequest::new(vec![c]).unwrap(), &live) {
        Err(DemandError::StaleContinuation { expected, actual, .. }) => {
            assert_eq!(expected, 2);
            assert_eq!(actual, 0);
        }
        other => panic!("expected StaleContinuation, got {other:?}"),
    }

    // The exhausted session refuses further deltas.
    match route.expand_delta(&session, &IncrementalDeltaRequest::new(vec![a]).unwrap(), &live) {
        Err(DemandError::SessionExhausted { .. }) => {}
        other => panic!("expected SessionExhausted, got {other:?}"),
    }
}

#[test]
fn corpus_delta_revalidates_live_handle_before_appending() {
    let a = digest(1);
    let b = digest(2);
    let c = digest(3);
    let envelope = [a, b, c];
    let m = manifest(
        vec![obj(1, 100), obj(2, 200), obj(3, 300)],
        vec![
            layer(a, Some(true), Some(true)),
            layer(b, Some(true), Some(true)),
            layer(c, Some(true), Some(true)),
        ],
        vec![scenario("task-main", &envelope, 1)],
    );
    let req = request("task-main", &[a, b]);
    let scp = scope(&[]);
    let inp = input(
        "index-v1",
        &[(a, Some(true)), (b, Some(true)), (c, Some(true))],
        1,
    );
    let mut route = route();
    let RouteOutcome::Issued { handle, .. } =
        route.compile_and_check(&m, &req, &scp, &inp).unwrap()
    else {
        panic!("must issue");
    };
    let live = live_for(&route, &handle);
    let expansion = route
        .expand_first(&handle, &live, &NativeBaseline::new(1000, 3))
        .unwrap();
    let session = expansion.session;

    // Stale live index: the delta is refused before any atom is appended.
    let mut stale_live = live.clone();
    stale_live.index_version = "index-v2".to_owned();
    match route.expand_delta(&session, &IncrementalDeltaRequest::new(vec![c]).unwrap(), &stale_live) {
        Err(error) => assert_unsafe_reason(&error, "index_version_mismatch"),
        other => panic!("expected revalidation refusal, got {other:?}"),
    }

    // The same delta with the current live state still succeeds.
    let delta = route
        .expand_delta(&session, &IncrementalDeltaRequest::new(vec![c]).unwrap(), &live)
        .unwrap();
    assert_eq!(delta.new_atoms, 1);
}

// ---------------------------------------------------------------------------
// False-complete blocker and Unknown honesty
// ---------------------------------------------------------------------------

#[test]
fn corpus_false_complete_blocker_under_declared_envelope_is_unsafe() {
    // Adjudicated ground truth: {A, B, C}. The request scenario declares only
    // {A, B}; the published coverage universe positively establishes C as
    // part of the task closure. The route must refuse -- selling {A, B} as
    // complete would be a false-complete.
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
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert!(matches!(verdict, SafetyVerdict::Unsafe { .. }));
    assert_reason_contains(&verdict, "coverage_exceeds_demand:");
    // No handle, no expansion: false-complete cannot occur.
}

#[test]
fn corpus_unknown_coverage_is_unknown_never_safe() {
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
    let inp = input(
        "index-v1",
        &[(a, Some(true)), (b, None), (c, Some(true))],
        1,
    );
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert!(matches!(verdict, SafetyVerdict::Unknown { .. }));
    assert_reason_contains(&verdict, "coverage_unknown:");
}

#[test]
fn corpus_demanded_atom_without_coverage_evidence_is_unknown() {
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
    let inp = input("index-v1", &[(a, Some(true)), (b, Some(true))], 1);
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert!(matches!(verdict, SafetyVerdict::Unknown { .. }));
    assert_reason_contains(&verdict, "demanded_atom_uncovered:");
}

#[test]
fn corpus_empty_coverage_universe_is_unknown() {
    let a = digest(1);
    let b = digest(2);
    let all = [a, b];
    let m = manifest(
        vec![obj(1, 100), obj(2, 200)],
        vec![
            layer(a, Some(true), Some(true)),
            layer(b, Some(true), Some(true)),
        ],
        vec![scenario("task-main", &all, 1)],
    );
    let req = request("task-main", &all);
    let scp = scope(&[]);
    let inp = input("index-v1", &[], 1);
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert!(matches!(verdict, SafetyVerdict::Unknown { .. }));
    assert_reason_contains(&verdict, "no_coverage_evidence");
}

#[test]
fn corpus_scenario_without_declared_envelope_is_unknown() {
    let a = digest(1);
    let b = digest(2);
    let m = manifest(
        vec![obj(1, 100), obj(2, 200)],
        vec![
            layer(a, Some(true), Some(true)),
            layer(b, Some(true), Some(true)),
        ],
        vec![scenario_without_envelope("task-main")],
    );
    let req = request("task-main", &[a, b]);
    let scp = scope(&[]);
    let inp = input("index-v1", &[(a, Some(true)), (b, Some(true))], 1);
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert!(matches!(verdict, SafetyVerdict::Unknown { .. }));
    assert_reason_contains(&verdict, "scenario_envelope_unknown:");
}

// ---------------------------------------------------------------------------
// Positive falsification -> Unsafe
// ---------------------------------------------------------------------------

#[test]
fn corpus_positively_uncovered_atom_is_unsafe() {
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
    let inp = input(
        "index-v1",
        &[(a, Some(true)), (b, Some(false)), (c, Some(true))],
        1,
    );
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert!(matches!(verdict, SafetyVerdict::Unsafe { .. }));
    assert_reason_contains(&verdict, "atom_not_covered:");
}

#[test]
fn corpus_protected_atom_demand_is_unsafe() {
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
    let scp = scope(&[b]);
    let inp = input(
        "index-v1",
        &[(a, Some(true)), (b, Some(true)), (c, Some(true))],
        1,
    );
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert!(matches!(verdict, SafetyVerdict::Unsafe { .. }));
    assert_reason_contains(&verdict, "protected_atom_demanded:");
}

#[test]
fn corpus_l2_invalid_demanded_atom_is_unsafe() {
    let a = digest(1);
    let b = digest(2);
    let c = digest(3);
    let all = [a, b, c];
    let m = manifest(
        vec![obj(1, 100), obj(2, 200), obj(3, 300)],
        vec![
            layer(a, Some(true), Some(true)),
            layer(b, Some(false), Some(true)),
            layer(c, Some(true), Some(true)),
        ],
        vec![scenario("task-main", &all, 1)],
    );
    let req = request("task-main", &all);
    let scp = scope(&[]);
    let inp = input(
        "index-v1",
        &[(a, Some(true)), (b, Some(true)), (c, Some(true))],
        1,
    );
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert!(matches!(verdict, SafetyVerdict::Unsafe { .. }));
    assert_reason_contains(&verdict, "demanded_atom_l2_invalid:");
}

#[test]
fn corpus_projection_exceeding_envelope_is_unsafe() {
    let a = digest(1);
    let b = digest(2);
    let c = digest(3);
    let d = digest(4);
    let m = manifest(
        vec![obj(1, 100), obj(2, 200), obj(3, 300), obj(4, 400)],
        vec![
            layer(a, Some(true), Some(true)),
            layer(b, Some(true), Some(true)),
            layer(c, Some(true), Some(true)),
            layer(d, Some(true), Some(true)),
        ],
        vec![scenario("task-main", &[a, b, c], 1)],
    );
    let req = request("task-main", &[a, b, d]);
    let scp = scope(&[]);
    let inp = input(
        "index-v1",
        &[
            (a, Some(true)),
            (b, Some(true)),
            (c, Some(true)),
            (d, Some(true)),
        ],
        1,
    );
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert!(matches!(verdict, SafetyVerdict::Unsafe { .. }));
    assert_reason_contains(&verdict, "projection_exceeds_demand:");
}

// ---------------------------------------------------------------------------
// Unknown image-side evidence
// ---------------------------------------------------------------------------

#[test]
fn corpus_l2_unknown_demanded_atom_is_unknown() {
    let a = digest(1);
    let b = digest(2);
    let c = digest(3);
    let all = [a, b, c];
    let m = manifest(
        vec![obj(1, 100), obj(2, 200), obj(3, 300)],
        vec![
            layer(a, Some(true), Some(true)),
            layer(b, None, None),
            layer(c, Some(true), Some(true)),
        ],
        vec![scenario("task-main", &all, 1)],
    );
    let req = request("task-main", &all);
    let scp = scope(&[]);
    let inp = input(
        "index-v1",
        &[(a, Some(true)), (b, Some(true)), (c, Some(true))],
        1,
    );
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert!(matches!(verdict, SafetyVerdict::Unknown { .. }));
    assert_reason_contains(&verdict, "demanded_atom_layers_unknown:");
}

#[test]
fn corpus_demanded_atom_missing_from_image_is_unknown() {
    let a = digest(1);
    let b = digest(2);
    let c = digest(3);
    let d = digest(4);
    let m = manifest(
        vec![obj(1, 100), obj(2, 200), obj(3, 300)],
        vec![
            layer(a, Some(true), Some(true)),
            layer(b, Some(true), Some(true)),
            layer(c, Some(true), Some(true)),
        ],
        vec![scenario("task-main", &[a, b, c, d], 1)],
    );
    let req = request("task-main", &[a, b, c]);
    let scp = scope(&[]);
    let inp = input(
        "index-v1",
        &[
            (a, Some(true)),
            (b, Some(true)),
            (c, Some(true)),
            (d, Some(true)),
        ],
        1,
    );
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert!(matches!(verdict, SafetyVerdict::Unknown { .. }));
    assert_reason_contains(&verdict, "demanded_atom_missing_from_image:");
}

#[test]
fn corpus_scenario_not_found_is_typed_error() {
    let a = digest(1);
    let m = manifest(
        vec![obj(1, 100)],
        vec![layer(a, Some(true), Some(true))],
        vec![scenario("task-main", &[a], 1)],
    );
    let req = request("other-scenario", &[a]);
    let scp = scope(&[]);
    let inp = input("index-v1", &[(a, Some(true))], 1);
    let mut route = route();
    match route.compile_and_check(&m, &req, &scp, &inp) {
        Err(DemandError::ScenarioNotFound { scenario_id }) => {
            assert_eq!(scenario_id, "other-scenario")
        }
        other => panic!("expected ScenarioNotFound, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Hidden retry and index evidence honesty
// ---------------------------------------------------------------------------

#[test]
fn corpus_retried_completeness_check_is_unsafe_at_issuance() {
    let a = digest(1);
    let m = manifest(
        vec![obj(1, 100)],
        vec![layer(a, Some(true), Some(true))],
        vec![scenario("task-main", &[a], 1)],
    );
    let req = request("task-main", &[a]);
    let scp = scope(&[]);
    let inp = input("index-v1", &[(a, Some(true))], 2);
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert!(matches!(verdict, SafetyVerdict::Unsafe { .. }));
    assert_reason_contains(&verdict, "hidden_retry");
}

#[test]
fn corpus_stale_index_evidence_is_unsafe_at_issuance() {
    let a = digest(1);
    let m = manifest(
        vec![obj(1, 100)],
        vec![layer(a, Some(true), Some(true))],
        vec![scenario("task-main", &[a], 1)],
    );
    let req = request("task-main", &[a]);
    let scp = scope(&[]);
    let inp = input("index-v2", &[(a, Some(true))], 1);
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert!(matches!(verdict, SafetyVerdict::Unsafe { .. }));
    assert_reason_contains(&verdict, "index_version_mismatch");
}

// ---------------------------------------------------------------------------
// Live revalidation of the handle (stale/mismatch/cross-tenant)
// ---------------------------------------------------------------------------

fn issue_complete_three_atom(route: &mut W9eRoute) -> SafeExpandHandle {
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
    let req = request("task-main", &[a, b]);
    let scp = scope(&[]);
    let inp = input(
        "index-v1",
        &[(a, Some(true)), (b, Some(true)), (c, Some(true))],
        1,
    );
    match route.compile_and_check(&m, &req, &scp, &inp).unwrap() {
        RouteOutcome::Issued { handle, .. } => handle,
        RouteOutcome::Refused { .. } => panic!("must issue"),
    }
}

#[test]
fn corpus_stale_index_revokes_live_handle() {
    let mut route = route();
    let handle = issue_complete_three_atom(&mut route);
    let mut live = live_for(&route, &handle);
    live.index_version = "index-v2".to_owned();
    let error = route
        .expand_first(&handle, &live, &NativeBaseline::new(1000, 3))
        .expect_err("stale index must revoke");
    assert_unsafe_reason(&error, "index_version_mismatch");
}

#[test]
fn corpus_cross_tenant_use_revokes_live_handle() {
    let mut route = route();
    let handle = issue_complete_three_atom(&mut route);
    let mut live = live_for(&route, &handle);
    live.tenant = "tenant-b".to_owned();
    let error = route
        .expand_first(&handle, &live, &NativeBaseline::new(1000, 3))
        .expect_err("cross-tenant use must revoke");
    assert_unsafe_reason(&error, "tenant_mismatch");
}

#[test]
fn corpus_hidden_retry_after_issue_revokes_live_handle() {
    let mut route = route();
    let handle = issue_complete_three_atom(&mut route);
    let live = route
        .current_live_state(&handle, SafetyVerdict::Safe, true)
        .unwrap();
    let error = route
        .expand_first(&handle, &live, &NativeBaseline::new(1000, 3))
        .expect_err("hidden retry must revoke");
    assert_unsafe_reason(&error, "hidden_retry_after_issue");
}

#[test]
fn corpus_certificate_tamper_revokes_live_handle() {
    let mut route = route();
    let handle = issue_complete_three_atom(&mut route);
    let mut live = live_for(&route, &handle);
    live.completeness.certificate_root = Some(digest(0x99));
    let error = route
        .expand_first(&handle, &live, &NativeBaseline::new(1000, 3))
        .expect_err("tampered certificate must revoke");
    assert_unsafe_reason(&error, "completeness_certificate_mismatch");
}

#[test]
fn corpus_altered_projection_revokes_live_handle() {
    let mut route = route();
    let handle = issue_complete_three_atom(&mut route);
    let mut live = live_for(&route, &handle);
    live.projection_root = digest(0x77);
    let error = route
        .expand_first(&handle, &live, &NativeBaseline::new(1000, 3))
        .expect_err("altered projection must revoke");
    assert_unsafe_reason(&error, "projection_mismatch");
}

// ---------------------------------------------------------------------------
// Exactly one first expansion
// ---------------------------------------------------------------------------

#[test]
fn corpus_exactly_one_first_expansion_per_handle() {
    let mut route = route();
    let handle = issue_complete_three_atom(&mut route);
    let live = live_for(&route, &handle);
    let native = NativeBaseline::new(1000, 3);
    route
        .expand_first(&handle, &live, &native)
        .expect("first expansion must succeed");
    match route.expand_first(&handle, &live, &native) {
        Err(DemandError::AlreadyFirstExpanded { .. }) => {}
        other => panic!("expected AlreadyFirstExpanded, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Determinism and bounds
// ---------------------------------------------------------------------------

#[test]
fn corpus_plan_and_projection_roots_are_deterministic_across_routes() {
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
    let mut first = route();
    let mut second = W9eRoute::new(
        [0x99; 32],
        "tenant-a".to_owned(),
        7,
        digest(0x31),
        "index-v1".to_owned(),
    )
    .unwrap();
    let outcome_a = first.compile_and_check(&m, &req, &scp, &inp).unwrap();
    let outcome_b = second.compile_and_check(&m, &req, &scp, &inp).unwrap();
    let RouteOutcome::Issued { plan: plan_a, .. } = outcome_a else {
        panic!("must issue")
    };
    let RouteOutcome::Issued { plan: plan_b, .. } = outcome_b else {
        panic!("must issue")
    };
    assert_eq!(plan_a.plan_root, plan_b.plan_root);
    assert_eq!(plan_a.projection_root, plan_b.projection_root);
    assert_eq!(projection_root_of(&sorted(&all)), plan_a.projection_root);
    // Handle ids differ across issuers (per-issuance nonce), but plans do not.
    assert_eq!(plan_a.demanded_atoms, plan_b.demanded_atoms);
}

#[test]
fn corpus_over_budget_coverage_universe_fails_closed() {
    let pairs: Vec<(Sha256Digest, Option<bool>)> = (1u16..=129)
        .map(|seed| (digest(seed as u8), Some(true)))
        .collect();
    match GraphZeroCompletenessInput::new(
        digest(0x31),
        "index-v1".to_owned(),
        "task-main".to_owned(),
        universe(&pairs),
        1,
    ) {
        Err(DemandError::BoundExceeded { field, .. }) => {
            assert_eq!(field, "coverage_universe")
        }
        other => panic!("expected BoundExceeded, got {other:?}"),
    }
}
