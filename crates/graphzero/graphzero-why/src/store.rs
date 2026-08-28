//! `.graphzero/why/` persistence (ADR-WHY-001).

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::edge_id::compute_edge_id;
use crate::evidence::validate_evidence_refs;
use crate::schema::{
    ProvenanceSource, ProvenanceSourceKind, RedactionState, SourceCursor, WhyEdge,
    WhyQueryManifest, WhyRelation,
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WhyLedger {
    pub schema_version: u32,
    pub edges: BTreeMap<String, WhyEdge>,
    pub cursors: BTreeMap<String, SourceCursor>,
    pub replay_digest: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WhyChainEntry {
    pub edge_id: String,
    pub source: ProvenanceSource,
    pub relation: WhyRelation,
    pub confidence: f32,
    pub source_freshness: Option<String>,
    pub evidence_refs: Vec<String>,
    pub redaction_state: RedactionState,
}

pub struct WhyStore {
    root: PathBuf,
}

fn finish_atomic_write(result: Result<()>, temp_path: &Path) -> Result<()> {
    if result.is_err() {
        let _ = fs::remove_file(temp_path);
    }
    result
}

fn validate_node_ref_identity_policy(
    existing: &BTreeMap<String, WhyEdge>,
    incoming: &[WhyEdge],
) -> Result<()> {
    type SourceKey = (ProvenanceSourceKind, String);
    type NodePolicy = (bool, BTreeSet<String>);
    let mut groups = BTreeMap::<SourceKey, BTreeMap<String, NodePolicy>>::new();

    // Provenance source identity is the merge key. Incoming edges replace an
    // existing edge with the same content-derived id before policy evaluation.
    let replaced_ids: BTreeSet<&str> = incoming.iter().map(|edge| edge.edge_id.as_str()).collect();
    for edge in existing
        .values()
        .filter(|edge| !replaced_ids.contains(edge.edge_id.as_str()))
        .chain(incoming.iter())
    {
        let Some(node_ref) = edge.node_ref.as_ref() else {
            continue;
        };
        let source_key = (edge.source.kind, edge.source.stable_id.clone());
        let node_policy = groups
            .entry(source_key)
            .or_default()
            .entry(node_ref.clone())
            .or_default();
        match edge.node_ref_split_key().map_err(anyhow::Error::msg)? {
            Some(split_key) => {
                node_policy.1.insert(split_key.to_owned());
            }
            None => node_policy.0 = true,
        }
    }

    for ((kind, stable_id), nodes) in groups {
        if nodes.len() < 2 {
            continue;
        }
        let mut split_to_node = BTreeMap::<String, String>::new();
        for (node_ref, (has_unsplit_edge, split_keys)) in nodes {
            if has_unsplit_edge || split_keys.len() != 1 {
                anyhow::bail!(
                    "source {kind:?}:{stable_id} maps to multiple node_ref values; every edge requires one explicit node_ref_split_key"
                );
            }
            let split_key = split_keys
                .into_iter()
                .next()
                .expect("checked one split key");
            if let Some(other_node) = split_to_node.insert(split_key.clone(), node_ref.clone())
                && other_node != node_ref
            {
                anyhow::bail!(
                    "source {kind:?}:{stable_id} reuses node_ref_split_key {split_key:?} for {other_node:?} and {node_ref:?}"
                );
            }
        }
    }
    Ok(())
}

impl WhyStore {
    pub fn open(graphzero_root: &Path) -> Result<Self> {
        let root = graphzero_root.join("why");
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    fn edges_path(&self) -> PathBuf {
        self.root.join("edges.jsonl")
    }

    fn cursors_path(&self) -> PathBuf {
        self.root.join("cursors.json")
    }

    fn manifest_path(&self) -> PathBuf {
        self.root.join("manifest.json")
    }

    fn acquire_ledger_lock(&self) -> Result<File> {
        let path = self.root.join("ledger.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("open why ledger lock {}", path.display()))?;
        file.lock().context("acquire why ledger lock")?;
        Ok(file)
    }

    pub fn graphzero_root(&self) -> PathBuf {
        self.root.parent().unwrap().to_path_buf()
    }

    pub fn load_ledger(&self) -> Result<WhyLedger> {
        let mut ledger = WhyLedger {
            schema_version: 1,
            ..Default::default()
        };
        ledger.edges = load_edges_from_jsonl(&self.edges_path())?;
        validate_node_ref_identity_policy(&ledger.edges, &[])?;
        ledger.cursors = load_cursors_from_path(&self.cursors_path())?;
        ledger.replay_digest = compute_replay_digest(&ledger.edges);
        Ok(ledger)
    }

    pub fn persist_ledger(&self, ledger: &WhyLedger) -> Result<()> {
        let _lock = self.acquire_ledger_lock()?;
        self.persist_ledger_unlocked(ledger)
    }

    fn persist_ledger_unlocked(&self, ledger: &WhyLedger) -> Result<()> {
        self.write_edges_jsonl_atomic(&ledger.edges)?;
        self.write_json_pretty_atomic(self.cursors_path(), &ledger.cursors)?;
        let manifest = build_query_manifest(ledger);
        self.write_json_pretty_atomic(self.manifest_path(), &manifest)?;
        Ok(())
    }

    fn write_edges_jsonl_atomic(&self, edges: &BTreeMap<String, WhyEdge>) -> Result<()> {
        let dest = self.edges_path();
        let tmp = dest.with_extension("tmp");
        let result = (|| -> Result<()> {
            let mut f = File::create(&tmp)?;
            for edge in edges.values() {
                let line = serde_json::to_string(edge)?;
                writeln!(f, "{line}")?;
            }
            f.sync_data()?;
            fs::rename(&tmp, &dest)?;
            Ok(())
        })();
        finish_atomic_write(result, &tmp)
    }

    /// Append-only path for edges that are all new keys (graphzero-2ajds).
    fn append_edges_jsonl(&self, new_edges: &[WhyEdge]) -> Result<()> {
        if new_edges.is_empty() {
            return Ok(());
        }
        let dest = self.edges_path();
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&dest)
            .with_context(|| format!("append why edges {}", dest.display()))?;
        for edge in new_edges {
            let line = serde_json::to_string(edge)?;
            writeln!(f, "{line}")?;
        }
        f.sync_data()?;
        Ok(())
    }

    fn write_json_pretty_atomic<T: Serialize>(&self, path: PathBuf, value: &T) -> Result<()> {
        let tmp = path.with_extension("tmp");
        let result = (|| -> Result<()> {
            let body = serde_json::to_string_pretty(value)?;
            let mut f = File::create(&tmp)?;
            f.write_all(body.as_bytes())?;
            f.sync_data()?;
            fs::rename(&tmp, &path)?;
            Ok(())
        })();
        finish_atomic_write(result, &tmp)
    }

    fn update_ledger<T>(&self, update: impl FnOnce(&mut WhyLedger) -> Result<T>) -> Result<T> {
        let _lock = self.acquire_ledger_lock()?;
        let mut ledger = self.load_ledger()?;
        let output = update(&mut ledger)?;
        ledger.replay_digest = compute_replay_digest(&ledger.edges);
        self.persist_ledger_unlocked(&ledger)?;
        Ok(output)
    }

    pub fn upsert_edges(
        &self,
        mut edges: Vec<WhyEdge>,
        repo_root: Option<&Path>,
    ) -> Result<Vec<WhyEdge>> {
        let gz_root = self.graphzero_root();
        for edge in &mut edges {
            edge.edge_id = compute_edge_id(edge);
            edge.validate_for_persist().map_err(anyhow::Error::msg)?;
            validate_evidence_refs(&gz_root, repo_root, &edge.evidence_refs)?;
        }
        self.update_ledger_edges(&edges, |ledger| {
            validate_node_ref_identity_policy(&ledger.edges, &edges)?;
            for edge in &edges {
                ledger.edges.insert(edge.edge_id.clone(), edge.clone());
            }
            Ok(())
        })?;
        Ok(edges)
    }

    /// Like `update_ledger`, but appends to edges.jsonl when every upserted edge id is new.
    fn update_ledger_edges<T>(
        &self,
        edges: &[WhyEdge],
        update: impl FnOnce(&mut WhyLedger) -> Result<T>,
    ) -> Result<T> {
        let _lock = self.acquire_ledger_lock()?;
        let mut ledger = self.load_ledger()?;
        let all_new = edges.iter().all(|e| !ledger.edges.contains_key(&e.edge_id));
        let output = update(&mut ledger)?;
        ledger.replay_digest = compute_replay_digest(&ledger.edges);
        if all_new {
            self.append_edges_jsonl(edges)?;
            self.write_json_pretty_atomic(self.cursors_path(), &ledger.cursors)?;
            let manifest = build_query_manifest(&ledger);
            self.write_json_pretty_atomic(self.manifest_path(), &manifest)?;
        } else {
            self.persist_ledger_unlocked(&ledger)?;
        }
        Ok(output)
    }

    pub fn upsert_edge(&self, edge: WhyEdge, repo_root: Option<&Path>) -> Result<WhyEdge> {
        self.upsert_edges(vec![edge], repo_root)?
            .pop()
            .context("single-edge upsert returned no edge")
    }

    pub fn upsert_cursor(&self, cursor: SourceCursor) -> Result<()> {
        let key = cursor_key(&cursor.source);
        self.update_ledger(|ledger| {
            if let Some(existing) = ledger.cursors.get(&key) {
                if existing.position == cursor.position {
                    if existing.digest == cursor.digest
                        && existing.last_event_id == cursor.last_event_id
                    {
                        return Ok(());
                    }
                    anyhow::bail!("cursor replay at same position changed digest or last event");
                }
                if cursor.position < existing.position {
                    anyhow::bail!("cursor position moved backwards");
                }
            }
            ledger.cursors.insert(key, cursor);
            Ok(())
        })
    }

    pub fn all_edges(&self) -> Result<Vec<WhyEdge>> {
        Ok(self.load_ledger()?.edges.into_values().collect())
    }

    pub fn why_chain_for_node(&self, node_ref: &str) -> Result<Vec<WhyChainEntry>> {
        Ok(build_why_chain_for_node(&self.load_ledger()?, node_ref))
    }
}

pub fn build_why_chain_for_node(ledger: &WhyLedger, node_ref: &str) -> Vec<WhyChainEntry> {
    let mut chain: Vec<_> = ledger
        .edges
        .values()
        .filter(|edge| edge.node_ref.as_deref() == Some(node_ref))
        .map(|edge| WhyChainEntry {
            edge_id: edge.edge_id.clone(),
            source: edge.source.clone(),
            relation: edge.relation,
            confidence: edge.confidence,
            source_freshness: edge.source_freshness.clone(),
            evidence_refs: edge.evidence_refs.clone(),
            redaction_state: edge.redaction_state,
        })
        .collect();
    chain.sort_by(|a, b| {
        // Unknown freshness sorts before any concrete timestamp.
        match (a.source_freshness.as_ref(), b.source_freshness.as_ref()) {
            (None, None) => a.edge_id.cmp(&b.edge_id),
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (Some(a_fresh), Some(b_fresh)) => match a_fresh.cmp(b_fresh) {
                Ordering::Equal => a.edge_id.cmp(&b.edge_id),
                ord => ord,
            },
        }
    });
    chain
}

fn load_edges_from_jsonl(path: &Path) -> Result<BTreeMap<String, WhyEdge>> {
    let mut edges = BTreeMap::new();
    if !path.exists() {
        return Ok(edges);
    }
    let f = fs::File::open(path)?;
    for line in BufReader::new(f).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let edge: WhyEdge = serde_json::from_str(&line)?;
        edge.validate_for_persist().map_err(anyhow::Error::msg)?;
        edges.insert(edge.edge_id.clone(), edge);
    }
    Ok(edges)
}

fn load_cursors_from_path(path: &Path) -> Result<BTreeMap<String, SourceCursor>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let data = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&data)?)
}

pub fn cursor_key(source: &crate::schema::ProvenanceSource) -> String {
    let tag = match source.kind {
        ProvenanceSourceKind::GitCommit => "git_commit",
        ProvenanceSourceKind::PrThread => "pr_thread",
        ProvenanceSourceKind::Issue => "issue",
        ProvenanceSourceKind::AgentTrace => "agent_trace",
    };
    format!("{tag}:{}", source.stable_id)
}

fn compute_replay_digest(edges: &BTreeMap<String, WhyEdge>) -> String {
    use sha2::{Digest, Sha256};
    let mut ids: Vec<_> = edges.keys().cloned().collect();
    ids.sort();
    let mut h = Sha256::new();
    for id in ids {
        h.update(id.as_bytes());
        h.update(b"\n");
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

pub fn build_query_manifest(ledger: &WhyLedger) -> WhyQueryManifest {
    let mut by_node: BTreeMap<String, usize> = BTreeMap::new();
    for e in ledger.edges.values() {
        if let Some(n) = &e.node_ref {
            *by_node.entry(n.clone()).or_default() += 1;
        }
    }
    WhyQueryManifest {
        schema_version: 1,
        edge_count: ledger.edges.len(),
        by_node: by_node.into_iter().collect(),
    }
}
