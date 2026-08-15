//! End-to-end tests for per-request cancellation and the manual mutation
//! attempt journal reconciliation API, exercised only through the public
//! `ZsxSession` surface with in-test `DomainAdapter`s.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

use serde_json::json;
use zero_abi::{
    ApprovalMetadata, ApprovalState, EffectClass, EngineIdentity, RefOwnership, RevertMetadata,
    WorkerResult, WorkerResultMetadata,
};
use zero_store::{AttemptRecoveryOutcomeV1, AttemptStateV1};
use zsx_core::{
    AdapterBinding, AdapterCall, AdapterError, AdapterResponse, DomainAdapter,
    SessionApprovalGrantV1, SessionReplacementReason, ZsxSession, ZsxSessionFailureCode,
};

/// Minimal in-process adapter honoring the per-request cancellation token,
/// with hooks for a cooperative delay, an ambiguous failure after dispatch
/// crossing, and a call counter.
#[derive(Clone)]
struct TestAdapter {
    engine: EngineIdentity,
    session_id: String,
    calls: Arc<AtomicU64>,
}

impl TestAdapter {
    fn new(engine: EngineIdentity, session_id: &str) -> Self {
        Self {
            engine,
            session_id: session_id.to_owned(),
            calls: Arc::new(AtomicU64::new(0)),
        }
    }

    fn calls(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }

    fn binding(&self) -> AdapterBinding {
        AdapterBinding::new(
            self.engine,
            "test-revision",
            "test.v1",
            "a".repeat(64),
            "b".repeat(64),
            match self.engine {
                EngineIdentity::FsZero => "fz://",
                EngineIdentity::GraphZero => "gz://",
                EngineIdentity::TokenZero => "tz://",
            },
        )
        .expect("test binding is valid")
    }
}

impl DomainAdapter for TestAdapter {
    fn engine(&self) -> EngineIdentity {
        self.engine
    }

    fn binding(&self) -> AdapterBinding {
        self.binding()
    }

    fn call(&self, call: AdapterCall<'_>) -> Result<AdapterResponse, AdapterError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let request = call.request;
        if call.cancellation.is_cancelled() {
            return Err(AdapterError::new(
                "cancelled",
                "test adapter cancelled at entry",
                false,
                Some(request.trace.clone()),
            ));
        }
        if let Some(delay_ms) = request.args["__delay_ms"].as_u64() {
            let started = Instant::now();
            let budget = Duration::from_millis(delay_ms);
            let committed = request.args["__committed"].as_bool() == Some(true);
            loop {
                if !committed && call.cancellation.is_cancelled() {
                    return Err(AdapterError::new(
                        "cancelled",
                        "test adapter cancelled during delay",
                        false,
                        Some(request.trace.clone()),
                    ));
                }
                if let Some(deadline) = request.deadline_unix_ms
                    && now_ms() >= deadline
                {
                    return Err(AdapterError::new(
                        "deadline_exceeded",
                        "test adapter deadline exceeded",
                        false,
                        Some(request.trace.clone()),
                    ));
                }
                if started.elapsed() >= budget {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        if request.args["__fail"] == true {
            return Err(AdapterError::new(
                "internal",
                "test adapter failed after dispatch crossing",
                false,
                Some(request.trace.clone()),
            ));
        }
        Ok(AdapterResponse {
            result: WorkerResult {
                value: json!({ "args": request.args, "session_id": self.session_id }),
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
                    trace: request.trace.clone(),
                },
            },
            engine_timeline: None,
            worker_token_accounting: None,
        })
    }
}

fn build_session(
    root: &Path,
) -> (
    Arc<ZsxSession>,
    Arc<TestAdapter>,
    Arc<TestAdapter>,
    Arc<TestAdapter>,
) {
    let session_id = format!("zsx-cancel-{:x}", std::process::id());
    let fs = Arc::new(TestAdapter::new(EngineIdentity::FsZero, &session_id));
    let graph = Arc::new(TestAdapter::new(EngineIdentity::GraphZero, &session_id));
    let token = Arc::new(TestAdapter::new(EngineIdentity::TokenZero, &session_id));
    let session = ZsxSession::builder(root.to_path_buf())
        .with_session_id(session_id.clone())
        .fszero(fs.clone())
        .graphzero(graph.clone())
        .tokenzero(token.clone())
        .build()
        .expect("session builds");
    (Arc::new(session), fs, graph, token)
}

#[test]
fn cancelled_in_flight_request_stops_dispatching_and_later_request_succeeds() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());
    let (session, fs, _, _) = build_session(&root);

