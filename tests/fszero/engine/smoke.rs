#[path = "../common/mod.rs"]
mod common;

use common::{TestRoot, assert_one_token, codemode_tree, env_var, expand_text, sample_workspace};
use fs_zero::{
    ERROR_REF, FSZeroSession, FsConnector, RESULT_REF, STEPS_REF, SurfaceKind, TELEMETRY_REF,
    codemode_execute_plan, estimate_visible_tokens, parse_program, surfaces,
};
use serde_json::Value;
use std::fs;

#[test]
fn kernel_ls_read_search_one_token() {
    let (root, _) = sample_workspace();
    let mut s = FSZeroSession::with_root(&root);
    for (op, arg) in [
        ('L', None),
        ('S', Some("process_request")),
        ('R', Some("src/main.rs")),
    ] {
        let (tok, ok, _) = s.execute(op, arg);
        assert!(ok);
        assert_one_token(&tok);
    }
    let payload = expand_text(&s, "search");
    assert!(payload.contains("process_request"));
}

#[test]
fn kernel_edit_recovery_and_restore() {
    let (root, pristine) = sample_workspace();
    let mut s = FSZeroSession::with_root(&root);
    let main = root.join("src/main.rs");
    let (tok, ok, _) = s.execute(
        'E',
        Some("src/main.rs:hello from main|hello from main EDITED"),
    );
    assert!(ok);
    assert_one_token(&tok);
    assert!(fs::read_to_string(&main).unwrap().contains("EDITED"));
    assert!(s.has_recovery_payloads());
    fs::write(&main, pristine).unwrap();
}

#[test]
fn repo_store_persists_refs() {
    let (root, _) = sample_workspace();
    let read_ref = {
        let mut s = FSZeroSession::with_repo_store(&root);
        let (tok, ok, _) = s.execute('R', Some("src/main.rs"));
        assert!(ok);
        assert_one_token(&tok);
        expand_text(&s, &format!("view_{}/ref", tok.trim_start_matches('R')))
    };
    assert!(root.join(".fszero").is_dir());
    let reopened = FSZeroSession::with_repo_store(&root);
    assert!(expand_text(&reopened, &read_ref).contains("process_request"));
}

#[test]
fn repo_store_startup_defers_index_but_read_works() {
    let (root, _) = sample_workspace();
    let mut s = FSZeroSession::with_repo_store(&root);

    let (tok, ok, detail) = s.execute('R', Some("src/main.rs"));

    assert!(ok, "read failed: {detail:?}");
    assert_one_token(&tok);
    assert!(
        expand_text(&s, &format!("view_{}/bytes", tok.trim_start_matches('R')))
            .contains("process_request")
    );
}

#[test]
fn search_builds_deferred_index_on_first_use() {
    let (root, _) = sample_workspace();
    let mut s = FSZeroSession::with_repo_store(&root);

    let (_, ok, detail) = s.execute('S', Some("process_request"));

    assert!(ok, "search failed: {detail:?}");
    assert!(expand_text(&s, "search").contains("process_request"));
}

#[test]
fn search_refreshes_after_external_edit() {
    let root = TestRoot::new("refresh");
    root.write("src/main.rs", "fn main() { let needle = 1; }\n");
    let mut s = FSZeroSession::with_root(&root);
    assert!(s.execute('S', Some("needle")).1);
    fs::write(root.join("src/main.rs"), "fn main() { let needle = 99; }\n").unwrap();
    let (_, ok, _) = s.execute('S', Some("needle"));
    assert!(ok);
    assert!(expand_text(&s, "search").contains("99"));
}

#[test]
fn budgets_enforced() {
    let root = TestRoot::new("budget");
    root.write("src/main.rs", "use std::io;\nfn main() {}\n");
    let startup = env_var("FSZERO_STARTUP_INDEX", "1");
    let mut s = FSZeroSession::with_root(&root);
    drop(startup);
    let _a = env_var("FSZERO_BUDGET_AST_NODES", "0");
    let (_, ok, detail) = s.execute('S', Some("imports"));
    drop(_a);
    assert!(!ok);
    assert!(detail.unwrap().contains("budget:0 ast_nodes"));
}

#[test]
fn codemode_json_plan_one_token() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);
    let plan = r#"{"label":"t","steps":[
        {"call":"fs.ls","args":{}},
        {"call":"fs.search","args":{"query":"process_request"}},
        {"call":"fs.read","args":{"path":"src/main.rs"}}
    ]}"#;
    assert_eq!(codemode_execute_plan(&mut s, plan), "C");
    assert_one_token("C");
    let steps = expand_text(&s, STEPS_REF);
    assert!(steps.contains("method=fs.ls"));
    let telemetry = expand_text(&s, TELEMETRY_REF);
    let telemetry_json: Value = serde_json::from_str(&telemetry).unwrap();
    assert_eq!(telemetry_json["extra"]["steps_run"], 3);
    assert_eq!(telemetry_json["internal_actions"], 3);
}

/// fszero-ht91: sequential plans in one durable-store session must each open
/// and close their own execution transaction. A leaked BEGIN surfaced as
/// "payload begin: internal error: cannot start a transaction within a
/// transaction" on the second call.
#[test]
fn sequential_codemode_plans_do_not_leak_a_transaction() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_repo_store(&root);

    let read_plan = r#"{"label":"seq_read","steps":[
        {"call":"fs.search","args":{"query":"process_request"}},
        {"call":"fs.read","args":{"path":"src/main.rs"}}
    ]}"#;
    // A failing plan rolls back its journal; the exec txn must still close.
    let failing_plan = r#"{"label":"seq_fail","steps":[
        {"call":"fs.compound","args":{"name":"mutate","path":"src/main.rs","old":"ABSENT_ANCHOR_ht91","new":"x"}}
    ]}"#;
    let write_plan = r#"{"label":"seq_write","steps":[
        {"call":"fs.write","args":{"path":"ht91.txt","content":"ht91"}}
    ]}"#;

    for (i, plan) in [read_plan, read_plan, write_plan, read_plan]
        .iter()
        .enumerate()
    {
        let ack = codemode_execute_plan(&mut s, plan);
        let detail = s
            .expand("codemode/error")
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        assert_eq!(ack, "C", "sequential plan {i} failed: {detail}");
        assert!(
            !detail.contains("transaction within a transaction"),
            "plan {i} leaked a transaction: {detail}"
        );
    }

    assert_ne!(codemode_execute_plan(&mut s, failing_plan), "C");
    let after_failure = codemode_execute_plan(&mut s, read_plan);
    let detail = s
        .expand("codemode/error")
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    assert_eq!(
        after_failure, "C",
        "a failed plan must not leave an open transaction: {detail}"
    );
}

#[test]
fn codemode_recipes_and_bindings() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);
    for recipe in ["explore", "impact:process_request"] {
        let ack = codemode_execute_plan(&mut s, recipe);
        assert_eq!(ack, "C");
        assert_one_token(&ack);
    }
    let plan = r#"{"label":"bind","steps":[
        {"call":"fs.search","args":{"query":"defs:process_request"}},
        {"call":"fs.search","args":{"query":"helper"}},
        {"call":"fs.read","args":{"path":"$step0.path"}},
        {"call":"fs.read","args":{"path":"$step1.path"}}
    ]}"#;
    assert_eq!(codemode_execute_plan(&mut s, plan), "C");
    assert_eq!(expand_text(&s, STEPS_REF).matches("fs.read").count(), 2);
}

#[test]
fn codemode_parse_rejects_invalid_programs() {
    let cases = [
        (
            r#"{"steps":[{"call":"fs.nope","args":{}}]}"#,
            "unknown call",
        ),
        (
            r#"{"steps":[{"call":"fs.ls","args":{},"parallel":[{"id":"a","call":"fs.search","args":{"query":"x"}}]}]}"#,
            "both call and parallel",
        ),
        (
            r#"{"steps":[{"id":"x","call":"fs.ls","args":{}},{"parallel":[{"id":"x","call":"fs.search","args":{"query":"y"}}]}]}"#,
            "collides",
        ),
    ];
    for (json, needle) in cases {
        let err = parse_program(json).unwrap_err();
        assert!(err.contains(needle), "got: {err}");
    }
}

#[test]
fn codemode_transaction_rolls_back_on_failure() {
    let root = TestRoot::new("txn");
    root.write("src/main.rs", "fn main() { let x = 1; }\n");
    let mut s = FSZeroSession::with_root(&root);
    let plan = r#"{"transaction":true,"steps":[
        {"call":"fs.write","args":{"path":"src/main.rs","content":"fn main() { let x = 2; }\n"}},
        {"call":"fs.edit","args":{"spec":"src/main.rs:let x = 1|let x = 3"}}
    ]}"#;
    let ack = codemode_execute_plan(&mut s, plan);
    assert_ne!(ack, "C");
    assert!(
        fs::read_to_string(root.join("src/main.rs"))
            .unwrap()
            .contains("let x = 1")
    );
    let telemetry: Value = serde_json::from_str(&expand_text(&s, TELEMETRY_REF)).unwrap();
    assert_eq!(telemetry["extra"]["transaction_rolled_back"], true);
}

/// fszero-quer: durable CodeMode failures roll back the exec SQLite overlay
/// (pending put_keys). ERROR_REF must still expand so the CLI can print the
/// real reason instead of "no recorded reason".
#[test]
fn codemode_failure_reason_survives_exec_txn_rollback() {
    let root = TestRoot::new("quer_error_survives");
    root.write("src/main.rs", "fn main() {}\n");
    let mut s = FSZeroSession::with_repo_store(root.path());
    // Mutation + later failure: file journal rolls back AND plan.rs rolls
    // back the exec txn that held ERROR_REF in the pending overlay.
    let plan = r#"{"transaction":true,"steps":[
        {"call":"fs.write","args":{"path":"src/main.rs","content":"partial\n"}},
        {"call":"fs.read","args":{"path":"absent-quer.txt"}}
    ]}"#;
    let ack = codemode_execute_plan(&mut s, plan);
    assert_ne!(ack, "C", "plan must fail");
    assert_eq!(
        fs::read_to_string(root.join("src/main.rs")).unwrap(),
        "fn main() {}\n",
        "journal must restore preimage"
    );
    let err = s
        .expand(ERROR_REF)
        .expect("ERROR_REF must survive durable exec-txn rollback");
    let text = String::from_utf8_lossy(&err);
    assert!(
        !text.is_empty() && !text.contains("no recorded reason"),
        "failure reason must be non-empty: {text}"
    );
    assert!(
        text.contains("absent-quer")
            || text.contains("not found")
            || text.contains("No such file")
            || text.contains("missing")
            || text.contains("read"),
        "reason must identify the failing read, got: {text}"
    );
}

#[test]
fn codemode_auto_transaction_rolls_back_write_create_and_overwrite() {
    let root = TestRoot::new("txn_write_auto");
    root.write("existing.txt", "before");
    let mut session = FSZeroSession::with_root(&root);
    let plan = r#"{"steps":[{"call":"fs.write","args":{"path":"existing.txt","content":"after"}},{"call":"fs.write","args":{"path":"new/child/created.txt","content":"new"}},{"call":"fs.edit","args":{"spec":"missing.txt:x|y"}}]}"#;
    assert_ne!(codemode_execute_plan(&mut session, plan), "C");
    assert_eq!(
        fs::read_to_string(root.join("existing.txt")).unwrap(),
        "before"
    );
    assert!(!root.join("new/child/created.txt").exists());
    assert!(!root.join("new/child").exists());
    assert!(!root.join("new").exists());
    let telemetry: Value = serde_json::from_str(&expand_text(&session, TELEMETRY_REF)).unwrap();
    assert_eq!(telemetry["extra"]["transaction_rolled_back"], true);
}

