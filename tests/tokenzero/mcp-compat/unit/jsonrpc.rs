use super::*;
use tempfile::tempdir;
use tokenzero_core::MCP_SCHEMA_VERSION;

use super::support::*;

#[test]
fn malformed_json_returns_error_and_does_not_panic() {
    let (_dir, engine) = test_engine();
    let response = handle_jsonrpc(&engine, "{bad").unwrap();
    let parsed = response_json(&response);
    // JSON-RPC §4.2: -32700 is "Parse error".
    assert_structured_error(&parsed, -32700, None);
    assert!(
        parsed["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Parse error"),
        "{parsed:#}"
    );
}

#[test]
fn tools_list_includes_aliases_with_stub_schema() {
    let specs = tool_specs();
    let by_name = |name: &str| -> &ToolSpec { specs.iter().find(|s| s.name == name).unwrap() };

    // Every canonical tool has a matching alias.
    for (canonical, alias) in [
        ("tz_read", "read"),
        ("tz_grep", "grep"),
        ("tz_glob", "glob"),
    ] {
        let c = by_name(canonical);
        let a = by_name(alias);
        // Canonical schema has real properties; alias advertises a permissive stub.
        assert!(
            c.input_schema["properties"].is_object(),
            "{canonical} must have real schema properties"
        );
        assert_eq!(
            a.input_schema,
            json!({"type": "object"}),
            "{alias} must advertise a stub schema"
        );
    }
}

#[test]
fn tools_call_rejects_mixed_type_argv_arrays() {
    let dir = tempdir().unwrap();
    let engine = TokenZeroEngine::new(EngineConfig::for_root(dir.path()));
    engine.mark_lifecycle_ready_for_tests();

    for invalid_argv in [
        json!(["printf", 7, "ignored"]),
        json!(["printf", null, "ignored"]),
        json!(["printf", {"bad": true}, "ignored"]),
        json!(["printf", false, "ignored"]),
    ] {
        let request = json!({
            "jsonrpc": "2.0",
            "id": "bad-argv",
            "method": "tools/call",
            "params": {
                "name": "rewrite",
                "arguments": {"argv": invalid_argv}
            }
        });
        let response = handle_jsonrpc(&engine, &request.to_string()).unwrap();
        let parsed: Value = serde_json::from_str(&response).unwrap();

        assert_eq!(parsed["error"]["code"], -32602, "{parsed:#}");
        assert_eq!(parsed["error"]["data"]["kind"], "invalid_params");
        assert!(
            parsed["error"]["data"]["reason"]
                .as_str()
                .unwrap()
                .contains("array of strings"),
            "{parsed:#}"
        );
    }
}

#[test]
fn tools_call_rejects_mixed_type_path_arrays() {
    let dir = tempdir().unwrap();
    let engine = TokenZeroEngine::new(EngineConfig::for_root(dir.path()));
    engine.mark_lifecycle_ready_for_tests();

    for invalid_path in [
        json!(["missing.txt", 1]),
        json!(["missing.txt", null]),
        json!(["missing.txt", {"bad": true}]),
        json!(["missing.txt", false]),
    ] {
        let request = json!({
            "jsonrpc": "2.0",
            "id": "bad-path",
            "method": "tools/call",
            "params": {
                "name": "read",
                "arguments": {"path": invalid_path}
            }
        });
        let response = handle_jsonrpc(&engine, &request.to_string()).unwrap();
        let parsed: Value = serde_json::from_str(&response).unwrap();

        assert_eq!(parsed["error"]["code"], -32602, "{parsed:#}");
        assert_eq!(parsed["error"]["data"]["kind"], "invalid_params");
        assert!(
            parsed["error"]["data"]["reason"]
                .as_str()
                .unwrap()
                .contains("array of strings"),
            "{parsed:#}"
        );
    }
}

