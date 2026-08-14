//! V6-R6 acceptance: identity-kernel runtime wiring (ZS-KERNEL-003/006/008).
//!
//! Matrix: honest derivation accepted; tampered identity refused; invariant
//! violation loud with receipt; determinism (same input => same identity).

use std::fs::OpenOptions;
use std::io::Write;

use zero_abi::identity::EventClassV1;
use zero_abi::{
    DigestV1, PayloadFormationReceiptV1, ROOTED_ABI_VERSION_V6,
};
use zero_abi::cache_entry::{CacheKeyV1, CacheRootV1, CompletenessWitnessV1, OperatorIdentityV1};
use zero_cert::{
    CacheAdmissionGateV1, EVENT_JOURNAL_RECORDS_FILE_V1,
    EVENT_JOURNAL_SEALED_HEAD_FILE_V1, FileEventJournalStore, InMemoryJournalStore,
    JournalStore, KernelEventJournalV1, KernelRuntimeError, ProjectRootGateV1, RootGateFaultV1,
};

fn genesis() -> DigestV1 {
    zero_abi::event_log_genesis()
}

fn receipt(
    contract_root: DigestV1,
    dependencies: Vec<&str>,
    payload_root: &str,
) -> PayloadFormationReceiptV1 {
    PayloadFormationReceiptV1::new(
        "constructor:seed-42",
        contract_root,
        dependencies.into_iter().map(str::to_owned).collect(),
        "fz://blob/exec-1",
        payload_root,
        7,
    )
    .unwrap()
}

// ---------------------------------------------------------------------------
// ZS-KERNEL-006: durable event journal.
// ---------------------------------------------------------------------------

/// Honest derivation accepted: a journal replays its own chain and seals.
#[test]
fn journal_honest_chain_replays_and_seals() {
    let store = InMemoryJournalStore::new();
    let mut journal = KernelEventJournalV1::open(store).unwrap();
    assert_eq!(journal.head().unwrap(), genesis());
    assert!(journal.current_project_root().unwrap().is_none());

    journal
        .append(EventClassV1::EvidenceObservation, "fz://blob/ev-1", "verifier-a")
        .unwrap();
    journal
        .append(EventClassV1::Verification, "fz://blob/verif-1", "verifier-a")
        .unwrap();
    let sealed = journal.seal().unwrap();
    assert_eq!(sealed, journal.head().unwrap());
    assert_eq!(journal.verify_chain().unwrap(), sealed);

    // Persist the records into a second store and reopen: replay matches,
    // sealed head verifies.
    let records = journal.records().to_vec();
    let mut store = InMemoryJournalStore::new();
    for record in &records {
        store.persist_record(record).unwrap();
    }
    let mut reopened = KernelEventJournalV1::open(store).unwrap();
    assert_eq!(reopened.head().unwrap(), sealed);
    assert_eq!(reopened.records(), records.as_slice());
    assert_eq!(reopened.seal().unwrap(), sealed);
}

/// All nine authoritative event classes chain, including resource charges.
#[test]
fn journal_all_nine_event_classes_chain() {
    let mut journal = KernelEventJournalV1::open(InMemoryJournalStore::new()).unwrap();
    let payloads: Vec<(&str, &str)> = vec![
        ("state_transition", "fz://blob/state-1"),
        ("evidence_observation", "fz://blob/ev-1"),
        ("cache_decision", "fz://blob/cache-1"),
        ("execution", "fz://blob/exec-1"),
        ("verification", "fz://blob/verif-1"),
        ("authority_issuance", "fz://blob/permit-1"),
        ("commit", "fz://blob/commit-1"),
        ("rollback", "fz://blob/rollback-1"),
        ("resource_charge", "fz://blob/charge-1"),
    ];
    for (class, payload) in &payloads {
        let class = EventClassV1::from_str(class).unwrap();
        journal.append(class, *payload, "kernel").unwrap();
    }
    assert!(journal.verify_chain().is_ok(), "all classes must chain");

    // Every record carries one of the nine typed classes.
    for record in journal.records() {
        assert!(EventClassV1::from_str(&record.event_type).is_ok());
    }
}