    let worker_session = Arc::clone(&session);
    let slow = std::thread::spawn(move || {
        worker_session.execute(
            1,
            1,
            r#"await zero.fs.compound('search', {query: 'x', __delay_ms: 900});"#,
            Duration::from_secs(30),
        )
    });
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        session.cancellation().cancel_request(1, 1),
        "in-flight request must be actively cancelled"
    );

    let error = slow
        .join()
        .expect("worker thread")
        .expect_err("cancelled request must fail");
    assert_eq!(error.code, ZsxSessionFailureCode::Cancelled);
    assert!(
        error.detail.contains("cancelled"),
        "adapter must have observed the per-request cancellation: {}",
        error.detail
    );
    assert_eq!(fs.calls(), 1, "only the pre-cancellation dispatch ran");

    // A later request in the same generation runs under a fresh token.
    let ok = session
        .execute(
            1,
            2,
            r#"await zero.fs.compound('list', {path: '.'});"#,
            Duration::from_secs(30),
        )
        .expect("later request in the same generation succeeds");
    assert_eq!(ok.generation, 1);
    assert_eq!(ok.request_id, 2);
    assert_eq!(fs.calls(), 2);

    session.shutdown().expect("shutdown");
}

#[test]
fn replace_rejects_queued_execute_under_old_generation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());
    let (session, fs, _, _) = build_session(&root);

    let blocker = Arc::clone(&session);
    let occupy = std::thread::spawn(move || {
        blocker.execute(
            1,
            1,
            r#"await zero.fs.compound('search', {query: 'x', __delay_ms: 800});"#,
            Duration::from_secs(30),
        )
    });
    std::thread::sleep(Duration::from_millis(80));
    let queued = Arc::clone(&session);
    let queued_join = std::thread::spawn(move || {
        queued.execute(
            1,
            2,
            r#"await zero.fs.compound('search', {query: 'queued'});"#,
            Duration::from_secs(30),
        )
    });
    std::thread::sleep(Duration::from_millis(40));
    session
        .replace(1, SessionReplacementReason::Manual)
        .expect("replace advances generation");
    let _ = occupy.join();
    let queued_result = queued_join.join().expect("queued thread");
    let queued_error = queued_result.expect_err("queued g1 work must not succeed");
    assert_eq!(queued_error.code, ZsxSessionFailureCode::StaleGeneration);
    // Occupier may have started; queued work must not add a second dispatch.
    assert!(
        fs.calls() <= 1,
        "queued execute after replace must not dispatch: calls={}",
        fs.calls()
    );
    assert_eq!(
        session
            .reconcile_request(1, 2)
            .expect_err("old-generation reconcile is stale")
            .code,
        ZsxSessionFailureCode::StaleGeneration
    );
    let admit = session
        .execute(
            1,
            3,
            r#"await zero.fs.compound('search', {query: 'after'});"#,
            Duration::from_secs(30),
        )
        .expect_err("new work on the retired generation is stale at admit");
    assert_eq!(admit.code, ZsxSessionFailureCode::StaleGeneration);
    session.shutdown().expect("shutdown");
}

