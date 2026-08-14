#![cfg(feature = "fixture-adapters")]

use std::sync::Arc;
use std::time::Duration;

use zero_abi::WorkerTokenCountKind;
use zsx_core::fixture::fixture_adapters;
use zsx_core::{VerdictDecision, VerdictLoopEnvelope, ZsxSession, ZsxSessionFailureCode};

fn envelope(max_dispatches: u64) -> VerdictLoopEnvelope {
    VerdictLoopEnvelope {
        max_logical_dispatches: max_dispatches,
        max_raw_worker_input_bytes: 64 * 1024,
        max_raw_worker_output_bytes: 64 * 1024,
        max_raw_tokens: 1_000,
        max_visible_tokens: 1_000,
        max_recovery_tokens: 1_000,
        max_billed_tokens: 1_000,
        max_cached_tokens: 1_000,
    }
}

fn fixture_session() -> (
    tempfile::TempDir,
    ZsxSession,
    [Arc<zsx_core::fixture::FixtureAdapter>; 3],
) {
    let root = tempfile::tempdir().expect("root");
    let root_path = root.path().canonicalize().expect("canonical root");
    let (fs, graph, token) = fixture_adapters(&root_path, "verdict-loop");
    let session = ZsxSession::builder(&root_path)
        .with_session_id("verdict-loop")
        .fszero(fs.clone())
        .graphzero(graph.clone())
        .tokenzero(token.clone())
        .build()
        .expect("session");
    (root, session, [fs, graph, token])
}

#[test]
fn production_session_launches_polls_asserts_and_returns_only_an_atom() {
    let (_root, session, adapters) = fixture_session();
    let verdict = session
        .execute_verdict_loop(
            1,
            1,
            r#"const launched=await zero.token.shell("fixture",{background:true});
               const id=launched.content.value.value.job;
               const poll=await zero.token.job(id,{waitMs:0,since:0,tailBytes:32});
               const result=poll.content.value.value;
               if(result.status!=="exited"||result.tail!=="ok")throw new Error("job verdict failed");
               return "pass";"#,
            Duration::from_secs(5),
            envelope(2),
        )
        .expect("verdict loop");
    assert_eq!(verdict.decision, VerdictDecision::Pass);
    assert_eq!(verdict.receipt.logical_dispatches, 2);
    assert_eq!(verdict.receipt.raw_tokens, 16);
    assert_eq!(verdict.receipt.visible_tokens, 8);
    assert_eq!(verdict.receipt.billed_tokens, 16);
    assert_eq!(verdict.receipt.cached_tokens, 4);
    assert_eq!(verdict.receipt.exact_ref_tokens, Some(0));
    assert_eq!(
        verdict.receipt.count_kinds,
        vec![WorkerTokenCountKind::Exact]
    );
    assert_eq!(verdict.receipt.tokenizer_ids, vec!["fixture-tokenizer-v1"]);
    assert_eq!(verdict.receipt.final_atom_json_bytes, 6);
    assert!(verdict.receipt.raw_worker_input_bytes > 0);
    assert!(verdict.receipt.raw_worker_output_bytes > 0);
    assert_eq!(adapters[2].calls(), 2);
    session.shutdown().expect("shutdown");
}

#[test]
fn dispatch_budget_rejects_before_additional_adapter_work_even_when_caught() {
    let (_root, session, adapters) = fixture_session();
    let error = session
        .execute_verdict_loop(
            1,
            2,
            r#"await zero.fs.compound("read",{});
               try{await zero.graph.index();}catch(_error){}
               return "pass";"#,
            Duration::from_secs(5),
            envelope(1),
        )
        .expect_err("second dispatch must exceed the envelope");
    assert!(
        error
            .to_string()
            .contains("logical_dispatches budget exceeded"),
        "{error}"
    );
    assert_eq!(error.code, ZsxSessionFailureCode::VerdictRejected);
    assert_eq!(adapters[0].calls(), 1);
    assert_eq!(adapters[1].calls(), 0);
    session.shutdown().expect("shutdown");
}

#[test]
fn missing_estimated_and_failed_accounting_are_sticky() {
    for (request_id, args, expected) in [
        (
            3,
            r#"{__fixture_accounting:"missing"}"#,
            "omitted worker token accounting",
        ),
        (
            4,
            r#"{__fixture_accounting:"estimate"}"#,
            "estimated token accounting",
        ),
        (
            5,
            r#"{__fixture_fail:true}"#,
            "fixture adapter failed by request",
        ),
    ] {
        let (_root, session, _adapters) = fixture_session();
        let source = format!(
            "try{{await zero.fs.compound(\"read\",{args});}}catch(_error){{}} return \"pass\";"
        );
        let error = session
            .execute_verdict_loop(1, request_id, source, Duration::from_secs(5), envelope(1))
            .expect_err("caught connector failure must remain terminal");
        assert!(
            error.to_string().contains(expected),
            "expected {expected}: {error}"
        );
        assert_eq!(error.code, ZsxSessionFailureCode::VerdictRejected);
        session.shutdown().expect("shutdown");
    }
}

#[test]
fn non_atom_result_is_rejected_after_metered_work() {
    let (_root, session, _adapters) = fixture_session();
    let error = session
        .execute_verdict_loop(
            1,
            6,
            r#"await zero.fs.compound("read",{});return {decision:"pass"};"#,
            Duration::from_secs(5),
            envelope(1),
        )
        .expect_err("structured result is not a decision atom");
    assert!(
        error
            .to_string()
            .contains("exactly the string pass or fail"),
        "{error}"
    );
    assert_eq!(error.code, ZsxSessionFailureCode::VerdictRejected);
    session.shutdown().expect("shutdown");
}

#[test]
fn accounting_budget_and_overflow_stop_later_dispatches() {
    let (_root, session, adapters) = fixture_session();
    let mut tight = envelope(2);
    tight.max_raw_tokens = 7;
    let error = session
        .execute_verdict_loop(
            1,
            7,
            r#"try{await zero.fs.compound("read",{});}catch(_error){}
               try{await zero.graph.index();}catch(_error){}
               return "pass";"#,
            Duration::from_secs(5),
            tight,
        )
        .expect_err("raw-token budget must fail before graph work");
    assert!(
        error.to_string().contains("raw_tokens budget exceeded"),
        "{error}"
    );
    assert_eq!(adapters[0].calls(), 1);
    assert_eq!(adapters[1].calls(), 0);
    session.shutdown().expect("shutdown");

    let (_root, session, _adapters) = fixture_session();
    let mut unbounded_raw = envelope(2);
    unbounded_raw.max_raw_tokens = u64::MAX;
    let error = session
        .execute_verdict_loop(
            1,
            8,
            r#"await zero.fs.compound("read",{__fixture_accounting:"max"});
               try{await zero.graph.index();}catch(_error){}
               return "pass";"#,
            Duration::from_secs(5),
            unbounded_raw,
        )
        .expect_err("inconsistent max accounting must fail closed");
    // W2: the resource ledger gate now rejects the internally inconsistent
    // accounting (raw=u64::MAX with billed=8) before the meter's second
    // addition can overflow -- a stronger, earlier fail-closed gate. The
    // graph dispatch must never run.
    assert!(
        error.to_string().contains("resource ledger charge")
            || error.to_string().contains("raw_tokens overflowed"),
        "{error}"
    );
    session.shutdown().expect("shutdown");
}
