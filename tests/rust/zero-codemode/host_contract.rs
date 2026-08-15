use std::time::Duration;
use std::time::Instant;

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use serde_json::{Value, json};
use zero_codemode::PUBLIC_RESULT_FIELDS;
use zero_codemode::{
    Connector, ConnectorCompletion, ConnectorError, DispatchContext, runtime_creation_count,
};

use zero_codemode::{
    CapabilityDescriptor, GlobalRegistration, Host, HostError, HostLimits, PlanError,
    RegistrationError, wrap_plan, OUTPUT_WALL_ARRANGEMENTS,
};
use zero_codemode::{
    DEFAULT_MAX_VISIBLE_RESULT_BYTES, MAX_INFLIGHT_CONNECTOR_CALLS,
    MAX_RESULT_SPILL_ENVELOPE_BYTES, MAX_VISIBLE_ERROR_BYTES, RESULT_SPILL_PREVIEW_BYTES,
    RESULT_SPILL_SCHEMA, finalize_visible_error,
};
use zero_store::SharedCas;

#[test]
fn cancel_timeout_entrypoint_is_available() {
    let _entrypoint = Host::execute_with_cancel_timeout;
}

struct C {
    calls: RefCell<Vec<Value>>,
    fail: bool,
    delay: Duration,
    result: Option<String>,
}

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

impl Connector for C {
    fn dispatch(
        &self,
        _: &CapabilityDescriptor,
        args_json: &str,
        context: DispatchContext,
        completion: ConnectorCompletion,
    ) -> Result<(), ConnectorError> {
        assert!(context.max_json_bytes > 0);
        let args: Value = serde_json::from_str(args_json)
            .map_err(|error| ConnectorError::new(error.to_string()))?;
        self.calls.borrow_mut().push(args.clone());
        let result = if self.fail {
            Err(ConnectorError::new("connector refused request"))
        } else if let Some(result) = &self.result {
            Ok(result.clone())
        } else {
            serde_json::to_string(&json!({ "echo": args }))
                .map_err(|error| ConnectorError::new(error.to_string()))
        };
        if self.delay.is_zero() {
            completion.complete(result)
        } else {
            let delay = self.delay;
            thread::spawn(move || {
                thread::sleep(delay);
                let _ = completion.complete(result);
            });
            Ok(())
        }
    }
}

struct CoordinatedConnector {
    expected: usize,
    completions: RefCell<Vec<(u64, ConnectorCompletion)>>,
}

struct TailConnector {
    completions: Arc<std::sync::atomic::AtomicU64>,
}

impl Connector for TailConnector {
    fn dispatch(
        &self,
        _: &CapabilityDescriptor,
        args_json: &str,
        _: DispatchContext,
        completion: ConnectorCompletion,
    ) -> Result<(), ConnectorError> {
        let sequence = serde_json::from_str::<Value>(args_json)
            .map_err(|error| ConnectorError::new(error.to_string()))?["sequence"]
            .as_u64()
            .ok_or_else(|| ConnectorError::new("missing sequence"))?;
        let completions = Arc::clone(&self.completions);
        let finish = move || {
            let _ = completion.complete(Ok(json!({"sequence":sequence}).to_string()));
            completions.fetch_add(1, Ordering::Release);
        };
        if sequence == 0 {
            finish();
        } else {
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(20));
                finish();
            });
        }
        Ok(())
    }
}

impl Connector for CoordinatedConnector {
    fn dispatch(
        &self,
        _: &CapabilityDescriptor,
        args_json: &str,
        _: DispatchContext,
        completion: ConnectorCompletion,
    ) -> Result<(), ConnectorError> {
        let args: Value = serde_json::from_str(args_json)
            .map_err(|error| ConnectorError::new(error.to_string()))?;
        let sequence = args
            .get("sequence")
            .and_then(Value::as_u64)
            .ok_or_else(|| ConnectorError::new("missing sequence"))?;
        let mut completions = self.completions.borrow_mut();
        completions.push((sequence, completion));
        if completions.len() == self.expected {
            let mut ready = std::mem::take(&mut *completions);
            ready.sort_by_key(|(sequence, _)| *sequence);
            drop(completions);
            for (sequence, completion) in ready {
                completion.complete(
                    serde_json::to_string(&json!({
                        "sequence": sequence,
                    }))
                    .map_err(|error| ConnectorError::new(error.to_string())),
                )?;
            }
        }
        Ok(())
    }
}

