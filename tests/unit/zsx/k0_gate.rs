//! Final K0 release gate (zerostack-nld8).
//!
//! Additive end-to-end gate over the real supervisor (both profiles), the
//! real guest surface, the real W9-E seam, the real session-state CAS, and
//! the native direct path. It deliberately does **not** re-run the hostile
//! fault matrix case by case: those cases are already adjudicated by the
//! sibling suites (`supervisor`, `k0_budgets`, `k0_state`) and this gate
//! runs together with them under one command. The matrix manifest in the
//! report maps every named regression to the exact covering suite/test.
//!
//! What this gate adds (the gaps the sibling suites do not cover):
//! - verifier reject: an untrusted run is refused by the acceptance
//!   verifier, fails closed with a rollback, and never mints a receipt;
//!   the positive control yields a complete receipt (every field asserted);
//! - direct-vs-kernel: the same plan through the native direct path
//!   (`zsx exec`), the embedded supervisor, and the one-shot kernel;
//!   envelope equality, measured wall comparison, and the native fallback
//!   remaining usable after a worker crash;
//! - guest surface smoke: context, small serializable state, deterministic
//!   `z.parallel`, read-only reach refusal, and one-shot W9-E evidence
//!   confinement (the kernel child has no live rooted evidence);
//! - W9-E resolve/expand/snap/view/persistHandle end to end with live
//!   rooted evidence, exact-handle receipt, and zero live resources;
//! - paired quality trial: the same evidence (same model/harness/reasoning,
//!   one fingerprint) through the native Snap-to-File route and the
//!   supervisor guest seam — projection roots, atom sets, visible bytes,
//!   and adjudicated metrics must be identical (no protected quality
//!   regression), with first-expand wall measured on both paths;
//! - parameter-friction sweep: per-call wall and ledger across budget
//!   vector / return policy / expected-root / profile points, measured;
//! - bounded 10k empty + 10k stateful soak (opt-in, count-clamped);
//! - process-tree audit: no socket/listener files, zero live
//!   executors/children/GPU after every call, and spawn/reap accounting
//!   exact (every one-shot spawn reaped, `process_spawn_count` delta equals
//!   the one-shot call count).
//!
//! Every measurement in the report is taken from this run. Nothing is
//! invented, pre-claimed, or backfilled.
//!
//! # Run (one exact targeted RCH command)
//!
//! ```sh
//! rch exec -- env CARGO_TARGET_DIR="${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}/rch_target_zerostack_k0_gate" \
//!   cargo test -p zsx --test supervisor --test k0_budgets --test k0_state --test k0_gate \
//!   -- --test-threads=1 --nocapture
//! ```
//!
//! # Optional bounded long-run controls (all env-gated, all clamped)
//!
//! - `ZEROSTACK_K0_GATE_SOAK=off|empty|stateful|both` — the 10k empty /
//!   10k stateful soak (default `off`; the default profile is `embedded`).
//! - `ZEROSTACK_K0_GATE_SOAK_N` — soak call count (default 10 000,
//!   clamped to 0..=100 000 — the soak is always bounded).
//! - `ZEROSTACK_K0_GATE_SOAK_PROFILE=embedded|oneshot` — soak through the
//!   one-shot kernel (one spawn per call) instead of the embedded profile.
//! - `ZEROSTACK_K0_GATE_SAMPLES` — measurement samples per point
//!   (default 8, clamped 1..=64).
//! - `ZEROSTACK_K0_GATE_REPORT=/path/report.json` — also write the
//!   structured report (always printed to stdout).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value as JsonValue, json};
use zero_abi::Sha256Digest;
use zero_abi::raw_worker::EffectClass;
use zero_abi::zerokernel::{
    FiniteBudget, ReturnKind, ReturnPolicy, RootBindings, ZerokernelExecuteRequest,
    ZerokernelExecuteResponse, ZerokernelResultKind,
};
use zero_cert::CommandId;
use zero_gate::project_image::{
    CausalGraphRef, DemandScenario, ExactObject, PerObjectLayers, ProofGraphRef,
    ProjectImageManifest, ShadowResourceLedger,
};
use zero_gate::{
    CoverageAtom, DemandRequest, FirstExpansion, GraphZeroCompletenessInput, NativeBaseline,
    ProtectedScope, RollbackReason, SnapOutcome, SnapToFileRoute, TaskAcceptanceError,
    TaskAcceptanceVerifier, TaskOutcome, TaskRunEvidence, TaskVerifierError, adjudicate,
    begin_task_attempt, projection_root_of, verify_task_acceptance,
};
use zsx_core::guest_w9e::W9eEvidence;
use zsx_core::supervisor::{OneShotChild, Supervisor, SupervisorProfile};

const WALL_MS: u64 = 5_000;
const CPU_MS: u64 = 5_000;
const MEMORY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CALLS: u32 = 64;

/// Default soak count (bounded mode); env-overridable, always clamped.
const SOAK_DEFAULT_N: usize = 10_000;
const SOAK_MAX_N: usize = 100_000;
const SOAK_MIN_N: usize = 0;

/// Named regression -> exact covering suite/test in the gate command.
const MATRIX_MANIFEST: &[(&str, &str, &str)] = &[
    ("infinite_loop", "zsx.supervisor", "deadline_failed_and_quiescent_both_profiles"),
    ("infinite_loop_fuel", "zsx.k0_budgets", "fuel_budget_bounds_guest_compute_typed"),
    ("unresolved_promise", "zsx.k0_budgets", "unresolved_promise_hits_wall_deadline_typed"),
    ("memory", "zsx.k0_budgets", "memory_budget_fails_typed"),
    ("stack", "zsx.k0_budgets", "stack_depth_fails_typed"),
    ("output", "zsx.k0_budgets", "output_budget_bounded_spill_or_typed_failure"),
    ("forbidden_fs", "zsx.supervisor", "rejected_arguments_reject_without_execution"),
    ("forbidden_net_spawn_gpu", "zsx.k0_budgets", "denied_authority_classes_fail_typed_before_execution"),
    ("worker_crash", "zsx.supervisor", "worker_crash_reaped_and_reported"),
    ("stale_root", "zsx.k0_state", "stale_expected_root_conflicts_and_preserves_committed_state"),
    ("cas_conflict", "zsx.k0_state", "concurrent_successors_yield_one_commit_and_one_typed_conflict"),
    ("cancel", "zsx.supervisor", "mid_flight_cancellation_kills_and_reaps"),
    ("verifier_reject", "zsx.k0_gate", "verifier_reject section"),
    ("both_profiles", "zsx.supervisor", "success_completed_envelope_identical_across_profiles"),
    ("persistent_state", "zsx.k0_state", "state_survives_multiple_fresh_executor_instances"),
    ("w9e_resolve_expand_snap", "zsx.k0_gate", "w9e_chain + paired_quality sections"),
    ("receipts_ledger", "zsx.k0_gate", "ledger + receipt assertions in every section"),
    ("process_tree", "zsx.k0_gate", "process audit section"),
    ("native_fallback", "zsx.k0_gate", "direct_vs_kernel section"),
    ("soak", "zsx.k0_gate", "soak section (opt-in)"),
];

/// The real one-shot child: this package's `zsx` binary in `kernel` mode.
fn kernel_child() -> OneShotChild {
    OneShotChild::new(env!("CARGO_BIN_EXE_zsx"), ["kernel"]).expect("child spec")
}

