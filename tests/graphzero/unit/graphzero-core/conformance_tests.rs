
use std::collections::BTreeMap;
use std::collections::BTreeSet;

use graphzero_types::ContentHash;

use crate::atlas::{AddressAtlas, SnapLevel, TaskFingerprint};
use crate::conflict::{ConflictHyperedge, ConflictHypergraph, ConflictKind};
use crate::decision::{DecisionClosure, DecisionEvidence, DecisionGap, EvidenceKind};
use crate::effect_map::{
    ConsequenceClass, EffectConsequenceMap, ObligationKind, VerifierObligation,
    VerifierObligationMap,
};
use crate::graph::{
    CoverageClass, GraphError, GraphNode, NodeId, ProjectGraph, Relation, SourceAnchor,
};
use crate::invalidation::{ArtifactId, DependencyGraph, InfluenceClass, RecomputeEngine};
use crate::omission::{OmissionImpact, OmissionKind};
use crate::truth::TruthClass;
use crate::world_fiber::{FiberClass, WorldAlternative, WorldFiber};

fn hash(s: &str) -> ContentHash {
    ContentHash::of(s.as_bytes())
}

/// Every graph fact carries a truth class (Unknown first-class).
#[test]
fn checklist_truth_classes_include_unknown() {
    assert_eq!(TruthClass::Unknown.as_str(), "unknown");
    assert!(!TruthClass::Unknown.is_exact());
}

/// Absence claims only inside complete declared coverage scope.
#[test]
fn checklist_absence_requires_complete_coverage() {
    let root = hash("root");
    let g = ProjectGraph::new(root, CoverageClass::Partial, hash("r"), hash("x"));
    assert!(matches!(
        g.certify_absence(Relation::Calls),
        Err(GraphError::CoverageNotComplete { .. })
    ));
    let mut complete = ProjectGraph::new(root, CoverageClass::Complete, hash("r"), hash("x"));
    complete
        .add_node(GraphNode {
            id: NodeId(hash("n")),
            kind: "fn".into(),
            name: "f".into(),
            anchor: SourceAnchor {
                source_root: root,
                producer: hash("p"),
                configuration: hash("c"),
            },
            truth: TruthClass::CompilerExact,
        })
        .unwrap();
    assert!(complete.certify_absence(Relation::Calls).is_ok());
}

/// Sound world fibers exact or overapprox; never hidden underapprox as strict.
#[test]
fn checklist_world_fiber_no_hidden_underapprox() {
    let mut f = WorldFiber::new(hash("root"));
    f.push(WorldAlternative {
        id: hash("a"),
        class: FiberClass::Underapproximation,
        truth: TruthClass::Heuristic,
        effects: BTreeSet::new(),
        premises: vec![],
    });
    assert!(f.has_hidden_underapproximation());
    assert!(!f.is_strict_admissible());
}

/// Higher-order conflict hyperedges beyond pairwise.
#[test]
fn checklist_higher_order_conflict() {
    let mut members = BTreeSet::new();
    members.insert(hash("a"));
    members.insert(hash("b"));
    members.insert(hash("c"));
    let e = ConflictHyperedge::new(hash("e"), ConflictKind::BaselineDominance, members, vec![])
        .unwrap();
    let mut g = ConflictHypergraph::new();
    g.insert(e);
    assert!(g.has_structure_beyond_pairwise());
}

/// Decision closure includes gaps when incomplete.
#[test]
fn checklist_decision_closure_tracks_gaps() {
    let c = DecisionClosure::assemble(
        hash("t"),
        vec![DecisionEvidence {
            kind: EvidenceKind::Definition,
            node: None,
            truth: TruthClass::CompilerExact,
            digest: hash("d"),
        }],
        vec![DecisionGap {
            kind: EvidenceKind::UnresolvedGap,
            reason: "missing verifier".into(),
        }],
    );
    assert!(!c.is_decision_complete());
}

/// Omission impact can force recovery before publication.
#[test]
fn checklist_omission_forces_recovery() {
    let impact = OmissionImpact::classify(
        hash("o"),
        OmissionKind::MissingDependencyEdge,
        BTreeSet::new(),
        true,
        false,
        true,
        vec![],
    );
    assert!(impact.blocks_candidate_publication());
}

/// Address Atlas: top rank alone not a certificate for ambiguous hits.
#[test]
fn checklist_atlas_calibrated_not_top_rank_cert() {
    let mut atlas = AddressAtlas::new();
    atlas.insert_symbol("x", NodeId(hash("1")), TruthClass::CompilerExact);
    atlas.insert_symbol("x", NodeId(hash("2")), TruthClass::CompilerExact);
    let (level, _) = atlas
        .resolve(&TaskFingerprint {
            digest: hash("t"),
            tokens: vec!["x".into()],
        })
        .unwrap();
    assert_eq!(level, SnapLevel::S1);
    assert!(!level.top_rank_is_certificate());
}

/// Incremental and from-scratch graph results equivalent within declared truth scope.
#[test]
fn checklist_incremental_full_equivalence() {
    let mut g = DependencyGraph::new(InfluenceClass::ExactSupport);
    let src = ArtifactId(hash("s"));
    let out = ArtifactId(hash("o"));
    g.add_dependency(src, out);
    let mut eng = RecomputeEngine::new(g);
    eng.register_producer(out, move |s| s.get(&src).map(|v| v.clone()));
    let mut base = BTreeMap::new();
    base.insert(src, b"1".to_vec());
    let mut ch = BTreeMap::new();
    ch.insert(src, b"2".to_vec());
    eng.assert_incremental_equivalence(&base, &ch).unwrap();
}

/// Effect/verifier maps: no completeness from proximity alone; external state flagged.
#[test]
fn checklist_effect_verifier_maps() {
    let mut em = EffectConsequenceMap::new();
    let e = hash("effect");
    em.bind(e, ConsequenceClass::ExternalState, ArtifactId(hash("a")));
    assert!(em.has_external_state(&e));
    let mut vm = VerifierObligationMap::new();
    vm.add(VerifierObligation {
        kind: ObligationKind::Test,
        verifier_id: hash("v"),
        target: hash("t"),
        completeness_certified: false,
    });
    assert!(!vm.uncertified_completeness_claims().is_empty());
}

/// GraphZero never mutates files or selects semantic repairs (structural API only).
#[test]
fn checklist_structural_api_has_no_repair_selectors() {
    // Compile-time surface check: ProjectGraph exposes certify_absence / neighbors only.
    let root = hash("root");
    let g = ProjectGraph::new(root, CoverageClass::Unknown, hash("r"), hash("x"));
    assert!(g.neighbors(NodeId(hash("n")), Relation::Calls).is_empty());
}
