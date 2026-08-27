//! Source adapters and ingest orchestration (FR-002..FR-005).

use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::connectors::{ConnectorConfig, ConnectorStatus};
use crate::evidence::store_text_evidence;
use crate::redaction::redact_text;
use crate::schema::{
    ConnectorAvailability, NodeLinkState, ProvenanceSource, ProvenanceSourceKind, RedactionState,
    SCHEMA_VERSION, SourceCursor, WhyEdge, WhyRelation,
};
use crate::store::WhyStore;

#[derive(Clone, Debug, Deserialize)]
pub struct CommitFixtureEvent {
    pub commit_oid: String,
    pub message: String,
    #[serde(default)]
    pub author: Option<String>,
    pub path: String,
    pub node_ref: String,
    pub freshness: String,
    #[serde(default)]
    pub touched: Vec<CommitTouchedEntity>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CommitTouchedEntity {
    pub path: String,
    #[serde(default)]
    pub node_ref: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PrIssueFixture {
    pub pr: Option<PrFixture>,
    pub issue: Option<IssueFixture>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PrFixture {
    pub stable_id: String,
    pub title: String,
    pub node_ref: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct IssueFixture {
    pub stable_id: String,
    pub body: String,
    pub node_ref: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TraceFixture {
    pub session_id: String,
    pub rationale: String,
    pub touched_paths: Vec<String>,
    pub node_ref: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct IngestReport {
    pub new_edges: usize,
    pub total_edges: usize,
    pub redactions: usize,
    pub replay_digest: String,
    pub connectors: ConnectorStatus,
    pub evidence_failures: usize,
    pub unresolved_nodes: usize,
}

fn connector_available(cfg: &ConnectorConfig, kind: ProvenanceSourceKind) -> bool {
    cfg.availability(kind) == ConnectorAvailability::Available
}

fn ingest_report_from_ledger(
    ledger: &crate::store::WhyLedger,
    cfg: &ConnectorConfig,
    new_edges: usize,
    evidence_failures: usize,
    unresolved_nodes: usize,
) -> IngestReport {
    let redactions = ledger
        .edges
        .values()
        .filter(|e| e.redaction_state != RedactionState::None)
        .count();
    IngestReport {
        new_edges,
        total_edges: ledger.edges.len(),
        redactions,
        replay_digest: ledger.replay_digest.clone(),
        connectors: ConnectorStatus::from_config(cfg),
        evidence_failures,
        unresolved_nodes,
    }
}

fn count_unresolved_in_ledger(ledger: &crate::store::WhyLedger) -> usize {
    ledger
        .edges
        .values()
        .filter(|e| e.node_link_state == NodeLinkState::Pending)
        .count()
}

#[allow(clippy::too_many_arguments)]
fn upsert_resolved_edge(
    store: &WhyStore,
    repo_root: Option<&Path>,
    source_kind: ProvenanceSourceKind,
    stable_id: String,
    node_ref: String,
    relation: WhyRelation,
    confidence: f32,
    freshness: &str,
    evidence: String,
    redaction_state: RedactionState,
) -> Result<WhyEdge> {
    let edge = WhyEdge {
        schema_version: SCHEMA_VERSION,
        edge_id: String::new(),
        source: ProvenanceSource {
            kind: source_kind,
            stable_id,
        },
        node_ref: Some(node_ref.clone()),
        relation,
        confidence,
        source_freshness: Some(freshness.into()),
        evidence_refs: vec![evidence],
        redaction_state,
        node_link_state: NodeLinkState::Resolved,
        reserved: serde_json::json!({
            (crate::schema::NODE_REF_SPLIT_KEY_FIELD): node_ref
        }),
    };
    store.upsert_edge(edge, repo_root)
}

fn ingest_pr_fixture_edge(
    store: &WhyStore,
    repo_root: Option<&Path>,
    pr: &PrFixture,
) -> Result<WhyEdge> {
    let text = format!("PR {}: {}", pr.stable_id, pr.title);
    let evidence = store_text_evidence(&store.graphzero_root(), repo_root, &text)?;
    upsert_resolved_edge(
        store,
        repo_root,
        ProvenanceSourceKind::PrThread,
        pr.stable_id.clone(),
        pr.node_ref.clone(),
        WhyRelation::Reviewed,
        0.75,
        "fixture",
        evidence,
        RedactionState::None,
    )
}

fn ingest_issue_fixture_edge(
    store: &WhyStore,
    repo_root: Option<&Path>,
    issue: &IssueFixture,
) -> Result<WhyEdge> {
    let redacted = redact_text(&issue.body);
    let evidence = store_text_evidence(&store.graphzero_root(), repo_root, &redacted.text)?;
    upsert_resolved_edge(
        store,
        repo_root,
        ProvenanceSourceKind::Issue,
        issue.stable_id.clone(),
        issue.node_ref.clone(),
        WhyRelation::Discussed,
        0.7,
        "fixture",
        evidence,
        redacted.state,
    )
}

pub fn ingest_status(store: &WhyStore, cfg: &ConnectorConfig) -> Result<IngestReport> {
    let ledger = store.load_ledger()?;
    Ok(ingest_report_from_ledger(
        &ledger,
        cfg,
        0,
        0,
        count_unresolved_in_ledger(&ledger),
    ))
}

pub fn ingest_commit_fixture(
    store: &WhyStore,
    repo_root: Option<&Path>,
    cfg: &ConnectorConfig,
    event: &CommitFixtureEvent,
) -> Result<Option<WhyEdge>> {
    let edges = ingest_commit_metadata_fixture(store, repo_root, cfg, event)?;
    Ok(edges.into_iter().next())
}

pub fn ingest_commit_metadata_fixture(
    store: &WhyStore,
    repo_root: Option<&Path>,
    cfg: &ConnectorConfig,
    event: &CommitFixtureEvent,
) -> Result<Vec<WhyEdge>> {
    if !connector_available(cfg, ProvenanceSourceKind::GitCommit) {
        return Ok(Vec::new());
    }

    let payload = commit_evidence_payload(event);
    let evidence = store_text_evidence(&store.graphzero_root(), repo_root, &payload)?;
    let mut edges = Vec::with_capacity(1 + event.touched.len());
    edges.push(upsert_resolved_edge(
        store,
        repo_root,
        ProvenanceSourceKind::GitCommit,
        event.commit_oid.clone(),
        event.node_ref.clone(),
        WhyRelation::Introduced,
        0.85,
        &event.freshness,
        evidence.clone(),
        RedactionState::None,
    )?);

    for touched in &event.touched {
        let node_ref = touched
            .node_ref
            .clone()
            .unwrap_or_else(|| format!("gz://file/{}", touched.path));
        edges.push(upsert_resolved_edge(
            store,
            repo_root,
            ProvenanceSourceKind::GitCommit,
            event.commit_oid.clone(),
            node_ref,
            WhyRelation::Modified,
            0.8,
            &event.freshness,
            evidence.clone(),
            RedactionState::None,
        )?);
    }

    Ok(edges)
}

fn commit_evidence_payload(event: &CommitFixtureEvent) -> String {
    let mut payload = format!(
        "commit {}
path {}
{}",
        event.commit_oid, event.path, event.message
    );
    if let Some(author) = &event.author {
        payload.push_str(
            "
author ",
        );
        payload.push_str(author);
    }
    for touched in &event.touched {
        payload.push_str(
            "
touched ",
        );
        payload.push_str(&touched.path);
        if let Some(node_ref) = &touched.node_ref {
            payload.push(' ');
            payload.push_str(node_ref);
        }
    }
    payload
}

fn optional_pr_fixture_edge(
    store: &WhyStore,
    repo_root: Option<&Path>,
    cfg: &ConnectorConfig,
    fixture: &PrIssueFixture,
) -> Result<Option<WhyEdge>> {
    let pr = match &fixture.pr {
        Some(p) => p,
        None => return Ok(None),
    };
    if !connector_available(cfg, ProvenanceSourceKind::PrThread) {
        return Ok(None);
    }
    Ok(Some(ingest_pr_fixture_edge(store, repo_root, pr)?))
}

fn optional_issue_fixture_edge(
    store: &WhyStore,
    repo_root: Option<&Path>,
    cfg: &ConnectorConfig,
    fixture: &PrIssueFixture,
) -> Result<Option<WhyEdge>> {
    let issue = match &fixture.issue {
        Some(i) => i,
        None => return Ok(None),
    };
    if !connector_available(cfg, ProvenanceSourceKind::Issue) {
        return Ok(None);
    }
    Ok(Some(ingest_issue_fixture_edge(store, repo_root, issue)?))
}

pub fn ingest_pr_issue_fixture(
    store: &WhyStore,
    repo_root: Option<&Path>,
    cfg: &ConnectorConfig,
    fixture: &PrIssueFixture,
) -> Result<Vec<WhyEdge>> {
    let mut out = Vec::new();
    if let Some(edge) = optional_pr_fixture_edge(store, repo_root, cfg, fixture)? {
        out.push(edge);
    }
    if let Some(edge) = optional_issue_fixture_edge(store, repo_root, cfg, fixture)? {
        out.push(edge);
    }
    Ok(out)
}

pub fn ingest_trace_fixture(
    store: &WhyStore,
    repo_root: Option<&Path>,
    cfg: &ConnectorConfig,
    trace: &TraceFixture,
) -> Result<Option<WhyEdge>> {
    if !connector_available(cfg, ProvenanceSourceKind::AgentTrace) {
        return Ok(None);
    }
    let redacted = redact_text(&trace.rationale);
    let paths = trace.touched_paths.join(",");
    let payload = format!(
        "session {}\npaths {}\n{}",
        trace.session_id, paths, redacted.text
    );
    let evidence = store_text_evidence(&store.graphzero_root(), repo_root, &payload)?;
    let edge = upsert_resolved_edge(
        store,
        repo_root,
        ProvenanceSourceKind::AgentTrace,
        trace.session_id.clone(),
        trace.node_ref.clone(),
        WhyRelation::Decided,
        0.65,
        "trace_fixture",
        evidence,
        redacted.state,
    )?;
    Ok(Some(edge))
}

pub fn ingest_unresolved_node_edge(
    store: &WhyStore,
    repo_root: Option<&Path>,
    stable_id: &str,
    evidence_text: &str,
) -> Result<WhyEdge> {
    let evidence = store_text_evidence(&store.graphzero_root(), repo_root, evidence_text)?;
    let edge = WhyEdge {
        schema_version: SCHEMA_VERSION,
        edge_id: String::new(),
        source: ProvenanceSource {
            kind: ProvenanceSourceKind::GitCommit,
            stable_id: stable_id.into(),
        },
        node_ref: None,
        relation: WhyRelation::Modified,
        confidence: 0.5,
        source_freshness: Some("pending".into()),
        evidence_refs: vec![evidence],
        redaction_state: RedactionState::None,
        node_link_state: NodeLinkState::Pending,
        reserved: serde_json::Value::Null,
    };
    store.upsert_edge(edge, repo_root)
}

pub fn advance_cursor(
    store: &WhyStore,
    source: ProvenanceSource,
    position: &str,
    digest: &str,
    last_event: Option<&str>,
) -> Result<()> {
    store.upsert_cursor(SourceCursor {
        source,
        position: position.into(),
        digest: digest.into(),
        last_event_id: last_event.map(str::to_string),
    })
}

pub fn cursor_digest_for_events(events: &[&str]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for e in events {
        h.update(e.as_bytes());
        h.update(b"\n");
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Replays the golden fixture as a resumable sequence of serialized ledger updates.
///
/// Each edge/cursor publication is written to a synced temporary file and atomically
/// renamed while holding the why-ledger writer lock. A failed replay can therefore
/// expose a prefix of complete edges, but never malformed JSON or an interleaved
/// writer result. Edge IDs and evidence blobs are content-derived, and the cursor is
/// advanced last, so retrying the same replay safely converges to the complete state.
pub fn replay_golden(
    store: &WhyStore,
    repo_root: Option<&Path>,
    cfg: &ConnectorConfig,
    commit: &CommitFixtureEvent,
    trace: &TraceFixture,
    pr_issue: &PrIssueFixture,
) -> Result<IngestReport> {
    let before = store.load_ledger()?.edges.len();
    ingest_commit_fixture(store, repo_root, cfg, commit)?;
    ingest_trace_fixture(store, repo_root, cfg, trace)?;
    ingest_pr_issue_fixture(store, repo_root, cfg, pr_issue)?;
    let source = ProvenanceSource {
        kind: ProvenanceSourceKind::GitCommit,
        stable_id: "golden-window".into(),
    };
    let digest = cursor_digest_for_events(&["commit", "trace", "pr", "issue"]);
    advance_cursor(store, source, "window-1", &digest, Some("golden"))?;
    let after = store.load_ledger()?;
    Ok(ingest_report_from_ledger(
        &after,
        cfg,
        after.edges.len().saturating_sub(before),
        0,
        0,
    ))
}

pub fn load_json_fixture<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let data = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
    Ok(serde_json::from_str(&data)?)
}
