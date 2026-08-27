//! JSON-RPC 2.0 and MCP conformance matrices.
mod conformance {
    use serde_json::{Value, json};
    use std::fs;
    use tempfile::TempDir;
    use tokenzero_mcp_compat::{EngineConfig, TokenZeroEngine, handle_jsonrpc};
    type ConformanceSection = (&'static str, fn(&Server));
    struct Server {
        dir: TempDir,
        engine: TokenZeroEngine,
    }
    impl Server {
        fn new() -> Self {
            let server = Self::uninitialized();
            server.complete_lifecycle();
            server
        }
        fn uninitialized() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let engine = TokenZeroEngine::new(EngineConfig::for_root(dir.path()));
            Self { dir, engine }
        }
        fn complete_lifecycle(&self) {
            let init = self.response(json!({
                "jsonrpc": "2.0",
                "id": "lifecycle-init",
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "conformance", "version": "1.0.0"}
                }
            }));
            assert!(
                init.get("result").is_some(),
                "lifecycle initialize failed: {init}"
            );
            assert!(
                self.raw(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                    .is_none(),
                "initialized notification must be response-suppressed"
            );
        }
        fn path(&self, name: &str) -> std::path::PathBuf {
            self.dir.path().join(name)
        }
        fn raw(&self, input: &str) -> Option<Value> {
            handle_jsonrpc(&self.engine, input).map(|text| serde_json::from_str(&text).unwrap())
        }
        fn response(&self, input: Value) -> Value {
            self.raw(&input.to_string())
                .unwrap_or_else(|| panic!("expected response to {input}"))
        }
        fn call(&self, id: &str, method: &str, params: Value) -> Value {
            self.response(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
        }
    }
    enum Input {
        Json(Value),
        Raw(&'static str),
    }
    enum Reply {
        None,
        Result(Value),
        Error(Value, i64),
        Batch(Vec<Reply>),
    }
    struct RpcCase {
        id: &'static str,
        input: Input,
        reply: Reply,
    }
    type InvalidCase = (String, &'static str, Value, &'static str);
    macro_rules! rpc_cases {
($($id:literal: $input:expr => $reply:expr;)+) => { [$(RpcCase { id: $id, input: $input, reply: $reply }),+] };
}
    macro_rules! invalid_cases {
($server:expr; $($prefix:expr, $id:literal, $description:literal, $request_id:literal, $method:literal, $params:expr, $kind:literal;)+) => {{
let cases = [$(invalid($prefix, $id, $description, $request_id, $method, $params, $kind)),+]; run_invalid($server, &cases);
}};
}
    macro_rules! error_cases {
($server:expr; $($id:literal: $input:expr => $code:literal, $response_id:expr, $kind:literal, [$($array:literal => $needle:literal),*] $(, $field:literal => $expected:expr)?;)+) => {$({
let payload = match $input { Input::Json(value) => value.to_string(), Input::Raw(text) => text.into() }; let actual = $server.raw(&payload).unwrap_or_else(|| panic!("{}: expected response", $id)); assert_error(&actual, &$response_id, $code).unwrap_or_else(|reason| panic!("{}: {}", $id, reason)); let data = &actual["error"]["data"]; assert_eq!(data["kind"], $kind, "{}: {}", $id, actual);
$(assert_eq!(data[$field], $expected, "{}: {}", $id, actual);)? $(assert!(array_has(data, $array, $needle), "{}: missing {} in {}", $id, $array, actual);)* })+};
}
    const P2025: &str = "MCP-2025-06-18-";
    const PDRAFT: &str = "MCP-DRAFT-";
    fn request(id: &str, method: &str, params: Value) -> Value {
        match params {
            Value::Null => json!({"jsonrpc":"2.0","id":id,"method":method}),
            params => json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
        }
    }
    fn invalid(
        prefix: &str,
        id: &str,
        description: &'static str,
        request_id: &str,
        method: &str,
        params: Value,
        kind: &'static str,
    ) -> InvalidCase {
        (
            format!("{prefix}{id}"),
            description,
            request(request_id, method, params),
            kind,
        )
    }
    #[test]
    fn jsonrpc_and_mcp_conformance() {
        let sections: &[ConformanceSection] = &[
            ("JSON-RPC envelopes", run_jsonrpc),
            ("protocol errors", run_protocol_errors),
            ("initialize", run_initialize),
            ("logging", run_logging),
            ("server discovery", run_discover),
            ("method params", run_invalid_method_params),
            ("result shapes", run_result_shapes),
            ("tool errors", run_tool_error),
            ("recall", run_recall),
            ("zero-hit search", run_zero_hit),
            ("edit", run_edit),
            ("tool filters", run_tool_filters),
        ];
        for (_, run) in sections {
            run(&Server::new());
        }
    }
    fn run_jsonrpc(server: &Server) {
        use Input::{Json, Raw};
        use Reply::{Batch, Error, None, Result};
        let cases = rpc_cases! {
        "JSONRPC-2.0-PARSE-001": Raw("{bad") => Error(Value::Null, -32700); "JSONRPC-2.0-REQ-001": Json(json!({"jsonrpc":"2.0","id":1,"method":"ping"})) => Result(json!(1)); "JSONRPC-2.0-REQ-002": Json(json!({"jsonrpc":"1.0","id":"bad-version","method":"ping"})) => Error(json!("bad-version"), -32600);
        "JSONRPC-2.0-REQ-003": Json(json!({"id":"missing-version","method":"ping"})) => Error(json!("missing-version"), -32600); "JSONRPC-2.0-REQ-004": Json(json!(1)) => Error(Value::Null, -32600); "JSONRPC-2.0-REQ-005": Json(json!({"jsonrpc":"2.0","id":"bad-method","method":7})) => Error(json!("bad-method"), -32600);
        "JSONRPC-2.0-REQ-006": Json(json!({"jsonrpc":"2.0","id":{"not":"valid"},"method":"ping"})) => Error(Value::Null, -32600); "JSONRPC-2.0-REQ-007": Json(json!({"jsonrpc":"2.0","id":"bad-params","method":"ping","params":true})) => Error(json!("bad-params"), -32600); "JSONRPC-2.0-NOTIF-001": Json(json!({"jsonrpc":"2.0","method":"ping"})) => None;
        "JSONRPC-2.0-NOTIF-002": Json(json!({"jsonrpc":"2.0","method":"unknown/notification"})) => None; "MCP-2025-06-18-INITIALIZED-NOTIF-001": Json(json!({"jsonrpc":"2.0","method":"notifications/initialized"})) => None; "JSONRPC-2.0-METHOD-001": Json(json!({"jsonrpc":"2.0","id":"unknown","method":"unknown/request"})) => Error(json!("unknown"), -32601);
        "JSONRPC-2.0-BATCH-001": Json(json!([{"jsonrpc":"2.0","id":"batch-ping","method":"ping"},{"jsonrpc":"2.0","method":"ping"}])) => Batch(vec![Result(json!("batch-ping"))]); "JSONRPC-2.0-BATCH-002": Json(json!([])) => Error(Value::Null, -32600); "JSONRPC-2.0-BATCH-003": Json(json!([{"jsonrpc":"2.0","method":"ping"},{"jsonrpc":"2.0","method":"unknown/notification"}])) => None;
        "JSONRPC-2.0-BATCH-004": Json(json!([1,{"jsonrpc":"2.0","id":"batch-valid","method":"ping"}])) => Batch(vec![Error(Value::Null, -32600), Result(json!("batch-valid"))]);
        };
        for case in &cases {
            run_rpc_case(server, case).unwrap_or_else(|reason| panic!("{}: {reason}", case.id));
        }
    }
    fn run_protocol_errors(server: &Server) {
        use Input::{Json, Raw};
        error_cases!(server;
        "PARAM-MISSING-TOOL": Json(json!({"jsonrpc":"2.0","id":"missing-tool","method":"tools/call","params":{}})) => -32602, json!("missing-tool"), "missing_param", [], "param" => json!("name");
        "PARAM-UNKNOWN-TOOL": Json(json!({"jsonrpc":"2.0","id":"unknown-tool","method":"tools/call","params":{"name":"does_not_exist","arguments":{}}})) => -32602, json!("unknown-tool"), "unknown_tool", ["available_tools" => "tz_read"], "tool" => json!("does_not_exist");
        "PARAM-UNKNOWN-RESOURCE": Json(json!({"jsonrpc":"2.0","id":"unknown-resource","method":"resources/read","params":{"uri":"resource://tokenzero/missing"}})) => -32602, json!("unknown-resource"), "unknown_resource", ["available_resources" => "resource://tokenzero/capabilities"], "uri" => json!("resource://tokenzero/missing");
        "PROTOCOL-PARSE": Raw("{bad") => -32700, Value::Null, "parse_error", []; "PROTOCOL-NON-OBJECT": Json(json!(1)) => -32600, Value::Null, "invalid_request", []; "PROTOCOL-PARAMS-ENVELOPE": Json(json!({"jsonrpc":"2.0","id":"bad-params-envelope","method":"ping","params":true})) => -32600, json!("bad-params-envelope"), "invalid_request", [];
        "PROTOCOL-UNKNOWN-METHOD": Json(json!({"jsonrpc":"2.0","id":"unknown-method","method":"unknown/request"})) => -32601, json!("unknown-method"), "unknown_method", ["available_methods" => "tools/list", "available_methods" => "notifications/initialized"], "method" => json!("unknown/request");
        );
    }
    fn initialize_params(version: &str) -> Value {
        json!({"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"tokenzero-conformance-client","version":"1.0.0"}})
    }
    fn run_initialize(server: &Server) {
        let stable = server.response(request(
            "init-stable",
            "initialize",
            initialize_params("2025-06-18"),
        ));
        assert_init(&stable["result"], "2025-06-18");
        assert_negotiation(
            &stable["result"]["_meta"]["tokenzero/protocolNegotiation"],
            "2025-06-18",
            "2025-06-18",
            false,
        );
        let initialized = server.response(request(
            "initialized-legacy-request",
            "notifications/initialized",
            json!({}),
        ));
        assert_eq!(
            initialized["result"],
            json!({}),
            "INITIALIZED-LEGACY: {initialized}"
        );
        let unsupported = server.response(request(
            "init-unsupported",
            "initialize",
            initialize_params("1900-01-01"),
        ));
        assert_init(&unsupported["result"], "2025-06-18");
        let negotiation = &unsupported["result"]["_meta"]["tokenzero/protocolNegotiation"];
        assert_negotiation(negotiation, "1900-01-01", "2025-06-18", true);
        assert!(
            array_has(negotiation, "supportedProtocolVersions", "2025-06-18"),
            "INIT-UNSUPPORTED: {unsupported}"
        );
        invalid_cases!(server;
        P2025, "INIT-PARAMS-001", "initialize params are required", "init-no-params", "initialize", Value::Null, "missing_param"; P2025, "INIT-PARAMS-002", "initialize params must be object", "init-array", "initialize", json!([]), "invalid_params";
        P2025, "INIT-PROTOCOL-001", "initialize protocolVersion is required", "init-no-version", "initialize", json!({"capabilities":{},"clientInfo":{"name":"client","version":"1.0.0"}}), "missing_param";
        P2025, "INIT-PROTOCOL-002", "initialize protocolVersion must be string", "init-number-version", "initialize", json!({"protocolVersion":1,"capabilities":{},"clientInfo":{"name":"client","version":"1.0.0"}}), "invalid_params";
        P2025, "INIT-CAPS-001", "initialize capabilities are required", "init-no-caps", "initialize", json!({"protocolVersion":"2025-06-18","clientInfo":{"name":"client","version":"1.0.0"}}), "missing_param";
        P2025, "INIT-CAPS-002", "initialize capabilities must be object", "init-array-caps", "initialize", json!({"protocolVersion":"2025-06-18","capabilities":[],"clientInfo":{"name":"client","version":"1.0.0"}}), "invalid_params";
        P2025, "INIT-CLIENT-001", "initialize clientInfo is required", "init-no-client", "initialize", json!({"protocolVersion":"2025-06-18","capabilities":{}}), "missing_param"; P2025, "INIT-CLIENT-002", "initialize clientInfo must be object", "init-array-client", "initialize", json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":[]}), "invalid_params";
        P2025, "INIT-CLIENT-003", "initialize clientInfo.name is required", "init-client-no-name", "initialize", json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"version":"1.0.0"}}), "missing_param";
        P2025, "INIT-CLIENT-004", "initialize clientInfo.version is required", "init-client-no-version", "initialize", json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"client"}}), "missing_param";
        P2025, "INIT-CLIENT-005", "initialize clientInfo.title must be string when present", "init-client-title-number", "initialize", json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"client","version":"1.0.0","title":7}}), "invalid_params";
        );
    }
    fn run_logging(server: &Server) {
        for level in [
            "debug",
            "info",
            "notice",
            "warning",
            "error",
            "critical",
            "alert",
            "emergency",
        ] {
            let actual = server.response(request(
                &format!("log-{level}"),
                "logging/setLevel",
                json!({"level":level}),
            ));
            assert!(
                actual["result"].is_object() && actual.get("error").is_none(),
                "LOGGING-{level}: {actual}"
            );
        }
        invalid_cases!(server;
        P2025, "LOGGING-LEVEL-001", "logging/setLevel params are required", "log-no-params", "logging/setLevel", Value::Null, "missing_param"; P2025, "LOGGING-LEVEL-002", "logging/setLevel params must be object", "log-array-params", "logging/setLevel", json!([]), "invalid_params";
        P2025, "LOGGING-LEVEL-003", "logging/setLevel level is required", "log-missing-level", "logging/setLevel", json!({}), "missing_param"; P2025, "LOGGING-LEVEL-004", "logging/setLevel level must be string", "log-number-level", "logging/setLevel", json!({"level":1}), "invalid_params";
        P2025, "LOGGING-LEVEL-005", "logging/setLevel level must be a valid syslog severity", "log-trace-level", "logging/setLevel", json!({"level":"trace"}), "invalid_param_value";
        );
        let missing = server.response(request(
            "log-missing-level-options",
            "logging/setLevel",
            json!({}),
        ));
        assert!(
            array_has(&missing["error"]["data"], "available_options", "info"),
            "LOGGING-MISSING-OPTIONS: {missing}"
        );
        let invalid = server.response(request(
            "log-invalid-level-options",
            "logging/setLevel",
            json!({"level":"trace"}),
        ));
        let data = &invalid["error"]["data"];
        assert_eq!(
            data["parameter"], "level",
            "LOGGING-INVALID-OPTIONS: {invalid}"
        );
        assert_eq!(
            data["provided"], "trace",
            "LOGGING-INVALID-OPTIONS: {invalid}"
        );
        assert!(
            array_has(data, "available_levels", "warning"),
            "LOGGING-INVALID-OPTIONS: {invalid}"
        );
        assert_eq!(
            data["suggested_tool_calls"][0]["method"], "logging/setLevel",
            "LOGGING-INVALID-OPTIONS: {invalid}"
        );
    }
    fn run_discover(server: &Server) {
        let discovered = server.response(request( "discover-draft", "server/discover", json!({"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"tokenzero-conformance-client","version":"1.0.0"},"io.modelcontextprotocol/clientCapabilities":{}}}), ));
        let result = &discovered["result"];
        assert_eq!(
            result["resultType"], "complete",
            "DISCOVER-DRAFT: {discovered}"
        );
        assert!(
            array_has(result, "supportedVersions", "2026-07-28"),
            "DISCOVER-DRAFT: {discovered}"
        );
        for capability in ["tools", "resources"] {
            assert!(
                result["capabilities"][capability].is_object(),
                "DISCOVER-DRAFT {capability}: {discovered}"
            );
        }
        for field in ["name", "version"] {
            assert_non_empty(
                &result["serverInfo"][field],
                &format!("DISCOVER-DRAFT serverInfo.{field}"),
            );
        }
        assert_non_empty(&result["instructions"], "DISCOVER-DRAFT instructions");
        assert_eq!(
            result["_meta"]["clientMetaAccepted"], true,
            "DISCOVER-DRAFT: {discovered}"
        );
        assert_eq!(
            result["protocolVersions"], result["supportedVersions"],
            "DISCOVER-DRAFT: {discovered}"
        );
        let no_params = server.response(
            json!({"jsonrpc":"2.0","id":"discover-no-params","method":"server/discover"}),
        );
        assert_eq!(
            no_params["result"]["resultType"], "complete",
            "DISCOVER-NO-PARAMS: {no_params}"
        );
        assert_eq!(
            no_params["result"]["_meta"]["clientMetaAccepted"], false,
            "DISCOVER-NO-PARAMS: {no_params}"
        );
        invalid_cases!(server;
        PDRAFT, "DISCOVER-PARAMS-001", "server/discover params must be object when present", "discover-array", "server/discover", json!([]), "invalid_params"; PDRAFT, "DISCOVER-PARAMS-002", "server/discover params._meta must be object when present", "discover-bad-meta", "server/discover", json!({"_meta":[]}), "invalid_params";
        PDRAFT, "DISCOVER-PARAMS-003", "server/discover carries no body params beyond standard _meta", "discover-extra", "server/discover", json!({"protocolVersion":"2026-07-28"}), "invalid_params";
        );
    }
    fn run_invalid_method_params(server: &Server) {
        invalid_cases!(server;
        P2025, "TOOLS-LIST-PARAMS-001", "tools/list params must be an object when present", "tools-list-array", "tools/list", json!([]), "invalid_params"; P2025, "RESOURCES-LIST-PARAMS-001", "resources/list params must be an object when present", "resources-list-array", "resources/list", json!([]), "invalid_params";
        P2025, "RESOURCES-TEMPLATES-LIST-PARAMS-001", "resources/templates/list params must be an object when present", "resources-templates-list-array", "resources/templates/list", json!([]), "invalid_params"; P2025, "PROMPTS-LIST-PARAMS-001", "prompts/list params must be an object when present", "prompts-list-array", "prompts/list", json!([]), "invalid_params";
        P2025, "RESOURCES-READ-PARAMS-001", "resources/read params must be an object with uri", "resources-read-array", "resources/read", json!([]), "invalid_params"; P2025, "RESOURCES-READ-PARAMS-002", "resources/read params.uri must be a string", "resources-read-uri-number", "resources/read", json!({"uri":7}), "invalid_params";
        P2025, "TOOLS-CALL-PARAMS-001", "tools/call params must be an object with name", "tools-call-array", "tools/call", json!([]), "invalid_params"; P2025, "TOOLS-CALL-PARAMS-002", "tools/call params.name must be a string", "tools-call-name-number", "tools/call", json!({"name":7}), "invalid_params";
        P2025, "TOOLS-CALL-ARGS-001", "tools/call arguments must be an object when present", "tools-call-args-array", "tools/call", json!({"name":"shell","arguments":["echo","should-not-run"]}), "invalid_params"; P2025, "TOOLS-LIST-CURSOR-001", "tools/list params.cursor must be a string when present", "tools-list-cursor-number", "tools/list", json!({"cursor":7}), "invalid_params";
        P2025, "RESOURCES-LIST-CURSOR-001", "resources/list params.cursor must be a string when present", "resources-list-cursor-number", "resources/list", json!({"cursor":7}), "invalid_params";
        P2025, "RESOURCES-TEMPLATES-LIST-CURSOR-001", "resources/templates/list params.cursor must be a string when present", "resources-templates-list-cursor-number", "resources/templates/list", json!({"cursor":7}), "invalid_params";
        P2025, "PROMPTS-LIST-CURSOR-001", "prompts/list params.cursor must be a string when present", "prompts-list-cursor-number", "prompts/list", json!({"cursor":7}), "invalid_params";
        );
    }
    fn run_result_shapes(server: &Server) {
        let fixture = server.path("fixture.txt");
        fs::write(
            &fixture,
            "tokenzero conformance fixture
",
        )
        .unwrap();
        let resources = server.call("resources-shape", "resources/list", json!({}));
        assert_list(&resources, "resources", "RESULT-RESOURCES");
        let items = resources["result"]["resources"].as_array().unwrap();
        assert!(!items.is_empty(), "RESULT-RESOURCES: {resources}");
        for item in items {
            assert_fields(item, "RESULT-RESOURCES", &["uri", "name", "mimeType"]);
            if let Some(description) = item.get("description") {
                assert_non_empty(description, "RESULT-RESOURCES description");
            }
        }
        let templates = server.call(
            "resource-templates-shape",
            "resources/templates/list",
            json!({}),
        );
        assert_list(&templates, "resourceTemplates", "RESULT-TEMPLATES");
        assert!(
            templates["result"]["resourceTemplates"]
                .as_array()
                .unwrap()
                .is_empty(),
            "RESULT-TEMPLATES: {templates}"
        );
        let prompts = server.call("prompts-shape", "prompts/list", json!({}));
        assert_list(&prompts, "prompts", "RESULT-PROMPTS");
        let tools = server.call("tools-shape", "tools/list", json!({}));
        assert_list(&tools, "tools", "RESULT-TOOLS");
        let tools_array = tools["result"]["tools"].as_array().unwrap();
        assert!(!tools_array.is_empty(), "RESULT-TOOLS: {tools}");
        for tool in tools_array {
            assert_fields(tool, "RESULT-TOOLS", &["name", "description"]);
            assert_eq!(
                tool["inputSchema"]["type"], "object",
                "RESULT-TOOLS: {tool}"
            );
        }
        let read = server.call(
            "read-resource-shape",
            "resources/read",
            json!({"uri":"resource://tokenzero/capabilities"}),
        );
        let contents = read["result"]["contents"].as_array().unwrap();
        assert!(!contents.is_empty(), "RESULT-READ: {read}");
        for content in contents {
            assert_fields(content, "RESULT-READ", &["uri", "mimeType", "text"]);
        }
        let call = server.call(
            "tool-call-shape",
            "tools/call",
            json!({"name":"read","arguments":{"path":fixture.display().to_string(),"raw":true}}),
        );
        let result = &call["result"];
        let content = result["content"].as_array().unwrap();
        assert!(
            call.get("error").is_none() && !content.is_empty(),
            "RESULT-CALL: {call}"
        );
        assert_eq!(content[0]["type"], "text", "RESULT-CALL: {call}");
        assert_non_empty(&content[0]["text"], "RESULT-CALL content text");
        assert!(
            result.get("structuredContent").is_none(),
            "RESULT-CALL: {call}"
        );
        assert!(
            result.get("isError").is_none() || result["isError"] == false,
            "RESULT-CALL: {call}"
        );
    }
    fn run_tool_error(server: &Server) {
        let actual = server.call(
            "tool-origin-error",
            "tools/call",
            json!({"name":"read","arguments":{"path":"/__tokenzero_conformance_outside_root__"}}),
        );
        assert!(actual.get("error").is_none(), "TOOL-ORIGIN-ERROR: {actual}");
        assert_eq!(
            actual["result"]["isError"], true,
            "TOOL-ORIGIN-ERROR: {actual}"
        );
        assert!(
            actual["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("outside allowed roots")),
            "TOOL-ORIGIN-ERROR: {actual}"
        );
    }
    fn run_recall(server: &Server) {
        let file = server.path("data.txt");
        fs::write(
            &file,
            "unique_recall_marker line
",
        )
        .unwrap();
        server.call(
            "recall-seed",
            "tools/call",
            json!({"name":"read","arguments":{"path":file.display().to_string()}}),
        );
        let recalled = server.call(
            "recall-query",
            "tools/call",
            json!({"name":"recall","arguments":{"query":"UNIQUE_RECALL_MARKER","max_hits":"5"}}),
        );
        let text = recalled["result"]["content"][0]["text"].as_str().unwrap();
        for needle in ["unique_recall_marker", "tz://"] {
            assert!(
                text.contains(needle),
                "RECALL-QUERY missing {needle}: {text}"
            );
        }
    }
    fn run_zero_hit(server: &Server) {
        fs::write(
            server.path("lib.rs"),
            "fn alpha() {}
",
        )
        .unwrap();
        let response = server.call("zero-hit-grep", "tools/call", json!({"name":"grep","arguments":{"query":"no_such_token","path":server.dir.path().display().to_string()}}));
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(
            lines.first().copied(),
            Some("# grep no_such_token — 0 matches"),
            "ZERO-HIT: {text}"
        );
        assert!(
            lines
                .get(1)
                .is_some_and(|line| line.starts_with("refs: tz://")),
            "ZERO-HIT: {text}"
        );
    }
    fn run_edit(server: &Server) {
        let file = server.path("lib.rs");
        fs::write(
            &file,
            "fn alpha() {}
fn beta() {}
",
        )
        .unwrap();
        let listed = server.response(request("edit-list", "tools/list", json!({})));
        assert_members(&tool_names(&listed), &["tz_edit", "edit"], &[], "EDIT-LIST");
        let docs = server.response(request(
            "edit-docs",
            "resources/read",
            json!({"uri":"resource://tokenzero/tools"}),
        ));
        let catalog: Value =
            serde_json::from_str(docs["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
        assert!(
            catalog["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == "tz_edit"),
            "EDIT-DOCS"
        );
        let edits = serde_json::to_string(&json!([{"find":"fn alpha() {}","replace":"fn alpha() -> u8 { 1 }","replace_all":"false"}])).unwrap();
        let response = server.call("edit-call", "tools/call", json!({"name":"edit","arguments":{"path":file.display().to_string(),"edits":edits,"dry_run":"false"}}));
        assert!(response.get("error").is_none(), "EDIT-CALL: {response}");
        let result = &response["result"];
        assert!(
            result.get("isError").is_none() || result["isError"] == false,
            "EDIT-CALL: {response}"
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        let footer = text
            .lines()
            .find(|line| line.starts_with("refs: "))
            .unwrap_or_else(|| panic!("EDIT-CALL missing refs: {text}"));
        let undo_ref = footer
            .split_whitespace()
            .find_map(|part| part.strip_prefix("undo:"))
            .unwrap_or_else(|| panic!("EDIT-CALL missing undo: {footer}"));
        assert!(undo_ref.starts_with("tz://"), "EDIT-CALL: {footer}");
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "fn alpha() -> u8 { 1 }
fn beta() {}
",
            "EDIT-CALL file"
        );
        let expanded = server.call(
            "edit-undo-expand",
            "tools/call",
            json!({"name":"expand","arguments":{"ref":undo_ref}}),
        );
        let expanded_text = expanded["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            expanded_text.contains("fn alpha() {}"),
            "EDIT-UNDO: {expanded_text}"
        );
    }
    fn run_tool_filters(server: &Server) {
        let default = server.call("tools-default", "tools/list", json!({}));
        let names = tool_names(&default);
        assert!(names.len() > 7, "TOOLS-DEFAULT: {default}");
        assert_members(&names, &["tz_read", "read"], &[], "TOOLS-DEFAULT");
        let material = server.call(
            "tools-material",
            "tools/list",
            json!({"_meta":{"tokenzero/toolCluster":"material"}}),
        );
        let names = tool_names(&material);
        assert!(names.len() <= 7, "TOOLS-MATERIAL: {material}");
        assert_members(
            &names,
            &["tz_read", "tz_expand"],
            &["tz_shell", "read"],
            "TOOLS-MATERIAL",
        );
        let filter = &material["result"]["_meta"]["tokenzero/toolFilter"];
        assert_eq!(filter["cluster"], "material", "TOOLS-MATERIAL: {material}");
        assert_eq!(
            filter["includeAliases"], false,
            "TOOLS-MATERIAL: {material}"
        );
        let execution = server.call(
            "tools-execution",
            "tools/list",
            json!({"profile":"execution"}),
        );
        let names = tool_names(&execution);
        assert!(names.len() <= 7, "TOOLS-EXECUTION: {execution}");
        assert_members(&names, &["tz_shell"], &["tz_read"], "TOOLS-EXECUTION");
        let aliased = server.call(
            "tools-material-aliases",
            "tools/list",
            json!({"_meta":{"tokenzero/toolCluster":"material","tokenzero/includeAliases":true}}),
        );
        assert_members(
            &tool_names(&aliased),
            &["tz_read", "read"],
            &[],
            "TOOLS-ALIASED",
        );
        assert_eq!(
            aliased["result"]["_meta"]["tokenzero/toolFilter"]["includeAliases"], true,
            "TOOLS-ALIASED: {aliased}"
        );
        let invalid = server.call(
            "tools-bad-cluster",
            "tools/list",
            json!({"_meta":{"tokenzero/toolCluster":"matrial"}}),
        );
        let data = &invalid["error"]["data"];
        assert_eq!(
            invalid["error"]["code"], -32602,
            "TOOLS-BAD-CLUSTER: {invalid}"
        );
        assert_eq!(
            data["kind"], "unknown_tool_cluster",
            "TOOLS-BAD-CLUSTER: {invalid}"
        );
        assert_eq!(
            data["error_type"], "INVALID_ARGUMENT",
            "TOOLS-BAD-CLUSTER: {invalid}"
        );
        assert!(
            array_has(data, "available_options", "material"),
            "TOOLS-BAD-CLUSTER: {invalid}"
        );
        assert_eq!(
            data["suggestions"][0]["value"], "material",
            "TOOLS-BAD-CLUSTER: {invalid}"
        );
    }
    fn run_invalid(server: &Server, cases: &[InvalidCase]) {
        for (id, description, input, kind) in cases {
            let actual = server.response(input.clone());
            assert_eq!(
                actual["error"]["code"], -32602,
                "{id}: {description}: {actual}"
            );
            assert_eq!(
                actual["error"]["data"]["kind"], *kind,
                "{id}: {description}: {actual}"
            );
            assert_protocol_data(&actual["error"]["data"])
                .unwrap_or_else(|reason| panic!("{id}: {description}: {reason}"));
        }
    }
    fn run_rpc_case(server: &Server, case: &RpcCase) -> Result<(), String> {
        let payload = match &case.input {
            Input::Json(value) => value.to_string(),
            Input::Raw(text) => (*text).into(),
        };
        let actual = server.raw(&payload);
        match (&case.reply, actual) {
            (Reply::None, None) => Ok(()),
            (Reply::None, Some(value)) => Err(format!("expected no response, got {value}")),
            (Reply::Result(id), Some(value)) => assert_result(&value, id),
            (Reply::Result(_), None) => Err("expected result response, got no response".into()),
            (Reply::Error(id, code), Some(value)) => assert_error(&value, id, *code),
            (Reply::Error(_, _), None) => Err("expected error response, got no response".into()),
            (Reply::Batch(expected), Some(Value::Array(actual)))
                if actual.len() == expected.len() =>
            {
                for (index, (expected, actual)) in expected.iter().zip(&actual).enumerate() {
                    match expected {
                        Reply::Result(id) => assert_result(actual, id),
                        Reply::Error(id, code) => assert_error(actual, id, *code),
                        _ => unreachable!(),
                    }
                    .map_err(|reason| format!("batch[{index}]: {reason}"))?;
                }
                Ok(())
            }
            (Reply::Batch(expected), Some(Value::Array(actual))) => Err(format!(
                "expected {} batch responses, got {}: {actual:?}",
                expected.len(),
                actual.len()
            )),
            (Reply::Batch(_), Some(value)) => Err(format!("expected batch array, got {value}")),
            (Reply::Batch(_), None) => Err("expected batch response, got no response".into()),
        }
    }
    fn assert_result(actual: &Value, id: &Value) -> Result<(), String> {
        check(actual["jsonrpc"] == "2.0", || {
            format!("missing jsonrpc 2.0 in {actual}")
        })?;
        check(&actual["id"] == id, || {
            format!("expected id {id}, got {}", actual["id"])
        })?;
        check(actual.get("result").is_some_and(Value::is_object), || {
            format!("expected object result, got {actual}")
        })?;
        check(actual.get("error").is_none(), || {
            format!("result response included error: {actual}")
        })
    }
    fn assert_error(actual: &Value, id: &Value, code: i64) -> Result<(), String> {
        check(actual["jsonrpc"] == "2.0", || {
            format!("missing jsonrpc 2.0 in {actual}")
        })?;
        check(&actual["id"] == id, || {
            format!("expected id {id}, got {}", actual["id"])
        })?;
        check(actual["error"]["code"] == code, || {
            format!(
                "expected error code {code}, got {} in {actual}",
                actual["error"]["code"]
            )
        })?;
        check(actual.get("result").is_none(), || {
            format!("error response included result: {actual}")
        })?;
        assert_protocol_data(&actual["error"]["data"])
    }
    fn assert_protocol_data(data: &Value) -> Result<(), String> {
        check(data.is_object(), || {
            format!("expected object error.data, got {data}")
        })?;
        for field in ["kind", "reason", "fix_hint"] {
            check(
                data[field].as_str().is_some_and(|text| !text.is_empty()),
                || format!("missing data.{field} in {data}"),
            )?;
        }
        check(data["recoverable"].is_boolean(), || {
            format!("missing data.recoverable in {data}")
        })
    }
    fn check(condition: bool, message: impl FnOnce() -> String) -> Result<(), String> {
        condition.then_some(()).ok_or_else(message)
    }
    fn assert_init(result: &Value, version: &str) {
        assert_eq!(result["protocolVersion"], version);
        for capability in ["logging", "tools", "resources", "prompts"] {
            assert!(
                result["capabilities"][capability].is_object(),
                "INIT {capability}: {result}"
            );
        }
        assert_eq!(result["serverInfo"]["name"], "tokenzero");
        assert_non_empty(&result["serverInfo"]["version"], "INIT server version");
    }
    fn assert_negotiation(value: &Value, requested: &str, negotiated: &str, fallback: bool) {
        assert_eq!(value["requestedProtocolVersion"], requested);
        assert_eq!(value["negotiatedProtocolVersion"], negotiated);
        assert_eq!(value["fallback"], fallback);
    }
    fn assert_fields(value: &Value, label: &str, fields: &[&str]) {
        for field in fields {
            assert_non_empty(&value[field], &format!("{label}.{field}"));
        }
    }
    fn assert_list(response: &Value, key: &str, label: &str) {
        assert!(response.get("error").is_none(), "{label}: {response}");
        assert!(response["result"][key].is_array(), "{label}: {response}");
        if let Some(cursor) = response["result"].get("nextCursor") {
            assert!(cursor.is_string(), "{label}: {response}");
        }
    }
    fn assert_non_empty(value: &Value, label: &str) {
        assert!(
            value.as_str().is_some_and(|text| !text.is_empty()),
            "{label}: {value}"
        );
    }
    fn assert_members(names: &[&str], included: &[&str], excluded: &[&str], label: &str) {
        for name in included {
            assert!(names.contains(name), "{label}: missing {name}");
        }
        for name in excluded {
            assert!(
                !names.contains(name),
                "{label}: unexpectedly included {name}"
            );
        }
    }
    fn array_has(value: &Value, field: &str, needle: &str) -> bool {
        value[field]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item == needle))
    }
    fn tool_names(response: &Value) -> Vec<&str> {
        response["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect()
    }
}