#[test]
fn mcp_idle_timeout_zero_disables_and_large_values_clamp() {
    assert_eq!(mcp_idle_timeout_from_secs(Some(0)), None);
    assert_eq!(
        mcp_idle_timeout_from_secs(Some(1)).unwrap(),
        Duration::from_secs(1)
    );
    assert_eq!(
        mcp_idle_timeout_from_secs(Some(u64::MAX)).unwrap(),
        Duration::from_secs(MAX_MCP_IDLE_TIMEOUT_SECS)
    );
    assert_eq!(mcp_idle_timeout_from_secs(None), None);
    assert_eq!(DEFAULT_MCP_IDLE_TIMEOUT_SECS, 0);
}

/// Find a tool by name in a tools array.
fn find_tool_by_name<'a>(tools: &'a [Value], name: &str) -> &'a Value {
    tools
        .iter()
        .find(|tool| tool["name"] == name)
        .unwrap_or_else(|| panic!("tool {name} not found in tools list"))
}

/// Find a tool by name in a tools/list response.
fn find_tool<'a>(listed: &'a Value, name: &str) -> &'a Value {
    find_tool_by_name(listed["result"]["tools"].as_array().unwrap(), name)
}

/// Assert no tool in a tools/list result advertises top-level schema combinators.
fn assert_no_schema_combinators(listed: &Value) {
    for tool in listed["result"]["tools"].as_array().unwrap() {
        let schema = &tool["inputSchema"];
        assert_eq!(schema["type"], "object", "tool {}", tool["name"]);
        for key in ["anyOf", "oneOf", "allOf"] {
            assert!(
                schema.get(key).is_none(),
                "tool {} advertises top-level {key}",
                tool["name"]
            );
        }
    }
}

#[test]
fn mcp_lists_and_calls_cache_pack_tool() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("AGENTS.md"), "stable\n").unwrap();
    let engine = TokenZeroEngine::new(EngineConfig::for_root(dir.path()));
    engine.mark_lifecycle_ready_for_tests();
    let listed: Value = serde_json::from_str(
        &handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":10,"method":"tools/list","params":{}}"#,
        )
        .unwrap(),
    )
    .unwrap();
    let names: Vec<_> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(names.contains(&"tz_cache_pack"));
    assert!(names.contains(&"cache_pack"));

    let read_tool = find_tool(&listed, "tz_read");
    assert!(
        read_tool["inputSchema"].get("$schema").is_none(),
        "tools/list schemas stay lean; the dialect is implied"
    );
    assert_eq!(read_tool["inputSchema"]["additionalProperties"], false);
    assert_eq!(read_tool["inputSchema"]["required"][0], "path");
    let description = read_tool["description"].as_str().unwrap();
    assert!(
        !description.is_empty() && description.len() < 300,
        "tools/list descriptions stay compact: {description}"
    );
    assert!(description.contains("tz://"), "{description}");

    // Long-form docs moved to the catalog resource (progressive disclosure).
    let docs: Value = serde_json::from_str(
            &handle_jsonrpc(
                &engine,
                r#"{"jsonrpc":"2.0","id":12,"method":"resources/read","params":{"uri":"resource://tokenzero/tools"}}"#,
            )
            .unwrap(),
        )
        .unwrap();
    let docs_text = docs["result"]["contents"][0]["text"].as_str().unwrap();
    let docs_payload: Value = serde_json::from_str(docs_text).unwrap();
    let read_doc = find_tool_by_name(docs_payload["tools"].as_array().unwrap(), "tz_read");
    let read_doc_description = read_doc["description"].as_str().unwrap();
    for required_section in [
        "Discovery",
        "When to use",
        "Do / Don't",
        "Examples",
        "Common mistakes",
        "Idempotency",
    ] {
        assert!(
            read_doc_description.contains(required_section),
            "missing {required_section} in {read_doc_description}"
        );
    }

    let alias_tool = find_tool(&listed, "read");
    // Aliases advertise a permissive stub on the wire; the canonical schema
    // stays recoverable from the catalog resource.
    assert_eq!(alias_tool["inputSchema"], json!({"type": "object"}));
    let alias_doc = find_tool_by_name(docs_payload["tools"].as_array().unwrap(), "read");
    assert_eq!(alias_doc["inputSchema"], read_tool["inputSchema"]);

    let shell_tool = find_tool(&listed, "tz_shell");
    assert_eq!(shell_tool["inputSchema"]["additionalProperties"], false);
    // Top-level schema combinators make some MCP clients (Claude Code
    // among them) drop the tool from the model's tool list entirely;
    // every advertised schema must stay a plain object.
    assert_no_schema_combinators(&listed);

    let called: Value = serde_json::from_str(
            &handle_jsonrpc(
                &engine,
                r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"cache_pack","arguments":{"scope":"agent"}}}"#,
            )
            .unwrap(),
        )
        .unwrap();
    assert!(
        called["result"].get("structuredContent").is_none(),
        "default envelope is text-only: {called}"
    );
    let called_text = called["result"]["content"][0]["text"].as_str().unwrap();
    assert!(called_text.contains("tz://"), "{called_text}");

    let pack = engine.cache_pack("agent");
    assert_eq!(pack.tool, "cache-pack");
    assert_eq!(
        pack.telemetry.as_ref().unwrap()["daemon_required"],
        false,
        "cache packs stay daemonless"
    );
}

