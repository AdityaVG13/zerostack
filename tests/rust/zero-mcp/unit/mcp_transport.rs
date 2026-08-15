    use super::*;
    use serde_json::json;

    #[test]
    fn mcp_registration_requires_the_mcp_surface() {
        let registration = test_registration(SurfaceKind::CodeMode);
        assert!(matches!(
            validate_mcp_registration(&registration),
            Err(McpTransportError::WrongSurface(SurfaceKind::CodeMode))
        ));
        assert!(validate_mcp_registration(&test_registration(SurfaceKind::Mcp)).is_ok());
        assert!(matches!(
            McpServerIdentity::new("", "1.0.0"),
            Err(McpTransportError::InvalidServerIdentity(_))
        ));
        assert!(matches!(
            McpServerIdentity::new("tokenzero", " 1.4.0"),
            Err(McpTransportError::InvalidServerIdentity(_))
        ));
    }

    #[test]
    fn structured_success_and_error_text_are_lossless() {
        let value = json!({"ack":"ok", "content":{"kind":"inline", "value":{"n":1}}});
        let callback_value = value.clone();
        let success = execute_call(
            Arc::new(move |_: &str, _: Value, _: &McpCallContext| Ok(callback_value.clone())),
            "fs.read",
            json!({}),
            McpTransportConfig::default(),
        )
        .unwrap();
        assert_eq!(success, value);

        let error = McpDispatchError::new("denied", "approval required", false)
            .with_op("fs.read")
            .with_data(json!({"approval_id":"a1"}));
        let round_trip: McpDispatchError = serde_json::from_str(&error.wire_text()).unwrap();
        assert_eq!(round_trip, error);
    }

    #[test]
    fn callback_timeout_sets_cancellation_and_returns_bounded_error() {
        let observed = Arc::new(AtomicBool::new(false));
        let callback_observed = Arc::clone(&observed);
        let result = execute_call(
            Arc::new(move |_: &str, _: Value, context: &McpCallContext| {
                while !context.is_cancelled() {
                    thread::sleep(Duration::from_millis(2));
                }
                callback_observed.store(true, Ordering::Release);
                Err(McpDispatchError::cancelled())
            }),
            "fs.read",
            json!({}),
            McpTransportConfig {
                tool_timeout: Duration::from_millis(25),
                max_inflight: 1,
            },
        );
        assert_eq!(result.unwrap_err().kind, "timeout");
        for _ in 0..50 {
            if observed.load(Ordering::Acquire) {
                return;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("callback did not observe timeout cancellation");
    }

    #[test]
    fn external_cancellation_is_reported_without_waiting_for_timeout() {
        let started = Instant::now();
        let result = execute_call_with_cancel(
            Arc::new(|_: &str, _: Value, context: &McpCallContext| {
                while !context.is_cancelled() {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(McpDispatchError::cancelled())
            }),
            "fs.read",
            json!({}),
            McpTransportConfig::default(),
            move || started.elapsed() >= Duration::from_millis(25),
        );
        assert_eq!(result.unwrap_err().kind, "cancelled");
        assert!(started.elapsed() < Duration::from_millis(250));
    }

    #[test]
    fn late_ok_after_cancel_is_commit_race_with_payload() {
        let started = Instant::now();
        let payload = json!({"committed": true, "n": 7});
        let callback_payload = payload.clone();
        let result = execute_call_with_cancel(
            Arc::new(move |_: &str, _: Value, context: &McpCallContext| {
                while !context.is_cancelled() {
                    thread::sleep(Duration::from_millis(2));
                }
                thread::sleep(Duration::from_millis(15));
                Ok(callback_payload.clone())
            }),
            "fs.write",
            json!({}),
            McpTransportConfig::default(),
            move || started.elapsed() >= Duration::from_millis(20),
        );
        let error = result.expect_err("late Ok after cancel must not be Success");
        assert_eq!(error.kind, "commit_race");
        assert!(!error.retryable);
        assert_eq!(
            error.data.as_ref().and_then(|data| data.get("result")),
            Some(&payload),
            "commit_race must keep the committed payload: {:?}",
            error.data
        );
    }

    #[test]
    fn late_err_after_cancel_stays_that_err() {
        let started = Instant::now();
        let result = execute_call_with_cancel(
            Arc::new(|_: &str, _: Value, context: &McpCallContext| {
                while !context.is_cancelled() {
                    thread::sleep(Duration::from_millis(2));
                }
                thread::sleep(Duration::from_millis(15));
                Err(McpDispatchError::new("denied", "approval required", false)
                    .with_op("fs.write")
                    .with_data(json!({"approval_id": "a1"})))
            }),
            "fs.write",
            json!({}),
            McpTransportConfig::default(),
            move || started.elapsed() >= Duration::from_millis(20),
        );
        let error = result.expect_err("late domain Err must stay Err");
        assert_eq!(error.kind, "denied");
        assert_eq!(
            error.data,
            Some(json!({"approval_id": "a1"})),
            "{error:?}"
        );
    }

    #[test]
    fn cancel_without_late_result_attaches_still_running_receipt() {
        let started = Instant::now();
        let result = execute_call_with_cancel(
            Arc::new(|_: &str, _: Value, context: &McpCallContext| {
                while !context.is_cancelled() {
                    thread::sleep(Duration::from_millis(2));
                }
                thread::sleep(Duration::from_millis(400));
                Ok(json!({"too_late": true}))
            }),
            "fs.write",
            json!({}),
            McpTransportConfig::default(),
            move || started.elapsed() >= Duration::from_millis(20),
        );
        let error = result.expect_err("unfinished worker is not Success");
        assert_eq!(error.kind, "cancelled");
        assert_eq!(
            error.data.as_ref().and_then(|data| data.get("still_running")),
            Some(&json!(true)),
            "empty late channel must not silent-detach: {:?}",
            error.data
        );
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "handler must not wait out the unfinished worker: {:?}",
            started.elapsed()
        );
    }

    #[cfg(feature = "fastmcp")]
    #[test]
    fn fastmcp_catalog_and_tools_call_preserve_structured_results() {
        use fastmcp_rust::{
            CallToolParams, CallToolResult, ClientCapabilities, ClientInfo, Content, Cx,
            JsonRpcRequest, NotificationSender, PendingRequests, RequestSender, ServerCapabilities,
            ServerInfo, Session,
        };

        struct TestDispatcher;
        impl McpDispatcher for TestDispatcher {
            fn dispatch(
                &self,
                _tool: &str,
                arguments: Value,
                _: &McpCallContext,
            ) -> Result<Value, McpDispatchError> {
                if arguments.get("fail").and_then(Value::as_bool) == Some(true) {
                    Err(McpDispatchError::new("denied", "approval required", false)
                        .with_op("fs.read")
                        .with_data(json!({"approval_id":"a1"})))
                } else {
                    Ok(json!({"value": arguments.get("value").cloned()}))
                }
            }

            fn dispatch_output(
                &self,
                tool: &str,
                arguments: Value,
                context: &McpCallContext,
            ) -> Result<McpDispatchOutput, McpDispatchError> {
                if arguments.get("lossless").and_then(Value::as_bool) == Some(true) {
                    return Ok(McpDispatchOutput::Text(vec![
                        McpTextContent::new("ack:exact"),
                        McpTextContent::new("metadata:compact"),
                    ]));
                }
                self.dispatch(tool, arguments, context)
                    .map(McpDispatchOutput::Json)
            }
        }

        let plain_error = super::fastmcp::present_dispatch_error(
            McpDispatchError::new("denied", "approval required", false),
            McpErrorPresentation::PlainMessage,
        );
        assert_eq!(i32::from(plain_error.code), -32_000);
        assert_eq!(plain_error.message, "approval required");
        let structured_error = super::fastmcp::present_dispatch_error(
            McpDispatchError::new("denied", "approval required", false),
            McpErrorPresentation::Structured,
        );
        let structured: McpDispatchError = serde_json::from_str(&structured_error.message).unwrap();
        assert_eq!(structured.kind, "denied");

        let transport = FastMcpTransport::new(
            test_registration(SurfaceKind::Mcp),
            Arc::new(TestDispatcher),
            McpTransportConfig::default(),
        )
        .unwrap()
        .with_server_identity("tokenzero", "1.4.0")
        .unwrap()
        .with_error_presentation(McpErrorPresentation::PlainMessage)
        .with_alias_metadata(vec![McpAliasMetadata {
            canonical_id: "fs.read".into(),
            name: "read".into(),
            description: Some("Alias summary".into()),
            input_schema: json!({"type":"object","additionalProperties":true}),
            output_schema: None,
        }])
        .unwrap();
        let catalog = transport.catalog();
        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog[0].name, "read_value");
        assert_eq!(catalog[1].name, "read");
        assert_eq!(catalog[0].description.as_deref(), Some("Read a value"));
        assert_eq!(catalog[0].input_schema, json!({"type":"object"}));
        assert_eq!(
            catalog[0].output_schema,
            Some(json!({
                "type": "object",
                "properties": {"value": {"type": "integer"}}
            }))
        );
        assert_eq!(catalog[1].description.as_deref(), Some("Alias summary"));
        assert_eq!(
            catalog[1].input_schema,
            json!({"type":"object","additionalProperties":true})
        );
        assert_eq!(catalog[1].output_schema, None);
        assert_eq!(
            catalog[0].annotations.as_ref().unwrap().read_only,
            Some(true)
        );

        let server = transport.build_server();
        assert_eq!(server.info().name, "tokenzero");
        assert_eq!(server.info().version, "1.4.0");
        let mut session = Session::new(
            ServerInfo {
                name: "zerostack".into(),
                version: "test".into(),
            },
            ServerCapabilities::default(),
        );
        session.initialize(
            ClientInfo {
                name: "test-client".into(),
                version: "test".into(),
            },
            ClientCapabilities::default(),
            "2025-06-18".into(),
        );
        let notification_sender: NotificationSender = Arc::new(|_| {});
        let request_sender =
            RequestSender::new(Arc::new(PendingRequests::new()), Arc::new(|_| Ok(())));
        let mut call = |name: &str, arguments: Value| {
            let request = JsonRpcRequest::new(
                "tools/call",
                Some(
                    serde_json::to_value(CallToolParams {
                        name: name.into(),
                        arguments: Some(arguments),
                        meta: None,
                    })
                    .unwrap(),
                ),
                1,
            );
            server
                .dispatch_request(
                    &Cx::for_testing(),
                    &mut session,
                    request,
                    &notification_sender,
                    &request_sender,
                )
                .unwrap()
        };

        let success: CallToolResult =
            serde_json::from_value(call("read_value", json!({"value":7})).result.unwrap()).unwrap();
        assert!(!success.is_error);
        assert_eq!(success.content.len(), 1);
        let Content::Text { text } = &success.content[0] else {
            panic!("FastMCP structured success must use text content");
        };
        assert_eq!(text, r#"{"value":7}"#);

        let alias_success: CallToolResult =
            serde_json::from_value(call("read", json!({"value":8})).result.unwrap()).unwrap();
        assert!(!alias_success.is_error);
        let lossless: CallToolResult =
            serde_json::from_value(call("read", json!({"lossless":true})).result.unwrap()).unwrap();
        assert_eq!(lossless.content.len(), 2);
        let Content::Text { text: primary } = &lossless.content[0] else {
            panic!("primary lossless content must remain text");
        };
        let Content::Text { text: metadata } = &lossless.content[1] else {
            panic!("secondary lossless content must remain text");
        };
        assert_eq!(primary, "ack:exact");
        assert_eq!(metadata, "metadata:compact");
        let failure: CallToolResult =
            serde_json::from_value(call("read", json!({"fail":true})).result.unwrap()).unwrap();
        assert!(failure.is_error);
        let Content::Text { text } = &failure.content[0] else {
            panic!("FastMCP plain error must use text content");
        };
        assert_eq!(text, "approval required");
    }

    #[test]
    fn configuration_preserves_engine_deadlines_and_rejects_invalid_bounds() {
        let result = execute_call(
            Arc::new(|_: &str, _: Value, context: &McpCallContext| {
                Ok(json!({"hub_deadline": context.deadline().is_some()}))
            }),
            "token.shell",
            json!({}),
            McpTransportConfig::default(),
        )
        .unwrap();
        assert_eq!(result, json!({"hub_deadline":false}));
        assert!(
            McpTransportConfig {
                tool_timeout: Duration::from_secs(3_600),
                max_inflight: 1,
            }
            .validate()
            .is_ok()
        );

        let result = execute_call(
            Arc::new(|_: &str, _: Value, _: &McpCallContext| Ok(json!(null))),
            "fs.read",
            json!({}),
            McpTransportConfig {
                tool_timeout: DEFAULT_MCP_TOOL_TIMEOUT,
                max_inflight: 0,
            },
        );
        assert_eq!(result.unwrap_err().kind, "invalid_config");

        let result = execute_call(
            Arc::new(|_: &str, _: Value, _: &McpCallContext| Ok(json!(null))),
            "fs.read",
            json!({}),
            McpTransportConfig {
                tool_timeout: DEFAULT_MCP_TOOL_TIMEOUT,
                max_inflight: MAX_MCP_MAX_INFLIGHT + 1,
            },
        );
        assert_eq!(result.unwrap_err().kind, "invalid_config");

        let result = execute_call(
            Arc::new(|_: &str, _: Value, _: &McpCallContext| Ok(json!(null))),
            "fs.read",
            json!({}),
            McpTransportConfig {
                tool_timeout: MAX_MCP_TOOL_TIMEOUT + Duration::from_secs(1),
                max_inflight: 1,
            },
        );
        assert_eq!(result.unwrap_err().kind, "invalid_config");
    }

    #[cfg(feature = "fastmcp")]
    #[test]
    fn fastmcp_resource_callback_failure_and_timeout_are_bounded() {
        use fastmcp_rust::ResourceHandler;
        use zero_abi::CanonicalResource;

        let resource = CanonicalResource {
            uri: "resource://fixture".into(),
            name: "Fixture".into(),
            description: "fixture resource".into(),
            mime_type: Some("application/json".into()),
        };
        let mut registration = test_registration(SurfaceKind::Mcp);
        registration.adapter.registry.resources = vec![resource.clone()];
        let reader: Arc<dyn McpResourceReader> = Arc::new(|uri: &str, context: &McpCallContext| {
            if uri == "resource://failure" {
                return Err(McpDispatchError::new(
                    "resource_failed",
                    "read failed",
                    false,
                ));
            }
            while !context.is_cancelled() {
                thread::sleep(Duration::from_millis(2));
            }
            Err(McpDispatchError::cancelled())
        });
        let missing_reader = FastMcpTransport::new(
            registration.clone(),
            Arc::new(|_: &str, _: Value, _: &McpCallContext| Ok(json!(null))),
            McpTransportConfig::default(),
        );
        assert!(matches!(
            missing_reader,
            Err(McpTransportError::MissingResourceReader)
        ));
        let transport = FastMcpTransport::with_resources(
            registration,
            Arc::new(|_: &str, _: Value, _: &McpCallContext| Ok(json!(null))),
            reader,
            McpTransportConfig {
                tool_timeout: Duration::from_millis(25),
                max_inflight: 1,
            },
        )
        .unwrap();
        assert_eq!(transport.resource_catalog()[0].uri, resource.uri);
        let success_handler = super::fastmcp::RegisteredResource::new(
            &resource,
            Arc::new(|_: &str, _: &McpCallContext| Ok(json!({"answer": 42}))),
            McpTransportConfig::default(),
            McpErrorPresentation::Structured,
            Arc::new(Inflight::new(1)),
        );
        let success = success_handler
            .read(&fastmcp_rust::McpContext::new(
                fastmcp_rust::Cx::for_testing(),
                1,
            ))
            .unwrap();
        assert_eq!(success.len(), 1);
        assert_eq!(success[0].uri, resource.uri);
        assert_eq!(success[0].mime_type, resource.mime_type);
        assert_eq!(success[0].text.as_deref(), Some(r#"{"answer":42}"#));

        struct ExactResourceReader;
        impl McpResourceReader for ExactResourceReader {
            fn read(&self, _: &str, _: &McpCallContext) -> Result<Value, McpDispatchError> {
                Ok(Value::Null)
            }

            fn read_output(
                &self,
                _: &str,
                _: &McpCallContext,
            ) -> Result<McpResourceOutput, McpDispatchError> {
                Ok(McpResourceOutput::Text("exact resource text".into()))
            }
        }
        let exact_handler = super::fastmcp::RegisteredResource::new(
            &resource,
            Arc::new(ExactResourceReader),
            McpTransportConfig::default(),
            McpErrorPresentation::Structured,
            Arc::new(Inflight::new(1)),
        );
        let exact = exact_handler
            .read(&fastmcp_rust::McpContext::new(
                fastmcp_rust::Cx::for_testing(),
                1,
            ))
            .unwrap();
        assert_eq!(exact[0].text.as_deref(), Some("exact resource text"));

        let handler = super::fastmcp::RegisteredResource::new(
            &resource,
            Arc::new(|_: &str, _: &McpCallContext| {
                Err(McpDispatchError::new(
                    "resource_failed",
                    "read failed",
                    false,
                ))
            }),
            McpTransportConfig::default(),
            McpErrorPresentation::Structured,
            Arc::new(Inflight::new(1)),
        );
        let error = handler
            .read(&fastmcp_rust::McpContext::new(
                fastmcp_rust::Cx::for_testing(),
                1,
            ))
            .unwrap_err();
        assert!(error.message.contains("resource_failed"));

        let timeout_handler = super::fastmcp::RegisteredResource::new(
            &resource,
            Arc::new(|_: &str, context: &McpCallContext| {
                while !context.is_cancelled() {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(McpDispatchError::cancelled())
            }),
            McpTransportConfig {
                tool_timeout: Duration::from_millis(25),
                max_inflight: 1,
            },
            McpErrorPresentation::Structured,
            Arc::new(Inflight::new(1)),
        );
        let started = Instant::now();
        let error = timeout_handler
            .read(&fastmcp_rust::McpContext::new(
                fastmcp_rust::Cx::for_testing(),
                1,
            ))
            .unwrap_err();
        assert!(error.message.contains("timeout") || error.message.contains("cancel"));
        assert!(started.elapsed() < Duration::from_millis(250));
    }

    fn test_registration(surface: SurfaceKind) -> SurfaceRegistration {
        use zero_abi::{
            ALL_DISPATCH_ERROR_CLASSES, ApprovalRequirement, CanonicalOperation, CanonicalRegistry,
            EffectClass, EffectPolicy, EngineIdentity, PermitRequirement, RefOwnership,
            RegistryEngine, TelemetrySchema,
        };
        use zero_abi::{CapabilityDescriptor, DomainAdapterRegistration};

        SurfaceRegistration::new(
            surface,
            "zero",
            DomainAdapterRegistration {
                engine: EngineIdentity::FsZero,
                registry: CanonicalRegistry {
                    version: zero_abi::CANONICAL_DISPATCH_VERSION.into(),
                    engine: RegistryEngine::FsZero,
                    operations: vec![CanonicalOperation {
                        canonical_id: "fs.read".into(),
                        description: "Read a value".into(),
                        aliases: vec!["read".into()],
                        args_schema: json!({"type":"object"}),
                        output_schema: Some(json!({
                            "type": "object",
                            "properties": {"value": {"type": "integer"}}
                        })),
                        mcp_tool_name: Some("read_value".into()),
                        effect_policy: EffectPolicy {
                            effect_class: EffectClass::ReadOnly,
                            permit: PermitRequirement::NotRequired,
                            approval: ApprovalRequirement::NotRequired,
                        },
                        errors: ALL_DISPATCH_ERROR_CLASSES.to_vec(),
                    }],
                    resources: vec![],
                },
                ref_ownership: RefOwnership {
                    engine: EngineIdentity::FsZero,
                    session_id: "session".into(),
                    refs: vec!["fz://ref".into()],
                    snapshot: None,
                },
                telemetry_schema: TelemetrySchema::V1,
                capabilities: vec![CapabilityDescriptor::new("fs", "read")],
            },
        )
    }
