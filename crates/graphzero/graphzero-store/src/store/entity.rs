//! Entity-addressed knowledge: dedup facts, not bytes. Content hashing dedups identical bytes.
//! callers meet the same knowledge as a read capsule, grep hit, diff hunk, stack frame, or blast
//! node -- five costumes, one fact.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::ContentHash;

/// Schema version embedded in the canonical entity key preimage.
pub const ENTITY_KEY_VERSION: u8 = 1;

/// Repeat encounters bill at most this percent of first-encounter tokens.
pub const REPEAT_ENCOUNTER_PCT: u32 = 10;

/// Local operational dedup ledger schema (never shareable telemetry).
pub const DEDUP_LEDGER_SCHEMA: &str = "graphzero.dedup_ledger";

/// Relative path under a GraphZero store root for the dedup ledger.
pub const DEDUP_LEDGER_REL: &str = "telemetry/dedup_ledger.json";

/// Full lowercase 64-hex SHA-256 identity of a knowledge fact.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EntityId(pub String);

impl EntityId {
    /// Parse a full lowercase 64-hex entity id.
    pub fn parse(hex: &str) -> Result<Self> {
        validate_entity_id_hex(hex)?;
        Ok(Self(hex.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Canonical bare ref: `entity/<64-hex>`.
    pub fn to_ref(&self) -> String {
        format!("entity/{}", self.0)
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Kind of knowledge fact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Symbol,
}

impl EntityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Symbol => "symbol",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "symbol" => Ok(Self::Symbol),
            other => bail!("unknown entity kind '{other}'"),
        }
    }
}

/// Canonical key material that hashes to an [`EntityId`]. Identity is
/// knowledge, not presentation: the same symbol + defining content digest
/// yields the same entity whether the caller met it via read, grep, diff, or trace.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntityKey {
    pub kind: EntityKind,
    /// Qualified symbol name (GraphZero node / symbol-table spelling).
    pub symbol: String,
    /// Full lowercase 64-hex digest of the defining content (blob or span).
    pub content_digest: String,
}

impl EntityKey {
    pub fn new(kind: EntityKind, symbol: impl Into<String>, content_digest: &str) -> Result<Self> {
        let symbol = symbol.into();
        if symbol.is_empty() {
            bail!("entity key symbol must be non-empty");
        }
        validate_entity_id_hex(content_digest)?;
        Ok(Self {
            kind,
            symbol,
            content_digest: content_digest.to_ascii_lowercase(),
        })
    }

    /// Deterministic preimage: `graphzero.entity\0{kind}\0{symbol}\0{content_digest}`.
    pub fn preimage(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            16 + self.kind.as_str().len() + self.symbol.len() + self.content_digest.len(),
        );
        out.push(b'v');
        out.push(b'0' + ENTITY_KEY_VERSION);
        out.push(0);
        out.extend_from_slice(self.kind.as_str().as_bytes());
        out.push(0);
        out.extend_from_slice(self.symbol.as_bytes());
        out.push(0);
        out.extend_from_slice(self.content_digest.as_bytes());
        out
    }

    pub fn entity_id(&self) -> EntityId {
        EntityId(ContentHash::of(&self.preimage()).to_hex())
    }
}

/// How the caller encountered the fact (presentation costume).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityViewKind {
    Read,
    Grep,
    Diff,
    Trace,
    Blast,
    Node,
}

impl EntityViewKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Grep => "grep",
            Self::Diff => "diff",
            Self::Trace => "trace",
            Self::Blast => "blast",
            Self::Node => "node",
        }
    }
}

/// One byte-level (or graph) view linked to an entity.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntityView {
    pub kind: EntityViewKind,
    /// Canonical view address, such as `z://blob/...` or `node/...`.
    pub view_ref: String,
}

/// Durable-enough in-memory record for one knowledge fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityRecord {
    pub id: EntityId,
    pub key: EntityKey,
    pub views: Vec<EntityView>,
}

impl EntityRecord {
    pub fn new(key: EntityKey) -> Self {
        Self {
            id: key.entity_id(),
            key,
            views: Vec::new(),
        }
    }

    pub fn link_view(&mut self, kind: EntityViewKind, view_ref: impl Into<String>) {
        let view = EntityView {
            kind,
            view_ref: view_ref.into(),
        };
        if !self.views.contains(&view) {
            self.views.push(view);
        }
    }
}

/// Default max entities retained in the process-local [`DEFAULT_ENTITY_REGISTRY`]. Override at
/// runtime with `GRAPHZERO_ENTITY_REGISTRY_MAX` (positive usize).
pub const DEFAULT_ENTITY_REGISTRY_MAX: usize = 100_000;