#[test]
fn mcp_envelope_is_text_only_by_default() {
    let dir = tempdir().unwrap();
    let engine = TokenZeroEngine::new(EngineConfig::for_root(dir.path()));
    engine.mark_lifecycle_ready_for_tests();

    let response: Value = serde_json::from_str(
            &handle_jsonrpc(
                &engine,
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"shell","arguments":{"command":"echo compact-envelope-check"}}}"#,
            )
            .unwrap(),
        )
        .unwrap();

    let result = &response["result"];
    assert!(
        result.get("structuredContent").is_none(),
        "default tool results are text-only: {result}"
    );
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("compact-envelope-check"), "{text}");
    assert!(
        text.ends_with(
            "
0"
        ),
        "MCP status footer is one ACK/2 atom: {text}"
    );
    assert!(
        text.lines()
            .filter(|line| line.contains("_ref:") || line.starts_with("refs:"))
            .count()
            <= 1,
        "shell action must expose at most one primary ref line: {text}"
    );
    assert!(
        text.contains("combined_ref: tz://") || text.contains("refs: tz://"),
        "shell text must keep a recovery anchor: {text}"
    );

    // Reads carry their recovery refs in a text footer instead of a
    // structured envelope.
    fs::write(dir.path().join("sample.txt"), "alpha\nbeta\n").unwrap();
    // JSON-encode the path so Windows backslashes survive the raw envelope.
    let sample_path =
        serde_json::to_string(&dir.path().join("sample.txt").display().to_string()).unwrap();
    let read: Value = serde_json::from_str(
            &handle_jsonrpc(
                &engine,
                &format!(
                    r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"read","arguments":{{"path":{sample_path}}}}}}}"#,
                ),
            )
            .unwrap(),
        )
        .unwrap();
    let read_text = read["result"]["content"][0]["text"].as_str().unwrap();
    assert!(read_text.contains("alpha"), "{read_text}");
    // Visible capsules carry a single recovery ref footer (c9b1ca0). The
    // ordinal alias rewrite (1glt) is published only when the complete
    // serialized response is strictly cheaper under the same token gauge;
    // under the default gauge a full `tz://blob/<64hex>` ref is cheaper than
    // its ordinal, so the footer keeps the full blob ref.
    assert!(read_text.contains("refs: tz://blob/"), "{read_text}");
    assert_eq!(
        read_text
            .lines()
            .filter(|line| line.starts_with("refs:"))
            .count(),
        1,
        "one primary ref footer per action: {read_text}"
    );
    let legacy_multi_ref = format!("{read_text} tz://s/aaaaaaaaaaaaaaaa tz://s/bbbbbbbbbbbbbbbb");
    assert!(
        tokenzero_core::count_tokens(read_text) < tokenzero_core::count_tokens(&legacy_multi_ref),
        "single primary footer must reduce TA against multi-ref corpus"
    );
    // The edit hint rides the refs footer on read responses only: it steers
    // agents to tz_edit instead of a doomed native-Edit-after-tz_read loop.
    assert!(read_text.contains("edit: tz_edit"), "{read_text}");
    assert!(
        !text.contains("edit: tz_edit"),
        "shell responses must not carry the read edit hint: {text}"
    );
    let read_ref = read_text
        .lines()
        .find_map(|line| line.strip_prefix("refs: "))
        .and_then(|tail| tail.split_whitespace().next())
        .expect("read footer ref");
    let expanded: Value = serde_json::from_str(
        &handle_jsonrpc(
            &engine,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {"name": "tz_expand", "arguments": {"ref": read_ref, "raw": true}}
            })
            .to_string(),
        )
        .unwrap(),
    )
    .unwrap();
    let expanded_text = expanded["result"]["content"][0]["text"]
        .as_str()
        .expect("expand text");
    assert!(
        expanded_text.starts_with("alpha\nbeta\n"),
        "{expanded_text}"
    );
    assert_eq!(expanded["result"]["recovery"]["exact_bytes"], true);
    assert_eq!(expanded["result"]["recovery"]["do_not_recompact"], true);

    // The opt-in compact envelope still prunes payload duplicates and
    // forensic telemetry.
    let shell = engine.shell(
        "echo compact-envelope-check",
        None,
        None,
        Mode::Auto,
        None,
        false,
        None,
        None,
        None,
    );
    let cli = tools::compact_cli_envelope(&shell);
    assert_eq!(cli["telemetry"]["command_success"], true);
    assert!(cli["accounting"].is_object(), "{cli}");
    assert!(
        cli.get("visible").is_none(),
        "capsule text must not be duplicated in the envelope: {cli}"
    );
    assert!(
        cli.get("detail_ref").is_none(),
        "detail_ref equal to a carried ref must not be duplicated: {cli}"
    );
    let mut distinct = shell.clone();
    distinct.detail_ref = Some("tz://file/distinct-recovery-anchor".into());
    let distinct_cli = tools::compact_cli_envelope(&distinct);
    assert_eq!(
        distinct_cli["detail_ref"], "tz://file/distinct-recovery-anchor",
        "a nonidentical detail ref must be retained"
    );
    for pruned in ["argv", "stdout_preview", "stderr_preview", "stdout_capture"] {
        assert!(
            cli["telemetry"].get(pruned).is_none(),
            "telemetry.{pruned} should be pruned: {cli}"
        );
    }
}

