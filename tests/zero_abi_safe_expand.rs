//! SafeExpandHandle issuance and live revalidation acceptance tests.

use serde_json::{Value, json};
use zero_abi::{
    CompletenessEvidence, ExpandOutcome, ExpandPermit, LiveCompleteness, LiveExpandState,
    SafeExpandError, SafeExpandHandle, SafeExpandIssueRequest, SafeExpandIssuer, SafetyVerdict,
    Sha256Digest, sha256,
};

// Fixture helpers

/// Deterministic nonzero digest distinct per seed.
fn digest(seed: u8) -> Sha256Digest {
    Sha256Digest::from_bytes(sha256(&[seed; 32]))
}

fn secret(seed: u8) -> [u8; 32] {
    [seed; 32]
}

fn evidence() -> CompletenessEvidence {
    CompletenessEvidence {
        certificate_root: digest(0x11),
        verdict: SafetyVerdict::Safe,
        checker_identity: "graphzero.completeness.total".to_owned(),
        checker_version: "1.0.0".to_owned(),
        first_attempt: true,
    }
}

fn request() -> SafeExpandIssueRequest {
    SafeExpandIssueRequest {
        project_root: digest(0x01),
        request_root: digest(0x02),
        protected_scope_root: digest(0x03),
        demand_plan_root: digest(0x04),
        index_root: digest(0x05),
        index_version: "index-current".to_owned(),
        renderer_contract: digest(0x06),
        tenant: "tenant-a".to_owned(),
        epoch: 7,
        projection_root: digest(0x30),
        completeness: evidence(),
        issue_nonce: digest(0x7f),
    }
}

/// Live state that mirrors the canonical fixture request exactly.
fn live() -> LiveExpandState {
    let fixture = request();
    LiveExpandState {
        project_root: fixture.project_root,
        request_root: fixture.request_root,
        protected_scope_root: fixture.protected_scope_root,
        demand_plan_root: fixture.demand_plan_root,
        index_root: fixture.index_root,
        index_version: fixture.index_version,
        renderer_contract: fixture.renderer_contract,
        tenant: fixture.tenant,
        epoch: fixture.epoch,
        projection_root: fixture.projection_root,
        completeness: LiveCompleteness {
            certificate_root: Some(fixture.completeness.certificate_root),
            verdict: SafetyVerdict::Safe,
            checker_identity: Some(fixture.completeness.checker_identity),
            checker_version: Some(fixture.completeness.checker_version),
            first_attempt: true,
        },
        hidden_retry_after_issue: false,
    }
}

fn issuer() -> SafeExpandIssuer {
    SafeExpandIssuer::new(secret(0xab))
}

/// Serialize a handle, mutate one field in the JSON, and deserialize.
fn mutate_handle(handle: &SafeExpandHandle, field: &str, value: Value) -> Value {
    let mut object = serde_json::to_value(handle).unwrap();
    object
        .as_object_mut()
        .unwrap()
        .insert(field.to_owned(), value);
    object
}

fn assert_unsafe(outcome: &ExpandOutcome, reasons: &[&str]) {
    match outcome {
        ExpandOutcome::Unsafe { reasons: actual } => {
            assert!(!actual.is_empty(), "expected typed Unsafe reasons");
            for reason in reasons {
                assert!(
                    actual.iter().any(|r| r == reason),
                    "expected reason {reason:?} in {actual:?}"
                );
            }
        }
        other => panic!("expected Unsafe, got {other:?}"),
    }
    assert!(
        outcome.permit().is_none(),
        "Unsafe must never carry a permit"
    );
}

