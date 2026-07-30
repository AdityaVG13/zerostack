use std::time::Duration;
#[cfg(feature = "quickjs")]
use std::time::Instant;

#[cfg(feature = "quickjs")]
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
};

#[cfg(feature = "quickjs")]
use serde_json::{json, Value};
#[cfg(feature = "quickjs")]
use zero_codemode::{runtime_creation_count, Connector, ConnectorError, DispatchContext};
use zero_codemode::{CANONICAL_REF_ALIASES, CANONICAL_RESULT_FIELDS, CANONICAL_TEXT_ALIASES};

use zero_codemode::{
    wrap_plan, CapabilityDescriptor, GlobalRegistration, Host, HostError, HostLimits, PlanError,
    RegistrationError,
};
#[cfg(feature = "quickjs")]
use zero_codemode::{RESULT_SPILL_PREVIEW_BYTES, RESULT_SPILL_SCHEMA};
#[cfg(feature = "quickjs")]
use zero_store::SharedCas;

#[cfg(feature = "quickjs")]
struct C {
    calls: RefCell<Vec<Value>>,
    fail: bool,
    delay: Duration,
    result: Option<String>,
}

#[cfg(feature = "quickjs")]
impl C {
    fn ok() -> Self {
        Self {
            calls: RefCell::new(vec![]),
            fail: false,
            delay: Duration::ZERO,
            result: None,
        }
    }
}

#[cfg(feature = "quickjs")]
impl Connector for C {
    fn call(
        &self,
        _: &CapabilityDescriptor,
        args_json: &str,
        context: DispatchContext,
    ) -> Result<String, ConnectorError> {
        assert!(context.max_json_bytes > 0);
        let args: Value = serde_json::from_str(args_json)
            .map_err(|error| ConnectorError::new(error.to_string()))?;
        self.calls.borrow_mut().push(args.clone());
        if !self.delay.is_zero() {
            thread::sleep(self.delay);
        }
        if self.fail {
            Err(ConnectorError::new("connector refused request"))
        } else if let Some(result) = &self.result {
            Ok(result.clone())
        } else {
            serde_json::to_string(&json!({ "echo": args }))
                .map_err(|error| ConnectorError::new(error.to_string()))
        }
    }
}

fn reg() -> GlobalRegistration {
    GlobalRegistration::zero(vec![CapabilityDescriptor::new("fs", "read")])
}

fn lim() -> HostLimits {
    HostLimits::new(
        16 * 1024 * 1024,
        256 * 1024,
        Duration::from_millis(250),
        10_000,
        64,
        16 * 1024,
        16 * 1024,
    )
    .unwrap_or_else(|error| panic!("limits: {error}"))
}

#[test]
fn wrap_injection_and_validation() {
    let plan = r#"return "x"; }); globalThis.pwned=true; //"#;
    let wrapped = wrap_plan(plan, 4096).unwrap_or_else(|error| panic!("wrap: {error}"));
    let quoted = serde_json::to_string(plan).unwrap_or_else(|error| panic!("json: {error}"));
    assert!(wrapped.contains(&format!("const __source = {quoted};")));
    assert_eq!(wrap_plan(" ", 8), Err(PlanError::Empty));
    assert!(matches!(
        wrap_plan("123", 2),
        Err(PlanError::TooLarge { .. })
    ));
    assert_eq!(wrap_plan("return '\0';", 64), Err(PlanError::Nul));
}

#[test]
fn invalid_duplicate_and_poison_identifiers() {
    let registration = GlobalRegistration::zero(vec![CapabilityDescriptor::new("bad-name", "x")]);
    assert!(matches!(
        registration.validate(),
        Err(RegistrationError::InvalidCapability(_))
    ));

    let registration = GlobalRegistration::zero(vec![
        CapabilityDescriptor::new("fs", "read"),
        CapabilityDescriptor::new("fs", "read"),
    ]);
    assert!(matches!(
        registration.validate(),
        Err(RegistrationError::DuplicateCapability(_))
    ));

    for poison in ["__proto__", "prototype", "constructor"] {
        let mut root = reg();
        root.root = poison.to_owned();
        assert!(matches!(
            root.validate(),
            Err(RegistrationError::InvalidGlobal(_))
        ));
        assert!(matches!(
            GlobalRegistration::zero(vec![CapabilityDescriptor::new(poison, "read")]).validate(),
            Err(RegistrationError::InvalidCapability(_))
        ));
        assert!(matches!(
            GlobalRegistration::zero(vec![CapabilityDescriptor::new("fs", poison)]).validate(),
            Err(RegistrationError::InvalidCapability(_))
        ));
    }
}

