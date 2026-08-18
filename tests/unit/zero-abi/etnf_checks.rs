//! V7 shadow checker tests (bead `zerostack-3cdn`, program `zerostack-vcqk`).
//!
//! Acceptance coverage:
//! - Positive, `Unsafe`, and `Unknown` fixtures for the certificate-chain
//!   (W7-T03), causal-closure (W7-T11), and savings-provenance (W7-T13)
//!   checkers, including root/scope/contract mismatches and missing
//!   transcript segments.
//! - Checker totality: no panic and no error on arbitrary untrusted bytes
//!   (property-tested over raw bytes and arbitrary JSON values).
//! - Shadow results never alter runtime routing or permits: no gate field
//!   appears in any emitted document, and no report deserializes as a
//!   write/permit grant.
//! - Resource cost is recorded (ledger) and the baseline remains available
//!   (explicit frozen-raw-baseline fallback on every verdict).

use proptest::prelude::*;
use proptest::test_runner::Config;
use serde_json::{json, Value};
use zero_abi::{
    check_causal_closure, check_certificate_chain, check_savings_provenance,
    savings_overhead_killed, ApprovalGrant, CheckerIdentity, EvidenceItem, ExplicitFallback,
    FallbackKind, Falsifier, FiniteWitness, KillMetrics, PermitGrant, ResourceLedger,
    RootedEvidence, SafetyVerdict, SavingsCategory, V7ShadowReport, VCQK_CHECKER_CAUSAL_ID,
    VCQK_CHECKER_CHAIN_ID, VCQK_CHECKER_SAVINGS_ID, VCQK_CONTRACT_CAUSAL, VCQK_CONTRACT_CHAIN,
    VCQK_CONTRACT_SAVINGS, VCQK_KILL_NONCONVERGENCE_MAX_ISSUES,
    VCQK_LEARNING_REFINEMENT_PUBLISH_AUTHORITY, VCQK_MAX_BASELINE_SEGMENTS, VCQK_MAX_CHAIN_LINKS,
    VCQK_MAX_CLOSURE_EDGES, VCQK_MAX_CLOSURE_NODES, VCQK_MAX_DEMANDED_OUTPUTS,
    VCQK_MAX_IDENTIFIER_BYTES, VCQK_MAX_SAVINGS_ENTRIES, VCQK_SCOPE_CAUSAL, VCQK_SCOPE_CHAIN,
    VCQK_SCOPE_SAVINGS,
};

const ROOT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ROOT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const DIGEST_1: &str = "1111111111111111111111111111111111111111111111111111111111111111";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fallback() -> ExplicitFallback {
    ExplicitFallback::new(
        FallbackKind::FrozenRawBaseline,
        "run the frozen raw baseline",
    )
    .unwrap()
}

/// A validated Safe report usable as a certificate-chain link.
fn link(anchor: &str, scope: &str, contract: &str, version: &str) -> V7ShadowReport {
    V7ShadowReport::new(
        SafetyVerdict::Safe,
        CheckerIdentity::new("w7/chain_v1", version).unwrap(),
        scope,
        contract,
        RootedEvidence::new(
            anchor,
            vec![EvidenceItem::new("fs.read:r1", DIGEST_1).unwrap()],
        )
        .unwrap(),
        FiniteWitness::new(vec!["chain link evidence".to_string()]).unwrap(),
        None,
        fallback(),
        vec![Falsifier::new("W7-T01-f1", "Unsafe issues authority").unwrap()],
        ResourceLedger::new(1, 1, 1, true),
    )
    .unwrap()
}

/// Canonical chain document: a JSON array of canonical report documents.
fn chain_bytes(links: &[V7ShadowReport]) -> Vec<u8> {
    let mut out = String::from("[");
    for (index, link) in links.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&String::from_utf8(link.to_canonical_bytes().unwrap()).unwrap());
    }
    out.push(']');
    out.into_bytes()
}

fn causal_bytes(demanded: &[&str], nodes: &[(&str, &str)], edges: &[(&str, &str)]) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "demanded": demanded,
        "nodes": nodes.iter().map(|(id, kind)| json!({"id": id, "kind": kind})).collect::<Vec<_>>(),
        "edges": edges.iter().map(|(from, to)| json!({"from": from, "to": to})).collect::<Vec<_>>(),
    }))
    .unwrap()
}

fn savings_bytes(baseline: &[(&str, &str)], savings: &[(&str, &str)]) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "baseline": baseline.iter().map(|(id, kind)| json!({"id": id, "kind": kind})).collect::<Vec<_>>(),
        "savings": savings.iter().map(|(segment, category)| json!({"segment": segment, "category": category})).collect::<Vec<_>>(),
    }))
    .unwrap()
}

