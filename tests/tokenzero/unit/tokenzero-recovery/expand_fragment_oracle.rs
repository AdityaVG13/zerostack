//! SPEC-TZ-FAIL-001: TokenZeroStore::expand through Spec `scenario()`.
//!
//! Subject is the product expand API. Oracle is the published fragment
//! grammar (B/L, in-range, fail-loud unknown/reversed). Both-error is
//! agreement; mixed Ok/Err is a hard fail. Byte mismatch emits a
//! FailureBundle with `/failure/first_divergence`.

use tokenzero_core::ContentType;
use tokenzero_recovery::embedded_store::TokenZeroStore;
use tokenzero_recovery::RecoveryStore;
use tokenzero_test_support::{
    ExecutionEnvelope, FAILURE_BUNDLE_SCHEMA, FAILURE_FIRST_DIVERGENCE_JSONPTR, FailureBundle,
    GauntletIdentityPair, GauntletOracle, ScenarioAgreement, compare_bytes, scenario,
};

const PAYLOAD: &[u8] = b"hello world";
const REPRO: &str =
    "cargo test -p tokenzero-recovery --test expand_fragment_oracle -- --test-threads=1";

fn pair() -> GauntletIdentityPair {
    GauntletIdentityPair::new(GauntletOracle::Spec)
}

fn envelope(scenario_id: &str, seed: u64, workload: &str) -> ExecutionEnvelope {
    let pair = pair();
    let env = ExecutionEnvelope::from_pair(scenario_id, seed, pair, vec![workload.into()]);
    env.assert_engine_identities(pair);
    env
}

fn subject_expand(payload: &[u8], fragment: &str) -> Result<Vec<u8>, String> {
    let mut store = TokenZeroStore::in_memory();
    let ref_id = store.put(payload, None).map_err(|err| format!("{err:?}"))?;
    store
        .expand(&format!("{ref_id}#{fragment}"))
        .map_err(|err| format!("{err:?}"))
}

/// Spec oracle for `#Bstart-end` (half-open). Unknown kind / reversed / OOR fail.
fn spec_byte_fragment(payload_len: usize, fragment: &str) -> Result<(), String> {
    let rest = fragment
        .strip_prefix('B')
        .ok_or_else(|| "fragment-unknown-kind".to_string())?;
    let (start, end) = match rest.split_once('-') {
        Some((start, end)) => (
            start
                .parse::<usize>()
                .map_err(|_| "fragment-malformed".to_string())?,
            end.parse::<usize>()
                .map_err(|_| "fragment-malformed".to_string())?,
        ),
        None => return Err("fragment-malformed".into()),
    };
    if start > end {
        return Err("fragment-reversed".into());
    }
    if end > payload_len {
        return Err("fragment-out-of-range".into());
    }
    Ok(())
}

#[test]
fn expand_fragment_byte_range_agrees_with_spec() {
    let pair = pair();
    let envelope = envelope("expand-B0-5", 11, "B0-5");
    match scenario(
        "expand-B0-5",
        pair,
        || subject_expand(PAYLOAD, "B0-5"),
        || spec_byte_fragment(PAYLOAD.len(), "B0-5"),
    ) {
        ScenarioAgreement::BothOk(bytes) => {
            assert_eq!(bytes, &PAYLOAD[0..5]);
            compare_bytes(&envelope, "expand-B0-5", REPRO, &bytes, &PAYLOAD[0..5])
                .expect("spec slice and product expand must match");
        }
        ScenarioAgreement::BothErr { subject, oracle } => {
            panic!("in-range B0-5 must be BothOk, got subject={subject:?} oracle={oracle:?}")
        }
    }
}

#[test]
fn expand_unknown_kind_both_error_is_spec_agreement() {
    let pair = pair();
    let envelope = envelope("expand-X0-1", 13, "X0-1");
    match scenario(
        "expand-X0-1",
        pair,
        || subject_expand(PAYLOAD, "X0-1"),
        || spec_byte_fragment(PAYLOAD.len(), "X0-1"),
    ) {
        ScenarioAgreement::BothErr { subject, oracle } => {
            assert!(
                subject.contains("fragment-unknown-kind") || subject.contains("Fragment"),
                "subject must fail the fragment taxonomy, got {subject}"
            );
            assert_eq!(oracle, "fragment-unknown-kind");
            assert_ne!(
                subject, oracle,
                "K-8: both-error is agreement even when messages differ"
            );
            assert_eq!(envelope.engines.subject_identity, pair.subject.as_str());
            assert_eq!(envelope.engines.oracle_identity, pair.oracle.as_str());
        }
        ScenarioAgreement::BothOk(bytes) => {
            panic!("unknown kind must be both-error, got Ok({bytes:?})")
        }
    }
}

#[test]
fn expand_reversed_range_both_error_is_spec_agreement() {
    let pair = pair();
    envelope("expand-B10-1", 17, "B10-1");
    match scenario(
        "expand-B10-1",
        pair,
        || subject_expand(PAYLOAD, "B10-1"),
        || spec_byte_fragment(PAYLOAD.len(), "B10-1"),
    ) {
        ScenarioAgreement::BothErr { subject, oracle } => {
            assert!(
                subject.contains("fragment-reversed") || subject.contains("Fragment"),
                "subject must fail reversed, got {subject}"
            );
            assert_eq!(oracle, "fragment-reversed");
        }
        ScenarioAgreement::BothOk(bytes) => {
            panic!("reversed B10-1 must be both-error, got Ok({bytes:?})")
        }
    }
}

