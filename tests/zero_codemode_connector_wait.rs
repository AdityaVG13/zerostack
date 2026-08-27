use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use serde_json::json;
use zero_abi::{CapabilityDescriptor, GlobalRegistration};
use zero_codemode::{
    Connector, ConnectorCompletion, ConnectorError, DispatchContext, GuestContext, GuestSurface,
    Host, HostLimits,
};

#[test]
fn top_level_literal_reads_are_prefetched_concurrently() {
    struct State {
        count: usize,
        arrivals: Vec<String>,
    }
    struct BarrierConnector {
        state: Arc<Mutex<State>>,
        cvar: Arc<Condvar>,
        threads: Arc<Mutex<Vec<std::thread::JoinHandle<()>>>>,
    }
    impl Connector for BarrierConnector {
        fn dispatch(
            &self,
            _capability: &CapabilityDescriptor,
            args_json: &str,
            _context: DispatchContext,
            completion: ConnectorCompletion,
        ) -> Result<(), ConnectorError> {
            let state = Arc::clone(&self.state);
            let cvar = Arc::clone(&self.cvar);
            let args: Vec<String> =
                serde_json::from_str(args_json).expect("connector arguments must be a JSON array");
            assert_eq!(args.len(), 1, "read accepts exactly one argument here");
            let arg = args.into_iter().next().unwrap();
            let handle = std::thread::spawn(move || {
                {
                    let mut guard = state.lock();
                    guard.arrivals.push(arg);
                    guard.count += 1;
                    cvar.notify_all();
                }
                let mut guard = state.lock();
                let deadline = Instant::now() + Duration::from_secs(5);
                while guard.count < 2 {
                    let now = Instant::now();
                    if now >= deadline {
                        panic!(
                            "deadlock: second concurrent read never arrived; arrivals={:?}",
                            guard.arrivals
                        );
                    }
                    let remaining = deadline - now;
                    let timeout = cvar.wait_for(&mut guard, remaining);
                    if timeout.timed_out() && guard.count < 2 {
                        panic!(
                            "deadlock timeout waiting for second concurrent read; arrivals={:?}",
                            guard.arrivals
                        );
                    }
                }
                drop(guard);
                completion
                    .complete(Ok(serde_json::to_string("ok").unwrap()))
                    .unwrap();
            });
            self.threads.lock().push(handle);
            Ok(())
        }
    }
    let limits = HostLimits::new(
        8 * 1024 * 1024,
        256 * 1024,
        Duration::from_secs(5),
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
        session_id: "prefetch".into(),
        protocol: "ZeroKernel".into(),
        capsule_root: "a".repeat(64),
    }));
    let host = Host::new_zero_kernel(limits, registration)
        .unwrap()
        .with_guest_surface(guest);
    let state = Arc::new(Mutex::new(State {
        count: 0,
        arrivals: Vec::new(),
    }));
    let cvar = Arc::new(Condvar::new());
    let threads = Arc::new(Mutex::new(Vec::new()));
    let connector = Rc::new(BarrierConnector {
        state: Arc::clone(&state),
        cvar: Arc::clone(&cvar),
        threads: Arc::clone(&threads),
    });
    let result = host
        .execute(
            r#"
            const [first, second] = await Promise.all([
              z.read("first"),
              z.read("second"),
            ]);
            return [first, second];
            "#,
            connector,
        )
        .unwrap();
    for thread in threads.lock().drain(..) {
        thread.join().expect("connector thread must finish cleanly");
    }
    assert_eq!(result, json!(["ok", "ok"]));
    let guard = state.lock();
    assert_eq!(
        guard.arrivals.len(),
        2,
        "both reads must have been dispatched"
    );
    let mut arrivals = guard.arrivals.clone();
    arrivals.sort();
    assert_eq!(arrivals, vec!["first".to_string(), "second".to_string()]);
}

#[test]
fn later_statement_is_not_prefetched_before_first_failure() {
    struct RejectingConnector(Arc<AtomicUsize>);
    impl Connector for RejectingConnector {
        fn dispatch(
            &self,
            _capability: &CapabilityDescriptor,
            _args_json: &str,
            _context: DispatchContext,
            completion: ConnectorCompletion,
        ) -> Result<(), ConnectorError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            completion.complete(Err(ConnectorError::new("first read failed")))
        }
    }

    let limits = HostLimits::new(
        8 * 1024 * 1024,
        256 * 1024,
        Duration::from_secs(5),
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
        session_id: "prefetch-failure".into(),
        protocol: "ZeroKernel".into(),
        capsule_root: "a".repeat(64),
    }));
    let host = Host::new_zero_kernel(limits, registration)
        .unwrap()
        .with_guest_surface(guest);
    let dispatches = Arc::new(AtomicUsize::new(0));
    let error = host
        .execute(
            r#"
            const first = await z.read("first");
            const second = await z.read("second");
            return [first, second];
            "#,
            Rc::new(RejectingConnector(Arc::clone(&dispatches))),
        )
        .unwrap_err();
    assert!(error.to_string().contains("first read failed"), "{error}");
    assert_eq!(
        dispatches.load(Ordering::SeqCst),
        1,
        "a later statement must not launch before the first statement succeeds"
    );
}
