use graphzero_reserve::{
    DeclareRequest, IntentOperation, ReservationLedger, ReservationStatus, ReserveService,
    check_reservation, check_reservation_with_ttl, contract_footprint, declare_reservation,
    ledger_state_hash, list_active_reservations, release_reservation, replay_ledger,
    test_notify_conflict, test_notify_hook_count, test_reset_notify_hook,
};
use std::sync::{Arc, Barrier};
use zerostack_test_support::{ReserveFixture, reserve_indexed_fixture};

fn signature_change(symbol: &str) -> Vec<IntentOperation> {
    vec![IntentOperation {
        kind: "change_signature".into(),
        target_symbol: Some(symbol.into()),
        intent_text: Some(format!("change signature of {symbol}")),
    }]
}

fn parse_ref_ops() -> Vec<IntentOperation> {
    signature_change("parse_ref")
}

fn load_config_ops() -> Vec<IntentOperation> {
    signature_change("load_config")
}

fn acquire_agent(fixture: &ReserveFixture, agent_id: &str, operations: &[IntentOperation]) {
    let result = check_reservation(
        &fixture.store_root,
        &fixture.repo_root,
        agent_id,
        operations,
        true,
    )
    .expect("reservation acquisition");
    assert_eq!(result.verdict, "clear", "reservation acquisition failed");
}

fn acquire_parse_ref(fixture: &ReserveFixture, agent_id: &str) {
    acquire_agent(fixture, agent_id, &parse_ref_ops());
}

#[test]
fn declare_acquire_release_lifecycle() {
    let fx = reserve_indexed_fixture();
    let ops = parse_ref_ops();
    let decl = declare_reservation(
        &fx.store_root,
        &fx.repo_root,
        DeclareRequest {
            agent_id: "AgentA".into(),
            intent_ops: ops.clone(),
            ttl_seconds: 3600,
        },
    )
    .unwrap();
    assert!(decl.reservation_id.starts_with("res_AgentA_"));
    assert!(decl.footprint_ref.starts_with("footprint/"));
    assert_eq!(decl.status, ReservationStatus::Declared);
    assert!(decl.ttl_seconds >= 60);

    acquire_parse_ref(&fx, "AgentA");
    let active = list_active_reservations(&fx.store_root, &fx.repo_root).unwrap();
    assert!(
        active
            .reservations
            .iter()
            .any(|r| r.status == ReservationStatus::Active)
    );
    release_reservation(
        &fx.store_root,
        &fx.repo_root,
        "AgentA",
        &decl.reservation_id,
    )
    .unwrap();
    assert_eq!(
        check_reservation(&fx.store_root, &fx.repo_root, "AgentB", &ops, false)
            .unwrap()
            .verdict,
        "clear"
    );
}

#[test]
fn conflict_declared_and_cross_agent() {
    let fx = reserve_indexed_fixture();
    let ops = parse_ref_ops();

    declare_reservation(
        &fx.store_root,
        &fx.repo_root,
        DeclareRequest {
            agent_id: "AgentA".into(),
            intent_ops: ops.clone(),
            ttl_seconds: 3600,
        },
    )
    .unwrap();
    let declared = check_reservation(&fx.store_root, &fx.repo_root, "AgentB", &ops, false).unwrap();
    assert_eq!(declared.verdict, "conflict");
    assert!(!declared.evidence_refs.is_empty());

    let fx2 = reserve_indexed_fixture();
    acquire_parse_ref(&fx, "AgentA");
    let conflict = check_reservation(&fx.store_root, &fx.repo_root, "AgentB", &ops, false).unwrap();
    assert_eq!(conflict.verdict, "conflict");
    assert!(!conflict.overlap_nodes.is_empty());
    for edge in conflict.conflict_edges {
        assert!(
            edge.evidence_ref.starts_with("z://blob/")
                || edge.evidence_ref.starts_with("node/")
                || edge.evidence_ref.starts_with("path/")
                || edge.evidence_ref.starts_with("file/")
        );
    }
    assert_eq!(
        check_reservation(&fx2.store_root, &fx2.repo_root, "OtherAgent", &ops, false)
            .unwrap()
            .verdict,
        "clear"
    );
}

