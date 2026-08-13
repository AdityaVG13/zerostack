#![cfg(feature = "fszero")]

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use zero_abi::{
    ApprovalMetadata, ApprovalState, EffectClass, EngineIdentity, RefOwnership, RevertMetadata,
    WorkerResult, WorkerResultMetadata,
};
use zsx_core::fszero::FsZeroAdapter;
use zsx_core::{
    AdapterBinding, AdapterCall, AdapterError, AdapterResponse, DomainAdapter, ZsxSession,
};

struct UnusedAdapter {
    engine: EngineIdentity,
    session_id: String,
}

impl UnusedAdapter {
    fn new(engine: EngineIdentity, session_id: &str) -> Self {
        Self {
            engine,
            session_id: session_id.into(),
        }
    }
}

impl DomainAdapter for UnusedAdapter {
    fn engine(&self) -> EngineIdentity {
        self.engine
    }

    fn binding(&self) -> AdapterBinding {
        AdapterBinding::new(
            self.engine,
            "aggregate-world-unused",
            "aggregate-world-unused.v1",
            "a".repeat(64),
            "b".repeat(64),
            match self.engine {
                EngineIdentity::FsZero => "fz://",
                EngineIdentity::GraphZero => "gz://",
                EngineIdentity::TokenZero => "tz://",
            },
        )
        .expect("unused binding")
    }

    fn call(&self, call: AdapterCall<'_>) -> Result<AdapterResponse, AdapterError> {
        Ok(AdapterResponse {
            result: WorkerResult {
                value: json!({"unused":true}),
                metadata: WorkerResultMetadata {
                    effect: EffectClass::ReadOnly,
                    approval: ApprovalMetadata {
                        state: ApprovalState::NotRequired,
                        approval_id: None,
                        policy: None,
                    },
                    revert: RevertMetadata {
                        supported: false,
                        journal_id: None,
                        rollback_op: None,
                    },
                    ownership: RefOwnership {
                        engine: self.engine,
                        session_id: self.session_id.clone(),
                        refs: Vec::new(),
                        snapshot: None,
                    },
                    trace: call.request.trace.clone(),
                },
            },
            engine_timeline: None,
            worker_token_accounting: None,
        })
    }
}

#[test]
fn one_call_atomically_commits_and_verifies_one_hundred_files() {
    const FILES: usize = 100;
    let workspace = tempfile::tempdir().expect("workspace");
    let state = tempfile::tempdir().expect("state");
    let root = workspace.path().canonicalize().expect("workspace root");
    let state_root = state.path().canonicalize().expect("state root");
    let session_id = "aggregate-world-100";

    let mut edit_specs = Vec::with_capacity(FILES);
    let mut paths = Vec::with_capacity(FILES);
    for index in 0..FILES {
        let path = format!("file-{index:03}.txt");
        std::fs::write(root.join(&path), format!("old-{index:03}\n")).expect("seed file");
        edit_specs.push(format!("{path}:old-{index:03}|new-{index:03}"));
        paths.push(path);
    }
    let batch = format!("newbatch:{}", edit_specs.join(";;"));
    let batch_json = serde_json::to_string(&batch).expect("batch JSON");
    let paths_json = serde_json::to_string(&paths).expect("paths JSON");
    let plan = format!(
        r#"const created=await zero.fs.world({batch_json});
            const createdDomain=created.content.value.value;
            if(!createdDomain.ok)throw new Error("world create failed");
            const detail=createdDomain.value.detail;
            if(!detail.includes("world:1 "))throw new Error("world id missing");
            const world=detail.split("world:1 ")[1].split(" ")[0];
            const committed=await zero.fs.world("commit",{{world}});
            const commitDomain=committed.content.value.value;
            if(!commitDomain.ok)throw new Error("world commit failed");
            const verified=await zero.fs.read_many({paths_json});
            if(!verified.content.value.value.ok)throw new Error("read-many verify failed");
            return {{world,commit:commitDomain.value.detail,refs:commitDomain.refs,verified:true}};"#
    );

    let session = ZsxSession::builder(root.clone())
        .with_state_root(state_root)
        .with_session_id(session_id)
        .fszero(Arc::new(FsZeroAdapter::new_with_state_root(
            &root,
            state.path(),
            session_id,
        )))
        .graphzero(Arc::new(UnusedAdapter::new(
            EngineIdentity::GraphZero,
            session_id,
        )))
        .tokenzero(Arc::new(UnusedAdapter::new(
            EngineIdentity::TokenZero,
            session_id,
        )))
        .build()
        .expect("session");

    let result = session
        .execute(1, 1, &plan, Duration::from_secs(30))
        .expect("one-call aggregate world");
    assert_eq!(result.value["verified"], true);
    assert_eq!(result.metrics.host.logical_operations, 3);
    assert_eq!(result.metrics.host.physical_dispatches, 3);
    assert_eq!(result.metrics.engine_dispatches, [3, 0, 0]);
    assert!(
        result.value["commit"]
            .as_str()
            .is_some_and(|detail| detail.contains("commit:W")),
        "unexpected result: {}",
        result.value
    );
    let visible_bytes = serde_json::to_vec(&result.value)
        .expect("result JSON")
        .len();
    assert!(
        visible_bytes <= 2_000,
        "100-file aggregate leaked {visible_bytes} visible bytes"
    );
    for (index, path) in paths.iter().enumerate() {
        assert_eq!(
            std::fs::read_to_string(root.join(path)).expect("committed file"),
            format!("new-{index:03}\n")
        );
    }
    println!(
        "{}",
        json!({
            "schema":"zerostack.aggregate_world_100.v1",
            "zerostack_source_sha":option_env!("ZEROSTACK_SOURCE_SHA").unwrap_or("unbound"),
            "zerostack_diff_sha256":option_env!("ZEROSTACK_WORKTREE_DIFF_SHA256").unwrap_or("unbound"),
            "fszero_source_sha":option_env!("FSZERO_SOURCE_SHA").unwrap_or("unbound"),
            "fszero_diff_sha256":option_env!("FSZERO_WORKTREE_DIFF_SHA256").unwrap_or("unbound"),
            "files":FILES,
            "model_visible_calls":1,
            "visible_bytes":visible_bytes,
            "metrics":result.metrics,
            "verified":true,
        })
    );
    session.shutdown().expect("shutdown");
}