fn reg() -> GlobalRegistration {
    GlobalRegistration::zero(vec![CapabilityDescriptor::new("fs", "read")])
}

fn reg_with_expand() -> GlobalRegistration {
    GlobalRegistration::zero(vec![
        CapabilityDescriptor::new("fs", "read"),
        CapabilityDescriptor::new("token", "expand"),
    ])
}

fn lim() -> HostLimits {
    HostLimits::new(
        16 * 1024 * 1024,
        256 * 1024,
        Duration::from_millis(250),
        10_000,
        64,
        MAX_INFLIGHT_CONNECTOR_CALLS,
        16 * 1024,
        16 * 1024,
    )
    .unwrap_or_else(|error| panic!("limits: {error}"))
}

#[test]
fn output_wall_arrangements_are_local_not_one_ceiling() {
    assert_eq!(
        OUTPUT_WALL_ARRANGEMENTS.len(),
        4,
        "a fifth silent constructor needs a fifth named row"
    );
    let names: Vec<&str> = OUTPUT_WALL_ARRANGEMENTS.iter().map(|row| row.name).collect();
    assert_eq!(
        names,
        [
            "capability-manifest-echo",
            "host-limits-default",
            "zsx-session-visible",
            "zsx-connector-host",
        ]
    );

    let default = HostLimits::default();
    let host_default = OUTPUT_WALL_ARRANGEMENTS
        .iter()
        .find(|row| row.name == "host-limits-default")
        .expect("host-limits-default row");
    assert_eq!(default.memory_bytes, host_default.memory_bytes);
    assert_eq!(default.wall_timeout, Duration::from_millis(host_default.wall_ms));
    assert_eq!(default.max_json_bytes, host_default.output_bytes);

    let point = lim();
    assert_ne!(
        point, default,
        "host_contract lim() is a Point arrangement, not HostLimits::default"
    );
    assert_eq!(point.wall_timeout, Duration::from_millis(250));
    assert_eq!(point.memory_bytes, 16 * 1024 * 1024);

    let echo = OUTPUT_WALL_ARRANGEMENTS
        .iter()
        .find(|row| row.name == "capability-manifest-echo")
        .expect("echo row");
    assert_eq!(echo.wall_ms, 250);
    assert_ne!(
        echo.memory_bytes, point.memory_bytes,
        "CONTRACT 32 MiB echo is not host_contract lim() 16 MiB"
    );
    assert_ne!(
        echo.output_bytes, DEFAULT_MAX_VISIBLE_RESULT_BYTES,
        "CONTRACT 64 KiB echo is not DEFAULT_MAX_VISIBLE_RESULT_BYTES"
    );
}

#[test]
fn wrap_injection_and_validation() {
    let plan = r#"return "x"; }); globalThis.pwned=true; //"#;
    let wrapped = wrap_plan(plan, 4096).unwrap_or_else(|error| panic!("wrap: {error}"));
    assert_eq!(wrapped, plan);
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
        json!({ "hello": "world", "call": { "ack":"ok", "content": {"kind":"inline", "value": { "echo": { "path": "a" } } } } })
    );
    assert_eq!(connector.calls.borrow().len(), 1);
    assert!(
        runtime_creation_count() > creations,
        "this execution must create one runtime even when tests run concurrently",
    );
}

#[test]
fn async_arrow_helper_can_await_spread_tool_call() {
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let value = host
        .execute(
            r#"const __fszeroDecorate = (__value) => ({ok:true, result:__value});
               const __fszeroCall = async (__method, __args) => __fszeroDecorate(await __method(...__args));
               const fs = {read: (__arg) => __fszeroCall(zero.fs.read, [__arg])};
               return await (async () => {
                   const r = await fs.read({path: "js.txt"});
                   if (!r || r.ok === false) throw new Error("read failed");
                   return r;
               })();"#,
            Rc::new(C::ok()),
        )
        .unwrap_or_else(|error| panic!("execute: {error}"));
    assert_eq!(value["ok"], true);
    assert_eq!(
        value["result"]["content"]["value"]["echo"]["path"],
        "js.txt"
    );

    let error = host
        .execute("return await;", Rc::new(C::ok()))
        .expect_err("await must not become an ambient function");
    assert!(error.to_string().contains("unknown identifier 'await'"));
}

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

