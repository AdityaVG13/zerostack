//! V7 ETNF shadow ABI tests (bead zerostack-4lfp).
//!
//! Covers the named acceptance criteria: canonical fixtures round-trip;
//! `Unsafe`/`Unknown` cannot serialize authority; the certificate root binds
//! evidence, scope, contract, checker version, and resource ledger; shadow
//! output is observable and comparable but cannot pass any existing
//! write/permit gate. Shadow artifacts grant no production authority.

use serde_json::json;
use zero_abi::{
    ApprovalGrant, CheckerIdentity, ETNF_MAX_EVIDENCE_ITEMS, ETNF_MAX_FALSIFIERS,
    ETNF_MAX_ID_BYTES, ETNF_MAX_STRING_BYTES, ETNF_MAX_WITNESS_FACTS, ETNF_SCHEMA_ID,
    EtnfError, EvidenceItem, ExplicitFallback, FallbackKind, FiniteWitness, Falsifier,
    PermitGrant, ProposedAuthorityTransition, ProposedTransitionKind, ResourceLedger,
    RootedEvidence, SafetyVerdict, ShadowCertificate, V7ShadowReport,
};

const ROOT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const DIGEST_2: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const EVIDENCE_ROOT: &str =
    "a300c0798e0f0d72e33647fd9afe7bc8173d50d93381a794258268b755c82c57";
const LEDGER_ROOT: &str = "db11272c6a8190420dd41c84e76cb2dc7f8b0243a1226b5d2eab2fbf4d18a9b2";
const CERT_ROOT: &str = "4bee74893b7038ca062abb6a3bd7576dffeb5a08c0ca8e1d9e5c99e2f11a958f";

/// Hand-written canonical fixture: Safe verdict with certificate.
const SAFE_FIXTURE: &str = r#"{"certificate":{"checker":{"id":"w7/verdict_v1","version":"1.0.0"},"contract":"zero.contract/v1","evidence_root":"a300c0798e0f0d72e33647fd9afe7bc8173d50d93381a794258268b755c82c57","resource_ledger_root":"db11272c6a8190420dd41c84e76cb2dc7f8b0243a1226b5d2eab2fbf4d18a9b2","root":"4bee74893b7038ca062abb6a3bd7576dffeb5a08c0ca8e1d9e5c99e2f11a958f","scope":"scope:project/main"},"checker":{"id":"w7/verdict_v1","version":"1.0.0"},"contract":"zero.contract/v1","evidence":{"anchor":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","items":[{"digest":"1111111111111111111111111111111111111111111111111111111111111111","name":"fs.read:r1"}]},"fallback":{"kind":"frozen_raw_baseline","obligation":"run the frozen raw baseline"},"falsifiers":[{"description":"Unsafe issues authority","id":"W7-T01-f1"}],"ledger":{"bytes_read":512,"checks":1,"complete":true,"items_checked":1},"schema":"zerostack/v7-shadow-report/1","scope":"scope:project/main","shadow":true,"transition":{"kind":"reuse_cached_result","target":"2222222222222222222222222222222222222222222222222222222222222222"},"verdict":"safe","witness":{"facts":["fs.read:r1 bytes == receipt"]}}"#;

/// Hand-written canonical fixture: Unsafe verdict, no certificate.
const UNSAFE_FIXTURE: &str = r#"{"checker":{"id":"w7/verdict_v1","version":"1.0.0"},"contract":"zero.contract/v1","evidence":{"anchor":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","items":[{"digest":"1111111111111111111111111111111111111111111111111111111111111111","name":"fs.read:r1"}]},"fallback":{"kind":"frozen_raw_baseline","obligation":"run the frozen raw baseline"},"falsifiers":[{"description":"Unsafe issues authority","id":"W7-T01-f1"}],"ledger":{"bytes_read":512,"checks":1,"complete":true,"items_checked":1},"schema":"zerostack/v7-shadow-report/1","scope":"scope:project/main","shadow":true,"verdict":{"unsafe":{"reasons":["premise_falsified"]}},"witness":{"facts":["premise falsified by receipt"]}}"#;

