//! Static module-boundary audit and replayed-authority tests (ZS-KERNEL-005 /
//! V6-R14): the sealed audit registry declares every authority artifact's
//! construction surface; the compile_fail doctests in
//! `crates/zero-cert/src/boundary_audit.rs` prove role code cannot construct
//! private-state authority; and captured-epoch replay is refused -- an
//! authority captured at epoch N replayed after the root advanced fails
//! loud with no journal event and no CAS mutation.

use zero_abi::identity::EventClassV1;
use zero_abi::{
    DigestV1, PayloadFormationReceiptV1, ROOTED_ABI_VERSION_V6,
};
use zero_cert::{
    AuthorityBoundaryAuditReportV1, CacheAdmissionGateV1, CacheAdmissionRecordV1,
    ConstructionSurfaceV1, InMemoryJournalStore, KernelEventJournalV1, KernelRuntimeError,
    ProjectRootGateV1, authority_boundary_audit_v1, verify_commit_authority_v1,
    verify_decision_authority_v1,
};

fn digest(byte: u8) -> DigestV1 {
    DigestV1::from_bytes([byte; 32])
}

fn genesis() -> DigestV1 {
    zero_abi::event_log_genesis()
}

fn receipt(contract_root: DigestV1, epoch: u64) -> PayloadFormationReceiptV1 {
    PayloadFormationReceiptV1::new(
        "constructor:seed-42",
        contract_root,
        vec!["dep-a".to_owned()],
        "fz://blob/exec-1",
        "fz://blob/payload-1",
        epoch,
    )
    .unwrap()
}

/// Commit one successor through the gate; returns the sealed successor
/// record and the journal.
fn commit_next(
    journal: &mut KernelEventJournalV1<InMemoryJournalStore>,
    gate: &mut ProjectRootGateV1,
    successor: DigestV1,
) -> zero_abi::SuccessorRecordV1 {
    let parent = gate.current();
    let mut session = gate.verify(parent, successor).unwrap();
    gate.authorize(&mut session).unwrap();
    gate.commit(session, journal).unwrap()
}

/// The static audit is a sealed registry: every authority artifact is
/// listed with its construction surface; private-state authority is
/// role-unconstructible; serializable records are honestly marked
/// public-with-verifier. The digest anchors the registry.
#[test]
fn static_audit_registry_seals_construction_surfaces() {
    let report: AuthorityBoundaryAuditReportV1 = authority_boundary_audit_v1();
    assert!(report.invariant.contains("cannot construct authority objects"));
    assert_eq!(report.entries.len(), 5);

    let surfaces: std::collections::HashMap<_, _> = report
        .entries
        .iter()
        .map(|entry| (entry.authority_type.as_str(), entry))
        .collect();

    let verified_evidence = surfaces["zero_cert::VerifiedEvidence"];
    assert_eq!(verified_evidence.construction_surface, ConstructionSurfaceV1::PrivateFields);
    assert!(!verified_evidence.role_constructible);

    let session = surfaces["zero_cert::RootGateSessionV1"];
    assert_eq!(session.construction_surface, ConstructionSurfaceV1::PrivateFields);
    assert!(!session.role_constructible);

    let decision = surfaces["zero_cert::CacheAdmissionRecordV1"];
    match &decision.construction_surface {
        ConstructionSurfaceV1::PublicArtifact { verified_by } => {
            assert!(verified_by.contains("CacheAdmissionGateV1::decide"));
        }
        other => panic!("expected PublicArtifact, got {other:?}"),
    }
    assert!(decision.role_constructible, "serializable record: construction is public, authority is not");

    let commit = surfaces["zero_abi::SuccessorRecordV1"];
    match &commit.construction_surface {
        ConstructionSurfaceV1::PublicArtifact { verified_by } => {
            assert!(verified_by.contains("ProjectRootGateV1::commit"));
        }
        other => panic!("expected PublicArtifact, got {other:?}"),
    }

    // The registry is deterministic and tamper-sensitive.
    let digest = report.digest().unwrap();
    assert_ne!(digest, DigestV1::ZERO);
    assert_eq!(authority_boundary_audit_v1().digest().unwrap(), digest);
}

/// Captured-epoch replay, commit authority: the successor record captured
/// at epoch N is refused after the root advanced to epoch N+1 -- a replayed
/// authority fails loud and mutates nothing.
#[test]
fn captured_epoch_successor_replay_is_refused() {
    let mut journal = KernelEventJournalV1::open(InMemoryJournalStore::new()).unwrap();
    let mut gate = ProjectRootGateV1::new(genesis(), "gate-a").unwrap();

    // Epoch 1: commit root R1 from genesis. Capture the authority record.
    let r1 = digest(0x11);
    let captured = commit_next(&mut journal, &mut gate, r1);
    assert_eq!(captured.verified_successor_root, r1);
    assert_eq!(gate.current(), r1);
    assert_eq!(journal.current_project_root().unwrap(), Some(r1));
    assert_eq!(journal.records().len(), 1);

    // The captured record is genuinely authoritative at epoch 1: the
    // journal's last Commit event carries its exact root.
    verify_commit_authority_v1(&journal, r1).unwrap();

    // Epoch 2: the root advances to R2.
    let r2 = digest(0x22);
    commit_next(&mut journal, &mut gate, r2);
    assert_eq!(gate.current(), r2);
    assert_eq!(journal.current_project_root().unwrap(), Some(r2));
    assert_eq!(journal.records().len(), 2);

    // REPLAY: present the captured epoch-1 authority (parent genesis ->
    // R1) at epoch 2. The gate refuses: stale project handle, loud, with an
    // unchanged-successor receipt.
    let error = gate.verify(genesis(), r1).unwrap_err();
    match error {
        KernelRuntimeError::StaleProjectHandle {
            declared_parent,
            current,
            receipt,
        } => {
            assert_eq!(declared_parent, genesis());
            assert_eq!(current, r2);
            assert!(!receipt.advanced);
        }
        other => panic!("expected stale-handle refusal, got {other:?}"),
    }

    // The read-side audit agrees: the captured root is no longer the
    // journal's committed root.
    assert!(matches!(
        verify_commit_authority_v1(&journal, r1),
        Err(zero_cert::BoundaryAuditErrorV1::UnauthorizedCommit { .. })
    ));

    // Nothing mutated: root and journal are exactly the epoch-2 state.
    assert_eq!(gate.current(), r2);
    assert_eq!(journal.current_project_root().unwrap(), Some(r2));
    assert_eq!(journal.records().len(), 2);
    assert_eq!(
        journal
            .records()
            .iter()
            .filter(|record| record.event_type == EventClassV1::Commit.as_str())
            .count(),
        2
    );
}