fn assert_no_gate_fields(report: &V7ShadowReport) {
    let text = String::from_utf8(report.to_canonical_bytes().unwrap()).unwrap();
    for key in [
        "grant_id",
        "approval_id",
        "permit_id",
        "canonical_operation_id",
        "session_id",
        "request_id",
        "authority_digest",
        "policy_digest",
        "issued_at_unix_ms",
        "expires_at_unix_ms",
        "effect_class",
        "engine",
        "operation",
    ] {
        assert!(
            !text.contains(&format!("\"{key}\"")),
            "shadow document must not contain gate field `{key}`"
        );
    }
    let value: Value = serde_json::from_slice(&report.to_canonical_bytes().unwrap()).unwrap();
    assert!(serde_json::from_value::<ApprovalGrant>(value.clone()).is_err());
    assert!(serde_json::from_value::<PermitGrant>(value).is_err());
}

fn assert_baseline_available(report: &V7ShadowReport, input_len: usize) {
    // Explicit fallback names the frozen raw baseline on every verdict.
    assert_eq!(report.fallback.kind, FallbackKind::FrozenRawBaseline);
    assert_eq!(report.fallback.obligation, "run the frozen raw baseline");
    // Resource cost is recorded exactly. Only an Unknown run cannot close
    // its ledger; Safe and Unsafe runs are fully accounted.
    assert_eq!(report.ledger.bytes_read, input_len as u64);
    assert_eq!(report.ledger.checks, 1);
    assert_eq!(
        report.ledger.complete,
        !matches!(report.verdict, SafetyVerdict::Unknown { .. })
    );
}

// ---------------------------------------------------------------------------
// Certificate chain (W7-T03): positive, Unsafe, Unknown fixtures
// ---------------------------------------------------------------------------

#[test]
fn chain_positive_fixture_is_safe_and_round_trips() {
    let first = link(ROOT_A, "scope:project/main", "zero.contract/v1", "1.0.0");
    let root1 = first.certificate.as_ref().unwrap().root.clone();
    let second = link(
        &root1,
        "scope:project/main/child",
        "zero.contract/v1",
        "1.0.0",
    );
    let root2 = second.certificate.as_ref().unwrap().root.clone();
    let third = link(
        &root2,
        "scope:project/main/child/grandchild",
        "zero.contract/v1/chain",
        "1.0.0",
    );
    let root3 = third.certificate.as_ref().unwrap().root.clone();

    let input = chain_bytes(&[first, second, third]);
    let report = check_certificate_chain(&input).unwrap();
    assert_eq!(report.verdict, SafetyVerdict::Safe);
    assert!(report.grants_authority());
    assert!(report.certificate.is_some());
    assert_eq!(report.checker.id, VCQK_CHECKER_CHAIN_ID);
    assert_eq!(report.checker.version, "1.0.0");
    assert_eq!(report.scope, VCQK_SCOPE_CHAIN);
    assert_eq!(report.contract, VCQK_CONTRACT_CHAIN);
    // Evidence binds every link's certificate root.
    assert_eq!(report.evidence.items.len(), 3);
    assert_eq!(report.evidence.items[0].digest, root1);
    assert_eq!(report.evidence.items[1].digest, root2);
    assert_eq!(report.evidence.items[2].digest, root3);
    // The proposed transition targets the chain head, not anything live.
    assert_eq!(report.transition.as_ref().unwrap().target, root3);
    // Canonical round-trip.
    assert_eq!(
        V7ShadowReport::from_canonical_bytes(&report.to_canonical_bytes().unwrap()).unwrap(),
        report
    );
    // Determinism: same bytes, same report.
    let again = check_certificate_chain(&input).unwrap();
    assert_eq!(report, again);
    assert_eq!(
        report.to_canonical_bytes().unwrap(),
        again.to_canonical_bytes().unwrap()
    );
    assert_baseline_available(&report, input.len());
    assert_no_gate_fields(&report);
}

#[test]
fn chain_single_link_is_a_well_formed_chain() {
    let report = check_certificate_chain(&chain_bytes(&[link(
        ROOT_A,
        "scope:project/main",
        "zero.contract/v1",
        "1.0.0",
    )]))
    .unwrap();
    assert_eq!(report.verdict, SafetyVerdict::Safe);
    assert!(report.grants_authority());
}

#[test]
fn chain_empty_input_is_unknown() {
    let report = check_certificate_chain(b"[]").unwrap();
    assert_eq!(
        report.verdict,
        SafetyVerdict::Unknown {
            reasons: vec!["no_chain_links".into()]
        }
    );
    assert!(!report.grants_authority());
    assert!(report.certificate.is_none());
    assert!(!report.ledger.complete);
    assert_baseline_available(&report, 2);
    assert_no_gate_fields(&report);
}

#[test]
fn chain_unparseable_document_is_unknown() {
    for bytes in [
        b"".as_slice(),
        b"null",
        b"{}",
        b"\"just a string\"",
        &[0xff, 0xfe, 0xfd][..],
    ] {
        let report = check_certificate_chain(bytes).unwrap();
        assert_eq!(
            report.verdict,
            SafetyVerdict::Unknown {
                reasons: vec!["unparseable_input".into()]
            }
        );
        assert!(!report.grants_authority());
        assert!(report.certificate.is_none());
        assert!(!report.ledger.complete);
    }
}

