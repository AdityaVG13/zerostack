//! Contract cases for exact-scenario-closure demand expansion.

use std::collections::BTreeSet;

use zero_abi::{LiveExpandState, SafeExpandHandle, SafetyVerdict, Sha256Digest, sha256};
use zero_gate::project_image::{
    CausalGraphRef, DemandScenario, ExactObject, PerObjectLayers, ProjectImageManifest,
    ProofGraphRef, ShadowResourceLedger,
};
use zero_gate::{
    CoverageAtom, DemandError, DemandRequest, ExactScenarioClosureRoute,
    GraphZeroCompletenessInput, IncrementalDeltaRequest, NativeBaseline, ProtectedScope,
    RouteOutcome, adjudicate,
};

// Fixture helpers

fn digest(seed: u8) -> Sha256Digest {
    Sha256Digest::from_bytes(sha256(&[seed; 32]))
}

fn sorted(atoms: &[Sha256Digest]) -> Vec<Sha256Digest> {
    let mut v = atoms.to_vec();
    v.sort();
    v
}

fn obj(seed: u8, byte_len: u64) -> ExactObject {
    ExactObject::new(digest(seed), byte_len).unwrap()
}

fn layer(atom: Sha256Digest, l2: Option<bool>, l3: Option<bool>) -> PerObjectLayers {
    let (l1, valid, resident, refetch, reason) = match (l2, l3) {
        (Some(true), Some(true)) => (Some(true), Some(true), Some(true), false, None),
        (Some(true), Some(false)) => (Some(true), Some(true), Some(false), false, None),
        (Some(false), Some(false)) => (Some(false), Some(false), Some(false), false, None),
        (Some(false), Some(true)) => (None, Some(false), Some(true), false, None),
        (None, None) => (None, None, None, false, Some("unknown")),
        _ => (l2, l2, l3, false, None),
    };
    PerObjectLayers {
        object_root: atom,
        l1_provider_cached: l1,
        l2_logically_valid: valid,
        l3_physically_resident: resident,
        l2_needs_refetch: refetch,
        unknown_reason: reason.map(|s: &str| s.to_owned()),
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
        demand_weight: 1,
        window_id: None,
        unknown_reason: Some("no envelope".to_owned()),
    }
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

fn request(id: &str, projection: &[Sha256Digest]) -> DemandRequest {
    DemandRequest::new(id.to_owned(), projection.to_vec()).unwrap()
}

fn scope(atoms: &[Sha256Digest]) -> ProtectedScope {
    ProtectedScope::new("test-scope".to_owned(), atoms.to_vec()).unwrap()
}

fn universe(pairs: &[(Sha256Digest, Option<bool>)]) -> Vec<CoverageAtom> {
    let mut atoms = pairs
        .iter()
        .map(|(root, covered)| CoverageAtom {
            atom_root: *root,
            covered: *covered,
        })
        .collect::<Vec<_>>();
    atoms.sort_by_key(|atom| atom.atom_root);
    atoms
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

fn route() -> ExactScenarioClosureRoute {
    ExactScenarioClosureRoute::new(
        [0x11; 32],
        "tenant-a".to_owned(),
        1,
        digest(0x31),
        "index-current".to_owned(),
    )
    .unwrap()
}

fn live_for(route: &ExactScenarioClosureRoute, handle: &SafeExpandHandle) -> LiveExpandState {
    route
        .current_live_state(handle, SafetyVerdict::Safe, false)
        .unwrap()
}

fn atom_set(expanded: &[zero_gate::ExpandedAtom]) -> BTreeSet<Sha256Digest> {
    expanded.iter().map(|atom| atom.atom_root).collect()
}

fn assert_refused(outcome: RouteOutcome) -> SafetyVerdict {
    match outcome {
        RouteOutcome::Refused { verdict } => verdict,
        RouteOutcome::Issued { .. } => panic!("route must refuse, got an issued handle"),
    }
}

// Positive route: exact first expansion + adjudicated metrics

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
        "index-current",
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
    assert_eq!(checker_identity, "zerostack.demand.completeness.total");
    assert_eq!(checker_version, "1.0.0");

    // The certified closure is the full multi-file envelope -- never a primary-file subset.
    assert_eq!(plan.demanded_atoms, sorted(&all));
    assert_eq!(plan.demand_weight, 42);

    let live = live_for(&route, &handle);
    let native = NativeBaseline::new(3000, 9);
    let expansion = route
        .expand_first(&handle, &live, &native)
        .expect("first expansion must succeed");

    // Independently derived projection root (not via production helper)
    let expected_projection_root = {
        const DOMAIN: &[u8] = b"zerostack.demand.projection\0";
        let mut hex_sorted = vec![a.to_hex(), b.to_hex(), c.to_hex()];
        hex_sorted.sort();
        let canonical = zero_abi::canonical_json(&serde_json::json!({ "atoms": hex_sorted }));
        Sha256Digest::from_bytes(sha256(&[DOMAIN, canonical.as_bytes()].concat()))
    };
    assert_eq!(expansion.projection_root, expected_projection_root);
    assert_eq!(
        expansion.projection_root,
        expansion.permit.projection_root()
    );
    assert_eq!(expansion.projection_root, plan.projection_root);

    let returned: Vec<Sha256Digest> = expansion.atoms.iter().map(|atom| atom.atom_root).collect();
    assert_eq!(returned, sorted(&all));

    // Metrics: visible bytes equals sum of returned lengths (independent), savings = baseline - visible
    let expected_visible: u64 = 100 + 200 + 300;
    assert_eq!(expansion.visible_bytes, expected_visible);
    assert_eq!(expansion.ledger.total("visible_bytes"), expected_visible);
    // retry_count is exactly 0 and false_complete 0 for a safe expansion
    assert_eq!(expansion.ledger.total("retry_count"), 0);
    assert_eq!(expansion.ledger.total("false_complete"), 0);
    assert!(expansion.first_try_sufficiency);
    // native savings derived independently
    let expected_savings = 3000 - expected_visible;
    assert_eq!(expansion.native_baseline.discovery_bytes, 3000);
    assert_eq!(expansion.ledger.total("native_baseline_bytes"), 3000);
    assert_eq!(expansion.ledger.total("native_baseline_probes"), 9);
    // Projection covers whole envelope: continuation is terminal.
    assert!(expansion.session.terminal());

    // Adjudication against ground truth: no false-complete
    let ground_truth: BTreeSet<Sha256Digest> = [a, b, c].into_iter().collect();
    let metrics = adjudicate(&expansion, &ground_truth);
    assert!(!metrics.false_complete);
    assert!(metrics.first_try_sufficiency);
    assert_eq!(metrics.visible_bytes, expected_visible);
    assert_eq!(metrics.native_savings_bytes, expected_savings);
}

#[test]
fn corpus_complete_multi_file_permit_and_plan_roots_are_canonical() {
    // Focused supplement: verifies permit and plan roots via independent canonical vectors.
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
        "index-current",
        &[(a, Some(true)), (b, Some(true)), (c, Some(true))],
        1,
    );
    let mut route = route();
    let RouteOutcome::Issued { plan, handle, .. } =
        route.compile_and_check(&m, &req, &scp, &inp).unwrap()
    else {
        panic!("must issue")
    };
    let live = live_for(&route, &handle);
    let expansion = route
        .expand_first(&handle, &live, &NativeBaseline::new(3000, 9))
        .unwrap();
    // Plan root independent: domain || canonical JSON of demanded atoms
    let expected_plan_root = {
        const DOMAIN: &[u8] = b"zerostack.demand.demand_plan\0";
        let mut demanded_hex: Vec<String> = vec![a.to_hex(), b.to_hex(), c.to_hex()];
        demanded_hex.sort();
        let canonical = zero_abi::canonical_json(&serde_json::json!({
            "demand_weight": 42,
            "demanded_atoms": demanded_hex,
            "scenario_id": "task-main",
        }));
        Sha256Digest::from_bytes(sha256(&[DOMAIN, canonical.as_bytes()].concat()))
    };
    assert_eq!(plan.plan_root, expected_plan_root);
    assert_eq!(expansion.plan.plan_root, expected_plan_root);
    assert_eq!(expansion.permit.demand_plan_root(), expected_plan_root);
    // Permit projection root already verified in primary test; re-assert binding
    assert_eq!(expansion.permit.projection_root(), plan.projection_root);
    expansion.validate().unwrap();
}