#[cfg(not(feature = "quickjs"))]
#[test]
fn disabled() {
    assert!(matches!(
        Host::new(lim(), reg()),
        Err(HostError::QuickJsDisabled)
    ));
}

#[cfg(feature = "quickjs")]
#[test]
fn hello_dispatch_and_counter() {
    let connector = Rc::new(C::ok());
    let creations = runtime_creation_count();
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let value = host
        .execute(
            "const f=zero['fs'].read;return {hello:'world',call:await f({path:'a'})};",
            connector.clone(),
        )
        .unwrap_or_else(|error| panic!("execute: {error}"));
    assert_eq!(
        value,
        json!({ "hello": "world", "call": { "echo": { "path": "a" } } })
    );
    assert_eq!(connector.calls.borrow().len(), 1);
    assert!(
        runtime_creation_count() > creations,
        "this execution must create one runtime even when tests run concurrently",
    );
}

#[cfg(feature = "quickjs")]
#[test]
fn registered_objects_have_no_inherited_to_string() {
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let value = host
        .execute(
            "return {root:typeof zero.toString,surface:typeof zero['fs'].toString,rootProto:Object.getPrototypeOf(zero),surfaceProto:Object.getPrototypeOf(zero['fs'])};",
            Rc::new(C::ok()),
        )
        .unwrap_or_else(|error| panic!("execute: {error}"));
    assert_eq!(
        value,
        json!({
            "root": "undefined",
            "surface": "undefined",
            "rootProto": null,
            "surfaceProto": null
        })
    );
}

#[cfg(feature = "quickjs")]
#[test]
fn connector_error() {
    let connector = Rc::new(C {
        calls: RefCell::new(vec![]),
        fail: true,
        delay: Duration::ZERO,
        result: None,
    });
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    assert!(host
        .execute("return await zero['fs'].read({});", connector.clone())
        .expect_err("fail")
        .to_string()
        .contains("connector refused request"));
}

#[cfg(feature = "quickjs")]
#[test]
fn oversized_connector_result_is_rejected_before_parse() {
    let mut limits = lim();
    limits.max_json_bytes = 32;
    let connector = Rc::new(C {
        calls: RefCell::new(vec![]),
        fail: false,
        delay: Duration::ZERO,
        result: Some(format!("\"{}\"", "x".repeat(64))),
    });
    let host = Host::new(limits, reg()).unwrap_or_else(|error| panic!("host: {error}"));
    assert!(host
        .execute("return await zero['fs']['read']({});", connector)
        .expect_err("oversized connector result")
        .to_string()
        .contains("result exceeds JSON limit"));
}

#[cfg(feature = "quickjs")]
#[test]
fn invalid_connector_json_is_rejected() {
    let connector = Rc::new(C {
        calls: RefCell::new(vec![]),
        fail: false,
        delay: Duration::ZERO,
        result: Some("not json".to_owned()),
    });
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    assert!(host
        .execute("return await zero['fs']['read']({});", connector)
        .is_err());
}

#[cfg(feature = "quickjs")]
#[test]
fn late_connector_result_maps_to_deadline() {
    let mut limits = lim();
    limits.wall_timeout = Duration::from_millis(2);
    let connector = Rc::new(C {
        calls: RefCell::new(vec![]),
        fail: false,
        delay: Duration::from_millis(10),
        result: Some("{}".to_owned()),
    });
    let host = Host::new(limits, reg()).unwrap_or_else(|error| panic!("host: {error}"));
    assert_eq!(
        host.execute("return await zero['fs']['read']({});", connector)
            .expect_err("late connector"),
        HostError::DeadlineExceeded
    );
}

#[cfg(feature = "quickjs")]
#[test]
fn plan_cannot_clobber_promise_completion() {
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let plan = r#"
for (const key of Reflect.ownKeys(globalThis)) {
    if (typeof key === "string" && key.startsWith("__zero_codemode_private_")) {
        globalThis[key] = "clobbered";
    }
}
return {ok: true};
"#;
    assert_eq!(
        host.execute(plan, Rc::new(C::ok()))
            .unwrap_or_else(|error| panic!("execute: {error}")),
        json!({ "ok": true })
    );
}

#[cfg(feature = "quickjs")]
#[test]
fn fuel_interrupt() {
    let mut limits = lim();
    limits.instruction_budget = 1;
    limits.wall_timeout = Duration::from_secs(2);
    let host = Host::new(limits, reg()).unwrap_or_else(|error| panic!("host: {error}"));
    assert_eq!(
        host.execute("for(;;){}", Rc::new(C::ok()))
            .expect_err("interrupt"),
        HostError::FuelExhausted
    );
}