/// Env var for process entity-registry soft cap (positive usize).
pub const ENTITY_REGISTRY_MAX_ENV: &str = "GRAPHZERO_ENTITY_REGISTRY_MAX";

/// Resolved process registry entity cap (env override or [`DEFAULT_ENTITY_REGISTRY_MAX`]).
pub fn entity_registry_max() -> usize {
    std::env::var(ENTITY_REGISTRY_MAX_ENV)
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_ENTITY_REGISTRY_MAX)
}

/// Process-local registry: entity id ↔ views.
#[derive(Default)]
pub struct EntityRegistry {
    by_id: BTreeMap<String, EntityRecord>,
    /// view_ref → entity id hex
    by_view: BTreeMap<String, String>,
}

impl EntityRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(&mut self, key: EntityKey) -> &mut EntityRecord {
        let id = key.entity_id();
        self.by_id
            .entry(id.0.clone())
            .or_insert_with(|| EntityRecord::new(key))
    }

    pub fn link(
        &mut self,
        key: EntityKey,
        kind: EntityViewKind,
        view_ref: impl Into<String>,
    ) -> EntityId {
        let view_ref = view_ref.into();
        let id = {
            let record = self.upsert(key);
            record.link_view(kind, view_ref.clone());
            record.id.clone()
        };
        self.by_view.insert(view_ref, id.0.clone());
        id
    }

    pub fn get(&self, id: &EntityId) -> Option<&EntityRecord> {
        self.by_id.get(id.as_str())
    }

    pub fn entity_for_view(&self, view_ref: &str) -> Option<&EntityRecord> {
        let id = self.by_view.get(view_ref)?;
        self.by_id.get(id)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn view_len(&self) -> usize {
        self.by_view.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Stable snapshot of all records (sorted by entity id).
    pub fn records(&self) -> Vec<EntityRecord> {
        self.by_id.values().cloned().collect()
    }

    /// Merge an existing record (idempotent view union).
    pub fn merge_record(&mut self, record: &EntityRecord) {
        let slot = self
            .by_id
            .entry(record.id.0.clone())
            .or_insert_with(|| EntityRecord::new(record.key.clone()));
        for view in &record.views {
            slot.link_view(view.kind, view.view_ref.clone());
            self.by_view
                .insert(view.view_ref.clone(), record.id.0.clone());
        }
    }

    /// Drop lowest entity-id entries until `by_id.len <= max_entities`. Soft
    /// cap: preferred over fail-loud so multi-repo hydrate cannot abort
    /// queries; callers that need fail-loud can check [`Self::len`] after merge.
    pub fn enforce_cap(&mut self, max_entities: usize) {
        if max_entities == 0 || self.by_id.len() <= max_entities {
            return;
        }
        while self.by_id.len() > max_entities {
            let Some(id) = self.by_id.keys().next().cloned() else {
                break;
            };
            if let Some(rec) = self.by_id.remove(&id) {
                for view in &rec.views {
                    self.by_view.remove(&view.view_ref);
                }
            }
        }
        self.by_view.retain(|_, eid| self.by_id.contains_key(eid));
    }
}

struct RegistryInner {
    registry: EntityRegistry,
    /// Exact sidecar artifacts already merged into `registry`.
    hydrated_sidecars: BTreeSet<(PathBuf, ContentHash)>,
}

static DEFAULT_ENTITY_REGISTRY: LazyLock<Mutex<RegistryInner>> = LazyLock::new(|| {
    Mutex::new(RegistryInner {
        registry: EntityRegistry::new(),
        hydrated_sidecars: BTreeSet::new(),
    })
});

fn enforce_process_entity_cap(g: &mut RegistryInner) -> bool {
    let before = g.registry.len();
    g.registry.enforce_cap(entity_registry_max());
    let evicted = g.registry.len() < before;
    if evicted {
        // An evicted entity may belong to any hydrated sidecar. Forget every
        // marker so a later open can restore the exact published records.
        g.hydrated_sidecars.clear();
    }
    evicted
}

/// Clear the process-local entity registry and hydration markers.
pub fn clear_entity_registry() {
    let mut g = DEFAULT_ENTITY_REGISTRY.lock();
    g.registry = EntityRegistry::new();
    g.hydrated_sidecars.clear();
}

/// Hydrate the process registry from the latest published sidecar on first use.
pub fn entity_registry_hydrate(store_root: &Path) {
    {
        let g = DEFAULT_ENTITY_REGISTRY.lock();
        if !g.registry.is_empty() || !g.hydrated_sidecars.is_empty() {
            return; // already hydrated (or deliberately cleared): nothing to do.
        }
    }
    let Ok(manifest) = super::manifest::Manifest::load(store_root) else {
        return;
    };
    let Some(latest) = manifest.latest() else {
        return;
    };
    let _ = hydrate_published_entities(&store_root.join("shards"), latest.snapshot_id);
}

/// Current process registry entity count (after caps).
pub fn entity_registry_len() -> usize {
    DEFAULT_ENTITY_REGISTRY.lock().registry.len()
}

/// Link a view to an entity in the process-local registry.
pub fn link_view(key: EntityKey, kind: EntityViewKind, view_ref: impl Into<String>) -> EntityId {
    let mut g = DEFAULT_ENTITY_REGISTRY.lock();
    let id = g.registry.link(key, kind, view_ref);
    enforce_process_entity_cap(&mut g);
    id
}

/// Link `view_ref` as `kind` onto the entity already known via `anchor_ref`.
/// Returns `None` when the anchor is not registered (no mint / no hydrate yet).
pub fn link_view_from_anchor(
    anchor_ref: &str,
    kind: EntityViewKind,
    view_ref: impl Into<String>,
) -> Option<EntityId> {
    link_view_from_anchor_with_root(std::env::temp_dir().as_path(), anchor_ref, kind, view_ref)
}

/// Store-root-aware variant of [`link_view_from_anchor`]: hydrates the
/// registry from `store_root`'s latest sidecar on first use, preserving
/// pre-lazy linking semantics for callers that know their store.
pub fn link_view_from_anchor_with_root(
    store_root: &Path,
    anchor_ref: &str,
    kind: EntityViewKind,
    view_ref: impl Into<String>,
) -> Option<EntityId> {
    if anchor_ref.is_empty() {
        return None;
    }
    let Some(record) = entity_for_view(anchor_ref) else {
        // First-use hydration: published sidecar may hold this anchor.
        entity_registry_hydrate(store_root);
        let record = entity_for_view(anchor_ref)?;
        return Some(link_view(record.key, kind, view_ref));
    };
    Some(link_view(record.key, kind, view_ref))
}

/// Link an emitted costume to a known entity via any of `anchors` (node,
/// evidence, …). Tries each non-empty anchor, then `view_ref` itself. Used by
/// read/grep/diff/trace/blast emission so every costume points at the same [`EntityId`].
pub fn link_emitted_view(
    kind: EntityViewKind,
    view_ref: &str,
    anchors: &[&str],
) -> Option<EntityId> {
    if view_ref.is_empty() {
        return None;
    }
    for anchor in anchors {
        if let Some(id) = link_view_from_anchor(anchor, kind, view_ref) {
            return Some(id);
        }
    }
    link_view_from_anchor(view_ref, kind, view_ref)
}

/// Link a symbol view through its canonical `node/<symbol>` ref.
pub fn link_emitted_symbol_view(
    kind: EntityViewKind,
    symbol: &str,
    view_ref: &str,
) -> Option<EntityId> {
    if symbol.is_empty() {
        return link_emitted_view(kind, view_ref, &[]);
    }
    let node = format!("node/{symbol}");
    link_emitted_view(kind, view_ref, &[node.as_str(), view_ref])
}

/// Look up an entity record by id from the process-local registry.
pub fn lookup_entity(id: &EntityId) -> Option<EntityRecord> {
    let g = DEFAULT_ENTITY_REGISTRY.lock();
    g.registry.get(id).cloned()
}

/// Look up the entity behind a byte-level / node view ref.
pub fn entity_for_view(view_ref: &str) -> Option<EntityRecord> {
    let g = DEFAULT_ENTITY_REGISTRY.lock();
    g.registry.entity_for_view(view_ref).cloned()
}

/// Look up the entity behind a view ref, hydrating from the latest published
/// sidecar on first use (`store_root` locates the sidecar). Preserves the
/// pre-lazy semantics for callers that hold a store root.
pub fn entity_for_view_with_store(store_root: &Path, view_ref: &str) -> Option<EntityRecord> {
    if entity_for_view(view_ref).is_none() {
        entity_registry_hydrate(store_root);
    }
    entity_for_view(view_ref)
}

/// Schema id for publish-time entity sidecars.
pub const ENTITY_SIDECAR_SCHEMA: &str = "graphzero.entities";

/// One symbol span ready for index-time entity minting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolSpanMint {
    pub symbol: String,
    /// Full lowercase 64-hex digest of defining content bytes.
    pub content_digest: String,
    /// Canonical `node/<symbol>` address.
    pub node_ref: String,
    /// `z://blob/<hash>#B<start>-<end>` name/evidence span.
    pub blob_span_ref: String,
}

