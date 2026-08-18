//! K0 capability broker preflight boundary (zerostack-pvwg).
//!
//! Exercises the broker's parse / resolve / normalize / inject / validate
//! boundary: the structural-repair corpus succeeds in one call, the
//! semantic-ambiguity corpus never auto-selects (typed DecisionRequired),
//! receipts bind the injected roots and stale/mismatched usage fails
//! closed, pointed-at external read paths are preserved through explicit
//! grants only, and an already-correct plan passes through unmodified
//! (the direct path is preserved — this suite never rewrites a program).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use zero_abi::zerokernel::{
    FiniteBudget, ReturnKind, ReturnPolicy, RootBindings, ZerokernelExecuteRequest,
};
use zsx_core::preflight::{
    BrokerOutcome, CAPABILITY_MANIFEST_SCHEMA, CAPABILITY_MANIFEST_VERSION,
    OBSERVATION_CLASS_CAPABILITY_RESOLVE, broker, scan_plan,
};

const WALL_MS: u64 = 5_000;
const CPU_MS: u64 = 5_000;
const SESSION: &str = "k0-test-session";

static DIR_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One isolated canonical workspace root plus an external (out-of-root)
/// directory, both under one removable base.
struct Fixture {
    base: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let sequence = DIR_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!(
            "zerostack-k0-preflight-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(base.join("workspace")).unwrap();
        fs::create_dir_all(base.join("external")).unwrap();
        let root = base.join("workspace").canonicalize().unwrap();
        Self { base, root }
    }