#[test]
fn replace_reports_commit_race_when_inflight_execute_commits() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());
    let (session, fs, _, _) = build_session(&root);

    let occupier = Arc::clone(&session);
    let occupy = std::thread::spawn(move || {
        occupier.execute(
            1,
            1,
            r#"await zero.fs.compound('search', {query: 'x', __delay_ms: 400, __committed: true});"#,
            Duration::from_secs(30),
        )
    });
    std::thread::sleep(Duration::from_millis(80));
    session
        .replace(1, SessionReplacementReason::Manual)
        .expect("replace advances generation");
    let occupy_error = occupy
        .join()
        .expect("occupier thread")
        .expect_err("committed work after replace must not be Success");
    assert_eq!(occupy_error.code, ZsxSessionFailureCode::CommitRace);
    assert_eq!(occupy_error.generation, 1);
    assert!(
        occupy_error.detail.contains("generation 1"),
        "commit_race must name the committed generation: {}",
        occupy_error.detail
    );
    assert!(
        fs.calls() >= 1,
        "in-flight occupier must have dispatched: calls={}",
        fs.calls()
    );
    session.shutdown().expect("shutdown");
}

#[test]
fn replace_timeout_readmits_on_new_generation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());
    let (session, _, _, _) = build_session(&root);

    let occupier = Arc::clone(&session);
    let occupy = std::thread::spawn(move || {
        occupier.execute(
            1,
            1,
            r#"await zero.fs.compound('search', {query: 'x', __delay_ms: 6000, __committed: true});"#,
            Duration::from_secs(30),
        )
    });
    std::thread::sleep(Duration::from_millis(80));
    let replace_error = session
        .replace(1, SessionReplacementReason::Manual)
        .expect_err("in-flight occupier longer than settle timeout");
    assert_eq!(replace_error.code, ZsxSessionFailureCode::BackendUnavailable);
    assert!(
        replace_error.detail.contains("did not settle"),
        "{}",
        replace_error.detail
    );
    assert_eq!(session.generation().expect("generation"), 2);
    let ok = session
        .execute(
            2,
            2,
            r#"await zero.fs.compound('list', {path: '.'});"#,
            Duration::from_secs(30),
        )
        .expect("new generation must accept after replace timeout");
    assert_eq!(ok.generation, 2);
    assert_eq!(ok.request_id, 2);
    let _ = occupy.join();
    session.shutdown().expect("shutdown");
}

#[test]
fn cancelled_before_start_never_dispatches_and_later_request_succeeds() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());
    let (session, fs, _, _) = build_session(&root);

    let worker_session = Arc::clone(&session);
    let slow = std::thread::spawn(move || {
        worker_session.execute(
            1,
            1,
            r#"await zero.fs.compound('search', {query: 'x', __delay_ms: 900});"#,
            Duration::from_secs(30),
        )
    });
    std::thread::sleep(Duration::from_millis(200));

    // Request 2 is admitted while the worker is busy, so it is queued.
    let queued_session = Arc::clone(&session);
    let queued = std::thread::spawn(move || {
        queued_session.execute(
            1,
            2,
            r#"await zero.fs.compound('mutate', {path: 'b.txt', text: 'y'});"#,
            Duration::from_secs(30),
        )
    });
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        !session.cancellation().cancel_request(1, 2),
        "a queued request is not active, so cancel_request records it"
    );

    let error = queued
        .join()
        .expect("worker thread")
        .expect_err("cancelled-before-start request must fail");
    assert_eq!(error.code, ZsxSessionFailureCode::Cancelled);
    assert!(error.detail.contains("before start"), "{}", error.detail);

    slow.join()
        .expect("worker thread")
        .expect("the slow request was never cancelled and must succeed");
    assert_eq!(
        fs.calls(),
        1,
        "the cancelled-before-start request must never dispatch"
    );
    assert!(
        session
            .reconcile_request(1, 2)
            .expect("reconcile")
            .is_empty(),
        "a request that never dispatched journals nothing"
    );

    let ok = session
        .execute(
            1,
            3,
            r#"await zero.fs.compound('list', {path: '.'});"#,
            Duration::from_secs(30),
        )
        .expect("later request in the same generation succeeds");
    assert_eq!(ok.request_id, 3);
    assert_eq!(fs.calls(), 2);

    session.shutdown().expect("shutdown");
}

