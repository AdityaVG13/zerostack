use tokenzero_core::representation_economics::RepresentationResources;
use tokenzero_core::{
    EvidenceFreshness, LiveCandidate, LiveParetoDecision, MetricOrder, Mode, ProtectedOutcome,
    VerifierIdentity, count_tokens, decide_live_pareto, make_capsule,
    make_capsule_with_recovery_ref, savings_ratio, savings_ratio_u64,
};

fn resources(value: u64) -> RepresentationResources {
    RepresentationResources {
        stored_bytes: value,
        wire_bytes: value,
        source_tokens: value,
        visible_tokens: value,
        expansion_work: value,
        verification_work: value,
        latency_micros: value,
        metadata_bytes: value,
    }
}

fn protected(metric: &str, order: MetricOrder, baseline: i64, candidate: i64) -> ProtectedOutcome {
    ProtectedOutcome {
        metric_id: metric.into(),
        order,
        baseline_value: baseline,
        candidate_value: candidate,
    }
}

fn candidate(
    id: &str,
    freshness: EvidenceFreshness,
    protected_vector: Vec<ProtectedOutcome>,
    resource_cost: u64,
    exact: bool,
) -> LiveCandidate {
    LiveCandidate {
        candidate_id: id.into(),
        semantic_root: "semantic".into(),
        adapter_root: "adapter".into(),
        verifier: VerifierIdentity {
            verifier_id: "verifier".into(),
            verifier_version: "1".into(),
        },
        freshness,
        protected_vector,
        resources: resources(resource_cost),
        exact,
    }
}

#[test]
fn live_pareto_digest_is_deterministic_and_detects_tampering() {
    let input = candidate(
        "candidate",
        EvidenceFreshness::Fresh,
        vec![protected("accuracy", MetricOrder::AtLeast, 80, 90)],
        10,
        true,
    );

    let first = decide_live_pareto(std::slice::from_ref(&input)).expect("first decision");
    let second = decide_live_pareto(&[input]).expect("second decision");
    assert_eq!(first.decision_digest, second.decision_digest);
    assert_eq!(first.canonical_json, second.canonical_json);
    assert_eq!(
        LiveParetoDecision::from_canonical_bytes(first.canonical_json.as_bytes())
            .expect("decode canonical decision"),
        first
    );

    let mut tampered = first;
    tampered.entries[0].resources = resources(1);
    assert!(tampered.validate().is_err());
}

#[test]
fn stale_unknown_and_missing_evidence_never_hide_fresh_candidate() {
    let fresh = candidate(
        "fresh",
        EvidenceFreshness::Fresh,
        vec![protected("accuracy", MetricOrder::AtLeast, 80, 90)],
        10,
        true,
    );
    let stale = candidate(
        "stale",
        EvidenceFreshness::Stale,
        vec![protected("accuracy", MetricOrder::AtLeast, 80, 95)],
        1,
        true,
    );
    let unknown = candidate(
        "unknown",
        EvidenceFreshness::Unknown,
        vec![protected("accuracy", MetricOrder::AtLeast, 80, 95)],
        1,
        true,
    );
    let missing = candidate("missing", EvidenceFreshness::Missing, vec![], 1, true);

    let decision = decide_live_pareto(&[stale, unknown, missing, fresh]).expect("decision");
    assert_eq!(decision.frontier_ids, vec!["fresh"]);
    assert_eq!(
        decision.entries.len(),
        4,
        "non-frontier evidence remains visible"
    );
    let stale_entry = decision
        .entries
        .iter()
        .find(|entry| entry.candidate_id == "stale")
        .expect("stale entry");
    assert!(
        stale_entry
            .reasons
            .iter()
            .any(|reason| reason == "stale_evidence")
    );
}

