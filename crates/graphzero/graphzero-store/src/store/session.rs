//! P4.3 session providers: seen-set + navigation traces (ref-contract §5).
//!
//! Standalone [`LocalSeenProvider`] / [`LocalTraceProvider`] back byte-span
//! identity. Default snap ranking wraps them in [`EntityAwareSeenProvider`] so
//! session dedup also treats linked entities as known facts (bead `.3`).
//! Dedup counters classify byte vs entity hits for the `.4` ledger metric.
//! Optional TokenZero enrichment lives in `tokenzero_adapter`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use parking_lot::Mutex;
use serde::Deserialize;

use crate::ContentHash;

use super::csr::edge_kind;
use super::entity::{EntityId, EntityNovelty, entity_for_view, record_process_destination_hit};
use super::indexer::{EdgeRecord, IndexData};
use super::query::DestinationRef;
use super::refs::GzRef;

pub const TRACE_SCHEMA: &str = "graphzero-trace/v1";
pub const SOURCE_TRACE_INGEST: &str = "trace-ingest";

/// Default max destinations retained per seen-scope in the process ledger.
pub const DEFAULT_SESSION_SEEN_MAX: usize = 50_000;
/// Default max entity-novelty ids retained per scope.
pub const DEFAULT_SESSION_NOVELTY_MAX: usize = 50_000;
/// Default max trace events retained per session_id (ring: drop oldest).
pub const DEFAULT_SESSION_TRACE_EVENTS_MAX: usize = 10_000;
/// Default max distinct scopes / session_ids retained process-wide.
pub const DEFAULT_SESSION_SCOPES_MAX: usize = 256;

pub const SESSION_SEEN_MAX_ENV: &str = "GRAPHZERO_SESSION_SEEN_MAX";
pub const SESSION_NOVELTY_MAX_ENV: &str = "GRAPHZERO_SESSION_NOVELTY_MAX";
pub const SESSION_TRACE_EVENTS_MAX_ENV: &str = "GRAPHZERO_SESSION_TRACE_EVENTS_MAX";
pub const SESSION_SCOPES_MAX_ENV: &str = "GRAPHZERO_SESSION_SCOPES_MAX";

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default)
}

pub fn session_seen_max() -> usize {
    env_usize(SESSION_SEEN_MAX_ENV, DEFAULT_SESSION_SEEN_MAX)
}

pub fn session_novelty_max() -> usize {
    env_usize(SESSION_NOVELTY_MAX_ENV, DEFAULT_SESSION_NOVELTY_MAX)
}

pub fn session_trace_events_max() -> usize {
    env_usize(
        SESSION_TRACE_EVENTS_MAX_ENV,
        DEFAULT_SESSION_TRACE_EVENTS_MAX,
    )
}

pub fn session_scopes_max() -> usize {
    env_usize(SESSION_SCOPES_MAX_ENV, DEFAULT_SESSION_SCOPES_MAX)
}

fn cap_string_set(set: &mut BTreeSet<String>, max: usize) {
    if max == 0 {
        return;
    }
    while set.len() > max {
        let Some(first) = set.iter().next().cloned() else {
            break;
        };
        set.remove(&first);
    }
}

fn cap_scope_map<V>(map: &mut BTreeMap<String, V>, max: usize) {
    if max == 0 {
        return;
    }
    while map.len() > max {
        let Some(k) = map.keys().next().cloned() else {
            break;
        };
        map.remove(&k);
    }
}

fn cap_trace_ring(events: &mut Vec<TraceEvent>, max: usize) {
    if max == 0 || events.len() <= max {
        return;
    }
    let drop_n = events.len() - max;
    events.drain(0..drop_n);
}

/// Scope for seen-set lookups (PRD TokenZero adapter contract).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SeenScope {
    Session(String),
    Repo(String),
    Workspace(String),
    Global,
}

/// Result of a seen-set probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeenStatus {
    pub seen: bool,
    pub scope: SeenScope,
    pub source: &'static str,
}

/// Content hash + byte span key for batch seen lookup.
///
/// Span identity stays byte-addressed (`blob` + `#Bstart-end`). Entity novelty
/// is layered on top by [`EntityAwareSeenProvider`] via view→entity links; this
/// key is never rewritten to an entity id.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SeenKey {
    pub blob_sha256: String,
    pub start: u64,
    pub end: u64,
    pub scope: SeenScope,
}