#[test]
fn query_active_footprint_reports_active_reservations() {
    let fx = reserve_indexed_fixture();
    acquire_parse_ref(&fx, "A1");
    acquire_agent(&fx, "A2", &load_config_ops());
    assert_eq!(
        list_active_reservations(&fx.store_root, &fx.repo_root)
            .unwrap()
            .active_count,
        2
    );

    let snap = graphzero_store::Snapshot::open(&fx.store_root, Some(&fx.repo_root)).unwrap();
    let fp = contract_footprint(&snap, "change signature of parse_ref").unwrap();
    assert!(fp.contract_nodes.iter().any(|n| n.contains("parse_ref")));
    assert!(fp.footprint_ref.starts_with("footprint/"));
}

#[test]
fn notify_hook_and_ledger_replay() {
    test_reset_notify_hook();
    let fx = reserve_indexed_fixture();
    acquire_parse_ref(&fx, "AgentA");
    let ops = parse_ref_ops();
    let _ = check_reservation(&fx.store_root, &fx.repo_root, "AgentB", &ops, false).unwrap();
    assert_eq!(test_notify_hook_count(), 0);
    test_notify_conflict("AgentB", &["node/parse_ref".into()]);
    assert_eq!(test_notify_hook_count(), 1);

    let live = ReservationLedger::open(&fx.store_root).unwrap();
    let replayed = replay_ledger(&fx.store_root).unwrap();
    assert_eq!(
        ledger_state_hash(live.records()).unwrap(),
        ledger_state_hash(&replayed).unwrap()
    );
}

#[test]
fn service_reopens_snapshot_after_store_is_reindexed() {
    let fx = reserve_indexed_fixture();
    let service = ReserveService::new(&fx.store_root, &fx.repo_root);
    let first = service.check("AgentA", &parse_ref_ops(), false).unwrap();
    assert_ne!(first.verdict, "conflict");

    std::fs::write(
        fx.repo_root.join("src/new_target.rs"),
        "pub fn newly_indexed_target() {}\n",
    )
    .unwrap();
    std::fs::write(
        fx.repo_root.join("src/lib.rs"),
        "pub mod parse_ref;\npub mod caller_a;\npub mod config_loader;\npub mod new_target;\n",
    )
    .unwrap();
    graphzero_store::store::indexer::index_repo(&fx.repo_root, &fx.store_root).unwrap();

    let new_ops = vec![graphzero_reserve::IntentOperation {
        kind: "change_signature".into(),
        target_symbol: Some("newly_indexed_target".into()),
        intent_text: Some("change signature of newly_indexed_target".into()),
    }];
    let refreshed = service.check("AgentA", &new_ops, false).unwrap();
    assert_ne!(refreshed.verdict, "conflict");
}

#[test]
fn concurrent_acquire_same_symbol_allows_only_one_active_reservation() {
    let fx = Arc::new(reserve_indexed_fixture());
    let ops = parse_ref_ops();
    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();

    for agent in ["AgentA", "AgentB"] {
        let fx = Arc::clone(&fx);
        let ops = ops.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            check_reservation(&fx.store_root, &fx.repo_root, agent, &ops, true)
                .unwrap()
                .verdict
        }));
    }

    let verdicts: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(verdicts.iter().filter(|v| v.as_str() == "clear").count(), 1);
    assert_eq!(
        verdicts.iter().filter(|v| v.as_str() == "conflict").count(),
        1
    );

    let active = list_active_reservations(&fx.store_root, &fx.repo_root).unwrap();
    assert_eq!(active.active_count, 1);
}

#[test]
fn acquired_reservation_uses_requested_ttl() {
    let fx = reserve_indexed_fixture();
    let ops = parse_ref_ops();
    let before = graphzero_reserve::now_ts();

    let check = check_reservation_with_ttl(
        &fx.store_root,
        &fx.repo_root,
        "AgentTTL",
        &ops,
        true,
        Some(120),
    )
    .unwrap();
    assert_eq!(check.verdict, "clear");

    let active = list_active_reservations(&fx.store_root, &fx.repo_root).unwrap();
    let reservation = active
        .reservations
        .iter()
        .find(|r| r.agent_id == "AgentTTL")
        .expect("active ttl reservation");
    assert_eq!(reservation.ttl_seconds, 120);
    assert!(reservation.expires_at >= before + 120);
    assert!(reservation.expires_at <= graphzero_reserve::now_ts() + 120);
}
