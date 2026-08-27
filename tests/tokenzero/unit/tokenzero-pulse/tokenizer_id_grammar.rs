//! SPEC-TZ-TOK-002: Pulse tokenizer_id is estimator:<slug>, tiktoken:<encoding>,
//! or provider/model@hex. Never treat the default or tiktoken: as
//! ExactTokenizerIdentity / Q99.

use tokenzero_pulse::{
    PulseEvent, SessionLedgerReport, pulse_counts_certified, pulse_counts_class,
    pulse_task_lossless, record_event, report_for_path,
};

#[test]
fn default_tool_call_is_labelled_estimator_not_exact() {
    let event = PulseEvent::tool_call("read", "auto", 1, 1, 0, 0, 0, None);
    assert_eq!(event.tokenizer_id, "estimator:tokenzero-core");
    assert!(event.tokenizer_id.starts_with("estimator:"));
}

#[test]
fn tokenizer_id_grammar_accepts_estimator_and_digest_rejects_q99() {
    let event = PulseEvent::tool_call("read", "auto", 1, 1, 0, 0, 0, None);
    event
        .clone()
        .with_tokenizer_id("estimator:bytes-ceil-div4")
        .expect("labelled estimator");
    let digest = "a".repeat(64);
    event
        .clone()
        .with_tokenizer_id(&format!("openai/gpt-4@{digest}"))
        .expect("provider/model@hex");
    event
        .clone()
        .with_tokenizer_id("tiktoken:o200k_base")
        .expect("labelled bundled BPE");
    event
        .clone()
        .with_tokenizer_id("tiktoken:cl100k_base")
        .expect("cl100k");
    assert!(event.clone().with_tokenizer_id("tiktoken:").is_err());
    let unlabeled = event
        .clone()
        .with_tokenizer_id("estimate:tokenzero-lexical")
        .expect_err("estimate: is unlabeled, not estimator:");
    assert!(
        unlabeled.contains("estimate:"),
        "refusal must name the unlabeled prefix, got {unlabeled}"
    );
    assert!(
        unlabeled.contains("estimator:"),
        "refusal must name the labeled grammar, got {unlabeled}"
    );
    let q99 = event
        .clone()
        .with_tokenizer_id("Q99")
        .expect_err("Q99 is never exact");
    assert!(
        q99.contains("Q99") && q99.contains("never exact"),
        "Q99 refusal must be specific, got {q99}"
    );
    assert!(event.clone().with_tokenizer_id("exact").is_err());
    assert!(event.clone().with_tokenizer_id("gpt-4o").is_err());
    assert!(
        event.clone().with_tokenizer_id("estimator:q99").is_err(),
        "Q99 slug is not an exact tokenizer and not a labeled estimator"
    );
    assert!(
        event
            .with_tokenizer_id("EngineIdentity::TokenZero")
            .is_err()
    );
}

#[test]
fn tiktoken_label_is_not_exact_tokenizer_identity() {
    let id = "tiktoken:o200k_base";
    PulseEvent::tool_call("measure", "auto", 1, 1, 0, 0, 0, None)
        .with_tokenizer_id(id)
        .expect("Pulse must accept the kernel tiktoken: class");
    assert!(id.starts_with("tiktoken:"));
    assert!(!id.contains('@'), "tiktoken: is not provider/model@hex");
    assert!(!id.starts_with("estimator:"));
}

#[test]
fn pulse_report_spent_above_raw_is_negative_savings_not_a_clamped_save() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("events.jsonl");
    let event = PulseEvent::tool_call("compress", "auto", 10, 15, 0, 0, 0, None);
    assert_eq!(event.tokenizer_id, "estimator:tokenzero-core");
    record_event(&path, &event).expect("record");
    let report = report_for_path(&path).expect("report");
    assert!(
        report.visible_savings < 0.0,
        "spent>raw must not clamp to a 0% save, got {}",
        report.visible_savings
    );
    assert!((report.visible_savings - (-0.5)).abs() < 1e-12);
    assert!(report.recovery_adjusted_savings < 0.0);
}

#[test]
fn omitted_without_recovery_is_not_task_lossless() {
    let event = PulseEvent::tool_call("compress", "auto", 10, 4, 0, 0, 0, None);
    assert!(
        !event.task_lossless,
        "visible<raw with recovery=0 must not count as lossless"
    );
    assert!(!pulse_task_lossless(10, 4, 0));
    assert!(
        pulse_task_lossless(10, 4, 6),
        "recovery charged back is lossless"
    );
    assert!(pulse_task_lossless(10, 10, 0));
}