/// Dedup counters surfaced in capsule diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionDedupStats {
    pub session_deduped: usize,
    pub repo_deduped: usize,
    pub workspace_deduped: usize,
    /// Same destination bytes already seen (byte-layer hit).
    pub byte_deduped: usize,
    /// Different costume, same entity already known (cross-view hit).
    pub entity_deduped: usize,
}

impl SessionDedupStats {
    pub fn total_removed(&self) -> usize {
        self.session_deduped + self.repo_deduped + self.workspace_deduped
    }
}

/// Ordered navigation event (query log / pulse export).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TraceEvent {
    pub tool: String,
    pub reference: String,
    #[serde(default)]
    pub tokens: u32,
    #[serde(default)]
    pub expanded: bool,
}

/// Abstract seen-set for ranking (ref-contract §5).
pub trait SeenProvider: Send + Sync {
    fn is_seen(&self, scope: &SeenScope, destination_ref: &str) -> bool;
    fn mark_seen(&self, scope: &SeenScope, destination_ref: &str);
    /// Batch lookup; default loops `is_seen`.
    fn batch_seen(&self, keys: &[SeenKey]) -> Vec<SeenStatus> {
        keys.iter()
            .map(|k| SeenStatus {
                seen: self.is_seen(&k.scope, &destination_key(&k.blob_sha256, k.start, k.end)),
                scope: k.scope.clone(),
                source: "local",
            })
            .collect()
    }
}

/// Abstract navigation trace feed (ref-contract §5).
pub trait TraceProvider: Send + Sync {
    fn record(&self, session_id: &str, event: TraceEvent);
    fn events(&self, session_id: &str) -> Vec<TraceEvent>;
}

fn destination_key(blob_sha256: &str, start: u64, end: u64) -> String {
    format!("gz://blob/{blob_sha256}#B{start}-{end}")
}

/// Resolve the entity behind a destination / view ref, if linked or direct.
fn resolve_entity_id(destination_ref: &str) -> Option<EntityId> {
    if destination_ref.is_empty() {
        return None;
    }
    if let Some(record) = entity_for_view(destination_ref) {
        return Some(record.id);
    }
    match GzRef::parse(destination_ref).ok()? {
        GzRef::Entity { id } => EntityId::parse(&id).ok(),
        _ => None,
    }
}

struct LedgerInner {
    seen_by_scope: BTreeMap<String, BTreeSet<String>>,
    /// Per-scope entity novelty (know-this-fact), composed with byte seen-sets.
    entity_novelty_by_scope: BTreeMap<String, EntityNovelty>,
    traces: BTreeMap<String, Vec<TraceEvent>>,
}

pub struct SessionLedger {
    inner: Mutex<LedgerInner>,
}

impl Default for SessionLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionLedger {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(LedgerInner {
                seen_by_scope: BTreeMap::new(),
                entity_novelty_by_scope: BTreeMap::new(),
                traces: BTreeMap::new(),
            }),
        }
    }

    fn scope_key(scope: &SeenScope) -> String {
        match scope {
            SeenScope::Session(s) => format!("session:{s}"),
            SeenScope::Repo(r) => format!("repo:{r}"),
            SeenScope::Workspace(w) => format!("workspace:{w}"),
            SeenScope::Global => "global".to_string(),
        }
    }

    fn entity_known(scope: &SeenScope, id: &EntityId) -> bool {
        let key = Self::scope_key(scope);
        let g = DEFAULT_LEDGER.inner.lock();
        g.entity_novelty_by_scope
            .get(&key)
            .is_some_and(|n| n.knows(id))
    }

    fn entity_mark_known(scope: &SeenScope, id: &EntityId) {
        let key = Self::scope_key(scope);
        let mut g = DEFAULT_LEDGER.inner.lock();
        g.entity_novelty_by_scope
            .entry(key.clone())
            .or_default()
            .mark_known(id);
        g.enforce_novelty_caps(&key);
        g.enforce_scope_caps();
    }

    /// Merge shared-store entity ids into the process novelty set for `scope`.
    pub fn merge_shared_entity_ids(
        scope: &SeenScope,
        ids: impl IntoIterator<Item = EntityId>,
    ) -> usize {
        let key = Self::scope_key(scope);
        let mut g = DEFAULT_LEDGER.inner.lock();
        let added = g
            .entity_novelty_by_scope
            .entry(key.clone())
            .or_default()
            .merge_ids(ids);
        g.enforce_novelty_caps(&key);
        g.enforce_scope_caps();
        added
    }

    /// Snapshot known entity ids for `scope` (for shared-store flush).
    pub fn known_entity_ids(scope: &SeenScope) -> Vec<EntityId> {
        let key = Self::scope_key(scope);
        let g = DEFAULT_LEDGER.inner.lock();
        g.entity_novelty_by_scope
            .get(&key)
            .map(|n| n.known_ids())
            .unwrap_or_default()
    }

    /// Process ledger sizes after caps (diagnostics / tests).
    pub fn process_stats() -> SessionLedgerStats {
        let g = DEFAULT_LEDGER.inner.lock();
        SessionLedgerStats {
            seen_scopes: g.seen_by_scope.len(),
            novelty_scopes: g.entity_novelty_by_scope.len(),
            trace_sessions: g.traces.len(),
            seen_destinations: g.seen_by_scope.values().map(|s| s.len()).sum(),
            novelty_entities: g.entity_novelty_by_scope.values().map(|n| n.len()).sum(),
            trace_events: g.traces.values().map(|v| v.len()).sum(),
        }
    }

    pub fn clear(&self) {
        let mut g = self.inner.lock();
        g.seen_by_scope.clear();
        g.entity_novelty_by_scope.clear();
        g.traces.clear();
    }
}