#[test]
fn chain_adjacent_root_mismatch_is_unsafe() {
    // Successor evidence anchors on an unrelated root, not the predecessor
    // certificate root.
    let first = link(ROOT_A, "scope:project/main", "zero.contract/v1", "1.0.0");
    let second = link(
        ROOT_B,
        "scope:project/main/child",
        "zero.contract/v1",
        "1.0.0",
    );
    let input = chain_bytes(&[first, second]);
    let report = check_certificate_chain(&input).unwrap();
    assert_eq!(
        report.verdict,
        SafetyVerdict::Unsafe {
            reasons: vec!["adjacent_root_not_bound".into()]
        }
    );
    assert!(!report.grants_authority());
    assert!(report.certificate.is_none());
    assert!(report.ledger.complete);
    assert_baseline_available(&report, input.len());
    assert_no_gate_fields(&report);
}

#[test]
fn chain_scope_mismatch_is_unsafe() {
    let first = link(ROOT_A, "scope:project/main", "zero.contract/v1", "1.0.0");
    let root1 = first.certificate.as_ref().unwrap().root.clone();
    // Sibling scope, not a descendant of scope:project/main.
    let second = link(&root1, "scope:project/other", "zero.contract/v1", "1.0.0");
    let report = check_certificate_chain(&chain_bytes(&[first, second])).unwrap();
    assert_eq!(
        report.verdict,
        SafetyVerdict::Unsafe {
            reasons: vec!["scope_does_not_chain".into()]
        }
    );
    assert!(!report.grants_authority());
}

#[test]
fn chain_contract_mismatch_is_unsafe() {
    let first = link(ROOT_A, "scope:project/main", "zero.contract/v1", "1.0.0");
    let root1 = first.certificate.as_ref().unwrap().root.clone();
    // Contract v2 does not extend v1 as a path descendant.
    let second = link(
        &root1,
        "scope:project/main/child",
        "zero.contract/v2",
        "1.0.0",
    );
    let report = check_certificate_chain(&chain_bytes(&[first, second])).unwrap();
    assert_eq!(
        report.verdict,
        SafetyVerdict::Unsafe {
            reasons: vec!["contract_does_not_chain".into()]
        }
    );
    assert!(!report.grants_authority());
}

#[test]
fn chain_checker_identity_mismatch_is_unsafe() {
    let first = link(ROOT_A, "scope:project/main", "zero.contract/v1", "1.0.0");
    let root1 = first.certificate.as_ref().unwrap().root.clone();
    // An upgraded checker invalidates every prior certificate: the chain
    // cannot span versions.
    let second = link(
        &root1,
        "scope:project/main/child",
        "zero.contract/v1",
        "2.0.0",
    );
    let report = check_certificate_chain(&chain_bytes(&[first, second])).unwrap();
    assert_eq!(
        report.verdict,
        SafetyVerdict::Unsafe {
            reasons: vec!["checker_identity_broken".into()]
        }
    );
    assert!(!report.grants_authority());
}

#[test]
fn chain_non_certificate_link_is_unsafe() {
    let first = link(ROOT_A, "scope:project/main", "zero.contract/v1", "1.0.0");
    let unknown_link = V7ShadowReport::new(
        SafetyVerdict::Unknown {
            reasons: vec!["missing_evidence".into()],
        },
        CheckerIdentity::new("w7/chain_v1", "1.0.0").unwrap(),
        "scope:project/main/child",
        "zero.contract/v1",
        RootedEvidence::new(ROOT_B, vec![]).unwrap(),
        FiniteWitness::new(vec!["incomplete".to_string()]).unwrap(),
        None,
        fallback(),
        vec![],
        ResourceLedger::new(1, 1, 1, false),
    )
    .unwrap();
    let mut bytes = chain_bytes(&[first]);
    bytes.pop(); // drop trailing ']'
    bytes.push(b',');
    bytes.extend_from_slice(&unknown_link.to_canonical_bytes().unwrap());
    bytes.push(b']');
    let report = check_certificate_chain(&bytes).unwrap();
    assert_eq!(
        report.verdict,
        SafetyVerdict::Unsafe {
            reasons: vec!["non_certificate_link:1".into()]
        }
    );
    assert!(!report.grants_authority());
}

#[test]
fn chain_unparseable_link_is_unknown() {
    let first = link(ROOT_A, "scope:project/main", "zero.contract/v1", "1.0.0");
    let mut bytes = chain_bytes(&[first]);
    bytes.pop();
    bytes.extend_from_slice(b",{\"bogus\":true}]");
    let report = check_certificate_chain(&bytes).unwrap();
    assert_eq!(
        report.verdict,
        SafetyVerdict::Unknown {
            reasons: vec!["unparseable_link".into()]
        }
    );
    assert!(!report.grants_authority());
    assert!(!report.ledger.complete);
}