#[test]
fn corpus_complete_multi_file_metrics_are_reconciled() {
    // Focused supplement: documented metric relationships.
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
        "index-current",
        &[(a, Some(true)), (b, Some(true)), (c, Some(true))],
        1,
    );
    let mut route = route();
    let RouteOutcome::Issued { handle, .. } =
        route.compile_and_check(&m, &req, &scp, &inp).unwrap()
    else {
        panic!("must issue")
    };
    let live = live_for(&route, &handle);
    let native = NativeBaseline::new(4000, 10);
    let expansion = route.expand_first(&handle, &live, &native).unwrap();
    // visible bytes equals sum of returned lengths
    let sum: u64 = expansion.atoms.iter().map(|a| a.byte_len).sum();
    assert_eq!(expansion.visible_bytes, sum);
    assert_eq!(expansion.ledger.total("visible_bytes"), sum);
    // savings = baseline - visible (independent)
    assert_eq!(
        expansion.native_baseline.discovery_bytes - expansion.visible_bytes,
        4000 - sum
    );
    // adjudication savings matches ledger
    let ground_truth: BTreeSet<Sha256Digest> = [a, b, c].into_iter().collect();
    let metrics = adjudicate(&expansion, &ground_truth);
    assert_eq!(metrics.native_savings_bytes, 4000 - sum);
}

