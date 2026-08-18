//! Wave 10 call-scoped supervisor contract (zerostack-s0lx).
//!
//! Exercises both profiles (embedded reentrant and one-shot isolate) against
//! the same zerokernel protocol envelope:
//! - success, syntax error, JS exception, deadline, cancellation, and worker
//!   crash all leave executor count zero after the call;
//! - the one-shot process-tree audit finds no live child and every child is
//!   reaped on every terminal path;
//! - no socket or listener is created;
//! - both profiles return the same protocol envelope;
//! - the native path is untouched (this suite only exercises the supervisor;
//!   `zsx exec`/`zsx mcp` are unchanged and remain the fallback).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;
use zero_abi::zerokernel::{
    FiniteBudget, ReturnKind, ReturnPolicy, RootBindings, ZerokernelExecuteRequest,
    ZerokernelResultKind,
};
use zsx_core::supervisor::{
    OneShotChild, Supervisor, SupervisorError, SupervisorProfile,
};

const WALL_MS: u64 = 5_000;
const CPU_MS: u64 = 5_000;

/// The real one-shot child: this package's `zsx` binary in `kernel` mode.
fn kernel_child() -> OneShotChild {
    OneShotChild::new(env!("CARGO_BIN_EXE_zsx"), ["kernel"]).expect("child spec")
}

fn unique_root(label: &str) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    for _ in 0..100 {
        let candidate = std::env::temp_dir().join(format!(
            "zerostack-supervisor-{label}-{}-{}-{:x}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        match std::fs::create_dir(&candidate) {
            Ok(()) => return candidate,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("create temp root {}: {error}", candidate.display()),
        }
    }
    panic!("cannot allocate a unique temp root")
}

struct Fixture {
    root: PathBuf,
    state_root: PathBuf,
    session: String,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let root = unique_root(label);
        let state_root = root.join(".zerostack");
        std::fs::create_dir_all(&state_root).expect("create state root");
        Self {
            session: format!("supervisor-test-{}-{}", std::process::id(), label),
            root,
            state_root,
        }
    }

    fn request(&self, program: &str, wall_ms: u64) -> ZerokernelExecuteRequest {
        request_for(program, wall_ms, &self.root, &self.state_root, &self.session)
    }

    fn embedded(&self) -> Supervisor {
        Supervisor::builder(self.root.clone())
            .with_state_root(self.state_root.clone())
            .with_session_id(self.session.clone())
            .with_profile(SupervisorProfile::Embedded)
            .build_canonical()
            .expect("embedded supervisor builds")
    }

    fn oneshot(&self) -> Supervisor {
        Supervisor::builder(self.root.clone())
            .with_state_root(self.state_root.clone())
            .with_session_id(self.session.clone())
            .with_profile(SupervisorProfile::OneShot)
            .with_one_shot_child(kernel_child())
            .build()
            .expect("one-shot supervisor builds")
    }
}

fn request_for(
    program: &str,
    wall_ms: u64,
    root: &Path,
    state_root: &Path,
    session: &str,
) -> ZerokernelExecuteRequest {
    let root_text = root.to_string_lossy().into_owned();
    let state_text = state_root.to_string_lossy().into_owned();
    ZerokernelExecuteRequest::new(
        program.into(),
        Some(session.into()),
        FiniteBudget::new(wall_ms, CPU_MS, 64 * 1024 * 1024, 64).expect("budget"),
        ReturnPolicy::new(ReturnKind::Inline, 4096).expect("policy"),
        RootBindings::new(
            Some(root_text.clone()),
            root_text,
            None,
            None,
            Some(state_text),
        )
        .expect("roots"),
    )
    .expect("request")
}

/// Recursively assert no socket file exists under `root` (a supervisor must
/// never create a listener; the only IPC is stdio pipes).
fn assert_no_sockets(root: &Path) {
    fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                walk(&path, found);
            } else if file_type.is_socket() {
                found.push(path);
            }
        }
    }
    let mut sockets = Vec::new();
    walk(root, &mut sockets);
    assert!(
        sockets.is_empty(),
        "socket files found under {}: {sockets:?}",
        root.display()
    );
}