#[test]
fn codemode_statement_position_write_then_return() {
    // fszero-j7r: `await zero.fs.write(...);return{...};` (result discarded,
    // no space after `;`) was wrapped as `return (a;b;)` — a SyntaxError —
    // so the plan died with X0 and the write never happened.
    let root = TestRoot::new("stmt_write");
    let mut s = FSZeroSession::with_root(&root);
    let plan = r#"await zero.fs.write({path:"j7r.txt",content:"v0"});return{done:true};"#;
    assert_eq!(codemode_execute_plan(&mut s, plan), "C");
    assert_eq!(fs::read_to_string(root.join("j7r.txt")).unwrap(), "v0");
}

#[test]
fn codemode_bare_write_expression_returns_ack() {
    // A single mutation expression with a trailing semicolon must run and
    // yield its op result (previously `return (expr;)` — SyntaxError).
    let root = TestRoot::new("bare_write");
    let mut s = FSZeroSession::with_root(&root);
    let plan = r#"zero.fs.write({path:"bare.txt",content:"b1"});"#;
    assert_eq!(codemode_execute_plan(&mut s, plan), "C");
    assert_eq!(fs::read_to_string(root.join("bare.txt")).unwrap(), "b1");
}

#[test]
fn memory_volume_survives_repo_store_restart() {
    let root = TestRoot::new("mem_vol");
    root.write("src/main.rs", "fn main() {}\n");
    {
        let mut s = FSZeroSession::with_repo_store(&root);
        let (tok, ok, detail) =
            s.execute('M', Some("put:system/constraints.md|always use uv not pip"));
        assert!(ok, "{tok} {:?}", detail);
    }
    let mut s = FSZeroSession::with_repo_store(&root);
    let (tok, ok, detail) = s.execute('M', Some("get:system/constraints.md"));
    assert!(ok, "{tok} {:?}", detail);
    let detail = detail.unwrap_or_default();
    assert!(
        !detail.contains("always use uv not pip"),
        "get must be ref-first (no body in detail): {detail}"
    );
    assert!(
        detail.contains("ref=fz://blob/"),
        "get must mint ref: {detail}"
    );
    let mem_bytes = s.expand("memory").expect("memory payload");
    let body = String::from_utf8_lossy(&mem_bytes);
    assert!(
        body.contains("always use uv not pip"),
        "memory must rehydrate via expand: {body}"
    );
    let (_, ok, detail) = s.execute('M', Some("ls:system"));
    assert!(ok, "ls failed detail={detail:?}");
    assert_eq!(
        detail.as_deref(),
        Some("mem:1 ls count=1"),
        "ls after restart must see the durable path"
    );
}

/// get and ls must not share recovery key `memory` — ls used to overwrite get.
#[test]
fn memory_get_expand_must_not_be_clobbered_by_ls() {
    let root = TestRoot::new("mem_clobber");
    root.write("src/main.rs", "fn main() {}\n");
    let mut s = FSZeroSession::with_repo_store(&root);
    let (tok, ok, _) = s.execute('M', Some("put:system/note.md|get-body-must-survive-ls"));
    assert!(ok, "{tok}");
    let (tok, ok, detail) = s.execute('M', Some("get:system/note.md"));
    assert!(ok, "{tok}");
    let detail = detail.unwrap_or_default();
    let blob_ref = detail
        .split_whitespace()
        .find(|t| t.starts_with("ref=fz://blob/"))
        .map(|t| t.trim_start_matches("ref=").to_string())
        .expect("get must mint fz://blob ref");
    let get_bytes = s.expand("memory").expect("get body under memory");
    assert_eq!(
        String::from_utf8_lossy(&get_bytes),
        "get-body-must-survive-ls"
    );
    let blob_bytes = s.expand(&blob_ref).expect("content ref expand");
    assert_eq!(blob_bytes, get_bytes);

    let (tok, ok, _) = s.execute('M', Some("ls:system"));
    assert!(ok, "{tok}");
    let after_ls = s.expand("memory").expect("get body must survive ls");
    assert_eq!(
        String::from_utf8_lossy(&after_ls),
        "get-body-must-survive-ls",
        "ls must not clobber recovery key memory"
    );
    assert_eq!(
        s.expand(&blob_ref).expect("blob ref after ls"),
        get_bytes,
        "fz://blob expand must still return get body after ls"
    );
    let ls_bytes = s.expand("memory/ls").expect("ls under memory/ls");
    assert!(
        String::from_utf8_lossy(&ls_bytes).contains("system/note.md"),
        "ls listing parked under memory/ls"
    );
}

#[test]
fn memory_mcp_tools_return_ref_first_acks() {
    let root = TestRoot::new("mem_mcp");
    root.write("src/main.rs", "fn main() {}\n");
    let mut s = FSZeroSession::with_repo_store(&root);
    let put = SurfaceKind::PerOp
        .call_tool(
            &mut s,
            "fszero.memory_put",
            &serde_json::json!({
                "path": "system/rules.md",
                "content": "prefer CodeMode for multi-step work"
            }),
        )
        .unwrap();
    assert!(
        put["structuredContent"]["ack"]
            .as_str()
            .unwrap_or("")
            .starts_with('M'),
        "put ack must be Memory opcode: {put}"
    );
    assert_eq!(put["structuredContent"]["ok"], true);
    let refs = put["structuredContent"]["refs"].as_array().unwrap();
    assert!(
        refs.iter()
            .any(|r| r.as_str().unwrap_or("").starts_with("fz://blob/")),
        "put must mint a content ref: {put}"
    );

    let get = SurfaceKind::PerOp
        .call_tool(
            &mut s,
            "fszero.memory_get",
            &serde_json::json!({ "path": "system/rules.md" }),
        )
        .unwrap();
    assert!(
        get["structuredContent"]["ack"]
            .as_str()
            .unwrap_or("")
            .starts_with('M')
    );
    let get_detail = get["structuredContent"]["detail"]
        .as_str()
        .or_else(|| get["content"][0]["text"].as_str())
        .unwrap_or("");
    assert!(
        !get_detail.contains("prefer CodeMode"),
        "get detail must not inline body: {get}"
    );
    assert!(
        get["structuredContent"]["refs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r.as_str().unwrap_or("").starts_with("fz://blob/"))
    );

    let renamed = SurfaceKind::PerOp
        .call_tool(
            &mut s,
            "fszero.memory_rename",
            &serde_json::json!({
                "from": "system/rules.md",
                "to": "system/rules-v2.md"
            }),
        )
        .unwrap();
    assert_eq!(renamed["structuredContent"]["ok"], true);
    let deleted = SurfaceKind::PerOp
        .call_tool(
            &mut s,
            "fszero.memory_delete",
            &serde_json::json!({ "path": "system/rules-v2.md" }),
        )
        .unwrap();
    assert_eq!(deleted["structuredContent"]["ok"], true);
}

/// Paths containing `|` must round-trip through put path|content wire
/// (MCP / CodeMode encode `%7C`; kernel decodes before store).
#[test]
fn memory_put_rejects_or_preserves_pipe_in_path() {
    let root = TestRoot::new("mem_pipe");
    root.write("src/main.rs", "fn main() {}\n");
    let mut s = FSZeroSession::with_repo_store(&root);
    let path = "weird|name.md";
    let content = "pipe-path-body|still-content";

    let put = SurfaceKind::PerOp
        .call_tool(
            &mut s,
            "fszero.memory_put",
            &serde_json::json!({ "path": path, "content": content }),
        )
        .unwrap();
    assert_eq!(
        put["structuredContent"]["ok"], true,
        "MCP put with pipe in path must succeed: {put}"
    );

    let get = SurfaceKind::PerOp
        .call_tool(
            &mut s,
            "fszero.memory_get",
            &serde_json::json!({ "path": path }),
        )
        .unwrap();
    assert_eq!(
        get["structuredContent"]["ok"], true,
        "get must find pipe path: {get}"
    );
    assert_eq!(expand_text(&s, "memory"), content);

    let _ = SurfaceKind::PerOp
        .call_tool(
            &mut s,
            "fszero.memory_ls",
            &serde_json::json!({ "prefix": "" }),
        )
        .unwrap();
    let listed = expand_text(&s, "memory/ls");
    assert!(
        listed.lines().any(|l| l == path),
        "ls must list full path with pipe, not truncated: {listed}"
    );
    assert!(
        !listed.lines().any(|l| l == "weird"),
        "must not store truncated path 'weird': {listed}"
    );

    let renamed = SurfaceKind::PerOp
        .call_tool(
            &mut s,
            "fszero.memory_rename",
            &serde_json::json!({
                "from": path,
                "to": "other|renamed.md"
            }),
        )
        .unwrap();
    assert_eq!(
        renamed["structuredContent"]["ok"], true,
        "rename with pipe paths must succeed: {renamed}"
    );

    let mut conn = FsConnector::new(&mut s);
    let step = conn.memory_put("a|b/c.md", "via-connector");
    assert!(
        step.ok,
        "CodeMode put must preserve pipe: {:?}",
        step.detail
    );
    let got = conn.memory_get("a|b/c.md");
    assert!(got.ok, "{:?}", got.detail);
    assert_eq!(expand_text(&s, "memory"), "via-connector");
}

#[test]
fn memory_codemode_recipe_and_js_surface() {
    let root = TestRoot::new("mem_cm");
    root.write("src/main.rs", "fn main() {}\n");
    let mut s = FSZeroSession::with_repo_store(&root);
    let ack = codemode_execute_plan(
        &mut s,
        "memory:put:system/persona.md|you are a careful coding agent",
    );
    assert_eq!(ack, "C");
    let ack = codemode_execute_plan(&mut s, "memory:get:system/persona.md");
    assert_eq!(ack, "C");
    let result = expand_text(&s, "codemode/result");
    let mem = expand_text(&s, "memory");
    assert!(
        mem.contains("careful coding agent")
            || result.contains("careful coding agent")
            || result.contains("fz://blob/"),
        "recipe get must recover memory bytes or ref: result={result} mem={mem}"
    );

    let ack = codemode_execute_plan(
        &mut s,
        r#"export default function ({ fs }) {
  const w = fs.memory.put({ path: "facts/lib.md", content: "use uv" });
  const r = fs.memory.get({ path: "facts/lib.md" });
  const ren = fs.memory.rename({ from: "facts/lib.md", to: "facts/uv.md" });
  const del = fs.memory.delete({ path: "facts/uv.md" });
  return { ok: w.ok && r.ok && ren.ok && del.ok, ref: r.ref };
}"#,
    );
    if ack != "C" {
        eprintln!("js error={}", expand_text(&s, "codemode/error"));
        eprintln!("js result={}", expand_text(&s, "codemode/result"));
    }
    assert_eq!(ack, "C", "js fs.memory.* must succeed");
}