/// Killed-process replay at the store level: a process that dies after
/// durable appends but before sealing replays its full prefix; a partial
/// final write is a torn tail and fails closed; tampered history fails
/// closed.
#[test]
fn file_journal_killed_process_replay_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEventJournalStore::new(dir.path());

    // Round 1: process A opens, appends three events, dies (no seal).
    let mut journal = KernelEventJournalV1::open(store.clone()).unwrap();
    journal
        .append(EventClassV1::Execution, "fz://blob/exec-1", "runner")
        .unwrap();
    journal
        .append(EventClassV1::Verification, "fz://blob/verif-1", "verifier")
        .unwrap();
    journal
        .append(EventClassV1::ResourceCharge, "fz://blob/charge-1", "ledger")
        .unwrap();
    let killed_head = journal.head().unwrap();
    drop(journal); // simulated kill: no seal call

    // Round 2: process B opens the same directory: the durable prefix
    // replays to the exact head process A saw.
    let reopened = KernelEventJournalV1::open(store.clone()).unwrap();
    assert_eq!(reopened.head().unwrap(), killed_head);
    assert_eq!(reopened.records().len(), 3);

    // Torn tail: simulate a kill mid-append (partial JSON line). The store
    // fails closed instead of replaying a corrupt record.
    {
        let mut file = OpenOptions::new()
            .append(true)
            .open(dir.path().join(EVENT_JOURNAL_RECORDS_FILE_V1))
            .unwrap();
        file.write_all(b"{\"seq\":3,\"parent_root\":\"").unwrap();
    }
    let torn = KernelEventJournalV1::open(store.clone());
    assert!(matches!(torn, Err(KernelRuntimeError::TornJournalTail { seq: 3 })));

    // Tampered identity: rewrite the middle record's payload root, keeping
    // valid JSON. Replay must fail closed on the broken chain.
    let path = dir.path().join(EVENT_JOURNAL_RECORDS_FILE_V1);
    let mut lines: Vec<String> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    lines.truncate(3); // drop the torn tail line
    let mut tampered: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();
    tampered["payload_root"] = serde_json::json!("fz://blob/tampered");
    lines[1] = serde_json::to_string(&tampered).unwrap();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    let tampered_open = KernelEventJournalV1::open(store.clone());
    // The tampered record chains against its own parent, so the break shows
    // up at the FOLLOWING record, whose parent no longer matches.
    assert!(matches!(
        tampered_open,
        Err(KernelRuntimeError::InvalidJournalRecord { seq: 2, .. })
    ));
}

/// Sealed-head verification: a sealed journal reopened after its history was
/// truncated (torn tail at the head level) fails with a head mismatch.
#[test]
fn file_journal_sealed_head_mismatch_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEventJournalStore::new(dir.path());

    let mut journal = KernelEventJournalV1::open(store.clone()).unwrap();
    journal
        .append(EventClassV1::Execution, "fz://blob/exec-1", "runner")
        .unwrap();
    journal
        .append(EventClassV1::Commit, "fz://blob/commit-1", "gate")
        .unwrap();
    let sealed = journal.seal().unwrap();
    drop(journal);

    // Reopen verifies against the sealed head.
    let reopened = KernelEventJournalV1::open(store.clone()).unwrap();
    assert_eq!(reopened.head().unwrap(), sealed);

    // Stale sealed head (as if the marker belonged to a different chain).
    std::fs::write(
        dir.path().join(EVENT_JOURNAL_SEALED_HEAD_FILE_V1),
        format!("{}\n", DigestV1::from_bytes([9; 32]).to_hex()),
    )
    .unwrap();
    let mismatch = KernelEventJournalV1::open(store.clone());
    assert!(matches!(
        mismatch,
        Err(KernelRuntimeError::JournalHeadMismatch { .. })
    ));
}