#[test]
fn first_divergence_bundle_from_product_expand() {
    let pair = pair();
    let envelope = envelope("expand-first-divergence", 19, "B0-2-vs-B0-3");
    let short = subject_expand(PAYLOAD, "B0-2").expect("B0-2");
    let long = subject_expand(PAYLOAD, "B0-3").expect("B0-3");
    assert_eq!(short, &PAYLOAD[0..2]);
    assert_eq!(long, &PAYLOAD[0..3]);

    compare_bytes(&envelope, "expand-B0-2", REPRO, &short, &PAYLOAD[0..2])
        .expect("product B0-2 must match spec slice");

    let bundle = compare_bytes(&envelope, "expand-B0-2-vs-B0-3", REPRO, &short, &long)
        .expect_err("B0-2 vs B0-3 must diverge");
    assert_eq!(bundle.schema, FAILURE_BUNDLE_SCHEMA);
    assert_eq!(
        bundle.first_divergence_jsonptr(),
        FAILURE_FIRST_DIVERGENCE_JSONPTR
    );
    let first = bundle
        .dereference(FAILURE_FIRST_DIVERGENCE_JSONPTR)
        .expect("jsonptr must resolve");
    assert_eq!(first["byte_offset"], 2);
    assert_eq!(first["subject_byte"], serde_json::Value::Null);
    assert_eq!(first["oracle_byte"], "0x6c");
    assert_eq!(bundle.provenance.seed, 19);
    assert_eq!(bundle.provenance.fixture_id, "expand-B0-2-vs-B0-3");
    assert_eq!(bundle.provenance.repro_command, REPRO);
    assert!(!bundle.provenance.schedule_fingerprint.is_empty());
    assert_eq!(
        bundle.provenance.git_sha,
        "862e3e682cb8aee0e150c1cb0b116cb2e23a44e2"
    );
    assert_eq!(bundle.engines.subject_identity, pair.subject.as_str());
    assert_eq!(bundle.engines.oracle_identity, pair.oracle.as_str());
    assert_ne!(
        bundle.engines.subject_identity, bundle.engines.oracle_identity,
        "FailureBundle must carry Subject≠Oracle"
    );
    assert_eq!(bundle.envelope_artifact_id, envelope.artifact_id());
    let _ =
        FailureBundle::from_byte_divergence(&envelope, "expand-B0-2-vs-B0-3", REPRO, &short, &long)
            .expect("constructor agrees with compare_bytes");
}

#[test]
fn persist_expand_full_and_fragments_are_original_bytes() {
    let mut store = TokenZeroStore::in_memory();
    let payload = b"hello\nworld\n";
    let ref_id = store.put(payload, None).expect("put");
    assert_eq!(store.expand(&ref_id).expect("full"), payload);
    assert_eq!(
        store.expand(&format!("{ref_id}#B0-5")).expect("B0-5"),
        b"hello"
    );
    assert_eq!(
        store.expand(&format!("{ref_id}#B0+5")).expect("B0+5"),
        b"hello"
    );
    assert_eq!(
        store.expand(&format!("{ref_id}#B0")).expect("B0 single byte"),
        b"h"
    );
    assert_eq!(
        store.expand(&format!("{ref_id}#B5-5")).expect("empty at 5"),
        b""
    );
    assert_eq!(
        store.expand(&format!("{ref_id}#L1-1")).expect("L1-1"),
        b"hello\n"
    );
    assert_eq!(
        store.expand(&format!("{ref_id}#L1-L2")).expect("L1-L2"),
        payload
    );
}

#[test]
fn dual_store_persist_expand_agrees_on_byte_and_line_fragments() {
    let payload = "hello\nworld\n";
    let mut embedded = TokenZeroStore::in_memory();
    let mut recovery = RecoveryStore::new(None);
    let embedded_ref = embedded.put(payload.as_bytes(), None).expect("put");
    let recovery_ref = recovery
        .store_blob(payload, ContentType::Unknown)
        .expect("store_blob");
    for fragment in ["", "#B0-5", "#B0+5", "#B0", "#L1-1", "#L1-2", "#L1-L2"] {
        let embedded_bytes = embedded
            .expand(&format!("{embedded_ref}{fragment}"))
            .unwrap_or_else(|err| panic!("embedded {fragment}: {err:?}"));
        let recovery_result = recovery.expand(
            &format!("{recovery_ref}{fragment}"),
            None,
            None,
            None,
            None,
            None,
        );
        assert!(
            recovery_result.found,
            "recovery {fragment} missing: {}",
            recovery_result.reason
        );
        assert_eq!(
            embedded_bytes,
            recovery_result.content.as_bytes(),
            "dual-store diverge at {fragment}"
        );
        if fragment.is_empty() {
            assert_eq!(embedded_bytes, payload.as_bytes());
        }
    }
}