#[test]
fn chain_mismatch_dominates_missing_link_evidence() {
    // An adjacent mismatch (Unsafe) plus a gap (Unknown) must yield Unsafe:
    // the lattice law is Unsafe > Unknown > Safe.
    let first = link(ROOT_A, "scope:project/main", "zero.contract/v1", "1.0.0");
    let second = link(
        ROOT_B,
        "scope:project/main/child",
        "zero.contract/v1",
        "1.0.0",
    );
    let mut bytes = chain_bytes(&[first, second]);
    bytes.pop();
    bytes.extend_from_slice(b",{\"bogus\":true}]");
    let report = check_certificate_chain(&bytes).unwrap();
    assert_eq!(
        report.verdict,
        SafetyVerdict::Unsafe {
            reasons: vec!["adjacent_root_not_bound".into()]
        }
    );
}

#[test]
fn chain_oversized_chain_is_unknown() {
    let elements: Vec<Value> = (0..=VCQK_MAX_CHAIN_LINKS).map(|_| json!({})).collect();
    let bytes = serde_json::to_vec(&elements).unwrap();
    let report = check_certificate_chain(&bytes).unwrap();
    assert_eq!(
        report.verdict,
        SafetyVerdict::Unknown {
            reasons: vec!["input_exceeds_checker_bounds".into()]
        }
    );
    assert!(!report.grants_authority());
    assert!(!report.ledger.complete);
}

// ---------------------------------------------------------------------------
// Causal closure (W7-T11): positive, Unsafe, Unknown fixtures
// ---------------------------------------------------------------------------

#[test]
fn causal_positive_fixture_is_safe() {
    let bytes = causal_bytes(
        &["d1", "d2"],
        &[
            ("d1", "declared_output"),
            ("d2", "declared_output"),
            ("x", "derived"),
        ],
        &[("x", "d1"), ("d1", "d2")],
    );
    let report = check_causal_closure(&bytes).unwrap();
    assert_eq!(report.verdict, SafetyVerdict::Safe);
    assert!(report.grants_authority());
    assert!(report.certificate.is_some());
    assert_eq!(report.checker.id, VCQK_CHECKER_CAUSAL_ID);
    assert_eq!(report.scope, VCQK_SCOPE_CAUSAL);
    assert_eq!(report.contract, VCQK_CONTRACT_CAUSAL);
    // One derived-digest evidence item per demanded output, sorted.
    assert_eq!(report.evidence.items.len(), 2);
    assert_eq!(report.evidence.items[0].name, "demand:0");
    assert!(report.transition.is_some());
    assert!(report.ledger.complete);
    assert!(report.ledger.bytes_read > 0);
    assert_baseline_available(&report, bytes.len());
    assert_no_gate_fields(&report);
    // Deterministic.
    let again = check_causal_closure(&bytes).unwrap();
    assert_eq!(report, again);
    assert_eq!(
        report.to_canonical_bytes().unwrap(),
        again.to_canonical_bytes().unwrap()
    );
}

#[test]
fn causal_duplicate_demands_dedupe() {
    let bytes = causal_bytes(
        &["d1", "d1", "d2"],
        &[("d1", "declared_output"), ("d2", "declared_output")],
        &[],
    );
    let report = check_causal_closure(&bytes).unwrap();
    assert_eq!(report.verdict, SafetyVerdict::Safe);
    assert_eq!(report.evidence.items.len(), 2);
}

#[test]
fn causal_demanded_output_outside_closure_is_unsafe() {
    let bytes = causal_bytes(&["d1", "ghost"], &[("d1", "declared_output")], &[]);
    let report = check_causal_closure(&bytes).unwrap();
    assert_eq!(
        report.verdict,
        SafetyVerdict::Unsafe {
            reasons: vec!["demanded_output_outside_declared_closure".into()]
        }
    );
    assert!(!report.grants_authority());
    assert!(report.certificate.is_none());
    assert!(report.ledger.complete);
    assert_baseline_available(&report, bytes.len());
    assert_no_gate_fields(&report);
}

#[test]
fn causal_open_dependency_edge_is_unsafe() {
    // Edge (d1 -> ghost) where ghost is not a declared node.
    let bytes = causal_bytes(&["d1"], &[("d1", "declared_output")], &[("d1", "ghost")]);
    let report = check_causal_closure(&bytes).unwrap();
    assert_eq!(
        report.verdict,
        SafetyVerdict::Unsafe {
            reasons: vec!["open_dependency_edge".into()]
        }
    );
    assert!(!report.grants_authority());
    assert!(report.ledger.complete);
}

#[test]
fn causal_unparseable_document_is_unknown() {
    for bytes in [b"".as_slice(), b"not json at all", b"[]", b"42"] {
        let report = check_causal_closure(bytes).unwrap();
        assert_eq!(
            report.verdict,
            SafetyVerdict::Unknown {
                reasons: vec!["unparseable_input".into()]
            }
        );
        assert!(!report.grants_authority());
        assert!(!report.ledger.complete);
    }
}