#[test]
fn initialize_echoes_supported_stable_protocol() {
    let dir = tempdir().unwrap();
    let engine = TokenZeroEngine::new(EngineConfig::for_root(dir.path()));
    engine.mark_lifecycle_ready_for_tests();
    let response = handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"conformance-client","version":"1.0.0"}}}"#,
        )
        .unwrap();
    let parsed: Value = serde_json::from_str(&response).unwrap();

    assert_eq!(parsed["result"]["protocolVersion"], "2025-06-18");
    assert!(parsed["result"]["capabilities"]["tools"].is_object());
}

#[test]
fn resource_discovery_and_prompt_lists_are_supported() {
    let dir = tempdir().unwrap();
    let engine = TokenZeroEngine::new(EngineConfig::for_root(dir.path()));
    engine.mark_lifecycle_ready_for_tests();

    let resources: Value = serde_json::from_str(
        &handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":2,"method":"resources/list","params":{}}"#,
        )
        .unwrap(),
    )
    .unwrap();
    let prompts: Value = serde_json::from_str(
        &handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":3,"method":"prompts/list","params":{}}"#,
        )
        .unwrap(),
    )
    .unwrap();

    let resource_uris = resources["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|resource| resource["uri"].as_str())
        .collect::<Vec<_>>();
    assert!(resource_uris.contains(&"resource://tokenzero/capabilities"));
    assert!(resource_uris.contains(&"resource://tokenzero/tools"));
    assert_eq!(resources["result"]["resultType"], "complete");
    assert_eq!(prompts["result"]["prompts"].as_array().unwrap().len(), 0);

    let capabilities: Value = serde_json::from_str(
            &handle_jsonrpc(
                &engine,
                r#"{"jsonrpc":"2.0","id":4,"method":"resources/read","params":{"uri":"resource://tokenzero/capabilities"}}"#,
            )
            .unwrap(),
        )
        .unwrap();
    let text = capabilities["result"]["contents"][0]["text"]
        .as_str()
        .unwrap();
    let payload: Value = serde_json::from_str(text).unwrap();
    assert_eq!(payload["schema_version"], MCP_SCHEMA_VERSION);
    assert!(
        payload["tool_clusters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|cluster| cluster["cluster"] == "material")
    );
    assert!(payload["next_actions"].as_array().unwrap().len() >= 2);
}

#[test]
fn mcp_error_data_guides_unknown_tools_and_resources() {
    let dir = tempdir().unwrap();
    let engine = TokenZeroEngine::new(EngineConfig::for_root(dir.path()));
    engine.mark_lifecycle_ready_for_tests();

    let unknown_tool: Value = serde_json::from_str(
            &handle_jsonrpc(
                &engine,
                r#"{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"tz_reed","arguments":{}}}"#,
            )
            .unwrap(),
        )
        .unwrap();
    let tool_data = &unknown_tool["error"]["data"];
    assert_eq!(unknown_tool["error"]["code"], -32602);
    assert_eq!(tool_data["error_type"], "NOT_FOUND");
    assert_eq!(tool_data["recoverable"], true);
    assert_eq!(tool_data["entity_type"], "tool");
    assert_eq!(tool_data["provided"], "tz_reed");
    assert!(
        tool_data["available_options"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool == "tz_read")
    );
    assert_eq!(
        tool_data["suggestions"][0]["value"], "tz_read",
        "{tool_data}"
    );
    assert!(
        tool_data["fix_hint"]
            .as_str()
            .unwrap()
            .contains("tools/list")
    );
    assert_eq!(tool_data["suggested_tool_calls"][0]["method"], "tools/list");

    let unknown_resource: Value = serde_json::from_str(
            &handle_jsonrpc(
                &engine,
                r#"{"jsonrpc":"2.0","id":13,"method":"resources/read","params":{"uri":"resource://tokenzero/toolz"}}"#,
            )
            .unwrap(),
        )
        .unwrap();
    let resource_data = &unknown_resource["error"]["data"];
    assert_eq!(unknown_resource["error"]["code"], -32602);
    assert_eq!(resource_data["error_type"], "NOT_FOUND");
    assert_eq!(resource_data["recoverable"], true);
    assert_eq!(resource_data["entity_type"], "resource");
    assert!(
        resource_data["available_options"]
            .as_array()
            .unwrap()
            .iter()
            .any(|uri| uri == "resource://tokenzero/tools")
    );
    assert_eq!(
        resource_data["suggestions"][0]["value"], "resource://tokenzero/tools",
        "{resource_data}"
    );
    assert!(
        resource_data["fix_hint"]
            .as_str()
            .unwrap()
            .contains("resources/list")
    );
    assert_eq!(
        resource_data["suggested_tool_calls"][0]["method"],
        "resources/list"
    );
}

#[test]
fn mcp_error_data_guides_missing_params_and_unknown_methods() {
    let dir = tempdir().unwrap();
    let engine = TokenZeroEngine::new(EngineConfig::for_root(dir.path()));
    engine.mark_lifecycle_ready_for_tests();

    let missing_tool_name: Value = serde_json::from_str(
        &handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":14,"method":"tools/call","params":{}}"#,
        )
        .unwrap(),
    )
    .unwrap();
    let missing_data = &missing_tool_name["error"]["data"];
    assert_eq!(missing_tool_name["error"]["code"], -32602);
    assert_eq!(missing_data["error_type"], "INVALID_ARGUMENT");
    assert_eq!(missing_data["recoverable"], true);
    assert_eq!(missing_data["param"], "name");
    assert!(
        missing_data["available_options"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool == "tz_read")
    );
    assert!(
        missing_data["fix_hint"]
            .as_str()
            .unwrap()
            .contains("tools/list")
    );
    assert_eq!(
        missing_data["suggested_tool_calls"][0]["method"],
        "tools/list"
    );

    let unknown_method: Value = serde_json::from_str(
        &handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":15,"method":"tools/lits","params":{}}"#,
        )
        .unwrap(),
    )
    .unwrap();
    let method_data = &unknown_method["error"]["data"];
    assert_eq!(unknown_method["error"]["code"], -32601);
    assert_eq!(method_data["error_type"], "NOT_FOUND");
    assert_eq!(method_data["recoverable"], true);
    assert_eq!(method_data["entity_type"], "method");
    assert_eq!(method_data["suggestions"][0]["value"], "tools/list");
    assert!(
        method_data["available_options"]
            .as_array()
            .unwrap()
            .iter()
            .any(|method| method == "tools/call")
    );
    assert_eq!(
        method_data["suggested_tool_calls"][0]["method"],
        "server/discover"
    );
}