#[test]
fn expand_line_window_is_exact_and_bypasses_seen_set() {
    let root = TestRoot::new("expand_window");
    let body: String = (1..=50).map(|i| format!("line-{i}\n")).collect();
    root.write("src/big.txt", &body);
    let mut s = FSZeroSession::with_repo_store(&root);
    let (_, ok, detail) = s.execute('R', Some("src/big.txt"));
    assert!(ok, "{:?}", detail);
    let detail = detail.unwrap_or_default();
    let cref = detail
        .split("ref=")
        .nth(1)
        .map(|s| s.trim().to_string())
        .expect("read must mint ref");
    // Windowed expand returns EXACTLY the requested lines.
    let arg = format!("{cref}#L10-12");
    let (tok, ok, d1) = s.execute('X', Some(&arg));
    assert!(ok, "{tok} {:?}", d1);
    assert_eq!(d1.as_deref(), Some("X:ok"));
    let bytes = s.expand("expand").expect("window payload");
    assert_eq!(
        String::from_utf8_lossy(&bytes),
        "line-10\nline-11\nline-12\n"
    );
    // Repeating the SAME window still delivers (windows bypass the seen-set).
    let (tok, ok, d2) = s.execute('X', Some(&arg));
    assert!(ok, "{tok} {:?}", d2);
    assert_eq!(
        d2.as_deref(),
        Some("X:ok"),
        "windows must not dedupe to unchanged"
    );
    let again = s.expand("expand").expect("repeat window payload");
    assert_eq!(
        String::from_utf8_lossy(&again),
        "line-10\nline-11\nline-12\n"
    );
    // Malformed windows reject instead of resolving the wrong content.
    let (_, ok, d3) = s.execute('X', Some(&format!("{cref}#L12-10")));
    assert!(!ok, "reversed window must reject: {:?}", d3);
}

#[test]
fn world_single_commit_and_drop() {
    let root = TestRoot::new("world");
    root.write("src/main.rs", "hello from main\n");
    let main = root.join("src/main.rs");
    let mut s = FSZeroSession::with_root(&root);
    let (_, ok, _) = s.execute(
        'W',
        Some("new:src/main.rs:hello from main|hello from world"),
    );
    assert!(ok);
    assert!(
        !fs::read_to_string(&main)
            .unwrap()
            .contains("hello from world")
    );
    let (_, ok, _) = s.execute('W', Some("commit:W1"));
    assert!(ok);
    assert!(
        fs::read_to_string(&main)
            .unwrap()
            .contains("hello from world")
    );
    assert!(!s.execute('W', Some("drop:W1")).1);
}

#[test]
fn world_survives_repo_store_restart() {
    let root = TestRoot::new("world_persist");
    root.write("src/main.rs", "hello from main\n");
    let main = root.join("src/main.rs");
    {
        let mut s = FSZeroSession::with_repo_store(&root);
        let (tok, ok, _) = s.execute(
            'W',
            Some("new:src/main.rs:hello from main|hello from durable world"),
        );
        assert!(ok, "{tok}");
        assert!(
            !fs::read_to_string(&main)
                .unwrap()
                .contains("hello from durable world")
        );
    }
    // New process-equivalent session: active world must rehydrate from SQLite.
    let mut s = FSZeroSession::with_repo_store(&root);
    let (tok, ok, _) = s.execute('W', Some("commit:W1"));
    assert!(ok, "rehydrated world commit failed: {tok}");
    assert!(
        fs::read_to_string(&main)
            .unwrap()
            .contains("hello from durable world")
    );
}

#[test]
fn world_batch_atomic_and_stale_abort() {
    let root = TestRoot::new("batch");
    root.write("src/a.txt", "alpha\n");
    root.write("src/b.txt", "beta\n");
    let mut s = FSZeroSession::with_root(&root);
    let (_, ok, _) = s.execute(
        'W',
        Some("newbatch:src/a.txt:alpha|ALPHA;;src/b.txt:beta|BETA"),
    );
    assert!(ok);
    let (_, ok, _) = s.execute('W', Some("commit:W1"));
    assert!(ok);
    assert_eq!(
        fs::read_to_string(root.join("src/a.txt")).unwrap(),
        "ALPHA\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("src/b.txt")).unwrap(),
        "BETA\n"
    );

    let mut s2 = FSZeroSession::with_root(&root);
    let (_, ok, _) = s2.execute(
        'W',
        Some("newbatch:src/a.txt:ALPHA|ALPHA2;;src/b.txt:BETA|BETA2"),
    );
    assert!(ok);
    fs::write(root.join("src/b.txt"), "mutated\n").unwrap();
    assert!(!s2.execute('W', Some("commit:W1")).1);
    assert_eq!(
        fs::read_to_string(root.join("src/a.txt")).unwrap(),
        "ALPHA\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("src/b.txt")).unwrap(),
        "mutated\n"
    );
}

fn world_op(s: &mut FSZeroSession, arg: &str) -> (bool, String) {
    let (_, ok, detail) = s.execute('W', Some(arg));
    (ok, detail.unwrap_or_default())
}

#[test]
fn world_fork_and_edit_accumulation() {
    // fszero-ap9: fork is a first-class O(1) op; edits accumulate afterwards.
    let root = TestRoot::new("world_fork");
    root.write("src/a.txt", "l1\nl2\nl3\nl4\nl5\n");
    let mut s = FSZeroSession::with_root(&root);
    let (ok, detail) = world_op(&mut s, "fork");
    assert!(ok, "{detail}");
    assert!(detail.contains("W1"), "{detail}");
    let (ok, detail) = world_op(&mut s, "edit:W1:src/a.txt:l2|L2");
    assert!(ok, "{detail}");
    assert!(detail.contains("hunk=2-2"), "{detail}");
    // Staging never touches disk.
    assert_eq!(
        fs::read_to_string(root.join("src/a.txt")).unwrap(),
        "l1\nl2\nl3\nl4\nl5\n"
    );
    let (ok, detail) = world_op(&mut s, "commit:W1");
    assert!(ok, "{detail}");
    assert_eq!(
        fs::read_to_string(root.join("src/a.txt")).unwrap(),
        "l1\nL2\nl3\nl4\nl5\n"
    );
    // Unknown world / bad specs fail loudly.
    assert!(!world_op(&mut s, "edit:W9:src/a.txt:l1|X").0);
    assert!(!world_op(&mut s, "edit:W1").0);
}

#[test]
fn world_fork_is_repo_size_independent_o1() {
    // fszero-ap9 micro-gate: fork does no tree scan, no file reads, no store
    // writes — 100 forks must be far under the 10ms-per-fork budget even in
    // a debug build.
    let root = TestRoot::new("world_fork_perf");
    root.write("src/a.txt", "x\n");
    let mut s = FSZeroSession::with_root(&root);
    let t0 = std::time::Instant::now();
    for _ in 0..100 {
        let (ok, detail) = world_op(&mut s, "fork");
        assert!(ok, "{detail}");
    }
    let per_fork = t0.elapsed() / 100;
    assert!(
        per_fork < std::time::Duration::from_millis(10),
        "fork p50 {per_fork:?} >= 10ms"
    );
}

#[test]
fn world_conflicts_overlap_adjacent_disjoint() {
    // fszero-4wp: cross-world hunk overlap detected at edit time.
    let root = TestRoot::new("world_conflicts");
    root.write("src/a.txt", "l1\nl2\nl3\nl4\nl5\n");
    let mut s = FSZeroSession::with_root(&root);
    assert!(world_op(&mut s, "fork").0); // W1
    let (ok, detail) = world_op(&mut s, "edit:W1:src/a.txt:l2|L2");
    assert!(ok, "{detail}");
    assert!(!detail.contains("conflicts="), "{detail}");

    // Overlapping hunk in another world → flagged on the staging ack.
    assert!(world_op(&mut s, "fork").0); // W2
    let (ok, detail) = world_op(&mut s, "edit:W2:src/a.txt:l2\nl3|Z2\nZ3");
    assert!(ok, "{detail}");
    assert!(
        detail.contains("conflicts=W1:src/a.txt:2-3&2-2"),
        "{detail}"
    );

    // Live query surface (before W3 exists: exactly the W1 overlap).
    let (ok, detail) = world_op(&mut s, "conflicts:W2");
    assert!(ok, "{detail}");
    assert!(detail.contains("n=1"), "{detail}");
    let payload = expand_text(&s, "world_W2/conflicts");
    let report: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(report["conflicts"][0]["with"], "W1");
    assert_eq!(report["conflicts"][0]["ours"][0], 2);
    assert_eq!(report["conflicts"][0]["theirs"][1], 2);

    // Adjacent (touching, non-overlapping) is NOT a conflict.
    assert!(world_op(&mut s, "fork").0); // W3
    let (ok, detail) = world_op(&mut s, "edit:W3:src/a.txt:l3|L3");
    assert!(ok, "{detail}");
    assert!(
        !detail.contains("conflicts=W1"),
        "adjacent flagged as conflict: {detail}"
    );

    // Disjoint file region: no conflict.
    assert!(world_op(&mut s, "fork").0); // W4
    let (ok, detail) = world_op(&mut s, "edit:W4:src/a.txt:l5|L5");
    assert!(ok, "{detail}");
    assert!(!detail.contains("conflicts=W1"), "{detail}");

    // W3 (adjacent to W1, overlapping W2's 2-3 span) now raises W2's count.
    let (ok, detail) = world_op(&mut s, "conflicts:W2");
    assert!(ok && detail.contains("n=2"), "{detail}");
    let (ok, detail) = world_op(&mut s, "conflicts:W4");
    assert!(ok && detail.contains("n=0"), "{detail}");
}

#[test]
fn world_commit_three_way_merge_disjoint_hunks() {
    // fszero-glg: two worlds edit disjoint hunks of the same file; both
    // commit cleanly, second one auto-merges over the first's result.
    let root = TestRoot::new("world_merge");
    root.write("src/a.txt", "l1\nl2\nl3\nl4\nl5\n");
    let mut s = FSZeroSession::with_root(&root);
    assert!(world_op(&mut s, "new:src/a.txt:l2|L2").0); // W1
    assert!(world_op(&mut s, "new:src/a.txt:l5|L5").0); // W2
    assert!(world_op(&mut s, "commit:W1").0);
    let (ok, detail) = world_op(&mut s, "commit:W2");
    assert!(ok, "{detail}");
    assert_eq!(
        fs::read_to_string(root.join("src/a.txt")).unwrap(),
        "l1\nL2\nl3\nl4\nL5\n"
    );
}

#[test]
fn world_commit_conflict_structured_report_never_clobbers() {
    // fszero-glg: overlapping edits — second commit yields a structured
    // conflict report, writes nothing, and leaves the world active.
    let root = TestRoot::new("world_conflict_report");
    root.write("src/a.txt", "l1\nl2\nl3\n");
    let mut s = FSZeroSession::with_root(&root);
    assert!(world_op(&mut s, "new:src/a.txt:l2|X2").0); // W1
    assert!(world_op(&mut s, "new:src/a.txt:l2|Y2").0); // W2
    assert!(world_op(&mut s, "commit:W1").0);
    let (ok, detail) = world_op(&mut s, "commit:W2");
    assert!(!ok, "conflicting commit must fail: {detail}");
    assert!(detail.contains("merge conflict files=1"), "{detail}");
    assert!(detail.contains("ref="), "{detail}");
    // Nothing clobbered: W1's result intact.
    assert_eq!(
        fs::read_to_string(root.join("src/a.txt")).unwrap(),
        "l1\nX2\nl3\n"
    );
    // Structured report persisted with three-way refs.
    let payload = expand_text(&s, "world_W2/conflict");
    let report: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(report["world"], "W2");
    assert_eq!(report["conflicts"][0]["file"], "src/a.txt");
    assert_eq!(report["conflicts"][0]["reason"], "no match");
    assert!(
        report["conflicts"][0]["base_ref"]
            .as_str()
            .unwrap()
            .starts_with("fz://")
    );
    assert!(
        report["conflicts"][0]["ours_ref"]
            .as_str()
            .unwrap()
            .starts_with("fz://")
    );
    assert!(
        report["conflicts"][0]["theirs_ref"]
            .as_str()
            .unwrap()
            .starts_with("fz://")
    );
    // World stays active for resolution (drop succeeds exactly once).
    assert!(world_op(&mut s, "drop:W2").0);
    assert!(!world_op(&mut s, "drop:W2").0);
}