/// Hand-written canonical fixture: Unknown verdict, no certificate.
const UNKNOWN_FIXTURE: &str = r#"{"checker":{"id":"w7/verdict_v1","version":"1.0.0"},"contract":"zero.contract/v1","evidence":{"anchor":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","items":[{"digest":"1111111111111111111111111111111111111111111111111111111111111111","name":"fs.read:r1"}]},"fallback":{"kind":"direct_native_path","obligation":"run the native direct path"},"falsifiers":[{"description":"Unsafe issues authority","id":"W7-T01-f1"}],"ledger":{"bytes_read":512,"checks":1,"complete":false,"items_checked":1},"schema":"zerostack/v7-shadow-report/1","scope":"scope:project/main","shadow":true,"verdict":{"unknown":{"reasons":["missing_evidence"]}},"witness":{"facts":["evidence cone incomplete"]}}"#;

fn checker() -> CheckerIdentity {
    CheckerIdentity::new("w7/verdict_v1", "1.0.0").unwrap()
}

fn evidence() -> RootedEvidence {
    RootedEvidence::new(ROOT_A, vec![EvidenceItem::new("fs.read:r1", DIGEST_1).unwrap()]).unwrap()
}

fn witness(facts: &[&str]) -> FiniteWitness {
    FiniteWitness::new(facts.iter().map(|fact| fact.to_string()).collect()).unwrap()
}

fn fallback() -> ExplicitFallback {
    ExplicitFallback::new(FallbackKind::FrozenRawBaseline, "run the frozen raw baseline").unwrap()
}

fn falsifiers() -> Vec<Falsifier> {
    vec![Falsifier::new("W7-T01-f1", "Unsafe issues authority").unwrap()]
}

fn ledger() -> ResourceLedger {
    ResourceLedger::new(512, 1, 1, true)
}

fn safe_report() -> V7ShadowReport {
    V7ShadowReport::new(
        SafetyVerdict::Safe,
        checker(),
        "scope:project/main",
        "zero.contract/v1",
        evidence(),
        witness(&["fs.read:r1 bytes == receipt"]),
        Some(
            ProposedAuthorityTransition::new(
                ProposedTransitionKind::ReuseCachedResult,
                DIGEST_2,
            )
            .unwrap(),
        ),
        fallback(),
        falsifiers(),
        ledger(),
    )
    .unwrap()
}

fn unsafe_report() -> V7ShadowReport {
    V7ShadowReport::new(
        SafetyVerdict::Unsafe { reasons: vec!["premise_falsified".into()] },
        checker(),
        "scope:project/main",
        "zero.contract/v1",
        evidence(),
        witness(&["premise falsified by receipt"]),
        None,
        fallback(),
        falsifiers(),
        ledger(),
    )
    .unwrap()
}

fn unknown_report() -> V7ShadowReport {
    V7ShadowReport::new(
        SafetyVerdict::Unknown { reasons: vec!["missing_evidence".into()] },
        checker(),
        "scope:project/main",
        "zero.contract/v1",
        evidence(),
        witness(&["evidence cone incomplete"]),
        None,
        ExplicitFallback::new(FallbackKind::DirectNativePath, "run the native direct path")
            .unwrap(),
        falsifiers(),
        ResourceLedger::new(512, 1, 1, false),
    )
    .unwrap()
}

#[test]
fn safe_fixture_round_trips() {
    let parsed = V7ShadowReport::from_canonical_bytes(SAFE_FIXTURE.as_bytes()).unwrap();
    assert_eq!(parsed, safe_report());
    assert!(parsed.grants_authority());
    assert!(parsed.certificate.is_some());
    // Canonical bytes are byte-stable: re-serialization reproduces the fixture.
    assert_eq!(parsed.to_canonical_bytes().unwrap(), SAFE_FIXTURE.as_bytes());
    // Parsing the canonical bytes again is idempotent.
    let bytes = parsed.to_canonical_bytes().unwrap();
    assert_eq!(V7ShadowReport::from_canonical_bytes(&bytes).unwrap(), parsed);
}

#[test]
fn unsafe_and_unknown_fixtures_round_trip_without_authority() {
    for (fixture, expected) in [
        (UNSAFE_FIXTURE, unsafe_report()),
        (UNKNOWN_FIXTURE, unknown_report()),
    ] {
        let parsed = V7ShadowReport::from_canonical_bytes(fixture.as_bytes()).unwrap();
        assert_eq!(parsed, expected);
        assert!(!parsed.grants_authority());
        assert!(parsed.certificate.is_none());
        assert_eq!(parsed.to_canonical_bytes().unwrap(), fixture.as_bytes());
        // No certificate serializes: authority never leaves the process.
        let text = String::from_utf8(parsed.to_canonical_bytes().unwrap()).unwrap();
        assert!(!text.contains("certificate"));
    }
}