// Continuation-bound incremental deltas: new atoms only, live revalidation

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
        "index-current",
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
        expansion
            .atoms
            .iter()
            .map(|atom| atom.atom_root)
            .collect::<Vec<_>>(),
        sorted(&[a, b])
    );
    assert_eq!(expansion.visible_bytes, 300);
    assert!(!expansion.session.terminal());
    let mut session = expansion.session.clone();

    // Delta 1: atom C -- new, certified, live handle revalidated.
    let delta = route
        .expand_delta(
            &session,
            &IncrementalDeltaRequest::new(vec![c]).unwrap(),
            &live,
        )
        .unwrap();
    assert_eq!(delta.delta_seq, 1);
    assert_eq!(delta.new_atoms, 1);
    assert_eq!(
        delta
            .atoms
            .iter()
            .map(|atom| atom.atom_root)
            .collect::<Vec<_>>(),
        vec![c]
    );
    // set difference and monotonic totals
    assert_eq!(delta.visible_bytes_delta, 300);
    assert_eq!(delta.visible_bytes_total, 600);
    assert!(!delta.terminal);
    delta.validate().unwrap();
    session = delta.session;

    // Delta 2: atom D -- now envelope exhausted.
    let delta = route
        .expand_delta(
            &session,
            &IncrementalDeltaRequest::new(vec![d]).unwrap(),
            &live,
        )
        .unwrap();
    assert_eq!(delta.delta_seq, 2);
    assert_eq!(delta.new_atoms, 1);
    assert_eq!(delta.visible_bytes_delta, 400);
    assert_eq!(delta.visible_bytes_total, 1000);
    assert!(delta.terminal);

    // Every atom was expanded exactly once (new atoms only).
    let mut expanded = atom_set(&expansion.atoms);
    expanded.insert(c);
    expanded.insert(d);
    assert_eq!(expanded, envelope.into_iter().collect());
}

