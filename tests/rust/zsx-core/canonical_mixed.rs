#![cfg(all(feature = "fszero", feature = "graphzero", feature = "tokenzero"))]

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zsx_core::{ZsxSession, process_spawn_count};

#[test]
fn one_cell_dispatches_all_three_real_engines_without_processes() {
    let workspace = tempfile::tempdir().expect("workspace");
    let state = tempfile::tempdir().expect("state");
    let root = workspace.path().canonicalize().expect("workspace root");
    std::fs::write(root.join("seed.txt"), "mixed-engine-seed\n").expect("seed file");
    let session = ZsxSession::builder(root)
        .with_state_root(state.path())
        .with_session_id("canonical-mixed")
        .build_canonical()
        .expect("canonical session");
    let spawns_before = process_spawn_count();
    let started = Instant::now();
    let result = session
        .execute(
            1,
            1,
            r#"const fs=await zero.fs.multi_read(["seed.txt"]);
               await zero.graph.index();
               const graph=await zero.graph.query("symbol","mixed-engine-seed");
               const token=await zero.token.find("mixed-engine-seed");
               return {
                 fs:fs.content.value.metadata.ownership.engine,
                 graph:graph.content.value.metadata.ownership.engine,
                 token:token.content.value.metadata.ownership.engine
               };"#,
            Duration::from_secs(30),
        )
        .expect("mixed real-engine cell");
    assert_eq!(
        result.value,
        json!({"fs":"fszero","graph":"graphzero","token":"tokenzero"})
    );
    assert_eq!(result.metrics.host.logical_operations, 4);
    assert_eq!(result.metrics.host.physical_dispatches, 4);
    assert_eq!(result.metrics.engine_dispatches, [1, 2, 1]);
    assert_eq!(process_spawn_count(), spawns_before);
    println!(
        "{}",
        json!({
            "schema":"zerostack.canonical_mixed_cell.v1",
            "zerostack_source_sha":option_env!("ZEROSTACK_SOURCE_SHA").unwrap_or("unbound"),
            "zerostack_diff_sha256":option_env!("ZEROSTACK_WORKTREE_DIFF_SHA256").unwrap_or("unbound"),
            "fszero_source_sha":option_env!("FSZERO_SOURCE_SHA").unwrap_or("unbound"),
            "fszero_diff_sha256":option_env!("FSZERO_WORKTREE_DIFF_SHA256").unwrap_or("unbound"),
            "graphzero_source_sha":option_env!("GRAPHZERO_SOURCE_SHA").unwrap_or("unbound"),
            "graphzero_diff_sha256":option_env!("GRAPHZERO_WORKTREE_DIFF_SHA256").unwrap_or("unbound"),
            "tokenzero_source_sha":option_env!("TOKENZERO_SOURCE_SHA").unwrap_or("unbound"),
            "tokenzero_diff_sha256":option_env!("TOKENZERO_WORKTREE_DIFF_SHA256").unwrap_or("unbound"),
            "engines":["fszero","graphzero","tokenzero"],
            "model_visible_calls":1,
            "process_spawns":0,
            "metrics":result.metrics,
            "wall_ns":started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64,
        })
    );
    session.shutdown().expect("shutdown");
}

#[test]
fn packaged_token_exact_ref_survives_session_restart() {
    let workspace = tempfile::tempdir().expect("workspace");
    let state = tempfile::tempdir().expect("state");
    let root = workspace.path().canonicalize().expect("workspace root");
    let state_root = state.path().canonicalize().expect("state root");
    // Cross the engine's exact-ref threshold without crossing its 65,536-byte
    // visible-output cap when the second session expands the whole blob.
    let payload = "portable ref payload\n".repeat(2_400);
    assert!(payload.len() > 40 * 1024 && payload.len() < 60 * 1024);
    std::fs::write(root.join("large.txt"), &payload).expect("seed large file");

    let first = ZsxSession::builder(root.clone())
        .with_state_root(state_root.clone())
        .with_session_id("canonical-token-exact-ref-first")
        .build_canonical()
        .expect("first canonical session");
    let result = first
        .execute(
            1,
            1,
            r#"return await zero.token.read({path:"large.txt"});"#,
            Duration::from_secs(30),
        )
        .expect("token read exact ref must pass packaged validation");
    let envelope = result.value;
    let ref_value = envelope["content"]["value"]["metadata"]["ownership"]["refs"]
        .as_array()
        .and_then(|refs| refs.iter().find_map(Value::as_str))
        .expect("token read returns exact ref")
        .to_owned();
    assert!(
        ref_value.starts_with("tz://blob/"),
        "unexpected ref: {ref_value}"
    );
    first.shutdown().expect("first shutdown");

    let second = ZsxSession::builder(root)
        .with_state_root(state_root.clone())
        .with_session_id("canonical-token-exact-ref-second")
        .build_canonical()
        .expect("second canonical session");
    let mut recovered = String::new();
    for (index, start) in (0..payload.len()).step_by(16_000).enumerate() {
        let end = (start + 16_000).min(payload.len());
        let fragment = format!("{ref_value}#B{start}-{end}");
        let expanded = second
            .execute(
                1,
                index as u64 + 1,
                format!(
                    "return await zero.token.expand({});",
                    serde_json::to_string(&fragment).expect("fragment ref JSON")
                ),
                Duration::from_secs(30),
            )
            .expect("exact ref fragment expands after parent session restart");
        let expanded_value = if expanded.value["spilled"].as_bool() == Some(true) {
            let spill_ref = expanded.value["ref"].as_str().expect("spill ref");
            let parsed = zero_ref::ZeroRefV1::parse(spill_ref).expect("portable spill ref");
            let bytes = zero_store::SharedCas::open(&state_root)
                .get_verified(&parsed.hash)
                .expect("spilled expansion remains reachable");
            serde_json::from_slice::<Value>(&bytes).expect("spilled expansion JSON")
        } else {
            expanded.value
        };
        recovered.push_str(
            expanded_value["content"]["value"]["value"]["tool_response"]["visible"]["text"]
                .as_str()
                .expect("expanded fragment is text"),
        );
    }
    assert_eq!(recovered, payload, "recovered bytes must be exact");
    second.shutdown().expect("second shutdown");
}
