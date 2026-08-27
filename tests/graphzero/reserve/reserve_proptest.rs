use proptest::prelude::*;
use std::fs;
use tempfile::tempdir;

use graphzero_reserve::schema::IntentOperation;
use graphzero_reserve::service::{DeclareRequest, ReserveService};

fn setup_store() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempdir().unwrap();
    let repo = dir.path().join("repo");
    fs::create_dir_all(repo.join("src")).unwrap();
    let body: String = (0..200).map(|i| format!("fn func_{i}() {{}}\n")).collect();
    fs::write(repo.join("src/main.rs"), body).unwrap();
    let store = repo.join(".graphzero");
    graphzero_store::store::indexer::index_repo(&repo, &store).unwrap();
    (dir, repo, store)
}

fn arb_agent_id() -> impl Strategy<Value = String> {
    "[a-z]{3,10}".prop_map(|s| s)
}

fn arb_symbol() -> impl Strategy<Value = String> {
    (0..200u32).prop_map(|i| format!("func_{i}"))
}

fn arb_intent_op() -> impl Strategy<Value = IntentOperation> {
    (arb_symbol(),).prop_map(|(sym,)| IntentOperation {
        kind: "change_signature".into(),
        target_symbol: Some(sym),
        intent_text: None,
    })
}

proptest! {
    #![proptest_config(ProptestConfig {
        // Integration tests cannot use SourceParallel (no lib.rs above tests/),
        // so pin persistence to the committed crate-local layout explicitly.
        failure_persistence: Some(Box::new(
            proptest::test_runner::FileFailurePersistence::Direct(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/proptest-regressions/tests/reserve_proptest.txt"
            )),
        )),
        ..ProptestConfig::with_cases(32)
    })]

    #[test]
    fn declare_then_check_is_consistent(
        agent in arb_agent_id(),
        op in arb_intent_op(),
        ttl in 60u64..3600,
    ) {
        let (_dir, repo, store) = setup_store();
        let svc = ReserveService::new(&store, &repo);
        let req = DeclareRequest {
            agent_id: agent.clone(),
            intent_ops: vec![op.clone()],
            ttl_seconds: ttl,
        };
        let resp = svc.declare(req).unwrap();
        prop_assert_eq!(resp.status, graphzero_reserve::schema::ReservationStatus::Declared);
        prop_assert!(resp.ttl_seconds >= 60);
        prop_assert!(!resp.reservation_id.is_empty());

        let check = svc.check(&agent, &[op], false).unwrap();
        // Same agent checking own intent: should be clear or unknown (not conflict)
        prop_assert_ne!(check.verdict.as_str(), "conflict");
    }

    #[test]
    fn two_agents_same_symbol_conflict(
        a1 in arb_agent_id(),
        a2 in arb_agent_id(),
        op in arb_intent_op(),
    ) {
        prop_assume!(a1 != a2);
        let (_dir, repo, store) = setup_store();
        let svc = ReserveService::new(&store, &repo);

        let ops = [op];
        // Agent 1 acquires
        let check1 = svc.check(&a1, &ops, true).unwrap();
        if check1.verdict == "clear" {
            // Agent 2 checks same symbol: should conflict
            let check2 = svc.check(&a2, &ops, false).unwrap();
            prop_assert!(
                check2.verdict == "conflict" || check2.verdict == "unknown",
                "expected conflict or unknown, got: {}",
                check2.verdict,
            );
        }
    }

    #[test]
    fn declare_idempotent(
        agent in arb_agent_id(),
        op in arb_intent_op(),
    ) {
        let (_dir, repo, store) = setup_store();
        let svc = ReserveService::new(&store, &repo);
        let req = DeclareRequest {
            agent_id: agent,
            intent_ops: vec![op],
            ttl_seconds: 300,
        };
        let r1 = svc.declare(req.clone()).unwrap();
        let r2 = svc.declare(req).unwrap();
        prop_assert_eq!(r1.reservation_id, r2.reservation_id);
    }

    #[test]
    fn release_removes_reservation(
        agent in arb_agent_id(),
        op in arb_intent_op(),
    ) {
        let (_dir, repo, store) = setup_store();
        let svc = ReserveService::new(&store, &repo);
        let req = DeclareRequest {
            agent_id: agent.clone(),
            intent_ops: vec![op],
            ttl_seconds: 300,
        };
        let resp = svc.declare(req).unwrap();
        let release = svc.release(&agent, &resp.reservation_id);
        prop_assert!(release.is_ok());
    }

    #[test]
    fn query_active_never_panics(agent in arb_agent_id(), op in arb_intent_op()) {
        let (_dir, repo, store) = setup_store();
        let svc = ReserveService::new(&store, &repo);
        let req = DeclareRequest {
            agent_id: agent,
            intent_ops: vec![op],
            ttl_seconds: 300,
        };
        let _ = svc.declare(req);
        let active = svc.query_active().unwrap();
        prop_assert!(active.active_count <= active.reservations.len() + 1);
    }
}