#[test]
fn successful_mutation_is_journaled_succeeded_and_reconcile_never_calls_an_adapter() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());
    let (session, fs, _, _) = build_session(&root);

    session
        .execute(
            1,
            1,
            r#"await zero.fs.compound('mutate', {path: 'a.txt', text: 'x'});"#,
            Duration::from_secs(30),
        )
        .expect("mutation executes");

    let calls_before = fs.calls();
    let statuses = session.reconcile_request(1, 1).expect("reconcile");
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].state, AttemptStateV1::Succeeded);
    assert_eq!(
        statuses[0].recovery.outcome,
        AttemptRecoveryOutcomeV1::AlreadySucceeded
    );
    assert_eq!(statuses[0].engine, Some(EngineIdentity::FsZero));
    assert_eq!(statuses[0].operation.as_deref(), Some("fs.edit"));
    assert_eq!(
        statuses[0].effect_class,
        Some(EffectClass::ReversibleMutation)
    );
    assert_eq!(
        fs.calls(),
        calls_before,
        "manual reconciliation must never call an adapter"
    );

    // Reconciliation is idempotent: the terminal journal is returned unchanged.
    let again = session.reconcile_request(1, 1).expect("reconcile again");
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].state, AttemptStateV1::Succeeded);

    // Read-only dispatches are never journaled.
    session
        .execute(
            1,
            2,
            r#"await zero.fs.compound('list', {path: '.'});"#,
            Duration::from_secs(30),
        )
        .expect("read executes");
    assert!(
        session
            .reconcile_request(1, 2)
            .expect("reconcile")
            .is_empty(),
        "read-only dispatches must not create attempt journals"
    );

    session.shutdown().expect("shutdown");
}

#[test]
fn reconcile_all_omits_replaced_generation_journals() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());
    let (session, fs, _, _) = build_session(&root);

    session
        .execute(
            1,
            1,
            r#"await zero.fs.compound('mutate', {path: 'a.txt', text: 'x'});"#,
            Duration::from_secs(30),
        )
        .expect("g1 mutation journals");
    let before = session
        .reconcile_all_attempts()
        .expect("reconcile before replace");
    assert!(
        before.iter().any(|status| {
            status.generation == 1 && status.state == AttemptStateV1::Succeeded
        }),
        "live generation must still reconcile"
    );

    session
        .replace(1, SessionReplacementReason::Manual)
        .expect("replace advances generation");
    let after = session
        .reconcile_all_attempts()
        .expect("reconcile after replace");
    assert!(
        after.iter().all(|status| status.generation != 1),
        "replaced generation must not appear in the live resume set: {after:?}"
    );
    assert!(
        after
            .iter()
            .all(|status| status.state != AttemptStateV1::SafeToRetry),
        "SafeToRetry must not appear for a replaced generation"
    );

    session
        .execute(
            2,
            1,
            r#"await zero.fs.compound('mutate', {path: 'b.txt', text: 'y'});"#,
            Duration::from_secs(30),
        )
        .expect("g2 mutation journals");
    let live = session
        .reconcile_all_attempts()
        .expect("reconcile live gen");
    assert!(
        live.iter().any(|status| {
            status.generation == 2 && status.state == AttemptStateV1::Succeeded
        }),
        "live generation must still reconcile after replace"
    );
    assert!(
        live.iter().all(|status| status.generation == 2),
        "only the live generation is a resume row"
    );
    assert_eq!(fs.calls(), 2);
    session.shutdown().expect("shutdown");
}