#[test]
fn incomparable_protected_metrics_remain_on_frontier() {
    let accuracy = candidate(
        "accuracy",
        EvidenceFreshness::Fresh,
        vec![protected("accuracy", MetricOrder::AtLeast, 80, 90)],
        5,
        true,
    );
    let latency = candidate(
        "latency",
        EvidenceFreshness::Fresh,
        vec![protected("latency", MetricOrder::AtMost, 100, 50)],
        5,
        true,
    );

    let decision = decide_live_pareto(&[accuracy, latency]).expect("decision");
    assert_eq!(decision.frontier_ids.len(), 2);
}

#[test]
fn dominance_requires_exact_complete_nonregressing_evidence() {
    let exact = candidate(
        "exact",
        EvidenceFreshness::Fresh,
        vec![protected("accuracy", MetricOrder::AtLeast, 80, 90)],
        10,
        true,
    );
    let inexact = candidate(
        "inexact",
        EvidenceFreshness::Fresh,
        vec![protected("accuracy", MetricOrder::AtLeast, 80, 90)],
        10,
        false,
    );
    assert_eq!(
        decide_live_pareto(&[exact, inexact])
            .expect("exactness decision")
            .frontier_ids,
        vec!["exact"]
    );

    let missing = candidate("missing", EvidenceFreshness::Fresh, vec![], 10, true);
    let complete = candidate(
        "complete",
        EvidenceFreshness::Fresh,
        vec![protected("accuracy", MetricOrder::AtLeast, 80, 90)],
        10,
        true,
    );
    assert_eq!(
        decide_live_pareto(&[missing, complete])
            .expect("completeness decision")
            .frontier_ids,
        vec!["complete"]
    );
}

#[test]
fn savings_accounting_preserves_overspend_as_negative() {
    assert!((savings_ratio(10, 15) - (-0.5)).abs() < 1e-12);
    assert_eq!(savings_ratio(10, 10), 0.0);
    assert!((savings_ratio(10, 5) - 0.5).abs() < 1e-12);
    assert!(savings_ratio_u64(4, 10) < 0.0);
}

#[test]
fn exact_capsules_never_cost_more_than_raw_and_keep_recovery_selectors() {
    let tiny = "hi";
    let passthrough = make_capsule(tiny, Mode::Exact, 64, None).expect("tiny capsule");
    assert_eq!(passthrough.text, tiny);
    assert_eq!(passthrough.visible_tokens, count_tokens(tiny));
    assert_eq!(passthrough.mode, Mode::Passthrough);

    let text = (0..80)
        .map(|index| format!("tok{index:02}"))
        .collect::<Vec<_>>()
        .join(" ");
    let digest = "a".repeat(64);
    for selector in ["#L1-3", "#L1-L3", "#B0+5"] {
        let handle = format!("z://blob/{digest}{selector}");
        let capsule = make_capsule_with_recovery_ref(
            &text,
            count_tokens(&text),
            Mode::Structured,
            8,
            None,
            Some(&handle),
        )
        .expect("capsule with recovery selector");
        assert!(
            capsule
                .exact_refs
                .iter()
                .any(|reference| reference.contains(selector))
                || capsule.text.contains(selector),
            "missing recovery selector {selector}: text={:?} refs={:?}",
            capsule.text,
            capsule.exact_refs
        );
    }
}

#[test]
fn harness_model_environment_is_not_product_configuration() {
    const CHILD: &str = "ZEROSTACK_TEST_TOKEN_MODEL_CHILD";
    if std::env::var_os(CHILD).is_some() {
        assert_eq!(tokenzero_core::active_model_id(), None);
        return;
    }

    let status = std::process::Command::new(std::env::current_exe().expect("current test binary"))
        .args([
            "harness_model_environment_is_not_product_configuration",
            "--nocapture",
        ])
        .env_remove("TOKENZERO_MODEL")
        .env_remove("OPENAI_MODEL")
        .env("OMP_MODEL", "gpt-4o")
        .env(CHILD, "1")
        .status()
        .expect("run isolated environment probe");
    assert!(status.success(), "child accepted non-product OMP_MODEL");
}