#[test]
fn success_completed_envelope_identical_across_profiles() {
    let fixture = Fixture::new("success");
    let embedded = fixture.embedded();
    let oneshot = fixture.oneshot();
    let request = fixture.request("return 42;", WALL_MS);

    let embedded_response = embedded.execute(request.clone()).expect("embedded executes");
    let oneshot_response = oneshot.execute(request).expect("one-shot executes");

    assert_eq!(embedded_response.kind, ZerokernelResultKind::Completed);
    assert_eq!(oneshot_response.kind, ZerokernelResultKind::Completed);
    assert_eq!(embedded_response.result, Some(json!(42)));
    assert_eq!(oneshot_response.result, Some(json!(42)));
    // Same protocol envelope: kind, result, preflight, and root evidence are
    // identical; the ledger differs only in measured wall (separate
    // processes), never in call or byte counts for this plan.
    assert_eq!(embedded_response.preflight, oneshot_response.preflight);
    assert_eq!(embedded_response.root_evidence, oneshot_response.root_evidence);
    assert_eq!(embedded_response.handles, oneshot_response.handles);
    assert_eq!(embedded_response.ledger.calls_made, 0);
    assert_eq!(oneshot_response.ledger.calls_made, 0);
    assert_eq!(embedded_response.ledger.bytes_out, oneshot_response.ledger.bytes_out);
    assert!(embedded_response.preflight.ok);
    assert!(embedded_response.root_evidence.unchanged);

    assert_eq!(embedded.live_executors(), 0);
    assert_eq!(oneshot.live_executors(), 0);
    assert_eq!(oneshot.live_children(), 0);
    assert_eq!(oneshot.child_spawn_count(), 1);
    assert_no_sockets(&fixture.root);
}

#[test]
fn syntax_error_failed_and_quiescent() {
    let fixture = Fixture::new("syntax");
    for supervisor in [fixture.embedded(), fixture.oneshot()] {
        let response = supervisor
            .execute(fixture.request("return ;;;", WALL_MS))
            .expect("syntax error is a protocol response");
        assert_eq!(response.kind, ZerokernelResultKind::Failed);
        assert!(!response.preflight.ok);
        assert!(
            response
                .preflight
                .errors
                .iter()
                .any(|error| error.contains("syntax")),
            "errors={:?}",
            response.preflight.errors
        );
        assert!(response.root_evidence.unchanged);
        assert!(response.result.is_none());
        assert_eq!(supervisor.live_executors(), 0);
        assert_eq!(supervisor.live_children(), 0);
    }
    assert_no_sockets(&fixture.root);
}