/// Digest of defining content bytes (block or name span). Empty ⇒ `None`.
pub fn defining_content_digest(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    Some(ContentHash::of(bytes).to_hex())
}

/// Slice `content[start..end]` when in range; otherwise `None`.
pub fn slice_defining_bytes(content: &[u8], start: u32, end: u32) -> Option<&[u8]> {
    let start = start as usize;
    let end = end as usize;
    if end < start || end > content.len() {
        return None;
    }
    Some(&content[start..end])
}

/// Mint a symbol entity and link node + blob-span evidence views.
/// Same symbol + defining digest ⇒ same [`EntityId`] across encounters.
pub fn mint_symbol_span_entity(span: &SymbolSpanMint) -> Result<EntityId> {
    let key = EntityKey::new(
        EntityKind::Symbol,
        span.symbol.as_str(),
        &span.content_digest,
    )?;
    let mut g = DEFAULT_ENTITY_REGISTRY.lock();
    let id = g
        .registry
        .link(key.clone(), EntityViewKind::Node, span.node_ref.clone());
    g.registry
        .link(key, EntityViewKind::Read, span.blob_span_ref.clone());
    enforce_process_entity_cap(&mut g);
    Ok(id)
}

/// Mint many symbol spans into a fresh registry (for sidecar serialization).
pub fn mint_symbol_spans(spans: &[SymbolSpanMint]) -> Result<(EntityRegistry, Vec<EntityId>)> {
    let mut registry = EntityRegistry::new();
    let mut ids = Vec::with_capacity(spans.len());
    for span in spans {
        let key = EntityKey::new(
            EntityKind::Symbol,
            span.symbol.as_str(),
            &span.content_digest,
        )?;
        let id = registry.link(key.clone(), EntityViewKind::Node, span.node_ref.clone());
        registry.link(key, EntityViewKind::Read, span.blob_span_ref.clone());
        ids.push(id);
    }
    Ok((registry, ids))
}

