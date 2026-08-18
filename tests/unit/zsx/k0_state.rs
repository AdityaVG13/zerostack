//! K0 session-state persistence across fresh executors with CAS rollback
//! (zerostack-7inx).
//!
//! Exercises the end-to-end session-state path through the real supervisor
//! (embedded and one-shot profiles):
//! - state survives multiple fresh executor instances (brand-new
//!   `Supervisor` instances over the same state root and session, hydrated
//!   from the committed root);
//! - two concurrent successors against one expected root yield exactly one
//!   commit and one typed conflict;
//! - syntax error, JS exception, deadline, cancellation, stale root,
//!   output-limit failure, and worker crash all preserve the prior session
//!   and project roots (no write, unchanged evidence);
//! - a no-delta success leaves the committed root untouched;
//! - state deletions propagate through successors;
//! - invalid expected-root shapes fail closed (non-identity refused before
//!   execution or spawn; unknown or non-state objects fail typed preflight).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use zero_abi::zerokernel::{
    FiniteBudget, ReturnKind, ReturnPolicy, RootBindings, ZerokernelExecuteRequest,
    ZerokernelResultKind, ZerokernelExecuteResponse,
};
use zsx_core::supervisor::{OneShotChild, Supervisor, SupervisorError, SupervisorProfile};

const WALL_MS: u64 = 5_000;
const CPU_MS: u64 = 5_000;
const MEMORY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CALLS: u32 = 64;

/// The real one-shot child: this package's `zsx` binary in `kernel` mode.
fn kernel_child() -> OneShotChild {
    OneShotChild::new(env!("CARGO_BIN_EXE_zsx"), ["kernel"]).expect("child spec")
}

