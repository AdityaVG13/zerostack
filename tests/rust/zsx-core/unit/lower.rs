    use super::*;
    use serde_json::json;

    fn assert_lower(
        surface: &str,
        method: &str,
        input: Value,
        engine: EngineIdentity,
        op: &str,
        args: Value,
    ) {
        let lowered = lower(surface, method, input).unwrap();
        assert_eq!(lowered.0, engine);
        assert_eq!(lowered.1, op);
        assert_eq!(lowered.2, args);
    }

    #[test]
    fn fs_methods_use_canonical_domain_operations() {
        let plan = lower("fs", "plan", json!("map widget entrypoint")).unwrap();
        assert_eq!(plan.0, EngineIdentity::FsZero);
        assert_eq!(plan.1, "fs.searchMany");
        assert_eq!(plan.2["queries"], json!(["widget"]));
        assert_lower(
            "fs",
            "structural",
            json!(["callers", "Widget"]),
            EngineIdentity::FsZero,
            "fs.search",
            json!({"query":"callers:Widget"}),
        );
        assert_lower(
            "fs",
            "compound",
            json!(["read", {"path":"src/lib.rs"}]),
            EngineIdentity::FsZero,
            "fs.read",
            json!({"path":"src/lib.rs"}),
        );
        assert_lower(
            "fs",
            "world",
            json!(["commit", {"world":"W7"}]),
            EngineIdentity::FsZero,
            "fs.world",
            json!({"action":"commit","world":"W7"}),
        );
        assert_lower(
            "fs",
            "world",
            json!("newbatch:a.txt:a|A;;b.txt:b|B"),
            EngineIdentity::FsZero,
            "fs.world",
            json!({"arg":"newbatch:a.txt:a|A;;b.txt:b|B"}),
        );
        assert_lower(
            "fs",
            "read_many",
            json!([["a.rs"], {"max_bytes":32}]),
            EngineIdentity::FsZero,
            "fs.readMany",
            json!({"paths":["a.rs"],"max_bytes":32}),
        );
        assert_lower(
            "fs",
            "search_many",
            json!(["one", "two"]),
            EngineIdentity::FsZero,
            "fs.searchMany",
            json!({"queries":["one","two"]}),
        );
    }

    #[test]
    fn graph_and_token_methods_use_bare_domain_operations() {
        assert_lower(
            "graph",
            "blast",
            json!(["Widget", {"depth":2}]),
            EngineIdentity::GraphZero,
            "blast",
            json!({"intent":"Widget","depth":2}),
        );
        assert_lower(
            "graph",
            "query",
            json!(["symbol", "Widget"]),
            EngineIdentity::GraphZero,
            "query",
            json!({"surface":"symbol","query":"Widget"}),
        );
        assert_lower(
            "token",
            "shell",
            json!(["printf ok", {"timeout_seconds":1}]),
            EngineIdentity::TokenZero,
            "shell",
            json!({"command":"printf ok","timeout_seconds":1}),
        );
        assert_lower(
            "token",
            "find",
            json!("Widget"),
            EngineIdentity::TokenZero,
            "find",
            json!({"query":"Widget"}),
        );
        assert_lower(
            "token",
            "expand",
            json!("g:42"),
            EngineIdentity::GraphZero,
            "expand",
            json!({"reference":"g:42"}),
        );
        assert_lower(
            "token",
            "expand",
            json!("q:abc"),
            EngineIdentity::GraphZero,
            "expand",
            json!({"reference":"q:abc"}),
        );
    }

    #[test]
    fn fs_edit_and_write_lower_to_fszero_with_args_passthrough() {
        assert!(METHODS.contains(&("fs", "edit")));
        assert!(METHODS.contains(&("fs", "write")));
        assert_lower(
            "fs",
            "edit",
            json!([{"path":"a.rs","find":"old","replace":"new",
                    "base":"fz://blob/aa"}]),
            EngineIdentity::FsZero,
            "fs.edit",
            json!({"path":"a.rs","find":"old","replace":"new",
                   "base":"fz://blob/aa"}),
        );
        assert_lower(
            "fs",
            "write",
            json!([{"path":"b.txt","content":"x","base":null}]),
            EngineIdentity::FsZero,
            "fs.write",
            json!({"path":"b.txt","content":"x","base":null}),
        );
        // Bare object (non-positional) form also passes through.
        assert_lower(
            "fs",
            "write",
            json!({"path":"c.txt","content":"y"}),
            EngineIdentity::FsZero,
            "fs.write",
            json!({"path":"c.txt","content":"y"}),
        );
        for input in [json!([]), json!(["c.txt"]), json!([{}, {}]), json!("c.txt")] {
            assert!(lower("fs", "edit", input.clone()).is_err());
            assert!(lower("fs", "write", input).is_err());
        }
        // compound must not smuggle a scalar past the write/edit object rule.
        for (name, scalar) in [
            ("write", json!("c.txt")),
            ("verifiedEdit", json!("c.txt")),
            ("edit", json!("c.txt")),
        ] {
            let err = lower("fs", "compound", json!([name, scalar])).unwrap_err();
            assert!(
                err.to_string().contains("exactly one options object"),
                "{name}: {err}"
            );
        }
        // Tool-result / provenance objects must not stringify into file bytes.
        let tool_result = json!({"ack":"C","content":{"kind":"inline","value":"secret"}});
        for input in [
            json!({"path":"tracked.txt","content": tool_result.clone()}),
            json!([{"path":"tracked.txt","content": {"kind":"ref","ref":"fz://blob/abc"}}]),
        ] {
            let err = lower("fs", "write", input).unwrap_err();
            assert!(
                err.to_string().contains("non_byte_provenance"),
                "direct write: {err}"
            );
        }
        let err = lower(
            "fs",
            "compound",
            json!(["write", {"path":"tracked.txt","content": tool_result.clone()}]),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("non_byte_provenance")
                && err.to_string().contains("ctx.payload"),
            "compound write: {err}"
        );
        let err = lower(
            "fs",
            "write",
            json!({"path":"tracked.txt","content":"[object Object]"}),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("non_byte_provenance"),
            "js object coercion: {err}"
        );
        let err = lower(
            "fs",
            "transact",
            json!([{"op":"write","path":"tracked.txt","content": tool_result}]),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("non_byte_provenance"),
            "transact write: {err}"
        );

        assert!(METHODS.contains(&("fs", "transact")));
        let steps = json!([
            {"op":"edit","path":"a.rs","find":"x","replace":"y"},
            {"op":"write","path":"b.txt","content":"z","base":null}
        ]);
        assert_lower(
            "fs",
            "transact",
            json!([steps.clone()]),
            EngineIdentity::FsZero,
            "fs.transact",
            json!({"steps": steps.clone()}),
        );
        // Spread form: zero.fs.transact(step, step).
        assert_lower(
            "fs",
            "transact",
            steps.clone(),
            EngineIdentity::FsZero,
            "fs.transact",
            json!({"steps": steps}),
        );
        for input in [json!([]), json!("steps"), json!(["a", "b"])] {
            assert!(lower("fs", "transact", input).is_err());
        }

        assert!(METHODS.contains(&("fs", "multi_edit")));
        let implicit = json!([
            {"path":"a.rs","find":"x","replace":"y"},
            {"path":"b.txt","content":"z","base":null}
        ]);
        let expected_steps = json!([
            {"path":"a.rs","find":"x","replace":"y","op":"edit"},
            {"path":"b.txt","content":"z","base":null,"op":"write"}
        ]);
        assert_lower(
            "fs",
            "multi_edit",
            implicit.clone(),
            EngineIdentity::FsZero,
            "fs.transact",
            json!({"steps": expected_steps}),
        );
        let err = lower(
            "fs",
            "multi_edit",
            json!([{"op":"write","path":"tracked.txt","content": tool_result}]),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("non_byte_provenance"),
            "multi_edit write: {err}"
        );
    }

    #[test]
    fn token_read_and_shell_options_are_strict_and_forwarded_once() {
        assert!(METHODS.contains(&("token", "read")));
        assert_lower(
            "token",
            "read",
            json!(["fresh-raw.txt", {
                "mode":"exact","start_line":1,"end_line":2,"raw":true,
                "fresh":true,"max_files":1,"max_visible_tokens":512
            }]),
            EngineIdentity::TokenZero,
            "read",
            json!({
                "path":"fresh-raw.txt","mode":"exact","start_line":1,
                "end_line":2,"raw":true,"fresh":true,"max_files":1,
                "max_visible_tokens":512
            }),
        );
        assert_lower(
            "token",
            "read",
            json!(["one.txt", "two.txt"]),
            EngineIdentity::TokenZero,
            "read",
            json!({"path":["one.txt","two.txt"]}),
        );
        assert_lower(
            "token",
            "shell",
            json!([["printf", "ok"], {
                "cwd":".","mode":"exact","rewrite":"off","no_rewrite":true,
                "stdin":"input","timeout_ms":25,"timeout_seconds":1,"background":false
            }]),
            EngineIdentity::TokenZero,
            "shell",
            json!({
                "command":["printf","ok"],"cwd":".","mode":"exact",
                "rewrite":"off","no_rewrite":true,"stdin":"input",
                "timeout_ms":25,"timeout_seconds":1,"background":false
            }),
        );

        for input in [
            json!(["file", {"unknown":true}]),
            json!(["file", {"fresh":"yes"}]),
            json!(["file", {"max_files":0}]),
            json!(["file", {}, "extra"]),
        ] {
            assert!(lower("token", "read", input).is_err());
        }
        let shell_raw = lower(
            "token",
            "shell",
            json!(["printf must-not-run", {"raw":true}]),
        )
        .unwrap_err()
        .to_string();
        assert!(shell_raw.contains("unknown option 'raw'"), "{shell_raw}");
        assert!(shell_raw.contains(r#"mode: "exact""#), "{shell_raw}");
        assert!(lower("token", "shell", json!(["printf ok", {"timeout_ms":0}])).is_err());
        assert!(lower("token", "shell", json!(["printf ok", {}, "extra"])).is_err());
    }

    #[test]
    fn token_job_lowering_uses_the_shared_typed_request() {
        assert!(METHODS.contains(&("token", "job")));
        assert_lower(
            "token",
            "job",
            json!("tzjob-7"),
            EngineIdentity::TokenZero,
            "job",
            json!({"id":"tzjob-7","waitMs":30000,"since":0,"tailBytes":8192}),
        );
        assert_lower(
            "token",
            "job",
            json!(["tzjob-7", {"waitMs":25,"since":9,"tailBytes":64}]),
            EngineIdentity::TokenZero,
            "job",
            json!({"id":"tzjob-7","waitMs":25,"since":9,"tailBytes":64}),
        );
        assert!(lower("token", "job", json!(["tzjob-7", {"extra":true}])).is_err());
        assert!(lower("token", "job", json!(["tzjob-7", {"tailBytes":0}])).is_err());
        assert!(lower("token", "job", json!(["tzjob-7", {}, "extra"])).is_err());
    }

    #[test]
    fn expansion_routes_to_the_ref_owner() {
        for (reference, engine, op, key) in [
            ("fz://blob/00", EngineIdentity::FsZero, "fs.expand", "ref"),
            (
                "gz://blob/00",
                EngineIdentity::GraphZero,
                "expand",
                "reference",
            ),
            ("tz://blob/00", EngineIdentity::TokenZero, "expand", "ref"),
        ] {
            let mut expected = serde_json::Map::new();
            expected.insert(key.into(), Value::String(reference.into()));
            assert_lower(
                "token",
                "expand",
                json!(reference),
                engine,
                op,
                Value::Object(expected),
            );
        }
        assert!(lower("token", "expand", json!("https://invalid")).is_err());
        // `gz://` must win over the `g:` prefix (first-match table order).
        assert_lower(
            "token",
            "expand",
            json!("g:42"),
            EngineIdentity::GraphZero,
            "expand",
            json!({"reference":"g:42"}),
        );
        assert!(engine_for("help").is_err(), "help is not an engine surface");
        assert!(engine_for("fs").is_ok());
    }