/// Merge records into the process-local registry (idempotent view union). After merge,
/// enforces [`entity_registry_max`] so multi-repo hydrate cannot grow process RSS without bound.
pub fn register_entity_records(records: &[EntityRecord]) {
    let mut g = DEFAULT_ENTITY_REGISTRY.lock();
    for record in records {
        g.registry.merge_record(record);
    }
    enforce_process_entity_cap(&mut g);
}

/// On-disk publish-time entity index (sidecar next to shards).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedEntityIndex {
    pub schema: String,
    pub snapshot_id: u64,
    pub entities: Vec<EntityRecord>,
}

impl PublishedEntityIndex {
    pub fn from_registry(snapshot_id: u64, registry: &EntityRegistry) -> Self {
        Self {
            schema: ENTITY_SIDECAR_SCHEMA.to_string(),
            snapshot_id,
            entities: registry.records(),
        }
    }
}

/// Sidecar file name: `entities_{snapshot_id:08}.json`.
pub fn entities_file_name(snapshot_id: u64) -> String {
    format!("entities_{snapshot_id:08}.json")
}

pub fn entities_sidecar_path(shards_dir: &Path, snapshot_id: u64) -> PathBuf {
    shards_dir.join(entities_file_name(snapshot_id))
}

/// Persist minted entities next to shards (JSON, pretty for expand parity).
pub fn write_published_entities(
    shards_dir: &Path,
    snapshot_id: u64,
    index: &PublishedEntityIndex,
) -> Result<PathBuf> {
    if index.schema != ENTITY_SIDECAR_SCHEMA {
        bail!("unsupported entity sidecar schema {}", index.schema);
    }
    fs::create_dir_all(shards_dir)
        .with_context(|| format!("create shards dir {}", shards_dir.display()))?;
    let path = entities_sidecar_path(shards_dir, snapshot_id);
    let bytes = serde_json::to_vec_pretty(index).context("serialize entity sidecar")?;
    fs::write(&path, bytes).with_context(|| format!("write entity sidecar {}", path.display()))?;
    Ok(path)
}

/// Read and identity-bind a published sidecar if present.
fn load_published_entities_with_identity(
    shards_dir: &Path,
    snapshot_id: u64,
) -> Result<Option<(PublishedEntityIndex, (PathBuf, ContentHash))>> {
    let path = entities_sidecar_path(shards_dir, snapshot_id);
    if !path.is_file() {
        return Ok(None);
    }
    let bytes =
        fs::read(&path).with_context(|| format!("read entity sidecar {}", path.display()))?;
    let index: PublishedEntityIndex = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse entity sidecar {}", path.display()))?;
    if index.schema != ENTITY_SIDECAR_SCHEMA {
        bail!(
            "unsupported entity sidecar schema {} at {}",
            index.schema,
            path.display()
        );
    }
    let identity_path = fs::canonicalize(&path).unwrap_or(path);
    Ok(Some((index, (identity_path, ContentHash::of(&bytes)))))
}