fn assert_unknown(outcome: &ExpandOutcome, reasons: &[&str]) {
    match outcome {
        ExpandOutcome::Unknown { reasons: actual } => {
            assert!(!actual.is_empty(), "expected typed Unknown reasons");
            for reason in reasons {
                assert!(
                    actual.iter().any(|r| r == reason),
                    "expected reason {reason:?} in {actual:?}"
                );
            }
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
    assert!(
        outcome.permit().is_none(),
        "Unknown must never carry a permit"
    );
}

// Trusted route: issuance and valid revalidation

#[test]
fn canonical_valid_fixture_issues_only_through_trusted_route() {
    let issuer = issuer();
    let fixture = request();
    let handle = issuer.issue(&fixture).expect("valid fixture must issue");
    assert_eq!(handle.handle_version(), 1);
    assert_eq!(handle.abi_version(), "zerostack.racc");
    assert_eq!(handle.epoch(), 7);
    assert_eq!(handle.tenant(), "tenant-a");
    assert_eq!(handle.completeness().verdict(), &SafetyVerdict::Safe);
    assert!(handle.completeness().first_attempt());
    issuer.verify(&handle).expect("issued handle must verify");

    // The one valid fixture expands exactly once: revalidation is Safe and
    // the permit reproduces the bound projection and demand plan exactly.
    let outcome = issuer.revalidate(&handle, &live());
    match &outcome {
        ExpandOutcome::Safe(permit) => {
            assert_eq!(permit.handle_id(), handle.handle_id());
            assert_eq!(permit.projection_root(), digest(0x30));
            assert_eq!(permit.demand_plan_root(), digest(0x04));
            assert_eq!(permit.project_root(), digest(0x01));
            assert_eq!(permit.tenant(), "tenant-a");
            assert_eq!(permit.epoch(), 7);
        }
        other => panic!("canonical valid fixture must be Safe, got {other:?}"),
    }
    assert!(outcome.is_safe());
    assert_eq!(
        outcome.to_verdict(),
        SafetyVerdict::Safe,
        "Safe outcome maps to the Safe verdict for ledgering"
    );
}

#[test]
fn handle_wire_round_trip_is_canonical_and_self_verifying() {
    let issuer = issuer();
    let handle = issuer.issue(&request()).unwrap();
    let bytes = handle.canonical_bytes().expect("canonical bytes");
    // Deterministic encoding: repeated encodes yield identical bytes.
    let bytes2 = handle.canonical_bytes().expect("second canonical bytes");
    assert_eq!(
        bytes, bytes2,
        "canonical bytes must be deterministic across repeated encodes"
    );
    let restored: SafeExpandHandle =
        serde_json::from_slice(&bytes).expect("canonical wire form must deserialize");
    assert_eq!(
        restored, handle,
        "round trip must reproduce the identical handle"
    );
    // Encode→decode→encode is byte-for-byte identical (canonicality).
    let reencoded = restored.canonical_bytes().expect("re-encoded bytes");
    assert_eq!(
        bytes, reencoded,
        "encode->decode->encode must be byte-for-byte identical"
    );
    // Authority is retained after round trip: typed verification succeeds and
    // revalidation yields Safe with independently derived bindings.
    issuer
        .verify(&restored)
        .expect("restored handle must verify");
    let outcome = issuer.revalidate(&restored, &live());
    assert!(
        outcome.is_safe(),
        "restored handle must revalidate Safe, got {outcome:?}"
    );
    match &outcome {
        ExpandOutcome::Safe(permit) => {
            // Independently derived bindings: compare against fixture digests,
            // not against the original handle object.
            assert_eq!(permit.projection_root(), digest(0x30));
            assert_eq!(permit.demand_plan_root(), digest(0x04));
            assert_eq!(permit.project_root(), digest(0x01));
            assert_eq!(permit.tenant(), "tenant-a");
            assert_eq!(permit.epoch(), 7);
            assert_eq!(permit.handle_id(), handle.handle_id());
        }
        other => panic!("restored handle must be Safe, got {other:?}"),
    }
}

// Forgery and tampering

#[test]
fn forged_handle_bound_field_tamper_fails() {
    let issuer = issuer();
    let handle = issuer.issue(&request()).unwrap();
    // Alter a bound field in the wire form: the issuance MAC no longer
    // verifies, so the altered handle is a forgery.
    let forged = mutate_handle(&handle, "tenant", json!("tenant-evil"));
    let forged: SafeExpandHandle = serde_json::from_value(forged).unwrap();
    assert_eq!(
        issuer.verify(&forged),
        Err(SafeExpandError::ForgedHandle),
        "tampered bindings must fail the issuance MAC"
    );
    assert_unsafe(&issuer.revalidate(&forged, &live()), &["forged_handle"]);
}

#[test]
fn tampered_handle_id_fails() {
    let issuer = issuer();
    let handle = issuer.issue(&request()).unwrap();
    // Replace only the recorded id: the MAC still verifies (the id is not
    // covered by the MAC payload) but the self-rooted id no longer matches
    // the bound fields, so the handle is tampered.
    let tampered = mutate_handle(&handle, "handle_id", json!(digest(0x99).to_hex()));
    let tampered: SafeExpandHandle = serde_json::from_value(tampered).unwrap();
    assert_eq!(
        issuer.verify(&tampered),
        Err(SafeExpandError::TamperedHandle),
        "a re-sealed id over unchanged bindings must not pass"
    );
    assert_unsafe(&issuer.revalidate(&tampered, &live()), &["tampered_handle"]);
}

#[test]
fn handle_from_wrong_issuer_fails() {
    let handle = issuer().issue(&request()).unwrap();
    // A different issuer secret cannot verify the same wire form.
    let other = SafeExpandIssuer::new(secret(0xcd));
    assert_eq!(
        other.verify(&handle),
        Err(SafeExpandError::ForgedHandle),
        "a handle is only valid from the issuer that sealed it"
    );
    assert_unsafe(&other.revalidate(&handle, &live()), &["forged_handle"]);
}

#[test]
fn arbitrary_wire_blob_cannot_gain_authority() {
    let issuer = issuer();
    // A guest-style hand-built blob: every seal is zeroed/absent.
    let blob = json!({
        "abi_version": "zerostack.racc",
        "handle_version": 1,
        "project_root": digest(0x01).to_hex(),
        "request_root": digest(0x02).to_hex(),
        "protected_scope_root": digest(0x03).to_hex(),
        "demand_plan_root": digest(0x04).to_hex(),
        "index_root": digest(0x05).to_hex(),
        "index_version": "index-current",
        "renderer_contract": digest(0x06).to_hex(),
        "tenant": "tenant-a",
        "epoch": 7,
        "projection_root": digest(0x30).to_hex(),
        "completeness": {
            "certificate_root": digest(0x11).to_hex(),
            "verdict": "safe",
            "checker_identity": "graphzero.completeness.total",
            "checker_version": "1.0.0",
            "first_attempt": true
        },
        "issue_nonce": digest(0x7f).to_hex(),
        "handle_id": digest(0x42).to_hex(),
        "issuance_mac": "00".repeat(32)
    });
    let blob: SafeExpandHandle = serde_json::from_value(blob).unwrap();
    assert_eq!(
        issuer.verify(&blob),
        Err(SafeExpandError::ForgedHandle),
        "a self-asserting blob without the issuer MAC is a forgery"
    );
    assert_unsafe(&issuer.revalidate(&blob, &live()), &["forged_handle"]);
}

#[test]
fn unknown_fields_on_handle_fail_closed() {
    let issuer = issuer();
    let handle = issuer.issue(&request()).unwrap();
    let extra = mutate_handle(&handle, "write_root", json!(digest(0x55).to_hex()));
    assert!(
        serde_json::from_value::<SafeExpandHandle>(extra).is_err(),
        "extra fields must be rejected on the handle wire form"
    );
}

#[test]
fn wrong_abi_version_is_typed_unsafe() {
    let issuer = issuer();
    let handle = issuer.issue(&request()).unwrap();
    let wrong = mutate_handle(&handle, "abi_version", json!("zerostack.old"));
    let wrong: SafeExpandHandle = serde_json::from_value(wrong).unwrap();
    assert_eq!(
        issuer.verify(&wrong),
        Err(SafeExpandError::WrongAbiVersion {
            actual: "zerostack.old".to_owned()
        })
    );
    assert_unsafe(&issuer.revalidate(&wrong, &live()), &["wrong_abi_version"]);
}

// Live revalidation: every binding is checked at use time

#[test]
fn cross_project_handle_fails() {
    let issuer = issuer();
    let handle = issuer.issue(&request()).unwrap();
    let mut live = live();
    live.project_root = digest(0xaa);
    assert_unsafe(&issuer.revalidate(&handle, &live), &["project_mismatch"]);
}

#[test]
fn cross_tenant_handle_fails() {
    let issuer = issuer();
    let handle = issuer.issue(&request()).unwrap();
    let mut live = live();
    live.tenant = "tenant-b".to_owned();
    assert_unsafe(&issuer.revalidate(&handle, &live), &["tenant_mismatch"]);
}

#[test]
fn stale_epoch_handle_fails() {
    let issuer = issuer();
    let handle = issuer.issue(&request()).unwrap();
    let mut live = live();
    live.epoch = 8;
    assert_unsafe(&issuer.revalidate(&handle, &live), &["epoch_mismatch"]);
}

#[test]
fn stale_index_handle_fails() {
    let issuer = issuer();
    let handle = issuer.issue(&request()).unwrap();
    let mut live = live();
    live.index_root = digest(0xbb);
    live.index_version = "index-v8".to_owned();
    assert_unsafe(
        &issuer.revalidate(&handle, &live),
        &["index_root_mismatch", "index_version_mismatch"],
    );
}

#[test]
fn altered_scope_handle_fails() {
    let issuer = issuer();
    let handle = issuer.issue(&request()).unwrap();
    let mut live = live();
    live.protected_scope_root = digest(0xcc);
    assert_unsafe(&issuer.revalidate(&handle, &live), &["scope_mismatch"]);
}

#[test]
fn altered_projection_handle_fails() {
    let issuer = issuer();
    let handle = issuer.issue(&request()).unwrap();
    let mut live = live();
    live.projection_root = digest(0x31);
    assert_unsafe(&issuer.revalidate(&handle, &live), &["projection_mismatch"]);
}

#[test]
fn renderer_mismatch_handle_fails() {
    let issuer = issuer();
    let handle = issuer.issue(&request()).unwrap();
    let mut live = live();
    live.renderer_contract = digest(0xdd);
    assert_unsafe(&issuer.revalidate(&handle, &live), &["renderer_mismatch"]);
}

#[test]
fn request_and_demand_plan_mismatch_fail() {
    let issuer = issuer();
    let handle = issuer.issue(&request()).unwrap();
    let mut live = live();
    live.request_root = digest(0xee);
    live.demand_plan_root = digest(0xff);
    assert_unsafe(
        &issuer.revalidate(&handle, &live),
        &["request_mismatch", "demand_plan_mismatch"],
    );
}

// Completeness evidence: stale, mismatched, missing, or Unknown

#[test]
fn missing_evidence_is_unknown_never_safe() {
    let issuer = issuer();
    let handle = issuer.issue(&request()).unwrap();
    // Each mandatory completeness field is tested in isolation so the typed
    // Unknown reason is attributable to that specific absence.
    {
        let mut live = live();
        live.completeness.certificate_root = None;
        let outcome = issuer.revalidate(&handle, &live);
        assert_unknown(&outcome, &["completeness_evidence_missing"]);
        assert!(!outcome.is_safe(), "missing certificate must never be Safe");
    }
    {
        let mut live = live();
        live.completeness.checker_identity = None;
        let outcome = issuer.revalidate(&handle, &live);
        assert_unknown(&outcome, &["completeness_checker_missing"]);
        assert!(
            !outcome.is_safe(),
            "missing checker identity must never be Safe"
        );
    }
    {
        let mut live = live();
        live.completeness.checker_version = None;
        let outcome = issuer.revalidate(&handle, &live);
        assert_unknown(&outcome, &["completeness_checker_missing"]);
        assert!(
            !outcome.is_safe(),
            "missing checker version must never be Safe"
        );
    }
}

#[test]
fn unknown_verdict_is_unknown_never_safe() {
    let issuer = issuer();
    let handle = issuer.issue(&request()).unwrap();
    let mut live = live();
    live.completeness.verdict = SafetyVerdict::Unknown {
        reasons: vec!["graph_not_complete".to_owned()],
    };
    assert_unknown(
        &issuer.revalidate(&handle, &live),
        &["completeness_unknown"],
    );
}

#[test]
fn unsafe_verdict_is_unsafe() {
    let issuer = issuer();
    let handle = issuer.issue(&request()).unwrap();
    let mut live = live();
    live.completeness.verdict = SafetyVerdict::Unsafe {
        reasons: vec!["falsified".to_owned()],
    };
    assert_unsafe(&issuer.revalidate(&handle, &live), &["completeness_unsafe"]);
}

#[test]
fn stale_certificate_is_unsafe() {
    let issuer = issuer();
    let handle = issuer.issue(&request()).unwrap();
    let mut live = live();
    live.completeness.certificate_root = Some(digest(0x22));
    assert_unsafe(
        &issuer.revalidate(&handle, &live),
        &["completeness_certificate_mismatch"],
    );
}

#[test]
fn checker_identity_or_version_mismatch_is_unsafe() {
    let issuer = issuer();
    let handle = issuer.issue(&request()).unwrap();
    let mut live = live();
    live.completeness.checker_identity = Some("other.checker".to_owned());
    live.completeness.checker_version = Some("9.9.9".to_owned());
    assert_unsafe(
        &issuer.revalidate(&handle, &live),
        &["checker_identity_mismatch", "checker_version_mismatch"],
    );
}

#[test]
fn unsafe_dominates_unknown() {
    let issuer = issuer();
    let handle = issuer.issue(&request()).unwrap();
    let mut live = live();
    // A stale binding (Unsafe) plus missing evidence (Unknown): the outcome
    // must be Unsafe, never a downgrade or a guessed Safe.
    live.project_root = digest(0xaa);
    live.completeness.certificate_root = None;
    assert_unsafe(&issuer.revalidate(&handle, &live), &["project_mismatch"]);
}

// Hidden retry: pre-issue refusal and post-issue revocation

#[test]
fn hidden_retry_after_issue_revokes_handle() {
    let issuer = issuer();
    let handle = issuer.issue(&request()).unwrap();
    let mut live = live();
    live.hidden_retry_after_issue = true;
    assert_unsafe(
        &issuer.revalidate(&handle, &live),
        &["hidden_retry_after_issue"],
    );
}

#[test]
fn retried_completeness_check_revokes_handle() {
    let issuer = issuer();
    let handle = issuer.issue(&request()).unwrap();
    let mut live = live();
    live.completeness.first_attempt = false;
    assert_unsafe(&issuer.revalidate(&handle, &live), &["completeness_retry"]);
}

#[test]
fn issuance_refuses_hidden_retry_evidence() {
    let issuer = issuer();
    let mut fixture = request();
    fixture.completeness.first_attempt = false;
    assert_eq!(
        issuer.issue(&fixture),
        Err(SafeExpandError::HiddenRetryAtIssuance),
        "a retried completeness check must never issue"
    );
}

// Issuance fails closed on evidence and bindings

#[test]
fn issuance_refuses_unsafe_evidence() {
    let issuer = issuer();
    let mut fixture = request();
    fixture.completeness.verdict = SafetyVerdict::Unsafe {
        reasons: vec!["counterexample".to_owned()],
    };
    match issuer.issue(&fixture) {
        Err(SafeExpandError::UnsafeCompleteness { reasons }) => {
            assert_eq!(reasons, vec!["counterexample".to_owned()]);
        }
        other => panic!("expected UnsafeCompleteness refusal, got {other:?}"),
    }
}

#[test]
fn issuance_refuses_unknown_evidence() {
    let issuer = issuer();
    let mut fixture = request();
    fixture.completeness.verdict = SafetyVerdict::Unknown {
        reasons: vec!["graph_not_complete".to_owned()],
    };
    match issuer.issue(&fixture) {
        Err(SafeExpandError::UnknownCompleteness { reasons }) => {
            assert_eq!(reasons, vec!["graph_not_complete".to_owned()]);
        }
        other => panic!("expected UnknownCompleteness refusal, got {other:?}"),
    }
}

#[test]
fn issuance_refuses_missing_certificate() {
    let issuer = issuer();
    let mut fixture = request();
    fixture.completeness.certificate_root = Sha256Digest::ZERO;
    assert_eq!(
        issuer.issue(&fixture),
        Err(SafeExpandError::MissingCertificateRoot),
        "Safe must always carry a certificate root"
    );
}

#[test]
fn issuance_refuses_invalid_bindings() {
    let issuer = issuer();

    let mut zero_root = request();
    zero_root.project_root = Sha256Digest::ZERO;
    assert_eq!(
        issuer.issue(&zero_root),
        Err(SafeExpandError::ZeroRoot("project_root"))
    );

    let mut empty_tenant = request();
    empty_tenant.tenant = String::new();
    assert_eq!(
        issuer.issue(&empty_tenant),
        Err(SafeExpandError::EmptyString("tenant"))
    );

    let mut control_tenant = request();
    control_tenant.tenant = "tenant\n-a".to_owned();
    assert_eq!(
        issuer.issue(&control_tenant),
        Err(SafeExpandError::ControlCharacter { field: "tenant" })
    );

    let mut long_version = request();
    long_version.index_version = "x".repeat(257);
    assert_eq!(
        issuer.issue(&long_version),
        Err(SafeExpandError::StringTooLong {
            field: "index_version",
            actual: 257,
            maximum: 256,
        })
    );

    let mut zero_nonce = request();
    zero_nonce.issue_nonce = Sha256Digest::ZERO;
    assert_eq!(
        issuer.issue(&zero_nonce),
        Err(SafeExpandError::ZeroIssueNonce)
    );
}

// Read-only authority: no write or commit capability is encoded

#[test]
fn permit_encodes_read_only_authority_only() {
    let issuer = issuer();
    let fixture = request();
    let live_state = live();
    let handle = issuer.issue(&fixture).unwrap();
    let outcome = issuer.revalidate(&handle, &live_state);
    assert!(
        outcome.is_safe(),
        "valid fixture must be Safe, got {outcome:?}"
    );
    assert_eq!(outcome.to_verdict(), SafetyVerdict::Safe);
    let ExpandOutcome::Safe(permit) = outcome else {
        panic!("valid fixture must be Safe");
    };
    // Typed read-only bindings match independently derived fixture digests.
    assert_eq!(permit.handle_id(), handle.handle_id());
    assert_eq!(permit.project_root(), digest(0x01));
    assert_eq!(permit.request_root(), digest(0x02));
    assert_eq!(permit.protected_scope_root(), digest(0x03));
    assert_eq!(permit.demand_plan_root(), digest(0x04));
    assert_eq!(permit.index_root(), digest(0x05));
    assert_eq!(permit.index_version(), "index-current");
    assert_eq!(permit.renderer_contract(), digest(0x06));
    assert_eq!(permit.tenant(), "tenant-a");
    assert_eq!(permit.epoch(), 7);
    assert_eq!(permit.projection_root(), digest(0x30));
    // Documented wire schema: exactly the 11 read-only bindings, no more.
    let rendered = serde_json::to_value(&permit).unwrap();
    let object = rendered.as_object().unwrap();
    let mut keys: Vec<String> = object.keys().cloned().collect();
    keys.sort();
    let mut expected = vec![
        "demand_plan_root",
        "epoch",
        "handle_id",
        "index_root",
        "index_version",
        "projection_root",
        "project_root",
        "protected_scope_root",
        "renderer_contract",
        "request_root",
        "tenant",
    ]
    .into_iter()
    .map(String::from)
    .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(
        keys, expected,
        "permit wire schema must be exactly the documented read-only bindings"
    );
    // Each wire value equals the independently derived fixture value.
    assert_eq!(object["handle_id"], json!(handle.handle_id().to_hex()));
    assert_eq!(object["project_root"], json!(digest(0x01).to_hex()));
    assert_eq!(object["request_root"], json!(digest(0x02).to_hex()));
    assert_eq!(object["protected_scope_root"], json!(digest(0x03).to_hex()));
    assert_eq!(object["demand_plan_root"], json!(digest(0x04).to_hex()));
    assert_eq!(object["index_root"], json!(digest(0x05).to_hex()));
    assert_eq!(object["index_version"], json!("index-current"));
    assert_eq!(object["renderer_contract"], json!(digest(0x06).to_hex()));
    assert_eq!(object["tenant"], json!("tenant-a"));
    assert_eq!(object["epoch"], json!(7u64));
    assert_eq!(object["projection_root"], json!(digest(0x30).to_hex()));
    // Authorization boundary: a permit cannot carry write/commit/mutation
    // authority. The wire form is deny_unknown_fields, so any such field
    // fails to deserialize.
    for forbidden in [
        "write_root",
        "commit_root",
        "mutation",
        "transaction",
        "effect",
    ] {
        let mut with_extra = rendered.clone();
        with_extra
            .as_object_mut()
            .unwrap()
            .insert(forbidden.to_owned(), json!(digest(0x55).to_hex()));
        assert!(
            serde_json::from_value::<ExpandPermit>(with_extra).is_err(),
            "permit must reject {forbidden} authority"
        );
    }
    // Round-trip of the permit itself is canonical and preserves bindings.
    let restored: ExpandPermit = serde_json::from_value(rendered.clone()).unwrap();
    assert_eq!(restored, permit);
    assert_eq!(restored.projection_root(), digest(0x30));
}