/// Bounded sizes of the process-global session ledger.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionLedgerStats {
    pub seen_scopes: usize,
    pub novelty_scopes: usize,
    pub trace_sessions: usize,
    pub seen_destinations: usize,
    pub novelty_entities: usize,
    pub trace_events: usize,
}

impl LedgerInner {
    fn enforce_seen_caps(&mut self, scope_key: &str) {
        let max = session_seen_max();
        if let Some(set) = self.seen_by_scope.get_mut(scope_key) {
            cap_string_set(set, max);
        }
    }

    fn enforce_novelty_caps(&mut self, scope_key: &str) {
        let max = session_novelty_max();
        if let Some(n) = self.entity_novelty_by_scope.get_mut(scope_key) {
            n.enforce_cap(max);
        }
    }

    fn enforce_trace_caps(&mut self, session_id: &str) {
        let max = session_trace_events_max();
        if let Some(events) = self.traces.get_mut(session_id) {
            cap_trace_ring(events, max);
        }
    }

    fn enforce_scope_caps(&mut self) {
        let max = session_scopes_max();
        cap_scope_map(&mut self.seen_by_scope, max);
        cap_scope_map(&mut self.entity_novelty_by_scope, max);
        cap_scope_map(&mut self.traces, max);
    }
}

static DEFAULT_LEDGER: LazyLock<SessionLedger> = LazyLock::new(SessionLedger::new);

/// GraphZero-served destination refs for this process (byte-span identity only).
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalSeenProvider;

impl SeenProvider for LocalSeenProvider {
    fn is_seen(&self, scope: &SeenScope, destination_ref: &str) -> bool {
        let key = SessionLedger::scope_key(scope);
        let g = DEFAULT_LEDGER.inner.lock();
        g.seen_by_scope
            .get(&key)
            .is_some_and(|s| s.contains(destination_ref))
    }

    fn mark_seen(&self, scope: &SeenScope, destination_ref: &str) {
        let key = SessionLedger::scope_key(scope);
        let mut g = DEFAULT_LEDGER.inner.lock();
        g.seen_by_scope
            .entry(key.clone())
            .or_default()
            .insert(destination_ref.to_string());
        g.enforce_seen_caps(&key);
        g.enforce_scope_caps();
    }
}

/// Layers [`EntityNovelty`] over a byte-level [`SeenProvider`].
///
/// Destination ranking / session dedup treat a destination as seen when either
/// the inner provider has seen these bytes **or** the linked entity is already
/// known in scope. [`SeenKey`] remains a byte span key.
#[derive(Clone, Copy, Debug, Default)]
pub struct EntityAwareSeenProvider<P = LocalSeenProvider> {
    pub inner: P,
}

impl<P> EntityAwareSeenProvider<P> {
    pub fn new(inner: P) -> Self {
        Self { inner }
    }
}

impl EntityAwareSeenProvider<LocalSeenProvider> {
    pub fn local() -> Self {
        Self::new(LocalSeenProvider)
    }
}