#[test]
fn corpus_partial_projection_rejects_already_expanded() {
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
        "index-current",
        &[
            (a, Some(true)),
            (b, Some(true)),
            (c, Some(true)),
            (d, Some(true)),
        ],
        1,
    );
    let mut route = route();
    let RouteOutcome::Issued { handle, .. } =
        route.compile_and_check(&m, &req, &scp, &inp).unwrap()
    else {
        panic!("must issue")
    };
    let live = live_for(&route, &handle);
    let expansion = route
        .expand_first(&handle, &live, &NativeBaseline::new(4000, 12))
        .unwrap();
    let mut session = expansion.session.clone();
    let delta = route
        .expand_delta(
            &session,
            &IncrementalDeltaRequest::new(vec![c]).unwrap(),
            &live,
        )
        .unwrap();
    session = delta.session;
    // Re-expanding an already-expanded atom is refused with typed error.
    match route.expand_delta(
        &session,
        &IncrementalDeltaRequest::new(vec![c]).unwrap(),
        &live,
    ) {
        Err(DemandError::DeltaAtomAlreadyExpanded { atom_root }) => assert_eq!(atom_root, c),
        other => panic!("expected DeltaAtomAlreadyExpanded, got {other:?}"),
    }
}

#[test]
fn corpus_partial_projection_rejects_not_certified() {
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
        "index-current",
        &[
            (a, Some(true)),
            (b, Some(true)),
            (c, Some(true)),
            (d, Some(true)),
        ],
        1,
    );
    let mut route = route();
    let RouteOutcome::Issued { handle, .. } =
        route.compile_and_check(&m, &req, &scp, &inp).unwrap()
    else {
        panic!("must issue")
    };
    let live = live_for(&route, &handle);
    let expansion = route
        .expand_first(&handle, &live, &NativeBaseline::new(4000, 12))
        .unwrap();
    let session = expansion.session;
    // An atom outside the certified envelope is refused.
    match route.expand_delta(
        &session,
        &IncrementalDeltaRequest::new(vec![digest(9)]).unwrap(),
        &live,
    ) {
        Err(DemandError::DeltaAtomNotCertified { .. }) => {}
        other => panic!("expected DeltaAtomNotCertified, got {other:?}"),
    }
}

#[test]
fn corpus_partial_projection_rejects_stale_and_exhausted() {
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
        "index-current",
        &[
            (a, Some(true)),
            (b, Some(true)),
            (c, Some(true)),
            (d, Some(true)),
        ],
        1,
    );
    let mut route = route();
    let RouteOutcome::Issued { handle, .. } =
        route.compile_and_check(&m, &req, &scp, &inp).unwrap()
    else {
        panic!("must issue")
    };
    let live = live_for(&route, &handle);
    let expansion = route
        .expand_first(&handle, &live, &NativeBaseline::new(4000, 12))
        .unwrap();
    let mut session = expansion.session.clone();
    let d1 = route
        .expand_delta(
            &session,
            &IncrementalDeltaRequest::new(vec![c]).unwrap(),
            &live,
        )
        .unwrap();
    session = d1.session;
    let d2 = route
        .expand_delta(
            &session,
            &IncrementalDeltaRequest::new(vec![d]).unwrap(),
            &live,
        )
        .unwrap();
    session = d2.session;
    // Stale continuation (replayed pre-delta token) is refused.
    let stale = expansion.session.clone();
    match route.expand_delta(
        &stale,
        &IncrementalDeltaRequest::new(vec![c]).unwrap(),
        &live,
    ) {
        Err(DemandError::StaleContinuation {
            expected, actual, ..
        }) => {
            assert_eq!(expected, 2);
            assert_eq!(actual, 0);
        }
        other => panic!("expected StaleContinuation, got {other:?}"),
    }
    // Exhausted session refuses further deltas.
    match route.expand_delta(
        &session,
        &IncrementalDeltaRequest::new(vec![a]).unwrap(),
        &live,
    ) {
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
        "index-current",
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
    stale_live.index_version = "index-stale".to_owned();
    match route.expand_delta(
        &session,
        &IncrementalDeltaRequest::new(vec![c]).unwrap(),
        &stale_live,
    ) {
        Err(DemandError::RevalidationUnsafe { reasons }) => {
            assert_eq!(
                reasons,
                vec!["index_version_mismatch".to_owned()],
                "expected typed index_version_mismatch"
            );
        }
        other => panic!("expected RevalidationUnsafe with index_version_mismatch, got {other:?}"),
    }

    // Prove the rejected delta appended nothing and did not advance sequence.
    // Reusing the same continuation must still succeed with seq 1.
    assert_eq!(session.delta_seq(), 0);
    let delta = route
        .expand_delta(
            &session,
            &IncrementalDeltaRequest::new(vec![c]).unwrap(),
            &live,
        )
        .unwrap();
    assert_eq!(delta.new_atoms, 1);
    assert_eq!(delta.delta_seq, 1);
    assert_eq!(delta.atoms[0].atom_root, c);
}

