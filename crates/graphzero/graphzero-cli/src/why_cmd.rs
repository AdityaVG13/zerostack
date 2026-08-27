//! P4.4 `graphzero why` diagnostics (FR-011).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use graphzero_store::resolve_graphzero_store_root;
use graphzero_why::WhyStore;
use graphzero_why::connectors::ConnectorConfig;
use graphzero_why::ingest::{
    CommitFixtureEvent, PrIssueFixture, TraceFixture, ingest_commit_fixture,
    ingest_pr_issue_fixture, ingest_status, ingest_trace_fixture, load_json_fixture, replay_golden,
};
use serde::de::DeserializeOwned;

fn resolve_why_fixtures_dir(repo: &Path, override_dir: Option<&Path>) -> PathBuf {
    override_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| repo.join("tests/fixtures/why"))
}

fn open_why_store(repo: &Path) -> Result<WhyStore> {
    WhyStore::open(&resolve_graphzero_store_root(repo))
}

fn ingest_optional<T, F>(
    store: &WhyStore,
    repo: Option<&Path>,
    cfg: &ConnectorConfig,
    fixtures: &Path,
    filename: &str,
    ingest: F,
) -> Result<()>
where
    T: DeserializeOwned,
    F: FnOnce(&WhyStore, Option<&Path>, &ConnectorConfig, &T) -> Result<()>,
{
    let path = fixtures.join(filename);
    if !path.exists() {
        return Ok(());
    }
    let value: T = load_json_fixture(&path)?;
    ingest(store, repo, cfg, &value)
}

pub fn run_ingest(repo: &Path, fixtures_dir: Option<&Path>) -> Result<()> {
    let store_root = resolve_graphzero_store_root(repo);
    let store = WhyStore::open(&store_root)?;
    let cfg = ConnectorConfig::all_enabled();
    let fixtures = resolve_why_fixtures_dir(repo, fixtures_dir);

    ingest_optional(
        &store,
        Some(repo),
        &cfg,
        &fixtures,
        "commit.json",
        |s, r, c, v| ingest_commit_fixture(s, r, c, v).map(|_| ()),
    )?;
    ingest_optional(
        &store,
        Some(repo),
        &cfg,
        &fixtures,
        "trace.json",
        |s, r, c, v| ingest_trace_fixture(s, r, c, v).map(|_| ()),
    )?;
    ingest_optional(
        &store,
        Some(repo),
        &cfg,
        &fixtures,
        "pr_issue.json",
        |s, r, c, v| ingest_pr_issue_fixture(s, r, c, v).map(|_| ()),
    )?;

    let report = ingest_status(&store, &cfg)?;
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}

pub fn run_status(repo: &Path) -> Result<()> {
    let store = open_why_store(repo)?;
    let cfg = ConnectorConfig::all_enabled();
    let report = ingest_status(&store, &cfg)?;
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}

pub fn run_replay(repo: &Path, fixtures_dir: &Path) -> Result<()> {
    let store_root = resolve_graphzero_store_root(repo);
    let store = WhyStore::open(&store_root)?;
    let cfg = ConnectorConfig::all_enabled();
    let commit: CommitFixtureEvent = load_json_fixture(&fixtures_dir.join("commit.json"))?;
    let trace: TraceFixture = load_json_fixture(&fixtures_dir.join("trace.json"))?;
    let pr_issue: PrIssueFixture = load_json_fixture(&fixtures_dir.join("pr_issue.json"))?;
    let r1 = replay_golden(&store, Some(repo), &cfg, &commit, &trace, &pr_issue)?;
    let digest1 = r1.replay_digest;
    let r2 = replay_golden(&store, Some(repo), &cfg, &commit, &trace, &pr_issue)?;
    let out = serde_json::json!({
        "replay_digest": r2.replay_digest,
        "digest_unchanged": digest1 == r2.replay_digest,
        "total_edges": r2.total_edges,
        "new_edges_second_run": r2.new_edges,
    });
    println!("{}", serde_json::to_string(&out)?);
    Ok(())
}

pub fn run_evidence_check(repo: &Path) -> Result<()> {
    use graphzero_why::evidence::expand_evidence_ref;
    let store_root = resolve_graphzero_store_root(repo);
    let store = WhyStore::open(&store_root)?;
    let mut failures = 0usize;
    for edge in store.all_edges()? {
        for r in &edge.evidence_refs {
            if expand_evidence_ref(&store_root, Some(repo), r).is_err() {
                failures += 1;
            }
        }
    }
    let out = serde_json::json!({ "evidence_failures": failures, "ok": failures == 0 });
    println!("{}", serde_json::to_string(&out)?);
    if failures > 0 {
        anyhow::bail!("evidence expansion failures: {failures}");
    }
    Ok(())
}

pub fn repo_canonical(repo: &Path) -> Result<PathBuf> {
    repo.canonicalize().context("repo path")
}
