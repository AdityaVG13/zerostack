use graphzero_core::{CoverageClass, grades::*};

fn ledger_with(artifact: &str, grade: GradeName) -> GradeLedger {
    let mut ledger = GradeLedger::new();
    ledger.declare_grade(artifact, grade).unwrap();
    ledger
}

#[test]
fn evidence_free_upgrade_rejected() {
    let mut ledger = ledger_with("a", GradeName::ObservedOnly);
    // Literal construction with an empty receipt digest -- no evidence.
    let err = ledger
        .upgrade(
            "a",
            GradeName::Complete,
            GradeEvidence::VerificationReceipt {
                digest: String::new(),
            },
            "alice",
        )
        .unwrap_err();
    assert_eq!(err, GradeError::EvidenceRequired);
    // Constructors reject empty references up front too.
    assert_eq!(
        GradeEvidence::verification_receipt("").unwrap_err(),
        GradeError::EvidenceRequired
    );
    assert_eq!(
        GradeEvidence::test_run("").unwrap_err(),
        GradeError::EvidenceRequired
    );
    assert_eq!(
        GradeEvidence::bounded_analysis("", "d").unwrap_err(),
        GradeError::EvidenceRequired
    );
    assert_eq!(ledger.effective_grade("a"), Some(GradeName::ObservedOnly));
    assert!(ledger.history("a").is_empty());
}

#[test]
fn upgrade_with_evidence_recorded_and_queryable() {
    let mut ledger = ledger_with("a", GradeName::ObservedOnly);
    let record = ledger
        .upgrade(
            "a",
            GradeName::Complete,
            GradeEvidence::verification_receipt("receipt-abc123").unwrap(),
            "alice",
        )
        .unwrap();
    assert_eq!(record, 0, "first event is ordinal 0");
    assert_eq!(ledger.effective_grade("a"), Some(GradeName::Complete));
    let history = ledger.history("a");
    assert_eq!(history.len(), 1);
    let entry = &history[0];
    assert_eq!(entry.ordinal, 0);
    assert!(!entry.revoked);
    assert_eq!(entry.upgrade.from, GradeName::ObservedOnly);
    assert_eq!(entry.upgrade.to, GradeName::Complete);
    assert_eq!(entry.upgrade.actor, "alice");
    assert_eq!(
        entry.upgrade.evidence.evidence_id(),
        "receipt:receipt-abc123"
    );
    assert!(entry.revocations.is_empty());
    // Test-run evidence is a distinct recorded path.
    let mut ledger = ledger_with("b", GradeName::ObservedOnly);
    ledger
        .upgrade(
            "b",
            GradeName::SoundOverapproximation,
            GradeEvidence::test_run("run://ci/1234").unwrap(),
            "bob",
        )
        .unwrap();
    assert_eq!(
        ledger.effective_grade("b"),
        Some(GradeName::SoundOverapproximation)
    );
    assert_eq!(
        ledger.history("b")[0].upgrade.evidence.evidence_id(),
        "test_run:run://ci/1234"
    );
}

#[test]
fn upgrade_requires_declared_artifact_and_lattice_edge() {
    let mut ledger = GradeLedger::new();
    // Undeclared artifact.
    assert_eq!(
        ledger
            .upgrade(
                "ghost",
                GradeName::Complete,
                GradeEvidence::verification_receipt("r").unwrap(),
                "alice",
            )
            .unwrap_err(),
        GradeError::UnknownArtifact("ghost".into())
    );
    // Grades are fixed at construction: re-declaration rejected.
    ledger.declare_grade("a", GradeName::ObservedOnly).unwrap();
    assert_eq!(
        ledger.declare_grade("a", GradeName::Complete).unwrap_err(),
        GradeError::AlreadyDeclared("a".into())
    );
    // Downgrade attempts rejected.
    let mut ledger = ledger_with("d", GradeName::SoundOverapproximation);
    assert_eq!(
        ledger
            .upgrade(
                "d",
                GradeName::ObservedOnly,
                GradeEvidence::verification_receipt("r").unwrap(),
                "alice",
            )
            .unwrap_err(),
        GradeError::UpgradeNotAllowed {
            from: GradeName::SoundOverapproximation,
            to: GradeName::ObservedOnly
        }
    );
    // Top grade has nowhere to go.
    let mut ledger = ledger_with("t", GradeName::Complete);
    assert!(matches!(
        ledger.upgrade(
            "t",
            GradeName::Complete,
            GradeEvidence::verification_receipt("r").unwrap(),
            "alice",
        ),
        Err(GradeError::UpgradeNotAllowed { .. })
    ));
    // Unknown is terminal-epistemic: never upgradable.
    let mut ledger = ledger_with("u", GradeName::Unknown);
    assert_eq!(
        ledger
            .upgrade(
                "u",
                GradeName::ObservedOnly,
                GradeEvidence::verification_receipt("r").unwrap(),
                "alice",
            )
            .unwrap_err(),
        GradeError::UpgradeNotAllowed {
            from: GradeName::Unknown,
            to: GradeName::ObservedOnly
        }
    );
}