// False-complete blocker and Unknown honesty

#[test]
fn corpus_false_complete_blocker_under_declared_envelope_is_unsafe() {
    // Ground truth is {A, B, C}, but the request declares only {A, B}.
    // Published coverage establishes C, so the route must reject the incomplete
    // claim.
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
        "index-current",
        &[(a, Some(true)), (b, Some(true)), (c, Some(true))],
        1,
    );
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert_eq!(
        verdict,
        SafetyVerdict::Unsafe {
            reasons: vec![format!("coverage_exceeds_demand:{}", c.to_hex())]
        }
    );
    // No handle is issued on Unsafe: false-complete cannot occur.
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
        "index-current",
        &[(a, Some(true)), (b, None), (c, Some(true))],
        1,
    );
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert_eq!(
        verdict,
        SafetyVerdict::Unknown {
            reasons: vec![format!("coverage_unknown:{}", b.to_hex())]
        }
    );
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
    let inp = input("index-current", &[(a, Some(true)), (b, Some(true))], 1);
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert_eq!(
        verdict,
        SafetyVerdict::Unknown {
            reasons: vec![format!("demanded_atom_uncovered:{}", c.to_hex())]
        }
    );
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
    let inp = input("index-current", &[], 1);
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    let mut expected_reasons = sorted(&all)
        .into_iter()
        .map(|root| format!("demanded_atom_uncovered:{}", root.to_hex()))
        .collect::<Vec<_>>();
    expected_reasons.push("no_coverage_evidence".to_owned());
    assert_eq!(
        verdict,
        SafetyVerdict::Unknown {
            reasons: expected_reasons,
        }
    );
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
    let inp = input("index-current", &[(a, Some(true)), (b, Some(true))], 1);
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert_eq!(
        verdict,
        SafetyVerdict::Unknown {
            reasons: vec!["scenario_envelope_unknown:task-main".to_owned()]
        }
    );
}

// Positive falsification -> Unsafe

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
        "index-current",
        &[(a, Some(true)), (b, Some(false)), (c, Some(true))],
        1,
    );
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert_eq!(
        verdict,
        SafetyVerdict::Unsafe {
            reasons: vec![format!("atom_not_covered:{}", b.to_hex())]
        }
    );
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
        "index-current",
        &[(a, Some(true)), (b, Some(true)), (c, Some(true))],
        1,
    );
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert_eq!(
        verdict,
        SafetyVerdict::Unsafe {
            reasons: vec![format!("protected_atom_demanded:{}", b.to_hex())]
        }
    );
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
        "index-current",
        &[(a, Some(true)), (b, Some(true)), (c, Some(true))],
        1,
    );
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert_eq!(
        verdict,
        SafetyVerdict::Unsafe {
            reasons: vec![format!("demanded_atom_l2_invalid:{}", b.to_hex())]
        }
    );
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
        "index-current",
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
    assert_eq!(
        verdict,
        SafetyVerdict::Unsafe {
            reasons: vec![format!("projection_exceeds_demand:{}", d.to_hex())]
        }
    );
}