#[test]
fn unsafe_and_unknown_cannot_issue_certificates() {
    let non_safe = [
        SafetyVerdict::Unsafe { reasons: vec!["premise_falsified".into()] },
        SafetyVerdict::Unknown { reasons: vec!["missing_evidence".into()] },
    ];
    for verdict in non_safe {
        assert_eq!(
            ShadowCertificate::issue(&verdict, EVIDENCE_ROOT, "scope:project/main",
                "zero.contract/v1", &checker(), LEDGER_ROOT),
            Err(EtnfError::NotSafe)
        );
    }
}

#[test]
fn certificate_under_non_safe_verdict_is_rejected_on_parse() {
    // Same canonical document as the Unsafe fixture, with a certificate
    // attached from the Safe fixture.
    let mut value: serde_json::Value = serde_json::from_str(UNSAFE_FIXTURE).unwrap();
    let safe: serde_json::Value = serde_json::from_str(SAFE_FIXTURE).unwrap();
    value["certificate"] = safe["certificate"].clone();
    let bytes = serde_json::to_vec(&value).unwrap();
    assert_eq!(
        V7ShadowReport::from_canonical_bytes(&bytes),
        Err(EtnfError::CertificateWithoutSafe)
    );
}

#[test]
fn safe_verdict_without_certificate_is_rejected_on_parse() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&safe_report().to_canonical_bytes().unwrap()).unwrap();
    value.as_object_mut().unwrap().remove("certificate");
    let bytes = serde_json::to_vec(&value).unwrap();
    assert_eq!(
        V7ShadowReport::from_canonical_bytes(&bytes),
        Err(EtnfError::MissingCertificateForSafe)
    );
}

#[test]
fn certificate_root_binds_all_five_fields() {
    let base = safe_report();
    let certificate = base.certificate.as_ref().unwrap();
    // Deterministic recomputation over the stored fields.
    assert_eq!(
        ShadowCertificate::root_of(
            &certificate.evidence_root,
            &certificate.scope,
            &certificate.contract,
            &certificate.checker,
            &certificate.resource_ledger_root,
        ),
        certificate.root
    );
    let transition = Some(
        ProposedAuthorityTransition::new(ProposedTransitionKind::ReuseCachedResult, DIGEST_2)
            .unwrap(),
    );
    // Each bound dimension, varied through the validated constructor, changes
    // the root while keeping the report canonical and self-consistent.
    let variants = [
        // scope
        V7ShadowReport::new(
            SafetyVerdict::Safe,
            checker(),
            "scope:project/other",
            "zero.contract/v1",
            evidence(),
            witness(&["fs.read:r1 bytes == receipt"]),
            transition.clone(),
            fallback(),
            falsifiers(),
            ledger(),
        )
        .unwrap(),
        // contract
        V7ShadowReport::new(
            SafetyVerdict::Safe,
            checker(),
            "scope:project/main",
            "zero.contract/v2",
            evidence(),
            witness(&["fs.read:r1 bytes == receipt"]),
            transition.clone(),
            fallback(),
            falsifiers(),
            ledger(),
        )
        .unwrap(),
        // checker version
        V7ShadowReport::new(
            SafetyVerdict::Safe,
            CheckerIdentity::new("w7/verdict_v1", "2.0.0").unwrap(),
            "scope:project/main",
            "zero.contract/v1",
            evidence(),
            witness(&["fs.read:r1 bytes == receipt"]),
            transition.clone(),
            fallback(),
            falsifiers(),
            ledger(),
        )
        .unwrap(),
        // evidence
        V7ShadowReport::new(
            SafetyVerdict::Safe,
            checker(),
            "scope:project/main",
            "zero.contract/v1",
            RootedEvidence::new(
                ROOT_A,
                vec![
                    EvidenceItem::new("fs.read:r1", DIGEST_1).unwrap(),
                    EvidenceItem::new("fs.read:r2", DIGEST_2).unwrap(),
                ],
            )
            .unwrap(),
            witness(&["fs.read:r1 bytes == receipt"]),
            transition.clone(),
            fallback(),
            falsifiers(),
            ledger(),
        )
        .unwrap(),
        // resource ledger
        V7ShadowReport::new(
            SafetyVerdict::Safe,
            checker(),
            "scope:project/main",
            "zero.contract/v1",
            evidence(),
            witness(&["fs.read:r1 bytes == receipt"]),
            transition,
            fallback(),
            falsifiers(),
            ResourceLedger::new(513, 1, 1, true),
        )
        .unwrap(),
    ];
    for variant in variants {
        assert_ne!(variant.certificate.as_ref().unwrap().root, certificate.root);
        assert!(
            V7ShadowReport::from_canonical_bytes(&variant.to_canonical_bytes().unwrap()).is_ok()
        );
    }
}

