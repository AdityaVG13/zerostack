//! K0 read-only guest `z` surface contract (`zerostack-fhcj`).
//!
//! Exercises the guest surface inside the real supervisor runtime (fresh
//! bounded runtime per call, real engine adapters):
//! - generated fixtures run ordinary top-level-await JS against `z.context`
//!   and the small serializable `z.state` map;
//! - `z.parallel` preserves deterministic input-order results and enforces
//!   the fan-out bound; every spec must be a registered read-only
//!   capability;
//! - forbidden APIs fail typed (`z.transaction`, effect/invoke targets,
//!   unknown members, forged handles);
//! - `z.resolve`/`z.expand`/`z.snap`/`z.view` cannot operate without live
//!   rooted evidence, and with evidence run the real W9-E chain and persist
//!   the exact handle into the response;
//! - runtime teardown leaves no tasks: zero live executors/children after
//!   every call, and the session stays usable.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use serde_json::{Value as JsonValue, json};
use zero_abi::Sha256Digest;
use zero_abi::zerokernel::{
    FiniteBudget, ReturnKind, ReturnPolicy, RootBindings, ZerokernelExecuteRequest,
    ZerokernelResultKind,
};
use zero_gate::project_image::{
    CausalGraphRef, DemandScenario, ExactObject, PerObjectLayers, ProofGraphRef,
    ProjectImageManifest, ShadowResourceLedger,
};
use zero_gate::{CoverageAtom, GraphZeroCompletenessInput, NativeBaseline, ProtectedScope};
use zsx_core::guest_w9e::W9eEvidence;
use zsx_core::supervisor::{Supervisor, SupervisorProfile};

const WALL_MS: u64 = 10_000;
const CPU_MS: u64 = 10_000;

