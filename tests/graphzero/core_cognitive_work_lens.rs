//! internal contract laws for the GraphZero core engine.

use graphzero_core::*;
use graphzero_types::ContentHash;

fn edge(from: &str, to: &str) -> CompilerSemanticEdge {
    CompilerSemanticEdge {
        from: from.into(),
        to: to.into(),
        kind: CompilerEdgeKind::Call,
        language: "typescript".into(),
        compiler_root: "compiler".into(),
        configuration_root: "config".into(),
        freshness_root: "snap-a".into(),
        source_path: "src/index.ts".into(),
        source_line: 1,
        source_column: 1,
    }
}

fn report(resolved_edges: u64) -> SemanticExtractionReport {
    SemanticExtractionReport {
        language: "typescript".into(),
        compiler_root: "compiler".into(),
        configuration_root: "config".into(),
        freshness_root: "snap-a".into(),
        indexed_files: 1,
        resolved_edges,
        unresolved_sites: vec![],
        fatal_diagnostics: vec![],
    }
}

fn obligation(id: &str) -> TypedObligation {
    TypedObligation {
        id: id.into(),
        kind: TypedObligationKind::Verification,
        protected_scope_root: "scope-root".into(),
        required_evidence_kinds: vec!["type".into(), "test".into()],
    }
}

fn support(
    id: &str,
    obligation_id: &str,
    snapshot_root: &str,
    valid_to_epoch: Option<u64>,
) -> ProofSupportHyperedge {
    ProofSupportHyperedge {
        id: id.into(),
        obligation_id: obligation_id.into(),
        sources: vec!["a.ts".into(), "b.ts".into()],
        target: "cert".into(),
        proof_root: "scope-root".into(),
        verifier_contract_root: "verifier".into(),
        snapshot_root: snapshot_root.into(),
        provenance_root: "provenance".into(),
        valid_from_epoch: 1,
        valid_to_epoch,
    }
}

fn locus(node: u8, truth: TruthClass) -> LocusRank {
    LocusRank {
        node: NodeId(ContentHash([node; 32])),
        score: 1,
        truth,
        premises: vec!["evidence-ref".into()],
    }
}

fn region(
    loci: Vec<LocusRank>,
    impact_closure: ClosureClass,
    obligations: Vec<TypedObligation>,
    supports: Vec<ProofSupportHyperedge>,
) -> MechanicalRegionInput {
    MechanicalRegionInput {
        truth: TruthClass::CompilerExact,
        fiber: FiberClass::Exact,
        gaps: vec![],
        independently_verified: true,
        loci,
        impact_closure,
        obligations,
        supports,
        snapshot_root: "snap-a".into(),
        epoch: 5,
    }
}

#[test]
fn safe_requires_unique_locus_exact_impact_and_fresh_supports() {
    let input = region(
        vec![locus(1, TruthClass::CompilerExact)],
        ClosureClass::Exact,
        vec![obligation("o1")],
        vec![support("s1", "o1", "snap-a", None)],
    );
    assert_eq!(
        classify_mechanical_region(&input),
        MechanicalGraphVerdict::Safe
    );
}

#[test]
fn stale_support_degrades_to_unknown() {
    let input = region(
        vec![locus(1, TruthClass::CompilerExact)],
        ClosureClass::Exact,
        vec![obligation("o1")],
        vec![support("s1", "o1", "snap-a", Some(5))],
    );
    assert_eq!(
        classify_mechanical_region(&input),
        MechanicalGraphVerdict::Unknown
    );
}

#[test]
fn missing_support_is_unknown() {
    let input = region(
        vec![locus(1, TruthClass::CompilerExact)],
        ClosureClass::Exact,
        vec![obligation("o1")],
        vec![],
    );
    assert_eq!(
        classify_mechanical_region(&input),
        MechanicalGraphVerdict::Unknown
    );
}

#[test]
fn snapshot_mismatch_is_unknown() {
    let input = region(
        vec![locus(1, TruthClass::CompilerExact)],
        ClosureClass::Exact,
        vec![obligation("o1")],
        vec![support("s1", "o1", "snap-b", None)],
    );
    assert_eq!(
        classify_mechanical_region(&input),
        MechanicalGraphVerdict::Unknown
    );
}

#[test]
fn invalid_support_never_discharges() {
    let mut broken = support("s1", "o1", "snap-a", None);
    broken.sources = vec![];
    let input = region(
        vec![locus(1, TruthClass::CompilerExact)],
        ClosureClass::Exact,
        vec![obligation("o1")],
        vec![broken],
    );
    assert_eq!(
        classify_mechanical_region(&input),
        MechanicalGraphVerdict::Unknown
    );
}
#[test]
fn invalid_obligation_is_unknown() {
    let mut invalid = obligation("o1");
    invalid.id = String::new();
    let input = region(
        vec![locus(1, TruthClass::CompilerExact)],
        ClosureClass::Exact,
        vec![invalid],
        vec![support("s1", "o1", "snap-a", None)],
    );
    assert_eq!(
        classify_mechanical_region(&input),
        MechanicalGraphVerdict::Unknown
    );
}