#[test]
fn js_exception_failed_and_quiescent() {
    let fixture = Fixture::new("exception");
    for supervisor in [fixture.embedded(), fixture.oneshot()] {
        let response = supervisor
            .execute(fixture.request(r#"throw new Error("boom");"#, WALL_MS))
            .expect("exception is a protocol response");
        assert_eq!(response.kind, ZerokernelResultKind::Failed);
        assert!(
            response
                .preflight
                .errors
                .iter()
                .any(|error| error.contains("boom")),
            "errors={:?}",
            response.preflight.errors
        );
        assert!(response.root_evidence.unchanged);
        assert_eq!(supervisor.live_executors(), 0);
        assert_eq!(supervisor.live_children(), 0);
    }
}

#[test]
fn deadline_failed_and_quiescent_both_profiles() {
    let fixture = Fixture::new("deadline");
    for supervisor in [fixture.embedded(), fixture.oneshot()] {
        let response = supervisor
            .execute(fixture.request("while (true) {}", 400))
            .expect("deadline is a protocol response");
        assert_eq!(response.kind, ZerokernelResultKind::Failed);
        assert!(
            response
                .preflight
                .errors
                .iter()
                .any(|error| error.contains("deadline")),
            "errors={:?}",
            response.preflight.errors
        );
        assert!(response.root_evidence.unchanged);
        assert_eq!(supervisor.live_executors(), 0);
        assert_eq!(supervisor.live_children(), 0);
    }
}

#[test]
fn hanging_child_killed_and_reaped_on_parent_deadline() {
    // A child that ignores the protocol (never reads stdin, never responds)
    // must be killed by the parent's deadline path: SIGTERM, bounded grace,
    // SIGKILL to the exact process group, then reap.
    let fixture = Fixture::new("hang");
    let child = OneShotChild::new("/bin/sleep", ["30"]).expect("sleep child");
    let supervisor = Supervisor::builder(fixture.root.clone())
        .with_state_root(fixture.state_root.clone())
        .with_session_id(fixture.session.clone())
        .with_profile(SupervisorProfile::OneShot)
        .with_one_shot_child(child)
        .build()
        .expect("one-shot supervisor builds");
    let started = std::time::Instant::now();
    let response = supervisor
        .execute(fixture.request("return 1;", 300))
        .expect("parent deadline is a protocol response");
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "parent deadline must settle promptly, took {:?}",
        started.elapsed()
    );
    assert_eq!(response.kind, ZerokernelResultKind::Failed);
    assert!(
        response
            .preflight
            .errors
            .iter()
            .any(|error| error.contains("deadline")),
        "errors={:?}",
        response.preflight.errors
    );
    assert_eq!(supervisor.live_executors(), 0);
    assert_eq!(supervisor.live_children(), 0);
    assert_eq!(supervisor.child_spawn_count(), 1);
}

#[test]
fn cancellation_pre_set_never_spawns_or_runs() {
    let fixture = Fixture::new("cancel-pre");
    let embedded = fixture.embedded();
    let oneshot = fixture.oneshot();
    let cancel = Arc::new(AtomicBool::new(true));

    let embedded_response = embedded
        .execute_cancellable(fixture.request("return 42;", WALL_MS), Arc::clone(&cancel))
        .expect("cancelled is a protocol response");
    assert_eq!(embedded_response.kind, ZerokernelResultKind::Failed);
    assert!(
        embedded_response
            .preflight
            .errors
            .iter()
            .any(|error| error.contains("cancelled")),
        "errors={:?}",
        embedded_response.preflight.errors
    );

    let oneshot_response = oneshot
        .execute_cancellable(fixture.request("return 42;", WALL_MS), cancel)
        .expect("cancelled is a protocol response");
    assert_eq!(oneshot_response.kind, ZerokernelResultKind::Failed);
    assert!(
        oneshot_response
            .preflight
            .errors
            .iter()
            .any(|error| error.contains("cancelled")),
        "errors={:?}",
        oneshot_response.preflight.errors
    );
    // The one-shot profile must not spawn a child for a pre-cancelled call.
    assert_eq!(oneshot.child_spawn_count(), 0);
    assert_eq!(embedded.live_executors(), 0);
    assert_eq!(oneshot.live_executors(), 0);
    assert_eq!(oneshot.live_children(), 0);
}

#[test]
fn mid_flight_cancellation_kills_and_reaps() {
    let fixture = Fixture::new("cancel-mid");
    for supervisor in [fixture.embedded(), fixture.oneshot()] {
        let cancel = Arc::new(AtomicBool::new(false));
        let canceller = Arc::clone(&cancel);
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            canceller.store(true, Ordering::Release);
        });
        let response = supervisor
            .execute_cancellable(fixture.request("while (true) {}", WALL_MS), cancel)
            .expect("cancelled is a protocol response");
        handle.join().expect("canceller joins");
        assert_eq!(response.kind, ZerokernelResultKind::Failed);
        assert!(
            response
                .preflight
                .errors
                .iter()
                .any(|error| error.contains("cancelled")),
            "errors={:?}",
            response.preflight.errors
        );
        assert!(response.root_evidence.unchanged);
        assert_eq!(supervisor.live_executors(), 0);
        assert_eq!(supervisor.live_children(), 0);
    }
}

#[test]
fn worker_crash_reaped_and_reported() {
    // The child SIGKILLs itself before ever writing a response: the parent
    // must reap the exact tree and report the crash in the protocol envelope.
    let fixture = Fixture::new("crash");
    let child = OneShotChild::new("/bin/sh", ["-c", "kill -9 $$"]).expect("crash child");
    let supervisor = Supervisor::builder(fixture.root.clone())
        .with_state_root(fixture.state_root.clone())
        .with_session_id(fixture.session.clone())
        .with_profile(SupervisorProfile::OneShot)
        .with_one_shot_child(child)
        .build()
        .expect("one-shot supervisor builds");
    let response = supervisor
        .execute(fixture.request("return 42;", WALL_MS))
        .expect("crash is a protocol response");
    assert_eq!(response.kind, ZerokernelResultKind::Failed);
    assert!(
        response
            .preflight
            .errors
            .iter()
            .any(|error| error.contains("without a response")),
        "errors={:?}",
        response.preflight.errors
    );
    assert!(response.root_evidence.unchanged);
    assert_eq!(supervisor.live_executors(), 0);
    assert_eq!(supervisor.live_children(), 0);
    assert_eq!(supervisor.child_spawn_count(), 1);
}

