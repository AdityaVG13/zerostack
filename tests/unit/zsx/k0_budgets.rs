//! K0 budget vector, cancellation ordering, quiescence, and no-orphans
//! (zerostack-zksb).
//!
//! Exercises the complete finite K0 budget vector through the real
//! supervisor + guest surface, reusing the supervisor contract seams:
//! - guest fuel/CPU: `cpu_ms` maps to the instruction budget, so a bounded
//!   loop exhausts fuel typed;
//! - wall: an unresolved promise waits under the wall deadline and fails
//!   typed (never hangs); a pre-set cancel still wins immediately (soft
//!   cancel precedes hard termination);
//! - memory: a staged allocation past `memory_bytes` fails typed;
//! - stack: deep recursion fails typed at the derived depth ceiling;
//! - output: an oversized result is a bounded spill ref under the return
//!   policy, and fails typed when no spill root exists;
//! - host-call count: `max_calls` fails typed mid-flight and the failure
//!   ledger records exactly the admitted calls;
//! - parallel calls: a `z.parallel` fan-out past the remaining total call
//!   budget fails typed;
//! - unavailable authority: GPU / process / shell / network / database
//!   mentions fail typed as denied (broker and runtime), never as unknown
//!   names, and no one-shot child is spawned for a broker refusal;
//! - durable delta: per-call guest state never writes to the state root;
//!   failing calls leave project and session roots unchanged;
//! - every terminal leaves zero live executors/children/GPU and bounded
//!   ledger charges; the session stays usable.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;
use zero_abi::zerokernel::{
    FiniteBudget, ReturnKind, ReturnPolicy, RootBindings, ZerokernelExecuteRequest,
    ZerokernelResultKind,
};
use zsx_core::supervisor::{OneShotChild, Supervisor, SupervisorProfile};

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
            "zerostack-k0-budgets-{label}-{}-{}-{:x}",
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
            session: format!("k0-budgets-{}-{label}", std::process::id()),
            root,
            state_root,
        }
    }

    fn request(&self, program: &str) -> ZerokernelExecuteRequest {
        self.request_budgeted(
            program,
            FiniteBudget::new(WALL_MS, CPU_MS, MEMORY_BYTES, MAX_CALLS).expect("budget"),
            Some(self.state_root.clone()),
        )
    }

    fn request_budgeted(
        &self,
        program: &str,
        budget: FiniteBudget,
        expected_session_root: Option<PathBuf>,
    ) -> ZerokernelExecuteRequest {
        let root_text = self.root.to_string_lossy().into_owned();
        let state_text = expected_session_root
            .unwrap_or_else(|| self.state_root.clone())
            .to_string_lossy()
            .into_owned();
        ZerokernelExecuteRequest::new(
            program.into(),
            Some(self.session.clone()),
            budget,
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

    fn embedded(&self) -> Supervisor {
        Supervisor::builder(self.root.clone())
            .with_state_root(self.state_root.clone())
            .with_session_id(self.session.clone())
            .with_profile(SupervisorProfile::Embedded)
            .build_canonical()
            .expect("embedded supervisor builds")
    }

    /// Embedded supervisor whose session state root IS the project root:
    /// `state_root == root`, so the host installs no result-spill root and
    /// an oversized result fails typed instead of becoming a ref.
    fn embedded_no_state_root(&self) -> Supervisor {
        Supervisor::builder(self.root.clone())
            .with_state_root(self.root.clone())
            .with_session_id(self.session.clone())
            .with_profile(SupervisorProfile::Embedded)
            .build_canonical()
            .expect("embedded supervisor without separate state root builds")
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

/// Every settled call leaves zero live executors, children, and GPU
/// contexts (the no-orphan / no-resident-resource law).
fn assert_quiescent(supervisor: &Supervisor) {
    assert_eq!(supervisor.live_executors(), 0);
    assert_eq!(supervisor.live_children(), 0);
    assert_eq!(supervisor.live_gpu(), 0);
}

fn failed_errors(response: &zero_abi::zerokernel::ZerokernelExecuteResponse) -> Vec<String> {
    response.preflight.errors.clone()
}

fn has_error(response: &zero_abi::zerokernel::ZerokernelExecuteResponse, needle: &str) -> bool {
    failed_errors(response)
        .iter()
        .any(|error| error.contains(needle))
}

/// Sorted top-level entries of a directory (relative names). Used to prove
/// the state root gains no durable delta from per-call guest state.
fn dir_entries(root: &Path) -> Vec<String> {
    let mut entries: Vec<String> = match std::fs::read_dir(root) {
        Ok(entries) => entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    };
    entries.sort();
    entries
}

// ---------------------------------------------------------------------------
// Guest fuel / CPU budget
// ---------------------------------------------------------------------------

#[test]
fn fuel_budget_bounds_guest_compute_typed() {
    let fixture = Fixture::new("fuel");
    let program = r#"
        for (let i = 0; i < 200000; i++) { let x = i; }
        return 0;
    "#;
    // 10 ms of CPU budget = 100 000 instructions of fuel; the 200 000
    // iteration loop needs more, so it exhausts fuel typed long before the
    // 5 s wall deadline.
    let lean = FiniteBudget::new(WALL_MS, 10, MEMORY_BYTES, MAX_CALLS).expect("lean budget");
    for supervisor in [fixture.embedded(), fixture.oneshot()] {
        let response = supervisor
            .execute(fixture.request_budgeted(program, lean.clone(), None))
            .expect("fuel exhaustion is a protocol response");
        assert_eq!(response.kind, ZerokernelResultKind::Failed, "errors={:?}", failed_errors(&response));
        assert!(
            has_error(&response, "instruction budget exhausted"),
            "errors={:?}",
            failed_errors(&response)
        );
        assert!(response.root_evidence.unchanged);
        assert_eq!(response.ledger.calls_made, 0);
        assert_eq!(response.ledger.bytes_out, 0);
        assert_quiescent(&supervisor);
    }
    // Positive control: the same plan under a generous CPU budget
    // completes, proving the CPU budget is the bound.
    let response = fixture
        .embedded()
        .execute(fixture.request(program))
        .expect("generous fuel completes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", failed_errors(&response));
    assert_eq!(response.result, Some(json!(0)));
    assert_eq!(response.ledger.calls_made, 0);
}

// ---------------------------------------------------------------------------
// Memory budget
// ---------------------------------------------------------------------------

#[test]
fn memory_budget_fails_typed() {
    let fixture = Fixture::new("memory");
    let supervisor = fixture.embedded();
    // 70 000 000 repeated bytes preflights against the 64 MiB memory budget
    // before any allocation: typed MemoryLimit, never an OOM.
    let program = "return 'x'.repeat(70000000);";
    let response = supervisor
        .execute(fixture.request(program))
        .expect("memory refusal is a protocol response");
    assert_eq!(response.kind, ZerokernelResultKind::Failed, "result={:?}", response.result);
    assert!(
        has_error(&response, "memory budget exceeded"),
        "errors={:?}",
        failed_errors(&response)
    );
    assert!(response.root_evidence.unchanged);
    assert_eq!(response.ledger.calls_made, 0);
    assert_quiescent(&supervisor);
}

// ---------------------------------------------------------------------------
// Stack budget
// ---------------------------------------------------------------------------

#[test]
fn stack_depth_fails_typed() {
    let fixture = Fixture::new("stack");
    let supervisor = fixture.embedded();
    // 100 000 nested calls hit the derived evaluation-depth ceiling
    // (1 MiB stack / 2 KiB per frame, capped) and fail typed before the
    // native stack can overflow.
    let program = r#"
        function f(n) { return n === 0 ? 0 : 1 + f(n - 1); }
        return f(100000);
    "#;
    let response = supervisor
        .execute(fixture.request(program))
        .expect("depth refusal is a protocol response");
    assert_eq!(response.kind, ZerokernelResultKind::Failed, "result={:?}", response.result);
    assert!(
        has_error(&response, "evaluation depth exceeds"),
        "errors={:?}",
        failed_errors(&response)
    );
    assert!(response.root_evidence.unchanged);
    assert_quiescent(&supervisor);
}

// ---------------------------------------------------------------------------
// Output / result bytes
// ---------------------------------------------------------------------------

#[test]
fn output_budget_bounded_spill_or_typed_failure() {
    let fixture = Fixture::new("output");
    let supervisor = fixture.embedded();
    let program = "return 'x'.repeat(300000);";
    // With a spill root (the default fixture): the 300 KiB result is
    // published as a bounded ref under the 4096-byte visible budget; the
    // ledger counts the ref envelope, never the payload.
    let response = supervisor
        .execute(fixture.request(program))
        .expect("spill is a protocol response");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", failed_errors(&response));
    let result = response.result.expect("completed result");
    assert_eq!(result["content"]["kind"], json!("ref"), "result={result}");
    let reference = result["content"]["ref"].as_str().expect("spill ref");
    assert!(reference.starts_with("tz://blob/"), "ref={reference}");
    assert!(
        response.ledger.bytes_out > 0 && response.ledger.bytes_out < 4096,
        "bytes_out={}",
        response.ledger.bytes_out
    );
    assert_quiescent(&supervisor);

    // Without a spill root (state_root == root): the same result fails
    // typed with the exact bound, and the roots stay unchanged.
    let no_spill = fixture.embedded_no_state_root();
    let request = fixture.request_budgeted(
        program,
        FiniteBudget::new(WALL_MS, CPU_MS, MEMORY_BYTES, MAX_CALLS).expect("budget"),
        Some(fixture.root.clone()),
    );
    let response = no_spill
        .execute(request)
        .expect("oversized result is a protocol response");
    assert_eq!(response.kind, ZerokernelResultKind::Failed, "result={:?}", response.result);
    assert!(
        has_error(&response, "result is") && has_error(&response, "maximum is"),
        "errors={:?}",
        failed_errors(&response)
    );
    assert!(response.root_evidence.unchanged);
    assert_quiescent(&no_spill);
}

// ---------------------------------------------------------------------------
// Host-call count (max_calls)
// ---------------------------------------------------------------------------

#[test]
fn host_call_budget_fails_typed_mid_flight_with_exact_ledger() {
    let fixture = Fixture::new("calls");
    let program = r#"
        await zero.help.search({query: "fs"});
        await zero.help.search({query: "fs"});
        await zero.help.search({query: "fs"});
        await zero.help.search({query: "fs"});
        return 1;
    "#;
    let tight = FiniteBudget::new(WALL_MS, CPU_MS, MEMORY_BYTES, 3).expect("tight budget");
    for supervisor in [fixture.embedded(), fixture.oneshot()] {
        let response = supervisor
            .execute(fixture.request_budgeted(program, tight.clone(), None))
            .expect("call budget refusal is a protocol response");
        assert_eq!(response.kind, ZerokernelResultKind::Failed, "errors={:?}", failed_errors(&response));
        assert!(
            has_error(&response, "host-call budget exceeded"),
            "errors={:?}",
            failed_errors(&response)
        );
        // Exact ledger charges: exactly the three admitted calls, zero
        // output bytes on the failure terminal.
        assert_eq!(response.ledger.calls_made, 3, "ledger={:?}", response.ledger);
        assert_eq!(response.ledger.bytes_out, 0);
        assert!(response.ledger.wall_ms_used > 0);
        assert!(response.root_evidence.unchanged);
        assert_quiescent(&supervisor);
    }
    // Positive control: the same plan fits in a 4-call budget and the
    // ledger reports exactly four calls.
    let four = FiniteBudget::new(WALL_MS, CPU_MS, MEMORY_BYTES, 4).expect("four-call budget");
    let roomy = fixture.embedded();
    let response = roomy
        .execute(fixture.request_budgeted(program, four, None))
        .expect("fits budget completes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", failed_errors(&response));
    assert_eq!(response.ledger.calls_made, 4, "ledger={:?}", response.ledger);
    assert_quiescent(&roomy);
}

#[test]
fn parallel_fan_out_counts_against_total_call_budget() {
    let fixture = Fixture::new("parallel-calls");
    // Three parallel specs under a 2-call total budget: the third dispatch
    // is refused typed, and the ledger records exactly the two admitted
    // calls. The fan-out bound itself (K0_PARALLEL_LIMIT) is enforced
    // separately by the guest surface tests.
    let program = r#"
        return await z.parallel(["help.catalog", "help.catalog", "help.catalog"]);
    "#;
    let tight = FiniteBudget::new(WALL_MS, CPU_MS, MEMORY_BYTES, 2).expect("tight budget");
    let supervisor = fixture.embedded();
    let response = supervisor
        .execute(fixture.request_budgeted(program, tight, None))
        .expect("parallel call budget refusal is a protocol response");
    assert_eq!(response.kind, ZerokernelResultKind::Failed, "errors={:?}", failed_errors(&response));
    assert!(
        has_error(&response, "host-call budget exceeded"),
        "errors={:?}",
        failed_errors(&response)
    );
    assert_eq!(response.ledger.calls_made, 2, "ledger={:?}", response.ledger);
    assert!(response.root_evidence.unchanged);
    assert_quiescent(&supervisor);
}

// ---------------------------------------------------------------------------
// Source bytes
// ---------------------------------------------------------------------------

#[test]
fn source_bytes_beyond_protocol_bound_refused() {
    let fixture = Fixture::new("source-bytes");
    let oneshot = fixture.oneshot();
    // A program past the 64 KiB protocol bound is refused before any
    // execution and before any one-shot child spawn. The struct is built
    // directly (the `new` constructor validates and would refuse first).
    let root_text = fixture.root.to_string_lossy().into_owned();
    let state_text = fixture.state_root.to_string_lossy().into_owned();
    let request = ZerokernelExecuteRequest {
        abi_version: zero_abi::zerokernel::ZEROKERNEL_ABI_VERSION.into(),
        program: format!("/* {} */ return 1;", "x".repeat(70 * 1024)),
        session: Some(fixture.session.clone()),
        budget: FiniteBudget::new(WALL_MS, CPU_MS, MEMORY_BYTES, MAX_CALLS).expect("budget"),
        return_policy: ReturnPolicy::new(ReturnKind::Inline, 4096).expect("policy"),
        roots: RootBindings::new(
            Some(root_text.clone()),
            root_text,
            None,
            None,
            Some(state_text),
        )
        .expect("roots"),
    };
    let error = oneshot
        .execute(request)
        .expect_err("oversized program refuses");
    assert!(
        matches!(error, zsx_core::supervisor::SupervisorError::InvalidRequest(_)),
        "error={error:?}"
    );
    assert_eq!(oneshot.child_spawn_count(), 0);
    assert_quiescent(&oneshot);
}

// ---------------------------------------------------------------------------
// Unresolved promises and cancellation ordering
// ---------------------------------------------------------------------------

#[test]
fn unresolved_promise_hits_wall_deadline_typed() {
    let fixture = Fixture::new("unresolved");
    let program = "return await new Promise(() => {});";
    let budget = FiniteBudget::new(700, CPU_MS, MEMORY_BYTES, MAX_CALLS).expect("budget");
    for supervisor in [fixture.embedded(), fixture.oneshot()] {
        let started = std::time::Instant::now();
        let response = supervisor
            .execute(fixture.request_budgeted(program, budget.clone(), None))
            .expect("unresolved promise is a protocol response");
        assert_eq!(response.kind, ZerokernelResultKind::Failed, "errors={:?}", failed_errors(&response));
        assert!(
            has_error(&response, "wall-clock deadline exceeded"),
            "errors={:?}",
            failed_errors(&response)
        );
        // The promise waited under the wall deadline: the call did not fail
        // instantly and did not hang past the budget.
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(500) && elapsed < Duration::from_secs(15),
            "unresolved promise must wait under the wall deadline, took {elapsed:?}"
        );
        assert!(response.root_evidence.unchanged);
        assert_quiescent(&supervisor);
    }
}

#[test]
fn soft_cancel_precedes_deadline_for_unresolved_promise() {
    // A pre-set cancel must win immediately over a promise that would
    // otherwise wait out the wall deadline: soft cancellation precedes hard
    // termination on every path.
    let fixture = Fixture::new("cancel-before-deadline");
    let program = "return await new Promise(() => {});";
    for supervisor in [fixture.embedded(), fixture.oneshot()] {
        let cancel = Arc::new(AtomicBool::new(true));
        let started = std::time::Instant::now();
        let response = supervisor
            .execute_cancellable(
                fixture.request(program),
                cancel,
            )
            .expect("cancelled is a protocol response");
        assert_eq!(response.kind, ZerokernelResultKind::Failed);
        assert!(
            has_error(&response, "cancelled"),
            "errors={:?}",
            failed_errors(&response)
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "pre-set cancel must settle promptly, took {:?}",
            started.elapsed()
        );
        assert!(response.root_evidence.unchanged);
        assert_quiescent(&supervisor);
    }
}

#[test]
fn promise_settled_by_then_callback_after_connector_completes() {
    // A manual promise settled from a then-callback that runs only after a
    // connector dispatch completes must make progress instead of failing:
    // the unresolved-promise wait pumps connector completions and pending
    // then-chains under the deadline.
    let fixture = Fixture::new("settle-later");
    let supervisor = fixture.embedded();
    let program = r#"
        let settle;
        const p = new Promise((res) => { settle = res; });
        zero.help.search({query: "fs"}).then(() => { settle("done"); });
        return await p;
    "#;
    let response = supervisor
        .execute(fixture.request(program))
        .expect("settled-later plan executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", failed_errors(&response));
    assert_eq!(response.result, Some(json!("done")));
    assert_eq!(response.ledger.calls_made, 1, "ledger={:?}", response.ledger);
    assert_quiescent(&supervisor);
}

// ---------------------------------------------------------------------------
// Unavailable authority: explicit denial, never unknown names
// ---------------------------------------------------------------------------

#[test]
fn denied_authority_classes_fail_typed_before_execution() {
    let fixture = Fixture::new("denied");
    let embedded = fixture.embedded();
    let oneshot = fixture.oneshot();
    for (program, needle) in [
        (
            "return await zero.gpu.compute({kernels: 1});",
            "K0 grants no GPU authority",
        ),
        (
            r#"return await z.invoke("process.spawn", {cmd: "ls"});"#,
            "K0 grants no process authority",
        ),
        (
            r#"return await z.parallel(["net.fetch", "help.catalog"]);"#,
            "K0 grants no network authority",
        ),
        (
            r#"return await zero.shell.run({command: "ls"});"#,
            "K0 grants no shell authority",
        ),
        (
            r#"return await zero.db.query({sql: "select 1"});"#,
            "K0 grants no database authority",
        ),
        (
            r#"return await zero.os.env({key: "PATH"});"#,
            "K0 grants no operating-system authority",
        ),
        (
            r#"return await zero.daemon.start({name: "d"});"#,
            "K0 grants no daemon authority",
        ),
    ] {
        let response = embedded
            .execute(fixture.request(program))
            .expect("denied authority is a protocol response");
        assert_eq!(response.kind, ZerokernelResultKind::Failed, "errors={:?}", failed_errors(&response));
        assert!(has_error(&response, needle), "program={program} errors={:?}", failed_errors(&response));
        assert!(response.root_evidence.unchanged);
        assert_quiescent(&embedded);
    }
    // The broker refusal precedes any one-shot child spawn: no process was
    // ever created for a denied authority mention.
    let response = oneshot
        .execute(fixture.request(
            "return await zero.gpu.compute({kernels: 1});",
        ))
        .expect("denied authority is a protocol response");
    assert_eq!(response.kind, ZerokernelResultKind::Failed);
    assert!(has_error(&response, "K0 grants no GPU authority"));
    assert_eq!(oneshot.child_spawn_count(), 0, "denied authority must not spawn");
    assert_quiescent(&oneshot);
    // The runtime enforces the same denial for targets the broker scan
    // cannot see (object-form z.parallel specs): the failure is typed with
    // the authority class, not a generic reach error.
    let response = embedded
        .execute(fixture.request(
            r#"return await z.parallel([{surface: "gpu", method: "kernel"}]);"#,
        ))
        .expect("runtime denial is a protocol response");
    assert_eq!(response.kind, ZerokernelResultKind::Failed, "errors={:?}", failed_errors(&response));
    assert!(
        has_error(&response, "K0 grants no GPU authority"),
        "errors={:?}",
        failed_errors(&response)
    );
    assert!(response.root_evidence.unchanged);
    // A successful call still reports zero GPU contexts: the idle GPU count
    // is structurally zero.
    let response = embedded
        .execute(fixture.request("return await z.capabilities.search({query: 'fs'});"))
        .expect("successful call executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed);
    assert_eq!(embedded.live_gpu(), 0);
    assert_quiescent(&embedded);
}

// ---------------------------------------------------------------------------
// Durable delta and failure transactional integrity
// ---------------------------------------------------------------------------

#[test]
fn guest_state_is_per_call_and_failure_leaves_roots_unchanged() {
    let fixture = Fixture::new("durable-delta");
    let embedded = fixture.embedded();
    let before = dir_entries(&fixture.state_root);

    // A successful call that writes guest state: the delta is in-memory and
    // dies with the runtime — nothing durable appears in the state root.
    let response = embedded
        .execute(fixture.request(
            r#"
            z.state.set("k", {n: 1});
            return z.state.list();
        "#,
        ))
        .expect("state plan executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", failed_errors(&response));
    assert_eq!(response.result, Some(json!(["k"])));
    assert_eq!(dir_entries(&fixture.state_root), before, "guest state must not persist");

    // A failing call that first writes guest state and then spins to the
    // deadline: the failure terminal proves roots unchanged and the state
    // root gains no durable delta.
    let budget = FiniteBudget::new(400, CPU_MS, MEMORY_BYTES, MAX_CALLS).expect("budget");
    let response = embedded
        .execute(fixture.request_budgeted(
            r#"
            z.state.set("k", {n: 1});
            while (true) {}
        "#,
            budget,
            None,
        ))
        .expect("deadline is a protocol response");
    assert_eq!(response.kind, ZerokernelResultKind::Failed, "errors={:?}", failed_errors(&response));
    assert!(has_error(&response, "deadline"), "errors={:?}", failed_errors(&response));
    assert!(response.root_evidence.unchanged);
    assert_eq!(
        response.root_evidence.before,
        response.root_evidence.after,
        "failure must prove roots unchanged"
    );
    assert_eq!(dir_entries(&fixture.state_root), before, "failure must not write durable state");
    assert_quiescent(&embedded);
}

// ---------------------------------------------------------------------------
// Every terminal is bounded and the session stays usable
// ---------------------------------------------------------------------------

#[test]
fn budget_failures_leave_session_usable_with_zero_idle_resources() {
    let fixture = Fixture::new("usable");
    let supervisor = fixture.embedded();
    let default = FiniteBudget::new(WALL_MS, CPU_MS, MEMORY_BYTES, MAX_CALLS).expect("budget");
    // A run of every budget failure shape on one supervisor: each returns a
    // typed Failed envelope, leaves zero live resources, and the session
    // keeps executing afterwards.
    let cases: Vec<(&str, FiniteBudget)> = vec![
        // Fuel: a 10 ms CPU budget cannot cover 200k loop iterations.
        (
            "for (let i = 0; i < 200000; i++) { let x = i; } return 0;",
            FiniteBudget::new(WALL_MS, 10, MEMORY_BYTES, MAX_CALLS).expect("lean budget"),
        ),
        ("return 'x'.repeat(70000000);", default.clone()),
        (
            "function f(n) { return n === 0 ? 0 : 1 + f(n - 1); } return f(100000);",
            default.clone(),
        ),
        // Unresolved promise: waits under a 600 ms wall, then fails typed.
        (
            "return await new Promise(() => {});",
            FiniteBudget::new(600, CPU_MS, MEMORY_BYTES, MAX_CALLS).expect("short budget"),
        ),
        ("return await zero.gpu.compute({kernels: 1});", default.clone()),
    ];
    for (program, budget) in cases {
        let response = supervisor
            .execute(fixture.request_budgeted(program, budget, None))
            .expect("budget failure is a protocol response");
        assert_eq!(response.kind, ZerokernelResultKind::Failed, "errors={:?}", failed_errors(&response));
        assert!(response.root_evidence.unchanged);
        assert_quiescent(&supervisor);
    }
    let response = supervisor
        .execute(fixture.request("return 42;"))
        .expect("session stays usable");
    assert_eq!(response.kind, ZerokernelResultKind::Completed);
    assert_eq!(response.result, Some(json!(42)));
    assert_quiescent(&supervisor);
}