fn unique_root(label: &str) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    for _ in 0..100 {
        let candidate = std::env::temp_dir().join(format!(
            "zerostack-k0-state-{label}-{}-{}-{:x}",
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
            session: format!("k0-state-{}-{label}", std::process::id()),
            root,
            state_root,
        }
    }

    fn request(&self, program: &str) -> ZerokernelExecuteRequest {
        self.request_budgeted(program, FiniteBudget::new(WALL_MS, CPU_MS, MEMORY_BYTES, MAX_CALLS).expect("budget"), None)
    }

    /// `expected` is the committed-state CAS identity the call expects
    /// (`None` hydrates from the committed root and commits
    /// unconditionally).
    fn request_expected(
        &self,
        program: &str,
        expected: Option<String>,
    ) -> ZerokernelExecuteRequest {
        self.request_budgeted(
            program,
            FiniteBudget::new(WALL_MS, CPU_MS, MEMORY_BYTES, MAX_CALLS).expect("budget"),
            expected,
        )
    }

    fn request_budgeted(
        &self,
        program: &str,
        budget: FiniteBudget,
        expected: Option<String>,
    ) -> ZerokernelExecuteRequest {
        let root_text = self.root.to_string_lossy().into_owned();
        ZerokernelExecuteRequest::new(
            program.into(),
            Some(self.session.clone()),
            budget,
            ReturnPolicy::new(ReturnKind::Inline, 4096).expect("policy"),
            RootBindings::new(Some(root_text.clone()), root_text, None, None, expected)
                .expect("roots"),
        )
        .expect("request")
    }

    fn embedded(&self) -> Supervisor {
        Supervisor::builder(self.root.clone())
            .with_state_root(self.state_root.clone())
            .with_session_id(self.session.clone())
            .with_profile(SupervisorProfile::Embedded)
            .build_canonical()
            .expect("embedded supervisor builds")
    }

    fn embedded_no_state_root(&self) -> Supervisor {
        // state_root == root: no result-spill root, so an oversized result
        // fails typed instead of spilling.
        Supervisor::builder(self.root.clone())
            .with_session_id(self.session.clone())
            .with_profile(SupervisorProfile::Embedded)
            .build_canonical()
            .expect("embedded supervisor without state root builds")
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

    /// The committed session root under the fixture state root, if any.
    fn committed_root(&self) -> Option<String> {
        zsx_core::k0_state::current_session_root(&self.state_root, &self.session)
            .expect("committed root pointer readable")
    }
}

fn failed_errors(response: &ZerokernelExecuteResponse) -> Vec<String> {
    response.preflight.errors.clone()
}

fn has_error(response: &ZerokernelExecuteResponse, needle: &str) -> bool {
    failed_errors(response)
        .iter()
        .any(|error| error.contains(needle))
}

fn assert_quiescent(supervisor: &Supervisor) {
    assert_eq!(supervisor.live_executors(), 0);
    assert_eq!(supervisor.live_children(), 0);
}

// ---------------------------------------------------------------------------
// Survival across fresh executor instances
// ---------------------------------------------------------------------------

#[test]
fn state_survives_multiple_fresh_executor_instances() {
    let fixture = Fixture::new("survive");
    // Fresh session: no committed root, no expected root, empty state.
    let first = fixture.embedded();
    let response = first
        .execute(fixture.request(
            r#"
            z.state.set("counter", 1);
            return z.state.list();
        "#,
        ))
        .expect("first call executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", failed_errors(&response));
    assert_eq!(response.result, Some(json!(["counter"])));
    assert!(!response.root_evidence.unchanged, "a state write must commit");
    let root1 = response
        .root_evidence
        .successor_root
        .clone()
        .expect("first successor");
    assert_eq!(
        response.root_evidence.after.session_root.as_deref(),
        Some(root1.as_str())
    );
    assert_eq!(response.root_evidence.before.session_root, None);
    assert_eq!(fixture.committed_root().as_deref(), Some(root1.as_str()));
    assert_quiescent(&first);

    // A brand-new executor instance (fresh Supervisor over the same state
    // root and session) hydrates the committed state through the expected
    // root; a read-only call commits nothing.
    let second = fixture.embedded();
    let response = second
        .execute(fixture.request_expected("return z.state.get('counter');", Some(root1.clone())))
        .expect("hydrated read executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", failed_errors(&response));
    assert_eq!(response.result, Some(json!(1)));
    assert!(response.root_evidence.unchanged, "read-only call must not commit");
    assert!(response.root_evidence.successor_root.is_none());
    assert_eq!(fixture.committed_root().as_deref(), Some(root1.as_str()));
    assert_quiescent(&second);

    // A third fresh instance extends the state through a successor; the
    // untouched key survives the delta.
    let third = fixture.embedded();
    let response = third
        .execute(fixture.request_expected(
            r#"
            z.state.set("counter", 2);
            z.state.set("note", "kept");
            return z.state.get("counter");
        "#,
            Some(root1.clone()),
        ))
        .expect("successor call executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", failed_errors(&response));
    assert_eq!(response.result, Some(json!(2)));
    let root2 = response
        .root_evidence
        .successor_root
        .clone()
        .expect("second successor");
    assert_ne!(root2, root1);
    assert_eq!(fixture.committed_root().as_deref(), Some(root2.as_str()));
    assert_quiescent(&third);

    // A fourth fresh instance sees the full successor state and the
    // hydrated session root in `z.context`.
    let fourth = fixture.embedded();
    let response = fourth
        .execute(fixture.request_expected(
            r#"
            const ctx = z.context;
            return {
                counter: z.state.get("counter"),
                note: z.state.get("note"),
                sessionRoot: ctx.sessionRoot,
            };
        "#,
            Some(root2.clone()),
        ))
        .expect("final hydrated read executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", failed_errors(&response));
    assert_eq!(
        response.result,
        Some(json!({"counter": 2, "note": "kept", "sessionRoot": root2}))
    );
    assert_quiescent(&fourth);
}

#[test]
fn oneshot_profile_commits_and_hydrates_across_calls() {
    // The one-shot child runs the same embedded profile inside the sandbox:
    // the successor commit happens in the child against the same store root
    // and session identity, and the parent relays the response.
    let fixture = Fixture::new("oneshot-state");
    let oneshot = fixture.oneshot();
    let response = oneshot
        .execute(fixture.request(
            r#"
            z.state.set("k", {v: 7});
            return z.state.get("k").v;
        "#,
        ))
        .expect("one-shot state write executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", failed_errors(&response));
    assert_eq!(response.result, Some(json!(7)));
    let root = response
        .root_evidence
        .successor_root
        .clone()
        .expect("child-committed successor");
    assert_eq!(fixture.committed_root().as_deref(), Some(root.as_str()));

    let response = oneshot
        .execute(fixture.request_expected("return z.state.get('k').v;", Some(root)))
        .expect("one-shot hydrated read executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", failed_errors(&response));
    assert_eq!(response.result, Some(json!(7)));
    assert!(response.root_evidence.unchanged);
    assert_eq!(oneshot.child_spawn_count(), 2);
    assert_quiescent(&oneshot);
}

// ---------------------------------------------------------------------------
// Concurrent successors: one commit, one typed conflict
// ---------------------------------------------------------------------------

#[test]
fn concurrent_successors_yield_one_commit_and_one_typed_conflict() {
    let fixture = Fixture::new("concurrent");
    let supervisor = Arc::new(fixture.embedded());
    // Seed a committed root both calls will expect.
    let response = supervisor
        .execute(fixture.request("z.state.set('seed', 0); return z.state.list();"))
        .expect("seed call executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", failed_errors(&response));
    let base = response
        .root_evidence
        .successor_root
        .clone()
        .expect("seed successor");

    // Two plans over the same expected root, each staging a different key.
    let request_a = fixture.request_expected(
        "z.state.set('winner', 1); return z.state.get('winner');",
        Some(base.clone()),
    );
    let request_b = fixture.request_expected(
        "z.state.set('loser', 2); return z.state.get('loser');",
        Some(base),
    );

    let start = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel();
    let mut handles = Vec::new();
    for request in [request_a, request_b] {
        let supervisor = Arc::clone(&supervisor);
        let start = Arc::clone(&start);
        let tx = tx.clone();
        handles.push(std::thread::spawn(move || {
            while !start.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            let response = supervisor.execute(request).expect("race call returns a response");
            // Carry the canonical envelope across the thread boundary.
            tx.send(response.canonical_json()).expect("race result channel");
        }));
    }
    start.store(true, Ordering::Release);
    for handle in handles {
        handle.join().expect("race thread joins");
    }
    drop(tx);
    let responses: Vec<ZerokernelExecuteResponse> = rx
        .iter()
        .map(|json| {
            ZerokernelExecuteResponse::from_canonical_bytes(json.as_bytes())
                .expect("race response parses")
        })
        .collect();
    assert_eq!(responses.len(), 2);

    let completed: Vec<&ZerokernelExecuteResponse> = responses
        .iter()
        .filter(|response| response.kind == ZerokernelResultKind::Completed)
        .collect();
    let failed: Vec<&ZerokernelExecuteResponse> = responses
        .iter()
        .filter(|response| response.kind == ZerokernelResultKind::Failed)
        .collect();
    assert_eq!(completed.len(), 1, "exactly one successor commits");
    assert_eq!(failed.len(), 1, "exactly one typed conflict");

    let winner = completed[0];
    assert!(!winner.root_evidence.unchanged);
    let successor = winner
        .root_evidence
        .successor_root
        .clone()
        .expect("winner successor");
    assert_eq!(fixture.committed_root().as_deref(), Some(successor.as_str()));

    let loser = failed[0];
    assert!(has_error(loser, "session root conflict"), "errors={:?}", failed_errors(loser));
    assert!(loser.root_evidence.unchanged);
    assert_eq!(loser.root_evidence.before, loser.root_evidence.after);
    assert_eq!(
        loser.root_evidence.after.session_root.as_deref(),
        Some(successor.as_str()),
        "the conflict evidence shows the committed root"
    );
    assert!(loser.root_evidence.successor_root.is_none());

    // The committed state is exactly one successor: the winner's key and
    // the seed — never both racing deltas.
    let response = supervisor
        .execute(fixture.request_expected("return z.state.list();", Some(successor)))
        .expect("post-race hydrated read executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", failed_errors(&response));
    let keys = response.result.expect("post-race keys");
    assert!(
        keys == json!(["loser", "seed"]) || keys == json!(["seed", "winner"]),
        "committed state must be exactly one successor, got {keys}"
    );
    assert_quiescent(&supervisor);
}

// ---------------------------------------------------------------------------
// Stale root and conflict preservation
// ---------------------------------------------------------------------------

#[test]
fn stale_expected_root_conflicts_and_preserves_committed_state() {
    let fixture = Fixture::new("stale");
    let supervisor = fixture.embedded();
    let root1 = supervisor
        .execute(fixture.request("z.state.set('k', 1); return 1;"))
        .expect("first commit")
        .root_evidence
        .successor_root
        .clone()
        .expect("root1");
    let root2 = supervisor
        .execute(fixture.request_expected("z.state.set('k', 2); return 2;", Some(root1.clone())))
        .expect("second commit")
        .root_evidence
        .successor_root
        .clone()
        .expect("root2");
    assert_ne!(root1, root2);
    assert_eq!(fixture.committed_root().as_deref(), Some(root2.as_str()));

    // A call still expecting the superseded root conflicts typed and writes
    // nothing, even though its plan would produce a third state.
    let response = supervisor
        .execute(fixture.request_expected(
            "z.state.set('k', 3); return 3;",
            Some(root1),
        ))
        .expect("stale-root call returns a response");
    assert_eq!(response.kind, ZerokernelResultKind::Failed, "errors={:?}", failed_errors(&response));
    assert!(
        has_error(&response, "session root conflict"),
        "errors={:?}",
        failed_errors(&response)
    );
    assert!(response.root_evidence.unchanged);
    assert_eq!(response.root_evidence.before, response.root_evidence.after);
    assert_eq!(
        response.root_evidence.after.session_root.as_deref(),
        Some(root2.as_str()),
        "the conflict evidence shows the committed root"
    );
    assert!(response.root_evidence.successor_root.is_none());
    assert_eq!(fixture.committed_root().as_deref(), Some(root2.as_str()));
    assert_quiescent(&supervisor);

    // The committed state is the winner's; the stale writer's delta is gone.
    let response = supervisor
        .execute(fixture.request_expected("return z.state.get('k');", Some(root2)))
        .expect("post-conflict hydrated read executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", failed_errors(&response));
    assert_eq!(response.result, Some(json!(2)));
    assert_quiescent(&supervisor);
}

// ---------------------------------------------------------------------------
// Every failure terminal preserves the prior roots
// ---------------------------------------------------------------------------

#[test]
fn failure_terminals_preserve_session_and_project_roots() {
    let fixture = Fixture::new("fail-preserve");
    let embedded = fixture.embedded();
    let base = embedded
        .execute(fixture.request("z.state.set('k', 1); return z.state.list();"))
        .expect("seed executes")
        .root_evidence
        .successor_root
        .clone()
        .expect("seed successor");

    let expect_preserved = |response: &ZerokernelExecuteResponse| {
        assert_eq!(response.kind, ZerokernelResultKind::Failed, "errors={:?}", failed_errors(response));
        assert!(response.root_evidence.unchanged);
        assert_eq!(response.root_evidence.before, response.root_evidence.after);
        assert_eq!(
            response.root_evidence.after.session_root.as_deref(),
            Some(base.as_str())
        );
        assert!(response.root_evidence.successor_root.is_none());
        assert_eq!(fixture.committed_root().as_deref(), Some(base.as_str()));
    };

    // Syntax error.
    expect_preserved(
        &embedded
            .execute(fixture.request_expected("return (", Some(base.clone())))
            .expect("syntax terminal"),
    );
    // JS exception.
    expect_preserved(
        &embedded
            .execute(fixture.request_expected(r#"throw new Error("boom");"#, Some(base.clone())))
            .expect("exception terminal"),
    );
    // Wall deadline after staging a delta: the staged state is discarded.
    let tight = FiniteBudget::new(400, CPU_MS, MEMORY_BYTES, MAX_CALLS).expect("tight budget");
    expect_preserved(
        &embedded
            .execute(fixture.request_budgeted(
                "z.state.set('staged', 1); while (true) {}",
                tight,
                Some(base.clone()),
            ))
            .expect("deadline terminal"),
    );
    // Pre-set cancellation.
    expect_preserved(
        &embedded
            .execute_cancellable(
                fixture.request_expected("return 1;", Some(base.clone())),
                Arc::new(AtomicBool::new(true)),
            )
            .expect("cancel terminal"),
    );
    // Stale root: a well-formed identity that was never committed fails
    // typed in preflight, before any execution.
    let ghost = zero_abi::sha256_hex(b"never-published-session-root");
    let response = embedded
        .execute(fixture.request_expected("return 1;", Some(ghost)))
        .expect("stale-root terminal");
    assert_eq!(response.kind, ZerokernelResultKind::Failed, "errors={:?}", failed_errors(&response));
    assert!(
        has_error(&response, "not present in the session store"),
        "errors={:?}",
        failed_errors(&response)
    );
    assert!(response.root_evidence.unchanged);
    assert_eq!(fixture.committed_root().as_deref(), Some(base.as_str()));

    // Output-limit failure (no spill root): the oversized result fails
    // typed and the state staged before the failure is discarded.
    let no_spill = fixture.embedded_no_state_root();
    let no_spill_base = no_spill
        .execute(fixture.request("z.state.set('k', 1); return 1;"))
        .expect("no-spill seed executes")
        .root_evidence
        .successor_root
        .clone()
        .expect("no-spill successor");
    let no_spill_pointer = zsx_core::k0_state::session_root_pointer(&fixture.root, &fixture.session);
    let response = no_spill
        .execute(fixture.request_expected(
            "z.state.set('staged', 1); return 'x'.repeat(300000);",
            Some(no_spill_base.clone()),
        ))
        .expect("output-limit terminal");
    assert_eq!(response.kind, ZerokernelResultKind::Failed, "errors={:?}", failed_errors(&response));
    assert!(
        has_error(&response, "result is") && has_error(&response, "maximum is"),
        "errors={:?}",
        failed_errors(&response)
    );
    assert!(response.root_evidence.unchanged);
    assert_eq!(
        std::fs::read_to_string(&no_spill_pointer)
            .expect("no-spill pointer")
            .trim(),
        no_spill_base,
        "output-limit failure must not move the committed root"
    );
    let response = no_spill
        .execute(fixture.request_expected(
            "return z.state.get('k');",
            Some(no_spill_base),
        ))
        .expect("no-spill hydrated read executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", failed_errors(&response));
    assert_eq!(response.result, Some(json!(1)), "state survives the output-limit failure");
    assert_quiescent(&no_spill);

    // Worker crash: the one-shot child dies without a response; the parent
    // reports the crash and writes nothing.
    let crashing = Supervisor::builder(fixture.root.clone())
        .with_state_root(fixture.state_root.clone())
        .with_session_id(fixture.session.clone())
        .with_profile(SupervisorProfile::OneShot)
        .with_one_shot_child(OneShotChild::new("/bin/sh", ["-c", "kill -9 $$"]).expect("crash child"))
        .build()
        .expect("crashing supervisor builds");
    let response = crashing
        .execute(fixture.request_expected("return 1;", Some(base.clone())))
        .expect("crash terminal");
    assert_eq!(response.kind, ZerokernelResultKind::Failed, "errors={:?}", failed_errors(&response));
    assert!(
        has_error(&response, "without a response"),
        "errors={:?}",
        failed_errors(&response)
    );
    assert!(response.root_evidence.unchanged);
    assert_eq!(fixture.committed_root().as_deref(), Some(base.as_str()));
    assert_quiescent(&crashing);

    // After every failure terminal the committed state is intact.
    let response = embedded
        .execute(fixture.request_expected("return z.state.get('k');", Some(base)))
        .expect("final hydrated read executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", failed_errors(&response));
    assert_eq!(response.result, Some(json!(1)));
    assert_quiescent(&embedded);
}

// ---------------------------------------------------------------------------
// No-delta successes and deletions
// ---------------------------------------------------------------------------

#[test]
fn no_delta_success_leaves_committed_root_unchanged() {
    let fixture = Fixture::new("no-delta");
    let supervisor = fixture.embedded();
    let root1 = supervisor
        .execute(fixture.request("z.state.set('k', 1); return 1;"))
        .expect("commit executes")
        .root_evidence
        .successor_root
        .clone()
        .expect("root1");

    // A successful read-only call commits no successor: completed with
    // unchanged evidence and no successor root.
    let response = supervisor
        .execute(fixture.request_expected("return z.state.get('k');", Some(root1.clone())))
        .expect("read-only call executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", failed_errors(&response));
    assert_eq!(response.result, Some(json!(1)));
    assert!(response.root_evidence.unchanged);
    assert!(response.root_evidence.successor_root.is_none());
    assert_eq!(response.root_evidence.before, response.root_evidence.after);
    assert_eq!(
        response.root_evidence.after.session_root.as_deref(),
        Some(root1.as_str())
    );
    assert_eq!(fixture.committed_root().as_deref(), Some(root1.as_str()));

    // Writing the same value back stages no delta either (map equality).
    let response = supervisor
        .execute(fixture.request_expected(
            "z.state.set('k', 1); return 1;",
            Some(root1.clone()),
        ))
        .expect("idempotent call executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", failed_errors(&response));
    assert!(response.root_evidence.unchanged, "an idempotent state write is not a delta");
    assert_eq!(fixture.committed_root().as_deref(), Some(root1.as_str()));
    assert_quiescent(&supervisor);
}

#[test]
fn state_deletions_propagate_to_successor() {
    let fixture = Fixture::new("deletions");
    let supervisor = fixture.embedded();
    let root1 = supervisor
        .execute(fixture.request(
            "z.state.set('a', 1); z.state.set('b', 2); return z.state.list();",
        ))
        .expect("first commit executes")
        .root_evidence
        .successor_root
        .clone()
        .expect("root1");

    let root2 = supervisor
        .execute(fixture.request_expected(
            "z.state.delete('a'); return z.state.list();",
            Some(root1.clone()),
        ))
        .expect("delete commit executes")
        .root_evidence
        .successor_root
        .clone()
        .expect("root2");
    assert_ne!(root1, root2);
    assert_eq!(fixture.committed_root().as_deref(), Some(root2.as_str()));

    let response = supervisor
        .execute(fixture.request_expected("return z.state.list();", Some(root2)))
        .expect("hydrated list executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", failed_errors(&response));
    assert_eq!(response.result, Some(json!(["b"])));
    assert_quiescent(&supervisor);
}

// ---------------------------------------------------------------------------
// Invalid expected-root shapes fail closed
// ---------------------------------------------------------------------------

#[test]
fn invalid_expected_roots_fail_closed() {
    let fixture = Fixture::new("invalid-roots");
    let embedded = fixture.embedded();
    let oneshot = fixture.oneshot();

    // A non-identity expected root is a caller contract violation: refused
    // before any execution or spawn.
    let error = embedded
        .execute(fixture.request_expected("return 1;", Some("not-a-root".into())))
        .expect_err("non-identity refuses");
    assert!(
        matches!(error, SupervisorError::InvalidRequest(_)),
        "error={error}"
    );
    let error = oneshot
        .execute(fixture.request_expected("return 1;", Some("not-a-root".into())))
        .expect_err("non-identity refuses");
    assert!(
        matches!(error, SupervisorError::InvalidRequest(_)),
        "error={error}"
    );
    assert_eq!(oneshot.child_spawn_count(), 0, "no spawn for a refused request");
    assert_quiescent(&embedded);
    assert_quiescent(&oneshot);

    // An unknown identity fails typed in preflight (no execution, no
    // write), on both profiles.
    let ghost = zero_abi::sha256_hex(b"never-published-session-root");
    let response = embedded
        .execute(fixture.request_expected("return 1;", Some(ghost.clone())))
        .expect("unknown root is a protocol response");
    assert_eq!(response.kind, ZerokernelResultKind::Failed, "errors={:?}", failed_errors(&response));
    assert!(
        has_error(&response, "not present in the session store"),
        "errors={:?}",
        failed_errors(&response)
    );
    assert!(response.root_evidence.unchanged);
    assert!(response.root_evidence.successor_root.is_none());
    assert_quiescent(&embedded);
    let response = oneshot
        .execute(fixture.request_expected("return 1;", Some(ghost)))
        .expect("unknown root is a protocol response");
    assert_eq!(response.kind, ZerokernelResultKind::Failed, "errors={:?}", failed_errors(&response));
    assert!(
        has_error(&response, "not present in the session store"),
        "errors={:?}",
        failed_errors(&response)
    );
    assert_eq!(oneshot.child_spawn_count(), 0, "preflight refusal must not spawn");
    assert_quiescent(&oneshot);

    // Existing CAS objects that are not session-state maps fail typed too:
    // junk bytes, a non-object JSON value, and an over-budget map are all
    // refused as invalid session state roots.
    let cas = zero_store::SharedCas::open(fixture.state_root.clone());
    let junk = cas.put(b"not json at all").expect("publish junk object");
    let array = cas.put(br#"[1, 2, 3]"#).expect("publish array object");
    let over = cas
        .put(format!(r#"{{"k": "{}"}}"#, "x".repeat(5 * 1024)).as_bytes())
        .expect("publish over-budget object");
    for (root, needle) in [
        (junk, "not valid JSON"),
        (array, "not a JSON object of state entries"),
        (over, "violates the state budgets"),
    ] {
        let response = embedded
            .execute(fixture.request_expected("return 1;", Some(root)))
            .expect("invalid object is a protocol response");
        assert_eq!(response.kind, ZerokernelResultKind::Failed, "errors={:?}", failed_errors(&response));
        assert!(
            has_error(&response, "not a valid session state root") && has_error(&response, needle),
            "errors={:?}",
            failed_errors(&response)
        );
        assert!(response.root_evidence.unchanged);
        assert!(response.root_evidence.successor_root.is_none());
        assert_quiescent(&embedded);
    }
}
