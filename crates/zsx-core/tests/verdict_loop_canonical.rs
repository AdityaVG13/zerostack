#![cfg(all(feature = "fszero", feature = "graphzero", feature = "tokenzero"))]

use std::time::Duration;

use zero_abi::WorkerTokenCountKind;
use zsx_core::{VerdictDecision, VerdictLoopEnvelope, ZsxSession};

#[test]
fn canonical_token_job_verdict_loop_is_bounded_and_non_estimated() {
    let workspace = tempfile::tempdir().expect("workspace");
    let state = tempfile::tempdir().expect("state");
    let root = workspace.path().canonicalize().expect("workspace root");
    let session = ZsxSession::builder(&root)
        .with_state_root(state.path())
        .with_session_id("canonical-verdict-loop")
        .build_canonical()
        .expect("canonical session");
    let envelope = VerdictLoopEnvelope {
        max_logical_dispatches: 6,
        max_raw_worker_input_bytes: 128 * 1024,
        max_raw_worker_output_bytes: 512 * 1024,
        max_raw_tokens: 512 * 1024,
        max_visible_tokens: 512 * 1024,
        max_recovery_tokens: 512 * 1024,
        max_billed_tokens: 512 * 1024,
        max_cached_tokens: 512 * 1024,
    };
    let verdict = session
        .execute_verdict_loop(
            1,
            1,
            r#"const launched=await zero.token.shell("printf canonical-verdict-ok",{background:true});
               const id=launched.content.value.value.job;
               let cursor=0;
               let terminal=null;
               let output="";
               for(let attempt=0;attempt<5;attempt++){
                 const polled=await zero.token.job(id,{waitMs:30000,since:cursor,tailBytes:64});
                 const value=polled.content.value.value;
                 cursor=value.cursor;
                 output+=value.tail;
                 if(value.status!=="running"){terminal=value;break;}
               }
               if(terminal===null)throw new Error("job did not settle inside the declared loop");
               if(terminal.status!=="exited"||output!=="canonical-verdict-ok"){
                 throw new Error("job terminal assertion failed");
               }
               return "pass";"#,
            Duration::from_secs(60),
            envelope,
        )
        .expect("canonical verdict loop");
    assert_eq!(verdict.decision, VerdictDecision::Pass);
    assert!((2..=6).contains(&verdict.receipt.logical_dispatches));
    assert_eq!(
        verdict.receipt.count_kinds,
        vec![WorkerTokenCountKind::ConservativeUpperBound]
    );
    assert_eq!(
        verdict.receipt.tokenizer_ids,
        vec!["conservative:utf8-json-bytes-v1"]
    );
    assert_eq!(verdict.receipt.exact_ref_tokens, None);
    assert_eq!(verdict.receipt.final_atom_json_bytes, 6);
    session.shutdown().expect("shutdown");
}
