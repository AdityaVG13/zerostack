    use super::*;
    use crate::host::ConnectorError;
    use crate::{CapabilityDescriptor, GlobalRegistration, HostLimits};

    struct NullConnector;

    impl Connector for NullConnector {
        fn dispatch(
            &self,
            _: &CapabilityDescriptor,
            _: &str,
            _: DispatchContext,
            _: ConnectorCompletion,
        ) -> Result<(), ConnectorError> {
            Ok(())
        }
    }

    fn test_host(stack_bytes: usize, instruction_budget: u64) -> Host {
        let limits = HostLimits::new(
            16 * 1024 * 1024,
            stack_bytes,
            Duration::from_secs(2),
            instruction_budget,
            64,
            crate::MAX_INFLIGHT_CONNECTOR_CALLS,
            256 * 1024,
            16 * 1024 * 1024,
        )
        .unwrap();
        Host::new(limits, GlobalRegistration::zero(vec![])).unwrap()
    }

    fn test_interpreter(host: &Host) -> Interpreter<'_> {
        let mut parser = Parser::new();
        parser.set_language(&LANGUAGE.into()).unwrap();
        let tree = parser.parse("", None).unwrap();
        let tree: &'static tree_sitter::Tree = Box::leak(Box::new(tree));
        Interpreter::new(
            host,
            "",
            tree.root_node(),
            Rc::new(NullConnector),
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(2),
            0,
            0,
        )
    }

    #[test]
    fn execute_reuses_the_thread_local_parser() {
        let host = test_host(256 * 1024, 100_000);
        host.execute("return 1;", Rc::new(NullConnector)).unwrap();
        let first = PARSER.with(|slot| {
            slot.borrow()
                .as_ref()
                .map(|parser| std::ptr::from_ref(parser) as usize)
        });
        host.execute("return 2;", Rc::new(NullConnector)).unwrap();
        let second = PARSER.with(|slot| {
            slot.borrow()
                .as_ref()
                .map(|parser| std::ptr::from_ref(parser) as usize)
        });
        assert!(first.is_some());
        assert_eq!(first, second);
        let creations = PARSER_CREATIONS.load(Ordering::Relaxed);
        let interp = INTERPRETER_CREATIONS.load(Ordering::Relaxed);
        eprintln!(
            "CHAR tls parser_slot={:#x} parser_creations={creations} interpreter_creations={interp}",
            first.unwrap()
        );
        assert_ne!(
            std::ptr::from_ref(&PARSER_CREATIONS) as usize,
            std::ptr::from_ref(&INTERPRETER_CREATIONS) as usize
        );
    }

    #[test]
    fn ctx_step_seals_session_bound_receipt() {
        let host = test_host(256 * 1024, 100_000);
        let output = host
            .execute_with_cancel_timeout_context(
                "return ctx.step('gate', () => ({value: 42}));",
                Rc::new(NullConnector),
                Arc::new(AtomicBool::new(false)),
                Duration::from_secs(2),
                7,
                11,
            )
            .unwrap();
        assert_eq!(output["result"]["value"], 42);
        assert_eq!(output["step_receipt"]["generation"], 7);
        assert_eq!(output["step_receipt"]["request_id"], 11);
        assert_eq!(output["step_receipt"]["step_count"], 1);
        let receipt_sha = output["step_receipt"]["receipt_sha256"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut receipt_body = output["step_receipt"].clone();
        receipt_body
            .as_object_mut()
            .unwrap()
            .remove("receipt_sha256");
        assert_eq!(
            receipt_sha,
            sha256_hex(canonical_json(&receipt_body).as_bytes())
        );
        let entry = &output["step_receipt"]["steps"][0];
        assert_eq!(entry["entry_sha256"], output["step_receipt"]["head_sha256"]);
    }

    #[test]
    fn ctx_step_rejects_invalid_callback_and_name_bounds() {
        let host = test_host(256 * 1024, 100_000);
        let connector = Rc::new(NullConnector);
        let callback = host.execute("return ctx.step('gate', 42);", connector.clone());
        assert!(
            callback
                .unwrap_err()
                .to_string()
                .contains("callback function")
        );
        let name = "x".repeat(257);
        let error = host
            .execute(&format!("return ctx.step('{name}', () => 1);"), connector)
            .unwrap_err();
        assert!(error.to_string().contains("at most 256 bytes"));
    }

    #[test]
    fn depth_guard_unwinds_on_every_return_path() {
        let depth = Rc::new(Cell::new(0usize));
        {
            let _outer = DepthGuard::enter(&depth, 8).unwrap();
            assert_eq!(depth.get(), 1);
            {
                let _inner = DepthGuard::enter(&depth, 8).unwrap();
                assert_eq!(depth.get(), 2);
                drop(_inner);
                assert_eq!(depth.get(), 1);
            }
        }
        assert_eq!(depth.get(), 0);
    }

    #[test]
    fn depth_guard_rejects_past_limit_and_keeps_counter_stable() {
        let depth = Rc::new(Cell::new(8usize));
        let error = match DepthGuard::enter(&depth, 8) {
            Ok(_) => panic!("depth guard must reject entries past the limit"),
            Err(error) => error,
        };
        assert!(matches!(error, HostError::Data(_)));
        assert!(error.to_string().contains("depth"));
        assert_eq!(depth.get(), 8);
    }

    #[test]
    fn depth_guard_unwinds_after_early_error_return() {
        let depth = Rc::new(Cell::new(0usize));
        fn probe(depth: &Rc<Cell<usize>>, fail: bool) -> Result<(), HostError> {
            let _guard = DepthGuard::enter(depth, 16)?;
            if fail {
                return Err(HostError::Data("boom".into()));
            }
            Ok(())
        }
        let _ = probe(&depth, true);
        assert_eq!(depth.get(), 0);
        probe(&depth, false).unwrap();
        assert_eq!(depth.get(), 0);
    }

    #[test]
    fn to_json_rejects_cycles_before_recursion() {
        let host = test_host(256 * 1024, 100_000);
        let interpreter = test_interpreter(&host);
        let value = Value::Object(Rc::new(RefCell::new(ObjectValue {
            fields: BTreeMap::new(),
            getters: BTreeMap::new(),
            access: ObjectAccess::Open,
        })));
        if let Value::Object(object) = &value {
            object
                .borrow_mut()
                .fields
                .insert("self".into(), value.clone());
        }
        let error = interpreter.to_json(&value).unwrap_err();
        assert!(matches!(error, HostError::Data(_)));
        assert!(error.to_string().contains("cyclic"));
    }

    #[test]
    fn shorthand_object_destructure_binds_the_property() {
        let host = test_host(256 * 1024, 100_000);
        let output = host
            .execute(
                "const { x } = { x: 7, y: 9 }; return x;",
                Rc::new(NullConnector),
            )
            .unwrap();
        assert_eq!(output, serde_json::json!(7));
        let renamed = host
            .execute(
                "const { x: y } = { x: 3 }; return y;",
                Rc::new(NullConnector),
            )
            .unwrap();
        assert_eq!(renamed, serde_json::json!(3));
    }

    #[test]
    fn nested_destructure_of_cyclic_object_does_not_panic() {
        let host = test_host(256 * 1024, 100_000);
        let output = host
            .execute(
                "const obj = { x: 7 }; obj.self = obj; const { self: { x: y } } = obj; return y;",
                Rc::new(NullConnector),
            )
            .unwrap();
        assert_eq!(output, serde_json::json!(7));
    }

    #[test]
    fn nested_destructure_of_cyclic_array_does_not_panic() {
        let host = test_host(256 * 1024, 100_000);
        let output = host
            .execute(
                "const arr = []; arr.push(arr); const [[x]] = arr; return typeof x;",
                Rc::new(NullConnector),
            )
            .unwrap();
        assert_eq!(output, serde_json::json!("object"));
    }

    #[test]
    fn define_property_with_self_descriptor_does_not_panic() {
        let host = test_host(256 * 1024, 100_000);
        let output = host
            .execute(
                "const o = { value: 4 }; Object.defineProperty(o, 'x', o); return o.x;",
                Rc::new(NullConnector),
            )
            .unwrap();
        assert_eq!(output, serde_json::json!(4));
    }

    #[test]
    fn json_parse_rejects_input_over_max_json_bytes() {
        let limits = HostLimits::new(
            16 * 1024 * 1024,
            256 * 1024,
            Duration::from_secs(2),
            100_000,
            64,
            crate::MAX_INFLIGHT_CONNECTOR_CALLS,
            256 * 1024,
            32,
        )
        .unwrap();
        let host = Host::new(limits, GlobalRegistration::zero(vec![])).unwrap();
        let payload = format!("\"{}\"", "x".repeat(64));
        let error = host
            .execute(
                &format!("return JSON.parse({payload:?});"),
                Rc::new(NullConnector),
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("JSON.parse input exceeds JSON limit"),
            "unexpected error: {error}"
        );
        let ok = host
            .execute("return JSON.parse('{\"a\":1}');", Rc::new(NullConnector))
            .unwrap();
        assert_eq!(ok, serde_json::json!({"a": 1}));
    }

    #[test]
    fn from_json_caps_depth_with_typed_error() {
        let host = test_host(16 * 1024, 100_000); // derived ceiling = 8
        let interpreter = test_interpreter(&host);
        let mut json = JsonValue::Null;
        for _ in 0..32 {
            json = JsonValue::Array(vec![json]);
        }
        let error = interpreter.convert_from_json(json, false).unwrap_err();
        assert!(matches!(error, HostError::Data(_)));
        assert!(error.to_string().contains("depth"));
    }

    #[test]
    fn from_json_accepts_bounded_nesting() {
        let host = test_host(16 * 1024, 100_000); // derived ceiling = 8
        let interpreter = test_interpreter(&host);
        let mut json = JsonValue::Null;
        for _ in 0..4 {
            json = JsonValue::Array(vec![json]);
        }
        let value = interpreter.convert_from_json(json, false).unwrap();
        assert!(matches!(value, Value::Array(_)));
    }
    fn nested_array<'tree>(depth: usize) -> Value<'tree> {
        let mut value = Value::Number(1.0);
        for _ in 0..depth {
            value = new_array(vec![value]);
        }
        value
    }

    #[test]
    fn public_serialization_caps_depth_with_typed_error() {
        let host = test_host(256 * 1024, 100_000); // derived ceiling = 128
        let mut interpreter = test_interpreter(&host);
        let value = nested_array(160);
        let error = interpreter.serialize_public_json(&value).unwrap_err();
        assert!(matches!(error, HostError::Data(_)));
        assert!(error.to_string().contains("serialization depth"));
    }

    #[test]
    fn public_serialization_accepts_bounded_nesting() {
        let host = test_host(256 * 1024, 100_000); // derived ceiling = 128
        let mut interpreter = test_interpreter(&host);
        let value = nested_array(64);
        let (json, degraded) = interpreter.serialize_public_json(&value).unwrap();
        assert!(!degraded);
        let mut node = &json;
        for _ in 0..64 {
            node = node.as_array().unwrap().first().unwrap();
        }
        assert_eq!(node, &serde_json::json!(1));
        eprintln!("CHAR serialize depth=64 degraded=0 owner=interpreter");
    }
    #[test]
    fn promise_resolution_recursion_is_depth_bounded() {
        let host = test_host(16 * 1024, 100_000); // derived ceiling = 8
        let mut interpreter = test_interpreter(&host);
        interpreter
            .promises
            .insert(0, PromiseState::Fulfilled(Value::Number(1.0)));
        for id in 1..=16 {
            interpreter
                .promises
                .insert(id, PromiseState::Pending(PromiseKind::All(vec![id - 1])));
        }

        let error = match interpreter.resolve(16) {
            Ok(_) => panic!("nested promise resolution must be depth bounded"),
            Err(Fault::Host(error)) => error,
            Err(Fault::Throw(_)) => panic!("nested promise resolution returned a thrown value"),
        };
        assert!(matches!(error, HostError::Data(_)));
        assert!(error.to_string().contains("depth"));
        assert_eq!(interpreter.depth.get(), 0);
        eprintln!(
            "CHAR promise settle=16 kind=All state=depth_bounded inflight=0 depth={}",
            interpreter.depth.get()
        );
    }

    #[test]
    fn promise_pump_entry_obeys_depth_limit() {
        let host = test_host(16 * 1024, 100_000); // derived ceiling = 8
        let mut interpreter = test_interpreter(&host);
        interpreter
            .promises
            .insert(1, PromiseState::Fulfilled(Value::Number(1.0)));
        interpreter.depth.set(interpreter.max_depth);

        let error = interpreter
            .pump(1)
            .expect_err("promise pump must share the interpreter depth limit");
        assert!(matches!(error, HostError::Data(_)));
        assert!(error.to_string().contains("depth"));
        assert_eq!(interpreter.depth.get(), interpreter.max_depth);
        eprintln!(
            "CHAR promise settle=1 kind=Fulfilled state=depth_limit inflight=0 depth={}",
            interpreter.depth.get()
        );
    }