#[test]
fn mcp_tool_calls_are_pulse_accounted_with_attribution() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("sample.txt");
    fs::write(&file, "line one\nline two\n").unwrap();
    let engine = TokenZeroEngine::new(EngineConfig::for_root(dir.path()));
    engine.mark_lifecycle_ready_for_tests();

    let read_request = serde_json::json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": {"name": "tz_read", "arguments": {"path": file.display().to_string()}}
    });
    let read_response = handle_jsonrpc(&engine, &read_request.to_string()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&read_response).unwrap();
    fn first_payload_ref(value: &serde_json::Value) -> Option<&str> {
        match value {
            serde_json::Value::String(text) => text
                .split_whitespace()
                .find(|word| word.starts_with("tz://")),
            serde_json::Value::Array(items) => items.iter().find_map(first_payload_ref),
            serde_json::Value::Object(fields) => fields.values().find_map(first_payload_ref),
            _ => None,
        }
    }
    let ref_id = first_payload_ref(&parsed["result"])
        .unwrap_or_else(|| panic!("read response advertises an expandable payload ref: {parsed}"))
        .to_string();

    let expand_request = serde_json::json!({
        "jsonrpc": "2.0", "id": "call-8", "method": "tools/call",
        "params": {"name": "tz_expand", "arguments": {"ref": ref_id}}
    });
    handle_jsonrpc(&engine, &expand_request.to_string()).unwrap();
    let string_id_request = serde_json::json!({
        "jsonrpc": "2.0", "id": "7", "method": "tools/call",
        "params": {"name": "tz_read", "arguments": {"path": file.display().to_string(), "fresh": true}}
    });
    handle_jsonrpc(&engine, &string_id_request.to_string()).unwrap();

    let ledger = dir.path().join(".tokenzero/pulse/events.jsonl");
    let lines: Vec<serde_json::Value> = fs::read_to_string(&ledger)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 3, "one event per tools/call");

    let read_event = &lines[0];
    assert_eq!(read_event["tool"], "read");
    assert_eq!(read_event["session_id"].as_str(), Some(engine.session_id()));
    assert_eq!(read_event["call_id"].as_str(), Some("7"));
    // Pulse forensics keep full-hash refs for durable attribution; the
    // visible capsule advertises the session short alias of the same blob.
    let event_ref_ids = |event: &serde_json::Value| -> Vec<String> {
        event["ref_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect()
    };
    let matches_visible_ref = |full: &str| {
        full == ref_id
            || full.strip_prefix("tz://blob/").is_some_and(|hash| {
                ref_id
                    .strip_prefix("tz://s/")
                    .is_some_and(|short| hash.starts_with(short))
                    || ref_id.starts_with("tz://o/")
            })
    };
    let read_refs = event_ref_ids(read_event);
    assert!(
        read_refs.iter().any(|full| matches_visible_ref(full)),
        "read event refs {read_refs:?} must cover visible ref {ref_id}"
    );
    assert!(read_event["raw_tokens"].as_u64().unwrap_or(0) > 0);

    let expand_event = &lines[1];
    assert_eq!(expand_event["tool"], "expand");
    assert_eq!(expand_event["call_id"].as_str(), Some("\"call-8\""));
    assert_eq!(
        expand_event["session_id"].as_str(),
        Some(engine.session_id())
    );
    let expand_refs = event_ref_ids(expand_event);
    assert!(
        expand_refs.iter().any(|full| matches_visible_ref(full)),
        "expand event refs {expand_refs:?} must carry the expanded ref {ref_id} for attribution"
    );

    let string_id_event = &lines[2];
    assert_eq!(string_id_event["tool"], "read");
    assert_eq!(string_id_event["call_id"].as_str(), Some("\"7\""));
    assert_ne!(
        read_event["call_id"], string_id_event["call_id"],
        "numeric JSON-RPC id 7 and string id \"7\" must not collide"
    );
    assert!(
        expand_event["recovery_tokens"].as_u64().unwrap_or(0) > 0,
        "recovery tokens must be charged on the MCP surface"
    );
}