impl<P: SeenProvider> SeenProvider for EntityAwareSeenProvider<P> {
    fn is_seen(&self, scope: &SeenScope, destination_ref: &str) -> bool {
        if self.inner.is_seen(scope, destination_ref) {
            return true;
        }
        match resolve_entity_id(destination_ref) {
            Some(id) => SessionLedger::entity_known(scope, &id),
            None => false,
        }
    }

    fn mark_seen(&self, scope: &SeenScope, destination_ref: &str) {
        self.inner.mark_seen(scope, destination_ref);
        if let Some(id) = resolve_entity_id(destination_ref) {
            SessionLedger::entity_mark_known(scope, &id);
        }
    }

    fn batch_seen(&self, keys: &[SeenKey]) -> Vec<SeenStatus> {
        // Preserve byte SeenKey → blob span dest; entity novelty applies via is_seen.
        keys.iter()
            .map(|k| {
                let dest = destination_key(&k.blob_sha256, k.start, k.end);
                SeenStatus {
                    seen: self.is_seen(&k.scope, &dest),
                    scope: k.scope.clone(),
                    source: "entity-aware",
                }
            })
            .collect()
    }
}

/// In-process query / tool navigation log.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalTraceProvider;

impl TraceProvider for LocalTraceProvider {
    fn record(&self, session_id: &str, event: TraceEvent) {
        match GzRef::parse(&event.reference) {
            Ok(GzRef::Node { id }) => {
                let _ = crate::link_emitted_symbol_view(
                    crate::EntityViewKind::Trace,
                    &id,
                    &event.reference,
                );
            }
            Ok(_) => {
                let _ = crate::link_emitted_view(
                    crate::EntityViewKind::Trace,
                    &event.reference,
                    &[&event.reference],
                );
            }
            Err(_) => {}
        }
        let mut g = DEFAULT_LEDGER.inner.lock();
        g.traces
            .entry(session_id.to_string())
            .or_default()
            .push(event);
        g.enforce_trace_caps(session_id);
        g.enforce_scope_caps();
    }

    fn events(&self, session_id: &str) -> Vec<TraceEvent> {
        let g = DEFAULT_LEDGER.inner.lock();
        g.traces.get(session_id).cloned().unwrap_or_default()
    }
}

/// Clears process session ledger + process dedup ledger.
///
/// Production-safe reset for long-lived MCP/daemon sessions (also used by tests).
pub fn clear_session_state() {
    DEFAULT_LEDGER.clear();
    super::entity::clear_process_dedup_ledger();
}

/// Alias of [`clear_session_state`] for operator-facing docs.
#[inline]
pub fn reset_session_state() {
    clear_session_state();
}

pub fn default_seen_provider() -> EntityAwareSeenProvider<LocalSeenProvider> {
    EntityAwareSeenProvider::local()
}

pub fn default_trace_provider() -> LocalTraceProvider {
    LocalTraceProvider
}

/// Apply seen-set to capsule destinations; marks newly served refs in session scope.
///
/// Classifies removals into byte vs entity (cross-view) hits and records them on
/// the process [`super::entity::EntityDedupLedger`].
pub fn apply_seen_to_destinations(
    session_id: Option<&str>,
    seen: &dyn SeenProvider,
    destinations: &mut Vec<DestinationRef>,
) -> SessionDedupStats {
    destinations.sort_by(|a, b| a.destination_ref.cmp(&b.destination_ref));
    let Some(sid) = session_id.filter(|s| !s.is_empty()) else {
        return SessionDedupStats::default();
    };
    let scope = SeenScope::Session(sid.to_string());
    let mut stats = SessionDedupStats::default();
    const DEST_TOKENS: u32 = 100;
    destinations.retain(|d| {
        if seen.is_seen(&scope, &d.destination_ref) {
            stats.session_deduped += 1;
            let byte_hit = LocalSeenProvider.is_seen(&scope, &d.destination_ref);
            if byte_hit {
                stats.byte_deduped += 1;
                record_process_destination_hit(DEST_TOKENS, true, false, false);
            } else {
                stats.entity_deduped += 1;
                record_process_destination_hit(DEST_TOKENS, false, true, false);
            }
            false
        } else {
            record_process_destination_hit(DEST_TOKENS, false, false, true);
            seen.mark_seen(&scope, &d.destination_ref);
            true
        }
    });
    stats
}