#[cfg(feature = "quickjs")]
#[test]
fn duplicate_global() {
    let registration = GlobalRegistration::zero(vec![
        CapabilityDescriptor::new("fs", "read"),
        CapabilityDescriptor::new("fs", "read"),
    ]);
    assert!(matches!(
        Host::new(lim(), registration),
        Err(HostError::Registration(
            RegistrationError::DuplicateCapability(_)
        ))
    ));
}

#[cfg(feature = "quickjs")]
#[test]
fn bounded_microtasks() {
    let mut limits = lim();
    limits.microtask_ceiling = 2;
    let host = Host::new(limits, reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let plan = r#"
return new Promise(resolve => {
    let remaining = 20;
    for (let index = 0; index < 20; index += 1) {
        Promise.resolve().then(() => {
            remaining -= 1;
            if (remaining === 0) resolve("done");
        });
    }
});
"#;
    assert_eq!(
        host.execute(plan, Rc::new(C::ok())).expect_err("bounded"),
        HostError::MicrotaskLimit
    );
}

#[cfg(feature = "quickjs")]
#[test]
fn external_cancel_interrupts_sync_loop() {
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let cancelled = Arc::new(AtomicBool::new(false));
    let trigger = Arc::clone(&cancelled);
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(2));
        trigger.store(true, Ordering::Relaxed);
    });
    let started = Instant::now();
    assert_eq!(
        host.execute_with_cancel("for(;;){}", Rc::new(C::ok()), cancelled)
            .expect_err("cancel"),
        HostError::Cancelled
    );
    assert!(started.elapsed() < Duration::from_millis(50));
}

#[cfg(feature = "quickjs")]
#[test]
fn sync_loop_hits_deadline() {
    let mut limits = lim();
    limits.instruction_budget = u64::MAX;
    limits.wall_timeout = Duration::from_millis(1);
    let host = Host::new(limits, reg()).unwrap_or_else(|error| panic!("host: {error}"));
    assert_eq!(
        host.execute("for(;;){}", Rc::new(C::ok()))
            .expect_err("deadline"),
        HostError::DeadlineExceeded
    );
}

#[cfg(feature = "quickjs")]
#[test]
fn unknown_property_on_a_connector_result_throws() {
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let error = host
        .execute(
            "const r = await zero['fs'].read({path:'a'}); return r.stdout;",
            Rc::new(C::ok()),
        )
        .expect_err("unknown property must throw");
    let message = error.to_string();
    assert!(
        message.contains("unknown property 'stdout'"),
        "message must name the property: {message}"
    );
    assert!(
        message.contains("available properties: echo"),
        "message must list the real shape: {message}"
    );
}

#[cfg(feature = "quickjs")]
#[test]
fn known_properties_stay_readable_through_the_strict_guard() {
    let connector = Rc::new(C {
        calls: RefCell::new(vec![]),
        fail: false,
        delay: Duration::ZERO,
        result: Some("{\"result\":\"ok\",\"nested\":{\"visible\":1},\"list\":[1,2]}".to_owned()),
    });
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let value = host
        .execute(
            "const r = await zero['fs'].read({}); return [r.result, r.nested.visible, r.list.length];",
            connector,
        )
        .unwrap_or_else(|error| panic!("execute: {error}"));
    assert_eq!(value, json!(["ok", 1, 2]));
}

#[cfg(feature = "quickjs")]
#[test]
fn opts_bearing_calls_forward_every_argument_to_the_connector() {
    let connector = Rc::new(C::ok());
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    host.execute(
        "await zero['fs'].read({path:'a'}); await zero['fs'].read({path:'b'}, {timeoutMs:60000}); return null;",
        connector.clone(),
    )
    .unwrap_or_else(|error| panic!("execute: {error}"));

    let calls = connector.calls.borrow();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], json!({ "path": "a" }));
    assert_eq!(
        calls[1],
        json!([{ "path": "b" }, { "timeoutMs": 60_000 }]),
        "an opts argument must reach the connector instead of being dropped"
    );
}