#[test]
fn world_commit_delete_vs_edit_conflict() {
    // fszero-glg + V6-F4 (ZS-STORE-005): file deleted under a staged world --
    // loud structured external-effect refusal with a durable receipt, no
    // resurrection, world stays active. The rescan gate classifies the
    // external delete as an undeclared mutation (never a silent absorb).
    let root = TestRoot::new("world_delete_edit");
    root.write("src/a.txt", "l1\nl2\n");
    root.write("src/b.txt", "k1\n");
    let mut s = FSZeroSession::with_root(&root);
    assert!(world_op(&mut s, "newbatch:src/a.txt:l2|L2;;src/b.txt:k1|K1").0);
    fs::remove_file(root.join("src/a.txt")).unwrap();
    let (ok, detail) = world_op(&mut s, "commit:W1");
    assert!(!ok, "{detail}");
    assert!(
        detail.contains("external edit on committed path(s)"),
        "{detail}"
    );
    // Atomic: the still-present file was not written either.
    assert_eq!(fs::read_to_string(root.join("src/b.txt")).unwrap(), "k1\n");
    assert!(!root.join("src/a.txt").exists());
    // The receipt is bound into the world's durable record.
    let payload = expand_text(&s, "world_W1/external_effects");
    assert!(payload.contains("src/a.txt"), "{payload}");
}

#[test]
fn world_commit_unchanged_base_byte_identical_to_direct_edit() {
    // fszero-glg property: committing a world forked from an unchanged base
    // is byte-identical to applying the same unique replace directly.
    let root = TestRoot::new("world_identical");
    let content = "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n";
    root.write("src/m.rs", content);
    root.write("src/direct.rs", content);
    let mut s = FSZeroSession::with_root(&root);
    assert!(world_op(&mut s, "new:src/m.rs:let x = 1;|let x = 2;").0);
    assert!(world_op(&mut s, "commit:W1").0);
    let direct = content.replacen("let x = 1;", "let x = 2;", 1);
    assert_eq!(fs::read_to_string(root.join("src/m.rs")).unwrap(), direct);
}

#[test]
fn world_overlay_view_zero_disk_writes() {
    // fszero-1wm: view serves the world's would-be file content from the
    // journal overlay; disk is untouched until commit, and commit
    // materializes exactly the viewed bytes.
    let root = TestRoot::new("world_view");
    let base = "l1\nl2\nl3\n";
    root.write("src/a.txt", base);
    root.write("src/b.txt", "k1\nk2\n");
    let mut s = FSZeroSession::with_root(&root);
    assert!(world_op(&mut s, "fork").0);
    assert!(world_op(&mut s, "edit:W1:src/a.txt:l2|L2").0);
    assert!(world_op(&mut s, "edit:W1:src/a.txt:l3|L3").0);
    assert!(world_op(&mut s, "edit:W1:src/b.txt:k1|K1").0);

    let (ok, detail) = world_op(&mut s, "view:W1:src/a.txt");
    assert!(ok, "{detail}");
    let view_a = expand_text(&s, "world_W1/view");
    assert_eq!(view_a, "l1\nL2\nL3\n");
    // Zero disk mutation from viewing.
    assert_eq!(fs::read_to_string(root.join("src/a.txt")).unwrap(), base);

    // Enumeration lists both files with hunks.
    let (ok, detail) = world_op(&mut s, "view:W1");
    assert!(ok && detail.contains("files=2"), "{detail}");
    let listing: Value = serde_json::from_str(&expand_text(&s, "world_W1/view")).unwrap();
    assert_eq!(listing["files"].as_array().unwrap().len(), 2);

    // Overlay == materialization, byte for byte.
    assert!(world_op(&mut s, "commit:W1").0);
    assert_eq!(fs::read_to_string(root.join("src/a.txt")).unwrap(), view_a);
    assert_eq!(
        fs::read_to_string(root.join("src/b.txt")).unwrap(),
        "K1\nk2\n"
    );

    // Unknown world / missing path fail loudly.
    assert!(!world_op(&mut s, "view:W9").0);
    assert!(!world_op(&mut s, "view:W9:src/a.txt").0);
}

#[test]
fn world_ref_enumeration_v1_contract() {
    // fszero-cbt: the versioned overlay-enumeration contract graphzero
    // consumes (docs/design/world-ref.md). Base/post hashes+refs per file,
    // zero materialization, conflict/unreadable statuses.
    let root = TestRoot::new("world_ref");
    let base_a = "l1\nl2\nl3\n";
    root.write("src/a.txt", base_a);
    root.write("src/gone.txt", "g1\n");
    let mut s = FSZeroSession::with_root(&root);
    assert!(world_op(&mut s, "fork").0); // W1
    assert!(world_op(&mut s, "edit:W1:src/a.txt:l2|L2").0);
    assert!(world_op(&mut s, "edit:W1:src/gone.txt:g1|G1").0);
    fs::remove_file(root.join("src/gone.txt")).unwrap();

    let (ok, detail) = world_op(&mut s, "view:W1");
    assert!(ok, "{detail}");
    assert!(detail.contains("v=1 files=2"), "{detail}");
    let payload: Value = serde_json::from_str(&expand_text(&s, "world_W1/view")).unwrap();
    assert_eq!(payload["version"], 1);
    assert_eq!(payload["world_ref"], "fz://world/W1");
    let files = payload["files"].as_array().unwrap();
    let a = files.iter().find(|f| f["file"] == "src/a.txt").unwrap();
    assert_eq!(a["status"], "clean");
    assert_eq!(a["hunks"][0][0], 2);
    // post_ref resolves to the exact would-be bytes; base untouched on disk.
    let post_ref = a["post_ref"].as_str().unwrap();
    assert!(post_ref.starts_with("fz://blob/"), "{post_ref}");
    let post_bytes = s.expand(post_ref).expect("post blob resolvable");
    assert_eq!(String::from_utf8(post_bytes).unwrap(), "l1\nL2\nl3\n");
    assert_eq!(fs::read_to_string(root.join("src/a.txt")).unwrap(), base_a);
    assert_eq!(
        a["post_ref"].as_str().unwrap(),
        format!("fz://blob/{}", a["post_hash"].as_str().unwrap())
    );
    let gone = files.iter().find(|f| f["file"] == "src/gone.txt").unwrap();
    assert_eq!(gone["status"], "unreadable");
}

#[test]
fn world_resolve_mine_theirs_merged_abort() {
    // fszero-e8s: resolution API over a real content-overlap conflict.
    let root = TestRoot::new("world_resolve");
    root.write("src/a.txt", "l1\nl2\nl3\n");
    root.write("src/b.txt", "k1\nk2\n");
    let mut s = FSZeroSession::with_root(&root);

    // W1 wins the race; W2/W3/W4 conflict on the same hunk.
    assert!(world_op(&mut s, "new:src/a.txt:l2|X2").0); // W1
    assert!(world_op(&mut s, "new:src/a.txt:l2|Y2").0); // W2
    assert!(world_op(&mut s, "new:src/a.txt:l2|Z2").0); // W3
    assert!(world_op(&mut s, "newbatch:src/a.txt:l2|Q2;;src/b.txt:k2|K2").0); // W4
    assert!(world_op(&mut s, "commit:W1").0);

    // accept-mine: W2's content wins wholesale on the next commit.
    assert!(!world_op(&mut s, "commit:W2").0);
    let (ok, detail) = world_op(&mut s, "resolve:W2:src/a.txt:mine");
    assert!(ok, "{detail}");
    assert!(world_op(&mut s, "commit:W2").0);
    assert_eq!(
        fs::read_to_string(root.join("src/a.txt")).unwrap(),
        "l1\nY2\nl3\n"
    );

    // accept-theirs: W3 withdraws its edit; commit becomes a no-op.
    assert!(!world_op(&mut s, "commit:W3").0);
    let (ok, detail) = world_op(&mut s, "resolve:W3:src/a.txt:theirs");
    assert!(ok, "{detail}");
    assert!(world_op(&mut s, "commit:W3").0);
    assert_eq!(
        fs::read_to_string(root.join("src/a.txt")).unwrap(),
        "l1\nY2\nl3\n"
    );

    // supply-merged: agent hands the merged bytes verbatim (pipes, colons —
    // no grammar); the non-conflicting file in the batch still lands.
    assert!(!world_op(&mut s, "commit:W4").0);
    let (ok, detail) = world_op(&mut s, "resolve:W4:src/a.txt:merged:l1\nY2|Q2: both\nl3\n");
    assert!(ok, "{detail}");
    assert!(world_op(&mut s, "commit:W4").0);
    assert_eq!(
        fs::read_to_string(root.join("src/a.txt")).unwrap(),
        "l1\nY2|Q2: both\nl3\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("src/b.txt")).unwrap(),
        "k1\nK2\n"
    );

    // abort: drops the world exactly like drop:.
    assert!(world_op(&mut s, "new:src/a.txt:l3|W5EDIT").0); // W5
    let (ok, detail) = world_op(&mut s, "resolve:W5:abort");
    assert!(ok, "{detail}");
    assert!(!world_op(&mut s, "commit:W5").0);

    // Guardrails: unknown world / unstaged file / bad mode.
    assert!(!world_op(&mut s, "resolve:W99:src/a.txt:mine").0);
    assert!(world_op(&mut s, "fork").0); // W6
    assert!(!world_op(&mut s, "resolve:W6:src/a.txt:mine").0);
    assert!(!world_op(&mut s, "resolve:W6:src/a.txt:bogus").0);
}

#[test]
fn world_resolve_delete_vs_edit_recreates_on_mine() {
    // fszero-e8s: delete-vs-edit taxonomy — theirs keeps the file deleted,
    // mine recreates it with the world's intended content.
    let root = TestRoot::new("world_resolve_delete");
    root.write("gone.txt", "g1\ng2\n");
    root.write("kept.txt", "h1\nh2\n");
    let mut s = FSZeroSession::with_root(&root);
    assert!(world_op(&mut s, "new:gone.txt:g2|G2").0); // W1
    assert!(world_op(&mut s, "new:kept.txt:h2|H2").0); // W2
    fs::remove_file(root.join("gone.txt")).unwrap();
    fs::remove_file(root.join("kept.txt")).unwrap();

    // theirs: deletion wins; commit is an empty no-op.
    assert!(!world_op(&mut s, "commit:W2").0);
    assert!(world_op(&mut s, "resolve:W2:kept.txt:theirs").0);
    assert!(world_op(&mut s, "commit:W2").0);
    assert!(!root.join("kept.txt").exists());

    // mine: the world's would-be content is recreated.
    assert!(!world_op(&mut s, "commit:W1").0);
    let (ok, detail) = world_op(&mut s, "resolve:W1:gone.txt:mine");
    assert!(ok, "{detail}");
    let (ok, detail) = world_op(&mut s, "commit:W1");
    assert!(ok, "{detail}");
    assert_eq!(
        fs::read_to_string(root.join("gone.txt")).unwrap(),
        "g1\nG2\n"
    );
}

#[cfg(unix)]
#[test]
fn world_commit_mode_change_vs_edit_no_conflict() {
    // fszero-e8s taxonomy: a base mode change with unchanged content is NOT
    // a content conflict — the commit lands and the changed mode survives.
    use std::os::unix::fs::PermissionsExt;
    let root = TestRoot::new("world_mode_edit");
    root.write("run.sh", "#!/bin/sh\necho hi\n");
    let mut s = FSZeroSession::with_root(&root);
    assert!(world_op(&mut s, "new:run.sh:echo hi|echo hello").0);
    let p = root.join("run.sh");
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    let (ok, detail) = world_op(&mut s, "commit:W1");
    assert!(ok, "{detail}");
    assert_eq!(fs::read_to_string(&p).unwrap(), "#!/bin/sh\necho hello\n");
    assert_eq!(
        fs::metadata(&p).unwrap().permissions().mode() & 0o777,
        0o755,
        "mode change under the world must survive the commit"
    );
}