#[test]
fn causal_empty_demand_list_is_unknown_vacuous() {
    let bytes = causal_bytes(&[], &[("d1", "declared_output")], &[]);
    let report = check_causal_closure(&bytes).unwrap();
    assert_eq!(
        report.verdict,
        SafetyVerdict::Unknown {
            reasons: vec!["no_demanded_outputs".into()]
        }
    );
    assert!(!report.grants_authority());
}

#[test]
fn causal_oversized_declaration_is_unknown() {
    let many: Vec<String> = (0..=VCQK_MAX_DEMANDED_OUTPUTS)
        .map(|i| format!("d{i}"))
        .collect();
    let bytes = serde_json::to_vec(&json!({"demanded": many, "nodes": [], "edges": []})).unwrap();
    let report = check_causal_closure(&bytes).unwrap();
    assert_eq!(
        report.verdict,
        SafetyVerdict::Unknown {
            reasons: vec!["input_exceeds_checker_bounds".into()]
        }
    );
    assert!(!report.grants_authority());

    let too_many_nodes: Vec<Value> = (0..=VCQK_MAX_CLOSURE_NODES)
        .map(|i| json!({"id": format!("n{i}"), "kind": "derived"}))
        .collect();
    let bytes =
        serde_json::to_vec(&json!({"demanded": ["d1"], "nodes": too_many_nodes, "edges": []}))
            .unwrap();
    assert_eq!(
        check_causal_closure(&bytes).unwrap().verdict,
        SafetyVerdict::Unknown {
            reasons: vec!["input_exceeds_checker_bounds".into()]
        }
    );

    let too_many_edges: Vec<Value> = (0..=VCQK_MAX_CLOSURE_EDGES)
        .map(|i| json!({"from": format!("a{i}"), "to": format!("b{i}")}))
        .collect();
    let bytes =
        serde_json::to_vec(&json!({"demanded": ["d1"], "nodes": [], "edges": too_many_edges}))
            .unwrap();
    assert_eq!(
        check_causal_closure(&bytes).unwrap().verdict,
        SafetyVerdict::Unknown {
            reasons: vec!["input_exceeds_checker_bounds".into()]
        }
    );
}

#[test]
fn causal_oversized_identifier_is_unknown() {
    let long = "x".repeat(VCQK_MAX_IDENTIFIER_BYTES + 1);
    let bytes = causal_bytes(&[&long], &[], &[]);
    let report = check_causal_closure(&bytes).unwrap();
    assert_eq!(
        report.verdict,
        SafetyVerdict::Unknown {
            reasons: vec!["identifier_too_long".into()]
        }
    );
    assert!(!report.grants_authority());
}

#[test]
fn causal_unknown_and_unsafe_never_carry_authority() {
    let bytes = causal_bytes(&["ghost"], &[("d1", "declared_output")], &[]);
    let report = check_causal_closure(&bytes).unwrap();
    assert!(!report.grants_authority());
    let text = String::from_utf8(report.to_canonical_bytes().unwrap()).unwrap();
    assert!(!text.contains("\"certificate\""));
}

// ---------------------------------------------------------------------------
// Savings provenance (W7-T13): positive, Unsafe, Unknown fixtures
// ---------------------------------------------------------------------------

#[test]
fn savings_positive_fixture_is_safe_with_all_categories() {
    let bytes = savings_bytes(
        &[
            ("s1", "model_turn"),
            ("s2", "tool_call"),
            ("s3", "verifier_run"),
            ("s4", "tool_call"),
            ("s5", "model_turn"),
            ("s6", "model_turn"),
        ],
        &[
            ("s1", "reused"),
            ("s2", "private_execution"),
            ("s3", "verifier_collapsed"),
            ("s4", "proved_irrelevant"),
            ("s5", "policy_preauthorized"),
            ("s6", "baseline_preserved"),
        ],
    );
    let report = check_savings_provenance(&bytes).unwrap();
    assert_eq!(report.verdict, SafetyVerdict::Safe);
    assert!(report.grants_authority());
    assert!(report.certificate.is_some());
    assert_eq!(report.checker.id, VCQK_CHECKER_SAVINGS_ID);
    assert_eq!(report.scope, VCQK_SCOPE_SAVINGS);
    assert_eq!(report.contract, VCQK_CONTRACT_SAVINGS);
    assert_eq!(report.evidence.items.len(), 6);
    // Witness facts carry the category distribution.
    assert!(report.witness.facts.iter().any(|fact| fact == "reused: 1"));
    assert!(report
        .witness
        .facts
        .iter()
        .any(|fact| fact == "baseline_preserved: 1"));
    assert!(report
        .witness
        .facts
        .iter()
        .any(|fact| fact == "baseline segments: 6"));
    assert!(report.ledger.complete);
    assert_baseline_available(&report, bytes.len());
    assert_no_gate_fields(&report);
    // Deterministic.
    let again = check_savings_provenance(&bytes).unwrap();
    assert_eq!(report, again);
    assert_eq!(
        report.to_canonical_bytes().unwrap(),
        again.to_canonical_bytes().unwrap()
    );
}