#[test]
fn session_boot_resource_is_served() {
    let (_dir, engine) = test_engine();
    let response = handle_jsonrpc(
        &engine,
        r#"{"jsonrpc":"2.0","id":1,"method":"resources/read","params":{"uri":"resource://tokenzero/session-boot"}}"#,
    )
    .unwrap();
    let parsed = response_json(&response);
    let text = parsed["result"]["contents"][0]["text"].as_str().unwrap();
    let boot: Value = serde_json::from_str(text).unwrap();
    assert_eq!(boot["schema"], "tokenzero.session-boot.v1");
    assert!(boot["telemetry"]["total"].as_u64().unwrap() < 100);
}

#[test]
fn tool_metrics_resource_is_served() {
    let (_dir, engine) = test_engine();
    let response = handle_jsonrpc(
        &engine,
        r#"{"jsonrpc":"2.0","id":1,"method":"resources/read","params":{"uri":"resource://tokenzero/metrics"}}"#,
    )
    .unwrap();
    let parsed = response_json(&response);
    let text = parsed["result"]["contents"][0]["text"].as_str().unwrap();
    let metrics: Value = serde_json::from_str(text).unwrap();
    // The metrics payload must expose cumulative counters and the slow
    // threshold, not merely contain those words somewhere in the response.
    assert!(
        metrics["cumulative"].is_object(),
        "missing cumulative in metrics: {metrics:#}"
    );
    assert!(
        metrics["slow_threshold_ms"].as_u64().unwrap() > 0,
        "slow_threshold_ms must be positive: {metrics:#}"
    );
}