#[test]
fn world_preview_full_tree_pre_commit() {
    // fszero-otm: complete would-be tree with metadata; changed files carry
    // byte-exact post hash/ref; zero disk mutation.
    let root = TestRoot::new("world_preview");
    root.write("src/a.rs", "fn a() { let x = 1; }\n");
    root.write("src/b.rs", "fn b() {}\n");
    let mut s = FSZeroSession::with_root(&root);
    assert!(world_op(&mut s, "fork").0);
    assert!(world_op(&mut s, "edit:W1:src/a.rs:let x = 1;|let x = 9;").0);

    let (ok, detail) = world_op(&mut s, "preview:W1");
    assert!(ok, "{detail}");
    assert!(detail.contains("changed=1"), "{detail}");
    let payload: Value = serde_json::from_str(&expand_text(&s, "world_W1/preview")).unwrap();
    assert_eq!(payload["version"], 1);
    assert_eq!(payload["world_ref"], "fz://world/W1");
    let files = payload["files"].as_array().unwrap();
    assert!(files.len() >= 2, "full tree, not just the diff: {files:?}");
    let a = files.iter().find(|f| f["file"] == "src/a.rs").unwrap();
    assert_eq!(a["status"], "changed");
    let post = s.expand(a["post_ref"].as_str().unwrap()).unwrap();
    assert_eq!(String::from_utf8(post).unwrap(), "fn a() { let x = 9; }\n");
    let b = files.iter().find(|f| f["file"] == "src/b.rs").unwrap();
    assert!(
        b.get("status").is_none(),
        "unchanged file has no status: {b}"
    );
    assert!(b["size"].as_u64().unwrap() > 0);
    // Disk untouched.
    assert_eq!(
        fs::read_to_string(root.join("src/a.rs")).unwrap(),
        "fn a() { let x = 1; }\n"
    );
    assert!(!world_op(&mut s, "preview:W9").0);
}

#[test]
fn edit_object_form_expresses_pipes_and_colons() {
    // fszero-edit-spec-pipe-escape-beh: the `path:old|new` spec grammar
    // splits on the first pipe, so markdown tables and shell pipelines were
    // uneditable. The {path, find, replace} object form has no grammar.
    let root = TestRoot::new("edit_pipes");
    root.write("doc.md", "| col_a | col_b |\ncmd: foo | bar\n");
    let mut s = FSZeroSession::with_root(&root);
    let mut conn = FsConnector::new(&mut s);
    let step = conn.invoke(
        "fs.edit",
        &serde_json::json!({
            "path": "doc.md",
            "find": "| col_a | col_b |",
            "replace": "| col_x | col_y |",
        }),
    );
    assert!(step.ok, "{:?}", step.detail);
    let step = conn.invoke(
        "fs.edit",
        &serde_json::json!({
            "path": "doc.md",
            "find": "cmd: foo | bar",
            "replace": "cmd: baz | qux | quux",
        }),
    );
    assert!(step.ok, "{:?}", step.detail);
    assert_eq!(
        fs::read_to_string(root.join("doc.md")).unwrap(),
        "| col_x | col_y |\ncmd: baz | qux | quux\n"
    );
    // Spec form keeps working; object form with missing keys fails loudly.
    let step = conn.invoke("fs.edit", &serde_json::json!({"path": "doc.md"}));
    assert!(!step.ok);
}

#[test]
fn surfaces_disjoint_and_codemode_token_savings() {
    let mcp_tools = surfaces::mcp_tools();
    let cm_tools = surfaces::codemode_tools();
    let mcp = surfaces::tool_names(&mcp_tools);
    let cm = surfaces::tool_names(&cm_tools);
    assert!(mcp.contains(&"fszero.read"));
    assert!(!mcp.iter().any(|n| n.contains("codemode")));
    assert!(cm.contains(&"fz_execute_code"));
    assert!(!cm.contains(&"fszero.read"));

    let root = codemode_tree();
    let mut mcp_sess = FSZeroSession::with_repo_store(&root);
    let mut mcp_tokens = 0usize;
    for (op, arg) in [
        ('L', Some("--depth 1")),
        ('S', Some("process_request")),
        ('R', Some("src/main.rs")),
    ] {
        mcp_tokens += estimate_visible_tokens(&mcp_sess.execute(op, arg).0);
    }
    let mut cm_sess = FSZeroSession::with_repo_store(&root);
    let cm_ack = codemode_execute_plan(&mut cm_sess, "explore");
    assert_eq!(cm_ack, "C");
    assert!(mcp_tokens > estimate_visible_tokens(&cm_ack));
}

#[test]
fn expand_result_refs() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);
    codemode_execute_plan(&mut s, r#"{"steps":[{"call":"fs.ls","args":{}}]}"#);
    assert!(!expand_text(&s, RESULT_REF).is_empty());
    assert!(!expand_text(&s, STEPS_REF).is_empty());
    let latest = expand_text(&s, "codemode/execution/latest");
    assert!(latest.starts_with("fz://codemode/execution/"));
    assert!(!expand_text(&s, &format!("{latest}/telemetry")).is_empty());
    assert!(!expand_text(&s, &format!("{latest}/steps")).is_empty());
    assert!(!expand_text(&s, &format!("{latest}/result")).is_empty());
}

#[test]
fn codemode_executes_sandboxed_javascript() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);
    let code = r#"
        export default async function ({ fs, ctx }) {
            const search = await fs.search({ query: 'process_request' });
            const read = await fs.read({ path: 'src/main.rs' });
            return ctx.ref({ searchOk: search.ok, readOk: read.ok, searchMethod: search.method, readMethod: read.method });
        }
    "#;
    let ack = codemode_execute_plan(&mut s, code);
    assert_eq!(ack, "C", "{}", expand_text(&s, "codemode/error"));
    let steps = expand_text(&s, STEPS_REF);
    assert!(steps.contains("method=fs.search"));
    assert!(steps.contains("method=fs.read"));
    let telemetry = expand_text(&s, TELEMETRY_REF);
    let telemetry_json: Value = serde_json::from_str(&telemetry).unwrap();
    assert_eq!(telemetry_json["kind"], "codemode.execute");
    assert_eq!(telemetry_json["internal_actions"], 2);
}

#[test]
fn codemode_write_creates_new_files_and_stays_root_guarded() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);
    // Create a file in a directory that does not exist yet — the case the
    // old read-then-edit write emulation could never handle.
    let plan = r#"
        export default async function ({ fs }) {
            const w = await fs.write({ path: 'docs/new/advisory.md', content: '# hello\nbody line\n' });
            const back = await fs.read({ path: 'docs/new/advisory.md' });
            return { ack: w.ack, ok: w.ok, saw: back.payload };
        }
    "#;
    assert_eq!(codemode_execute_plan(&mut s, plan), "C");
    let result = expand_text(&s, RESULT_REF);
    assert!(result.contains("# hello"), "{result}");
    // Overwrite the same file (exists now).
    let plan2 = r#"
        export default async function ({ fs }) {
            const w = await fs.write({ path: 'docs/new/advisory.md', content: 'v2' });
            const back = await fs.read({ path: 'docs/new/advisory.md' });
            return { payload: back.payload };
        }
    "#;
    assert_eq!(codemode_execute_plan(&mut s, plan2), "C");
    let result = expand_text(&s, RESULT_REF);
    assert!(result.contains("v2"), "{result}");
    // Escaping the root must fail the plan.
    let plan3 = r#"
        export default async function ({ fs }) {
            const w = await fs.write({ path: '../outside.txt', content: 'nope' });
            if (!w.ok) throw new Error('write escaped-root rejected: ' + (w.detail || w.ack));
            return { ok: w.ok };
        }
    "#;
    assert_eq!(codemode_execute_plan(&mut s, plan3), "X0");
}

/// Full-file write Acceptance (fszero-full-file-write-api-nld): multi-line /
/// multi-KB create-or-overwrite round-trips byte-equal via CodeMode without
/// find/replace; written content is expandable via write-post + post content ref.
#[test]
fn codemode_full_file_write_round_trip_and_expand_ref() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);

    // Representative sizes: small, multi-line (~200 lines), multi-KB (~1200 lines).
    for (label, lines) in [("small", 3usize), ("mid", 200), ("large", 1200)] {
        let body: String = (0..lines)
            .map(|i| format!("line {i:04} -- full-file write fixture {label}\n"))
            .collect();
        assert!(
            body.len() > if lines > 100 { 4_000 } else { 10 },
            "{label} body too small: {}",
            body.len()
        );

        // Kernel path first (no JS sandbox): create/overwrite exact bytes.
        let spec = format!("docs/{label}.txt|{body}");
        let (ack, ok, detail) = s.execute('P', Some(&spec));
        assert!(ok, "{label} kernel write failed: {detail:?}");
        assert_one_token(&ack);
        assert!(ack.starts_with('P'), "{label} ack={ack}");
        let on_disk = fs::read_to_string(root.join(format!("docs/{label}.txt"))).unwrap();
        assert_eq!(on_disk, body, "{label} disk != written body");

        let write_post = expand_text(&s, "write-post");
        assert_eq!(write_post, body, "{label} write-post expand != body");

        // post=fz://blob/… (or hash ref) in detail must expand to exact bytes.
        let detail_s = detail.unwrap_or_default();
        let post_ref = detail_s
            .split_whitespace()
            .find_map(|tok| tok.strip_prefix("post="))
            .expect("write detail must include post=");
        assert!(
            post_ref.starts_with("fz://blob/") || post_ref.starts_with("blob/"),
            "{label} post ref shape: {post_ref}"
        );
        let expanded = s
            .expand(post_ref)
            .unwrap_or_else(|| panic!("{label} expand post_ref failed: {post_ref}"));
        assert_eq!(
            String::from_utf8_lossy(&expanded),
            body,
            "{label} post_ref expand != body"
        );

        // Overwrite with a different body — still no find/replace.
        let body2 = format!("OVERWRITE {label}\n{body}");
        let spec2 = format!("docs/{label}.txt|{body2}");
        let (_, ok2, _) = s.execute('P', Some(&spec2));
        assert!(ok2, "{label} overwrite failed");
        assert_eq!(
            fs::read_to_string(root.join(format!("docs/{label}.txt"))).unwrap(),
            body2
        );
        assert_eq!(expand_text(&s, "write-post"), body2);
    }

    // CodeMode path: multi-KB body through fs.write; large read payloads may
    // wire as refs, so equality is proven via disk + write-post expand.
    let kb_lines = 800usize;
    let kb_body: String = (0..kb_lines)
        .map(|i| format!("codemode-line-{i:04} payload bytes for full-file write\n"))
        .collect();
    assert!(kb_body.len() > 8_000, "multi-KB fixture: {}", kb_body.len());
    // Escape for embedding in a JS single-quoted string (no raw newlines in source).
    let js_content = kb_body
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    let plan = format!(
        r#"
        export default function ({{ fs }}) {{
            const content = '{js_content}';
            const w = fs.write({{ path: 'docs/codemode-large.txt', content }});
            return {{
                ok: w.ok,
                detail: w.detail || '',
                bytes: content.length
            }};
        }}
        "#
    );
    assert_eq!(codemode_execute_plan(&mut s, &plan), "C");
    let result = expand_text(&s, RESULT_REF);
    let result_json: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(result_json["ok"], true, "{result}");
    assert_eq!(
        fs::read_to_string(root.join("docs/codemode-large.txt")).unwrap(),
        kb_body
    );
    assert_eq!(expand_text(&s, "write-post"), kb_body);
}