#[test]
fn savings_missing_transcript_segment_is_unknown() {
    // s3 has no savings entry: missing evidence, no public saving claim.
    let bytes = savings_bytes(
        &[
            ("s1", "model_turn"),
            ("s2", "tool_call"),
            ("s3", "model_turn"),
        ],
        &[("s1", "reused"), ("s2", "private_execution")],
    );
    let report = check_savings_provenance(&bytes).unwrap();
    assert_eq!(
        report.verdict,
        SafetyVerdict::Unknown {
            reasons: vec!["segment_unmapped:s3".into()]
        }
    );
    assert!(!report.grants_authority());
    assert!(report.certificate.is_none());
    assert!(!report.ledger.complete);
    assert_baseline_available(&report, bytes.len());
    assert_no_gate_fields(&report);
}

#[test]
fn savings_duplicate_mapping_is_unsafe() {
    let bytes = savings_bytes(
        &[("s1", "model_turn"), ("s2", "tool_call")],
        &[
            ("s1", "reused"),
            ("s1", "private_execution"),
            ("s2", "reused"),
        ],
    );
    let report = check_savings_provenance(&bytes).unwrap();
    assert_eq!(
        report.verdict,
        SafetyVerdict::Unsafe {
            reasons: vec!["duplicate_mapping".into()]
        }
    );
    assert!(!report.grants_authority());
    assert!(report.ledger.complete);
}

#[test]
fn savings_entry_for_unknown_segment_is_unsafe() {
    let bytes = savings_bytes(
        &[("s1", "model_turn")],
        &[("s1", "reused"), ("ghost", "reused")],
    );
    let report = check_savings_provenance(&bytes).unwrap();
    assert_eq!(
        report.verdict,
        SafetyVerdict::Unsafe {
            reasons: vec!["unknown_segment_in_savings_map".into()]
        }
    );
    assert!(!report.grants_authority());
}

#[test]
fn savings_unsupported_category_is_unsafe() {
    let bytes = savings_bytes(&[("s1", "model_turn")], &[("s1", "teleported")]);
    let report = check_savings_provenance(&bytes).unwrap();
    assert_eq!(
        report.verdict,
        SafetyVerdict::Unsafe {
            reasons: vec!["unsupported_category".into()]
        }
    );
    assert!(!report.grants_authority());
    assert!(report.ledger.complete);
}

#[test]
fn savings_duplicate_baseline_segment_is_unsafe() {
    let bytes = savings_bytes(
        &[("s1", "model_turn"), ("s1", "model_turn")],
        &[("s1", "reused")],
    );
    let report = check_savings_provenance(&bytes).unwrap();
    assert_eq!(
        report.verdict,
        SafetyVerdict::Unsafe {
            reasons: vec!["duplicate_baseline_segment".into()]
        }
    );
    assert!(!report.grants_authority());
}

#[test]
fn savings_empty_baseline_is_unknown() {
    let bytes = savings_bytes(&[], &[]);
    let report = check_savings_provenance(&bytes).unwrap();
    assert_eq!(
        report.verdict,
        SafetyVerdict::Unknown {
            reasons: vec!["no_baseline_segments".into()]
        }
    );
    assert!(!report.grants_authority());
}

#[test]
fn savings_unparseable_document_is_unknown() {
    for bytes in [b"".as_slice(), b"{not json", b"[]", b"\"x\""] {
        let report = check_savings_provenance(bytes).unwrap();
        assert_eq!(
            report.verdict,
            SafetyVerdict::Unknown {
                reasons: vec!["unparseable_input".into()]
            }
        );
        assert!(!report.grants_authority());
    }
}

#[test]
fn savings_oversized_documents_are_unknown() {
    let many: Vec<Value> = (0..=VCQK_MAX_BASELINE_SEGMENTS)
        .map(|i| json!({"id": format!("s{i}"), "kind": "model_turn"}))
        .collect();
    let bytes = serde_json::to_vec(&json!({"baseline": many, "savings": []})).unwrap();
    let report = check_savings_provenance(&bytes).unwrap();
    assert_eq!(
        report.verdict,
        SafetyVerdict::Unknown {
            reasons: vec!["input_exceeds_checker_bounds".into()]
        }
    );
    assert!(!report.grants_authority());

    let too_many_entries: Vec<Value> = (0..=VCQK_MAX_SAVINGS_ENTRIES)
        .map(|i| json!({"segment": format!("s{i}"), "category": "reused"}))
        .collect();
    let bytes = serde_json::to_vec(&json!({"baseline": [], "savings": too_many_entries})).unwrap();
    assert_eq!(
        check_savings_provenance(&bytes).unwrap().verdict,
        SafetyVerdict::Unknown {
            reasons: vec!["input_exceeds_checker_bounds".into()]
        }
    );
}

