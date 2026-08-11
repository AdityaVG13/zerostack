#![cfg(all(feature = "fszero", feature = "graphzero", feature = "tokenzero"))]

use std::time::{Duration, Instant};

use serde_json::json;
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
            r#"const fs=await zero.fs.read_many(["seed.txt"]);
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