    fn external_file(&self) -> PathBuf {
        let path = self.base.join("external").join("granted.txt");
        fs::write(&path, "external").unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn request(program: &str, root: &Path, manifest: Option<&Path>) -> ZerokernelExecuteRequest {
    let root_text = root.to_string_lossy().into_owned();
    ZerokernelExecuteRequest::new(
        program.into(),
        Some(SESSION.into()),
        FiniteBudget::new(WALL_MS, CPU_MS, 64 * 1024 * 1024, 64).expect("budget"),
        ReturnPolicy::new(ReturnKind::Inline, 4096).expect("policy"),
        RootBindings::new(
            Some(root_text.clone()),
            root_text,
            None,
            manifest.map(|path| path.to_string_lossy().into_owned()),
            None,
        )
        .expect("roots"),
    )
    .expect("request")
}

fn broker_for(program: &str, root: &Path) -> BrokerOutcome {
    broker(&request(program, root, None), root, SESSION)
}

#[test]
fn scan_ignores_comments_and_strings_and_finds_qualified_calls() {
    let plan = r#"
        // zero.fs.read({path: "commented"})
        const note = "zero.fs.write({path: 'in-string'})";
        return await zero.fs.read({path: 'README.md', raw: true});
    "#;
    let scan = scan_plan(plan);
    assert_eq!(scan.mentions.len(), 1, "mentions={:?}", scan.mentions);
    let mention = &scan.mentions[0];
    assert!(mention.qualified);
    assert_eq!(mention.surface, "fs");
    assert_eq!(mention.method, "read");
    assert_eq!(
        mention
            .object_keys
            .iter()
            .find(|key| key.key == "path")
            .and_then(|key| key.single.clone()),
        Some("README.md".into())
    );
    assert!(!scan.opaque);
}

#[test]
fn already_correct_plan_proceeds_without_rewrite() {
    let fixture = Fixture::new();
    let program = "return await zero.fs.compound('list', {path: '.'});";
    match broker_for(program, &fixture.root) {
        BrokerOutcome::Proceed(receipt) => {
            let lines = receipt.warning_lines();
            assert!(
                lines.iter().any(|line| line.contains("k0: injected")),
                "receipt must carry injected context: {lines:?}"
            );
            assert!(
                lines
                    .iter()
                    .any(|line| line.contains("k0: repair fs.compound('list') resolves to fs.ls")),
                "receipt must record the approved compound fold: {lines:?}"
            );
            assert!(
                lines.iter().any(|line| line.contains("version=zerokernel")),
                "receipt must carry the ABI version: {lines:?}"
            );
            assert!(
                lines.iter().any(|line| line.contains("deadline=5000ms")),
                "receipt must carry the budget deadline: {lines:?}"
            );
            assert!(
                lines.iter().any(|line| line.contains("authority=")),
                "receipt must carry the authority digest: {lines:?}"
            );
            assert!(
                lines
                    .iter()
                    .any(|line| line.contains("resolved 1 capability mention")),
                "receipt must record resolution: {lines:?}"
            );
        }
        other => panic!("already-correct plan must proceed, got {other:?}"),
    }
}

#[test]
fn structural_repair_corpus_proceeds_in_one_call() {
    let fixture = Fixture::new();
    let corpus = [
        "return await zero.fs.read('README.md');",
        "return await zero.fs.compound('search', {query: 'foo', path: '.'});",
        "return await zero.fs.compound('grep', {regex: 'fn main', path: '.'});",
        "return await zero.token.shell({command: 'pwd', timeout_seconds: 5});",
        "return await zero.token.shell({command: 'pwd', timeoutMs: 30000});",
        "return await zero.fs.lookup({path: '.', pattern: '*.rs'});",
        "return await zero.fs.multi_read(['a.md', 'b.md']);",
        "return await zero.graph.query('symbol', 'main');",
        "return await zero.help.search({query: 'read'});",
        "return await zero.fs.structural('fn main');",
    ];
    for program in corpus {
        match broker_for(program, &fixture.root) {
            BrokerOutcome::Proceed(receipt) => {
                assert!(
                    receipt
                        .warning_lines()
                        .iter()
                        .any(|line| line.starts_with("k0: injected")),
                    "plan {program:?} must carry the injected-context receipt line"
                );
            }
            other => panic!("plan {program:?} must proceed, got {other:?}"),
        }
    }
}

#[test]
fn semantic_ambiguity_never_auto_selects() {
    let fixture = Fixture::new();

    // camelCase method spelling with one close candidate: still a typed
    // decision, never a silent correction.
    let outcome = broker_for(
        "return await zero.fs.readGrant({path: '/tmp/x'});",
        &fixture.root,
    );
    match outcome {
        BrokerOutcome::DecisionRequired(decision) => {
            assert_eq!(
                decision.observation_class.class_id,
                OBSERVATION_CLASS_CAPABILITY_RESOLVE
            );
            assert_eq!(decision.observed_value, "readGrant");
            assert!(
                decision.choices.contains(&"zero.fs.read_grant".to_owned()),
                "choices={:?}",
                decision.choices
            );
        }
        other => panic!("readGrant must be DecisionRequired, got {other:?}"),
    }

    // Typo with close candidates: neither candidate is auto-selected.
    let outcome = broker_for("return await zero.fs.rede({path: 'x'});", &fixture.root);
    match outcome {
        BrokerOutcome::DecisionRequired(decision) => {
            assert_eq!(decision.observed_value, "rede");
            assert!(
                decision.choices.contains(&"zero.fs.read".to_owned()),
                "choices={:?}",
                decision.choices
            );
        }
        other => panic!("rede must be DecisionRequired, got {other:?}"),
    }

    // Unknown surface with candidates.
    let outcome = broker_for("return await zero.fzz.read({path: 'x'});", &fixture.root);
    match outcome {
        BrokerOutcome::DecisionRequired(decision) => {
            assert_eq!(decision.observed_value, "fzz");
            assert!(
                decision.choices.contains(&"fs".to_owned()),
                "choices={:?}",
                decision.choices
            );
        }
        other => panic!("fzz surface must be DecisionRequired, got {other:?}"),
    }

    // Unknown compound operation with close candidates.
    let outcome = broker_for("return await zero.fs.compound('reade', {});", &fixture.root);
    match outcome {
        BrokerOutcome::DecisionRequired(decision) => {
            assert_eq!(decision.observed_value, "reade");
            assert!(
                decision.choices.contains(&"read".to_owned()),
                "choices={:?}",
                decision.choices
            );
        }
        other => panic!("compound('reade') must be DecisionRequired, got {other:?}"),
    }
}

#[test]
fn fs_write_fails_closed_without_approval_grants() {
    let fixture = Fixture::new();
    for program in [
        "return await zero.fs.write({path: 'a.txt', content: 'x'});",
        "return await zero.fs.compound('write', {path: 'a.txt', content: 'x'});",
    ] {
        match broker_for(program, &fixture.root) {
            BrokerOutcome::Refused(detail) => {
                assert!(detail.contains("fs.write"), "detail: {detail}");
                assert!(detail.contains("approval"), "detail: {detail}");
            }
            other => panic!("plan {program:?} must be refused, got {other:?}"),
        }
    }
}

#[test]
fn z_guest_surface_is_refused_not_auto_mapped() {
    let fixture = Fixture::new();
    for program in [
        "return await z.invoke('fs.read', {path: 'a'});",
        "return await z.return(1);",
    ] {
        match broker_for(program, &fixture.root) {
            BrokerOutcome::Refused(detail) => {
                assert!(
                    detail.contains("not part of the V6 surface"),
                    "plan {program:?} detail: {detail}"
                );
            }
            other => panic!("plan {program:?} must be refused, got {other:?}"),
        }
    }
}

#[test]
fn unqualified_capability_call_is_refused_with_fix() {
    let fixture = Fixture::new();
    match broker_for("return await fs.read({path: 'a'});", &fixture.root) {
        BrokerOutcome::Refused(detail) => {
            assert!(
                detail.contains("unqualified capability call fs.read"),
                "detail: {detail}"
            );
            assert!(detail.contains("zero.fs.read"), "detail: {detail}");
        }
        other => panic!("expected Refused, got {other:?}"),
    }
}

#[test]
fn shadowed_surface_names_are_not_mentions() {
    let fixture = Fixture::new();
    // An arrow parameter named fs must not turn a later use into a false
    // unqualified mention.
    match broker_for(
        "return await zero.fs.read({path: 'a'}).then((fs) => fs);",
        &fixture.root,
    ) {
        BrokerOutcome::Proceed(_) => {}
        other => panic!("expected Proceed, got {other:?}"),
    }
    // A const binding shadows the surface name.
    match broker_for("const fs = 1; return fs;", &fixture.root) {
        BrokerOutcome::Proceed(_) => {}
        other => panic!("expected Proceed, got {other:?}"),
    }
}

#[test]
fn conflicting_timeout_spellings_fail_closed() {
    let fixture = Fixture::new();
    match broker_for(
        "return await zero.token.shell({command: 'pwd', timeoutMs: 1000, timeout_ms: 2000});",
        &fixture.root,
    ) {
        BrokerOutcome::Refused(detail) => {
            assert!(
                detail.contains("must not include both 'timeoutMs' and 'timeout_ms'"),
                "detail: {detail}"
            );
        }
        other => panic!("expected Refused, got {other:?}"),
    }
}

#[test]
fn intrinsic_decision_surface_is_registered() {
    let fixture = Fixture::new();
    // zero.decision.require is intrinsic to the interpreter; the broker
    // must resolve it, not refuse or ask about it.
    match broker_for(
        r#"return await zero.decision.require(
            {decision_id: "d1", observation_class: {class_id: "branch.choice"},
             question: "which branch?", alternatives: ["left", "right"], evidence_refs: []},
            "left");"#,
        &fixture.root,
    ) {
        BrokerOutcome::Proceed(receipt) => {
            assert!(
                receipt
                    .warning_lines()
                    .iter()
                    .any(|line| line.contains("resolved 1 capability mention")),
                "receipt={:?}",
                receipt.warning_lines()
            );
        }
        other => panic!("decision.require must proceed, got {other:?}"),
    }
}

#[test]
fn external_read_requires_explicit_grant() {
    let fixture = Fixture::new();
    let external = fixture.external_file();
    let external_text = external.to_string_lossy().into_owned();

    // Read without a mint: fail closed with the corrective shape.
    let program = format!("return await zero.fs.read({{path: '{external_text}'}});");
    match broker_for(&program, &fixture.root) {
        BrokerOutcome::Refused(detail) => {
            assert!(detail.contains("requires an explicit grant"), "detail: {detail}");
            assert!(detail.contains("read_grant"), "detail: {detail}");
        }
        other => panic!("expected Refused, got {other:?}"),
    }

    // Mint then read: preserved through the explicit grant.
    let program = format!(
        "await zero.fs.read_grant({{path: '{external_text}'}}); return await zero.fs.read({{path: '{external_text}'}});"
    );
    match broker_for(&program, &fixture.root) {
        BrokerOutcome::Proceed(_) => {}
        other => panic!("expected Proceed, got {other:?}"),
    }

    // Compound mint and compound read forms.
    let program = format!(
        "await zero.fs.compound('readGrant', {{path: '{external_text}'}}); return await zero.fs.compound('read', {{path: '{external_text}'}});"
    );
    match broker_for(&program, &fixture.root) {
        BrokerOutcome::Proceed(_) => {}
        other => panic!("expected Proceed, got {other:?}"),
    }

    // multi_read external array without a mint fails closed.
    let program = format!("return await zero.fs.multi_read(['{external_text}']);");
    match broker_for(&program, &fixture.root) {
        BrokerOutcome::Refused(detail) => {
            assert!(detail.contains("requires an explicit grant"), "detail: {detail}");
        }
        other => panic!("expected Refused, got {other:?}"),
    }

    // An in-root absolute read needs no grant.
    let inside = fixture.root.join("inside.txt");
    fs::write(&inside, "inside").unwrap();
    let inside_text = inside.to_string_lossy().into_owned();
    let program = format!("return await zero.fs.read({{path: '{inside_text}'}});");
    match broker_for(&program, &fixture.root) {
        BrokerOutcome::Proceed(_) => {}
        other => panic!("expected Proceed, got {other:?}"),
    }
}

#[test]
fn capability_manifest_gates_plan_capabilities() {
    let fixture = Fixture::new();
    let manifest_path = fixture.root.join("capability-manifest.json");

    // Granted capabilities proceed.
    fs::write(
        &manifest_path,
        format!(
            r#"{{
            "schema": "{CAPABILITY_MANIFEST_SCHEMA}",
            "version": {CAPABILITY_MANIFEST_VERSION},
            "capabilities": ["fs.read", "fs.compound", "fs.read_grant"]
        }}"#
        ),
    )
    .unwrap();
    let req = request(
        "return await zero.fs.read({path: 'README.md'});",
        &fixture.root,
        Some(&manifest_path),
    );
    match broker(&req, &fixture.root, SESSION) {
        BrokerOutcome::Proceed(_) => {}
        other => panic!("granted capability must proceed, got {other:?}"),
    }

    // Ungranted capability fails closed.
    let req = request(
        "return await zero.graph.query('symbol', 'main');",
        &fixture.root,
        Some(&manifest_path),
    );
    match broker(&req, &fixture.root, SESSION) {
        BrokerOutcome::Refused(detail) => {
            assert!(
                detail.contains("not granted by the capability manifest"),
                "detail: {detail}"
            );
        }
        other => panic!("ungranted capability must be refused, got {other:?}"),
    }

    // A manifest naming an unregistered capability is defective.
    fs::write(
        &manifest_path,
        format!(
            r#"{{
            "schema": "{CAPABILITY_MANIFEST_SCHEMA}",
            "version": {CAPABILITY_MANIFEST_VERSION},
            "capabilities": ["fs.read", "fs.ls"]
        }}"#
        ),
    )
    .unwrap();
    let req = request(
        "return await zero.fs.read({path: 'README.md'});",
        &fixture.root,
        Some(&manifest_path),
    );
    match broker(&req, &fixture.root, SESSION) {
        BrokerOutcome::Refused(detail) => {
            assert!(
                detail.contains("not a registered V6 capability"),
                "detail: {detail}"
            );
        }
        other => panic!("defective manifest must be refused, got {other:?}"),
    }

    // Unknown schema fails closed.
    fs::write(
        &manifest_path,
        format!(
            r#"{{
            "schema": "zerostack.k0.capability_manifest.v999",
            "version": {CAPABILITY_MANIFEST_VERSION},
            "capabilities": ["fs.read"]
        }}"#
        ),
    )
    .unwrap();
    let req = request(
        "return await zero.fs.read({path: 'README.md'});",
        &fixture.root,
        Some(&manifest_path),
    );
    match broker(&req, &fixture.root, SESSION) {
        BrokerOutcome::Refused(detail) => {
            assert!(detail.contains("not usable"), "detail: {detail}");
        }
        other => panic!("unknown schema must be refused, got {other:?}"),
    }

    // A missing manifest root fails closed.
    let missing = fixture.root.join("missing-manifest.json");
    let req = request(
        "return await zero.fs.read({path: 'README.md'});",
        &fixture.root,
        Some(&missing),
    );
    match broker(&req, &fixture.root, SESSION) {
        BrokerOutcome::Refused(detail) => {
            assert!(detail.contains("not usable"), "detail: {detail}");
        }
        other => panic!("missing manifest must be refused, got {other:?}"),
    }
}