#[derive(Debug, Deserialize)]
pub struct TraceLine {
    pub schema: String,
    #[serde(default)]
    pub session: String,
    pub events: Vec<TraceEvent>,
}

fn symbol_from_ref(reference: &str) -> Option<String> {
    match GzRef::parse(reference).ok()? {
        GzRef::Node { id } => Some(id),
        GzRef::Entity { id } => Some(format!("entity:{id}")),
        GzRef::Blob {
            hash,
            fragment: super::refs::Fragment::Bytes { start, end },
        } => Some(format!("blob:{hash}#B{start}-{end}")),
        _ => None,
    }
}

/// Parse one JSONL line; returns `None` for blank lines.
pub fn parse_trace_line(line: &str) -> Result<Option<TraceLine>> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let parsed: TraceLine = serde_json::from_str(trimmed).context("trace json")?;
    if parsed.schema != TRACE_SCHEMA {
        bail!(
            "unsupported trace schema {}, expected {TRACE_SCHEMA}",
            parsed.schema
        );
    }
    Ok(Some(parsed))
}

/// Append `session_followed` edges from a JSONL trace file into index data.
pub fn ingest_traces_into_index(
    data: &mut IndexData,
    path: &Path,
    blob_for_evidence: ContentHash,
) -> Result<usize> {
    let file = File::open(path).with_context(|| format!("open trace file {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut added = 0usize;
    for line in reader.lines() {
        let line = line?;
        let Some(doc) = parse_trace_line(&line)? else {
            continue;
        };
        let session = if doc.session.is_empty() {
            "default".to_string()
        } else {
            doc.session
        };
        let mut prev_symbol: Option<String> = None;
        for ev in &doc.events {
            let Some(dst) = symbol_from_ref(&ev.reference) else {
                prev_symbol = None;
                continue;
            };
            if let Some(src) = &prev_symbol {
                let payload = format!("{session}:{src}->{dst}");
                let end = payload.len().min(u32::MAX as usize) as u32;
                data.edges.push(EdgeRecord {
                    src: src.clone(),
                    dst: dst.clone(),
                    kind: edge_kind::SESSION_FOLLOWED,
                    confidence: 200,
                    blob: blob_for_evidence,
                    start: 0,
                    end: end.max(1),
                });
                added += 1;
            }
            prev_symbol = Some(dst);
        }
    }
    Ok(added)
}

fn trace_evidence_blob(data: &IndexData) -> Result<ContentHash> {
    data.blob_order
        .first()
        .copied()
        .or_else(|| data.blobs.keys().next().copied())
        .context("index has no blobs for trace evidence")
}

fn wal_segment_ids(store_root: &Path) -> Result<Vec<u64>> {
    let wal_dir = store_root.join("wal");
    if wal_dir.is_dir() {
        super::delta_log::DeltaLog::segment_ids(&wal_dir)
    } else {
        Ok(Vec::new())
    }
}

fn publish_index_snapshot(
    store_root: &Path,
    data: &IndexData,
    manifest: &mut super::manifest::Manifest,
    segment_ids: &[u64],
) -> Result<()> {
    let snapshot_id = manifest.latest().map_or(1, |s| s.snapshot_id + 1);
    let written =
        super::indexer::write_snapshot(store_root, data, snapshot_id, segment_ids.to_vec())?;
    manifest.snapshots.push(written.entry.clone());
    manifest.snapshots.sort_by_key(|s| s.snapshot_id);
    while manifest.snapshots.len() > 2 {
        manifest.snapshots.remove(0);
    }
    manifest.atomic_publish(store_root)?;
    super::indexer::cleanup(store_root, manifest, segment_ids)?;
    Ok(())
}

/// Ingest traces and re-index the repo (publishes new snapshot).
pub fn ingest_traces_and_reindex(
    repo_root: &Path,
    store_root: &Path,
    trace_path: &Path,
) -> Result<usize> {
    let mut data = super::indexer::collect(repo_root, store_root)?;
    let evidence_blob = trace_evidence_blob(&data)?;
    let added = ingest_traces_into_index(&mut data, trace_path, evidence_blob)?;
    let _lock = super::lock::WriterLock::acquire(store_root).context("writer lock")?;
    let mut manifest = super::manifest::Manifest::load(store_root)?;
    let segment_ids = wal_segment_ids(store_root)?;
    publish_index_snapshot(store_root, &data, &mut manifest, &segment_ids)?;
    Ok(added)
}
