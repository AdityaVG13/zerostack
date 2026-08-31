//! Property-based tests for the certified invalidation engine.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use proptest::prelude::*;

use graphzero_core::invalidation::{
    ArtifactId, DependencyGraph, InfluenceClass, InvalidationError, RecomputeEngine,
};
use graphzero_types::ContentHash;

const INPUTS: usize = 4;
const MAX_NODES: usize = 20;
const MAX_PRODUCERS: usize = MAX_NODES - INPUTS;

/// Marker byte sequence that makes a flaky producer fail (recompute returns
/// None). Never present in baseline values.
const POISON: &[u8] = b"POISON";

fn aid(i: usize) -> ArtifactId {
    ArtifactId(ContentHash::of(format!("n{i}").as_bytes()))
}

/// Per-node predecessor strategy: 1..=3 predecessors among earlier indices,
/// sorted, so every node has at least one declared dependency and the graph
/// is acyclic by index order.
fn arb_preds() -> impl Strategy<Value = Vec<Vec<usize>>> {
    let preds: [BoxedStrategy<Vec<usize>>; MAX_PRODUCERS] = std::array::from_fn(|k| {
        let i = INPUTS + k;
        prop::collection::hash_set(0..i, 1..=3.min(i))
            .prop_map(|s: std::collections::HashSet<usize>| {
                let mut v: Vec<usize> = s.into_iter().collect();
                v.sort_unstable();
                v
            })
            .boxed()
    });
    preds.prop_map(|all: [Vec<usize>; MAX_PRODUCERS]| all.to_vec())
}

/// Random DAG: `n` nodes (4..=20), first `INPUTS` are sources, every later
/// node gets 1..=3 predecessors among earlier nodes. `constants[i]` marks
/// equal-value producers (output independent of inputs -> cutoff triggers).
fn arb_dag() -> impl Strategy<Value = (usize, Vec<Vec<usize>>, Vec<bool>)> {
    (4..=MAX_NODES).prop_flat_map(|n| {
        (
            Just(n),
            arb_preds().prop_map(move |preds| preds[..n - INPUTS].to_vec()),
            prop::collection::vec(any::<bool>(), n - INPUTS),
        )
    })
}

/// Like [`arb_dag`], plus a per-producer flaky mask for property 4.
fn arb_dag_with_flaky() -> impl Strategy<Value = (usize, Vec<Vec<usize>>, Vec<bool>, Vec<bool>)> {
    (4..=MAX_NODES).prop_flat_map(|n| {
        (
            Just(n),
            arb_preds().prop_map(move |preds| preds[..n - INPUTS].to_vec()),
            prop::collection::vec(any::<bool>(), n - INPUTS),
            prop::collection::vec(any::<bool>(), n - INPUTS),
        )
    })
}

fn baseline_inputs() -> BTreeMap<ArtifactId, Vec<u8>> {
    (0..INPUTS)
        .map(|k| (aid(k), format!("v0-{k}").into_bytes()))
        .collect()
}

fn changed_inputs(
    edited: &std::collections::HashSet<usize>,
    round: u32,
) -> BTreeMap<ArtifactId, Vec<u8>> {
    edited
        .iter()
        .map(|k| (aid(*k), format!("v{round}-{k}").into_bytes()))
        .collect()
}

/// Edit values that may carry the poison marker (flaky producers fail).
fn changed_inputs_poisonable(
    edited: &std::collections::HashSet<usize>,
    round: u32,
    poison: bool,
) -> BTreeMap<ArtifactId, Vec<u8>> {
    edited
        .iter()
        .map(|k| {
            let mut v = format!("v{round}-{k}").into_bytes();
            if poison {
                v.extend_from_slice(POISON);
            }
            (aid(*k), v)
        })
        .collect()
}

fn merged_inputs(
    base: &BTreeMap<ArtifactId, Vec<u8>>,
    ch: &BTreeMap<ArtifactId, Vec<u8>>,
) -> BTreeMap<ArtifactId, Vec<u8>> {
    let mut m = base.clone();
    for (k, v) in ch {
        m.insert(*k, v.clone());
    }
    m
}

struct TestEngine {
    graph: DependencyGraph,
    engine: RecomputeEngine,
    /// First declared predecessor per flaky producer, for the deterministic
    /// poison-based failure condition.
    flaky_first_pred: BTreeMap<ArtifactId, usize>,
}

