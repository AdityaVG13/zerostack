use std::time::{Duration, SystemTime};

use zero_abi::zbf::{DurableProfileV1, ZbfArtifactKindV1, ZbfObjectV1};
use zero_abi::{ArtifactOwnerV1, DigestV1, EngineIdentity};
use zero_gate::{
    LifecycleReport, LifecycleState, McpReport, PlannerReport, ProgramReports, ProgramUsage,
    RealGcConfig, RealGcError, WorkerClosureKind, WorkerReport, apply_real_reachability_gc,
    mcp_evidence_digest,
};
use zero_store::{
    SharedCas, current_reachability_snapshot, gc_project_id, publish_reachability_snapshot,
};

fn leaf(payload: &[u8]) -> ZbfObjectV1 {
    ZbfObjectV1::new_leaf(
        ZbfArtifactKindV1::Snapshot,
        ArtifactOwnerV1::ZeroStack,
        DigestV1::from_bytes([1; 32]),
        DurableProfileV1::portable_strict(),
        DigestV1::from_bytes([2; 32]),
        DigestV1::from_bytes([3; 32]),
        payload.to_vec(),
    )
    .unwrap()
}

fn container(children: Vec<ZbfObjectV1>) -> ZbfObjectV1 {
    ZbfObjectV1::new_container(
        ZbfArtifactKindV1::Snapshot,
        ArtifactOwnerV1::ZeroStack,
        DigestV1::from_bytes([1; 32]),
        DurableProfileV1::portable_strict(),
        DigestV1::from_bytes([2; 32]),
        DigestV1::from_bytes([3; 32]),
        children,
    )
    .unwrap()
}

fn publish_three(root: &std::path::Path, hashes: [&str; 3]) -> String {
    let project = gc_project_id(root).unwrap();
    for ((_, producer), hash) in [
        (EngineIdentity::FsZero, "fszero"),
        (EngineIdentity::GraphZero, "graphzero"),
        (EngineIdentity::TokenZero, "tokenzero"),
    ]
    .into_iter()
    .zip(hashes)
    {
        publish_reachability_snapshot(root, producer, &project, 1, &[hash.to_string()]).unwrap();
    }
    project
}

fn assemble_with_gc(gc: zero_gate::GcReport) {
    let id = [9; 32];
    let tools = [5; 32];
    ProgramReports::new()
        .planner(PlannerReport::new(1, id, [1; 32], 3))
        .worker(WorkerReport::new(
            1,
            id,
            [2; 32],
            3,
            WorkerClosureKind::Commit,
            mcp_evidence_digest(2, 5, tools),
            [3; 32],
            [4; 32],
            ProgramUsage {
                cpu_ns: 1,
                memory_bytes: 1,
                io_bytes: 1,
            },
        ))
        .mcp(McpReport::new(1, id, 2, 5, tools))
        .lifecycle(LifecycleReport::new(1, id, 5, 3, LifecycleState::Closed))
        .gc(gc)
        .assemble()
        .expect("real applied GC evidence must pass Program assembly")
        .verify()
        .unwrap();
}

#[test]
fn real_three_producer_gc_retains_roots_and_nested_zbf_and_collects_garbage() {
    let dir = tempfile::tempdir().unwrap();
    let cas = SharedCas::open(dir.path());
    let profile = DurableProfileV1::portable_strict();
    let child = leaf(b"nested-child");
    let child_bytes = child.to_bytes(profile).unwrap();
    let child_hash = cas.put(&child_bytes).unwrap();
    let inner = container(vec![child]);
    let inner_bytes = inner.to_bytes(profile).unwrap();
    let inner_hash = cas.put(&inner_bytes).unwrap();
    let outer = container(vec![inner]);
    let outer_hash = cas.put(&outer.to_bytes(profile).unwrap()).unwrap();
    let graph_hash = cas.put(b"graph-root").unwrap();
    let token_hash = cas.put(b"token-root").unwrap();
    let garbage = b"collect-only-after-close";
    let garbage_hash = cas.put(garbage).unwrap();
    let project = publish_three(dir.path(), [&outer_hash, &graph_hash, &token_hash]);

    let mut config = RealGcConfig::new(dir.path(), "real-three-engine-gc");
    config.now = SystemTime::now() + Duration::from_secs(3600);
    assert!(matches!(
        apply_real_reachability_gc(&config),
        Err(RealGcError::LifecycleOpen)
    ));
    assert!(cas.contains(&garbage_hash));

    config.lifecycle_closed = true;
    let outcome = apply_real_reachability_gc(&config).unwrap();
    assert_eq!(outcome.project_id, project);
    assert_eq!(outcome.run_receipt.deleted, vec![garbage_hash.clone()]);
    assert_eq!(outcome.verified_freed_bytes, garbage.len() as u64);
    for hash in [outer_hash, inner_hash, child_hash, graph_hash, token_hash] {
        assert!(cas.contains(&hash), "reachable {hash} must survive");
    }
    assert!(!cas.contains(&garbage_hash));
    assert_eq!(outcome.producer_epochs.len(), 3);
    assemble_with_gc(outcome.program_report([9; 32]));
}

