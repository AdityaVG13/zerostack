//! Shared five-step agent orientation loop for release-gate token accounting.

use std::path::PathBuf;

use crate::basic::{BasicFixture, indexed_fixture};
use crate::scaled::{ScaledFixture, indexed_scaled_repo};
use graphzero_engine::query_surface::{QuerySurfaceRequest, QuerySurfaceRouter};
use graphzero_store::store::query::QueryEngine;
use graphzero_store::{ExpandResolver, GzRef, Snapshot};
use serde_json::{Value, json};

use crate::gates::release_harness::record_step;

pub const TOKEN_BUDGET: usize = 1;
pub const MAX_SESSION_TOKENS_SMALL: usize = 500;
pub const MAX_SESSION_TOKENS_MEDIUM: usize = 600;
pub const MAX_SESSION_TOKENS_LARGE: usize = 700;

/// Indexed repo fixture for token-by-task and scaled gate tests.
pub struct ScaledRepo {
    _dir: tempfile::TempDir,
    pub repo_root: PathBuf,
    pub store_root: PathBuf,
    pub target_symbol: String,
    pub file_count: usize,
}

impl ScaledRepo {
    fn from_scaled(fx: ScaledFixture) -> Self {
        Self {
            _dir: fx.dir,
            repo_root: fx.repo_root,
            store_root: fx.store_root,
            target_symbol: fx.target_symbol,
            file_count: fx.file_count,
        }
    }

    fn from_basic(fx: BasicFixture, target_symbol: &str) -> Self {
        Self {
            _dir: fx.dir,
            repo_root: fx.repo_root,
            store_root: fx.store_root,
            target_symbol: target_symbol.into(),
            file_count: 2,
        }
    }
}

pub fn index_scaled_repo(file_count: usize) -> ScaledRepo {
    ScaledRepo::from_scaled(indexed_scaled_repo(file_count))
}

pub fn index_two_file_repo() -> ScaledRepo {
    ScaledRepo::from_basic(indexed_fixture(), "alpha")
}

pub struct TokenByTaskReport {
    pub fixture_name: String,
    pub file_count: usize,
    pub target_symbol: String,
    pub steps: Vec<Value>,
    pub total_visible_bytes: usize,
    pub total_approx_tokens: usize,
    pub expand_bytes: usize,
}

pub fn run_five_step_session(fx: &ScaledRepo, fixture_name: &str) -> TokenByTaskReport {
    let snapshot = Snapshot::open(&fx.store_root, Some(&fx.repo_root)).expect("open snapshot");
    let target = &fx.target_symbol;

    let mut steps: Vec<Value> = Vec::new();
    let mut total_bytes = 0usize;
    let mut total_tokens = 0usize;

    let run_surface = |surface: &str, name: &str| -> (String, Option<String>) {
        let req = QuerySurfaceRequest {
            surface: surface.into(),
            name: Some(name.into()),
            query: Some(name.into()),
            budget: Some(TOKEN_BUDGET),
            ..Default::default()
        };
        let resp = QuerySurfaceRouter::execute(&snapshot, &req).expect("surface");
        let decl = resp.decl_ref.clone();
        let json = QuerySurfaceRouter::to_json_string_with_budget(
            &resp,
            TOKEN_BUDGET,
            Some(&fx.store_root),
        );
        (json, decl)
    };

    let (j1, decl) = run_surface("symbol", target);
    record_step(
        &mut steps,
        "orient_symbol",
        &j1,
        &mut total_bytes,
        &mut total_tokens,
    );

    let (j2, _) = run_surface("callers", target);
    record_step(
        &mut steps,
        "orient_callers",
        &j2,
        &mut total_bytes,
        &mut total_tokens,
    );

    let (j3, _) = run_surface("search", target);
    record_step(
        &mut steps,
        "search",
        &j3,
        &mut total_bytes,
        &mut total_tokens,
    );

    let cap = QueryEngine::warm(&snapshot, target, TOKEN_BUDGET).expect("snap");
    let j4 = cap.to_json(Some(&fx.store_root));
    record_step(&mut steps, "snap", &j4, &mut total_bytes, &mut total_tokens);

    let mut expand_bytes = 0usize;
    if let Some(reference) = decl {
        let gz = GzRef::parse(&reference).expect("gz ref");
        let resolver = ExpandResolver::new(&fx.store_root, Some(&fx.repo_root)).expect("resolver");
        let res = resolver.resolve(&gz, &reference).expect("expand");
        expand_bytes = res.bytes.len();
        total_bytes += expand_bytes;
        total_tokens += expand_bytes / 4;
        steps.push(json!({
            "step": "expand_one_ref",
            "bytes": expand_bytes,
            "approx_tokens": expand_bytes / 4,
        }));
    }

    TokenByTaskReport {
        fixture_name: fixture_name.to_string(),
        file_count: fx.file_count,
        target_symbol: target.clone(),
        steps,
        total_visible_bytes: total_bytes,
        total_approx_tokens: total_tokens,
        expand_bytes,
    }
}

pub fn report_to_json(report: &TokenByTaskReport) -> Value {
    json!({
        "schema_version": 1,
        "repo_fixture": report.fixture_name,
        "file_count": report.file_count,
        "target_symbol": report.target_symbol,
        "budget": TOKEN_BUDGET,
        "steps": report.steps,
        "total_visible_bytes": report.total_visible_bytes,
        "total_approx_tokens": report.total_approx_tokens,
        "expand_bytes": report.expand_bytes,
    })
}

pub fn write_latest_report(report: &TokenByTaskReport) -> PathBuf {
    use std::fs;
    use std::path::PathBuf;

    // Redirect to target/gate-artifacts (not committed)
    let out_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/gate-artifacts/token-by-task");
    fs::create_dir_all(&out_dir).expect("out dir");
    let out_path = out_dir.join("latest.json");
    fs::write(
        &out_path,
        serde_json::to_string_pretty(&report_to_json(report)).expect("json"),
    )
    .expect("write latest");
    out_path
}