#[test]
fn tampered_certificate_root_is_rejected() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&safe_report().to_canonical_bytes().unwrap()).unwrap();
    value["certificate"]["root"] = json!(ROOT_A);
    let bytes = serde_json::to_vec(&value).unwrap();
    assert_eq!(
        V7ShadowReport::from_canonical_bytes(&bytes),
        Err(EtnfError::CertificateRootMismatch)
    );
}

#[test]
fn shadow_and_schema_markers_are_enforced() {
    let bytes = safe_report().to_canonical_bytes().unwrap();

    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["shadow"] = json!(false);
    let tampered = serde_json::to_vec(&value).unwrap();
    assert_eq!(
        V7ShadowReport::from_canonical_bytes(&tampered),
        Err(EtnfError::ShadowMarkerFalse)
    );

    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["schema"] = json!("zerostack/other/1");
    let tampered = serde_json::to_vec(&value).unwrap();
    assert_eq!(
        V7ShadowReport::from_canonical_bytes(&tampered),
        Err(EtnfError::InvalidSchema { actual: "zerostack/other/1".into() })
    );
}

#[test]
fn non_canonical_bytes_are_rejected() {
    let bytes = safe_report().to_canonical_bytes().unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let noncanonical = text.replacen("\"shadow\":true", "\"shadow\": true", 1);
    assert_eq!(
        V7ShadowReport::from_canonical_bytes(noncanonical.as_bytes()),
        Err(EtnfError::NonCanonicalBytes)
    );
}

#[test]
fn shadow_output_cannot_pass_write_permit_gates() {
    let bytes = safe_report().to_canonical_bytes().unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    // None of the ApprovalGrant/PermitGrant wire fields appear in the shadow
    // document, so no gate consumer can mistake it for a grant.
    for key in [
        "grant_id", "approval_id", "permit_id", "canonical_operation_id", "session_id",
        "request_id", "authority_digest", "policy_digest", "issued_at_unix_ms",
        "expires_at_unix_ms", "effect_class", "engine", "operation",
    ] {
        assert!(
            !text.contains(&format!("\"{key}\"")),
            "shadow document must not contain gate field `{key}`"
        );
    }
    // The report and its certificate cannot deserialize as gate inputs.
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(serde_json::from_value::<ApprovalGrant>(value.clone()).is_err());
    assert!(serde_json::from_value::<PermitGrant>(value.clone()).is_err());
    let certificate = value["certificate"].clone();
    assert!(serde_json::from_value::<ApprovalGrant>(certificate.clone()).is_err());
    assert!(serde_json::from_value::<PermitGrant>(certificate).is_err());
}

#[test]
fn shadow_output_is_observable_and_comparable() {
    // Same inputs, independently constructed: byte-identical, Eq-equal.
    let first = safe_report();
    let second = safe_report();
    assert_eq!(first, second);
    assert_eq!(
        first.to_canonical_bytes().unwrap(),
        second.to_canonical_bytes().unwrap()
    );
    // Authority shape: Safe + live certificate. The proposed transition is
    // observable but never grants anything by itself: a non-Safe report that
    // still records a proposal grants no authority and carries no certificate.
    assert!(first.grants_authority());
    assert!(first.transition.is_some());
    let non_safe_with_proposal = V7ShadowReport::new(
        SafetyVerdict::Unknown { reasons: vec!["missing_evidence".into()] },
        checker(),
        "scope:project/main",
        "zero.contract/v1",
        evidence(),
        witness(&["evidence cone incomplete"]),
        Some(
            ProposedAuthorityTransition::new(
                ProposedTransitionKind::ReuseCachedResult,
                DIGEST_2,
            )
            .unwrap(),
        ),
        fallback(),
        falsifiers(),
        ledger(),
    )
    .unwrap();
    assert!(!non_safe_with_proposal.grants_authority());
    assert!(non_safe_with_proposal.certificate.is_none());
    // Any difference in evidence is observable in the canonical bytes.
    let different_evidence = V7ShadowReport::new(
        SafetyVerdict::Safe,
        checker(),
        "scope:project/main",
        "zero.contract/v1",
        RootedEvidence::new(ROOT_A, vec![EvidenceItem::new("fs.read:r1", DIGEST_2).unwrap()])
            .unwrap(),
        witness(&["fs.read:r1 bytes == receipt"]),
        Some(
            ProposedAuthorityTransition::new(
                ProposedTransitionKind::ReuseCachedResult,
                DIGEST_2,
            )
            .unwrap(),
        ),
        fallback(),
        falsifiers(),
        ledger(),
    )
    .unwrap();
    assert_ne!(first, different_evidence);
    assert_ne!(
        first.to_canonical_bytes().unwrap(),
        different_evidence.to_canonical_bytes().unwrap()
    );
}