#[test]
fn opaque_scan_with_manifest_fails_closed_and_without_proceeds() {
    let fixture = Fixture::new();
    let manifest_path = fixture.root.join("capability-manifest.json");
    fs::write(
        &manifest_path,
        format!(
            r#"{{
            "schema": "{CAPABILITY_MANIFEST_SCHEMA}",
            "version": {CAPABILITY_MANIFEST_VERSION},
            "capabilities": ["fs.read"]
        }}"#
        ),
    )
    .unwrap();

    // Computed member access cannot be certified against a manifest.
    let req = request(
        "const op = 'read'; return await zero.fs[op]({path: 'README.md'});",
        &fixture.root,
        Some(&manifest_path),
    );
    match broker(&req, &fixture.root, SESSION) {
        BrokerOutcome::Refused(detail) => {
            assert!(detail.contains("cannot certify"), "detail: {detail}");
        }
        other => panic!("opaque scan with manifest must be refused, got {other:?}"),
    }

    // Without a manifest the same plan proceeds under runtime enforcement
    // and the receipt records the scan gap.
    match broker_for(
        "const op = 'read'; return await zero.fs[op]({path: 'README.md'});",
        &fixture.root,
    ) {
        BrokerOutcome::Proceed(receipt) => {
            assert!(
                receipt
                    .warning_lines()
                    .iter()
                    .any(|line| line.contains("k0: scan opaque")),
                "receipt must record the scan gap: {:?}",
                receipt.warning_lines()
            );
        }
        other => panic!("opaque scan without manifest must proceed, got {other:?}"),
    }
}

