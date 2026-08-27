//! Full ZeroStack agent loop: orient → blast → reserve → verify @ budget=1.

use graphzero_engine::blast::{blast_radius, blast_to_json_budget};
use graphzero_engine::query_surface::{QuerySurfaceRequest, QuerySurfaceRouter};
use graphzero_reserve::{
    DeclareRequest, check_reservation, declare_reservation, release_reservation,
};
use graphzero_store::store::query::QueryEngine;
use graphzero_store::{
    ClaimKind, ClaimVerifyConfig, ExpandResolver, GzRef, Snapshot, verify_claim,
};
use serde_json::{Value, json};
use std::path::PathBuf;

use crate::blast::BlastFixture;
use crate::gates::release_harness::{
    REF_FIRST_BUDGET, assert_ref_first, record_bytes_step, record_step, write_benchmark_artifact,
};

pub const MAX_FULL_LOOP_TOKENS: usize = 800;

const PARSE_REF_OPS: [(&str, &str); 1] = [("change_signature", "change signature of parse_ref")];

pub struct AgentLoopReport {
    pub fixture: String,
    pub target_symbol: String,
    pub steps: Vec<Value>,
    pub total_approx_tokens: usize,
}

pub fn run_full_agent_loop(fx: &BlastFixture) -> AgentLoopReport {
    let snapshot = Snapshot::open(&fx.store_root, Some(&fx.repo_root))
        .expect("failed to open fixture snapshot for agent-loop gate");
    let target = "parse_ref";
    let budget = REF_FIRST_BUDGET;

    let mut steps = Vec::new();
    let mut total_bytes = 0usize;
    let mut total_tokens = 0usize;

    let mut run_surface = |surface: &str, name: &str, path: Option<&str>| {
        let req = QuerySurfaceRequest {
            surface: surface.into(),
            name: Some(name.into()),
            query: Some(name.into()),
            path: path.map(str::to_string),
            budget: Some(budget),
            ..Default::default()
        };
        let resp = QuerySurfaceRouter::execute(&snapshot, &req)
            .expect("failed to execute query surface in agent-loop gate");
        let json =
            QuerySurfaceRouter::to_json_string_with_budget(&resp, budget, Some(&fx.store_root));
        assert_ref_first(&format!("orient_{surface}"), &json, budget);
        record_step(
            &mut steps,
            &format!("orient_{surface}"),
            &json,
            &mut total_bytes,
            &mut total_tokens,
        );
        resp.decl_ref
    };

    let decl = run_surface("symbol", target, None);
    run_surface("callers", target, None);
    run_surface("search", target, None);

    let snap_json = QueryEngine::warm(&snapshot, target, budget)
        .expect("failed to create snap capsule in agent-loop gate")
        .to_json(Some(&fx.store_root));
    assert_ref_first("snap", &snap_json, budget);
    record_step(
        &mut steps,
        "snap",
        &snap_json,
        &mut total_bytes,
        &mut total_tokens,
    );

    if let Some(reference) = decl {
        let gz = GzRef::parse(&reference)
            .expect("agent-loop snap must return a valid GraphZero reference");
        let resolver = ExpandResolver::new(&fx.store_root, Some(&fx.repo_root))
            .expect("failed to create expand resolver for agent-loop gate");
        let bytes = resolver
            .resolve(&gz, &reference)
            .expect("failed to expand snap reference in agent-loop gate")
            .bytes;
        record_bytes_step(
            &mut steps,
            "expand_evidence",
            &bytes,
            &mut total_bytes,
            &mut total_tokens,
        );
    }

    let intent = "change signature of parse_ref";
    let blast_cap = blast_radius(&snapshot, intent, budget)
        .expect("failed to compute blast radius in agent-loop gate");
    let blast_json = blast_to_json_budget(&blast_cap, budget, Some(&fx.store_root))
        .expect("failed to serialize blast capsule for agent-loop gate");
    assert_ref_first("blast", &blast_json, budget);
    record_step(
        &mut steps,
        "blast",
        &blast_json,
        &mut total_bytes,
        &mut total_tokens,
    );

    let intent_ops = vec![graphzero_reserve::IntentOperation {
        kind: PARSE_REF_OPS[0].0.into(),
        target_symbol: Some(target.into()),
        intent_text: Some(PARSE_REF_OPS[0].1.into()),
    }];

    let declare = declare_reservation(
        &fx.store_root,
        &fx.repo_root,
        DeclareRequest {
            agent_id: "LoopAgent".into(),
            intent_ops: intent_ops.clone(),
            ttl_seconds: 3600,
        },
    )
    .expect("failed to declare reservation in agent-loop gate");
    let declare_json = serde_json::to_string(&declare)
        .expect("failed to serialize reservation declaration as JSON");
    record_step(
        &mut steps,
        "reserve_declare",
        &declare_json,
        &mut total_bytes,
        &mut total_tokens,
    );

    let check = check_reservation(
        &fx.store_root,
        &fx.repo_root,
        "LoopAgent",
        &intent_ops,
        false,
    )
    .expect("failed to check reservation in agent-loop gate");
    assert!(
        check.verdict == "clear" || check.verdict == "unknown",
        "pre-acquire check must be clear or coverage-bound unknown, got {}",
        check.verdict
    );
    let check_json =
        serde_json::to_string(&check).expect("failed to serialize reservation check as JSON");
    record_step(
        &mut steps,
        "reserve_check",
        &check_json,
        &mut total_bytes,
        &mut total_tokens,
    );

    release_reservation(
        &fx.store_root,
        &fx.repo_root,
        "LoopAgent",
        &declare.reservation_id,
    )
    .expect("failed to release reservation in agent-loop gate");
    record_step(
        &mut steps,
        "reserve_release",
        r#"{"status":"released"}"#,
        &mut total_bytes,
        &mut total_tokens,
    );

    let refuted = verify_claim(
        &snapshot,
        ClaimKind::NoRemainingCallers,
        target,
        ClaimVerifyConfig::default(),
    )
    .expect("failed to verify caller claim in agent-loop gate");
    assert!(
        !refuted.verified,
        "parse_ref must still have callers in fixture"
    );
    let refuted_json = refuted
        .to_json_string()
        .expect("failed to serialize verification result as JSON");
    record_step(
        &mut steps,
        "verify_callers_refuted",
        &refuted_json,
        &mut total_bytes,
        &mut total_tokens,
    );

    let removed = verify_claim(
        &snapshot,
        ClaimKind::SymbolRemoved,
        "zz_ghost_symbol_no_match_xyz_999",
        ClaimVerifyConfig {
            tier_a_threshold: 0.85,
            check_freshness: false,
        },
    )
    .expect("failed to verify removal claim in agent-loop gate");
    assert!(
        removed.verified,
        "symbol_removed summary: {}",
        removed.summary
    );
    let removed_json = removed
        .to_json_string()
        .expect("failed to serialize verification result as JSON");
    record_step(
        &mut steps,
        "verify_symbol_removed",
        &removed_json,
        &mut total_bytes,
        &mut total_tokens,
    );

    AgentLoopReport {
        fixture: "blast_parse_ref".into(),
        target_symbol: target.into(),
        steps,
        total_approx_tokens: total_tokens,
    }
}

pub fn report_to_json(report: &AgentLoopReport) -> Value {
    json!({
        "schema_version": 1,
        "fixture": report.fixture,
        "target_symbol": report.target_symbol,
        "budget": REF_FIRST_BUDGET,
        "steps": report.steps,
        "total_approx_tokens": report.total_approx_tokens,
    })
}

pub fn write_agent_loop_artifact(report: &AgentLoopReport) -> PathBuf {
    let path = write_benchmark_artifact("agent-loop", "latest.json", &report_to_json(report));
    eprintln!("wrote {}", path.display());
    eprintln!("agent_loop_total_tokens={}", report.total_approx_tokens);
    path
}
