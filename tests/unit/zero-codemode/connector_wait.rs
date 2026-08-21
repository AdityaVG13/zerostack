use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use zero_abi::{CapabilityDescriptor, GlobalRegistration};
use zero_codemode::{
    Connector, ConnectorCompletion, ConnectorError, DispatchContext, GuestContext, GuestSurface,
    Host, HostLimits,
};

struct DelayedConnector;

impl Connector for DelayedConnector {
    fn dispatch(
        &self,
        _capability: &CapabilityDescriptor,
        _args_json: &str,
        _context: DispatchContext,
        completion: ConnectorCompletion,
    ) -> Result<(), ConnectorError> {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(75));
            completion
                .complete(Ok(serde_json::to_string("ok").unwrap()))
                .unwrap();
        });
        Ok(())
    }
}

#[test]
fn idle_connector_wait_does_not_consume_microtask_budget() {
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
    let guest = Arc::new(GuestSurface::new(
        GuestContext {
            project_root: "/project".into(),
            workspace_root: Some("/project".into()),
            request_root: Some("/project".into()),
            session_root: None,
            session_id: "connector-wait".into(),
            protocol: "ZeroKernel".into(),
        },
        2,
    ));
    let host = Host::new_zero_kernel(limits, registration)
        .unwrap()
        .with_guest_surface(guest);

    let result = host
        .execute(
            r#"
            const first = await z.read("first").then(value => value);
            return await z.read(first).then(value => value);
            "#,
            Rc::new(DelayedConnector),
        )
        .unwrap();
    assert_eq!(result, json!("ok"));
}