#[test]
fn codemode_bare_js_fs_expression_uses_sandbox() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);
    assert_eq!(
        codemode_execute_plan(&mut s, "fs.read({ path: 'src/main.rs' })"),
        "C"
    );
    let telemetry = expand_text(&s, TELEMETRY_REF);
    let telemetry_json: Value = serde_json::from_str(&telemetry).unwrap();
    assert_eq!(telemetry_json["kind"], "codemode.execute", "{telemetry}");
    assert_eq!(telemetry_json["physical_ops"], 1, "{telemetry}");
    let result = expand_text(&s, RESULT_REF);
    assert!(result.contains("process_request"), "{result}");
}

#[test]
fn codemode_batches_many_bindings() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);
    let code = r#"
        export default function ({ fs }) {
            return fs.multiRead({ paths: ['src/main.rs', 'src/lib.rs'] });
        }
    "#;
    assert_eq!(codemode_execute_plan(&mut s, code), "C");
    let telemetry = expand_text(&s, TELEMETRY_REF);
    let telemetry_json: Value = serde_json::from_str(&telemetry).unwrap();
    assert_eq!(telemetry_json["logical_ops"], 1, "{telemetry}");
    assert_eq!(telemetry_json["physical_ops"], 1, "{telemetry}");
    assert_eq!(telemetry_json["batched_ops"], 1, "{telemetry}");
    let steps = expand_text(&s, STEPS_REF);
    assert!(steps.contains("method=fs.multiRead"));
    assert_eq!(steps.matches("method=fs.read").count(), 1);
}

#[test]
fn codemode_execution_refs_do_not_collide() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_repo_store(&root);
    assert_eq!(
        codemode_execute_plan(&mut s, r#"{"steps":[{"call":"fs.ls","args":{}}]}"#),
        "C"
    );
    let first = expand_text(&s, "codemode/execution/latest");
    assert_eq!(
        codemode_execute_plan(&mut s, r#"{"steps":[{"call":"fs.ls","args":{}}]}"#),
        "C"
    );
    let second = expand_text(&s, "codemode/execution/latest");
    assert_ne!(first, second);
    assert!(!expand_text(&s, &format!("{first}/telemetry")).is_empty());
    assert!(!expand_text(&s, &format!("{second}/telemetry")).is_empty());
}

#[test]
fn codemode_batch_rows_have_unique_recoverable_refs() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);
    let code = r#"
        export default function ({ fs }) {
            return fs.multiRead({ paths: ['src/main.rs', 'src/lib.rs'] });
        }
    "#;
    assert_eq!(codemode_execute_plan(&mut s, code), "C");
    let raw = expand_text(&s, "codemode/batch");
    let rows: Value = serde_json::from_str(&raw).unwrap();
    let rows = rows.as_array().unwrap();
    let first_ref = rows[0]["ref"].as_str().unwrap();
    let second_ref = rows[1]["ref"].as_str().unwrap();
    assert_ne!(first_ref, second_ref);
    assert!(first_ref.starts_with("codemode/batch/"));
    assert!(expand_text(&s, first_ref).contains("process_request"));
    assert!(expand_text(&s, second_ref).contains("helper"));

    let first_batch_ref = first_ref.split("/0/").next().unwrap().to_string();
    assert_eq!(
        codemode_execute_plan(
            &mut s,
            "export default function ({ fs }) { return fs.multiSearch({ queries: ['helper'] }); }"
        ),
        "C"
    );
    let old_batch = expand_text(&s, &first_batch_ref);
    assert!(old_batch.contains("process_request"));
    assert_ne!(old_batch, expand_text(&s, "codemode/batch"));
}

#[test]
fn codemode_json_batch_plan_uses_native_plan_runtime() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);
    let plan = r#"{"steps":[{"call":"fs.multiRead","args":{"paths":["src/main.rs"]}}]}"#;
    assert_eq!(codemode_execute_plan(&mut s, plan), "C");
    let steps = expand_text(&s, STEPS_REF);
    assert!(steps.contains("method=fs.multiRead"));
    assert!(steps.contains("ref=codemode/batch/"), "{steps}");
    let raw = expand_text(&s, "codemode/batch");
    let rows: Value = serde_json::from_str(&raw).unwrap();
    let row_ref = rows[0]["ref"].as_str().unwrap();
    assert!(expand_text(&s, row_ref).contains("process_request"));
}

#[test]
fn codemode_failed_js_init_does_not_snapshot_stale_success_refs() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);
    assert_eq!(
        codemode_execute_plan(&mut s, "export default function () { return 'ok'; }"),
        "C"
    );
    let prior = expand_text(&s, "codemode/execution/latest");
    assert!(expand_text(&s, &format!("{prior}/result")).contains("ok"));

    let too_large = format!(
        "export default function () {{ return '{}'; }}",
        "x".repeat(70 * 1024)
    );
    assert_eq!(codemode_execute_plan(&mut s, &too_large), "X0");
    let latest = expand_text(&s, "codemode/execution/latest");
    assert_ne!(prior, latest);
    assert!(expand_text(&s, &format!("{latest}/steps")).contains("steps=0 ok=false"));
    let latest_result = expand_text(&s, &format!("{latest}/result"));
    assert!(
        latest_result.contains("plan is 71721 bytes; maximum is 65536"),
        "{latest_result}"
    );
}

#[test]
fn codemode_v2_plan_result_is_one_line_ref_envelope() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);
    let result = SurfaceKind::CodeMode
        .call_tool(
            &mut s,
            "fz_execute_code",
            &serde_json::json!({"plan": "return 1", "envelope": "v2"}),
        )
        .unwrap();
    let ack = result["content"][0]["text"].as_str().unwrap();
    assert!(ack.starts_with("ok fz"), "{ack}");
    assert!(ack.contains(" t:fz://"), "{ack}");
    assert!(estimate_visible_tokens(ack) <= 24, "{ack}");
    let structured = result["structuredContent"].as_object().unwrap();
    for key in structured.keys() {
        assert!(
            matches!(
                key.as_str(),
                "ack" | "value" | "ref" | "refs" | "owner_refs" | "telemetry"
            ),
            "{key}"
        );
    }
    assert_eq!(structured["ack"], ack);
    assert!(structured["ref"].as_str().unwrap().starts_with("fz://"));
}

#[test]
fn codemode_v2_big_read_payload_is_ref_first_and_exactly_expandable() {
    let root = TestRoot::new("big_read");
    let big = (0..120)
        .map(|i| format!("token{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    root.write("big.txt", format!("{big}\nsecond line\n"));
    let mut s = FSZeroSession::with_root(&root);
    let result = SurfaceKind::CodeMode
        .call_tool(
            &mut s,
            "fz_execute_code",
            &serde_json::json!({"plan": "fs.read({ path: 'big.txt' })", "envelope": "v2"}),
        )
        .unwrap();
    let inner = &result["structuredContent"]["value"]["result"]["payload"];
    let expected = format!("{big}\nsecond line\n");
    assert_eq!(
        inner["result"]
            .as_str()
            .unwrap_or_else(|| panic!("inner={inner}")),
        expected.as_str()
    );
    let payload_ref = inner["ref"]
        .as_str()
        .unwrap_or_else(|| panic!("inner={inner}"));
    assert!(payload_ref.starts_with("fz://blob/"), "{payload_ref}");
    assert_eq!(expand_text(&s, payload_ref), expected);
}

#[test]
fn codemode_plan_desugar_preserves_content_and_runs_async_wrapper() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);
    let probe = concat!("async ", "function probe() {}");
    let quoted_probe = serde_json::to_string(probe).unwrap();
    let content_plan = format!(
        "return zero.fs.compound('write', {{ path: 'probe.js', content: {quoted_probe} }})"
    );
    let result = SurfaceKind::CodeMode
        .call_tool(
            &mut s,
            "fz_execute_code",
            &serde_json::json!({"plan": content_plan, "envelope": "v2"}),
        )
        .unwrap();
    assert!(!result["isError"].as_bool().unwrap(), "{result:?}");
    assert_eq!(fs::read(root.join("probe.js")).unwrap(), probe.as_bytes());

    let wrapper_plan = format!(
        "export default {} run() {{ return zero.fs.compound('write', {{ path: 'wrapper.txt', content: 'ok' }}) }}",
        concat!("async ", "function")
    );
    let result = SurfaceKind::CodeMode
        .call_tool(
            &mut s,
            "fz_execute_code",
            &serde_json::json!({"plan": wrapper_plan, "envelope": "v2"}),
        )
        .unwrap();
    assert!(!result["isError"].as_bool().unwrap(), "{result:?}");
    assert_eq!(fs::read(root.join("wrapper.txt")).unwrap(), b"ok");
}

#[test]
fn codemode_compound_read_returns_requested_line_window() {
    let root = codemode_tree();
    let fixture = "alpha\nbeta\ngamma\ndelta\n";
    root.write("window.txt", fixture);
    let mut s = FSZeroSession::with_root(&root);
    let result = SurfaceKind::CodeMode
        .call_tool(
            &mut s,
            "fz_execute_code",
            &serde_json::json!({
                "plan": "return zero.fs.compound('read', { path: 'window.txt', start_line: 2, end_line: 3 })",
                "envelope": "v2"
            }),
        )
        .unwrap();
    assert!(!result["isError"].as_bool().unwrap(), "{result:?}");
    let inner = &result["structuredContent"]["value"]["result"]["payload"];
    assert_eq!(
        inner["payload"]
            .as_str()
            .unwrap_or_else(|| panic!("inner={inner}")),
        "beta\ngamma\n"
    );
}

#[test]
fn compound_read_resolves_nfd_filename_from_nfc_request() {
    let root = codemode_tree();
    root.write("caf\u{65}\u{301}.txt", "normalized\n");
    let mut session = FSZeroSession::with_root(&root);
    let step = FsConnector::new(&mut session).invoke(
        "fs.compound",
        &serde_json::json!({"name": "read", "path": "caf\u{e9}.txt"}),
    );
    assert!(step.ok, "{:?}", step.detail);
    assert_eq!(step.payload, b"normalized\n");
}

#[test]
fn compound_read_resolves_narrow_no_break_space_from_ascii_space() {
    let root = codemode_tree();
    root.write("Screenshot 2.17.54\u{202f}PM.jpg", "image-bytes");
    let mut session = FSZeroSession::with_root(&root);
    let step = FsConnector::new(&mut session).invoke(
        "fs.compound",
        &serde_json::json!({"name": "read", "path": "Screenshot 2.17.54 PM.jpg"}),
    );
    assert!(step.ok, "{:?}", step.detail);
    assert_eq!(step.payload, b"image-bytes");
}

#[test]
fn compound_read_rejects_ambiguous_unicode_matches_with_candidates() {
    let root = codemode_tree();
    root.write("shot PM.jpg", "ascii");
    root.write("shot\u{202f}PM.jpg", "narrow");
    let mut session = FSZeroSession::with_root(&root);
    let step = FsConnector::new(&mut session).invoke(
        "fs.compound",
        &serde_json::json!({"name": "read", "path": "shot\u{a0}PM.jpg"}),
    );
    let detail = step.detail.unwrap_or_default();
    assert!(!step.ok, "ambiguous read unexpectedly succeeded");
    assert!(detail.contains("ambiguous unicode path"), "{detail}");
    assert!(detail.contains("shot PM.jpg"), "{detail}");
    assert!(detail.contains("shot\u{202f}PM.jpg"), "{detail}");
}