#[test]
fn prior_upgrade_citations_validated() {
    let mut ledger = ledger_with("a", GradeName::ObservedOnly);
    assert_eq!(
        ledger
            .upgrade(
                "a",
                GradeName::SoundOverapproximation,
                GradeEvidence::prior_upgrade(99),
                "alice",
            )
            .unwrap_err(),
        GradeError::NoSuchRecord(99)
    );
    let record = ledger
        .upgrade(
            "a",
            GradeName::SoundOverapproximation,
            GradeEvidence::verification_receipt("r").unwrap(),
            "alice",
        )
        .unwrap();
    ledger
        .revoke_upgrade(record, "receipt r invalidated", "carol")
        .unwrap();
    assert_eq!(
        ledger
            .upgrade(
                "a",
                GradeName::Complete,
                GradeEvidence::prior_upgrade(record),
                "alice",
            )
            .unwrap_err(),
        GradeError::AlreadyRevoked(record)
    );
}

#[test]
fn revocation_cascades_exactly_and_unrelated_grades_untouched() {
    let mut ledger = ledger_with("dep", GradeName::ObservedOnly);
    ledger
        .declare_grade("other", GradeName::ObservedOnly)
        .unwrap();
    // dep: ObservedOnly -> SoundOverapproximation on receipt X,
    //      SoundOverapproximation -> Complete on prior-upgrade citation.
    let rec1 = ledger
        .upgrade(
            "dep",
            GradeName::SoundOverapproximation,
            GradeEvidence::verification_receipt("receipt:X").unwrap(),
            "alice",
        )
        .unwrap();
    let rec2 = ledger
        .upgrade(
            "dep",
            GradeName::Complete,
            GradeEvidence::prior_upgrade(rec1),
            "alice",
        )
        .unwrap();
    // other: unrelated evidence Y.
    ledger
        .upgrade(
            "other",
            GradeName::SoundOverapproximation,
            GradeEvidence::verification_receipt("receipt:Y").unwrap(),
            "bob",
        )
        .unwrap();
    assert_eq!(ledger.effective_grade("dep"), Some(GradeName::Complete));

    // Receipt X is invalidated (e.g. its cert was revoked).
    let revoked = ledger.revoke_evidence("receipt:receipt:X", "cert revoked", "carol");
    assert_eq!(revoked, 2, "rec1 (evidence match) + rec2 (PriorUpgrade)");
    assert!(ledger.is_revoked(rec1));
    assert!(ledger.is_revoked(rec2));
    assert_eq!(
        ledger.effective_grade("dep"),
        Some(GradeName::ObservedOnly),
        "fail-closed: rests at pre-upgrade grade"
    );
    // Unrelated artifact untouched, its record intact.
    assert!(!ledger.is_revoked(2));
    assert_eq!(
        ledger.effective_grade("other"),
        Some(GradeName::SoundOverapproximation)
    );
    // Both records stay in the history, flagged revoked, with notes.
    let history = ledger.history("dep");
    assert_eq!(history.len(), 2);
    assert!(history.iter().all(|e| e.revoked));
    let notes: Vec<&GradeRevocation> = history.iter().flat_map(|e| e.revocations.iter()).collect();
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0].record, rec1);
    assert_eq!(notes[1].record, rec2);
    assert_eq!(notes[0].restored_to, GradeName::ObservedOnly);
    assert_eq!(notes[1].restored_to, GradeName::ObservedOnly);
    assert_eq!(notes[0].reason, "cert revoked");
}

#[test]
fn revocation_fail_closed_implicit_from_chain() {
    // rec2 upgraded SO -> Complete on its own evidence Z, without citing
    // rec1. Revoking rec1's evidence must still pull rec2 down: the
    // implicit same-artifact from-chain is a sound over-approximation
    // (revoke too much, never too little).
    let mut ledger = ledger_with("chain", GradeName::ObservedOnly);
    let rec1 = ledger
        .upgrade(
            "chain",
            GradeName::SoundOverapproximation,
            GradeEvidence::verification_receipt("receipt:X").unwrap(),
            "alice",
        )
        .unwrap();
    ledger
        .upgrade(
            "chain",
            GradeName::Complete,
            GradeEvidence::verification_receipt("receipt:Z").unwrap(),
            "alice",
        )
        .unwrap();
    assert_eq!(
        ledger.revoke_evidence("receipt:receipt:X", "cert revoked", "carol"),
        2,
        "implicit from-chain dependent revoked too"
    );
    assert_eq!(
        ledger.effective_grade("chain"),
        Some(GradeName::ObservedOnly)
    );
    assert!(ledger.is_revoked(rec1));
}

