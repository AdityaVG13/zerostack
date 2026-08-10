#![cfg(unix)]
#![forbid(unsafe_code)]

use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
};

use serde_json::{Value, json};
use tempfile::TempDir;

#[test]
#[ignore = "requires three production raw-worker binaries"]
fn production_workers_execute_through_one_aggregate_host() {
    let root = TempDir::new().expect("session root");
    std::fs::write(root.path().join("smoke.txt"), "smoke\n").expect("seed FSZero root");
    let mut command = Command::new(env!("CARGO_BIN_EXE_zerostack-codemode-host"));
    command
        .env("ZEROSTACK_SESSION_ROOT", root.path())
        .env(
            "ZERO_FSZERO_RAW_BIN",
            required_worker("ZEROSTACK_REAL_FSZERO_RAW_BIN", "fszero"),
        )
        .env(
            "ZERO_GRAPHZERO_RAW_BIN",
            required_worker("ZEROSTACK_REAL_GRAPHZERO_RAW_BIN", "graphzero"),
        )
        .env(
            "ZERO_TOKENZERO_RAW_BIN",
            required_worker("ZEROSTACK_REAL_TOKENZERO_RAW_BIN", "tokenzero"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = command.spawn().expect("spawn aggregate host");
    let mut input = child.stdin.take().expect("host stdin");
    let mut output = BufReader::new(child.stdout.take().expect("host stdout"));

    let ready = read(&mut output);
    assert_eq!(ready["protocol"], "zerostack-codemode-host/v2");
    let generation = ready["generation"].as_u64().expect("generation");
    send(
        &mut input,
        json!({
            "type":"execute",
            "id":1,
            "cell_id":"real-three-engine",
            "generation":generation,
            "yield_ms":10_000,
            "source":r#"
                const a=await zero.fs.compound('read',{path:'smoke.txt'});
                const b=await zero.graph.index();
                const c=await zero.token.compact('real aggregate smoke');
                return {fs:a.content.kind,graph:b.content.kind,token:c.content.kind};
            "#
        }),
    );
    let result = read(&mut output);
    assert_eq!(result["type"], "response", "{result}");
    assert_eq!(result["kind"], "result", "{result}");
    assert_eq!(result["generation"], generation, "{result}");
    assert!(result.get("delegate_id").is_none(), "{result}");
    let text = result["contentItems"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no visible result: {result}"));
    let visible: Value = serde_json::from_str(text).expect("visible JSON");
    assert_eq!(
        visible,
        json!({"fs":"inline","graph":"inline","token":"inline"})
    );

    send(&mut input, json!({"type":"shutdown","id":2}));
    let shutdown = read(&mut output);
    assert_eq!(shutdown["ok"], true, "{shutdown}");
    drop(input);
    assert!(child.wait().expect("host exit").success());
}

fn required_worker(env_name: &str, key: &str) -> String {
    if let Ok(path) = std::env::var(env_name) {
        return path;
    }
    let manifest = std::fs::read_to_string("tests/data/real_worker_smoke_paths.json")
        .unwrap_or_else(|_| panic!("missing {env_name} and real-worker path manifest"));
    let paths: Value = serde_json::from_str(&manifest).expect("real-worker path manifest JSON");
    paths[key]
        .as_str()
        .unwrap_or_else(|| panic!("real-worker path manifest omitted {key}"))
        .to_owned()
}

fn send(writer: &mut impl Write, value: Value) {
    serde_json::to_writer(&mut *writer, &value).expect("encode frame");
    writer.write_all(b"\n").expect("newline");
    writer.flush().expect("flush");
}

fn read(reader: &mut impl BufRead) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).expect("read frame");
    assert!(!line.is_empty(), "host closed before response");
    serde_json::from_str(&line).expect("JSON frame")
}
