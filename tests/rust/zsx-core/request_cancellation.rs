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
    SessionReplacementReason, ZsxSession, ZsxSessionFailureCode,
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
            loop {
                if call.cancellation.is_cancelled() {
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
                        "deadline",
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
                "boom",
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