#[test]
fn inline_results_expose_only_the_typed_inline_variant() {
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
            "const r=await zero.graph.blast('missing');return [r.ack,r.content.kind,r.content.value.found];",
            connector.clone(),
        )
        .unwrap_or_else(|error| panic!("execute: {error}"));
    assert_eq!(value, json!(["C", "inline", false]));

    for property in ["ref", "fabricated"] {
        let error = host
            .execute(
                &format!("const r=await zero.graph.blast('missing');return r.{property};"),
                connector.clone(),
            )
            .expect_err("unadvertised fields remain typed failures");
        assert!(
            error
                .to_string()
                .contains(&format!("unknown property '{property}'"))
        );
    }
}

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
    assert_eq!(
        value,
        json!({"ack":"ok","content":{"kind":"inline","value":{"echo":{"command":command}}}})
    );
    assert_eq!(
        connector.calls.borrow().as_slice(),
        &[json!({"command": command})]
    );
}

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
        message.contains("available properties: ack, content"),
        "message must list the real shape: {message}"
    );
}

#[test]
fn wrong_property_inside_a_domain_array_fails_loud() {
    let connector = Rc::new(C {
        calls: RefCell::new(vec![]),
        fail: false,
        delay: Duration::ZERO,
        result: Some(r#"{"items":[{"name":"x"}]}"#.into()),
    });
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let error = host
        .execute(
            "const r=await zero.fs.read({});return r.content.value.items[0].missing;",
            connector,
        )
        .expect_err("nested wrong property must fail");
    assert!(
        error.to_string().contains("unknown property 'missing'"),
        "{error}"
    );
}

#[test]
fn wrong_connector_property_is_a_catchable_type_error() {
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let value = host
        .execute(
            "const r=await zero.fs.read({});try{return r.missing;}catch(error){return [error.name,error.message.includes(\"unknown property 'missing'\")];}",
            Rc::new(C::ok()),
        )
        .expect("TypeError must be catchable");
    assert_eq!(value, json!(["TypeError", true]));
}

#[test]
fn unsupported_map_and_set_globals_fail_at_construction() {
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    for source in ["return new Map();", "return new Set();"] {
        let error = host
            .execute(source, Rc::new(C::ok()))
            .expect_err("unsupported collection global must fail");
        assert!(error.to_string().contains("unknown identifier"), "{error}");
    }
}

// `ctx.payload` yields the exact payload text from a `payload_utf8` receipt
// (never parsed); `ctx.result` / `ctx.refs` keep hiding transport nesting.
#[test]
fn ctx_result_payload_and_refs_hide_transport_nesting() {
    let connector = Rc::new(C {
        calls: RefCell::new(vec![]),
        fail: false,
        delay: Duration::ZERO,
        result: Some(
            json!({
                "metadata":{"ownership":{"refs":["fz://blob/abc"]}},
                "value":{"operation":"fs.read","value":{"payload_utf8":"hello"},"refs":["fz://blob/abc"]}
            })
            .to_string(),
        ),
    });
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let value = host
        .execute(
            "const r=await zero.fs.read({});return [ctx.result(r).operation,ctx.payload(r),ctx.refs(r)[0]];",
            connector,
        )
        .expect("ergonomic helpers");
    assert_eq!(value, json!(["fs.read", "hello", "fz://blob/abc"]));
}

// Destructuring a connector result (an object) with an array pattern used to
// skip binding silently and surface later as "unknown identifier"; it must
// fail loud at the destructure site with a repair hint.
#[test]
fn destructuring_a_connector_result_fails_loud_at_the_site() {
    let connector = Rc::new(C::ok());
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let error = host
        .execute(
            "const [a, b] = await zero.fs.read({}); return a;",
            connector,
        )
        .expect_err("array-destructuring an object must fail");
    let text = error.to_string();
    assert!(text.contains("cannot destructure"), "{text}");
    assert!(text.contains("array pattern"), "{text}");
}

// Unsupported string methods must name the supported set so a failing plan
// can self-correct without a docs round-trip.
#[test]
fn unsupported_string_method_error_lists_supported_methods() {
    let connector = Rc::new(C::ok());
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let error = host
        .execute("return 'abc'.match('b');", connector)
        .expect_err("match is unsupported");
    let text = error.to_string();
    assert!(text.contains("string method 'match'"), "{text}");
    assert!(text.contains("supported: includes"), "{text}");
}

// The fallback error has advertised indexOf since its introduction; the arm
// must exist and use char indices consistent with slice/substring.
#[test]
fn string_index_of_matches_the_advertised_support_list() {
    let connector = Rc::new(C::ok());
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let value = host
        .execute(
            "const s='héllo world';return [s.indexOf('world'), s.indexOf('absent'), s.slice(s.indexOf('world'))];",
            connector,
        )
        .unwrap_or_else(|error| panic!("indexOf: {error}"));
    assert_eq!(value, json!([6, -1, "world"]));
}

// ctx.payload on a payload_utf8 receipt returns the exact text; a caller that
// knows the payload is JSON parses it explicitly.
#[test]
fn ctx_payload_text_round_trips_through_json_parse() {
    let connector = Rc::new(C {
        calls: RefCell::new(vec![]),
        fail: false,
        delay: Duration::ZERO,
        result: Some(
            json!({
                "metadata":{"ownership":{"refs":["fz://blob/abc"]}},
                "value":{"operation":"fs.read_many","value":{"payload_utf8":"[\"alpha\",\"beta\"]"},"refs":["fz://blob/abc"]}
            })
            .to_string(),
        ),
    });
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let value = host
        .execute(
            "const files = await zero.fs.read({}); const texts = JSON.parse(ctx.payload(files)); return texts[1];",
            connector,
        )
        .expect("payload text parses");
    assert_eq!(value, json!("beta"));
}

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
            "const r=await zero.fs.read({});const v=r.content.value;return [v.result,v.nested.visible,v.list.length];",
            connector,
        )
        .unwrap_or_else(|error| panic!("execute: {error}"));
    assert_eq!(value, json!(["ok", 1, 2]));
}

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