/// Determinism: identical inputs produce identical records and identical
/// journal heads across independent journals.
#[test]
fn journal_determinism_same_inputs_same_identity() {
    let mut a = KernelEventJournalV1::open(InMemoryJournalStore::new()).unwrap();
    let mut b = KernelEventJournalV1::open(InMemoryJournalStore::new()).unwrap();
    for class in EventClassV1::ALL {
        a.append(class, "fz://blob/p", "kernel").unwrap();
        b.append(class, "fz://blob/p", "kernel").unwrap();
    }
    assert_eq!(a.head().unwrap(), b.head().unwrap());
    assert_eq!(a.records(), b.records());
    assert_eq!(a.seal().unwrap(), b.seal().unwrap());
}

// ---------------------------------------------------------------------------
// ZS-KERNEL-008: project-root gate.
// ---------------------------------------------------------------------------

/// Honest derivation accepted: verify -> authorize -> commit advances the
/// CAS, emits a rooted successor receipt and a Commit journal event whose
/// payload is the new project root.
#[test]
fn project_gate_honest_commit_advances_with_receipt() {
    let mut journal = KernelEventJournalV1::open(InMemoryJournalStore::new()).unwrap();
    let mut gate = ProjectRootGateV1::new(genesis(), "transaction-gate").unwrap();
    let successor = DigestV1::from_bytes([7; 32]);

    let mut session = gate.verify(genesis(), successor).unwrap();
    assert!(!session.is_authorized());
    gate.authorize(&mut session).unwrap();
    assert!(session.is_authorized());

    let receipt = gate.commit(session, &mut journal).unwrap();
    assert!(receipt.advanced);
    assert_eq!(receipt.verified_successor_root, successor);
    assert_eq!(gate.current(), successor);

    // The journal carries the commit; recovery sees the new root.
    assert_eq!(journal.current_project_root().unwrap(), Some(successor));
    let last = journal.records().last().unwrap();
    assert_eq!(last.event_type, EventClassV1::Commit.as_str());
    assert_eq!(last.payload_root, successor.to_hex());

    // Receipt is rooted and canonical.
    let root = receipt.record_root().unwrap();
    assert!(zero_abi::verify_object_root(
        zero_abi::ObjectClassV1::SuccessorRecord,
        ROOTED_ABI_VERSION_V6,
        &receipt.canonical_bytes().unwrap(),
        root
    ));
}

/// Stale project handle: invariant violation is loud and carries an
/// unchanged-successor receipt; the CAS never mutates.
#[test]
fn project_gate_stale_handle_fails_loud_with_receipt() {
    let mut journal = KernelEventJournalV1::open(InMemoryJournalStore::new()).unwrap();
    let mut gate = ProjectRootGateV1::new(genesis(), "transaction-gate").unwrap();
    let successor = DigestV1::from_bytes([7; 32]);
    let next = DigestV1::from_bytes([8; 32]);

    // First commit advances.
    let mut session = gate.verify(genesis(), successor).unwrap();
    gate.authorize(&mut session).unwrap();
    gate.commit(session, &mut journal).unwrap();

    // Stale handle: declared parent is the pre-commit root.
    match gate.verify(genesis(), next) {
        Err(KernelRuntimeError::StaleProjectHandle {
            declared_parent,
            current,
            receipt,
        }) => {
            assert_eq!(declared_parent, genesis());
            assert_eq!(current, successor);
            assert!(!receipt.advanced, "violation receipt must be unchanged");
            assert_eq!(receipt.declared_parent_root, genesis());
        }
        other => panic!("expected StaleProjectHandle, got {other:?}"),
    }
    assert_eq!(gate.current(), successor, "CAS must not mutate on a violation");

    // No verified change: successor equals current.
    match gate.verify(successor, successor) {
        Err(KernelRuntimeError::NoVerifiedChange { receipt }) => {
            assert!(!receipt.advanced);
        }
        other => panic!("expected NoVerifiedChange, got {other:?}"),
    }
    assert_eq!(gate.current(), successor);
}