#[test]
fn pulse_report_spent_is_visible_plus_recovery() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("events.jsonl");
    let event = PulseEvent::tool_call("expand", "auto", 10, 4, 6, 1, 0, None);
    record_event(&path, &event).expect("record");
    let report = report_for_path(&path).expect("report");
    assert_eq!(report.spent_tokens, 10, "spent = visible + recovery");
    assert_eq!(report.visible_tokens, 4);
    assert_eq!(report.recovery_tokens, 6);
    assert!(event.task_lossless);
    assert_eq!(report.recovery_adjusted_savings, 0.0);
}

#[test]
fn pulse_report_estimator_totals_are_labelled_not_certified() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("events.jsonl");
    record_event(
        &path,
        &PulseEvent::tool_call("read", "auto", 10, 4, 0, 0, 0, None),
    )
    .expect("record");
    let report = report_for_path(&path).expect("report");
    assert_eq!(report.tokenizer_id, "estimator:tokenzero-core");
    assert_eq!(report.counts_class, "estimator");
    assert!(
        !report.certified,
        "estimator CLI totals must not certify as exact"
    );
    assert!(report.savings_commensurate);
    assert_eq!(report.status, "ok");
    assert!(!pulse_counts_certified(&report.tokenizer_id));
    assert_eq!(pulse_counts_class(&report.tokenizer_id), "estimator");
}

#[test]
fn pulse_report_mixed_tokenizer_ids_are_not_commensurate_or_certified() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("events.jsonl");
    record_event(
        &path,
        &PulseEvent::tool_call("read", "auto", 10, 4, 0, 0, 0, None),
    )
    .expect("estimator event");
    let tiktoken = PulseEvent::tool_call("measure", "auto", 10, 8, 0, 0, 0, None)
        .with_tokenizer_id("tiktoken:cl100k_base")
        .expect("tiktoken");
    record_event(&path, &tiktoken).expect("tiktoken event");
    let report = report_for_path(&path).expect("report");
    assert_eq!(report.tokenizer_id, "mixed");
    assert_eq!(report.counts_class, "mixed");
    assert!(!report.certified, "mixed units must not certify as exact");
    assert!(
        !report.savings_commensurate,
        "estimator+tiktoken savings are not one billed unit"
    );
    assert_eq!(report.status, "mixed_tokenizer");
}

#[test]
fn pulse_report_exact_identity_is_certified_when_unmixed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("events.jsonl");
    let digest = "a".repeat(64);
    let id = format!("openai/gpt-4@{digest}");
    let event = PulseEvent::tool_call("measure", "auto", 8, 8, 0, 0, 0, None)
        .with_tokenizer_id(&id)
        .expect("exact identity");
    record_event(&path, &event).expect("record");
    let report = report_for_path(&path).expect("report");
    assert_eq!(report.tokenizer_id, id);
    assert_eq!(report.counts_class, "exact");
    assert!(report.certified);
    assert!(report.savings_commensurate);
}

#[test]
fn session_ledger_mixed_tokenizer_totals_drop_headline_dpmt() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("events.jsonl");
    let mut estimator = PulseEvent::tool_call("read", "auto", 10, 4, 0, 0, 0, None);
    estimator.session_id = Some("s1".into());
    record_event(&path, &estimator).expect("estimator");
    let mut tiktoken = PulseEvent::tool_call("measure", "auto", 10, 8, 0, 0, 0, None)
        .with_tokenizer_id("tiktoken:o200k_base")
        .expect("tiktoken");
    tiktoken.session_id = Some("s1".into());
    record_event(&path, &tiktoken).expect("tiktoken");
    let report = SessionLedgerReport::from_ledger(&path).expect("ledger");
    assert_eq!(report.tokenizer_id, "mixed");
    assert_eq!(report.counts_class, "mixed");
    assert!(!report.certified);
    assert!(!report.savings_commensurate);
    assert!(
        report.dpmt.is_none(),
        "headline DPMT must not mix estimator and tiktoken units"
    );
    assert_eq!(report.sessions.len(), 2);
    assert!(
        report
            .sessions
            .iter()
            .any(|row| row.counts_class == "estimator" && !row.certified)
    );
    assert!(
        report
            .sessions
            .iter()
            .any(|row| row.counts_class == "tiktoken" && !row.certified)
    );
}