#[test]
fn oversized_result_spills_to_a_ref_with_a_bounded_preview() {
    let store = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let mut limits = lim();
    limits.max_json_bytes = 16 * 1024 * 1024;
    limits.memory_bytes = 512 * 1024 * 1024;
    limits.instruction_budget = 100_000_000;
    limits.wall_timeout = Duration::from_secs(60);
    let host = Host::new(limits, reg_with_expand())
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

#[test]
fn model_visible_error_text_is_bounded_without_splitting_utf8() {
    let error = "é".repeat(MAX_VISIBLE_ERROR_BYTES);
    let visible = finalize_visible_error(&error);
    assert!(visible.len() <= MAX_VISIBLE_ERROR_BYTES);
    assert!(visible.ends_with("... [truncated]"));
    assert!(std::str::from_utf8(visible.as_bytes()).is_ok());
    assert_eq!(finalize_visible_error("short"), "short");
}

#[test]
fn arbitrary_decimal_byte_arrays_finalize_once_behind_an_exact_ref() {
    let store = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let mut limits = lim();
    limits.max_json_bytes = 16 * 1024 * 1024;
    limits.memory_bytes = 128 * 1024 * 1024;
    limits.instruction_budget = 10_000_000;
    let host = Host::new(limits, reg_with_expand())
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

#[test]
fn result_finalizer_bounds_nested_width_cycle_ref_and_connector_shapes() {
    let store = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let mut limits = lim();
    limits.max_json_bytes = 16 * 1024 * 1024;
    limits.memory_bytes = 128 * 1024 * 1024;
    limits.instruction_budget = 20_000_000;
    let host = Host::new(limits, reg_with_expand())
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

#[test]
fn one_direct_exact_expand_bypasses_budget_but_broad_parent_cannot() {
    let store = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let reference = format!("tz://blob/{}", "a".repeat(64));
    let text = "exact expansion bytes\n".repeat(256);
    let worker_result = json!({
        "metadata": {
            "effect":"read_only",
            "approval":{"state":"not_required"},
            "revert":{"supported":false},
            "ownership":{"engine":"tokenzero","session_id":"session-1","refs":[]},
            "trace":{}
        },
        "value": {
            "op":"tz_expand",
            "status":"ok",
            "mode":"exact",
            "visible":text,
            "tool_response": {
                "tool":"expand",
                "status":"ok",
                "mode":"exact",
                "visible":{"kind":"capsule","text":text},
                "recovery":{"do_not_recompact":true,"exact_bytes":true,"terminal":true}
            }
        }
    });
    let encoded = serde_json::to_string(&worker_result).unwrap();
    let connector = || {
        Rc::new(C {
            calls: RefCell::new(Vec::new()),
            fail: false,
            delay: Duration::ZERO,
            result: Some(encoded.clone()),
        })
    };
    let registration = GlobalRegistration::zero(vec![CapabilityDescriptor::new("token", "expand")]);
    let host = Host::new(lim(), registration)
        .unwrap_or_else(|error| panic!("host: {error}"))
        .with_visible_result_budget(512)
        .unwrap_or_else(|error| panic!("result budget: {error}"))
        .with_result_spill(store.path());
    let reference_literal = serde_json::to_string(&reference).unwrap();

    let exact = host
        .execute(
            &format!("return await zero.token.expand({reference_literal});"),
            connector(),
        )
        .unwrap_or_else(|error| panic!("direct expand: {error}"));
    assert_ne!(exact["spilled"], true);
    assert_eq!(exact["content"]["kind"], "inline");
    assert_eq!(exact["content"]["value"]["value"]["visible"], text);
    assert!(serde_json::to_vec(&exact).unwrap().len() > 512);

    let broad = host
        .execute(
            &format!(
                "const expanded=await zero.token.expand({reference_literal});return {{expanded}};"
            ),
            connector(),
        )
        .unwrap_or_else(|error| panic!("broad parent: {error}"));
    assert_eq!(broad["spilled"], true);
    let sha = broad["sha256"].as_str().unwrap();
    let stored = SharedCas::open(store.path()).get_verified(sha).unwrap();
    let recovered: Value = serde_json::from_slice(&stored).unwrap();
    assert_eq!(
        recovered["expanded"]["content"]["value"]["value"]["visible"],
        text
    );

    let noncanonical = host
        .execute(
            r#"return await zero.token.expand("tz://blob/short");"#,
            connector(),
        )
        .unwrap_or_else(|error| panic!("noncanonical ref: {error}"));
    assert_eq!(noncanonical["spilled"], true);

    let forged = host
        .execute(&format!("return {encoded};"), Rc::new(C::ok()))
        .unwrap_or_else(|error| panic!("forged expansion shape: {error}"));
    assert_eq!(forged["spilled"], true);
}

#[test]
fn spill_root_without_expand_capability_fails_before_publication() {
    let store = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let mut limits = lim();
    limits.max_json_bytes = 16 * 1024;
    let host = Host::new(limits, reg())
        .unwrap_or_else(|error| panic!("host: {error}"))
        .with_visible_result_budget(64)
        .unwrap_or_else(|error| panic!("result budget: {error}"))
        .with_result_spill(store.path());
    let error = host
        .execute("return 'x'.repeat(128);", Rc::new(C::ok()))
        .expect_err("spill without expansion authority");
    assert!(
        error
            .to_string()
            .contains("token.expand capability is required"),
        "{error}"
    );
    assert_eq!(std::fs::read_dir(store.path()).unwrap().count(), 0);
}

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

#[test]
fn every_route_emits_only_zero_result_v1_without_alias_synthesis() {
    let route_shapes = [
        (
            json!({"text":"hi\n","stdout_ref":"tz://blob/a","exit_code":0}),
            "ok",
        ),
        (
            json!({"visible":"hi\n","result":"hi\n","ref":"tz://blob/a","status":"ok"}),
            "ok",
        ),
        (
            json!({"value":{"ack":"R","content":{"kind":"inline","value":{"text":"hi\n"}}},"metadata":{"engine":"tokenzero"}}),
            "R",
        ),
    ];
    for (shape, expected_ack) in route_shapes {
        let connector = Rc::new(C {
            calls: RefCell::new(vec![]),
            fail: false,
            delay: Duration::ZERO,
            result: Some(serde_json::to_string(&shape).unwrap()),
        });
        let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
        let value = host
            .execute(
                "const r=await zero.fs.read({});return {keys:Object.keys(r).sort(),ack:r.ack,kind:r.content.kind,value:r.content.value};",
                connector,
            )
            .unwrap_or_else(|error| panic!("execute {shape}: {error}"));
        assert_eq!(value["keys"], json!(["ack", "content"]));
        assert_eq!(value["ack"], expected_ack);
        assert_eq!(value["kind"], "inline");
        assert_eq!(value["value"], shape);
    }
}

#[test]
fn explicit_ref_content_stays_ref_content() {
    let reference = format!("tz://blob/{}", "a".repeat(64));
    let connector = Rc::new(C {
        calls: RefCell::new(vec![]),
        fail: false,
        delay: Duration::ZERO,
        result: Some(
            serde_json::to_string(&json!({
                "value": {
                    "tool_response": {
                        "ack":"R",
                        "visible":{"kind":"ref","ref":reference,"preview":"bounded"}
                    }
                }
            }))
            .unwrap(),
        ),
    });
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let value = host
        .execute("return await zero.fs.read({});", connector)
        .unwrap_or_else(|error| panic!("execute: {error}"));
    assert_eq!(
        value,
        json!({"ack":"R","content":{"kind":"ref","ref":reference,"preview":"bounded"}})
    );
}

#[test]
fn malformed_declared_or_explicit_ref_results_fail_closed() {
    let shapes = [
        json!({"ack":"","content":{"kind":"inline","value":1}}),
        json!({"ack":"R","content":{"kind":"bogus","ref":format!("tz://blob/{}", "a".repeat(64))}}),
        json!({"ack":"R","content":{"ref":format!("tz://blob/{}", "a".repeat(64))}}),
        json!({"content":{"kind":"inline","value":1}}),
        json!({"value":{"ack":"","content":{"kind":"inline","value":1}}}),
        json!({"value":{"content":{"kind":"bogus","value":1}}}),
        json!({"value":{"tool_response":{"ack":"R","visible":{"kind":"ref"}}}}),
        json!({"value":{"tool_response":{"ack":"R","visible":{"kind":"ref","ref":7}}}}),
        json!({"value":{"tool_response":{"ack":"R","visible":{"kind":"ref","ref":format!("tz://blob/{}", "a".repeat(64)),"preview":7}}}}),
        json!({"value":{"tool_response":{"ack":"R","visible":{"kind":"ref","ref":"not-a-ref"}}}}),
        json!({"value":{"tool_response":{"ack":"R","visible":{"kind":"ref","ref":format!("tz://blob/{}", "a".repeat(64)),"preview":"x".repeat(zero_abi::MAX_PREVIEW_CHARS + 1)}}}}),
    ];
    for shape in shapes {
        let connector = Rc::new(C {
            calls: RefCell::new(vec![]),
            fail: false,
            delay: Duration::ZERO,
            result: Some(serde_json::to_string(&shape).unwrap()),
        });
        let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
        let error = host
            .execute("return await zero.fs.read({});", connector)
            .expect_err("malformed public result must fail");
        assert!(matches!(error, HostError::Json(_)), "{error}");
    }
}

#[test]
fn legacy_or_wrong_result_access_fails_loud() {
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    for property in ["text", "ref", "visible", "stdout_ref"] {
        let error = host
            .execute(
                &format!("const r=await zero.fs.read({{}});return r.{property};"),
                Rc::new(C::ok()),
            )
            .expect_err("legacy accessor must fail");
        assert!(
            error
                .to_string()
                .contains(&format!("unknown property '{property}'")),
            "{error}"
        );
    }
}

#[test]
fn public_result_field_names_are_published() {
    assert_eq!(PUBLIC_RESULT_FIELDS, &["ack", "content"]);
}

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
             return {thenable, a: ra.content.value.echo.path, b: rb.content.value.echo.path};"#,
            connector.clone(),
        )
        .unwrap_or_else(|error| panic!("promise.all plan: {error}"));
    assert_eq!(value, json!({"thenable": true, "a": "a", "b": "b"}));
    assert_eq!(connector.calls.borrow().len(), 2);
}

#[test]
fn promise_all_dispatches_concurrently_and_settles_fifo_completions() {
    const CALLS: usize = 6;
    let connector = Rc::new(CoordinatedConnector {
        expected: CALLS,
        completions: RefCell::new(Vec::new()),
    });
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let value = host
        .execute(
            r#"const completionOrder = [];
               const calls = Array.from({length: 6}, (_, sequence) =>
                 zero.fs.read({sequence}).then(value => {
                   completionOrder.push(value.content.value.sequence);
                   return value.content.value.sequence;
                 }));
               const values = await Promise.all(calls);
               return {completionOrder, values};"#,
            connector,
        )
        .unwrap_or_else(|error| panic!("concurrent plan: {error}"));
    assert_eq!(
        value,
        json!({
            "completionOrder": [0, 1, 2, 3, 4, 5],
            "values": [0, 1, 2, 3, 4, 5],
        })
    );
}

#[test]
fn promise_race_leaves_no_asynchronous_connector_tail() {
    let completions = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let connector = Rc::new(TailConnector {
        completions: Arc::clone(&completions),
    });
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let started = Instant::now();
    let value = host
        .execute(
            "return await Promise.race([zero.fs.read({sequence:0}),zero.fs.read({sequence:1})]);",
            connector,
        )
        .unwrap_or_else(|error| panic!("race plan: {error}"));
    assert_eq!(value["content"]["value"]["sequence"], 0);
    assert_eq!(completions.load(Ordering::Acquire), 2);
    assert!(
        started.elapsed() >= Duration::from_millis(15),
        "host returned before the losing dispatch settled"
    );
}

#[test]
fn connector_inflight_capacity_applies_bounded_backpressure() {
    let connector = Rc::new(C::ok());
    let mut limits = lim();
    limits.microtask_ceiling = 1_024;
    limits.max_inflight_connector_calls = 4;
    let host = Host::new(limits, reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let calls = 9;
    let plan = format!(
        "const calls=Array.from({{length:{calls}}},(_,sequence)=>zero.fs.read({{sequence}}));return await Promise.all(calls);"
    );
    let outcome = host.execute_measured(&plan, connector.clone());
    let values = outcome
        .result
        .unwrap_or_else(|error| panic!("backpressured plan: {error}"));
    assert_eq!(values.as_array().map(Vec::len), Some(calls));
    assert_eq!(connector.calls.borrow().len(), calls);
    assert_eq!(outcome.metrics.logical_operations, calls as u64);
    assert_eq!(outcome.metrics.connector_dispatches, calls as u64);
    assert_eq!(outcome.metrics.physical_dispatches, calls as u64);
    assert_eq!(outcome.metrics.peak_inflight_connector_calls, 4);
    assert!(outcome.metrics.backpressure_events >= 2);
    assert_eq!(
        outcome.metrics.first_saturation_cause.as_deref(),
        Some("connector_concurrency")
    );
}

#[test]
fn connector_promise_retention_obeys_the_memory_budget() {
    let connector = Rc::new(C::ok());
    let mut limits = lim();
    limits.instruction_budget = 100_000;
    limits.microtask_ceiling = 10_000;
    limits.max_inflight_connector_calls = 5_000;
    let host = Host::new(limits, reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let outcome = host.execute_measured(
        "const calls=Array.from({length:5000},(_,sequence)=>zero.fs.read({sequence}));return await Promise.all(calls);",
        connector.clone(),
    );
    let error = outcome
        .result
        .expect_err("retained promises must fit the memory budget");
    assert!(matches!(error, HostError::MemoryLimit { .. }), "{error}");
    assert_eq!(connector.calls.borrow().len(), 4_096);
    assert_eq!(outcome.metrics.peak_retained_promises, 4_096);
    assert_eq!(
        outcome.metrics.first_saturation_cause.as_deref(),
        Some("memory_budget")
    );
}

#[test]
fn settled_calls_do_not_impose_a_logical_operation_ceiling() {
    let connector = Rc::new(C::ok());
    let mut limits = lim();
    limits.instruction_budget = 100_000;
    let host = Host::new(limits, reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let outcome = host.execute_measured(
        r#"for (let sequence = 0; sequence < 128; sequence += 1) {
                 await zero.fs.read({sequence});
               }
               return {count: 128};"#,
        connector.clone(),
    );
    let value = outcome
        .result
        .unwrap_or_else(|error| panic!("sequential aggregate plan: {error}"));

    assert_eq!(value, json!({"count": 128}));
    assert_eq!(connector.calls.borrow().len(), 128);
    assert_eq!(outcome.metrics.logical_operations, 128);
    assert_eq!(outcome.metrics.connector_dispatches, 128);
    assert_eq!(outcome.metrics.peak_inflight_connector_calls, 1);
    assert_eq!(outcome.metrics.peak_retained_promises, 1);
    assert_eq!(
        connector
            .calls
            .borrow()
            .last()
            .and_then(|call| call["sequence"].as_u64()),
        Some(127)
    );
}

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

#[test]
fn cyclic_json_stringify_is_rejected_with_typed_error() {
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let error = host
        .execute(
            r#"const value = {name: "loop"}; value.self = value; return JSON.stringify(value);"#,
            Rc::new(C::ok()),
        )
        .expect_err("cyclic JSON.stringify must be rejected");
    assert!(matches!(error, HostError::Data(_)), "{error}");
    assert!(error.to_string().contains("cyclic"), "{error}");
}

#[test]
fn cyclic_array_to_string_falls_back_deterministically() {
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let value = host
        .execute(
            "const a = []; a.push(a); return String(a);",
            Rc::new(C::ok()),
        )
        .unwrap_or_else(|error| panic!("cyclic String(a): {error}"));
    assert_eq!(value, json!(""));
}

#[test]
fn deep_nested_evaluation_is_capped_with_typed_error() {
    let mut limits = lim();
    limits.stack_bytes = 16 * 1024; // derived depth ceiling = 8
    let host = Host::new(limits, reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let plan = format!("return {};", "[[[[[[[[[[1]]]]]]]]]]");
    let error = host
        .execute(&plan, Rc::new(C::ok()))
        .expect_err("deep nesting must be rejected");
    assert!(matches!(error, HostError::Data(_)), "{error}");
    assert!(error.to_string().contains("depth"), "{error}");
    // Bounded nesting still evaluates and serializes with the same limits.
    let value = host
        .execute("return [[[1]]];", Rc::new(C::ok()))
        .unwrap_or_else(|error| panic!("bounded nesting: {error}"));
    assert_eq!(value, json!([[[1]]]));
}

#[test]
fn array_from_huge_length_is_rejected_before_allocation() {
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let outcome = host.execute_measured(
        "return Array.from({length: 1000000000000});",
        Rc::new(C::ok()),
    );
    let error = outcome
        .result
        .expect_err("huge array-like length must be rejected");
    assert!(matches!(error, HostError::MemoryLimit { .. }), "{error}");
    assert!(error.to_string().contains("memory budget"), "{error}");
    assert_eq!(
        outcome.metrics.first_saturation_cause.as_deref(),
        Some("memory_budget")
    );
}

#[test]
fn array_from_length_above_remaining_fuel_is_rejected() {
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    // 100_000 elements fit the 16 MiB memory limit but exceed the 10_000
    // instruction budget, so the pre-flight fuel check must fire.
    let error = host
        .execute("return Array.from({length: 100000});", Rc::new(C::ok()))
        .expect_err("length above remaining fuel must be rejected");
    assert_eq!(error, HostError::FuelExhausted);
}

#[test]
fn array_from_bounded_length_stays_compatible() {
    let host = Host::new(lim(), reg()).unwrap_or_else(|error| panic!("host: {error}"));
    let value = host
        .execute(
            "return Array.from({length: 100}, (_, i) => i * 2);",
            Rc::new(C::ok()),
        )
        .unwrap_or_else(|error| panic!("bounded Array.from: {error}"));
    let array = value.as_array().unwrap();
    assert_eq!(array.len(), 100);
    assert_eq!(array[99], json!(198));
}