fn unique_root(label: &str) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    for _ in 0..100 {
        let candidate = std::env::temp_dir().join(format!(
            "zerostack-guest-{label}-{}-{}-{:x}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
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
            session: format!("guest-test-{}-{label}", std::process::id()),
            root,
            state_root,
        }
    }

    fn request(&self, program: &str) -> ZerokernelExecuteRequest {
        let root_text = self.root.to_string_lossy().into_owned();
        // No expected session root: the call hydrates from the committed
        // root (none on a fresh session) and commits unconditionally.
        ZerokernelExecuteRequest::new(
            program.into(),
            Some(self.session.clone()),
            FiniteBudget::new(WALL_MS, CPU_MS, 64 * 1024 * 1024, 64).expect("budget"),
            ReturnPolicy::new(ReturnKind::Inline, 4096).expect("policy"),
            RootBindings::new(Some(root_text.clone()), root_text, None, None, None)
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
}

// ---------------------------------------------------------------------------
// W9-E fixture helpers (mirroring the snap-to-file corpus shapes)
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
    ProtectedScope::new("guest-test-scope".to_owned(), vec![]).unwrap()
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
    let index_version = "guest-iv-1".to_owned();
    W9eEvidence::new(
        [7u8; 32],
        "guest-tenant".to_owned(),
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
            "guest-task-1".to_owned(),
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

fn demand_program(member: &str, atom_hex: &str) -> String {
    format!(
        "return await z.{member}({{scenario_id: 's1', projection_atoms: ['{atom_hex}']}});"
    )
}

// ---------------------------------------------------------------------------
// Context and small serializable state
// ---------------------------------------------------------------------------

#[test]
fn guest_context_and_state_round_trip() {
    let fixture = Fixture::new("context-state");
    let supervisor = fixture.embedded();
    let program = r#"
        z.state.set('a', {n: 1, s: 'x'});
        z.state.set('b', [true, null]);
        const has = z.state.has('a');
        const got = z.state.get('a').n;
        const keys = z.state.list();
        const del = z.state.delete('a');
        const after = z.state.has('a');
        const kept = z.state.get('b')[1];
        const ctx = z.context;
        return {
            has, got, keys, del, after, kept,
            projectRoot: ctx.projectRoot,
            sessionRoot: ctx.sessionRoot,
            workspaceRoot: ctx.workspaceRoot,
            manifestRoot: ctx.capabilityManifestRoot,
        };
    "#;
    let response = supervisor
        .execute(fixture.request(program))
        .expect("guest plan executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", response.preflight.errors);
    let result = response.result.expect("completed result");
    assert_eq!(result["has"], json!(true));
    assert_eq!(result["got"], json!(1));
    assert_eq!(result["keys"], json!(["a", "b"]));
    assert_eq!(result["del"], json!(true));
    assert_eq!(result["after"], json!(false));
    assert_eq!(result["kept"], json!(null));
    assert_eq!(
        result["projectRoot"],
        json!(fixture.root.to_string_lossy().into_owned())
    );
    // A fresh session with no committed state and no expected root has no
    // session state root; the guest sees it as absent.
    assert_eq!(result["sessionRoot"], JsonValue::Null);
    assert_eq!(result["workspaceRoot"], result["projectRoot"]);
    assert_eq!(result["manifestRoot"], JsonValue::Null);
    assert_eq!(supervisor.live_executors(), 0);
    assert_eq!(supervisor.live_children(), 0);
}

#[test]
fn guest_state_is_bounded_and_typed() {
    let fixture = Fixture::new("state-bounds");
    let supervisor = fixture.embedded();
    // 65 distinct keys exceeds the 64-key bound.
    let program = r#"
        for (let i = 0; i < 65; i++) { z.state.set('k' + i, i); }
        return z.state.list().length;
    "#;
    let response = supervisor
        .execute(fixture.request(program))
        .expect("bounded state plan executes");
    assert_eq!(response.kind, ZerokernelResultKind::Failed, "result={:?}", response.result);
    assert!(
        response
            .preflight
            .errors
            .iter()
            .any(|error| error.contains("64-key bound")),
        "errors={:?}",
        response.preflight.errors
    );
    // The failed call never committed, so a fresh call (no expected root,
    // nothing committed) still sees an empty map.
    let response = supervisor
        .execute(fixture.request("return z.state.list();"))
        .expect("fresh runtime executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed);
    assert_eq!(response.result, Some(json!([])));
    // A value above the per-value bound fails typed.
    let big = "x".repeat(4097);
    let response = supervisor
        .execute(fixture.request(&format!(
            "z.state.set('big', '{big}'); return 1;"
        )))
        .expect("oversized state plan executes");
    assert_eq!(response.kind, ZerokernelResultKind::Failed);
    assert!(
        response
            .preflight
            .errors
            .iter()
            .any(|error| error.contains("per-value bound")),
        "errors={:?}",
        response.preflight.errors
    );
}

// ---------------------------------------------------------------------------
// Deterministic parallel ordering and limits
// ---------------------------------------------------------------------------

#[test]
fn parallel_preserves_input_order_and_reports_limits() {
    let fixture = Fixture::new("parallel");
    let supervisor = fixture.embedded();
    let program = r#"
        const out = await z.parallel([
            {surface: 'help', method: 'search', args: {query: 'fs.lookup'}},
            {surface: 'help', method: 'search', args: {query: 'fs.read_grant'}},
            'help.catalog',
        ]);
        return [
            out[0].content.value.results[0].path,
            out[1].content.value.results[0].path,
            out[2].content.value.operation,
        ];
    "#;
    let response = supervisor
        .execute(fixture.request(program))
        .expect("parallel plan executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", response.preflight.errors);
    assert_eq!(
        response.result,
        Some(json!(["fs.lookup", "fs.read_grant", "help.search"]))
    );
    // The fan-out bound: 17 specs fails typed before any dispatch.
    let mut specs = String::new();
    for i in 0..17 {
        if i > 0 {
            specs.push(',');
        }
        specs.push_str("'help.catalog'");
    }
    let response = supervisor
        .execute(fixture.request(&format!("return await z.parallel([{specs}]);")))
        .expect("over-limit parallel plan executes");
    assert_eq!(response.kind, ZerokernelResultKind::Failed);
    assert!(
        response
            .preflight
            .errors
            .iter()
            .any(|error| error.contains("at most 16")),
        "errors={:?}",
        response.preflight.errors
    );
    // A mutation spec in object form (invisible to the broker scan) still
    // fails typed at the runtime surface before any dispatch.
    let response = supervisor
        .execute(fixture.request(
            "return await z.parallel([{surface: 'fs', method: 'write', args: {path: 'x', content: 'y'}}]);",
        ))
        .expect("mutating parallel plan executes");
    assert_eq!(response.kind, ZerokernelResultKind::Failed);
    assert!(
        response
            .preflight
            .errors
            .iter()
            .any(|error| error.contains("read-only K0 reach")),
        "errors={:?}",
        response.preflight.errors
    );
    assert_eq!(supervisor.live_executors(), 0);
    assert_eq!(supervisor.live_children(), 0);
}

#[test]
fn parallel_determinism_across_calls() {
    let fixture = Fixture::new("parallel-determinism");
    let supervisor = fixture.embedded();
    let program = r#"
        const out = await z.parallel([
            {surface: 'help', method: 'search', args: {query: 'fs.lookup'}},
            {surface: 'help', method: 'search', args: {query: 'fs.read_grant'}},
        ]);
        return [out[0].content.value.results[0].path, out[1].content.value.results[0].path];
    "#;
    let first = supervisor
        .execute(fixture.request(program))
        .expect("first parallel plan executes");
    let second = supervisor
        .execute(fixture.request(program))
        .expect("second parallel plan executes");
    assert_eq!(first.kind, ZerokernelResultKind::Completed);
    assert_eq!(first.result, second.result);
    assert_eq!(
        first.result,
        Some(json!(["fs.lookup", "fs.read_grant"]))
    );
    assert_eq!(supervisor.live_executors(), 0);
}

// ---------------------------------------------------------------------------
// Typed forbidden APIs
// ---------------------------------------------------------------------------

#[test]
fn forbidden_apis_fail_typed() {
    let fixture = Fixture::new("forbidden");
    let supervisor = fixture.embedded();
    for (program, needle) in [
        (
            "return await z.transaction(async tx => tx);",
            "no effect or transaction authority",
        ),
        (
            "return await z.invoke('fs.write', {path: 'a', content: 'x'});",
            "read-only K0 reach",
        ),
        (
            "return await z.invoke('token.shell', {command: 'echo hi'});",
            "read-only K0 reach",
        ),
        (
            "return await z.invoke('fs.compound', {name: 'write', path: 'a', content: 'x'});",
            "read-only K0 reach",
        ),
        (
            "return await z.parallel(['fs.write']);",
            "read-only K0 reach",
        ),
        (
            "return await z.state.brew('k', 1);",
            "z.state.brew is not part of the K0 guest surface",
        ),
        (
            "return await z.definitelyNotAMember();",
            "not part of the K0 guest surface",
        ),
        (
            "return await z.persistHandle({handle_id: 'ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff'});",
            "not a host-minted handle",
        ),
    ] {
        let response = supervisor
            .execute(fixture.request(program))
            .expect("forbidden plan executes as a protocol response");
        assert_eq!(response.kind, ZerokernelResultKind::Failed, "plan {program:?} result={:?}", response.result);
        assert!(
            response
                .preflight
                .errors
                .iter()
                .any(|error| error.contains(needle)),
            "plan {program:?} errors={:?}",
            response.preflight.errors
        );
        assert!(response.root_evidence.unchanged);
        assert_eq!(supervisor.live_executors(), 0);
    }
}

// ---------------------------------------------------------------------------
// Live rooted evidence requirement
// ---------------------------------------------------------------------------

#[test]
fn wave9_requires_live_rooted_evidence() {
    let fixture = Fixture::new("no-evidence");
    let supervisor = fixture.embedded();
    let atom_hex = digest(1).to_hex();
    for member in ["resolve", "expand", "snap", "view"] {
        let response = supervisor
            .execute(fixture.request(&demand_program(member, &atom_hex)))
            .expect("wave-9 plan executes as a protocol response");
        assert_eq!(response.kind, ZerokernelResultKind::Failed, "member {member} result={:?}", response.result);
        assert!(
            response
                .preflight
                .errors
                .iter()
                .any(|error| error.contains("without live rooted evidence")),
            "member {member} errors={:?}",
            response.preflight.errors
        );
        assert_eq!(supervisor.live_executors(), 0);
    }
}

#[test]
fn wave9_safe_chain_with_live_rooted_evidence() {
    let fixture = Fixture::new("evidence");
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
        .expect("wave-9 chain plan executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", response.preflight.errors);
    let result = response.result.expect("completed result");
    let handle_id = result["handleId"].as_str().expect("handle id").to_owned();
    assert_eq!(handle_id.len(), 64);
    assert_eq!(result["atoms"], json!(2));
    assert_eq!(result["expandedRoot"], result["projectionRoot"]);
    assert_eq!(result["grade"], json!("Proved"));
    assert_eq!(result["persisted"], json!(handle_id));
    assert_eq!(result["snapped"], json!("snapped"));
    assert!(
        result["packetViewRoot"].as_str().is_some_and(|root| !root.is_empty()),
        "packet must carry a decision view root"
    );
    // Exact-handle persistence: the minted handle id rides the response's
    // exact-handle slot.
    assert_eq!(
        response.handles.continuation_handle.as_deref(),
        Some(handle_id.as_str())
    );
    assert_eq!(supervisor.live_executors(), 0);
    assert_eq!(supervisor.live_children(), 0);
}

#[test]
fn wave9_unsafe_demand_refuses_without_minting() {
    let fixture = Fixture::new("unsafe-demand");
    let supervisor = fixture.embedded_with_w9e(safe_evidence());
    // An atom outside the certified scenario envelope makes the demand
    // refuse: typed reasons, no handle, no atoms, nothing persisted.
    let outside = digest(3).to_hex();
    let program = format!(
        "return await z.resolve({{scenario_id: 's1', projection_atoms: ['{outside}']}});"
    );
    let response = supervisor
        .execute(fixture.request(&program))
        .expect("unsafe demand plan executes");
    assert_eq!(response.kind, ZerokernelResultKind::Failed);
    assert!(
        response
            .preflight
            .errors
            .iter()
            .any(|error| error.contains("z.resolve")),
        "errors={:?}",
        response.preflight.errors
    );
    assert_eq!(response.handles.continuation_handle, None);
    assert_eq!(supervisor.live_executors(), 0);
}

// ---------------------------------------------------------------------------
// Teardown and ordinary top-level-await JS
// ---------------------------------------------------------------------------

#[test]
fn teardown_leaves_no_tasks_and_session_stays_usable() {
    let fixture = Fixture::new("teardown");
    let supervisor = fixture.embedded();
    let calls = [
        "return await z.parallel(['help.catalog']);",
        "return await z.transaction(async tx => tx);",
        "return z.state.set('k', 1) && z.state.get('k');",
        "return await z.invoke('fs.write', {path: 'a', content: 'x'});",
        "return Promise.all([z.help(), z.inspect()]);",
    ];
    for (index, program) in calls.iter().enumerate() {
        let response = supervisor
            .execute(fixture.request(program))
            .expect("teardown plan executes as a protocol response");
        assert!(
            matches!(response.kind, ZerokernelResultKind::Completed | ZerokernelResultKind::Failed),
            "call {index} kind={:?}",
            response.kind
        );
        // Zero live executors/children after every call: no task leak by
        // construction, and the embedded profile never spawns.
        assert_eq!(supervisor.live_executors(), 0, "call {index}");
        assert_eq!(supervisor.live_children(), 0, "call {index}");
        assert_eq!(zsx_core::process_spawn_count(), 0, "call {index}");
    }
    // The session survives: a fresh call still works after failures.
    let response = supervisor
        .execute(fixture.request("return 42;"))
        .expect("session reuse executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed);
    assert_eq!(response.result, Some(json!(42)));
    assert_eq!(supervisor.live_executors(), 0);
}

#[test]
fn ordinary_top_level_await_js_still_runs() {
    let fixture = Fixture::new("plain-js");
    let supervisor = fixture.embedded();
    let program = r#"
        const [a, b] = await Promise.all([
            zero.help.search({query: 'fs.lookup'}),
            zero.help.search({query: 'fs.read_grant'}),
        ]);
        return [a.content.value.results[0].path, b.content.value.results[0].path];
    "#;
    let response = supervisor
        .execute(fixture.request(program))
        .expect("plain plan executes");
    assert_eq!(response.kind, ZerokernelResultKind::Completed, "errors={:?}", response.preflight.errors);
    assert_eq!(response.result, Some(json!(["fs.lookup", "fs.read_grant"])));
    assert_eq!(supervisor.live_executors(), 0);
}