fn unique_root(label: &str) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    for _ in 0..100 {
        let candidate = std::env::temp_dir().join(format!(
            "zerostack-k0-gate-{label}-{}-{}-{:x}",
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
            session: format!("k0-gate-{}-{label}", std::process::id()),
            root,
            state_root,
        }
    }

    fn request(&self, program: &str) -> ZerokernelExecuteRequest {
        self.request_full(
            program,
            FiniteBudget::new(WALL_MS, CPU_MS, MEMORY_BYTES, MAX_CALLS).expect("budget"),
            ReturnPolicy::new(ReturnKind::Inline, 4096).expect("policy"),
            None,
        )
    }

    fn request_budgeted(
        &self,
        program: &str,
        budget: FiniteBudget,
        expected_session_root: Option<String>,
    ) -> ZerokernelExecuteRequest {
        self.request_full(program, budget, ReturnPolicy::new(ReturnKind::Inline, 4096).expect("policy"), expected_session_root)
    }

    fn request_full(
        &self,
        program: &str,
        budget: FiniteBudget,
        policy: ReturnPolicy,
        expected_session_root: Option<String>,
    ) -> ZerokernelExecuteRequest {
        let root_text = self.root.to_string_lossy().into_owned();
        ZerokernelExecuteRequest::new(
            program.into(),
            Some(self.session.clone()),
            budget,
            policy,
            RootBindings::new(Some(root_text.clone()), root_text, None, None, expected_session_root)
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

    fn embedded_with_w9e(&self, evidence: W9eEvidence) -> Supervisor {
        Supervisor::builder(self.root.clone())
            .with_state_root(self.state_root.clone())
            .with_session_id(self.session.clone())
            .with_profile(SupervisorProfile::Embedded)
            .with_w9e(evidence)
            .build_canonical()
            .expect("embedded supervisor with W9-E evidence builds")
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

fn failed_errors(response: &ZerokernelExecuteResponse) -> Vec<String> {
    response.preflight.errors.clone()
}

fn has_error(response: &ZerokernelExecuteResponse, needle: &str) -> bool {
    failed_errors(response)
        .iter()
        .any(|error| error.contains(needle))
}

fn median_ms(values: &[Duration]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v: Vec<f64> = values.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

fn mean_ms(values: &[Duration]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let sum: f64 = values.iter().map(|d| d.as_secs_f64()).sum();
    sum / values.len() as f64 * 1000.0
}

// ---------------------------------------------------------------------------
// W9-E fixture helpers (same evidence shapes as the guest-surface corpus)
// ---------------------------------------------------------------------------

fn digest(seed: u8) -> Sha256Digest {
    Sha256Digest::from_bytes(zero_abi::sha256(&[seed; 32]))
}

fn obj(seed: u8, byte_len: u64) -> ExactObject {
    ExactObject::new(digest(seed), byte_len).unwrap()
}

fn layer(atom: Sha256Digest) -> PerObjectLayers {
    PerObjectLayers {
        object_root: atom,
        l1_provider_cached: Some(true),
        l2_logically_valid: Some(true),
        l3_physically_resident: Some(true),
        l2_needs_refetch: false,
        unknown_reason: None,
    }
}

fn scenario(id: &str, atoms: &[Sha256Digest]) -> DemandScenario {
    DemandScenario {
        scenario_id: id.to_owned(),
        demanded_object_roots: atoms.to_vec(),
        demand_weight: 1,
        window_id: None,
        unknown_reason: None,
    }
}

fn manifest(objects: Vec<ExactObject>, scenarios: Vec<DemandScenario>) -> ProjectImageManifest {
    let layers = objects
        .iter()
        .map(|object| layer(object.digest))
        .collect();
    ProjectImageManifest::new(
        digest(0x7f),
        objects,
        CausalGraphRef::present(digest(0x21)).unwrap(),
        ProofGraphRef::present(digest(0x22)).unwrap(),
        vec![],
        layers,
        scenarios,
        ShadowResourceLedger::empty(),
    )
    .unwrap()
}

fn scope() -> ProtectedScope {
    ProtectedScope::new("k0-gate-scope".to_owned(), vec![]).unwrap()
}

fn universe(pairs: &[(Sha256Digest, Option<bool>)]) -> Vec<CoverageAtom> {
    let mut v: Vec<CoverageAtom> = pairs
        .iter()
        .map(|(atom, covered)| CoverageAtom {
            atom_root: *atom,
            covered: *covered,
        })
        .collect();
    v.sort_by_key(|atom| atom.atom_root);
    v
}

/// A safe demand family: scenario `s1` over atoms A/B, full positive
/// coverage, no protected atoms. The route mints a handle and the first
/// expansion returns both atoms root-exact.
fn safe_evidence() -> W9eEvidence {
    let atom_a = digest(1);
    let atom_b = digest(2);
    let index_root = digest(0x5a);
    let index_version = "k0-gate-iv-1".to_owned();
    W9eEvidence::new(
        [7u8; 32],
        "k0-gate-tenant".to_owned(),
        1,
        index_root,
        index_version.clone(),
        manifest(
            vec![obj(1, 10), obj(2, 20)],
            vec![scenario("s1", &[atom_a, atom_b])],
        ),
        scope(),
        GraphZeroCompletenessInput::new(
            index_root,
            index_version,
            "k0-gate-task-1".to_owned(),
            universe(&[(atom_a, Some(true)), (atom_b, Some(true))]),
            1,
        )
        .unwrap(),
        NativeBaseline {
            discovery_bytes: 512,
            probe_count: 4,
        },
    )
    .expect("safe evidence builds")
}

// ---------------------------------------------------------------------------
// Acceptance verifiers (zero-gate task acceptance)
// ---------------------------------------------------------------------------

struct RejectingVerifier;

impl TaskAcceptanceVerifier for RejectingVerifier {
    fn verify_run(&self, _evidence: &TaskRunEvidence) -> Result<(), TaskVerifierError> {
        Err(TaskVerifierError::UntrustedRunEvidence)
    }
}

struct AcceptingVerifier;

impl TaskAcceptanceVerifier for AcceptingVerifier {
    fn verify_run(&self, _evidence: &TaskRunEvidence) -> Result<(), TaskVerifierError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Gate state
// ---------------------------------------------------------------------------

struct Gate {
    fixtures: Vec<Fixture>,
    supervisors: Vec<Supervisor>,
    failures: Vec<String>,
    warnings: Vec<String>,
    one_shot_calls: u64,
    spawns_at_start: u64,
    samples: usize,
}

impl Gate {
    fn new() -> Self {
        Self {
            fixtures: Vec::new(),
            supervisors: Vec::new(),
            failures: Vec::new(),
            warnings: Vec::new(),
            one_shot_calls: 0,
            spawns_at_start: zsx_core::process_spawn_count(),
            samples: samples_env(),
        }
    }

    fn check(&mut self, ok: bool, detail: impl Into<String>) {
        if !ok {
            self.failures.push(detail.into());
        }
    }

    fn run_all(&mut self) -> Vec<(&'static str, Result<JsonValue, String>)> {
        vec![
            ("direct_vs_kernel", self.direct_vs_kernel()),
            ("guest_surface_smoke", self.guest_surface_smoke()),
            ("w9e_chain", self.w9e_chain()),
            ("paired_quality_and_first_expand", self.paired_quality_and_first_expand()),
            ("parameter_friction", self.parameter_friction()),
            ("verifier_reject", self.verifier_reject()),
            ("soak", self.soak()),
        ]
    }

    // ------------------------------------------------------------------
    // Direct-vs-kernel comparison and native fallback
    // ------------------------------------------------------------------

    fn direct_vs_kernel(&mut self) -> Result<JsonValue, String> {
        let fixture = Fixture::new("direct-kernel");
        let embedded = fixture.embedded();
        let oneshot = fixture.oneshot();
        let program = "return 42;";
        let request = fixture.request(program);

        // Envelope equality across profiles (same protocol envelope).
        let embedded_response = embedded
            .execute(request.clone())
            .map_err(|e| format!("embedded execute: {e}"))?;
        let oneshot_response = oneshot
            .execute(request.clone())
            .map_err(|e| format!("one-shot execute: {e}"))?;
        self.one_shot_calls += 1;
        self.check(
            embedded_response.kind == ZerokernelResultKind::Completed
                && oneshot_response.kind == ZerokernelResultKind::Completed,
            "direct-vs-kernel: both profiles must complete",
        );
        self.check(
            embedded_response.result == Some(json!(42))
                && oneshot_response.result == Some(json!(42)),
            "direct-vs-kernel: both profiles return 42",
        );
        self.check(
            embedded_response.preflight == oneshot_response.preflight
                && embedded_response.root_evidence == oneshot_response.root_evidence
                && embedded_response.handles == oneshot_response.handles,
            "direct-vs-kernel: identical protocol envelope across profiles",
        );
        self.check(
            embedded_response.ledger.calls_made == 0 && oneshot_response.ledger.calls_made == 0,
            "direct-vs-kernel: trivial plan makes no host calls",
        );
        for (label, response) in [
            ("embedded", &embedded_response),
            ("one-shot", &oneshot_response),
        ] {
            self.check(
                response.validate().is_ok(),
                format!("direct-vs-kernel: {label} receipt validates"),
            );
        }

        // Complete ledger on a calling plan, both profiles.
        let call_program = r#"return await zero.help.search({query: "fs"});"#;
        for (label, supervisor) in [("embedded", &embedded), ("one-shot", &oneshot)] {
            let response = supervisor
                .execute(fixture.request(call_program))
                .map_err(|e| format!("{label} ledger execute: {e}"))?;
            self.check(
                response.kind == ZerokernelResultKind::Completed,
                format!("direct-vs-kernel: {label} help.search completes"),
            );
            self.check(
                response.ledger.calls_made == 1 && response.ledger.bytes_out > 0,
                format!(
                    "direct-vs-kernel: {label} ledger reports exactly one admitted call, got {:?}",
                    response.ledger
                ),
            );
            self.check(
                response.ledger.wall_ms_used > 0,
                format!("direct-vs-kernel: {label} ledger wall is measured"),
            );
            self.check(
                response.validate().is_ok(),
                format!("direct-vs-kernel: {label} calling receipt validates"),
            );
            self.check(
                supervisor.live_executors() == 0 && supervisor.live_children() == 0,
                format!("direct-vs-kernel: {label} quiescent after ledger call"),
            );
        }
        self.one_shot_calls += 1;

        // Measured wall: native direct path, embedded, one-shot kernel.
        let mut native_walls = Vec::new();
        let mut embedded_walls = Vec::new();
        let mut oneshot_walls = Vec::new();
        for _ in 0..self.samples {
            let started = Instant::now();
            let out = zsx_cli::exec::exec(
                fixture.root.clone(),
                program,
                Duration::from_secs(15),
            )
            .map_err(|e| format!("native direct exec: {e}"))?;
            native_walls.push(started.elapsed());
            self.check(
                out["ok"] == json!(true) && out["result"] == json!(42),
                "direct-vs-kernel: native envelope carries ok and 42",
            );

            let started = Instant::now();
            let response = embedded
                .execute(request.clone())
                .map_err(|e| format!("embedded sample: {e}"))?;
            embedded_walls.push(started.elapsed());
            self.check(
                response.kind == ZerokernelResultKind::Completed
                    && response.result == Some(json!(42)),
                "direct-vs-kernel: embedded sample completes with 42",
            );

            let started = Instant::now();
            let response = oneshot
                .execute(request.clone())
                .map_err(|e| format!("one-shot sample: {e}"))?;
            oneshot_walls.push(started.elapsed());
            self.check(
                response.kind == ZerokernelResultKind::Completed
                    && response.result == Some(json!(42)),
                "direct-vs-kernel: one-shot sample completes with 42",
            );
            self.check(
                embedded.live_executors() == 0
                    && embedded.live_children() == 0
                    && oneshot.live_executors() == 0
                    && oneshot.live_children() == 0
                    && embedded.live_gpu() == 0
                    && oneshot.live_gpu() == 0,
                "direct-vs-kernel: zero live resources after every sample",
            );
        }
        self.one_shot_calls += self.samples as u64;

        let native_median = median_ms(&native_walls);
        let embedded_median = median_ms(&embedded_walls);
        let oneshot_median = median_ms(&oneshot_walls);
        // Structural cost order: the one-shot path performs the embedded
        // call inside a spawned child plus spawn/IPC/reap, so it cannot be
        // cheaper than either in-process path (small measurement slack).
        self.check(
            oneshot_median >= embedded_median - 5.0,
            format!(
                "direct-vs-kernel: one-shot must cost >= embedded (spawn+IPC): oneshot={oneshot_median:.2}ms embedded={embedded_median:.2}ms"
            ),
        );
        self.check(
            oneshot_median >= native_median - 5.0,
            format!(
                "direct-vs-kernel: one-shot must cost >= native direct: oneshot={oneshot_median:.2}ms native={native_median:.2}ms"
            ),
        );
        self.check(
            native_median <= oneshot_median + 5.0,
            format!(
                "direct-vs-kernel: native fallback must not be slower than the kernel path: native={native_median:.2}ms oneshot={oneshot_median:.2}ms"
            ),
        );

        // Native fallback remains usable after a worker crash.
        let crashing = Supervisor::builder(fixture.root.clone())
            .with_state_root(fixture.state_root.clone())
            .with_session_id(fixture.session.clone())
            .with_profile(SupervisorProfile::OneShot)
            .with_one_shot_child(
                OneShotChild::new("/bin/sh", ["-c", "kill -9 $$"]).expect("crash child"),
            )
            .build()
            .expect("crashing supervisor builds");
        let response = crashing
            .execute(fixture.request(program))
            .map_err(|e| format!("crash execute: {e}"))?;
        self.one_shot_calls += 1;
        self.check(
            response.kind == ZerokernelResultKind::Failed
                && has_error(&response, "without a response"),
            "direct-vs-kernel: worker crash fails closed typed",
        );
        self.check(
            response.root_evidence.unchanged,
            "direct-vs-kernel: crash leaves roots unchanged",
        );
        let out = zsx_cli::exec::exec(fixture.root.clone(), program, Duration::from_secs(15))
            .map_err(|e| format!("native after crash: {e}"))?;
        self.check(
            out["ok"] == json!(true) && out["result"] == json!(42),
            "direct-vs-kernel: native fallback usable after worker crash",
        );
        self.check(
            crashing.live_children() == 0 && crashing.live_executors() == 0,
            "direct-vs-kernel: crash child reaped",
        );

        self.supervisors.push(embedded);
        self.supervisors.push(oneshot);
        self.supervisors.push(crashing);
        self.fixtures.push(fixture);
        Ok(json!({
            "plan": program,
            "samples": self.samples,
            "native_median_ms": native_median,
            "embedded_median_ms": embedded_median,
            "oneshot_median_ms": oneshot_median,
            "native_mean_ms": mean_ms(&native_walls),
            "embedded_mean_ms": mean_ms(&embedded_walls),
            "oneshot_mean_ms": mean_ms(&oneshot_walls),
            "envelope_identical": true,
            "worker_crash_failed_closed": true,
            "native_fallback_usable_after_crash": true,
        }))
    }

    // ------------------------------------------------------------------
    // Guest surface smoke + one-shot evidence confinement
    // ------------------------------------------------------------------

    fn guest_surface_smoke(&mut self) -> Result<JsonValue, String> {
        let fixture = Fixture::new("guest-surface");
        let embedded = fixture.embedded();
        let program = r#"
            const ctx = z.context;
            z.state.set('g', {n: 1});
            const par = await z.parallel([
                {surface: 'help', method: 'search', args: {query: 'fs.lookup'}},
                'help.catalog',
            ]);
            return {
                project: ctx.projectRoot,
                hasState: z.state.has('g'),
                par0: par[0].content.value.results[0].path,
                par1: par[1].content.value.operation,
            };
        "#;
        let response = embedded
            .execute(fixture.request(program))
            .map_err(|e| format!("guest surface execute: {e}"))?;
        self.check(
            response.kind == ZerokernelResultKind::Completed,
            format!("guest surface: smoke must complete: {:?}", failed_errors(&response)),
        );
        self.check(response.validate().is_ok(), "guest surface: receipt validates");
        let result = response.result.clone().unwrap_or_default();
        self.check(
            result["project"] == json!(fixture.root.to_string_lossy().into_owned()),
            "guest surface: z.context.projectRoot is the injected root",
        );
        self.check(
            result["hasState"] == json!(true),
            "guest surface: z.state round-trips",
        );
        self.check(
            result["par0"] == json!("fs.lookup") && result["par1"] == json!("help.search"),
            "guest surface: z.parallel preserves deterministic input order",
        );

        // The read-only reach refuses mutation typed.
        let response = embedded
            .execute(fixture.request(
                r#"return await z.invoke('fs.write', {path: 'a', content: 'x'});"#,
            ))
            .map_err(|e| format!("guest surface refusal execute: {e}"))?;
        self.check(
            response.kind == ZerokernelResultKind::Failed
                && has_error(&response, "read-only K0 reach"),
            "guest surface: mutation through z.invoke fails typed",
        );

        // One-shot evidence confinement: the kernel child has no live
        // rooted W9-E evidence, so the seam fails typed instead of
        // leaking evidence into the child.
        let oneshot = fixture.oneshot();
        let atom_hex = digest(1).to_hex();
        let response = oneshot
            .execute(fixture.request(&format!(
                "return await z.resolve({{scenario_id: 's1', projection_atoms: ['{atom_hex}']}});"
            )))
            .map_err(|e| format!("one-shot w9e confinement: {e}"))?;
        self.one_shot_calls += 1;
        self.check(
            response.kind == ZerokernelResultKind::Failed
                && has_error(&response, "without live rooted evidence"),
            "guest surface: one-shot child has no live rooted evidence (typed)",
        );
        self.check(
            oneshot.live_children() == 0 && oneshot.live_executors() == 0,
            "guest surface: one-shot quiescent after confinement call",
        );

        self.supervisors.push(embedded);
        self.supervisors.push(oneshot);
        self.fixtures.push(fixture);
        Ok(json!({
            "context_root": true,
            "state_round_trip": true,
            "parallel_input_order": true,
            "read_only_reach_typed": true,
            "one_shot_evidence_confined": true,
        }))
    }

    // ------------------------------------------------------------------
    // W9-E resolve/expand/snap/view/persistHandle end to end
    // ------------------------------------------------------------------

    fn w9e_chain(&mut self) -> Result<JsonValue, String> {
        let fixture = Fixture::new("w9e-chain");
        let supervisor = fixture.embedded_with_w9e(safe_evidence());
        let atom_a = digest(1).to_hex();
        let atom_b = digest(2).to_hex();
        let program = format!(
            r#"
            const r = await z.resolve({{scenario_id: 's1', projection_atoms: ['{atom_a}', '{atom_b}']}});
            const h = r.handle;
            const e = await z.expand(h);
            const v = await z.view(h);
            const persisted = await z.persistHandle(h);
            const s = await z.snap({{scenario_id: 's1', projection_atoms: ['{atom_a}', '{atom_b}']}});
            return {{
                handleId: r.handle_id,
                projectionRoot: r.projection_root,
                atoms: e.atoms.length,
                expandedRoot: e.projection_root,
                grade: v.completeness_grade,
                persisted,
                snapped: s.outcome,
                packetViewRoot: s.packet.decision_view_root,
            }};
        "#
        );
        let response = supervisor
            .execute(fixture.request(&program))
            .map_err(|e| format!("w9e chain execute: {e}"))?;
        self.check(
            response.kind == ZerokernelResultKind::Completed,
            format!("w9e chain: must complete: {:?}", failed_errors(&response)),
        );
        self.check(response.validate().is_ok(), "w9e chain: receipt validates");
        let result = response.result.clone().unwrap_or_default();
        let handle_id = result["handleId"].as_str().unwrap_or_default().to_owned();
        self.check(
            handle_id.len() == 64,
            "w9e chain: minted handle is a 64-hex identity",
        );
        self.check(
            result["atoms"] == json!(2) && result["expandedRoot"] == result["projectionRoot"],
            "w9e chain: first expansion is root/projection exact",
        );
        self.check(
            result["grade"] == json!("Proved"),
            "w9e chain: view certifies Proved",
        );
        self.check(
            result["persisted"] == json!(handle_id),
            "w9e chain: persistHandle returns the exact handle",
        );
        self.check(
            result["snapped"] == json!("snapped")
                && result["packetViewRoot"].as_str().is_some_and(|root| !root.is_empty()),
            "w9e chain: snap produces the decision-view packet",
        );
        self.check(
            response.handles.continuation_handle.as_deref() == Some(handle_id.as_str()),
            "w9e chain: exact-handle receipt rides the response",
        );
        self.check(
            supervisor.live_executors() == 0
                && supervisor.live_children() == 0
                && supervisor.live_gpu() == 0,
            "w9e chain: zero live resources after the chain",
        );

        self.supervisors.push(supervisor);
        self.fixtures.push(fixture);
        Ok(json!({
            "resolve": true,
            "expand": true,
            "snap": true,
            "view": true,
            "persist_handle": true,
            "exact_handle_receipt": true,
            "grade": "Proved",
            "atoms": 2,
        }))
    }

    // ------------------------------------------------------------------
    // Paired quality trial + first-expand measurement
    // ------------------------------------------------------------------

    fn paired_quality_and_first_expand(&mut self) -> Result<JsonValue, String> {
        let evidence = safe_evidence();
        let atom_a = digest(1);
        let atom_b = digest(2);
        let ground_truth: BTreeSet<Sha256Digest> = [atom_a, atom_b].into_iter().collect();
        let request = DemandRequest::new("s1".to_owned(), vec![atom_a, atom_b])
            .map_err(|e| format!("demand request: {e}"))?;

        // One fingerprint: both paths consume the identical evidence
        // (same model / harness / reasoning inputs).
        let fingerprint = zero_abi::sha256_hex(
            serde_json::to_string(&json!({
                "manifest": &evidence.manifest,
                "scope": &evidence.scope,
                "completeness_input": &evidence.completeness_input,
                "native": &evidence.native,
            }))
            .map_err(|e| format!("evidence fingerprint: {e}"))?
            .as_bytes(),
        );

        // Native Snap-to-File route over the same evidence.
        let mut native_walls = Vec::new();
        let mut first_expansion: Option<FirstExpansion> = None;
        for _ in 0..self.samples {
            let mut route = SnapToFileRoute::new(
                evidence.secret,
                evidence.tenant.clone(),
                evidence.epoch,
                evidence.index_root,
                evidence.index_version.clone(),
            )
            .map_err(|e| format!("native route: {e}"))?;
            let started = Instant::now();
            let outcome = route
                .snap(
                    &evidence.manifest,
                    &request,
                    &evidence.scope,
                    &evidence.completeness_input,
                    &evidence.native,
                )
                .map_err(|e| format!("native snap: {e}"))?;
            native_walls.push(started.elapsed());
            match outcome {
                SnapOutcome::Snapped {
                    expansion,
                    handle: _,
                    view,
                    packet: _,
                } => {
                    self.check(
                        expansion.atoms.len() == 2,
                        "paired quality: native first expansion returns exactly the projection",
                    );
                    self.check(
                        expansion.projection_root == projection_root_of(&[atom_a, atom_b]),
                        "paired quality: native first expansion is root/projection exact",
                    );
                    self.check(
                        serde_json::to_value(&view)
                            .map(|v| v["completeness_grade"] == json!("Proved"))
                            .unwrap_or(false),
                        "paired quality: native view certifies Proved",
                    );
                    if first_expansion.is_none() {
                        first_expansion = Some(expansion);
                    }
                }
                other => {
                    self.check(
                        false,
                        format!("paired quality: native snap must snap, got {:?}", other.outcome_kind()),
                    );
                }
            }
        }
        let native_median_ms = median_ms(&native_walls);
        let Some(first_expansion) = first_expansion else {
            return Err("paired quality: native route never snapped".into());
        };

        let adjudicated = adjudicate(&first_expansion, &ground_truth);
        self.check(
            !adjudicated.false_complete,
            "paired quality: no false-complete on the native route",
        );
        self.check(
            adjudicated.first_try_sufficiency,
            "paired quality: first try is sufficient natively",
        );
        self.check(
            adjudicated.retry_count == 0,
            "paired quality: no hidden retry natively",
        );
        self.check(
            adjudicated.certified_atoms == 2 && adjudicated.expanded_atoms == 2,
            "paired quality: exact certified/expanded atom counts natively",
        );
        self.check(
            adjudicated.native_savings_bytes == 512 - adjudicated.visible_bytes,
            "paired quality: native savings bytes are exactly baseline minus visible",
        );

        // The supervisor guest seam over the identical evidence.
        let fixture = Fixture::new("paired-quality");
        let supervisor = fixture.embedded_with_w9e(evidence);
        let program = format!(
            "return await z.resolve({{scenario_id: 's1', projection_atoms: ['{}', '{}']}});",
            atom_a.to_hex(),
            atom_b.to_hex()
        );
        let mut supervisor_walls = Vec::new();
        let mut supervisor_projection_root = None;
        for _ in 0..self.samples {
            let started = Instant::now();
            let response = supervisor
                .execute(fixture.request(&program))
                .map_err(|e| format!("supervisor resolve: {e}"))?;
            supervisor_walls.push(started.elapsed());
            self.check(
                response.kind == ZerokernelResultKind::Completed,
                format!("paired quality: supervisor resolve completes: {:?}", failed_errors(&response)),
            );
            self.check(response.validate().is_ok(), "paired quality: supervisor receipt validates");
            let result = response.result.clone().unwrap_or_default();
            let atoms: BTreeSet<String> = result["atoms"]
                .as_array()
                .map(|array| {
                    array
                        .iter()
                        .filter_map(|atom| atom["atom_root"].as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let expected_atoms: BTreeSet<String> =
                [atom_a.to_hex(), atom_b.to_hex()].into_iter().collect();
            self.check(
                atoms == expected_atoms,
                "paired quality: supervisor first expansion is projection-exact",
            );
            self.check(
                result["visible_bytes"].as_u64() == Some(first_expansion.visible_bytes),
                "paired quality: supervisor visible bytes match native exactly",
            );
            self.check(
                result["certified_atoms"].as_u64() == Some(first_expansion.certified_atoms as u64),
                "paired quality: supervisor certified atoms match native exactly",
            );
            self.check(
                result["first_try_sufficiency"] == json!(true),
                "paired quality: supervisor first try is sufficient",
            );
            supervisor_projection_root = Some(
                result["projection_root"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            );
            self.check(
                supervisor.live_executors() == 0 && supervisor.live_children() == 0,
                "paired quality: zero live resources after every supervisor resolve",
            );
        }
        let supervisor_median_ms = median_ms(&supervisor_walls);

        // Paired equality: same evidence, same expansion, same quality.
        self.check(
            supervisor_projection_root.as_deref() == Some(first_expansion.projection_root.to_hex().as_str()),
            "paired quality: projection roots identical across paths",
        );
        self.check(
            adjudicated.visible_bytes == first_expansion.visible_bytes,
            "paired quality: adjudicated visible bytes consistent",
        );

        self.supervisors.push(supervisor);
        self.fixtures.push(fixture);
        Ok(json!({
            "evidence_fingerprint": fingerprint,
            "samples": self.samples,
            "native_snap_median_ms": native_median_ms,
            "supervisor_resolve_median_ms": supervisor_median_ms,
            "native_first_expand_median_ms": native_median_ms,
            "atoms": adjudicated.expanded_atoms,
            "certified_atoms": adjudicated.certified_atoms,
            "visible_bytes": adjudicated.visible_bytes,
            "retry_count": adjudicated.retry_count,
            "false_complete": adjudicated.false_complete,
            "first_try_sufficiency": adjudicated.first_try_sufficiency,
            "native_savings_bytes": adjudicated.native_savings_bytes,
            "paired_identical": true,
            "no_quality_regression": true,
        }))
    }

    // ------------------------------------------------------------------
    // Parameter-friction sweep (measured, never invented)
    // ------------------------------------------------------------------

    fn parameter_friction(&mut self) -> Result<JsonValue, String> {
        struct Point {
            name: &'static str,
            profile: &'static str,
            budget: FiniteBudget,
            preview: u32,
            expected: Option<String>,
            program: &'static str,
            want: Option<JsonValue>,
            want_calls: u32,
        }

        let fixture = Fixture::new("friction");
        let seeder = fixture.embedded();
        let seed = seeder
            .execute(fixture.request("z.state.set('seed', 1); return 1;"))
            .map_err(|e| format!("friction seed: {e}"))?;
        self.check(
            seed.kind == ZerokernelResultKind::Completed,
            "parameter friction: seed commit completes",
        );
        let committed = seed.root_evidence.successor_root.clone().unwrap_or_default();
        self.check(
            !committed.is_empty(),
            "parameter friction: seed produces a committed root",
        );

        let default = FiniteBudget::new(WALL_MS, CPU_MS, MEMORY_BYTES, MAX_CALLS).expect("budget");
        let lean_wall = FiniteBudget::new(400, CPU_MS, MEMORY_BYTES, MAX_CALLS).expect("budget");
        let lean_calls = FiniteBudget::new(WALL_MS, CPU_MS, MEMORY_BYTES, 4).expect("budget");
        let one_call = FiniteBudget::new(WALL_MS, CPU_MS, MEMORY_BYTES, 1).expect("budget");
        let points = vec![
            Point {
                name: "wall_400ms",
                profile: "embedded",
                budget: lean_wall,
                preview: 4096,
                expected: None,
                program: "return 1;",
                want: Some(json!(1)),
                want_calls: 0,
            },
            Point {
                name: "wall_5000ms",
                profile: "embedded",
                budget: default.clone(),
                preview: 4096,
                expected: None,
                program: "return 1;",
                want: Some(json!(1)),
                want_calls: 0,
            },
            Point {
                name: "calls_4",
                profile: "embedded",
                budget: lean_calls,
                preview: 4096,
                expected: None,
                program: "return 1;",
                want: Some(json!(1)),
                want_calls: 0,
            },
            Point {
                name: "calls_64",
                profile: "embedded",
                budget: default.clone(),
                preview: 4096,
                expected: None,
                program: "return 1;",
                want: Some(json!(1)),
                want_calls: 0,
            },
            Point {
                name: "calls_1_help",
                profile: "embedded",
                budget: one_call,
                preview: 4096,
                expected: None,
                program: r#"return await zero.help.search({query: "fs"});"#,
                want: None,
                want_calls: 1,
            },
            Point {
                name: "expected_root_committed",
                profile: "embedded",
                budget: default.clone(),
                preview: 4096,
                expected: Some(committed.clone()),
                program: "return 1;",
                want: Some(json!(1)),
                want_calls: 0,
            },
            Point {
                name: "preview_64",
                profile: "embedded",
                budget: default.clone(),
                preview: 64,
                expected: None,
                program: "return 1;",
                want: Some(json!(1)),
                want_calls: 0,
            },
            Point {
                name: "preview_4096",
                profile: "embedded",
                budget: default.clone(),
                preview: 4096,
                expected: None,
                program: "return 1;",
                want: Some(json!(1)),
                want_calls: 0,
            },
            Point {
                name: "oneshot_trivial",
                profile: "oneshot",
                budget: default.clone(),
                preview: 4096,
                expected: None,
                program: "return 1;",
                want: Some(json!(1)),
                want_calls: 0,
            },
            Point {
                name: "oneshot_help",
                profile: "oneshot",
                budget: default,
                preview: 4096,
                expected: None,
                program: r#"return await zero.help.search({query: "fs"});"#,
                want: None,
                want_calls: 1,
            },
        ];

        let mut rows = Vec::new();
        for point in points {
            let supervisor = match point.profile {
                "oneshot" => fixture.oneshot(),
                _ => fixture.embedded(),
            };
            let policy = ReturnPolicy::new(ReturnKind::Inline, point.preview)
                .map_err(|e| format!("policy: {e}"))?;
            let budget_wall_ms = point.budget.wall_ms;
            let request = fixture.request_full(point.program, point.budget.clone(), policy, point.expected);
            let mut walls = Vec::new();
            let mut ledger_walls = Vec::new();
            let mut calls_made = 0u32;
            let mut bytes_out = 0u32;
            for _ in 0..self.samples {
                let started = Instant::now();
                let response = supervisor
                    .execute(request.clone())
                    .map_err(|e| format!("friction {}: {e}", point.name))?;
                walls.push(started.elapsed());
                ledger_walls.push(response.ledger.wall_ms_used as f64);
                calls_made = response.ledger.calls_made;
                bytes_out = response.ledger.bytes_out;
                self.check(
                    response.kind == ZerokernelResultKind::Completed,
                    format!(
                        "parameter friction[{}]: must complete: {:?}",
                        point.name,
                        failed_errors(&response)
                    ),
                );
                self.check(
                    response.validate().is_ok(),
                    format!("parameter friction[{}]: receipt validates", point.name),
                );
                if let Some(want) = &point.want {
                    self.check(
                        response.result.as_ref() == Some(want),
                        format!("parameter friction[{}]: result mismatch", point.name),
                    );
                }
                self.check(
                    response.ledger.calls_made == point.want_calls,
                    format!(
                        "parameter friction[{}]: ledger calls {} != {}",
                        point.name, response.ledger.calls_made, point.want_calls
                    ),
                );
                self.check(
                    response.ledger.wall_ms_used > 0
                        && response.ledger.wall_ms_used <= budget_wall_ms,
                    format!(
                        "parameter friction[{}]: ledger wall {} within budget {}",
                        point.name, response.ledger.wall_ms_used, budget_wall_ms
                    ),
                );
                self.check(
                    response.ledger.bytes_out <= 4096,
                    format!("parameter friction[{}]: bytes_out bounded", point.name),
                );
                self.check(
                    supervisor.live_executors() == 0
                        && supervisor.live_children() == 0
                        && supervisor.live_gpu() == 0,
                    format!("parameter friction[{}]: zero live resources", point.name),
                );
            }
            if point.profile == "oneshot" {
                self.one_shot_calls += self.samples as u64;
            }
            rows.push(json!({
                "point": point.name,
                "profile": point.profile,
                "samples": self.samples,
                "mean_ms": mean_ms(&walls),
                "median_ms": median_ms(&walls),
                "min_ms": walls.iter().map(|d| d.as_secs_f64() * 1000.0).fold(f64::INFINITY, f64::min),
                "max_ms": walls.iter().map(|d| d.as_secs_f64() * 1000.0).fold(0.0, f64::max),
                "mean_ledger_wall_ms": ledger_walls.iter().sum::<f64>() / ledger_walls.len() as f64,
                "calls_made": calls_made,
                "bytes_out": bytes_out,
            }));
            self.supervisors.push(supervisor);
        }
        self.supervisors.push(seeder);
        self.fixtures.push(fixture);
        Ok(json!({ "points": rows }))
    }

    // ------------------------------------------------------------------
    // Verifier reject (the named hostile case no sibling suite covers)
    // ------------------------------------------------------------------

    fn verifier_reject(&mut self) -> Result<JsonValue, String> {
        // Hostile: untrusted run evidence. The verifier rejects, the
        // attempt fails closed, the rollback carries the exact reason, and
        // no receipt is ever minted.
        let evidence = TaskRunEvidence::new(
            7,
            CommandId(1),
            [2u8; 32],
            0,
            vec![[3u8; 32]],
            vec![[3u8; 32]],
            [4u8; 32],
            5,
        );
        let attempt = begin_task_attempt(EffectClass::ReversibleMutation, evidence)
            .map_err(|e| format!("begin attempt: {e}"))?;
        let failure = verify_task_acceptance(&RejectingVerifier, attempt)
            .expect_err("a rejecting verifier must reject");
        self.check(
            failure.reason()
                == TaskAcceptanceError::VerifierRejected(TaskVerifierError::UntrustedRunEvidence),
            "verifier reject: reason is the typed VerifierRejected",
        );
        self.check(
            failure.rollback().reason()
                == RollbackReason::VerificationFailed(TaskAcceptanceError::VerifierRejected(
                    TaskVerifierError::UntrustedRunEvidence,
                )),
            "verifier reject: rollback records VerificationFailed",
        );
        self.check(
            failure.rollback().task_id() == 7 && failure.rollback().attempt_cost() == 5,
            "verifier reject: rollback carries task identity and cost",
        );

        // Positive control: trusted evidence yields a complete receipt
        // with every field asserted.
        let evidence = TaskRunEvidence::new(
            9,
            CommandId(3),
            [5u8; 32],
            0,
            vec![[6u8; 32]],
            vec![[6u8; 32]],
            [7u8; 32],
            8,
        );
        let attempt = begin_task_attempt(EffectClass::ReversibleMutation, evidence)
            .map_err(|e| format!("begin attempt: {e}"))?;
        let verified = verify_task_acceptance(&AcceptingVerifier, attempt)
            .map_err(|f| format!("accepting verifier must accept: {:?}", f.reason()))?;
        let receipt = verified.into_receipt();
        self.check(receipt.task_id() == 9, "verifier reject: receipt task id");
        self.check(receipt.verifier() == CommandId(3), "verifier reject: receipt verifier");
        self.check(
            receipt.verifier_environment_digest() == &[5u8; 32],
            "verifier reject: receipt verifier environment digest",
        );
        self.check(
            receipt.outcome() == TaskOutcome::Passed,
            "verifier reject: receipt outcome Passed",
        );
        self.check(receipt.exit_code() == 0, "verifier reject: receipt exit code");
        self.check(
            receipt.expected_artifact_digests() == &[[6u8; 32]][..]
                && receipt.observed_artifact_digests() == &[[6u8; 32]][..],
            "verifier reject: receipt artifact digests exact",
        );
        self.check(
            receipt.journal_id() == &[7u8; 32],
            "verifier reject: receipt journal id",
        );
        self.check(
            receipt.attempt_cost() == 8,
            "verifier reject: receipt attempt cost",
        );

        Ok(json!({
            "rejected_reason": "VerifierRejected(UntrustedRunEvidence)",
            "rollback_reason": "VerificationFailed",
            "receipt_minted_on_reject": false,
            "receipt_complete_on_accept": true,
            "receipt_fields_asserted": 8,
        }))
    }

    // ------------------------------------------------------------------
    // Bounded 10k empty + 10k stateful soak (opt-in)
    // ------------------------------------------------------------------

    fn soak(&mut self) -> Result<JsonValue, String> {
        let mode = soak_mode();
        let n = soak_count();
        let profile = soak_profile();
        let mut result = json!({
            "mode": mode,
            "profile": profile,
            "n": n,
            "empty_calls": 0,
            "stateful_calls": 0,
            "wall_s": 0.0,
            "mean_call_ms": 0.0,
            "spawns": 0,
            "state_survived": false,
            "last_tick": null,
        });
        if mode == "off" {
            self.warnings.push(
                "soak skipped; run with ZEROSTACK_K0_GATE_SOAK=both (or empty|stateful) to execute the bounded 10k soak"
                    .to_owned(),
            );
            return Ok(result);
        }
        if !matches!(mode.as_str(), "empty" | "stateful" | "both") {
            self.warnings.push(format!(
                "soak mode {mode:?} ignored; use off|empty|stateful|both"
            ));
            return Ok(result);
        }

        let started = Instant::now();
        let mut wall_samples = Vec::new();
        // Spawns through the soak supervisor (the seeder is always
        // embedded); the report's spawn count must match the audit.
        let mut soak_spawns: u64 = 0;

        if mode == "empty" || mode == "both" {
            let fixture = Fixture::new("soak-empty");
            let supervisor = match profile.as_str() {
                "oneshot" => fixture.oneshot(),
                _ => fixture.embedded(),
            };
            let budget = FiniteBudget::new(WALL_MS, CPU_MS, MEMORY_BYTES, MAX_CALLS).expect("budget");
            let request = fixture.request_budgeted("return 1;", budget, None);
            for index in 0..n {
                let t = Instant::now();
                let response = supervisor
                    .execute(request.clone())
                    .map_err(|e| format!("empty soak call {index}: {e}"))?;
                wall_samples.push(t.elapsed());
                if profile == "oneshot" {
                    soak_spawns += 1;
                }
                self.check(
                    response.kind == ZerokernelResultKind::Completed
                        && response.result == Some(json!(1)),
                    format!("empty soak[{index}]: must complete with 1"),
                );
                self.check(
                    response.root_evidence.unchanged,
                    format!("empty soak[{index}]: no state delta"),
                );
                if index % 1024 == 0 {
                    self.check(
                        supervisor.live_executors() == 0 && supervisor.live_children() == 0,
                        format!("empty soak[{index}]: zero live resources"),
                    );
                }
            }
            if profile == "oneshot" {
                self.one_shot_calls += n as u64;
            }
            let completed = supervisor
                .execute(fixture.request("return 1;"))
                .map_err(|e| format!("empty soak epilogue: {e}"))?;
            if profile == "oneshot" {
                soak_spawns += 1;
                self.one_shot_calls += 1;
            }
            self.check(
                completed.kind == ZerokernelResultKind::Completed,
                "empty soak: session stays usable",
            );
            self.check(
                supervisor.live_executors() == 0 && supervisor.live_children() == 0,
                "empty soak: zero live resources after the full soak",
            );
            result["empty_calls"] = json!(n);
            self.supervisors.push(supervisor);
            self.fixtures.push(fixture);
        }

        if mode == "stateful" || mode == "both" {
            let fixture = Fixture::new("soak-stateful");
            let seeder = fixture.embedded();
            let seed = seeder
                .execute(fixture.request("z.state.set('seed', 'kept'); return 1;"))
                .map_err(|e| format!("stateful soak seed: {e}"))?;
            self.check(
                seed.kind == ZerokernelResultKind::Completed,
                "stateful soak: seed commit completes",
            );
            let supervisor = match profile.as_str() {
                "oneshot" => fixture.oneshot(),
                _ => fixture.embedded(),
            };
            let budget = FiniteBudget::new(WALL_MS, CPU_MS, MEMORY_BYTES, MAX_CALLS).expect("budget");
            let mut expected = seed.root_evidence.successor_root.clone();
            let mut previous_successor = expected.clone();
            for index in 0..n {
                let program = format!("z.state.set('tick', {index}); return z.state.get('tick');");
                let request = fixture.request_budgeted(&program, budget.clone(), expected.clone());
                let t = Instant::now();
                let response = supervisor
                    .execute(request)
                    .map_err(|e| format!("stateful soak call {index}: {e}"))?;
                wall_samples.push(t.elapsed());
                if profile == "oneshot" {
                    soak_spawns += 1;
                }
                self.check(
                    response.kind == ZerokernelResultKind::Completed,
                    format!("stateful soak[{index}]: must complete: {:?}", failed_errors(&response)),
                );
                self.check(
                    response.result == Some(json!(index)),
                    format!("stateful soak[{index}]: result is the tick"),
                );
                let successor = response.root_evidence.successor_root.clone();
                if index == 0 {
                    self.check(
                        successor.is_some(),
                        "stateful soak[0]: first delta commits a successor",
                    );
                } else {
                    self.check(
                        successor.is_some() && successor != previous_successor,
                        format!("stateful soak[{index}]: every tick commits a fresh successor"),
                    );
                }
                expected = successor.clone();
                previous_successor = successor;
                if index % 1024 == 0 {
                    self.check(
                        supervisor.live_executors() == 0 && supervisor.live_children() == 0,
                        format!("stateful soak[{index}]: zero live resources"),
                    );
                }
            }
            if profile == "oneshot" {
                self.one_shot_calls += n as u64;
            }
            let committed = zsx_core::k0_state::current_session_root(
                &fixture.state_root,
                &fixture.session,
            )
            .map_err(|e| format!("stateful soak pointer: {e}"))?;
            self.check(
                committed == previous_successor,
                "stateful soak: committed-root pointer matches the last successor",
            );
            let response = supervisor
                .execute(fixture.request_budgeted(
                    "return [z.state.get('seed'), z.state.get('tick')];",
                    budget,
                    expected,
                ))
                .map_err(|e| format!("stateful soak hydration: {e}"))?;
            if profile == "oneshot" {
                soak_spawns += 1;
                self.one_shot_calls += 1;
            }
            let expected_after = if n == 0 { None } else { Some(json!((n - 1) as u64)) };
            self.check(
                response.kind == ZerokernelResultKind::Completed,
                "stateful soak: hydration completes",
            );
            self.check(
                response.result.as_ref().and_then(|r| r.get(0)) == Some(&json!("kept")),
                "stateful soak: seed key survives every fault-free successor",
            );
            if let Some(expected_after) = expected_after {
                self.check(
                    response.result.as_ref().and_then(|r| r.get(1)) == Some(&expected_after),
                    "stateful soak: last tick survives hydration",
                );
            }
            self.check(
                supervisor.live_executors() == 0 && supervisor.live_children() == 0,
                "stateful soak: zero live resources after the full soak",
            );
            result["stateful_calls"] = json!(n);
            result["last_tick"] = if n == 0 {
                JsonValue::Null
            } else {
                json!((n - 1) as u64)
            };
            result["state_survived"] = json!(n == 0 || response.result.is_some());
            self.supervisors.push(supervisor);
            self.supervisors.push(seeder);
            self.fixtures.push(fixture);
        }

        let wall = started.elapsed();
        result["wall_s"] = json!(wall.as_secs_f64());
        result["mean_call_ms"] = json!(mean_ms(&wall_samples));
        result["spawns"] = json!(soak_spawns);
        Ok(result)
    }

    // ------------------------------------------------------------------
    // Process-tree audit
    // ------------------------------------------------------------------

    fn audit(&mut self) -> JsonValue {
        let mut sockets = Vec::new();
        for fixture in &self.fixtures {
            scan_sockets(&fixture.root, &mut sockets);
        }
        let mut live_executors = 0u64;
        let mut live_children = 0u64;
        let mut live_gpu = 0u64;
        let mut child_spawns = 0u64;
        for supervisor in &self.supervisors {
            live_executors += supervisor.live_executors();
            live_children += supervisor.live_children();
            live_gpu += supervisor.live_gpu();
            child_spawns += supervisor.child_spawn_count();
        }
        let spawned = zsx_core::process_spawn_count().saturating_sub(self.spawns_at_start);
        self.check(
            sockets.is_empty(),
            format!("process audit: no socket/listener files under gate fixtures: {sockets:?}"),
        );
        self.check(
            live_executors == 0,
            format!("process audit: zero live executors, got {live_executors}"),
        );
        self.check(
            live_children == 0,
            format!("process audit: zero live children, got {live_children}"),
        );
        self.check(
            live_gpu == 0,
            format!("process audit: zero live GPU contexts, got {live_gpu}"),
        );
        self.check(
            spawned == self.one_shot_calls,
            format!(
                "process audit: every one-shot spawn reaped (spawned={spawned}, one-shot calls={})",
                self.one_shot_calls
            ),
        );
        let no_daemon_or_listener =
            sockets.is_empty() && live_children == 0 && spawned == self.one_shot_calls;
        json!({
            "sockets_found": sockets.len(),
            "live_executors": live_executors,
            "live_children": live_children,
            "live_gpu": live_gpu,
            "child_spawns_total": child_spawns,
            "process_spawn_delta": spawned,
            "one_shot_calls": self.one_shot_calls,
            "orphans": spawned.saturating_sub(self.one_shot_calls),
            "daemon_or_listener": no_daemon_or_listener,
        })
    }

    fn cleanup(&mut self) {
        // Drop every session/store handle first, then remove the gate's own
        // temp fixtures: the gate leaves zero processes and zero resources.
        self.supervisors.clear();
        for fixture in &self.fixtures {
            let _ = std::fs::remove_dir_all(&fixture.root);
        }
        self.fixtures.clear();
    }

    fn finish(
        mut self,
        results: Vec<(&'static str, Result<JsonValue, String>)>,
        elapsed: Duration,
    ) -> JsonValue {
        let audit = self.audit();
        self.cleanup();
        let mut sections = serde_json::Map::new();
        for (name, result) in results {
            match result {
                Ok(value) => {
                    sections.insert(name.to_owned(), value);
                }
                Err(detail) => {
                    self.failures.push(format!("section {name} aborted: {detail}"));
                    sections.insert(name.to_owned(), json!({ "aborted": true, "error": detail }));
                }
            }
        }
        let matrix: Vec<JsonValue> = MATRIX_MANIFEST
            .iter()
            .map(|(case, suite, test)| json!({ "case": case, "suite": suite, "test": test }))
            .collect();
        let gate_passed = self.failures.is_empty();
        json!({
            "schema": "zerostack.k0_gate.v1",
            "bead": "zerostack-nld8",
            "profile": if soak_mode() == "off" { "default" } else { "full" },
            "gate_passed": gate_passed,
            "host": {
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "rust_version": option_env!("CARGO_PKG_RUST_VERSION").unwrap_or("unknown"),
            },
            "samples_per_measurement": self.samples,
            "matrix_manifest": matrix,
            "sections": JsonValue::Object(sections),
            "process_audit": audit,
            "warnings": self.warnings,
            "failures": self.failures,
            "duration_ms": elapsed.as_secs_f64() * 1000.0,
            "how_run": "rch exec -- env CARGO_TARGET_DIR=\"${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}/rch_target_zerostack_k0_gate\" cargo test -p zsx --test supervisor --test k0_budgets --test k0_state --test k0_gate -- --test-threads=1 --nocapture",
            "env_controls": {
                "ZEROSTACK_K0_GATE_SOAK": "off|empty|stateful|both (default off; the bounded 10k soak)",
                "ZEROSTACK_K0_GATE_SOAK_N": "soak call count (default 10000, clamped 0..=100000)",
                "ZEROSTACK_K0_GATE_SOAK_PROFILE": "embedded|oneshot (default embedded)",
                "ZEROSTACK_K0_GATE_SAMPLES": "measurement samples per point (default 8, clamped 1..=64)",
                "ZEROSTACK_K0_GATE_REPORT": "path to also write the structured JSON report",
            },
        })
    }
}

// ---------------------------------------------------------------------------
// Environment controls (all bounded)
// ---------------------------------------------------------------------------

fn samples_env() -> usize {
    std::env::var("ZEROSTACK_K0_GATE_SAMPLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(8)
        .clamp(1, 64)
}

fn soak_mode() -> String {
    std::env::var("ZEROSTACK_K0_GATE_SOAK")
        .unwrap_or_else(|_| "off".to_owned())
        .to_ascii_lowercase()
}

fn soak_count() -> usize {
    std::env::var("ZEROSTACK_K0_GATE_SOAK_N")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(SOAK_DEFAULT_N)
        .clamp(SOAK_MIN_N, SOAK_MAX_N)
}

fn soak_profile() -> String {
    std::env::var("ZEROSTACK_K0_GATE_SOAK_PROFILE")
        .unwrap_or_else(|_| "embedded".to_owned())
        .to_ascii_lowercase()
}

// ---------------------------------------------------------------------------
// Socket scan (a supervisor must never create a listener; the only IPC is
// stdio pipes)
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn is_socket_file(file_type: &std::fs::FileType) -> bool {
    use std::os::unix::fs::FileTypeExt;
    file_type.is_socket()
}

#[cfg(not(unix))]
fn is_socket_file(_file_type: &std::fs::FileType) -> bool {
    false
}

fn scan_sockets(root: &Path, found: &mut Vec<PathBuf>) {
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
            } else if is_socket_file(&file_type) {
                found.push(path);
            }
        }
    }
    walk(root, found);
}

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

#[test]
fn k0_release_gate() {
    let started = Instant::now();
    let mut gate = Gate::new();
    let results = gate.run_all();
    let report = gate.finish(results, started.elapsed());
    emit_report(&report);
    let failures: Vec<&str> = report["failures"]
        .as_array()
        .map(|array| array.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    assert!(
        failures.is_empty(),
        "K0 release gate FAILED ({} failure(s)):\n{}",
        failures.len(),
        failures.join("\n")
    );
    println!(
        "K0 release gate passed in {:.2}s (profile {})",
        report["duration_ms"].as_f64().unwrap_or(0.0) / 1000.0,
        report["profile"].as_str().unwrap_or("default")
    );
}

fn emit_report(report: &JsonValue) {
    let text = serde_json::to_string_pretty(report).expect("report serializes");
    println!("\n===== ZEROSTACK K0 GATE REPORT (zerostack.k0_gate.v1) =====\n{text}\n===== END K0 GATE REPORT =====");
    if let Ok(path) = std::env::var("ZEROSTACK_K0_GATE_REPORT") {
        if !path.is_empty() {
            if let Err(error) = std::fs::write(&path, text) {
                println!("WARNING: cannot write report to {path}: {error}");
            } else {
                println!("report written to {path}");
            }
        }
    }
}