#[test]
fn finiteness_and_shape_bounds_are_enforced() {
    // Evidence item bound.
    let too_many_items: Vec<EvidenceItem> = (0..=ETNF_MAX_EVIDENCE_ITEMS)
        .map(|i| EvidenceItem::new(format!("item{i}"), DIGEST_1).unwrap())
        .collect();
    assert_eq!(
        RootedEvidence::new(ROOT_A, too_many_items),
        Err(EtnfError::TooManyItems {
            field: "items",
            actual: ETNF_MAX_EVIDENCE_ITEMS + 1,
            maximum: ETNF_MAX_EVIDENCE_ITEMS,
        })
    );

    // Witness fact bound.
    let too_many_facts: Vec<String> =
        (0..=ETNF_MAX_WITNESS_FACTS).map(|i| format!("fact{i}")).collect();
    assert_eq!(
        FiniteWitness::new(too_many_facts),
        Err(EtnfError::TooManyItems {
            field: "facts",
            actual: ETNF_MAX_WITNESS_FACTS + 1,
            maximum: ETNF_MAX_WITNESS_FACTS,
        })
    );

    // Falsifier bound (report-level validation).
    let too_many_falsifiers: Vec<Falsifier> = (0..=ETNF_MAX_FALSIFIERS)
        .map(|i| Falsifier::new(format!("f{i}"), "condition").unwrap())
        .collect();
    assert_eq!(
        V7ShadowReport::new(
            SafetyVerdict::Unknown { reasons: vec!["missing_evidence".into()] },
            checker(),
            "scope:project/main",
            "zero.contract/v1",
            evidence(),
            witness(&["f"]),
            None,
            fallback(),
            too_many_falsifiers,
            ledger(),
        ),
        Err(EtnfError::TooManyItems {
            field: "falsifiers",
            actual: ETNF_MAX_FALSIFIERS + 1,
            maximum: ETNF_MAX_FALSIFIERS,
        })
    );

    // Identifier shape: empty, oversized, control characters.
    assert_eq!(CheckerIdentity::new("", "1"), Err(EtnfError::Empty { field: "id" }));
    assert_eq!(
        CheckerIdentity::new("x", ""),
        Err(EtnfError::Empty { field: "version" })
    );
    let oversized = "x".repeat(ETNF_MAX_ID_BYTES + 1);
    assert_eq!(
        CheckerIdentity::new(oversized.clone(), "1"),
        Err(EtnfError::TooLong {
            field: "id",
            actual: ETNF_MAX_ID_BYTES + 1,
            maximum: ETNF_MAX_ID_BYTES,
        })
    );
    assert_eq!(
        CheckerIdentity::new("bad\nid", "1"),
        Err(EtnfError::ControlCharacter { field: "id" })
    );

    // Free-text bound.
    let oversized_text = "x".repeat(ETNF_MAX_STRING_BYTES + 1);
    assert_eq!(
        ExplicitFallback::new(FallbackKind::Abort, oversized_text),
        Err(EtnfError::TooLong {
            field: "obligation",
            actual: ETNF_MAX_STRING_BYTES + 1,
            maximum: ETNF_MAX_STRING_BYTES,
        })
    );

    // Digest shape.
    assert_eq!(
        EvidenceItem::new("x", "zz"),
        Err(EtnfError::InvalidHex { field: "digest" })
    );
    assert_eq!(
        RootedEvidence::new("zz", vec![]),
        Err(EtnfError::InvalidHex { field: "anchor" })
    );
    assert_eq!(
        ProposedAuthorityTransition::new(ProposedTransitionKind::SkipModelTurn, "zz"),
        Err(EtnfError::InvalidHex { field: "target" })
    );
}

#[test]
fn schema_identity_is_constant() {
    assert_eq!(safe_report().schema, ETNF_SCHEMA_ID);
}
