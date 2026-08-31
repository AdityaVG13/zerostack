use graphzero_core::*;
use graphzero_types::ContentHash;

#[test]
fn compiler_gaps_prevent_exact_closure() {
    let report = SemanticExtractionReport {
        language: "typescript".into(),
        compiler_root: "compiler".into(),
        configuration_root: "config".into(),
        freshness_root: "snap".into(),
        indexed_files: 10,
        resolved_edges: 20,
        unresolved_sites: vec!["dynamic import".into()],
        fatal_diagnostics: vec![],
    };
    assert_eq!(report.closure_class(), ClosureClass::SoundOverapproximation);
}

#[test]
fn compiler_reverse_impact_follows_resolved_dependents() {
    let report = SemanticExtractionReport {
        language: "typescript".into(),
        compiler_root: "compiler".into(),
        configuration_root: "config".into(),
        freshness_root: "snap".into(),
        indexed_files: 2,
        resolved_edges: 2,
        unresolved_sites: vec![],
        fatal_diagnostics: vec![],
    };
    let edge = |from: &str, to: &str, line| CompilerSemanticEdge {
        from: from.into(),
        to: to.into(),
        kind: CompilerEdgeKind::Call,
        language: "typescript".into(),
        compiler_root: "compiler".into(),
        configuration_root: "config".into(),
        freshness_root: "snap".into(),
        source_path: "src/index.ts".into(),
        source_line: line,
        source_column: 1,
    };
    let impact = compiler_reverse_impact(
        ["leaf".into()],
        &[edge("middle", "leaf", 1), edge("root", "middle", 2)],
        &report,
        2,
    )
    .unwrap();
    assert_eq!(impact.closure, ClosureClass::Exact);
    assert_eq!(impact.impacted, vec!["leaf", "middle", "root"]);
    assert_eq!(impact.freshness_root, "snap");
}

#[test]
fn tombstone_overlay_preserves_historical_edge() {
    let edge = TemporalEdge {
        id: "e1".into(),
        from: "a".into(),
        to: "b".into(),
        relation: "calls".into(),
        provenance_root: "root".into(),
        valid_from_epoch: 1,
        valid_to_epoch: None,
        supersedes: None,
    };
    let fused = fuse_edge_overlay(
        std::slice::from_ref(&edge),
        &[EdgeDelta::Tombstone {
            edge_id: "e1".into(),
            epoch: 3,
        }],
    )
    .unwrap();
    assert!(fused[0].live_at(2));
    assert!(!fused[0].live_at(3));
}

#[test]
fn exact_verified_region_is_mechanical() {
    let obligation = TypedObligation {
        id: "o1".into(),
        kind: TypedObligationKind::Verification,
        protected_scope_root: "proof".into(),
        required_evidence_kinds: vec!["type".into()],
    };
    let support = ProofSupportHyperedge {
        id: "s1".into(),
        obligation_id: "o1".into(),
        sources: vec!["src/index.ts".into()],
        target: "cert".into(),
        proof_root: "proof".into(),
        verifier_contract_root: "verifier".into(),
        snapshot_root: "snap".into(),
        provenance_root: "provenance".into(),
        valid_from_epoch: 1,
        valid_to_epoch: None,
    };
    assert_eq!(
        classify_mechanical_region(&MechanicalRegionInput {
            truth: TruthClass::CompilerExact,
            fiber: FiberClass::Exact,
            gaps: vec![],
            independently_verified: true,
            loci: vec![LocusRank {
                node: NodeId(ContentHash([1; 32])),
                score: 1,
                truth: TruthClass::CompilerExact,
                premises: vec!["evidence-ref".into()],
            }],
            impact_closure: ClosureClass::Exact,
            obligations: vec![obligation],
            supports: vec![support],
            snapshot_root: "snap".into(),
            epoch: 5,
        }),
        MechanicalGraphVerdict::Safe
    );
}