fn handle_jsonrpc_with_induced_panic(engine: &TokenZeroEngine, line: &str) -> Option<String> {
    handle_jsonrpc_dispatching(engine, line, |engine, item| {
        if item.get("method").and_then(Value::as_str) == Some("tokenzero/internal/test-panic") {
            panic!("test-induced tool panic");
        }
        handle_jsonrpc_request(engine, item)
    })
}

#[test]
fn batch_panic_isolates_sibling_responses() {
    let (_dir, engine) = test_engine();
    let batch = json!([
        {
            "jsonrpc": "2.0",
            "id": 701,
            "method": "tokenzero/internal/test-panic",
            "params": {}
        },
        {
            "jsonrpc": "2.0",
            "id": 702,
            "method": "ping",
            "params": {}
        }
    ]);
    let response = handle_jsonrpc_with_induced_panic(&engine, &batch.to_string()).unwrap();
    let parsed = response_json(&response);
    let responses = parsed
        .as_array()
        .unwrap_or_else(|| panic!("expected batch array, got {parsed:#}"));
    assert_eq!(responses.len(), 2, "{parsed:#}");

    let mut by_id = std::collections::BTreeMap::new();
    for item in responses {
        by_id.insert(item["id"].as_i64().unwrap(), item);
    }
    assert!(by_id.contains_key(&701), "missing panic id 701: {parsed:#}");
    assert!(
        by_id.contains_key(&702),
        "missing sibling id 702: {parsed:#}"
    );

    assert_structured_error(by_id[&701], -32603, Some("INTERNAL"));
    assert_eq!(by_id[&701]["error"]["message"], "Internal error");
    assert!(
        by_id[&702].get("result").is_some(),
        "ping must succeed: {parsed:#}"
    );
    assert!(
        by_id[&702].get("error").is_none(),
        "ping must not error: {parsed:#}"
    );
}