#[test]
fn mismatched_proof_root_is_unknown() {
    let mut foreign = support("s1", "o1", "snap-a", None);
    foreign.proof_root = "foreign-root".into();
    let input = region(
        vec![locus(1, TruthClass::CompilerExact)],
        ClosureClass::Exact,
        vec![obligation("o1")],
        vec![foreign],
    );
    assert_eq!(
        classify_mechanical_region(&input),
        MechanicalGraphVerdict::Unknown
    );
}

#[test]
fn semantic_choice_gap_is_unsafe() {
    let mut input = region(
        vec![locus(1, TruthClass::CompilerExact)],
        ClosureClass::Exact,
        vec![obligation("o1")],
        vec![support("s1", "o1", "snap-a", None)],
    );
    input.gaps = vec![DecisionGap {
        kind: EvidenceKind::UnresolvedGap,
        reason: "ambiguous overload set".into(),
    }];
    assert_eq!(
        classify_mechanical_region(&input),
        MechanicalGraphVerdict::Unsafe
    );
}

#[test]
fn ambiguous_candidate_loci_are_unknown_not_unsafe() {
    let input = region(
        vec![
            locus(1, TruthClass::CompilerExact),
            locus(2, TruthClass::CompilerExact),
        ],
        ClosureClass::Exact,
        vec![obligation("o1")],
        vec![support("s1", "o1", "snap-a", None)],
    );
    assert_eq!(
        classify_mechanical_region(&input),
        MechanicalGraphVerdict::Unknown
    );
}

#[test]
fn missing_locus_is_unknown() {
    let input = region(
        vec![],
        ClosureClass::Exact,
        vec![obligation("o1")],
        vec![support("s1", "o1", "snap-a", None)],
    );
    assert_eq!(
        classify_mechanical_region(&input),
        MechanicalGraphVerdict::Unknown
    );
}
#[test]
fn unrooted_locus_is_unknown() {
    let mut unrooted = locus(1, TruthClass::CompilerExact);
    unrooted.premises = vec![];
    let input = region(
        vec![unrooted],
        ClosureClass::Exact,
        vec![obligation("o1")],
        vec![support("s1", "o1", "snap-a", None)],
    );
    assert_eq!(
        classify_mechanical_region(&input),
        MechanicalGraphVerdict::Unknown
    );
}
#[test]
fn underapproximated_fiber_is_unknown() {
    let mut input = region(
        vec![locus(1, TruthClass::CompilerExact)],
        ClosureClass::Exact,
        vec![obligation("o1")],
        vec![support("s1", "o1", "snap-a", None)],
    );
    input.fiber = FiberClass::Underapproximation;
    assert_eq!(
        classify_mechanical_region(&input),
        MechanicalGraphVerdict::Unknown
    );
}

#[test]
fn incomplete_impact_is_unknown() {
    let input = region(
        vec![locus(1, TruthClass::CompilerExact)],
        ClosureClass::Incomplete,
        vec![obligation("o1")],
        vec![support("s1", "o1", "snap-a", None)],
    );
    assert_eq!(
        classify_mechanical_region(&input),
        MechanicalGraphVerdict::Unknown
    );
}

#[test]
fn overapproximated_impact_is_unknown() {
    let input = region(
        vec![locus(1, TruthClass::CompilerExact)],
        ClosureClass::SoundOverapproximation,
        vec![obligation("o1")],
        vec![support("s1", "o1", "snap-a", None)],
    );
    assert_eq!(
        classify_mechanical_region(&input),
        MechanicalGraphVerdict::Unknown
    );
}

#[test]
fn bounded_reverse_impact_is_incomplete_when_budget_exceeded() {
    let edges = [
        edge("middle", "leaf"),
        edge("root", "middle"),
        edge("top", "root"),
    ];
    let report = report(3);
    let bounded = compiler_reverse_impact(["leaf".into()], &edges, &report, 2).unwrap();
    assert_eq!(bounded.closure, ClosureClass::Incomplete);
    assert_eq!(bounded.impacted, vec!["leaf", "middle", "root"]);
    let full = compiler_reverse_impact(["leaf".into()], &edges, &report, 3).unwrap();
    assert_eq!(full.closure, ClosureClass::Exact);
    assert_eq!(full.impacted, vec!["leaf", "middle", "root", "top"]);
    assert_eq!(full.freshness_root, "snap-a");
}

#[test]
fn edge_freshness_mismatch_is_rejected() {
    let mut stale = edge("middle", "leaf");
    stale.freshness_root = "snap-b".into();
    let err = compiler_reverse_impact(["leaf".into()], &[stale], &report(1), 10).unwrap_err();
    assert!(err.contains("freshness root"));
}

#[test]
fn all_obligation_kinds_are_finite_and_valid() {
    for kind in [
        TypedObligationKind::Decision,
        TypedObligationKind::Execution,
        TypedObligationKind::Verification,
        TypedObligationKind::Restoration,
    ] {
        let obligation = TypedObligation {
            id: "o1".into(),
            kind,
            protected_scope_root: "scope".into(),
            required_evidence_kinds: vec!["type".into()],
        };
        assert!(obligation.validate().is_ok());
    }
}