/// Load published entity sidecar if present (`Ok(None)` for legacy snapshots).
pub fn try_load_published_entities(
    shards_dir: &Path,
    snapshot_id: u64,
) -> Result<Option<PublishedEntityIndex>> {
    Ok(
        load_published_entities_with_identity(shards_dir, snapshot_id)?
            .map(|(index, _identity)| index),
    )
}

/// Hydrate process registry from a published sidecar (no-op when missing or
/// when the exact sidecar bytes were already merged without later eviction).
pub fn hydrate_published_entities(shards_dir: &Path, snapshot_id: u64) -> Result<usize> {
    let Some((index, identity)) = load_published_entities_with_identity(shards_dir, snapshot_id)?
    else {
        return Ok(0);
    };
    let mut g = DEFAULT_ENTITY_REGISTRY.lock();
    if g.hydrated_sidecars.contains(&identity) {
        return Ok(0);
    }
    let n = index.entities.len();
    for record in &index.entities {
        g.registry.merge_record(record);
    }
    if !enforce_process_entity_cap(&mut g) {
        g.hydrated_sidecars.insert(identity);
    }
    Ok(n)
}

/// Resolve entity from process registry, else hydrate from store sidecar once.
pub fn lookup_entity_with_store(store_root: &Path, id: &EntityId) -> Result<Option<EntityRecord>> {
    if let Some(record) = lookup_entity(id) {
        return Ok(Some(record));
    }
    let manifest = super::manifest::Manifest::load(store_root)?;
    let Some(latest) = manifest.latest() else {
        return Ok(None);
    };
    let shards = store_root.join("shards");
    hydrate_published_entities(&shards, latest.snapshot_id)?;
    Ok(lookup_entity(id))
}

/// Novelty records whether this entity's facts have already been charged.
#[derive(Clone, Debug, Default)]
pub struct EntityNovelty {
    /// entity id hex → first-encounter view kind (None when marked without a view).
    known: BTreeMap<String, Option<EntityViewKind>>,
}

/// Billing outcome for one entity encounter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncounterCost {
    pub entity_id: EntityId,
    pub first_tokens: u32,
    pub billed_tokens: u32,
    pub known_before: bool,
    /// True when this repeat used a different [`EntityViewKind`] than the first.
    pub cross_view: bool,
}

impl EntityNovelty {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.known.len()
    }

    pub fn is_empty(&self) -> bool {
        self.known.is_empty()
    }

    /// Drop lowest entity-id keys until `len() <= max`.
    pub fn enforce_cap(&mut self, max: usize) {
        if max == 0 || self.known.len() <= max {
            return;
        }
        while self.known.len() > max {
            let Some(k) = self.known.keys().next().cloned() else {
                break;
            };
            self.known.remove(&k);
        }
    }

    pub fn knows(&self, id: &EntityId) -> bool {
        self.known.contains_key(id.as_str())
    }

    /// First encounter bills `first_tokens`; repeats bill
    /// `ceil(first_tokens * REPEAT_ENCOUNTER_PCT / 100)` (at most 10%).
    pub fn bill(&mut self, id: &EntityId, first_tokens: u32) -> EncounterCost {
        self.bill_with_view(id, first_tokens, None)
    }

    /// Like [`Self::bill`], tracking whether a repeat crossed view costumes.
    pub fn bill_with_view(
        &mut self,
        id: &EntityId,
        first_tokens: u32,
        view: Option<EntityViewKind>,
    ) -> EncounterCost {
        let prior = self.known.get(id.as_str()).copied();
        let known_before = prior.is_some();
        let cross_view = match (prior.flatten(), view) {
            (Some(first), Some(now)) => first != now,
            _ => false,
        };
        let billed_tokens = if known_before {
            repeat_bill(first_tokens)
        } else {
            self.known.insert(id.0.clone(), view);
            first_tokens
        };
        EncounterCost {
            entity_id: id.clone(),
            first_tokens,
            billed_tokens,
            known_before,
            cross_view,
        }
    }

    pub fn mark_known(&mut self, id: &EntityId) {
        self.known.entry(id.0.clone()).or_insert(None);
    }

    /// Sorted unique entity id hexes currently known.
    pub fn known_ids(&self) -> Vec<EntityId> {
        self.known.keys().cloned().map(EntityId).collect()
    }

    /// Union foreign known ids (no view costume) into this novelty set.
    pub fn merge_ids(&mut self, ids: impl IntoIterator<Item = EntityId>) -> usize {
        let mut added = 0;
        for id in ids {
            if let std::collections::btree_map::Entry::Vacant(e) = self.known.entry(id.0) {
                e.insert(None);
                added += 1;
            }
        }
        added
    }

    pub fn clear(&mut self) {
        self.known.clear();
    }
}