#[test]
fn batch_panic_preserves_notification_suppression() {
    let (_dir, engine) = test_engine();
    // Notification (no id) must stay suppressed even when a sibling panics.
    let batch = json!([
        {
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        },
        {
            "jsonrpc": "2.0",
            "id": 701,
            "method": "tokenzero/internal/test-panic",
            "params": {}
        },
        {
            "jsonrpc": "2.0",
            "id": 702,
            "method": "ping",
            "params": {}
        }
    ]);
    let response = handle_jsonrpc_with_induced_panic(&engine, &batch.to_string()).unwrap();
    let parsed = response_json(&response);
    let responses = parsed
        .as_array()
        .unwrap_or_else(|| panic!("expected batch array, got {parsed:#}"));
    assert_eq!(
        responses.len(),
        2,
        "notification must stay suppressed: {parsed:#}"
    );
    let ids: Vec<_> = responses
        .iter()
        .map(|item| item["id"].as_i64().unwrap())
        .collect();
    assert!(ids.contains(&701), "{parsed:#}");
    assert!(ids.contains(&702), "{parsed:#}");
}

#[test]
fn tools_list_requires_initialize_lifecycle() {
    let dir = tempdir().unwrap();
    let engine = TokenZeroEngine::new(EngineConfig::for_root(dir.path()));

    let before = handle_jsonrpc(
        &engine,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
    )
    .unwrap();
    let before: Value = serde_json::from_str(&before).unwrap();
    assert_eq!(before["error"]["code"], -32600, "{before:#}");
    assert!(
        before["error"]["data"]["reason"]
            .as_str()
            .unwrap_or("")
            .contains("lifecycle"),
        "{before:#}"
    );

    let init = handle_jsonrpc(
        &engine,
        r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"lifecycle","version":"1.0.0"}}}"#,
    )
    .unwrap();
    let init: Value = serde_json::from_str(&init).unwrap();
    assert!(init.get("result").is_some(), "{init:#}");

    let still_gated = handle_jsonrpc(
        &engine,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}"#,
    )
    .unwrap();
    let still_gated: Value = serde_json::from_str(&still_gated).unwrap();
    assert_eq!(still_gated["error"]["code"], -32600, "{still_gated:#}");

    assert!(
        handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        )
        .is_none()
    );

    let after = handle_jsonrpc(
        &engine,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/list","params":{}}"#,
    )
    .unwrap();
    let after: Value = serde_json::from_str(&after).unwrap();
    assert!(after.get("result").is_some(), "{after:#}");
    assert!(after["result"]["tools"].is_array(), "{after:#}");
}