#[test]
fn append_only_history_ordering_across_artifacts() {
    let mut ledger = ledger_with("a", GradeName::ObservedOnly);
    ledger.declare_grade("b", GradeName::ObservedOnly).unwrap();
    let r1 = ledger
        .upgrade(
            "a",
            GradeName::SoundOverapproximation,
            GradeEvidence::verification_receipt("r1").unwrap(),
            "alice",
        )
        .unwrap();
    let r2 = ledger
        .upgrade(
            "b",
            GradeName::Complete,
            GradeEvidence::verification_receipt("r2").unwrap(),
            "bob",
        )
        .unwrap();
    let r3 = ledger
        .upgrade(
            "a",
            GradeName::Complete,
            GradeEvidence::prior_upgrade(r1),
            "alice",
        )
        .unwrap();
    assert_eq!((r1, r2, r3), (0, 1, 2));
    // Revoke r2: revocation event gets the next ordinal (3).
    ledger.revoke_upgrade(r2, "evidence gone", "carol").unwrap();
    assert_eq!(ledger.history("b")[0].revocations[0].ordinal, 3);
    assert_eq!(ledger.history("b")[0].ordinal, 1);
    // History is strictly ordinal-ordered per artifact.
    let ordinals: Vec<u64> = ledger.history("a").iter().map(|e| e.ordinal).collect();
    assert_eq!(ordinals, vec![0, 2]);
    // Revoked record is retained, not deleted; unrelated artifact's
    // records are untouched by the revocation.
    assert!(ledger.history("b")[0].revoked);
    assert!(!ledger.history("a")[0].revoked);
    assert!(!ledger.history("a")[1].revoked);
    // The ledger is serializable: append-only state round-trips.
    let json = serde_json::to_string(&ledger).unwrap();
    let restored: GradeLedger = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, ledger);
    assert_eq!(restored.effective_grade("a"), Some(GradeName::Complete));
    assert_eq!(restored.effective_grade("b"), Some(GradeName::ObservedOnly));
    assert_eq!(restored.history("a"), ledger.history("a"));
}

#[test]
fn v6_mapping_divergence_rules() {
    // Lossless rows.
    assert_eq!(
        hub_equivalent(GradeName::Complete, ClaimKind::Absence),
        Some(HubGradeName::Proved)
    );
    assert_eq!(
        hub_equivalent(GradeName::ObservedOnly, ClaimKind::Positive),
        Some(HubGradeName::Observed)
    );
    assert_eq!(
        hub_equivalent(GradeName::Unknown, ClaimKind::Positive),
        Some(HubGradeName::Unknown)
    );
    // SoundOverapproximation certifies positive claims only.
    assert_eq!(
        hub_equivalent(GradeName::SoundOverapproximation, ClaimKind::Positive),
        Some(HubGradeName::BoundedComplete)
    );
    assert_eq!(
        hub_equivalent(GradeName::SoundOverapproximation, ClaimKind::Absence),
        None
    );
    // Reverse: BoundedComplete absence is never fed back as SO-certified.
    assert_eq!(
        grade_from_hub(HubGradeName::BoundedComplete, ClaimKind::Absence),
        None
    );
    assert_eq!(
        grade_from_hub(HubGradeName::BoundedComplete, ClaimKind::Positive),
        Some(GradeName::SoundOverapproximation)
    );
    // V6 Observed never promotes; Unknown is terminal both ways.
    assert_eq!(
        grade_from_hub(HubGradeName::Observed, ClaimKind::Positive),
        Some(GradeName::ObservedOnly)
    );
    assert_eq!(
        grade_from_hub(HubGradeName::Unknown, ClaimKind::Positive),
        Some(GradeName::Unknown)
    );
}

#[test]
fn coverage_class_maps_lossily_never_upgrades() {
    assert_eq!(
        GradeName::from(CoverageClass::Complete),
        GradeName::Complete
    );
    assert_eq!(
        GradeName::from(CoverageClass::SoundOverapproximation),
        GradeName::SoundOverapproximation
    );
    assert_eq!(
        GradeName::from(CoverageClass::ObservedOnly),
        GradeName::ObservedOnly
    );
    // Partial is weaker than ObservedOnly: lossy but never an upgrade.
    assert_eq!(
        GradeName::from(CoverageClass::Partial),
        GradeName::ObservedOnly
    );
    assert_eq!(GradeName::from(CoverageClass::Unknown), GradeName::Unknown);
}

#[test]
fn wire_names_match_both_vocabularies() {
    assert_eq!(
        serde_json::to_string(&GradeName::SoundOverapproximation).unwrap(),
        "\"sound_overapproximation\""
    );
    assert_eq!(
        serde_json::to_string(&GradeName::ObservedOnly).unwrap(),
        "\"observed_only\""
    );
    // Hub vocabulary mirror: identical serde names to zero-abi
    // CoverageGrade.
    assert_eq!(
        serde_json::to_string(&HubGradeName::BoundedComplete).unwrap(),
        "\"bounded_complete\""
    );
    assert_eq!(
        serde_json::to_string(&HubGradeName::Proved).unwrap(),
        "\"proved\""
    );
    assert_eq!(
        serde_json::to_string(&HubGradeName::Observed).unwrap(),
        "\"observed\""
    );
    assert_eq!(
        serde_json::to_string(&HubGradeName::Unknown).unwrap(),
        "\"unknown\""
    );
}