/// End-to-end: the embedded supervisor runs the broker boundary inside one
/// execute call — structural repair completes with the receipt bound,
/// semantic ambiguity returns a typed decision, and structural refusals
/// fail closed with unchanged roots.
#[cfg(all(feature = "fszero", feature = "graphzero", feature = "tokenzero"))]
#[test]
fn supervisor_binds_receipt_and_terminals_in_one_call() {
    use zsx_core::supervisor::{Supervisor, SupervisorProfile};

    let fixture = Fixture::new();
    let root = fixture.root.clone();
    let state_root = root.join(".zerostack");
    fs::create_dir_all(&state_root).unwrap();
    let session = format!("k0-supervisor-{}", std::process::id());
    let supervisor = Supervisor::builder(root.clone())
        .with_state_root(state_root.clone())
        .with_session_id(session.clone())
        .with_profile(SupervisorProfile::Embedded)
        .build_canonical()
        .expect("embedded supervisor builds");
    let root_text = root.to_string_lossy().into_owned();
    let state_text = state_root.to_string_lossy().into_owned();
    let make_request = |program: &str| {
        ZerokernelExecuteRequest::new(
            program.into(),
            Some(session.clone()),
            FiniteBudget::new(WALL_MS, CPU_MS, 64 * 1024 * 1024, 64).expect("budget"),
            ReturnPolicy::new(ReturnKind::Inline, 4096).expect("policy"),
            RootBindings::new(
                Some(root_text.clone()),
                root_text.clone(),
                None,
                None,
                Some(state_text.clone()),
            )
            .expect("roots"),
        )
        .expect("request")
    };

    // Structural repair completes in one call; the receipt binds the
    // injected roots, version, deadline and authority.
    let response = supervisor
        .execute(make_request("return await zero.fs.compound('list', {path: '.'});"))
        .expect("execute");
    assert_eq!(response.kind, ZerokernelResultKind::Completed);
    assert!(response.root_evidence.unchanged);
    assert!(
        response
            .preflight
            .checked_roots
            .iter()
            .any(|checked| checked == &root_text),
        "checked_roots={:?}",
        response.preflight.checked_roots
    );
    let joined = response.preflight.warnings.join("\n");
    assert!(joined.contains("k0: injected"), "receipt: {joined}");
    assert!(joined.contains("version=zerokernel"), "receipt: {joined}");
    assert!(joined.contains("authority="), "receipt: {joined}");
    assert!(joined.contains("resolved 1 capability mention"), "receipt: {joined}");

    // Semantic ambiguity never auto-selects: typed DecisionRequired.
    let response = supervisor
        .execute(make_request("return await zero.fs.readGrant({path: '/tmp/x'});"))
        .expect("execute");
    assert_eq!(response.kind, ZerokernelResultKind::DecisionRequired);
    let decision = response.decision.expect("decision payload");
    assert_eq!(
        decision.observation_class.class_id,
        OBSERVATION_CLASS_CAPABILITY_RESOLVE
    );
    assert_eq!(decision.observed_value, "readGrant");
    assert!(response.root_evidence.unchanged);

    // Structural refusal fails closed in the same call with unchanged
    // roots, before any execution.
    let response = supervisor
        .execute(make_request("return await zero.fs.write({path: 'a.txt', content: 'x'});"))
        .expect("execute");
    assert_eq!(response.kind, ZerokernelResultKind::Failed);
    assert!(response.root_evidence.unchanged);
    assert!(
        response
            .preflight
            .errors
            .iter()
            .any(|error| error.contains("approval-required")),
        "errors={:?}",
        response.preflight.errors
    );
    assert_eq!(supervisor.live_executors(), 0);
    assert!(!root.join("a.txt").exists(), "read-only protocol must not write");
}