/// Commit without authorize is loud and mutates nothing.
#[test]
fn project_gate_commit_without_authorize_fails_loud() {
    let mut journal = KernelEventJournalV1::open(InMemoryJournalStore::new()).unwrap();
    let mut gate = ProjectRootGateV1::new(genesis(), "transaction-gate").unwrap();
    let successor = DigestV1::from_bytes([7; 32]);

    let session = gate.verify(genesis(), successor).unwrap();
    let error = gate.commit(session, &mut journal).unwrap_err();
    assert!(matches!(error, KernelRuntimeError::Unauthorized { .. }));
    assert_eq!(gate.current(), genesis());
    assert!(journal.records().is_empty());
}

/// Runtime fault injection around verify/authorize/commit: a crash before
/// commit mutates nothing; an authorization refusal blocks commit; a crash
/// after commit is recoverable from the journal.
#[test]
fn project_gate_fault_injection_crash_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEventJournalStore::new(dir.path());
    let successor = DigestV1::from_bytes([7; 32]);

    // CrashBeforeCommit: the process dies at the commit boundary.
    let mut journal = KernelEventJournalV1::open(store.clone()).unwrap();
    let mut gate = ProjectRootGateV1::new(genesis(), "transaction-gate")
        .unwrap()
        .with_fault(RootGateFaultV1::CrashBeforeCommit);
    let mut session = gate.verify(genesis(), successor).unwrap();
    gate.authorize(&mut session).unwrap();
    let error = gate.commit(session, &mut journal).unwrap_err();
    assert_eq!(error, KernelRuntimeError::FaultInjected { phase: "commit" });
    assert_eq!(gate.current(), genesis(), "crash before commit leaves old root");
    assert!(journal.records().is_empty(), "no journal event before commit");

    // AuthorizationRefused: the session can never reach commit.
    let mut gate = ProjectRootGateV1::new(genesis(), "transaction-gate")
        .unwrap()
        .with_fault(RootGateFaultV1::AuthorizationRefused);
    let mut session = gate.verify(genesis(), successor).unwrap();
    let error = gate.authorize(&mut session).unwrap_err();
    assert_eq!(
        error,
        KernelRuntimeError::FaultInjected { phase: "authorize" }
    );
    assert!(!session.is_authorized());

    // Crash after commit: commit succeeds (event durably journaled), then
    // the process dies; recovery from the journal sees the complete new root.
    let mut journal = KernelEventJournalV1::open(store.clone()).unwrap();
    let mut gate = ProjectRootGateV1::new(genesis(), "transaction-gate").unwrap();
    let mut session = gate.verify(genesis(), successor).unwrap();
    gate.authorize(&mut session).unwrap();
    gate.commit(session, &mut journal).unwrap();
    assert_eq!(gate.current(), successor);
    drop(gate);
    drop(journal); // simulated kill after commit

    let recovered_journal = KernelEventJournalV1::open(store).unwrap();
    assert_eq!(
        recovered_journal.current_project_root().unwrap(),
        Some(successor)
    );
    let recovered_gate =
        ProjectRootGateV1::from_journal(&recovered_journal, "transaction-gate").unwrap();
    assert_eq!(recovered_gate.current(), successor);

    // A stale handle from before the crash is refused after recovery.
    match recovered_gate.verify(genesis(), DigestV1::from_bytes([8; 32])) {
        Err(KernelRuntimeError::StaleProjectHandle {
            declared_parent,
            current,
            receipt,
        }) => {
            assert_eq!(declared_parent, genesis());
            assert_eq!(current, successor);
            assert!(!receipt.advanced);
        }
        other => panic!("expected StaleProjectHandle, got {other:?}"),
    }
    assert_eq!(recovered_gate.current(), successor);
}

// ---------------------------------------------------------------------------
// ZS-KERNEL-003: cache admission gate.
// ---------------------------------------------------------------------------

