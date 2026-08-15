#![cfg(feature = "fixture-adapters")]

//! ZS-BASE-001 acceptance: a conformance task that disables ZeroStack
//! mid-run completes through the same native tool path. The fixture adapter
//! serving the fs engine goes away after its first call; the plan's later
//! steps (including a loud adapter-refusal handled by the plan) complete
//! through native interpreter operations only, and the session stays
//! healthy for subsequent requests -- native path, not adapter path.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use zero_abi::EngineIdentity;
use zsx_core::{
    AdapterBinding, AdapterCall, AdapterError, AdapterResponse, DomainAdapter,
    SessionEnvelopeContextV1, ZsxSession,
    fixture::{FixtureAdapter, fixture_adapters},
};

/// A fixture adapter that serves exactly `budget` calls and then goes away
/// (engine unavailable) for the rest of the session.
struct DisablingAdapter {
    inner: Arc<FixtureAdapter>,
    remaining: AtomicUsize,
}

impl DisablingAdapter {
    fn new(inner: Arc<FixtureAdapter>, budget: usize) -> Self {
        Self {
            inner,
            remaining: AtomicUsize::new(budget),
        }
    }
}

impl DomainAdapter for DisablingAdapter {
    fn engine(&self) -> EngineIdentity {
        self.inner.engine()
    }

    fn binding(&self) -> AdapterBinding {
        self.inner.binding()
    }

    fn call(&self, call: AdapterCall<'_>) -> Result<AdapterResponse, AdapterError> {
        if self.remaining.load(Ordering::Relaxed) == 0 {
            return Err(AdapterError::new(
                "substrate",
                "ZeroStack disabled mid-run: fixture fs adapter is gone",
                false,
                Some(call.request.trace.clone()),
            ));
        }
        self.remaining.fetch_sub(1, Ordering::Relaxed);
        self.inner.call(call)
    }
}

fn fixture_session(budget: usize) -> (tempfile::TempDir, ZsxSession, Arc<DisablingAdapter>) {
    let root = tempfile::tempdir().expect("root");
    let root_path = root.path().canonicalize().expect("canonical root");
    let (fs, graph, token) = fixture_adapters(&root_path, "native-path");
    let disabling = Arc::new(DisablingAdapter::new(fs, budget));
    let session = ZsxSession::builder(&root_path)
        .with_session_id("native-path")
        .fszero(disabling.clone())
        .graphzero(graph.clone())
        .tokenzero(token.clone())
        .build()
        .expect("session");
    (root, session, disabling)
}

const PLAN: &str = r#"
const first = await zero.fs.multi_read({paths:["a.txt"]});
let phase = "adapter-alive";
try {
  await zero.fs.multi_read({paths:["b.txt"]});
  phase = "adapter-still-alive";
} catch (e) {
  phase = "native-continued:" + e.name;
}
const n = first.content.value.value.args.paths.paths.length + 41;
const tag = "native:" + ("ok".toUpperCase());
return {phase, n, tag, firstPath: first.content.value.value.args.paths.paths[0]};
"#;

#[test]
fn task_disables_zerostack_mid_run_and_completes_through_the_native_path() {
    let (_root, session, fs_adapter) = fixture_session(1);
    let result = session
        .execute(1, 1, PLAN, Duration::from_secs(10))
        .expect("the plan completes through the native path after the adapter dies");
    let value = result
        .value
        .as_object()
        .expect("plan returns an object")
        .clone();
    assert_eq!(value["firstPath"], serde_json::json!("a.txt"));
    assert_eq!(value["n"], serde_json::json!(42), "native arithmetic completed");
    assert_eq!(value["tag"], serde_json::json!("native:OK"));
    let phase = value["phase"].as_str().expect("phase string");
    assert!(
        phase.starts_with("native-continued:"),
        "the mid-run adapter failure must surface loudly and be handled by the plan, got {phase}"
    );
    // The adapter really was disabled: it served exactly one call.
    assert_eq!(fs_adapter.remaining.load(Ordering::Relaxed), 0);
    assert!(fs_adapter.inner.calls() >= 1);

    // The session stays healthy: a second native-only request completes and
    // the V6 surface reports a plain success without inventing a kind.
    let second = session
        .execute(1, 2, "return {still: 'native'};".to_string(), Duration::from_secs(10))
        .expect("session survives the disabled adapter");
    assert_eq!(second.value["still"], serde_json::json!("native"));

    let ledger = SessionEnvelopeContextV1::new(
        "a".repeat(64),
        zero_abi::AuditEventRangeV1::new(1, 1).unwrap(),
    )
    .expect("ledger");
    let v6 = session
        .execute_v6(
            1,
            3,
            "return 7 * 6;".to_string(),
            Duration::from_secs(10),
            ledger,
        )
        .expect("v6 execute completes through the native path");
    assert_eq!(v6.value, Some(serde_json::json!(42)));
    assert!(
        v6.envelope.is_none(),
        "a plain native success must not claim a V6 kind it cannot prove"
    );
    session.shutdown().expect("shutdown");
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[test]
fn complete_refusal_without_native_fallback_fails_loudly() {
    // Negative control: when the plan has NO native fallback and keeps
    // calling the disabled adapter, the session fails loudly instead of
    // silently returning a partial result.
    let (_root, session, _fs_adapter) = fixture_session(0);
    let error = session
        .execute(
            1,
            1,
            "return await zero.fs.multi_read({paths:['x']});".to_string(),
            Duration::from_secs(10),
        )
        .expect_err("an unhandled disabled-adapter call must fail");
    assert!(
        matches!(
            error.code,
            zsx_core::ZsxSessionFailureCode::BackendExecution
        ),
        "disabled adapter surfaces as a backend execution failure, got {:?}",
        error.code
    );
    let _ = now_ms();
    session.shutdown().expect("shutdown");
}
