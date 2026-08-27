//! Phase 7: weak evidence cannot close.

use super::*;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("TokenZero repo root")
}

fn obligation(hash: &str, path: &str, status: ProofStatus) -> ProofObligation {
    ProofObligation {
        kind: ProofKind::ProptestInvariant,
        evidence_ref: ArtifactRef {
            path: PathBuf::from(path),
            hash: hash.to_string(),
            schema_version: "tokenzero.test.v1".to_string(),
        },
        status,
        notes: None,
    }
}

fn catalog_with(obl: ProofObligation) -> InvariantCatalog {
    InvariantCatalog::new(vec![ParityInvariant {
        invariant_id: InvariantId("INV-TZ-TEST-001".to_string()),
        statement: "synthetic".to_string(),
        assumptions: Vec::new(),
        linked_feature_ids: vec![FeatureId("F-TZ-021".to_string())],
        proof_obligations: vec![obl],
    }])
}

#[test]
fn todo_hash_cannot_close() {
    let root = repo_root();
    let catalog = catalog_with(obligation(
        "TODO",
        "tests/unit/tokenzero-test-support/invariant_catalog_tests.rs",
        ProofStatus::Satisfied,
    ));
    let status = catalog.contract_status(&root);
    assert_eq!(status, ContractStatus::FailInvalidReferences);
    assert_ne!(
        close_decision(status, BaseGate::Allowed),
        CloseDecision::Close
    );
    assert!(
        catalog
            .validate(&root)
            .iter()
            .any(|v| matches!(v, CatalogViolation::TodoHash(_))),
        "TODO hash must be a violation, got {:?}",
        catalog.validate(&root)
    );
}

#[test]
fn empty_hash_on_satisfied_cannot_close() {
    let root = repo_root();
    let catalog = catalog_with(obligation(
        "",
        "tests/unit/tokenzero-test-support/invariant_catalog_tests.rs",
        ProofStatus::Satisfied,
    ));
    assert_eq!(
        catalog.contract_status(&root),
        ContractStatus::FailInvalidReferences
    );
}

#[test]
fn missing_file_cannot_close() {
    let root = repo_root();
    let catalog = catalog_with(obligation(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "tests/artifacts/does-not-exist.json",
        ProofStatus::Satisfied,
    ));
    assert_eq!(
        catalog.contract_status(&root),
        ContractStatus::FailMissingEvidence
    );
    assert_ne!(
        close_decision(ContractStatus::FailMissingEvidence, BaseGate::Allowed),
        CloseDecision::Close
    );
}

#[test]
fn pending_does_not_round_up_to_pass() {
    let root = repo_root();
    let catalog = catalog_with(obligation(
        "",
        "tests/unit/tokenzero-test-support/invariant_catalog_tests.rs",
        ProofStatus::Pending,
    ));
    assert!(!ProofStatus::Pending.is_met());
    assert_eq!(
        catalog.contract_status(&root),
        ContractStatus::FailMissingEvidence
    );
    match close_decision(catalog.contract_status(&root), BaseGate::Allowed) {
        CloseDecision::Block {
            reason: "contract-missing-evidence",
            ..
        } => {}
        other => panic!("Pending must not close, got {other:?}"),
    }
}

#[test]
fn only_pass_and_allowed_closes() {
    assert_eq!(
        close_decision(ContractStatus::Pass, BaseGate::Allowed),
        CloseDecision::Close
    );
    for contract in [
        ContractStatus::FailMissingEvidence,
        ContractStatus::FailInvalidReferences,
        ContractStatus::FailMixed,
    ] {
        assert_ne!(
            close_decision(contract, BaseGate::Allowed),
            CloseDecision::Close
        );
    }
    assert_ne!(
        close_decision(ContractStatus::Pass, BaseGate::BlockedByBaseGate),
        CloseDecision::Close
    );
}

#[test]
fn live_catalog_closes_when_pattern_65_is_satisfied() {
    let root = repo_root();
    let mut catalog = InvariantCatalog::tokenzero_phase7();
    assert!(unique_invariant_ids(&catalog));
    seal_satisfied_hashes(&mut catalog, &root);
    let pending = catalog
        .invariants()
        .iter()
        .filter(|inv| {
            inv.proof_obligations
                .iter()
                .any(|o| o.status == ProofStatus::Pending)
        })
        .count();
    assert_eq!(pending, 0, "Pattern 65 subprocess abort must be Satisfied");
    let status = catalog.contract_status(&root);
    assert_eq!(status, ContractStatus::Pass);
    assert_eq!(
        close_decision(status, BaseGate::Allowed),
        CloseDecision::Close
    );
    let satisfied_violations: Vec<_> = catalog
        .validate(&root)
        .into_iter()
        .filter(|v| {
            !matches!(
                v,
                CatalogViolation::EmptyHash(_) | CatalogViolation::TodoHash(_)
            )
        })
        .collect();
    assert!(
        satisfied_violations.is_empty(),
        "sealed Satisfied drivers must resolve: {satisfied_violations:?}"
    );
}

#[test]
fn target_dir_and_escape_are_invalid() {
    let root = repo_root();
    let catalog = catalog_with(obligation(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "target/debug/proof.json",
        ProofStatus::Satisfied,
    ));
    assert!(
        catalog
            .validate(&root)
            .iter()
            .any(|v| matches!(v, CatalogViolation::TargetDirPath(_)))
    );
    let catalog = catalog_with(obligation(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "../../etc/passwd",
        ProofStatus::Satisfied,
    ));
    assert!(
        catalog
            .validate(&root)
            .iter()
            .any(|v| matches!(v, CatalogViolation::PathEscape(_)))
    );
}