#[test]
fn compound_list_honors_glob_pattern() {
    let root = codemode_tree();
    root.write("shots/Screenshot-one.jpg", "one");
    root.write("shots/Screenshot-two.jpg", "two");
    root.write("shots/notes.txt", "notes");
    let mut session = FSZeroSession::with_root(&root);
    let step = FsConnector::new(&mut session).invoke(
        "fs.compound",
        &serde_json::json!({"name": "list", "path": "shots", "pattern": "Screenshot*"}),
    );
    let manifest = String::from_utf8(step.payload).unwrap();
    assert!(step.ok, "{:?}", step.detail);
    assert!(manifest.contains("Screenshot-one.jpg"), "{manifest}");
    assert!(manifest.contains("Screenshot-two.jpg"), "{manifest}");
    assert!(!manifest.contains("notes.txt"), "{manifest}");
}

#[test]
fn codemode_write_invalidates_warmed_compound_read() {
    let root = codemode_tree();
    root.write("fresh.txt", "before\n");
    let mut session = FSZeroSession::with_root(&root);
    let result = SurfaceKind::CodeMode
        .call_tool(
            &mut session,
            "fz_execute_code",
            &serde_json::json!({
                "plan": r#"{"steps":[{"call":"fs.compound","args":{"name":"read","path":"fresh.txt"}},{"call":"fs.write","args":{"path":"fresh.txt","content":"after\n"}},{"call":"fs.compound","args":{"name":"read","path":"fresh.txt"}}]}"#,
                "envelope": "v2"
            }),
        )
        .unwrap();
    assert!(!result["isError"].as_bool().unwrap(), "{result:?}");
    let steps_ref = result["structuredContent"]["value"]["refs"]["steps"]
        .as_str()
        .expect("steps ref");
    let steps = expand_text(&session, steps_ref);
    let read_ref = steps
        .lines()
        .last()
        .and_then(|line| {
            line.split_whitespace()
                .find_map(|part| part.strip_prefix("ref="))
        })
        .expect("last step ref");
    assert_eq!(expand_text(&session, read_ref), "after\n");
}

#[test]
fn codemode_v2_directory_read_names_inventory_fix() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);
    let result = SurfaceKind::CodeMode
        .call_tool(
            &mut s,
            "fz_execute_code",
            &serde_json::json!({
                "plan": "return zero.fs.compound('read', { path: '.' })",
                "envelope": "v2"
            }),
        )
        .unwrap();
    let ack = result["content"][0]["text"].as_str().unwrap();
    assert!(ack.starts_with("err runtime final "), "{ack}");
    assert!(
        ack.contains("path is a directory; use zero.fs.compound('inventory',{path})"),
        "{ack}"
    );
    assert!(result["isError"].as_bool().unwrap());
}

#[test]
fn codemode_v2_missing_read_path_lists_nearest_names() {
    let root = codemode_tree();
    root.write("src/misc.rs", "pub fn misc() {}\n");
    let mut s = FSZeroSession::with_root(&root);
    let result = SurfaceKind::CodeMode
        .call_tool(
            &mut s,
            "fz_execute_code",
            &serde_json::json!({
                "plan": "return zero.fs.compound('read', { path: 'src/mai.rs' })",
                "envelope": "v2"
            }),
        )
        .unwrap();
    let ack = result["content"][0]["text"].as_str().unwrap();
    assert!(
        ack.starts_with("err runtime final path not found: src/mai.rs"),
        "{ack}"
    );
    assert!(ack.contains("nearest:"), "{ack}");
    assert!(ack.contains("src/main.rs"), "{ack}");
    assert!(result["isError"].as_bool().unwrap());
}

#[test]
fn codemode_v2_unknown_compound_lists_candidates() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);
    let result = SurfaceKind::CodeMode
        .call_tool(
            &mut s,
            "fz_execute_code",
            &serde_json::json!({
                "plan": "return zero.fs.compound('reed', { path: 'src/main.rs' })",
                "envelope": "v2"
            }),
        )
        .unwrap();
    let ack = result["content"][0]["text"].as_str().unwrap();
    assert!(
        ack.starts_with("err runtime final unknown compound 'reed'"),
        "{ack}"
    );
    assert!(ack.contains("closest valid names:"), "{ack}");
    assert!(ack.contains("read"), "{ack}");
    assert!(ack.contains("zero_describe('read')"), "{ack}");
    assert!(result["isError"].as_bool().unwrap());
}

#[test]
fn codemode_v1_escape_hatch_preserves_legacy_wire_shape() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);
    let result = SurfaceKind::CodeMode
        .call_tool(
            &mut s,
            "fz_execute_code",
            &serde_json::json!({"plan": "return 1", "envelope": "v1"}),
        )
        .unwrap();
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with('{'), "{text}");
    assert_eq!(result["structuredContent"].to_string(), text);
    assert!(
        result["structuredContent"]["execution_id"]
            .as_str()
            .is_some()
    );
    assert!(result["structuredContent"]["telemetry"].is_object());
}

#[test]
fn codemode_read_cargo_toml_visible_counts() {
    let root = std::env::current_dir().unwrap();
    let args = serde_json::json!({
        "plan": "fs.read({ path: 'Cargo.toml' })",
        "envelope": "v1"
    });
    let args = serde_json::json!({
        "plan": "fs.read({ path: 'Cargo.toml' })",
        "envelope": "v2"
    });
    let mut before_s = FSZeroSession::with_root(&root);
    let before = SurfaceKind::CodeMode
        .call_tool(&mut before_s, "fz_execute_code", &args)
        .unwrap();
    let before_text = before["content"][0]["text"].as_str().unwrap();
    let before_tokens = estimate_visible_tokens(before_text);

    let mut after_s = FSZeroSession::with_root(&root);
    let after = SurfaceKind::CodeMode
        .call_tool(&mut after_s, "fz_execute_code", &args)
        .unwrap();
    let after_text = after["content"][0]["text"].as_str().unwrap();
    let after_tokens = estimate_visible_tokens(after_text);

    eprintln!("read-Cargo.toml visible tokens: v1={before_tokens} v2={after_tokens}");
    assert!(
        before_tokens > after_tokens,
        "v1={before_tokens} v2={after_tokens}"
    );
    assert!(after_tokens <= 24, "{after_text}");
}

#[test]
fn codemode_schema_matches_tool_result_shape() {
    let tools = surfaces::codemode_tools();
    let tool = tools
        .iter()
        .find(|t| t["name"] == "fz_execute_code")
        .expect("codemode tool");
    let required = tool["outputSchema"]["required"].as_array().unwrap();
    assert!(required.iter().any(|v| v == "ack"));
    assert!(!required.iter().any(|v| v == "execution_id"));
    assert!(!required.iter().any(|v| v == "content"));
    assert!(!required.iter().any(|v| v == "structuredContent"));
    assert!(tool["description"].as_str().unwrap().contains("CodeMode"));
}

#[test]
fn codemode_batch_calls_enforce_logical_limit() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);
    // Physical ceiling (256) is tighter than logical (1000); 257 reads hit it first.
    let code = "export default function ({ fs }) { for (let i = 0; i < 257; i++) { fs.read({ path: 'src/main.rs' }); } return true; }";
    assert_eq!(codemode_execute_plan(&mut s, code), "X0");
    let err = expand_text(&s, "codemode/error");
    assert!(err.contains("max physical ops exceeded"), "{err}");
}

#[test]
fn codemode_sandbox_allows_denied_words_inside_string_literals() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);
    let plan = r#"
        export default function ({ fs }) {
            const text = 'this content says fetch subprocess eval but is just bytes';
            const w = fs.write({ path: 'notes.txt', content: text });
            const r = fs.read({ path: 'notes.txt' });
            return { ok: w.ok, payload: r.payload };
        }
    "#;
    assert_eq!(codemode_execute_plan(&mut s, plan), "C");
    let result = expand_text(&s, RESULT_REF);
    assert!(result.contains("fetch subprocess eval"), "{result}");
}

/// fszero-sandbox-string-literal-tokens-7nl + ah3: fences, nested/escaped
/// backticks, require/import samples, and Promise prose inside string/template
/// bodies must not deny; real fetch( still denied.
#[test]
fn codemode_sandbox_allows_fences_backticks_and_fetched_prose() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);

    // (a) "fetched from API" prose
    let plan_fetched = r#"
        export default function ({ fs }) {
            const text = "fetched from API";
            const w = fs.write({ path: 'fetched.txt', content: text });
            return { ok: w.ok, payload: text };
        }
    "#;
    assert_eq!(codemode_execute_plan(&mut s, plan_fetched), "C");
    assert!(expand_text(&s, RESULT_REF).contains("fetched from API"));

    // (b) markdown fences + nested backticks + require/import samples in body
    // Build a multi-KB README-style body with fenced code blocks.
    let mut readme = String::from("# Project README\n\nFetched from API notes.\n\n");
    for i in 0..80 {
        readme.push_str(&format!(
            "## Section {i}\n\n```rust\nfn sample_{i}() {{ /* require('fs'); fetch('x'); Promise */ }}\n```\n\n"
        ));
        readme.push_str("Also see `inline code` and nested ``ticks``.\n\n");
    }
    assert!(readme.len() > 4_000, "multi-KB README: {}", readme.len());
    assert!(readme.contains("```rust"), "fixture must contain fences");
    let js_body = readme
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n");
    let plan_readme = format!(
        r#"
        export default function ({{ fs }}) {{
            const body = '{js_body}';
            const w = fs.write({{ path: 'README_FIELD.md', content: body }});
            return {{ ok: w.ok, bytes: body.length, detail: w.detail || '' }};
        }}
        "#
    );
    assert_eq!(
        codemode_execute_plan(&mut s, &plan_readme),
        "C",
        "fenced README write denied: {}",
        expand_text(&s, "codemode/error")
    );
    let result = expand_text(&s, RESULT_REF);
    let v: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["ok"], true, "{result}");
    let disk = fs::read_to_string(root.join("README_FIELD.md")).unwrap();
    assert_eq!(disk, readme);
    assert_eq!(expand_text(&s, "write-post"), readme);
    assert!(disk.contains("```rust"));

    // Hub JS has no String.indexOf; build real fences with join.
    let plan_template = r#"
        export default function ({ fs }) {
            const fence = ['`','`','`'].join('');
            const body = '# Title\n' + fence + 'js\nrequire(\'fs\');\nfetch(\'https://example.com\');\nPromise.resolve(1);\n' + fence + '\n';
            const w = fs.write({ path: 'templ.md', content: body });
            return { ok: w.ok, hasFence: body.split(fence).length > 1 };
        }
    "#;
    assert_eq!(
        codemode_execute_plan(&mut s, plan_template),
        "C",
        "template fence write denied: {}",
        expand_text(&s, "codemode/error")
    );
    let templ_disk = fs::read_to_string(root.join("templ.md")).unwrap();
    assert!(templ_disk.contains("```js"), "{templ_disk}");
    assert!(templ_disk.contains("require('fs')"), "{templ_disk}");
}

#[test]
fn codemode_sandbox_still_denies_actual_fetch_call() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);
    let ack = codemode_execute_plan(
        &mut s,
        "export default function () { return fetch('https://example.com'); }",
    );
    assert_eq!(ack, "X0");
    assert!(expand_text(&s, "codemode/error").contains("network/fetch"));
}