/// Build an engine whose influence edges exactly mirror the producers' true reads, except for
/// `hidden` reads (undeclared true dependencies, property 5). Constant producers ignore all
/// inputs; flaky producers return None while their first prerequisite carries the poison marker.
#[allow(clippy::too_many_arguments)]
fn build_engine(
    n: usize,
    preds: &[Vec<usize>],
    constants: &[bool],
    flaky: &[bool],
    hidden: &[Option<usize>],
) -> TestEngine {
    let mut graph = DependencyGraph::new(InfluenceClass::ExactSupport);
    for i in INPUTS..n {
        for &p in &preds[i - INPUTS] {
            graph.add_dependency(aid(p), aid(i));
        }
    }
    let mut engine = RecomputeEngine::new(graph.clone());
    let mut flaky_first_pred = BTreeMap::new();
    for i in INPUTS..n {
        let id = aid(i);
        let ps: Vec<usize> = preds[i - INPUTS].clone();
        let constant = constants[i - INPUTS];
        let is_flaky = flaky.get(i - INPUTS).copied().unwrap_or(false);
        let hidden_pred = hidden[i - INPUTS];
        let first_pred = ps[0];
        // Constant producers short-circuit before the flaky check inside the
        // producer closure: a constant+flaky producer can never actually
        // fail, so it must not be counted by `flaky_failed`.
        if is_flaky && !constant {
            flaky_first_pred.insert(id, first_pred);
        }
        engine.register_producer(id, move |s| {
            if constant {
                return Some(format!("const:{i}").into_bytes());
            }
            if is_flaky
                && s.get(&aid(first_pred))
                    .is_some_and(|v| v.windows(POISON.len()).any(|w| w == POISON))
            {
                return None;
            }
            let mut out = format!("node{i}:").into_bytes();
            for p in &ps {
                if let Some(v) = s.get(&aid(*p)) {
                    out.extend_from_slice(v);
                } else {
                    out.extend_from_slice(b"<missing>");
                }
                out.push(b'|');
            }
            if let Some(h) = hidden_pred {
                // Undeclared true dependency: read, but no influence edge.
                if let Some(v) = s.get(&aid(h)) {
                    out.extend_from_slice(b"|hidden:");
                    out.extend_from_slice(v);
                }
            }
            Some(out)
        });
    }
    TestEngine {
        graph,
        engine,
        flaky_first_pred,
    }
}

/// First index in `0..i` that is not a declared predecessor: a node whose
/// value exists by the time producer `i` runs but that carries no influence
/// edge (hidden true dependency). Always `Some` because `i >= 4`.
fn hidden_pred(i: usize, ps: &[usize]) -> Option<usize> {
    (0..i).find(|c| !ps.contains(c))
}

/// Producers that failed this pass, detected deterministically from the final state: a flaky
/// producer fails iff its first prerequisite's (final) value carries the poison marker.
fn flaky_failed(te: &TestEngine, state: &BTreeMap<ArtifactId, Vec<u8>>) -> BTreeSet<ArtifactId> {
    te.flaky_first_pred
        .iter()
        .filter(|(_, first_pred)| {
            state
                .get(&aid(**first_pred))
                .is_some_and(|v| v.windows(POISON.len()).any(|w| w == POISON))
        })
        .map(|(id, _)| *id)
        .collect()
}