#[test]
fn decision_required_carries_typed_payload() {
    let fixture = Fixture::new("decision");
    let program = r#"return await zero.decision.require(
        {decision_id: "d1", observation_class: {class_id: "branch.choice"},
         question: "which branch?", alternatives: ["left", "right"], evidence_refs: []},
        "left");"#;
    for supervisor in [fixture.embedded(), fixture.oneshot()] {
        let response = supervisor
            .execute(fixture.request(program, WALL_MS))
            .expect("decision is a protocol response");
        assert_eq!(response.kind, ZerokernelResultKind::DecisionRequired);
        assert!(response.preflight.ok, "preflight={:?}", response.preflight);
        let decision = response.decision.expect("decision payload present");
        assert_eq!(decision.decision_id, "d1");
        assert_eq!(decision.question, "which branch?");
        assert!(response.result.is_none());
        assert!(response.root_evidence.unchanged);
        assert_eq!(supervisor.live_executors(), 0);
        assert_eq!(supervisor.live_children(), 0);
    }
}

#[test]
fn ledger_reports_calls_made() {
    let fixture = Fixture::new("ledger");
    for supervisor in [fixture.embedded(), fixture.oneshot()] {
        let response = supervisor
            .execute(fixture.request(r#"return await zero.help.search({query: "fs"});"#, WALL_MS))
            .expect("help.search executes");
        assert_eq!(response.kind, ZerokernelResultKind::Completed);
        assert_eq!(response.ledger.calls_made, 1, "ledger={:?}", response.ledger);
        assert!(response.ledger.bytes_out > 0);
        assert_eq!(supervisor.live_executors(), 0);
        assert_eq!(supervisor.live_children(), 0);
    }
}

#[test]
fn invalid_requests_fail_closed_without_spawn() {
    let fixture = Fixture::new("invalid");
    let oneshot = fixture.oneshot();

    // Wrong ABI version is refused before any execution.
    let mut request = request_for(
        "return 42;",
        WALL_MS,
        &fixture.root,
        &fixture.state_root,
        &fixture.session,
    );
    request.abi_version = "not-zerokernel".into();
    let error = oneshot.execute(request).expect_err("wrong abi version refuses");
    assert!(matches!(error, SupervisorError::InvalidRequest(_)));

    // Zero budget is refused before any execution.
    let mut request = request_for(
        "return 42;",
        WALL_MS,
        &fixture.root,
        &fixture.state_root,
        &fixture.session,
    );
    request.budget = FiniteBudget::new(0, CPU_MS, 64 * 1024 * 1024, 64).expect("budget");
    let error = oneshot.execute(request).expect_err("zero budget refuses");
    assert!(matches!(error, SupervisorError::InvalidRequest(_)));

    // Unbounded budget (over the protocol maximum) is refused.
    let mut request = request_for(
        "return 42;",
        WALL_MS,
        &fixture.root,
        &fixture.state_root,
        &fixture.session,
    );
    request.budget = FiniteBudget::new(10_000_000, CPU_MS, 64 * 1024 * 1024, 64).expect("budget");
    let error = oneshot.execute(request).expect_err("unbounded budget refuses");
    assert!(matches!(error, SupervisorError::InvalidRequest(_)));

    // No child was ever spawned for refused requests.
    assert_eq!(oneshot.child_spawn_count(), 0);
    assert_eq!(oneshot.live_executors(), 0);
    assert_eq!(oneshot.live_children(), 0);
}

#[test]
fn session_and_root_binding_fail_closed() {
    let fixture = Fixture::new("binding");
    let embedded = fixture.embedded();

    let mut request = fixture.request("return 42;", WALL_MS);
    request.session = Some("some-other-session".into());
    let error = embedded
        .execute(request)
        .expect_err("foreign session refuses");
    assert!(matches!(
        error,
        SupervisorError::SessionMismatch { .. }
    ));

    let mut request = fixture.request("return 42;", WALL_MS);
    request.roots = RootBindings::new(
        Some(fixture.root.to_string_lossy().into_owned()),
        std::env::temp_dir().to_string_lossy().into_owned(),
        None,
        None,
        Some(fixture.state_root.to_string_lossy().into_owned()),
    )
    .expect("roots");
    let error = embedded.execute(request).expect_err("foreign root refuses");
    assert!(matches!(error, SupervisorError::RootMismatch(_)));
    assert_eq!(embedded.live_executors(), 0);
}

#[test]
fn preflight_reports_missing_request_root() {
    let fixture = Fixture::new("preflight");
    let missing = unique_root("missing-request-root");
    // unique_root creates the directory; remove it so the preflight sees a
    // genuinely missing root.
    std::fs::remove_dir(&missing).expect("remove marker dir");
    let request = ZerokernelExecuteRequest::new(
        "return 42;".into(),
        Some(fixture.session.clone()),
        FiniteBudget::new(WALL_MS, CPU_MS, 64 * 1024 * 1024, 64).expect("budget"),
        ReturnPolicy::new(ReturnKind::Inline, 4096).expect("policy"),
        RootBindings::new(
            Some(fixture.root.to_string_lossy().into_owned()),
            fixture.root.to_string_lossy().into_owned(),
            Some(missing.to_string_lossy().into_owned()),
            None,
            Some(fixture.state_root.to_string_lossy().into_owned()),
        )
        .expect("roots"),
    )
    .expect("request");
    for supervisor in [fixture.embedded(), fixture.oneshot()] {
        let response = supervisor.execute(request.clone()).expect("preflight response");
        assert_eq!(response.kind, ZerokernelResultKind::Failed);
        assert!(!response.preflight.ok);
        assert!(
            response
                .preflight
                .errors
                .iter()
                .any(|error| error.contains("request_root")),
            "errors={:?}",
            response.preflight.errors
        );
        assert!(response.root_evidence.unchanged);
        assert_eq!(supervisor.live_executors(), 0);
        assert_eq!(supervisor.live_children(), 0);
    }
}

#[test]
fn multiple_calls_keep_session_but_never_executors() {
    // The supervisor (session shell) survives across calls; every call gets
    // a fresh runtime and leaves zero live executors behind.
    let fixture = Fixture::new("multi");
    let embedded = fixture.embedded();
    for index in 0..5 {
        let program = format!("return {index};");
        let response = embedded
            .execute(fixture.request(&program, WALL_MS))
            .expect("call executes");
        assert_eq!(response.kind, ZerokernelResultKind::Completed);
        assert_eq!(response.result, Some(json!(index)));
        assert_eq!(embedded.live_executors(), 0);
        assert_eq!(embedded.live_children(), 0);
    }
    let oneshot = fixture.oneshot();
    for index in 0..3 {
        let program = format!("return {index};");
        let response = oneshot
            .execute(fixture.request(&program, WALL_MS))
            .expect("call executes");
        assert_eq!(response.kind, ZerokernelResultKind::Completed);
        assert_eq!(oneshot.live_executors(), 0);
        assert_eq!(oneshot.live_children(), 0);
    }
    assert_eq!(oneshot.child_spawn_count(), 3);
    assert_no_sockets(&fixture.root);
}

#[test]
fn failed_embedded_call_leaves_no_runtime_behind() {
    // A failed embedded call (exception) must also prove quiescence: the
    // per-call connector dispatcher threads are joined and the runtime is
    // dropped before the response is returned.
    let fixture = Fixture::new("embedded-fail");
    let embedded = fixture.embedded();
    let response = embedded
        .execute(fixture.request("throw new Error('x');", WALL_MS))
        .expect("exception is a protocol response");
    assert_eq!(response.kind, ZerokernelResultKind::Failed);
    assert_eq!(embedded.live_executors(), 0);
    // And the supervisor stays fully usable afterwards.
    let response = embedded
        .execute(fixture.request("return 7;", WALL_MS))
        .expect("subsequent call executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed);
    assert_eq!(response.result, Some(json!(7)));
    assert_eq!(embedded.live_executors(), 0);
}

#[test]
fn rejected_arguments_reject_without_execution() {
    // fs.write is approval-required: with no grants installed the read-only
    // supervisor must fail closed at the connector boundary before any
    // adapter call, exactly like native without grants, and leave zero live
    // executors and no written file.
    let fixture = Fixture::new("write-refused");
    let program = r#"return await zero.fs.write({path: "nope.txt", content: "x"});"#;
    for supervisor in [fixture.embedded(), fixture.oneshot()] {
        let response = supervisor
            .execute(fixture.request(program, WALL_MS))
            .expect("write refusal is a protocol response");
        assert_eq!(response.kind, ZerokernelResultKind::Failed);
        assert!(
            response
                .preflight
                .errors
                .iter()
                .any(|error| error.contains("approval") || error.contains("grant")),
            "errors={:?}",
            response.preflight.errors
        );
        assert!(response.root_evidence.unchanged);
        assert_eq!(supervisor.live_executors(), 0);
        assert_eq!(supervisor.live_children(), 0);
    }
    assert!(
        !fixture.root.join("nope.txt").exists(),
        "read-only protocol must not write"
    );
}