#[test]
fn codemode_sandbox_uses_identifier_boundaries_for_fetch() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);
    let ack = codemode_execute_plan(
        &mut s,
        "export default function () { function refetch_data() { return 7; } return refetch_data(); }",
    );
    assert_eq!(ack, "C");
    assert_eq!(expand_text(&s, RESULT_REF), "7");

    let denied = codemode_execute_plan(&mut s, "export default function () { return fetch(); }");
    assert_eq!(denied, "X0");
    assert!(expand_text(&s, "codemode/error").contains("network/fetch"));
}

#[test]
fn codemode_javascript_denies_raw_host_access() {
    let root = codemode_tree();
    let mut s = FSZeroSession::with_root(&root);
    let ack = codemode_execute_plan(
        &mut s,
        "export default function () { return typeof process; }",
    );
    assert_eq!(ack, "X0");
    let process_err = expand_text(&s, "codemode/error");
    assert!(process_err.contains("process"), "{process_err}");

    let ack = codemode_execute_plan(
        &mut s,
        "export default function () { return fetch('https://example.com'); }",
    );
    assert_eq!(ack, "X0");
    let telemetry: Value = serde_json::from_str(&expand_text(&s, TELEMETRY_REF)).unwrap();
    assert_eq!(telemetry["status"], "error");
}

#[test]
fn access_log_records_reads_and_survives_repo_store_restart() {
    let (root, _) = sample_workspace();
    {
        let mut s = FSZeroSession::with_repo_store(&root);
        assert!(s.execute('R', Some("src/main.rs")).1);
        assert!(s.recovery.access_log_row_count() >= 1);
    }
    let reopened = FSZeroSession::with_repo_store(&root);
    assert!(reopened.recovery.access_log_row_count() >= 1);
}

#[test]
fn world_access_hot_recent_coaccess() {
    let root = TestRoot::new("world_access");
    root.write("src/a.rs", "fn a() {}\n");
    root.write("src/b.rs", "fn b() {}\n");
    let mut s = FSZeroSession::with_repo_store(&root);
    assert!(s.execute('R', Some("src/a.rs")).1);
    assert!(s.execute('R', Some("src/b.rs")).1);
    assert!(s.execute('R', Some("src/a.rs")).1);
    let hot = s.recovery.query_access_hot(10);
    assert!(hot.iter().any(|(p, c)| p.contains("a.rs") && *c >= 2));
    let recent = s.recovery.query_access_recent(10);
    assert!(recent.len() >= 2);
    let co = s.recovery.query_access_coaccess(10);
    assert!(
        co.iter()
            .any(|(a, b, _)| a.contains("a.rs") && b.contains("b.rs"))
    );
    let mut conn = FsConnector::new(&mut s);
    let step = conn.invoke("fs.world", &serde_json::json!({"query": "hot", "limit": 5}));
    assert!(step.ok);
    let payload = expand_text(&s, "world/access");
    assert!(payload.contains("hot"));
}

#[test]
fn verified_edit_success_and_verify_rollback() {
    let root = TestRoot::new("vedit");
    root.write("scratch.txt", "MARKER\n");
    let mut s = FSZeroSession::with_root(&root);
    let path = root.join("scratch.txt");
    let pristine = fs::read_to_string(&path).unwrap();
    let mut conn = FsConnector::new(&mut s);
    let ok_step = conn.invoke(
        "fs.compound",
        &serde_json::json!({
            "name": "verifiedEdit",
            "path": "scratch.txt",
            "edits": [{"old": "MARKER", "new": "MARKER_OK"}],
            "verify": "true"
        }),
    );
    assert!(ok_step.ok, "{:?}", ok_step.detail);
    assert!(fs::read_to_string(&path).unwrap().contains("MARKER_OK"));
    let ok_json = expand_text(&s, "verifiedEdit/ok");
    assert!(ok_json.contains("ref_before"));
    assert!(ok_json.contains("ref_after"));
    assert!(!ok_json.contains("MARKER_OK"));
    let mut s2 = FSZeroSession::with_root(&root);
    fs::write(&path, pristine.clone()).unwrap();
    let mut conn2 = FsConnector::new(&mut s2);
    let fail_step = conn2.invoke(
        "fs.compound",
        &serde_json::json!({
            "name": "verifiedEdit",
            "path": "scratch.txt",
            "edits": [{"old": "MARKER", "new": "CHANGED"}],
            "verify": "false"
        }),
    );
    assert!(!fail_step.ok);
    assert_eq!(fs::read_to_string(&path).unwrap(), pristine);
    let err_json = expand_text(&s2, "verifiedEdit/err");
    assert!(err_json.contains("verify_tail"));
    assert!(!err_json.contains("CHANGED"));
}

#[test]
fn verified_edit_visible_to_history_and_undo() {
    // fszero-chg: verifiedEdit writes were journal-invisible — no
    // record_mutation, so fs.history skipped them and fs.undo could not
    // revert them.
    let root = TestRoot::new("vedit_journal");
    root.write("j.txt", "ORIGINAL\n");
    let mut s = FSZeroSession::with_repo_store(&root);
    let mut conn = FsConnector::new(&mut s);
    let step = conn.invoke(
        "fs.compound",
        &serde_json::json!({
            "name": "verifiedEdit",
            "path": "j.txt",
            "edits": [{"old": "ORIGINAL", "new": "EDITED"}],
        }),
    );
    assert!(step.ok, "{:?}", step.detail);
    assert_eq!(fs::read_to_string(root.join("j.txt")).unwrap(), "EDITED\n");

    // Visible in the mutation timeline.
    let (_, ok, detail) = s.execute('H', Some("j.txt"));
    assert!(ok, "{detail:?}");
    let history = detail.unwrap_or_default();
    assert!(history.contains("verifiedEdit"), "{history}");

    // And undoable, byte-exact.
    let (_, ok, detail) = s.execute('U', Some("j.txt"));
    assert!(ok, "{detail:?}");
    assert_eq!(
        fs::read_to_string(root.join("j.txt")).unwrap(),
        "ORIGINAL\n"
    );

    // A verify-failed edit rolls back and journals NOTHING new.
    let (_, _, before) = s.execute('H', Some("j.txt"));
    let fail = conn_history_count(&before.unwrap_or_default());
    let mut conn = FsConnector::new(&mut s);
    let step = conn.invoke(
        "fs.compound",
        &serde_json::json!({
            "name": "verifiedEdit",
            "path": "j.txt",
            "edits": [{"old": "ORIGINAL", "new": "NOPE"}],
            "verify": "false",
        }),
    );
    assert!(!step.ok);
    let (_, _, after) = s.execute('H', Some("j.txt"));
    assert_eq!(fail, conn_history_count(&after.unwrap_or_default()));
}

fn conn_history_count(history_detail: &str) -> usize {
    history_detail
        .lines()
        .filter(|l| l.contains("verifiedEdit") || l.contains("edit") || l.contains("undo"))
        .count()
}

#[test]
fn telemetry_ring_keeps_recent_executions() {
    // fszero-qkn: bounded ring of execution summaries under telemetry/ring.
    // Hot ok path samples every 16th plan (errors always record). Reset the
    // process-global tick so 5 oks produce exactly one sample.
    fs_zero::codemode::reset_ok_ring_tick_for_tests();
    let root = TestRoot::new("telemetry_ring");
    root.write("a.txt", "x\n");
    let mut s = FSZeroSession::with_repo_store(&root);
    for i in 0..5 {
        let plan = format!("return{{n:{i}}};");
        assert_eq!(codemode_execute_plan(&mut s, &plan), "C");
    }
    let ring: Value = serde_json::from_str(&expand_text(&s, "telemetry/ring")).unwrap();
    assert_eq!(ring["version"], 1);
    let entries = ring["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert!(entries.iter().all(|e| e["status"] == "ok"));
    assert!(entries.iter().all(|e| e["wall_ms"].is_number()));
    assert!(entries[0]["ts"].as_u64().unwrap() > 0);
}

/// fszero-ztd-plan-1chm.19 (b): Auto mode must enable the journal for a
/// WRITE-ONLY program. The pre-existing auto test also contains an `fs.edit`
/// step, so it proves nothing about `fs.write` alone reaching
/// `call_is_mutating`.
#[test]
fn codemode_auto_transaction_enables_journal_for_write_only_program() {
    let root = TestRoot::new("txn_write_only_auto");
    root.write("existing.txt", "before");
    let mut session = FSZeroSession::with_root(&root);
    // Every step is fs.write; the last one escapes the root and fails.
    let plan = r#"{"steps":[{"call":"fs.write","args":{"path":"existing.txt","content":"after"}},{"call":"fs.write","args":{"path":"created.txt","content":"new"}},{"call":"fs.write","args":{"path":"../escaped.txt","content":"nope"}}]}"#;
    assert_ne!(codemode_execute_plan(&mut session, plan), "C");
    assert_eq!(
        fs::read_to_string(root.join("existing.txt")).unwrap(),
        "before"
    );
    assert!(!root.join("created.txt").exists());
    let telemetry: Value = serde_json::from_str(&expand_text(&session, TELEMETRY_REF)).unwrap();
    assert_eq!(telemetry["extra"]["transaction_rolled_back"], true);
}

/// fszero-ztd-plan-1chm.19 (a): an explicit `transaction:true` plan that writes
/// then fails restores the preimage and removes the created file.
#[test]
fn codemode_explicit_transaction_rolls_back_writes() {
    let root = TestRoot::new("txn_write_explicit");
    root.write("existing.txt", "before");
    let mut session = FSZeroSession::with_root(&root);
    let plan = r#"{"transaction":true,"steps":[{"call":"fs.write","args":{"path":"existing.txt","content":"after"}},{"call":"fs.write","args":{"path":"nested/created.txt","content":"new"}},{"call":"fs.edit","args":{"spec":"missing.txt:x|y"}}]}"#;
    assert_ne!(codemode_execute_plan(&mut session, plan), "C");
    assert_eq!(
        fs::read_to_string(root.join("existing.txt")).unwrap(),
        "before"
    );
    assert!(!root.join("nested/created.txt").exists());
    assert!(!root.join("nested").exists());
    let telemetry: Value = serde_json::from_str(&expand_text(&session, TELEMETRY_REF)).unwrap();
    assert_eq!(telemetry["extra"]["transaction_rolled_back"], true);
}

/// fszero-rotation-i1-gqgt.25: `fs.compound` with a `mem:` intent reaches
/// `do_memory` and mutates durable memory, but was classified read-only and
/// never journaled, so a later plan failure left the mutation committed.
#[test]
fn codemode_transaction_rolls_back_compound_mem_intent() {
    let root = TestRoot::new("txn_compound_mem_intent");
    let mut session = FSZeroSession::with_root(&root);
    let seed =
        r#"{"steps":[{"call":"fs.memory.put","args":{"path":"note.txt","content":"before"}}]}"#;
    assert_eq!(codemode_execute_plan(&mut session, seed), "C");

    // No explicit `transaction`: Auto mode must recognize the `mem:` intent as a
    // mutation and arm the journal (`compound_intent_mutates`).
    // The only mutation is the `mem:` intent, and the failing step is a plain
    // read: Auto mode must arm the journal from `compound_intent_mutates` alone.
    let plan = r#"{"steps":[{"call":"fs.compound","args":{"intent":"mem:put:note.txt|after"}},{"call":"fs.read","args":{"path":"missing.txt"}}]}"#;
    assert_ne!(codemode_execute_plan(&mut session, plan), "C");
    let read_back = r#"{"steps":[{"call":"fs.memory.get","args":{"path":"note.txt"}}]}"#;
    assert_eq!(codemode_execute_plan(&mut session, read_back), "C");
    assert_eq!(
        expand_text(&session, "memory"),
        "before",
        "compound mem: intent must be journaled and rolled back"
    );
}