/// Honest derivation accepted and deterministic: an exact receipt admits,
/// and identical inputs produce identical decision record roots.
#[test]
fn cache_admission_honest_admitted_and_deterministic() {
    let contract_root = DigestV1::from_bytes([1; 32]);
    let formation = receipt(
        contract_root,
        vec!["fz://blob/dep-a", "fz://blob/dep-b"],
        "fz://blob/payload-A",
    );
    let sealed = formation.receipt_root().unwrap();

    let decision = CacheAdmissionGateV1::decide(
        &formation,
        sealed,
        contract_root,
        "fz://blob/payload-A",
        &["fz://blob/dep-a".to_owned(), "fz://blob/dep-b".to_owned()],
    )
    .unwrap();
    assert!(decision.admitted, "honest receipt must admit: {decision:?}");
    assert!(decision.reason.is_empty());
    assert_eq!(decision.receipt_root, sealed.to_hex());

    // Determinism: same inputs, same decision root (order-insensitive deps).
    let again = CacheAdmissionGateV1::decide(
        &formation,
        sealed,
        contract_root,
        "fz://blob/payload-A",
        &["fz://blob/dep-b".to_owned(), "fz://blob/dep-a".to_owned()],
    )
    .unwrap();
    assert!(again.admitted);
    assert_eq!(again.record_root(), decision.record_root());

    // Decision records journal as CacheDecision events and chain.
    let mut journal = KernelEventJournalV1::open(InMemoryJournalStore::new()).unwrap();
    journal.append_cache_decision(&decision).unwrap();
    assert!(journal.verify_chain().is_ok());
}

/// Tampered identity refused: a receipt whose canonical bytes no longer
/// match the sealed formation root is refused at the authority boundary.
#[test]
fn cache_admission_tampered_receipt_refused() {
    let contract_root = DigestV1::from_bytes([1; 32]);
    let formation = receipt(contract_root, vec!["fz://blob/dep-a"], "fz://blob/payload-A");
    let sealed = formation.receipt_root().unwrap();

    // Tamper the epoch on the wire, then deserialize: the struct now carries
    // different bytes than the one the authority sealed.
    let mut tampered_wire = serde_json::to_value(&formation).unwrap();
    tampered_wire["epoch"] = serde_json::json!(99);
    let tampered: PayloadFormationReceiptV1 = serde_json::from_value(tampered_wire).unwrap();
    assert_ne!(
        tampered.canonical_bytes().unwrap(),
        formation.canonical_bytes().unwrap()
    );

    let decision = CacheAdmissionGateV1::decide(
        &tampered,
        sealed,
        contract_root,
        "fz://blob/payload-A",
        &["fz://blob/dep-a".to_owned()],
    )
    .unwrap();
    assert!(!decision.admitted);
    assert!(decision.reason.contains("receipt root mismatch"));

    // Registry mismatch: the authority's sealed root is not the receipt's.
    let wrong_sealed = DigestV1::from_bytes([9; 32]);
    let decision = CacheAdmissionGateV1::decide(
        &formation,
        wrong_sealed,
        contract_root,
        "fz://blob/payload-A",
        &["fz://blob/dep-a".to_owned()],
    )
    .unwrap();
    assert!(!decision.admitted);
}

/// Relabeled payload refused: the payload/contract bindings must hold.
#[test]
fn cache_admission_relabeled_payload_refused() {
    let contract_root = DigestV1::from_bytes([1; 32]);
    let formation = receipt(contract_root, vec!["fz://blob/dep-a"], "fz://blob/payload-A");
    let sealed = formation.receipt_root().unwrap();

    let decision = CacheAdmissionGateV1::decide(
        &formation,
        sealed,
        contract_root,
        "fz://blob/payload-B",
        &["fz://blob/dep-a".to_owned()],
    )
    .unwrap();
    assert!(!decision.admitted);
    assert!(decision.reason.contains("relabeled payload"));

    let decision = CacheAdmissionGateV1::decide(
        &formation,
        sealed,
        DigestV1::from_bytes([2; 32]),
        "fz://blob/payload-A",
        &["fz://blob/dep-a".to_owned()],
    )
    .unwrap();
    assert!(!decision.admitted);
}

