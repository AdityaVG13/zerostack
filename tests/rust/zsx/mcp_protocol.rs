//! Native `zsx mcp` is the harness surface: initialize/list must not open a
//! store, and two executes against one root reuse the in-process session.

use serde_json::json;
use tempfile::TempDir;
use zsx_cli::mcp::{handle, McpHost};

#[test]
fn initialize_and_list_do_not_touch_a_store() {
    let dir = TempDir::new().unwrap();
    let mut host = McpHost::new(dir.path().to_path_buf());
    let init = handle(
        &mut host,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
    )
    .unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "zerostack-zsx");
    assert_eq!(init["result"]["serverInfo"]["lifetime"], "harness-stdio");

    let listed = handle(
        &mut host,
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    )
    .unwrap();
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["zero_execute", "zero_wait"]);
}

#[test]
fn second_execute_reuses_the_same_session() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("hello.txt"), "hi\n").unwrap();
    let mut host = McpHost::new(dir.path().to_path_buf());
    let plan = "return await zero.fs.compound(\"read\", {path: \"hello.txt\"});";

    let first = host
        .zero_execute(plan, None, 30_000)
        .expect("first execute");
    let second = host
        .zero_execute(plan, None, 30_000)
        .expect("second execute");

    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(second["ok"], true, "{second}");
    assert_eq!(first["request_id"], 1, "{first}");
    assert_eq!(
        second["request_id"], 2,
        "second call must reuse the live session: {second}"
    );
}

#[test]
fn zero_execute_fs_write_creates_the_file() {
    let dir = TempDir::new().unwrap();
    let mut host = McpHost::new(dir.path().to_path_buf());
    let plan = r#"return await zero.fs.write({ path: "created.txt", content: "WRITE_OK\n" });"#;
    let result = host
        .zero_execute(plan, None, 30_000)
        .expect("fs.write via MCP must succeed with harness grants");
    assert_eq!(result["ok"], true, "{result}");
    let written = std::fs::read_to_string(dir.path().join("created.txt"))
        .expect("write must land on disk");
    assert_eq!(written, "WRITE_OK\n");
}
