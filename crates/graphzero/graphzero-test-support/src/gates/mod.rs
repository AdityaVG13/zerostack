//! Release-gate harness: MCP sessions, token accounting, ref-first runners.

pub mod agent_loop;
pub mod blast_contract;
pub mod git_indexed_fixture;
pub mod mcp_session;
pub mod proof_matrix;
pub mod ref_first_gate;
pub mod release_harness;
pub mod snap_export_perf_gate;
pub mod token_accounting;
pub mod token_by_task;

pub use agent_loop::{
    AgentLoopReport, MAX_FULL_LOOP_TOKENS, run_full_agent_loop, write_agent_loop_artifact,
};
pub use blast_contract::{LOAD_CONFIG_INTENT, PARSE_REF_INTENT, blast_capsule};
pub use git_indexed_fixture::{GitIndexedFixture, cochange_git_indexed_fixture};
pub use mcp_session::{
    McpSession, assert_cli_mcp_fields_match, graphzero, mcp_tool_json, result_value, run_cli,
    tool_text,
};
pub use proof_matrix::{
    PROOF_SCALE_LARGE, PROOF_SCALE_SMALL, PROOF_TASKS, ProofFixture, ProofTask,
    assert_beats_ripgrep_at_scale, assert_compact_shell, assert_lossless_expand,
    build_proof_report, proof_fixture, run_graphzero,
};
pub use ref_first_gate::{
    RefFirstGateArtifact, RefFirstGateRun, finish_ref_first_gate, orient_body,
    run_blast_ref_first_gate, run_ref_first_query_surfaces, run_ref_first_query_surfaces_hook,
    scaled_orient_surface_requests, tier_c_git_surface_requests,
};
pub use release_harness::{
    REF_FIRST_BUDGET, REF_FIRST_MAX_VISIBLE_TOKENS, assert_ref_first, record_step,
    write_benchmark_artifact,
};
pub use snap_export_perf_gate::{
    SNAP_EXPORT_MAX_LATENCY_MS, SNAP_EXPORT_MAX_P99_LATENCY_MS, SNAP_EXPORT_MAX_SIZE_BUDGET1,
    SNAP_EXPORT_MIN_COMPETITOR_RATIO, SNAP_EXPORT_P99_ITERATIONS, SnapExportGateFailure,
    SnapExportGateReport, SnapExportMeasureError, assert_snap_export_contract,
    assert_snap_export_gate, assert_snap_export_perf_thresholds, measure_snap_export_gate,
    run_snap_export_gate,
};
pub use token_accounting::{
    McpTurnEstimate, TokenCounts, TokenizerFamily, estimate_orient_turn, record_mcp_turn,
    record_token_counts,
};
pub use token_by_task::{
    MAX_SESSION_TOKENS_LARGE, MAX_SESSION_TOKENS_MEDIUM, MAX_SESSION_TOKENS_SMALL, ScaledRepo,
    TOKEN_BUDGET, TokenByTaskReport, index_scaled_repo, index_two_file_repo, report_to_json,
    run_five_step_session, write_latest_report,
};