fn dirty_producers(te: &TestEngine, changed_keys: &BTreeSet<ArtifactId>) -> BTreeSet<ArtifactId> {
    te.graph
        .upward_closure(changed_keys)
        .iter()
        .copied()
        .filter(|id| te.engine.producers.contains_key(id))
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig {
        // Integration tests cannot use SourceParallel (no lib.rs above tests/),
        // so pin persistence to the committed crate-local layout explicitly.
        failure_persistence: Some(Box::new(
            proptest::test_runner::FileFailurePersistence::Direct(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/proptest-regressions/tests/core_invalidation_proptest.txt"
            )),
        )),
        ..ProptestConfig::with_cases(64)
    })]

    /// Property 1: upward-closure soundness. On exact random DAGs, every artifact whose
    /// recompute differs between baseline and edited full rebuilds is inside the certified
    /// invalidated set -- never under invalidate (over-invalidation is allowed for sound overapprox).
    #[test]
    fn upward_closure_never_under_invalidates(
        (n, preds, constants) in arb_dag(),
        edited in prop::collection::hash_set(0..INPUTS, 0..=INPUTS),
        round in 1u32..1000,
    ) {
        let hidden: Vec<Option<usize>> = (0..n - INPUTS).map(|_| None).collect();
        let te = build_engine(n, &preds, &constants, &[], &hidden);
        let base_inputs = baseline_inputs();
        let base = te.engine.full_recompute(&base_inputs).unwrap();
        let ch = changed_inputs(&edited, round);
        let full = te
            .engine
            .full_recompute(&merged_inputs(&base_inputs, &ch))
            .unwrap();
        let changed_keys: BTreeSet<_> = ch.keys().copied().collect();
        let cert = te.graph.certify_invalidation(&changed_keys);
        let differed: BTreeSet<_> = full
            .state
            .iter()
            .filter(|(id, v)| base.state.get(id) != Some(*v))
            .map(|(id, _)| *id)
            .collect();
        prop_assert!(
            differed.is_subset(&cert.invalidated),
            "under-invalidation: recompute-differing artifacts {:?} not subset of \
             certified invalidated set {:?}",
            differed,
            cert.invalidated
        );
        prop_assert!(
            changed_keys.is_subset(&cert.invalidated),
            "changed inputs must be inside the invalidated set"
        );
    }

    /// Property 2: incremental-with-cutoff state equality with a full rebuild
    /// on random edits, with constant (equal-value) producers active.
    #[test]
    fn incremental_equals_full_with_cutoff(
        (n, preds, constants) in arb_dag(),
        edited in prop::collection::hash_set(0..INPUTS, 0..=INPUTS),
        round in 1u32..1000,
    ) {
        let hidden: Vec<Option<usize>> = (0..n - INPUTS).map(|_| None).collect();
        let te = build_engine(n, &preds, &constants, &[], &hidden);
        let base_inputs = baseline_inputs();
        let primed = te.engine.full_recompute(&base_inputs).unwrap();
        let ch = changed_inputs(&edited, round);
        let (incr, _) = te
            .engine
            .incremental_recompute_with_report(&primed.state, &ch)
            .unwrap();
        let full = te
            .engine
            .full_recompute(&merged_inputs(&base_inputs, &ch))
            .unwrap();
        prop_assert_eq!(
            incr.state, full.state,
            "incremental-with-cutoff must be bit-identical to full rebuild"
        );
        te.engine
            .assert_incremental_equivalence(&base_inputs, &ch)
            .unwrap();
    }

    /// Property 3: CutoffReport invariants -- recomputed/cut_off disjoint,
    /// boundaries subset of recomputed, and every dirty producer is either
    /// recomputed or cut off (honest measured savings).
    #[test]
    fn cutoff_report_invariants(
        (n, preds, constants) in arb_dag(),
        edited in prop::collection::hash_set(0..INPUTS, 0..=INPUTS),
        round in 1u32..1000,
    ) {
        let hidden: Vec<Option<usize>> = (0..n - INPUTS).map(|_| None).collect();
        let te = build_engine(n, &preds, &constants, &[], &hidden);
        let base_inputs = baseline_inputs();
        let primed = te.engine.full_recompute(&base_inputs).unwrap();
        let ch = changed_inputs(&edited, round);
        let (_, report) = te
            .engine
            .incremental_recompute_with_report(&primed.state, &ch)
            .unwrap();
        prop_assert!(
            report.recomputed.is_disjoint(&report.cut_off),
            "recomputed and cut_off must be disjoint: {:?} vs {:?}",
            report.recomputed,
            report.cut_off
        );
        prop_assert!(
            report.boundary_nodes.is_subset(&report.recomputed),
            "boundary nodes must be a subset of recomputed: {:?} not subset of {:?}",
            report.boundary_nodes,
            report.recomputed
        );
        let changed_keys: BTreeSet<_> = ch.keys().copied().collect();
        let covered: BTreeSet<_> = report
            .recomputed
            .union(&report.cut_off)
            .copied()
            .collect();
        prop_assert_eq!(
            covered,
            dirty_producers(&te, &changed_keys),
            "every dirty producer must be either recomputed or cut off"
        );
    }

    /// Property 4: taint fail-closed. With randomly failing producers, no
    /// boundary node is ever downstream of a failed producer: equality below
    /// a failure is untrustworthy and must never cut off propagation.
    #[test]
    fn failed_producer_never_cuts_off_downstream(
        (n, preds, constants, flaky) in arb_dag_with_flaky(),
        edited in prop::collection::hash_set(0..INPUTS, 0..=INPUTS),
        round in 1u32..1000,
        poison in any::<bool>(),
    ) {
        let hidden: Vec<Option<usize>> = (0..n - INPUTS).map(|_| None).collect();
        let te = build_engine(n, &preds, &constants, &flaky, &hidden);
        let base_inputs = baseline_inputs();
        // Baseline has no poison, so the full prime always succeeds.
        let primed = te.engine.full_recompute(&base_inputs).unwrap();
        let ch = changed_inputs_poisonable(&edited, round, poison);
        let (incr, report) = te
            .engine
            .incremental_recompute_with_report(&primed.state, &ch)
            .unwrap();
        let failed = flaky_failed(&te, &incr.state);
        for f in &failed {
            let mut downstream = BTreeSet::from([*f]);
            let mut q: VecDeque<ArtifactId> = VecDeque::from([*f]);
            while let Some(x) = q.pop_front() {
                if let Some(nexts) = te.graph.forward.get(&x) {
                    for y in nexts {
                        if downstream.insert(*y) {
                            q.push_back(*y);
                        }
                    }
                }
            }
            downstream.remove(f);
            prop_assert!(
                report.boundary_nodes.is_disjoint(&downstream),
                "boundary node {:?} is downstream of failed producer {f:?} \
                 (taint must propagate fail-closed)",
                report
                    .boundary_nodes
                    .iter()
                    .find(|b| downstream.contains(b))
            );
        }
        // Report invariants hold under failures too.
        prop_assert!(report.recomputed.is_disjoint(&report.cut_off));
        prop_assert!(report.boundary_nodes.is_subset(&report.recomputed));
    }

    /// Property 5: under-invalidation is never silent.
    #[test]
    fn under_invalidation_is_never_silent(
        (n, preds, constants) in arb_dag(),
        edited in prop::collection::hash_set(0..INPUTS, 0..=INPUTS),
        round in 1u32..1000,
    ) {
        let hidden: Vec<Option<usize>> = (INPUTS..n)
            .map(|i| hidden_pred(i, &preds[i - INPUTS]))
            .collect();
        let te = build_engine(n, &preds, &constants, &[], &hidden);
        let base_inputs = baseline_inputs();
        let base = te.engine.full_recompute(&base_inputs).unwrap();
        let ch = changed_inputs(&edited, round);
        let merged = merged_inputs(&base_inputs, &ch);
        let full = te.engine.full_recompute(&merged).unwrap();
        let (incr, _) = te
            .engine
            .incremental_recompute_with_report(&base.state, &ch)
            .unwrap();
        let changed_keys: BTreeSet<_> = ch.keys().copied().collect();
        let _ = changed_keys;
        if incr.state == full.state {
            // Incremental (with cutoff active) matched the full rebuild:
            // the protected check must agree.
            te.engine
                .assert_incremental_equivalence(&base_inputs, &ch)
                .unwrap();
        } else {
            // A hidden (undeclared) dependency made incremental diverge from the full rebuild -- whether by
            // escaping the certified set or by an equality cutoff that a complete graph would have forbidden.
            match te.engine.assert_incremental_equivalence(&base_inputs, &ch) {
                Err(InvalidationError::EquivalenceDivergence(_)) => {}
                other => {
                    return Err(TestCaseError::fail(format!(
                        "under-invalidation was silently accepted: {other:?}; \
                         incremental diverged from full rebuild",
                    )))
                }
            }
        }
    }
}

/// Deterministic anchor for property 5's failure branch: a graph whose
/// producer reads an undeclared input must always be caught by the protected
/// equivalence check (the proptest randomizes this; this case pins it).
#[test]
fn hidden_dependency_divergence_is_detected_fail_closed() {
    let src = aid(0);
    let out = aid(1);
    let mut graph = DependencyGraph::new(InfluenceClass::ExactSupport);
    graph.ensure_node(out); // NO edge src -> out
    let mut engine = RecomputeEngine::new(graph);
    let src_key = src;
    engine.register_producer(out, move |s| s.get(&src_key).cloned());
    let mut base = BTreeMap::new();
    base.insert(src, b"v0".to_vec());
    engine.full_recompute(&base).expect("prime");
    let mut ch = BTreeMap::new();
    ch.insert(src, b"content".to_vec());
    match engine.assert_incremental_equivalence(&base, &ch) {
        Err(InvalidationError::EquivalenceDivergence(_)) => {}
        other => panic!("hidden dependency must fail closed, got {other:?}"),
    }
}