/// `ceil(first * REPEAT_ENCOUNTER_PCT / 100)`, saturating at `first`.
pub fn repeat_bill(first_tokens: u32) -> u32 {
    if first_tokens == 0 {
        return 0;
    }
    let num = u64::from(first_tokens) * u64::from(REPEAT_ENCOUNTER_PCT);
    let billed = num.div_ceil(100) as u32;
    billed.min(first_tokens)
}

/// Cross-view entity dedup ledger, surfaced next to byte dedup. Tracks
/// naive token mass, mass after identical-byte dedup, and mass after
/// entity novelty (`REPEAT_ENCOUNTER_PCT`). Rates are integer percents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityDedupLedger {
    pub schema: String,
    /// Token mass if every encounter billed full (no dedup of any kind).
    pub naive_tokens: u64,
    /// Token mass after identical-byte / same-destination removal.
    pub after_byte_dedup_tokens: u64,
    /// Token mass after entity novelty billing (≤10% repeats).
    pub after_entity_dedup_tokens: u64,
    pub byte_first_encounters: u64,
    pub byte_repeat_encounters: u64,
    pub entity_first_encounters: u64,
    pub entity_repeat_encounters: u64,
    /// Repeats where the view costume differed from the first encounter.
    pub entity_cross_view_repeats: u64,
    /// Max observed `ceil(billed*100/first)` over repeats (gate: ≤ [`REPEAT_ENCOUNTER_PCT`]).
    pub max_repeat_encounter_pct: u32,
}

impl Default for EntityDedupLedger {
    fn default() -> Self {
        Self::empty()
    }
}

impl EntityDedupLedger {
    pub fn empty() -> Self {
        Self {
            schema: DEDUP_LEDGER_SCHEMA.to_string(),
            naive_tokens: 0,
            after_byte_dedup_tokens: 0,
            after_entity_dedup_tokens: 0,
            byte_first_encounters: 0,
            byte_repeat_encounters: 0,
            entity_first_encounters: 0,
            entity_repeat_encounters: 0,
            entity_cross_view_repeats: 0,
            max_repeat_encounter_pct: 0,
        }
    }

    /// Record an encounter. `byte_repeat` means the same destination bytes were
    /// already seen (full save at the byte layer). Otherwise entity novelty
    /// bills via `cost`.
    pub fn record_encounter(&mut self, cost: &EncounterCost, byte_repeat: bool) {
        let naive = u64::from(cost.first_tokens);
        self.naive_tokens = self.naive_tokens.saturating_add(naive);
        if byte_repeat {
            self.byte_repeat_encounters = self.byte_repeat_encounters.saturating_add(1);
            return;
        }
        self.byte_first_encounters = self.byte_first_encounters.saturating_add(1);
        self.after_byte_dedup_tokens = self.after_byte_dedup_tokens.saturating_add(naive);
        if cost.known_before {
            self.entity_repeat_encounters = self.entity_repeat_encounters.saturating_add(1);
            if cost.cross_view {
                self.entity_cross_view_repeats = self.entity_cross_view_repeats.saturating_add(1);
            }
            let pct = repeat_encounter_pct(cost.first_tokens, cost.billed_tokens);
            self.max_repeat_encounter_pct = self.max_repeat_encounter_pct.max(pct);
            self.after_entity_dedup_tokens = self
                .after_entity_dedup_tokens
                .saturating_add(u64::from(cost.billed_tokens));
        } else {
            self.entity_first_encounters = self.entity_first_encounters.saturating_add(1);
            self.after_entity_dedup_tokens = self
                .after_entity_dedup_tokens
                .saturating_add(u64::from(cost.billed_tokens));
        }
    }