// Unknown image-side evidence

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
        "index-current",
        &[(a, Some(true)), (b, Some(true)), (c, Some(true))],
        1,
    );
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert_eq!(
        verdict,
        SafetyVerdict::Unknown {
            reasons: vec![format!("demanded_atom_layers_unknown:{}", b.to_hex())]
        }
    );
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
        "index-current",
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
    assert_eq!(
        verdict,
        SafetyVerdict::Unknown {
            reasons: vec![format!("demanded_atom_missing_from_image:{}", d.to_hex())]
        }
    );
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
    let inp = input("index-current", &[(a, Some(true))], 1);
    let mut route = route();
    match route.compile_and_check(&m, &req, &scp, &inp) {
        Err(DemandError::ScenarioNotFound { scenario_id }) => {
            assert_eq!(scenario_id, "other-scenario")
        }
        other => panic!("expected ScenarioNotFound, got {other:?}"),
    }
}

// Hidden retry and index evidence honesty

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
    let inp = input("index-current", &[(a, Some(true))], 2);
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert_eq!(
        verdict,
        SafetyVerdict::Unsafe {
            reasons: vec!["hidden_retry".to_owned()]
        }
    );
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
    let inp = input("index-stale", &[(a, Some(true))], 1);
    let mut route = route();
    let verdict = assert_refused(route.compile_and_check(&m, &req, &scp, &inp).unwrap());
    assert_eq!(
        verdict,
        SafetyVerdict::Unsafe {
            reasons: vec!["index_version_mismatch".to_owned()]
        }
    );
}

// Live revalidation of the handle (stale/mismatch/cross-tenant)

fn issue_complete_three_atom(route: &mut ExactScenarioClosureRoute) -> SafeExpandHandle {
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
        "index-current",
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
    live.index_version = "index-stale".to_owned();
    let error = route
        .expand_first(&handle, &live, &NativeBaseline::new(1000, 3))
        .expect_err("stale index must revoke");
    match error {
        DemandError::RevalidationUnsafe { reasons } => {
            assert_eq!(reasons, vec!["index_version_mismatch".to_owned()]);
        }
        other => panic!("expected RevalidationUnsafe with index_version_mismatch, got {other:?}"),
    }
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
    match error {
        DemandError::RevalidationUnsafe { reasons } => {
            assert_eq!(reasons, vec!["tenant_mismatch".to_owned()]);
        }
        other => panic!("expected RevalidationUnsafe with tenant_mismatch, got {other:?}"),
    }
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
    match error {
        DemandError::RevalidationUnsafe { reasons } => {
            assert_eq!(reasons, vec!["hidden_retry_after_issue".to_owned()]);
        }
        other => panic!("expected RevalidationUnsafe with hidden_retry_after_issue, got {other:?}"),
    }
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
    match error {
        DemandError::RevalidationUnsafe { reasons } => {
            assert_eq!(
                reasons,
                vec!["completeness_certificate_mismatch".to_owned()]
            );
        }
        other => panic!(
            "expected RevalidationUnsafe with completeness_certificate_mismatch, got {other:?}"
        ),
    }
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
    match error {
        DemandError::RevalidationUnsafe { reasons } => {
            assert_eq!(reasons, vec!["projection_mismatch".to_owned()]);
        }
        other => panic!("expected RevalidationUnsafe with projection_mismatch, got {other:?}"),
    }
}

// Exactly one first expansion

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

// Determinism and bounds

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
        "index-current",
        &[(a, Some(true)), (b, Some(true)), (c, Some(true))],
        1,
    );
    let mut first = route();
    let mut second = ExactScenarioClosureRoute::new(
        [0x99; 32],
        "tenant-a".to_owned(),
        7,
        digest(0x31),
        "index-current".to_owned(),
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
    // Independent projection root without helper
    let expected = {
        const DOMAIN: &[u8] = b"zerostack.demand.projection\0";
        let mut hex_sorted = vec![a.to_hex(), b.to_hex(), c.to_hex()];
        hex_sorted.sort();
        let canonical = zero_abi::canonical_json(&serde_json::json!({ "atoms": hex_sorted }));
        Sha256Digest::from_bytes(sha256(&[DOMAIN, canonical.as_bytes()].concat()))
    };
    assert_eq!(plan_a.projection_root, expected);
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
        "index-current".to_owned(),
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