#[test]
fn missing_or_empty_producer_and_stale_epoch_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let cas = SharedCas::open(dir.path());
    let fs_hash = cas.put(b"fs").unwrap();
    let graph_hash = cas.put(b"graph").unwrap();
    let project = gc_project_id(dir.path()).unwrap();
    publish_reachability_snapshot(dir.path(), "fszero", &project, 1, &[fs_hash]).unwrap();
    publish_reachability_snapshot(dir.path(), "graphzero", &project, 1, &[graph_hash]).unwrap();
    let mut config = RealGcConfig::new(dir.path(), "missing-producer");
    config.lifecycle_closed = true;
    assert!(matches!(
        apply_real_reachability_gc(&config),
        Err(RealGcError::MissingProducer("tokenzero"))
    ));

    publish_reachability_snapshot(dir.path(), "tokenzero", &project, 1, &[]).unwrap();
    assert!(matches!(
        apply_real_reachability_gc(&config),
        Err(RealGcError::EmptyProducer("tokenzero"))
    ));
    assert!(publish_reachability_snapshot(dir.path(), "tokenzero", &project, 1, &[]).is_err());
    assert_eq!(
        current_reachability_snapshot(dir.path(), "tokenzero", &project)
            .unwrap()
            .unwrap()
            .epoch,
        1
    );
}

#[test]
fn foreign_malformed_and_corrupt_engine_roots_fail_closed() {
    let local = tempfile::tempdir().unwrap();
    let foreign = tempfile::tempdir().unwrap();
    let local_cas = SharedCas::open(local.path());
    let foreign_hash = SharedCas::open(foreign.path()).put(b"foreign").unwrap();
    let local_graph = local_cas.put(b"local-graph").unwrap();
    let local_token = local_cas.put(b"local-token").unwrap();
    let project = gc_project_id(local.path()).unwrap();

    assert!(
        publish_reachability_snapshot(local.path(), "fszero", &project, 1, &["bad-ref".into()])
            .is_err()
    );
    publish_reachability_snapshot(local.path(), "fszero", &project, 1, &[foreign_hash]).unwrap();
    publish_reachability_snapshot(
        local.path(),
        "graphzero",
        &project,
        1,
        &[local_graph.clone()],
    )
    .unwrap();
    publish_reachability_snapshot(local.path(), "tokenzero", &project, 1, &[local_token]).unwrap();
    let mut config = RealGcConfig::new(local.path(), "foreign-root");
    config.lifecycle_closed = true;
    assert!(matches!(
        apply_real_reachability_gc(&config),
        Err(RealGcError::Store(error)) if error.contains("verify fszero root")
    ));

    let corrupt = tempfile::tempdir().unwrap();
    let corrupt_cas = SharedCas::open(corrupt.path());
    let fs_hash = corrupt_cas.put(b"fs-root").unwrap();
    let graph_hash = corrupt_cas.put(b"graph-root").unwrap();
    let token_hash = corrupt_cas.put(b"token-root").unwrap();
    publish_three(corrupt.path(), [&fs_hash, &graph_hash, &token_hash]);
    std::fs::write(corrupt_cas.object_path(&fs_hash), b"tampered").unwrap();
    let mut corrupt_config = RealGcConfig::new(corrupt.path(), "corrupt-root");
    corrupt_config.lifecycle_closed = true;
    assert!(matches!(
        apply_real_reachability_gc(&corrupt_config),
        Err(RealGcError::Store(error)) if error.contains("verify fszero root")
    ));
}