/// Dependency mutation revokes reuse: any added, removed, or changed current
/// dependency root refuses admission, and every refusal is journaled as a
/// CacheDecision event.
#[test]
fn cache_admission_dependency_mutation_revokes_reuse() {
    let contract_root = DigestV1::from_bytes([1; 32]);
    let formation = receipt(
        contract_root,
        vec!["fz://blob/dep-a", "fz://blob/dep-b"],
        "fz://blob/payload-A",
    );
    let sealed = formation.receipt_root().unwrap();
    let mut journal = KernelEventJournalV1::open(InMemoryJournalStore::new()).unwrap();

    let refuse = |current: Vec<&str>| {
        CacheAdmissionGateV1::decide(
            &formation,
            sealed,
            contract_root,
            "fz://blob/payload-A",
            &current.into_iter().map(str::to_owned).collect::<Vec<_>>(),
        )
        .unwrap()
    };

    // Changed dependency.
    let decision = refuse(vec!["fz://blob/dep-a", "fz://blob/dep-c"]);
    assert!(!decision.admitted);
    assert!(decision.reason.contains("dependency roots mutated"));
    journal.append_cache_decision(&decision).unwrap();

    // Removed dependency.
    let decision = refuse(vec!["fz://blob/dep-a"]);
    assert!(!decision.admitted);
    journal.append_cache_decision(&decision).unwrap();

    // Added dependency.
    let decision = refuse(vec![
        "fz://blob/dep-a",
        "fz://blob/dep-b",
        "fz://blob/dep-c",
    ]);
    assert!(!decision.admitted);
    journal.append_cache_decision(&decision).unwrap();

    // The refusal trail is a valid chain (honest identity preserved).
    assert!(journal.verify_chain().is_ok());
    assert!(journal
        .records()
        .iter()
        .all(|r| r.event_type == EventClassV1::CacheDecision.as_str()));
}

/// The zero-store `cache_entry` mapping: a candidate CacheKeyV1's minimum
/// dependency roots are the current dependency set for the gate.
#[test]
fn cache_admission_for_cache_key_maps_dependency_roots() {
    let contract_root = DigestV1::from_bytes([1; 32]);
    let formation = receipt(contract_root, vec!["fz://blob/dep-a"], "fz://blob/payload-A");
    let sealed = formation.receipt_root().unwrap();

    let dep_a = CacheRootV1::new("fz://blob/dep-a").unwrap();
    let witness = CompletenessWitnessV1::new(
        CacheRootV1::new("fz://blob/proof").unwrap(),
        vec![dep_a.clone()],
    )
    .unwrap();
    let key = CacheKeyV1::new(
        OperatorIdentityV1::new("operator", "1.0").unwrap(),
        serde_json::json!({"query": 1}),
        vec![dep_a.clone()],
        vec![],
        vec![],
        witness,
    )
    .unwrap();

    let decision = CacheAdmissionGateV1::decide_for_cache_key(
        &formation,
        sealed,
        contract_root,
        "fz://blob/payload-A",
        &key,
    )
    .unwrap();
    assert!(decision.admitted);

    // A key depending on an extra root is refused: dependency set changed.
    let dep_c = CacheRootV1::new("fz://blob/dep-c").unwrap();
    let witness = CompletenessWitnessV1::new(
        CacheRootV1::new("fz://blob/proof").unwrap(),
        vec![dep_a.clone(), dep_c.clone()],
    )
    .unwrap();
    let mutated = CacheKeyV1::new(
        OperatorIdentityV1::new("operator", "1.0").unwrap(),
        serde_json::json!({"query": 1}),
        vec![dep_a, dep_c],
        vec![],
        vec![],
        witness,
    )
    .unwrap();
    let decision = CacheAdmissionGateV1::decide_for_cache_key(
        &formation,
        sealed,
        contract_root,
        "fz://blob/payload-A",
        &mutated,
    )
    .unwrap();
    assert!(!decision.admitted);
}