/// Captured-epoch replay, cache authority: a cache admission record sealed
/// at epoch N is refused when a dependency root changed before the replay,
/// and a record the journal never issued has no authority at all.
#[test]
fn captured_epoch_cache_decision_replay_is_refused() {
    let mut journal = KernelEventJournalV1::open(InMemoryJournalStore::new()).unwrap();
    let contract_root = digest(0x31);

    // Epoch N: a cache admission is issued for dependency set [dep-a].
    let receipt = receipt(contract_root, 7);
    let receipt_root = receipt.receipt_root().unwrap();
    let dependencies_n = vec!["dep-a".to_owned()];
    let record_n = CacheAdmissionGateV1::decide(
        &receipt,
        receipt_root,
        contract_root,
        "fz://blob/payload-1",
        &dependencies_n,
    )
    .unwrap();
    assert!(record_n.admitted);
    journal
        .append_cache_decision(&record_n)
        .expect("journal the epoch-N decision");

    // Read-side authority at epoch N: the journal carries the exact record
    // root.
    verify_decision_authority_v1(&journal, &record_n).unwrap();

    // REPLAY at a later epoch with a mutated dependency root: the gate
    // refuses with a sealed decision record (dependency roots mutated).
    let dependencies_later = vec!["dep-a".to_owned(), "dep-b".to_owned()];
    let replayed = CacheAdmissionGateV1::decide(
        &receipt,
        receipt_root,
        contract_root,
        "fz://blob/payload-1",
        &dependencies_later,
    )
    .unwrap();
    assert!(!replayed.admitted);
    assert!(replayed.reason.contains("dependency roots mutated"));

    // The replay was journaled as its own refused decision -- the original
    // epoch-N admission is still authoritative for its own epoch.
    journal.append_cache_decision(&replayed).unwrap();
    verify_decision_authority_v1(&journal, &record_n).unwrap();
    verify_decision_authority_v1(&journal, &replayed).unwrap();

    // A FORGED record -- same shape as the epoch-N admission but with
    // content the gate never issued (a dependency set it refused) -- has no
    // authority: its root never appears in the journal. Note that a
    // bit-exact copy of an issued record IS the issued record (the root is
    // content-derived over the same bytes); forgery is any content the gate
    // never produced, which the journal root check refuses.
    let forged = CacheAdmissionRecordV1 {
        record_version: 1,
        admitted: true,
        reason: String::new(),
        receipt_root: receipt_root.to_hex(),
        contract_root: contract_root.to_hex(),
        payload_root: "fz://blob/payload-1".to_owned(),
        dependency_roots: vec!["dep-x".to_owned()], // never issued by the gate
        abi_version: ROOTED_ABI_VERSION_V6.to_owned(),
    };
    assert_ne!(forged.record_root(), record_n.record_root(), "the forged record must not copy the issued root");
    assert!(matches!(
        verify_decision_authority_v1(&journal, &forged),
        Err(zero_cert::BoundaryAuditErrorV1::UnauthorizedDecision { .. })
    ));
}

/// The compile-time boundary is enforced by the crate layout itself:
/// private-field authority types have no public constructor, and the
/// compile_fail doctests in `boundary_audit.rs` (RootGateSessionV1,
/// VerifiedEvidence) prove role code cannot construct them. This runtime
/// test confirms the guards also reject forgeries when called with
/// untrusted inputs.
#[test]
fn guards_reject_untrusted_inputs_at_runtime() {
    let journal = KernelEventJournalV1::open(InMemoryJournalStore::new()).unwrap();
    let gate = ProjectRootGateV1::new(genesis(), "gate-a").unwrap();

    // A forged successor claim (parent == current but no verified change)
    // is refused loudly.
    let error = gate.verify(genesis(), genesis()).unwrap_err();
    assert!(matches!(error, KernelRuntimeError::NoVerifiedChange { .. }));

    // Commit without an authorized session is refused: authority is
    // short-lived and scoped to one verify -> authorize -> commit chain.
    // (RootGateSessionV1 cannot be constructed externally -- see compile_fail
    // doctest -- so the only way to reach commit without authorization is
    // the gate's own fault, covered by kernel_runtime tests.)

    // A decision record built from a tampered receipt root is refused by
    // the gate.
    let contract_root = digest(0x41);
    let receipt = receipt(contract_root, 9);
    let record = CacheAdmissionGateV1::decide(
        &receipt,
        digest(0x99), // not the sealed receipt root
        contract_root,
        "fz://blob/payload-1",
        &["dep-a".to_owned()],
    )
    .unwrap();
    assert!(!record.admitted);
    assert!(record.reason.contains("not the sealed formation receipt"));

    let _ = journal;
}