#[test]
fn savings_classify_is_total() {
    assert_eq!(
        SavingsCategory::classify("reused"),
        Some(SavingsCategory::Reused)
    );
    assert_eq!(
        SavingsCategory::classify("private_execution"),
        Some(SavingsCategory::PrivateExecution)
    );
    assert_eq!(
        SavingsCategory::classify("proved_irrelevant"),
        Some(SavingsCategory::ProvedIrrelevant)
    );
    assert_eq!(
        SavingsCategory::classify("verifier_collapsed"),
        Some(SavingsCategory::VerifierCollapsed)
    );
    assert_eq!(
        SavingsCategory::classify("policy_preauthorized"),
        Some(SavingsCategory::PolicyPreauthorized)
    );
    assert_eq!(
        SavingsCategory::classify("baseline_preserved"),
        Some(SavingsCategory::BaselinePreserved)
    );
    for garbage in ["", "REUSED", "reused ", "teleported", "x"] {
        assert_eq!(SavingsCategory::classify(garbage), None);
    }
    assert_eq!(SavingsCategory::ALL.len(), 6);
}

// ---------------------------------------------------------------------------
// Kill metrics
// ---------------------------------------------------------------------------

#[test]
fn kill_false_authority_counts_refuted_safe_roots() {
    let mut metrics = KillMetrics::new();
    let report =
        check_causal_closure(&causal_bytes(&["d1"], &[("d1", "declared_output")], &[])).unwrap();
    let root = report.certificate.as_ref().unwrap().root.clone();
    metrics.observe_report(&report);
    assert_eq!(metrics.safe_reports(), 1);
    assert_eq!(metrics.tracked_certificate_roots(), 1);
    assert_eq!(metrics.false_authority(), 0);

    // Refutation of the tracked Safe root is the false-authority kill.
    metrics.observe_refutation(&root);
    assert_eq!(metrics.false_authority(), 1);
    assert_eq!(metrics.refutations_total(), 1);
    assert_eq!(metrics.tracked_certificate_roots(), 0);

    // Refuting an untracked root is not false authority.
    metrics.observe_refutation(ROOT_A);
    assert_eq!(metrics.false_authority(), 1);
    assert_eq!(metrics.refutations_total(), 2);
}

#[test]
fn kill_unsafe_and_unknown_reports_are_counted_without_roots() {
    let mut metrics = KillMetrics::new();
    let unsafe_report =
        check_causal_closure(&causal_bytes(&["ghost"], &[("d1", "declared_output")], &[])).unwrap();
    let unknown_report = check_causal_closure(b"garbage").unwrap();
    metrics.observe_report(&unsafe_report);
    metrics.observe_report(&unknown_report);
    assert_eq!(metrics.unsafe_reports(), 1);
    assert_eq!(metrics.unknown_reports(), 1);
    assert_eq!(metrics.tracked_certificate_roots(), 0);
}

#[test]
fn kill_non_converging_counterexamples() {
    let mut metrics = KillMetrics::new();
    // Under the bound: no kill.
    for _ in 0..VCQK_KILL_NONCONVERGENCE_MAX_ISSUES {
        metrics.observe_counterexample(ROOT_A);
    }
    assert_eq!(metrics.non_converging_counterexamples(), 0);
    assert_eq!(
        metrics.counterexample_reissues(),
        VCQK_KILL_NONCONVERGENCE_MAX_ISSUES - 1
    );
    // One more issue crosses the bound: exactly one kill per root.
    metrics.observe_counterexample(ROOT_A);
    assert_eq!(metrics.non_converging_counterexamples(), 1);
    metrics.observe_counterexample(ROOT_A);
    metrics.observe_counterexample(ROOT_A);
    assert_eq!(metrics.non_converging_counterexamples(), 1);
    // A second root under the bound stays clean.
    metrics.observe_counterexample(ROOT_B);
    assert_eq!(metrics.non_converging_counterexamples(), 1);
    metrics.observe_counterexample(ROOT_B);
    metrics.observe_counterexample(ROOT_B);
    metrics.observe_counterexample(ROOT_B);
    assert_eq!(metrics.non_converging_counterexamples(), 2);
}

#[test]
fn kill_savings_overhead() {
    let mut metrics = KillMetrics::new();
    metrics.observe_savings(10, 5);
    assert_eq!(metrics.savings_overhead(), 0);
    metrics.observe_savings(5, 5);
    assert_eq!(metrics.savings_overhead(), 0);
    metrics.observe_savings(5, 10);
    assert_eq!(metrics.savings_overhead(), 1);
    metrics.observe_savings(0, 1);
    assert_eq!(metrics.savings_overhead(), 2);
    assert!(!savings_overhead_killed(10, 5));
    assert!(!savings_overhead_killed(5, 5));
    assert!(savings_overhead_killed(5, 10));
}

#[test]
fn learning_and_refinement_have_no_publish_authority() {
    let mut metrics = KillMetrics::new();
    metrics.observe_report(
        &check_certificate_chain(&chain_bytes(&[link(
            ROOT_A,
            "scope:project/main",
            "zero.contract/v1",
            "1.0.0",
        )]))
        .unwrap(),
    );
    metrics.observe_counterexample(DIGEST_1);
    metrics.observe_savings(0, 1);
    // No API can raise learning publications: it is structurally zero.
    assert_eq!(metrics.learning_publications(), 0);
    assert!(KillMetrics::learning_has_no_publish_authority());
    assert!(!VCQK_LEARNING_REFINEMENT_PUBLISH_AUTHORITY);
    // Refinement evidence is observable only: a counterexample digest can be
    // recorded as evidence, but nothing here can turn it into a certificate.
    assert_eq!(metrics.non_converging_counterexamples(), 0);
}

