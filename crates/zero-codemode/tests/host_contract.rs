use std::time::Duration;
#[cfg(feature = "quickjs")]
use std::time::Instant;

#[cfg(feature = "quickjs")]
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

#[cfg(feature = "quickjs")]
use serde_json::{Value, json};
use zero_codemode::{CANONICAL_REF_ALIASES, CANONICAL_RESULT_FIELDS, CANONICAL_TEXT_ALIASES};
#[cfg(feature = "quickjs")]
use zero_codemode::{Connector, ConnectorError, DispatchContext, runtime_creation_count};

use zero_codemode::{
    CapabilityDescriptor, GlobalRegistration, Host, HostError, HostLimits, PlanError,
    RegistrationError, wrap_plan,
};
#[cfg(feature = "quickjs")]
use zero_codemode::{
    DEFAULT_MAX_VISIBLE_RESULT_BYTES, MAX_RESULT_SPILL_ENVELOPE_BYTES, MAX_VISIBLE_ERROR_BYTES,
    RESULT_SPILL_PREVIEW_BYTES, RESULT_SPILL_SCHEMA, finalize_visible_error,
};
#[cfg(feature = "quickjs")]
use zero_store::SharedCas;

#[cfg(not(feature = "quickjs"))]
#[test]
fn cancel_timeout_entrypoint_is_available_without_quickjs() {
    let _entrypoint = Host::execute_with_cancel_timeout;
}

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
fn absent_optional_ref_is_undefined_without_weakening_unknown_field_errors() {
    let registration = GlobalRegistration::zero(vec![CapabilityDescriptor::new("graph", "blast")]);
    let host = Host::new(lim(), registration).unwrap_or_else(|error| panic!("host: {error}"));
    let connector = Rc::new(C {
        calls: RefCell::new(vec![]),
        fail: false,
        delay: Duration::ZERO,
        result: Some(r#"{"ack":"C","found":false}"#.into()),
    });
    let value = host
        .execute(
            "const result=await zero.graph.blast('missing');return {optionalRefIsUndefined:result.ref===undefined};",
            connector.clone(),
        )
        .unwrap_or_else(|error| panic!("execute: {error}"));
    assert_eq!(value, json!({"optionalRefIsUndefined":true}));

    let error = host
        .execute(
            "const result=await zero.graph.blast('missing');return result.fabricated;",
            connector,
        )
        .expect_err("unadvertised fields remain typed failures");
    assert!(error.to_string().contains("unknown property 'fabricated'"));
}

#[cfg(feature = "quickjs")]
#[test]
fn unknown_surfaces_and_methods_fail_loud_with_closest_names() {
    let registration = GlobalRegistration::zero(vec![
        CapabilityDescriptor::new("fs", "read"),
        CapabilityDescriptor::new("fs", "write"),
        CapabilityDescriptor::new("token", "shell"),
    ]);
    let host = Host::new(lim(), registration).unwrap_or_else(|error| panic!("host: {error}"));
    let connector = Rc::new(C::ok());

    let method_error = host
        .execute("return await zero.fs.reed({});", connector.clone())
        .expect_err("unknown method must fail");
    assert!(matches!(method_error, HostError::MethodNotFound(_)));
    assert_eq!(
        method_error.to_string(),
        "JavaScript exception: method_not_found: unknown method 'reed' on zero.fs; closest methods: read, write"
    );

    let surface_error = host
        .execute("return await zero.graph.read({});", connector.clone())
        .expect_err("unknown surface must fail");
    assert!(matches!(surface_error, HostError::SurfaceNotFound(_)));
    let message = surface_error.to_string();
    assert!(
        message.contains("surface_not_found: unknown surface 'graph' on zero"),
        "unexpected error: {message}"
    );
    assert!(
        message.contains("closest surfaces:"),
        "unexpected error: {message}"
    );
    assert!(
        connector.calls.borrow().is_empty(),
        "unknown names must not dispatch or degrade to catalog search"
    );
}

#[cfg(feature = "quickjs")]
#[test]
fn string_literal_tokens_never_change_capability_routing() {
    let registration = GlobalRegistration::zero(vec![
        CapabilityDescriptor::new("fs", "read"),
        CapabilityDescriptor::new("token", "shell"),
    ]);
    let connector = Rc::new(C::ok());
    let host = Host::new(lim(), registration).unwrap_or_else(|error| panic!("host: {error}"));
    let command =
        r#"printf 'zero.fs.read({path:"x"}) zero.graph.query()'; nohup worker & disown background"#;
    let plan = format!(
        "const command = {}; return await zero.token.shell({{command}});",
        serde_json::to_string(command).unwrap_or_else(|error| panic!("encode command: {error}"))
    );

    let value = host
        .execute(&plan, connector.clone())
        .unwrap_or_else(|error| panic!("execute: {error}"));
    assert_eq!(value, json!({"echo": {"command": command}}));
    assert_eq!(
        connector.calls.borrow().as_slice(),
        &[json!({"command": command})]
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
    assert!(
        host.execute("return await zero['fs'].read({});", connector.clone())
            .expect_err("fail")
            .to_string()
            .contains("connector refused request")
    );
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
    assert!(
        host.execute("return await zero['fs']['read']({});", connector)
            .expect_err("oversized connector result")
            .to_string()
            .contains("result exceeds JSON limit")
    );
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
    assert!(
        host.execute("return await zero['fs']['read']({});", connector)
            .is_err()
    );
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
fn explicit_timeout_is_bounded_by_host_limit() {
    let mut limits = lim();
    limits.instruction_budget = u64::MAX;
    limits.wall_timeout = Duration::from_millis(2);
    let host = Host::new(limits, reg()).unwrap_or_else(|error| panic!("host: {error}"));
    for requested in [Duration::from_millis(1), Duration::from_secs(1)] {
        assert_eq!(
            host.execute_with_cancel_timeout(
                "for(;;){}",
                Rc::new(C::ok()),
                Arc::new(AtomicBool::new(false)),
                requested,
            )
            .expect_err("deadline"),
            HostError::DeadlineExceeded
        );
    }
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
    let visible_bytes = serde_json::to_vec(&value).unwrap().len();
    assert!(visible_bytes <= MAX_RESULT_SPILL_ENVELOPE_BYTES);
    assert_eq!(value["receipt"]["finalizedValueJsonBytes"], visible_bytes);
    assert_eq!(value["receipt"]["rawResultJsonBytes"], 20_971_522);
    assert_eq!(value["receipt"]["inlineResultBytes"], 0);
    assert_eq!(value["receipt"]["omittedBehindExactRefBytes"], 20_971_522);
    assert_eq!(
        value["receipt"]["savingsBytes"],
        20_971_522_usize.saturating_sub(visible_bytes)
    );
    assert_eq!(
        value["receipt"]["visibleTokenCountStatus"],
        "requires_tokenzero_certification"
    );
    assert!(value["receipt"]["visibleTokenCount"].is_null());

    let sha = value["sha256"].as_str().expect("sha256");
    assert_eq!(value["ref"], format!("tz://blob/{sha}"));
    let stored = SharedCas::open(store.path())
        .get_verified(sha)
        .unwrap_or_else(|error| panic!("verify spilled object: {error}"));
    assert_eq!(stored.len(), 20_971_522);
}

#[cfg(feature = "quickjs")]
#[test]
fn model_visible_error_text_is_bounded_without_splitting_utf8() {
    let error = "é".repeat(MAX_VISIBLE_ERROR_BYTES);
    let visible = finalize_visible_error(&error);
    assert!(visible.len() <= MAX_VISIBLE_ERROR_BYTES);
    assert!(visible.ends_with("... [truncated]"));
    assert!(std::str::from_utf8(visible.as_bytes()).is_ok());
    assert_eq!(finalize_visible_error("short"), "short");
}

#[cfg(feature = "quickjs")]
#[test]
fn arbitrary_decimal_byte_arrays_finalize_once_behind_an_exact_ref() {
    let store = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let mut limits = lim();
    limits.max_json_bytes = 16 * 1024 * 1024;
    limits.memory_bytes = 128 * 1024 * 1024;
    limits.instruction_budget = 10_000_000;
    let host = Host::new(limits, reg())
        .unwrap_or_else(|error| panic!("host: {error}"))
        .with_visible_result_budget(DEFAULT_MAX_VISIBLE_RESULT_BYTES)
        .unwrap_or_else(|error| panic!("result budget: {error}"))
        .with_result_spill(store.path());
    let value = host
        .execute(
            "const payload=Array.from({length:4096},(_,i)=>i%256); return {contract:{payload},env:{status:'ok'},refs:['tz://blob/a','tz://blob/b']};",
            Rc::new(C::ok()),
        )
        .unwrap_or_else(|error| panic!("finalize: {error}"));

    assert_eq!(value["spilled"], true);
    let visible = serde_json::to_string(&value).unwrap();
    assert!(visible.len() <= MAX_RESULT_SPILL_ENVELOPE_BYTES);
    assert!(!visible.contains("\"payload\":[0,1,2"));
    assert_eq!(value["receipt"]["finalizedValueJsonBytes"], visible.len());
    let raw_bytes = value["receipt"]["rawResultJsonBytes"].as_u64().unwrap() as usize;
    assert!(raw_bytes > DEFAULT_MAX_VISIBLE_RESULT_BYTES);
    assert_eq!(value["receipt"]["omittedBehindExactRefBytes"], raw_bytes);
    let sha = value["sha256"].as_str().unwrap();
    let stored = SharedCas::open(store.path()).get_verified(sha).unwrap();
    let exact: Value = serde_json::from_slice(&stored).unwrap();
    assert_eq!(exact["contract"]["payload"].as_array().unwrap().len(), 4096);
    assert_eq!(exact["refs"].as_array().unwrap().len(), 2);
}

#[cfg(feature = "quickjs")]
#[test]
fn result_finalizer_bounds_nested_width_cycle_ref_and_connector_shapes() {
    let store = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let mut limits = lim();
    limits.max_json_bytes = 16 * 1024 * 1024;
    limits.memory_bytes = 128 * 1024 * 1024;
    limits.instruction_budget = 20_000_000;
    let host = Host::new(limits, reg())
        .unwrap_or_else(|error| panic!("host: {error}"))
        .with_visible_result_budget(DEFAULT_MAX_VISIBLE_RESULT_BYTES)
        .unwrap_or_else(|error| panic!("result budget: {error}"))
        .with_result_spill(store.path());
    let plans = [
        "return {nested:{value:'x'.repeat(4096)}};",
        "return Array.from({length:256},(_,i)=>'tz://blob/'+String(i).padStart(64,'0'));",
        "const value={};for(let i=0;i<512;i++)value['field'+i]=i;return value;",
        "let value='x'.repeat(2048);for(let i=0;i<64;i++)value={child:value};return value;",
        "const value={payload:Array.from({length:1024},(_,i)=>i%256)};value.cycle=value;return value;",
        "const connector=await zero.fs.read({path:'a'});return {connector,payload:'x'.repeat(4096)};",
        "return 'x'.repeat(1024);",
        "return 'x'.repeat(1025);",
    ];
    for plan in plans {
        let value = host
            .execute(plan, Rc::new(C::ok()))
            .unwrap_or_else(|error| panic!("finalize {plan:?}: {error}"));
        let visible_bytes = serde_json::to_vec(&value).unwrap().len();
        assert!(
            visible_bytes <= MAX_RESULT_SPILL_ENVELOPE_BYTES,
            "{visible_bytes} visible bytes for {plan:?}"
        );
        if value["spilled"] == true {
            assert_eq!(value["receipt"]["finalizedValueJsonBytes"], visible_bytes);
            let sha = value["sha256"].as_str().unwrap();
            SharedCas::open(store.path()).get_verified(sha).unwrap();
        } else {
            assert!(visible_bytes <= DEFAULT_MAX_VISIBLE_RESULT_BYTES);
        }
    }
}

#[cfg(feature = "quickjs")]
#[test]
fn oversized_result_without_a_spill_root_still_reports_the_limit() {
    let mut limits = lim();
    limits.max_json_bytes = 32;
    let host = Host::new(limits, reg()).unwrap_or_else(|error| panic!("host: {error}"));
    assert!(
        host.execute("return 'x'.repeat(64);", Rc::new(C::ok()))
            .expect_err("oversized result")
            .to_string()
            .contains("maximum is 32")
    );
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

#[cfg(feature = "quickjs")]
#[test]
fn capability_calls_return_thenables_usable_with_promise_all() {
    let connector = Rc::new(C::ok());
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let value = host
        .execute(
            r#"const a = zero["fs"].read({path:"a"});
             const b = zero["fs"].read({path:"b"});
             const thenable = typeof a.then === "function";
             const [ra, rb] = await Promise.all([a, b]);
             return {thenable, a: ra.echo.path, b: rb.echo.path};"#,
            connector.clone(),
        )
        .unwrap_or_else(|error| panic!("promise.all plan: {error}"));
    assert_eq!(value, json!({"thenable": true, "a": "a", "b": "b"}));
    assert_eq!(connector.calls.borrow().len(), 2);
}

#[cfg(feature = "quickjs")]
#[test]
fn unserializable_plan_result_degrades_instead_of_failing() {
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let plan = r#"
const value = { status: "committed", count: 2, ref: "fz://blob/abc" };
Object.defineProperty(value, "handle", {
  enumerable: true,
  get() { throw new TypeError("host-guarded property"); },
});
value.cycle = value;
return value;
"#;
    let result: Value = host
        .execute(plan, Rc::new(C::ok()))
        .unwrap_or_else(|error| panic!("execute: {error}"));
    assert_eq!(result["serialization_degraded"], json!(true));
    assert_eq!(result["result"]["status"], json!("committed"));
    assert_eq!(result["result"]["count"], json!(2));
    assert_eq!(result["result"]["handle"], json!("[unreadable]"));
    assert_eq!(result["refs"], json!(["fz://blob/abc"]));
}