#[test]
fn ambiguous_mutation_failure_is_indeterminate_and_never_redispatched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());
    let (session, fs, _, _) = build_session(&root);

    let error = session
        .execute(
            1,
            1,
            r#"await zero.fs.compound('mutate', {path: 'a.txt', text: 'x', __fail: true});"#,
            Duration::from_secs(30),
        )
        .expect_err("ambiguous adapter failure must fail the request");
    assert_eq!(error.code, ZsxSessionFailureCode::BackendExecution);

    let calls_before = fs.calls();
    let statuses = session.reconcile_request(1, 1).expect("reconcile");
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].state, AttemptStateV1::Indeterminate);
    assert_eq!(
        statuses[0].recovery.outcome,
        AttemptRecoveryOutcomeV1::AlreadyIndeterminate
    );
    assert_eq!(
        fs.calls(),
        calls_before,
        "manual reconciliation must never call an adapter"
    );

    session.shutdown().expect("shutdown");
}

#[test]
fn whole_session_cancel_still_terminates_acceptance() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());
    let (session, _, _, _) = build_session(&root);

    session.cancellation().cancel();
    let generation = session.generation().expect("generation");
    let error = session
        .execute(generation, 1, "return null", Duration::from_secs(30))
        .expect_err("terminated session must reject execution");
    assert_eq!(error.code, ZsxSessionFailureCode::Terminating);

    session.shutdown().expect("shutdown");
}

fn e2e_log(phase: &str, event: &str, data: serde_json::Value) {
    eprintln!(
        "{}",
        json!({
            "ts": now_ms(),
            "suite": "sm9i",
            "phase": phase,
            "event": event,
            "data": data,
        })
    );
}

fn refuse_if_zerostack_checkout(workspace: &Path) {
    let canon = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let mut roots = vec![
        Path::new("/Users/aditya/AI/ZeroStack").to_path_buf(),
        Path::new("/home/aditya/AI/ZeroStack").to_path_buf(),
    ];
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        let manifest = Path::new(&manifest);
        roots.push(manifest.to_path_buf());
        if let Some(parent) = manifest.parent() {
            roots.push(parent.to_path_buf());
        }
    }
    for root in roots {
        let Ok(root) = root.canonicalize() else {
            continue;
        };
        if canon == root {
            panic!(
                "sm9i workspace must not be the ZeroStack checkout root: {}",
                canon.display()
            );
        }
    }
}

fn write_grant(root: &Path, request_id: u64) -> SessionApprovalGrantV1 {
    let now = now_ms();
    SessionApprovalGrantV1 {
        schema: "zerostack.session.approval_grant.v1".into(),
        grant_id: format!("grant-sm9i-{request_id}"),
        engine: EngineIdentity::FsZero,
        root: root.to_string_lossy().into_owned(),
        generation: 1,
        request_id,
        operation: "write".into(),
        effect: EffectClass::ApprovalRequiredMutation,
        authority_digest: "a".repeat(64),
        policy_digest: "b".repeat(64),
        issued_at_unix_ms: now.saturating_sub(1),
        expires_at_unix_ms: now.saturating_add(60_000),
    }
}