    /// Record a destination-level hit without a full novelty bill (session apply). `byte_repeat`:
    /// identical destination already seen. `entity_cross_view`: different costume, same fact
    /// already known. `first`: newly served destination (counts toward naive + after_byte + after_entity).
    pub fn record_destination_hit(
        &mut self,
        tokens: u32,
        byte_repeat: bool,
        entity_cross_view: bool,
        first: bool,
    ) {
        let naive = u64::from(tokens);
        self.naive_tokens = self.naive_tokens.saturating_add(naive);
        if byte_repeat {
            self.byte_repeat_encounters = self.byte_repeat_encounters.saturating_add(1);
            return;
        }
        self.byte_first_encounters = self.byte_first_encounters.saturating_add(1);
        self.after_byte_dedup_tokens = self.after_byte_dedup_tokens.saturating_add(naive);
        if first {
            self.entity_first_encounters = self.entity_first_encounters.saturating_add(1);
            self.after_entity_dedup_tokens = self.after_entity_dedup_tokens.saturating_add(naive);
            return;
        }
        if entity_cross_view {
            self.entity_repeat_encounters = self.entity_repeat_encounters.saturating_add(1);
            self.entity_cross_view_repeats = self.entity_cross_view_repeats.saturating_add(1);
            let billed = repeat_bill(tokens);
            let pct = repeat_encounter_pct(tokens, billed);
            self.max_repeat_encounter_pct = self.max_repeat_encounter_pct.max(pct);
            self.after_entity_dedup_tokens = self
                .after_entity_dedup_tokens
                .saturating_add(u64::from(billed));
        } else {
            // Non-cross-view entity repeat (same costume re-ranked) — still ≤10%.
            self.entity_repeat_encounters = self.entity_repeat_encounters.saturating_add(1);
            let billed = repeat_bill(tokens);
            let pct = repeat_encounter_pct(tokens, billed);
            self.max_repeat_encounter_pct = self.max_repeat_encounter_pct.max(pct);
            self.after_entity_dedup_tokens = self
                .after_entity_dedup_tokens
                .saturating_add(u64::from(billed));
        }
    }

    /// Fraction of naive tokens saved by identical-byte dedup (0–100).
    pub fn byte_dedup_rate_pct(&self) -> u32 {
        rate_pct(self.naive_tokens, self.after_byte_dedup_tokens)
    }

    /// Fraction of post-byte tokens saved by entity novelty (0–100).
    pub fn entity_cross_view_dedup_rate_pct(&self) -> u32 {
        rate_pct(self.after_byte_dedup_tokens, self.after_entity_dedup_tokens)
    }

    /// `true` when every recorded repeat billed at ≤ [`REPEAT_ENCOUNTER_PCT`].
    pub fn repeat_encounter_gate_ok(&self) -> bool {
        self.max_repeat_encounter_pct <= REPEAT_ENCOUNTER_PCT
    }

    /// Fail if any repeat exceeded [`REPEAT_ENCOUNTER_PCT`].
    pub fn assert_repeat_encounter_gate(&self) {
        assert!(
            self.repeat_encounter_gate_ok(),
            "REPEAT_ENCOUNTER_PCT gate failed: max_repeat_encounter_pct={} > {}",
            self.max_repeat_encounter_pct,
            REPEAT_ENCOUNTER_PCT
        );
    }

    /// Compact JSON object for capsule/telemetry ledger surfaces.
    pub fn to_ledger_json(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": self.schema,
            "naive_tokens": self.naive_tokens,
            "after_byte_dedup_tokens": self.after_byte_dedup_tokens,
            "after_entity_dedup_tokens": self.after_entity_dedup_tokens,
            "byte_dedup_rate_pct": self.byte_dedup_rate_pct(),
            "entity_cross_view_dedup_rate_pct": self.entity_cross_view_dedup_rate_pct(),
            "byte_first_encounters": self.byte_first_encounters,
            "byte_repeat_encounters": self.byte_repeat_encounters,
            "entity_first_encounters": self.entity_first_encounters,
            "entity_repeat_encounters": self.entity_repeat_encounters,
            "entity_cross_view_repeats": self.entity_cross_view_repeats,
            "max_repeat_encounter_pct": self.max_repeat_encounter_pct,
            "repeat_encounter_pct_gate": REPEAT_ENCOUNTER_PCT,
            "repeat_encounter_gate_ok": self.repeat_encounter_gate_ok(),
        })
    }
}

fn rate_pct(before: u64, after: u64) -> u32 {
    if before == 0 {
        return 0;
    }
    let saved = before.saturating_sub(after);
    ((saved * 100) / before) as u32
}

/// `ceil(billed * 100 / first)` for gate accounting (0 when first is 0).
pub fn repeat_encounter_pct(first_tokens: u32, billed_tokens: u32) -> u32 {
    if first_tokens == 0 {
        return 0;
    }
    let num = u64::from(billed_tokens) * 100;
    num.div_ceil(u64::from(first_tokens)) as u32
}

/// Path to the dedup ledger under a store root.
pub fn dedup_ledger_path(store_root: &Path) -> PathBuf {
    store_root.join(DEDUP_LEDGER_REL)
}

