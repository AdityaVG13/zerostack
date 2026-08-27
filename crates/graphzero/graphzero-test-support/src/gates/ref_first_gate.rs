//! Ref-first release gate harness for query surfaces and blast.

use std::path::Path;

use graphzero_engine::query_surface::{
    QuerySurfaceRequest, QuerySurfaceResponse, QuerySurfaceRouter,
};
use graphzero_store::Snapshot;
use serde_json::{Value, json};

use crate::gates::release_harness::{
    REF_FIRST_BUDGET, assert_ref_first, record_step, write_benchmark_artifact,
};

pub struct RefFirstGateRun {
    pub steps: Vec<Value>,
    pub total_tokens: usize,
}

fn surface_request(
    surface: &str,
    query: Option<String>,
    name: Option<String>,
    path: Option<String>,
) -> QuerySurfaceRequest {
    QuerySurfaceRequest {
        surface: surface.into(),
        name,
        query,
        path,
        budget: Some(REF_FIRST_BUDGET),
        ..Default::default()
    }
}

pub fn orient_body(snapshot: &Snapshot, store_root: &Path, surface: &str, query: &str) -> String {
    let req = surface_request(surface, Some(query.into()), None, None);
    let resp = QuerySurfaceRouter::execute(snapshot, &req).expect(surface);
    QuerySurfaceRouter::to_json_string_with_budget(&resp, REF_FIRST_BUDGET, Some(store_root))
}

pub fn tier_c_git_surface_requests() -> Vec<(&'static str, QuerySurfaceRequest)> {
    ["hot", "changes"]
        .into_iter()
        .map(|surface| {
            (
                surface,
                surface_request(surface, Some(surface.into()), None, None),
            )
        })
        .collect()
}

pub fn scaled_orient_surface_requests(
    target: &str,
    outline_path: &str,
) -> Vec<(&'static str, QuerySurfaceRequest)> {
    vec![
        (
            "symbol",
            surface_request("symbol", Some(target.into()), Some(target.into()), None),
        ),
        (
            "callers",
            surface_request("callers", Some(target.into()), Some(target.into()), None),
        ),
        (
            "deps",
            surface_request("deps", Some(target.into()), Some(target.into()), None),
        ),
        (
            "outline",
            surface_request("outline", None, None, Some(outline_path.into())),
        ),
        (
            "context",
            surface_request(
                "context",
                Some(format!("impact of changing {target}")),
                None,
                None,
            ),
        ),
        (
            "hot",
            surface_request("hot", Some("hot".into()), None, None),
        ),
        (
            "changes",
            surface_request("changes", Some("changes".into()), None, None),
        ),
        (
            "word",
            surface_request("word", Some("sym".into()), None, None),
        ),
        (
            "search",
            surface_request("search", Some(target.into()), None, None),
        ),
    ]
}

pub fn run_ref_first_query_surfaces(
    snapshot: &Snapshot,
    store_root: &Path,
    surfaces: &[(&str, QuerySurfaceRequest)],
) -> RefFirstGateRun {
    run_ref_first_query_surfaces_hook(snapshot, store_root, surfaces, |_, _, _| {})
}

pub fn run_ref_first_query_surfaces_hook<F>(
    snapshot: &Snapshot,
    store_root: &Path,
    surfaces: &[(&str, QuerySurfaceRequest)],
    mut after_step: F,
) -> RefFirstGateRun
where
    F: FnMut(&str, &QuerySurfaceResponse, &str),
{
    let mut steps = Vec::new();
    let mut total_bytes = 0usize;
    let mut total_tokens = 0usize;

    for (name, req) in surfaces {
        let resp = QuerySurfaceRouter::execute(snapshot, req).expect(name);
        let body = QuerySurfaceRouter::to_json_string_with_budget(
            &resp,
            REF_FIRST_BUDGET,
            Some(store_root),
        );
        assert_ref_first(name, &body, REF_FIRST_BUDGET);
        after_step(name, &resp, &body);
        record_step(&mut steps, name, &body, &mut total_bytes, &mut total_tokens);
    }

    RefFirstGateRun {
        steps,
        total_tokens,
    }
}

pub struct RefFirstGateArtifact {
    pub subdir: &'static str,
    pub artifact: &'static str,
    pub fixture_label: &'static str,
    pub surface_count: usize,
    pub extra: Value,
    pub max_total_tokens: usize,
    pub gate_label: &'static str,
}

pub fn finish_ref_first_gate(run: RefFirstGateRun, cfg: RefFirstGateArtifact) {
    let report = merge_extra(
        json!({
            "schema_version": 1,
            "fixture": cfg.fixture_label,
            "surface_count": cfg.surface_count,
            "budget": REF_FIRST_BUDGET,
            "steps": run.steps,
            "total_approx_tokens": run.total_tokens,
        }),
        cfg.extra,
    );
    let path = write_benchmark_artifact(cfg.subdir, cfg.artifact, &report);
    eprintln!("wrote {}", path.display());
    eprintln!("{}_total_tokens={}", cfg.gate_label, run.total_tokens);
    assert!(
        run.total_tokens < cfg.max_total_tokens,
        "{} at budget=1 must stay under {}: {}",
        cfg.gate_label,
        cfg.max_total_tokens,
        run.total_tokens
    );
}

fn merge_extra(mut report: Value, extra: Value) -> Value {
    if let (Some(obj), Some(extra_obj)) = (report.as_object_mut(), extra.as_object()) {
        for (k, v) in extra_obj {
            obj.insert(k.clone(), v.clone());
        }
    }
    report
}

pub fn run_blast_ref_first_gate(
    snapshot: &Snapshot,
    store_root: &Path,
    intent: &str,
    max_total_tokens: usize,
) -> RefFirstGateRun {
    use graphzero_engine::blast::{blast_radius, blast_to_json_budget};

    let capsule = blast_radius(snapshot, intent, REF_FIRST_BUDGET).expect("blast");
    let body = blast_to_json_budget(&capsule, REF_FIRST_BUDGET, Some(store_root))
        .expect("serialize blast capsule");
    assert!(
        body.starts_with("q:"),
        "blast budget=1 must return q: ref, got: {body}"
    );
    assert_ref_first("blast", &body, REF_FIRST_BUDGET);

    let mut steps = Vec::new();
    let mut total_bytes = 0usize;
    let mut total_tokens = 0usize;
    record_step(
        &mut steps,
        "blast",
        &body,
        &mut total_bytes,
        &mut total_tokens,
    );
    assert!(
        total_tokens <= max_total_tokens,
        "blast at budget=1 must stay under {max_total_tokens}: {total_tokens}"
    );
    RefFirstGateRun {
        steps,
        total_tokens,
    }
}