#[cfg(feature = "fszero")]
#[test]
fn real_fszero_write_queued_across_replace_is_honest() {
    let workspace = tempfile::tempdir().expect("workspace");
    let state = tempfile::tempdir().expect("state");
    let root = workspace
        .path()
        .canonicalize()
        .unwrap_or_else(|_| workspace.path().to_path_buf());
    refuse_if_zerostack_checkout(&root);
    refuse_if_zerostack_checkout(state.path());

    let unique = format!("queued-sm9i-{}-{}.txt", std::process::id(), now_ms());
    let written = root.join(&unique);
    e2e_log(
        "setup",
        "phase_start",
        json!({
            "workspace": root.display().to_string(),
            "state": state.path().display().to_string(),
            "path": unique,
        }),
    );

    let session_id = "sm9i-real-write";
    let fszero = Arc::new(zsx_core::fszero::FsZeroAdapter::new_with_state_root(
        &root,
        state.path(),
        session_id,
    ));
    assert!(
        !fszero.degraded(),
        "real durable FSZero must open; inert adapter is not this e2e"
    );
    let graph = Arc::new(TestAdapter::new(EngineIdentity::GraphZero, session_id));
    let token = Arc::new(TestAdapter::new(EngineIdentity::TokenZero, session_id));
    let session = Arc::new(
        ZsxSession::builder(&root)
            .with_state_root(state.path())
            .with_session_id(session_id)
            .fszero(fszero)
            .graphzero(graph)
            .tokenzero(token)
            .build()
            .expect("session with real FSZero builds"),
    );
    let generation_before = session.generation().expect("generation");
    e2e_log(
        "setup",
        "session_ready",
        json!({"generation": generation_before}),
    );

    let occupy_session = Arc::clone(&session);
    let occupy = std::thread::spawn(move || {
        occupy_session.execute(
            1,
            1,
            r#"await zero.fs.compound('search', {query: 'x', __delay_ms: 800});"#,
            Duration::from_secs(30),
        )
    });
    std::thread::sleep(Duration::from_millis(80));

    let queued_session = Arc::clone(&session);
    let queued_path = unique.clone();
    let grant = write_grant(&root, 2);
    let queued = std::thread::spawn(move || {
        let source = format!(
            r#"await zero.fs.compound("write", {{path:{path:?}, content:"sm9i-payload"}});"#,
            path = queued_path
        );
        queued_session.execute_with_approvals(1, 2, source, Duration::from_secs(30), vec![grant])
    });
    std::thread::sleep(Duration::from_millis(40));

    e2e_log(
        "act",
        "replace",
        json!({"generation_before": generation_before}),
    );
    session
        .replace(1, SessionReplacementReason::Manual)
        .expect("replace advances generation");
    let generation_after = session.generation().expect("generation after replace");
    e2e_log(
        "act",
        "replaced",
        json!({"generation_after": generation_after}),
    );
    assert!(
        generation_after > generation_before,
        "replace must bump generation"
    );

    let _ = occupy.join();
    let queued_result = queued.join().expect("queued write thread");
    let file_exists = written.exists();
    e2e_log(
        "assert",
        "queued_outcome",
        json!({
            "ok": queued_result.is_ok(),
            "file_exists": file_exists,
            "generation_before": generation_before,
            "generation_after": generation_after,
            "error": queued_result.as_ref().err().map(|error| json!({
                "code": format!("{:?}", error.code),
                "generation": error.generation,
                "detail": error.detail,
            })),
        }),
    );

    match queued_result {
        Ok(success) => {
            e2e_log(
                "assert",
                "test_end",
                json!({"result": "fail", "reason": "success"}),
            );
            panic!(
                "queued real fs.write after replace must not succeed: generation={} file_exists={file_exists}",
                success.generation
            );
        }
        Err(error) => {
            if file_exists && error.code == ZsxSessionFailureCode::StaleGeneration {
                e2e_log(
                    "assert",
                    "test_end",
                    json!({"result": "fail", "reason": "file_plus_stale"}),
                );
                panic!(
                    "silent file+StaleGeneration: wrote {} but reported {:?}: {}",
                    written.display(),
                    error.code,
                    error.detail
                );
            }
            assert!(
                matches!(
                    error.code,
                    ZsxSessionFailureCode::StaleGeneration
                        | ZsxSessionFailureCode::CommitRace
                        | ZsxSessionFailureCode::Cancelled
                ),
                "queued write must be an honest failure, got {:?}: {}",
                error.code,
                error.detail
            );
            if error.code == ZsxSessionFailureCode::StaleGeneration {
                assert!(
                    !file_exists,
                    "StaleGeneration must not leave a written file"
                );
            }
            e2e_log(
                "assert",
                "test_end",
                json!({
                    "result": "pass",
                    "code": format!("{:?}", error.code),
                    "file_exists": file_exists,
                }),
            );
        }
    }

    session.shutdown().expect("shutdown");
}
