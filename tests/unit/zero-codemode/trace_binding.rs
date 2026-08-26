use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use zero_abi::{
    CapabilityDescriptor, GlobalRegistration, KernelLedger, StateEvidence, ZERO_KERNEL_PROTOCOL,
    ZeroHandle, ZeroKernelOutcome, ZeroKernelResponse, ZeroOperationStatus, ZeroOperationTrace,
};
use zero_codemode::{
    Connector, ConnectorCompletion, ConnectorError, DispatchContext, GuestContext, GuestSurface,
    Host, HostLimits,
};

/// Synchronous connector: every dispatch completes immediately with a
/// JSON-encoded string, so both operations settle before the plan returns
/// and the trace is fully bound at dispatch start.
struct ImmediateConnector;

impl Connector for ImmediateConnector {
    fn dispatch(
        &self,
        _capability: &CapabilityDescriptor,
        _args_json: &str,
        _context: DispatchContext,
        completion: ConnectorCompletion,
    ) -> Result<(), ConnectorError> {
        completion.complete(Ok(r#""ok""#.into()))
    }
}

fn canonical_root(fill: char) -> String {
    fill.to_string().repeat(64)
}

fn host_with(capsule_root: &str) -> (Host, Arc<GuestSurface>) {
    let limits = HostLimits::new(
        8 * 1024 * 1024,
        256 * 1024,
        Duration::from_secs(1),
        10_000,
        1,
        2,
        4,
        4 * 1024,
        1024 * 1024,
    )
    .unwrap();
    let registration = GlobalRegistration {
        root: "z".into(),
        capabilities: vec![CapabilityDescriptor::new("z", "read")],
    };
    let guest = Arc::new(GuestSurface::new(GuestContext {
        project_root: "/project".into(),
        workspace_root: Some("/project".into()),
        request_root: Some("/project".into()),
        session_root: None,
        session_id: "trace-binding".into(),
        protocol: "ZeroKernel".into(),
        capsule_root: capsule_root.into(),
    }));
    let host = Host::new_zero_kernel(limits, registration)
        .unwrap()
        .with_guest_surface(Arc::clone(&guest));
    (host, guest)
}

/// Two sequential guest dispatches; both are bound at dispatch start.
const TWO_READS: &str = r#"
    const first = await z.read("first");
    const second = await z.read("second");
    return [first, second];
"#;
/// One synchronous state write plus a guest read; the state operation
/// consumes a sequence from the same allocator as connector calls.
const STATE_AND_READ: &str = r#"
    z.state.set("seen", true);
    const first = await z.read("first");
    return first;
"#;

fn completed_response(operations: Vec<ZeroOperationTrace>) -> ZeroKernelResponse {
    ZeroKernelResponse {
        protocol: ZERO_KERNEL_PROTOCOL.into(),
        outcome: ZeroKernelOutcome::Completed,
        value: Some(json!({"ok": true})),
        error: None,
        operations,
        operations_truncated: false,
        handles: vec![],
        event: ZeroHandle::from_digest(&canonical_root('e')).unwrap(),
        state: StateEvidence {
            before: None,
            after: None,
            unchanged: true,
        },
        ledger: KernelLedger::default(),
        turn: None,
    }
}

#[test]
fn guest_traces_bind_same_canonical_capsule_root_at_dispatch_start() {
    let root = canonical_root('c');
    let (host, guest) = host_with(&root);

    assert_eq!(guest.capsule_root(), root);
    assert!(
        guest.context_json().get("capsuleRoot").is_none(),
        "capsule root is not model-facing"
    );

    let outcome = host.execute_measured(TWO_READS, Rc::new(ImmediateConnector));
    assert_eq!(outcome.result.expect("plan executes"), json!(["ok", "ok"]));

    let operations = &outcome.operations;
    assert_eq!(
        operations.len(),
        2,
        "two guest dispatches trace two operations"
    );
    for (index, operation) in operations.iter().enumerate() {
        assert_eq!(
            operation.capsule_root,
            root,
            "trace {} must be bound to the installed capsule root",
            index + 1
        );
        assert_eq!(operation.sequence, (index + 1) as u64);
        assert_eq!(
            operation.occurrence, operation.sequence,
            "occurrence is the positive monotonic dispatch sequence"
        );
        assert_eq!(operation.status, ZeroOperationStatus::Completed);
    }

    // The traces bound at dispatch start form a valid ZeroKernel response;
    // nothing stamps or rewrites them after execution.
    completed_response(outcome.operations.clone())
        .validate()
        .expect("bound traces validate as a ZeroKernel response");
}

#[test]
fn empty_or_malformed_guest_capsule_never_yields_a_valid_zero_kernel_trace() {
    for bad_root in [String::new(), "not-a-root".into(), "A".repeat(64)] {
        let (host, guest) = host_with(&bad_root);
        assert_eq!(guest.capsule_root(), bad_root);

        let outcome = host.execute_measured(STATE_AND_READ, Rc::new(ImmediateConnector));
        outcome.result.expect("plan executes");
        assert!(!outcome.operations.is_empty());

        for operation in &outcome.operations {
            assert_eq!(
                operation.capsule_root, bad_root,
                "trace carries the installed root verbatim; no root is ever synthesized"
            );
        }
        assert!(
            completed_response(outcome.operations).validate().is_err(),
            "no ZeroKernel response may validate a trace bound to {bad_root:?}"
        );
    }
}

#[test]
fn state_operations_share_the_connector_sequence_allocator() {
    let root = canonical_root('b');
    let (host, _) = host_with(&root);

    let outcome = host.execute_measured(
        r#"
        const first = await z.read("first");
        const key = z.state.get("key");
        const second = await z.read(first);
        z.state.set("seen", true);
        return [first, second, key];
        "#,
        Rc::new(ImmediateConnector),
    );
    outcome.result.expect("plan executes");

    let operations = &outcome.operations;
    let methods: Vec<&str> = operations
        .iter()
        .map(|operation| operation.method.as_str())
        .collect();
    assert_eq!(methods, ["read", "state", "read", "state"]);
    for (index, operation) in operations.iter().enumerate() {
        let sequence = (index + 1) as u64;
        assert_eq!(operation.capsule_root, root);
        assert_eq!(operation.sequence, sequence);
        assert_eq!(operation.occurrence, sequence);
        assert_eq!(operation.status, ZeroOperationStatus::Completed);
    }
    // Synchronous state consumption sits inside the connector sequence from
    // the same allocator: no later promise reuses a state sequence, and the
    // target is the state key.
    assert_eq!(operations[1].target.as_deref(), Some("key"));
    assert_eq!(operations[3].target.as_deref(), Some("seen"));

    completed_response(outcome.operations.clone())
        .validate()
        .expect("interleaved traces validate");
}

#[test]
fn failed_state_invocation_still_completes_its_trace() {
    let root = canonical_root('f');
    let (host, _) = host_with(&root);

    let outcome = host.execute_measured("return z.state.get(42);", Rc::new(ImmediateConnector));
    assert!(outcome.result.is_err(), "non-string key must fail");

    assert_eq!(outcome.operations.len(), 1);
    let operation = &outcome.operations[0];
    assert_eq!(operation.method, "state");
    assert_eq!(operation.status, ZeroOperationStatus::Failed);
    assert_eq!(operation.capsule_root, root);
    assert_eq!(operation.sequence, 1);
    assert_eq!(operation.occurrence, 1);
    assert_eq!(operation.target.as_deref(), Some("get"));
    assert!(
        operation
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("string key")),
        "trace detail must describe the failure: {:?}",
        operation.detail
    );
}