// ---------------------------------------------------------------------------
// Totality on untrusted bytes
// ---------------------------------------------------------------------------

fn config() -> Config {
    Config {
        cases: if cfg!(miri) { 8 } else { 64 },
        failure_persistence: None,
        ..Config::default()
    }
}

fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        any::<String>().prop_map(Value::String),
        Just(Value::Null),
    ];
    leaf.prop_recursive(3, 32, 8, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..16).prop_map(Value::Array),
            prop::collection::btree_map(any::<String>(), inner, 0..16)
                .prop_map(|map| Value::Object(map.into_iter().collect())),
        ]
    })
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn checkers_never_panic_or_fail_on_arbitrary_bytes(bytes in any::<Vec<u8>>()) {
        // Totality by construction: every input maps to a report, never a
        // panic and never an Err.
        assert!(check_certificate_chain(&bytes).is_ok());
        assert!(check_causal_closure(&bytes).is_ok());
        assert!(check_savings_provenance(&bytes).is_ok());
    }

    #[test]
    fn checkers_never_panic_or_fail_on_arbitrary_json(value in json_value()) {
        let bytes = serde_json::to_vec(&value).unwrap();
        let chain = check_certificate_chain(&bytes).unwrap();
        let causal = check_causal_closure(&bytes).unwrap();
        let savings = check_savings_provenance(&bytes).unwrap();
        // Every emitted document is canonical and revalidates.
        assert!(V7ShadowReport::from_canonical_bytes(&chain.to_canonical_bytes().unwrap()).is_ok());
        assert!(V7ShadowReport::from_canonical_bytes(&causal.to_canonical_bytes().unwrap()).is_ok());
        assert!(V7ShadowReport::from_canonical_bytes(&savings.to_canonical_bytes().unwrap()).is_ok());
        // Non-Safe verdicts never serialize a certificate. The JSON key
        // (with quotes) cannot appear inside string values, so this is exact
        // even though falsifier descriptions mention "certificate".
        for report in [&chain, &causal, &savings] {
            if !report.verdict.grants_authority() {
                assert!(report.certificate.is_none());
                let text = String::from_utf8(report.to_canonical_bytes().unwrap()).unwrap();
                assert!(!text.contains("\"certificate\""));
            }
        }
    }

    #[test]
    fn chain_checkers_are_deterministic(bytes in any::<Vec<u8>>()) {
        assert_eq!(
            check_certificate_chain(&bytes).unwrap(),
            check_certificate_chain(&bytes).unwrap()
        );
        assert_eq!(
            check_causal_closure(&bytes).unwrap(),
            check_causal_closure(&bytes).unwrap()
        );
        assert_eq!(
            check_savings_provenance(&bytes).unwrap(),
            check_savings_provenance(&bytes).unwrap()
        );
    }

    #[test]
    fn kill_metrics_never_panic_on_arbitrary_strings(value in any::<String>()) {
        let mut metrics = KillMetrics::new();
        metrics.observe_counterexample(&value);
        metrics.observe_refutation(&value);
        metrics.observe_savings(value.len() as u64, value.len() as u64);
        assert_eq!(metrics.learning_publications(), 0);
    }
}

#[test]
fn checkers_are_total_on_nasty_hand_written_inputs() {
    for bytes in [
        b"".as_slice(),
        b"\x00\x01\x02",
        b"\xff\xfe\xfd\xfc",
        b"null",
        b"true",
        b"[]",
        b"[null]",
        b"[[[[[[[[[[[[[[[[",
        b"{\"a\":{\"b\":{\"c\":[1,{\"d\":null}]}}}",
    ] {
        assert!(check_certificate_chain(bytes).is_ok());
        assert!(check_causal_closure(bytes).is_ok());
        assert!(check_savings_provenance(bytes).is_ok());
    }
}

#[test]
fn checkers_are_versioned() {
    assert_eq!(
        check_certificate_chain(b"[]").unwrap().checker.id,
        VCQK_CHECKER_CHAIN_ID
    );
    assert_eq!(
        check_causal_closure(b"[]").unwrap().checker.id,
        VCQK_CHECKER_CAUSAL_ID
    );
    assert_eq!(
        check_savings_provenance(b"[]").unwrap().checker.id,
        VCQK_CHECKER_SAVINGS_ID
    );
    for report in [
        check_certificate_chain(b"[]").unwrap(),
        check_causal_closure(b"[]").unwrap(),
        check_savings_provenance(b"[]").unwrap(),
    ] {
        assert_eq!(report.checker.version, "1.0.0");
        assert_eq!(report.schema, "zerostack/v7-shadow-report/1");
    }
}