/// Read the dedup ledger; missing file is an empty ledger.
pub fn read_dedup_ledger(store_root: &Path) -> io::Result<EntityDedupLedger> {
    let path = dedup_ledger_path(store_root);
    match fs::read(&path) {
        Ok(bytes) => {
            let mut ledger: EntityDedupLedger = serde_json::from_slice(&bytes)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
            if ledger.schema.is_empty() {
                ledger.schema = DEDUP_LEDGER_SCHEMA.to_string();
            }
            Ok(ledger)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(EntityDedupLedger::empty()),
        Err(err) => Err(err),
    }
}

/// Persist the dedup ledger (local operational only).
pub fn write_dedup_ledger(store_root: &Path, ledger: &EntityDedupLedger) -> io::Result<()> {
    let path = dedup_ledger_path(store_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut out = ledger.clone();
    if out.schema.is_empty() {
        out.schema = DEDUP_LEDGER_SCHEMA.to_string();
    }
    let bytes = serde_json::to_vec_pretty(&out)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    fs::write(path, bytes)
}

/// Merge `delta` into the on-disk ledger (additive counters; max of gate pct).
pub fn record_dedup_ledger(store_root: &Path, delta: &EntityDedupLedger) -> io::Result<()> {
    let mut ledger = read_dedup_ledger(store_root)?;
    ledger.naive_tokens = ledger.naive_tokens.saturating_add(delta.naive_tokens);
    ledger.after_byte_dedup_tokens = ledger
        .after_byte_dedup_tokens
        .saturating_add(delta.after_byte_dedup_tokens);
    ledger.after_entity_dedup_tokens = ledger
        .after_entity_dedup_tokens
        .saturating_add(delta.after_entity_dedup_tokens);
    ledger.byte_first_encounters = ledger
        .byte_first_encounters
        .saturating_add(delta.byte_first_encounters);
    ledger.byte_repeat_encounters = ledger
        .byte_repeat_encounters
        .saturating_add(delta.byte_repeat_encounters);
    ledger.entity_first_encounters = ledger
        .entity_first_encounters
        .saturating_add(delta.entity_first_encounters);
    ledger.entity_repeat_encounters = ledger
        .entity_repeat_encounters
        .saturating_add(delta.entity_repeat_encounters);
    ledger.entity_cross_view_repeats = ledger
        .entity_cross_view_repeats
        .saturating_add(delta.entity_cross_view_repeats);
    ledger.max_repeat_encounter_pct = ledger
        .max_repeat_encounter_pct
        .max(delta.max_repeat_encounter_pct);
    write_dedup_ledger(store_root, &ledger)
}

struct ProcessDedupLedger {
    ledger: EntityDedupLedger,
}

static DEFAULT_DEDUP_LEDGER: LazyLock<Mutex<ProcessDedupLedger>> = LazyLock::new(|| {
    Mutex::new(ProcessDedupLedger {
        ledger: EntityDedupLedger::empty(),
    })
});

/// Snapshot of the process-local dedup ledger.
pub fn process_dedup_ledger() -> EntityDedupLedger {
    DEFAULT_DEDUP_LEDGER.lock().ledger.clone()
}

/// Record into the process-local dedup ledger.
pub fn record_process_dedup_encounter(cost: &EncounterCost, byte_repeat: bool) {
    DEFAULT_DEDUP_LEDGER
        .lock()
        .ledger
        .record_encounter(cost, byte_repeat);
}

/// Record a destination-level hit on the process-local dedup ledger.
pub fn record_process_destination_hit(
    tokens: u32,
    byte_repeat: bool,
    entity_cross_view: bool,
    first: bool,
) {
    DEFAULT_DEDUP_LEDGER.lock().ledger.record_destination_hit(
        tokens,
        byte_repeat,
        entity_cross_view,
        first,
    );
}

/// Clears the process-local dedup ledger (tests).
#[doc(hidden)]
pub fn clear_process_dedup_ledger() {
    DEFAULT_DEDUP_LEDGER.lock().ledger = EntityDedupLedger::empty();
}

fn validate_entity_id_hex(hex: &str) -> Result<()> {
    if hex.len() != 64 {
        bail!("entity id must be full 64-hex SHA-256");
    }
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("entity id must be hexadecimal");
    }
    // EntityId::parse normalizes valid hexadecimal input to lowercase.
    Ok(())
}

/// Validate a canonical lowercase entity id.
pub fn validate_entity_ref_id(hex: &str) -> Result<()> {
    if hex.len() != 64 {
        bail!("entity id must be full 64-hex SHA-256");
    }
    if !hex
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        bail!("entity id must be lowercase hexadecimal");
    }
    Ok(())
}