#[cfg(feature = "quickjs")]
#[test]
fn oversized_result_spills_to_a_ref_with_a_bounded_preview() {
    let store = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let mut limits = lim();
    limits.max_json_bytes = 16 * 1024 * 1024;
    limits.memory_bytes = 512 * 1024 * 1024;
    limits.instruction_budget = 100_000_000;
    limits.wall_timeout = Duration::from_secs(60);
    let host = Host::new(limits, reg())
        .unwrap_or_else(|error| panic!("host: {error}"))
        .with_result_spill(store.path());

    // 20971522 bytes encoded: the exact size that used to be a hard error.
    let plan = "return 'x'.repeat(20971520);";
    let value = host
        .execute(plan, Rc::new(C::ok()))
        .unwrap_or_else(|error| panic!("spill: {error}"));

    assert_eq!(value["schema"], RESULT_SPILL_SCHEMA);
    assert_eq!(value["spilled"], true);
    assert_eq!(value["bytes"], 20_971_522);
    assert_eq!(value["previewTruncated"], true);
    let preview = value["preview"].as_str().expect("preview text");
    assert!(preview.len() <= RESULT_SPILL_PREVIEW_BYTES);
    assert_eq!(value["previewBytes"], preview.len());

    let sha = value["sha256"].as_str().expect("sha256");
    assert_eq!(value["ref"], format!("tz://blob/{sha}"));
    let stored = SharedCas::open(store.path())
        .get_verified(sha)
        .unwrap_or_else(|error| panic!("verify spilled object: {error}"));
    assert_eq!(stored.len(), 20_971_522);
}

#[cfg(feature = "quickjs")]
#[test]
fn oversized_result_without_a_spill_root_still_reports_the_limit() {
    let mut limits = lim();
    limits.max_json_bytes = 32;
    let host = Host::new(limits, reg()).unwrap_or_else(|error| panic!("host: {error}"));
    assert!(host
        .execute("return 'x'.repeat(64);", Rc::new(C::ok()))
        .expect_err("oversized result")
        .to_string()
        .contains("maximum is 32"));
}

#[cfg(feature = "quickjs")]
#[test]
fn both_routes_expose_the_same_canonical_result_fields() {
    // Shape A: the single-surface route names inline output `text` and the
    // ref-ed output `stdout_ref`. Shape B: the cross-surface route names them
    // `visible`/`result` and `ref`. A plan must read both identically.
    let route_shapes = [
        r#"{"text":"hi\n","stdout_ref":"tz://blob/a","combined_ref":"tz://blob/c","exit_code":0}"#,
        r#"{"visible":"hi\n","result":"hi\n","ref":"tz://blob/a","status":"ok"}"#,
    ];
    for shape in route_shapes {
        let connector = Rc::new(C {
            calls: RefCell::new(vec![]),
            fail: false,
            delay: Duration::ZERO,
            result: Some(shape.to_owned()),
        });
        let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
        let value = host
            .execute(
                "const r = await zero['fs'].read({}); return [r.text, r.ref];",
                connector,
            )
            .unwrap_or_else(|error| panic!("execute {shape}: {error}"));
        assert_eq!(
            value,
            json!(["hi\n", "tz://blob/a"]),
            "canonical fields must be identical for {shape}"
        );
    }
}

#[cfg(feature = "quickjs")]
#[test]
fn canonical_fields_never_shadow_what_the_connector_returned() {
    let connector = Rc::new(C {
        calls: RefCell::new(vec![]),
        fail: false,
        delay: Duration::ZERO,
        result: Some(
            r#"{"text":"own","visible":"other","ref":"tz://blob/own","stdout_ref":"tz://blob/other"}"#
                .to_owned(),
        ),
    });
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let value = host
        .execute(
            "const r = await zero['fs'].read({}); return [r.text, r.ref, r.visible, r.stdout_ref];",
            connector,
        )
        .unwrap_or_else(|error| panic!("execute: {error}"));
    assert_eq!(
        value,
        json!(["own", "tz://blob/own", "other", "tz://blob/other"])
    );
}

#[cfg(feature = "quickjs")]
#[test]
fn canonical_fields_pass_the_strict_guard_and_are_enumerable() {
    let connector = Rc::new(C {
        calls: RefCell::new(vec![]),
        fail: false,
        delay: Duration::ZERO,
        result: Some(r#"{"visible":"hi","stdout_ref":"tz://blob/a"}"#.to_owned()),
    });
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let value = host
        .execute(
            "const r = await zero['fs'].read({}); return Object.keys(r).sort();",
            connector,
        )
        .unwrap_or_else(|error| panic!("execute: {error}"));
    assert_eq!(value, json!(["ref", "stdout_ref", "text", "visible"]));
}

#[cfg(feature = "quickjs")]
#[test]
fn a_result_without_any_output_alias_gains_no_canonical_fields() {
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let error = host
        .execute(
            "const r = await zero['fs'].read({path:'a'}); return r.text;",
            Rc::new(C::ok()),
        )
        .expect_err("normalization must not invent output fields");
    assert!(
        error.to_string().contains("unknown property 'text'"),
        "{error}"
    );
}

#[test]
fn canonical_field_names_are_published() {
    assert_eq!(CANONICAL_RESULT_FIELDS, &["text", "ref"]);
    assert_eq!(CANONICAL_TEXT_ALIASES[0], "text");
    assert_eq!(CANONICAL_REF_ALIASES[0], "ref");
}
