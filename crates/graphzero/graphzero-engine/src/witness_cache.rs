//! Witness cache keyed by minimum dependency sets and anti-dependency scope roots.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use anyhow::{Result, anyhow};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

use graphzero_store::Snapshot;
use graphzero_store::store::format::FORMAT_VERSION;

use graphzero_core::invalidation::CutoffReport;

/// Wire discriminator of the shared cache-entry contract.
pub const CACHE_ENTRY_SCHEMA: &str = "cache-entry";

/// Operator id for the snapshot symbol query cached here.
pub const SYMBOL_QUERY_OPERATOR: &str = "graphzero.symbol_query";

const QUERY_CACHE_CONTRACT_VERSION: &str = "1";

/// Validation failures, stable enough to classify in conformance checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WitnessCacheError {
    EmptyField(&'static str),
    RootNotWitnessed(String),
    MissingScopeRoots,
    UnexpectedScopeRoots,
    WitnessMismatch,
    UnsupportedSchema(String),
}

impl std::fmt::Display for WitnessCacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyField(field) => write!(f, "{field} must not be empty"),
            Self::RootNotWitnessed(root) => {
                write!(f, "root is not covered by completeness witness: {root}")
            }
            Self::MissingScopeRoots => write!(f, "negative entry requires scope roots"),
            Self::UnexpectedScopeRoots => write!(f, "positive entry cannot carry scope roots"),
            Self::WitnessMismatch => write!(f, "completeness witness does not match checked roots"),
            Self::UnsupportedSchema(schema) => write!(f, "unsupported cache schema: {schema}"),
        }
    }
}

impl std::error::Error for WitnessCacheError {}

/// A non-empty content-addressed root. Empty roots are never cache evidence.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CacheRoot(String);

impl CacheRoot {
    pub fn new(root: impl Into<String>) -> Result<Self, WitnessCacheError> {
        let root = root.into();
        if root.is_empty() {
            return Err(WitnessCacheError::EmptyField("cache root"));
        }
        Ok(Self(root))
    }

    /// Exact dependency root for one file: relative path plus content digest.
    pub fn file(rel_path: &str, content_hash: &str) -> Result<Self, WitnessCacheError> {
        if rel_path.is_empty() || content_hash.is_empty() {
            return Err(WitnessCacheError::EmptyField("file root"));
        }
        Self::new(format!("file/{rel_path}@{content_hash}"))
    }

    /// Anti-dependency root for a searched scope: prefix plus a digest over the
    /// complete (path, content digest) listing inside it.
    pub fn scope(prefix: &str, listing_digest: &str) -> Result<Self, WitnessCacheError> {
        if listing_digest.is_empty() {
            return Err(WitnessCacheError::EmptyField("scope root"));
        }
        Self::new(format!("scope/{prefix}@{listing_digest}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Locked operator identity: parsing/index semantics live in the version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorIdentity {
    id: String,
    version: String,
}

impl OperatorIdentity {
    pub fn new(
        id: impl Into<String>,
        version: impl Into<String>,
    ) -> Result<Self, WitnessCacheError> {
        let id = id.into();
        let version = version.into();
        if id.is_empty() {
            return Err(WitnessCacheError::EmptyField("operator id"));
        }
        if version.is_empty() {
            return Err(WitnessCacheError::EmptyField("operator version"));
        }
        Ok(Self { id, version })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}

/// Durable proof for the dependency cone examined by the operator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletenessWitness {
    proof_root: CacheRoot,
    checked_roots: Vec<CacheRoot>,
}

impl CompletenessWitness {
    /// Derive the witness from the roots that were actually checked. The proof
    /// root is a digest of the normalized checked-root set, so a tampered root
    /// list no longer recomputes to the recorded proof.
    pub fn over(mut checked_roots: Vec<CacheRoot>) -> Result<Self, WitnessCacheError> {
        normalize_roots(&mut checked_roots);
        if checked_roots.is_empty() {
            return Err(WitnessCacheError::EmptyField("checked roots"));
        }
        let proof_root = CacheRoot::new(format!("witness/{}", proof_digest(&checked_roots)))?;
        Ok(Self {
            proof_root,
            checked_roots,
        })
    }

    pub fn proof_root(&self) -> &CacheRoot {
        &self.proof_root
    }

    pub fn checked_roots(&self) -> &[CacheRoot] {
        &self.checked_roots
    }

    /// Recompute the proof root from the checked roots.
    pub fn recomputes(&self) -> bool {
        let expected = format!("witness/{}", proof_digest(&self.checked_roots));
        self.proof_root.as_str() == expected
    }

    fn validate(&self) -> Result<(), WitnessCacheError> {
        if self.checked_roots.is_empty() {
            return Err(WitnessCacheError::EmptyField("checked roots"));
        }
        for root in &self.checked_roots {
            if root.as_str().is_empty() {
                return Err(WitnessCacheError::EmptyField("checked root"));
            }
        }
        if !self.recomputes() {
            return Err(WitnessCacheError::WitnessMismatch);
        }
        Ok(())
    }
}

fn proof_digest(roots: &[CacheRoot]) -> String {
    let listing = Value::Array(
        roots
            .iter()
            .map(|r| Value::String(r.as_str().to_owned()))
            .collect(),
    );
    zero_abi::sha256_hex(zero_abi::canonical_json(&listing).as_bytes())
}

fn normalize_roots(roots: &mut Vec<CacheRoot>) {
    roots.sort();
    roots.dedup();
}

/// Complete cache key. scope_roots is empty for a positive hit and non-empty for
/// a certified no-matches answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheKey {
    operator: OperatorIdentity,
    canonical_parameters: Value,
    minimum_dependency_roots: Vec<CacheRoot>,
    environment_roots: Vec<CacheRoot>,
    toolchain_roots: Vec<CacheRoot>,
    completeness_witness: CompletenessWitness,
    #[serde(default)]
    scope_roots: Vec<CacheRoot>,
    /// Declared network-fixture roots: absent = no fixtures
    /// declared. Only present roots take part in the key digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    network_fixture_roots: Option<Vec<CacheRoot>>,
    /// Declared clock/randomness policy root: absent = no policy
    /// declared. Only a present root takes part in the key digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    clock_randomness_policy_root: Option<CacheRoot>,
}

#[derive(Debug, Deserialize)]
struct CacheKeyWire {
    operator: OperatorIdentity,
    canonical_parameters: Value,
    minimum_dependency_roots: Vec<CacheRoot>,
    environment_roots: Vec<CacheRoot>,
    toolchain_roots: Vec<CacheRoot>,
    completeness_witness: CompletenessWitness,
    #[serde(default)]
    scope_roots: Vec<CacheRoot>,
    #[serde(default)]
    network_fixture_roots: Option<Vec<CacheRoot>>,
    #[serde(default)]
    clock_randomness_policy_root: Option<CacheRoot>,
}

impl<'de> Deserialize<'de> for CacheKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = CacheKeyWire::deserialize(deserializer)?;
        CacheKey::build(
            wire.operator,
            wire.canonical_parameters,
            wire.minimum_dependency_roots,
            wire.environment_roots,
            wire.toolchain_roots,
            wire.completeness_witness,
            wire.scope_roots,
            wire.network_fixture_roots,
            wire.clock_randomness_policy_root,
        )
        .map_err(serde::de::Error::custom)
    }
}

impl CacheKey {
    /// Positive-result key: exact minimum dependency set, no scope roots.
    pub fn new(
        operator: OperatorIdentity,
        canonical_parameters: Value,
        minimum_dependency_roots: Vec<CacheRoot>,
        environment_roots: Vec<CacheRoot>,
        toolchain_roots: Vec<CacheRoot>,
        completeness_witness: CompletenessWitness,
    ) -> Result<Self, WitnessCacheError> {
        Self::build(
            operator,
            canonical_parameters,
            minimum_dependency_roots,
            environment_roots,
            toolchain_roots,
            completeness_witness,
            Vec::new(),
            None,
            None,
        )
    }

    /// Negative-result key: scope roots are anti-dependencies, so any change in a
    /// searched scope invalidates the no-matches answer.
    pub fn with_scope_roots(
        operator: OperatorIdentity,
        canonical_parameters: Value,
        minimum_dependency_roots: Vec<CacheRoot>,
        environment_roots: Vec<CacheRoot>,
        toolchain_roots: Vec<CacheRoot>,
        completeness_witness: CompletenessWitness,
        scope_roots: Vec<CacheRoot>,
    ) -> Result<Self, WitnessCacheError> {
        if scope_roots.is_empty() {
            return Err(WitnessCacheError::MissingScopeRoots);
        }
        Self::build(
            operator,
            canonical_parameters,
            minimum_dependency_roots,
            environment_roots,
            toolchain_roots,
            completeness_witness,
            scope_roots,
            None,
            None,
        )
    }

    /// Key with declared network-fixture and clock/randomness roots.
    /// Empty declarations omit those roots and preserve the same digest.
    #[allow(clippy::too_many_arguments)]
    pub fn with_declared_causal_roots(
        operator: OperatorIdentity,
        canonical_parameters: Value,
        minimum_dependency_roots: Vec<CacheRoot>,
        environment_roots: Vec<CacheRoot>,
        toolchain_roots: Vec<CacheRoot>,
        completeness_witness: CompletenessWitness,
        scope_roots: Vec<CacheRoot>,
        network_fixture_roots: Option<Vec<CacheRoot>>,
        clock_randomness_policy_root: Option<CacheRoot>,
    ) -> Result<Self, WitnessCacheError> {
        Self::build(
            operator,
            canonical_parameters,
            minimum_dependency_roots,
            environment_roots,
            toolchain_roots,
            completeness_witness,
            scope_roots,
            network_fixture_roots,
            clock_randomness_policy_root,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        operator: OperatorIdentity,
        canonical_parameters: Value,
        mut minimum_dependency_roots: Vec<CacheRoot>,
        mut environment_roots: Vec<CacheRoot>,
        mut toolchain_roots: Vec<CacheRoot>,
        completeness_witness: CompletenessWitness,
        mut scope_roots: Vec<CacheRoot>,
        mut network_fixture_roots: Option<Vec<CacheRoot>>,
        clock_randomness_policy_root: Option<CacheRoot>,
    ) -> Result<Self, WitnessCacheError> {
        if operator.id.is_empty() {
            return Err(WitnessCacheError::EmptyField("operator id"));
        }
        if operator.version.is_empty() {
            return Err(WitnessCacheError::EmptyField("operator version"));
        }
        normalize_roots(&mut minimum_dependency_roots);
        normalize_roots(&mut environment_roots);
        normalize_roots(&mut toolchain_roots);
        normalize_roots(&mut scope_roots);
        if let Some(fixtures) = network_fixture_roots.as_mut() {
            normalize_roots(fixtures);
            if fixtures.is_empty() {
                // Empty fixture sets contribute no causal root to the digest.
                network_fixture_roots = None;
            }
        }
        completeness_witness.validate()?;
        for root in minimum_dependency_roots
            .iter()
            .chain(environment_roots.iter())
            .chain(toolchain_roots.iter())
            .chain(scope_roots.iter())
            .chain(network_fixture_roots.iter().flatten())
            .chain(clock_randomness_policy_root.iter())
        {
            if root.as_str().is_empty() {
                return Err(WitnessCacheError::EmptyField("cache root"));
            }
            if !completeness_witness.checked_roots.contains(root) {
                return Err(WitnessCacheError::RootNotWitnessed(
                    root.as_str().to_owned(),
                ));
            }
        }
        Ok(Self {
            operator,
            canonical_parameters,
            minimum_dependency_roots,
            environment_roots,
            toolchain_roots,
            completeness_witness,
            scope_roots,
            network_fixture_roots,
            clock_randomness_policy_root,
        })
    }

    pub fn operator(&self) -> &OperatorIdentity {
        &self.operator
    }

    pub fn canonical_parameters(&self) -> &Value {
        &self.canonical_parameters
    }

    pub fn minimum_dependency_roots(&self) -> &[CacheRoot] {
        &self.minimum_dependency_roots
    }

    pub fn environment_roots(&self) -> &[CacheRoot] {
        &self.environment_roots
    }

    pub fn toolchain_roots(&self) -> &[CacheRoot] {
        &self.toolchain_roots
    }

    pub fn completeness_witness(&self) -> &CompletenessWitness {
        &self.completeness_witness
    }

    pub fn scope_roots(&self) -> &[CacheRoot] {
        &self.scope_roots
    }

    /// Declared network-fixture roots, when declared.
    pub fn network_fixture_roots(&self) -> Option<&[CacheRoot]> {
        self.network_fixture_roots.as_deref()
    }

    /// Declared clock/randomness policy root, when declared.
    pub fn clock_randomness_policy_root(&self) -> Option<&CacheRoot> {
        self.clock_randomness_policy_root.as_ref()
    }

    /// Every root the key depends on, including anti-dependencies and declared
    /// causal roots.
    pub fn all_roots(&self) -> impl Iterator<Item = &CacheRoot> {
        self.minimum_dependency_roots
            .iter()
            .chain(self.environment_roots.iter())
            .chain(self.toolchain_roots.iter())
            .chain(self.scope_roots.iter())
            .chain(self.network_fixture_roots.iter().flatten())
            .chain(self.clock_randomness_policy_root.iter())
    }

    /// Canonical-JSON key bytes (sorted object keys, normalized root sets).
    pub fn canonical_key_json(&self) -> String {
        let value = serde_json::to_value(self).expect("cache key is JSON-serializable");
        zero_abi::canonical_json(&value)
    }

    /// SHA-256 of canonical_key_json: the lookup identity.
    pub fn key_hash_hex(&self) -> String {
        zero_abi::sha256_hex(self.canonical_key_json().as_bytes())
    }

    /// Coarse bucket used to find candidate entries before root verification.
    /// Only operator identity and parameters take part; dependency roots must
    /// not be needed to *find* a candidate, only to accept it.
    pub fn operation_bucket(&self) -> String {
        operation_bucket(&self.operator, &self.canonical_parameters)
    }
}

/// Bucket identity for (operator, canonical parameters).
pub fn operation_bucket(operator: &OperatorIdentity, canonical_parameters: &Value) -> String {
    let value = json!({
        "operator": operator,
        "canonical_parameters": canonical_parameters,
    });
    zero_abi::sha256_hex(zero_abi::canonical_json(&value).as_bytes())
}

/// Cached value: an output root, or a certified no-matches answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CacheValue {
    Hit { output_root: CacheRoot },
    NoMatches,
}

/// One validated cache entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheEntry {
    schema: String,
    key: CacheKey,
    value: CacheValue,
}

#[derive(Debug, Deserialize)]
struct CacheEntryWire {
    schema: String,
    key: CacheKey,
    value: CacheValue,
}

impl<'de> Deserialize<'de> for CacheEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = CacheEntryWire::deserialize(deserializer)?;
        let entry = Self {
            schema: wire.schema,
            key: wire.key,
            value: wire.value,
        };
        entry.validate().map_err(serde::de::Error::custom)?;
        Ok(entry)
    }
}

impl CacheEntry {
    pub fn positive(key: CacheKey, output_root: CacheRoot) -> Result<Self, WitnessCacheError> {
        let entry = Self {
            schema: CACHE_ENTRY_SCHEMA.to_owned(),
            key,
            value: CacheValue::Hit { output_root },
        };
        entry.validate()?;
        Ok(entry)
    }

    pub fn negative(key: CacheKey) -> Result<Self, WitnessCacheError> {
        let entry = Self {
            schema: CACHE_ENTRY_SCHEMA.to_owned(),
            key,
            value: CacheValue::NoMatches,
        };
        entry.validate()?;
        Ok(entry)
    }

    pub fn schema(&self) -> &str {
        &self.schema
    }

    pub fn key(&self) -> &CacheKey {
        &self.key
    }

    pub fn value(&self) -> &CacheValue {
        &self.value
    }

    pub fn key_hash_hex(&self) -> String {
        self.key.key_hash_hex()
    }

    fn validate(&self) -> Result<(), WitnessCacheError> {
        if self.schema != CACHE_ENTRY_SCHEMA {
            return Err(WitnessCacheError::UnsupportedSchema(self.schema.clone()));
        }
        self.key.completeness_witness.validate()?;
        match &self.value {
            CacheValue::Hit { output_root } => {
                if !self.key.scope_roots.is_empty() {
                    return Err(WitnessCacheError::UnexpectedScopeRoots);
                }
                if output_root.as_str().is_empty() {
                    return Err(WitnessCacheError::EmptyField("output root"));
                }
            }
            CacheValue::NoMatches => {
                if self.key.scope_roots.is_empty() {
                    return Err(WitnessCacheError::MissingScopeRoots);
                }
            }
        }
        Ok(())
    }
}

/// Outcome of re-resolving one recorded root against current content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootResolution {
    Unchanged,
    Changed,
    /// The root shape is unknown or current content is unavailable: fail closed.
    Unresolvable,
}

/// Resolves recorded roots against current content.
pub trait RootResolver {
    fn resolve(&self, root: &CacheRoot) -> RootResolution;
}

/// Whether a stored entry may still be reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryStatus {
    Reusable,
    /// A dependency, toolchain, or scope root changed.
    Invalidated,
    /// Evidence missing or witness unverifiable: never treated as unchanged.
    Unverifiable,
}

/// Measured verification/reuse telemetry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CacheReuseReport {
    /// Cache entries whose roots were re-resolved during lookups.
    pub entries_verified: u64,
    /// Individual roots re-resolved during verification (verification work).
    pub roots_resolved: u64,
    /// Entries accepted as reusable (full verified reuse).
    pub hits: u64,
    /// Lookups that found no reusable candidate.
    pub misses: u64,
    /// Entries rejected because a recorded root changed or could not be
    /// verified.
    pub invalidations: u64,
    /// Inserts that collapsed onto an existing row with an identical causal
    /// key digest (dedup measured at insert time).
    pub deduplicated_inserts: u64,
    /// Equality-boundary cutoff passes reported via
    /// [`WitnessCache::record_cutoff_savings`].
    pub cutoff_passes: u64,
    /// Producers recomputed inside reported cutoff passes.
    pub cutoff_recomputed: u64,
    /// Producers saved by equality boundaries inside reported cutoff passes.
    pub cutoff_saved: u64,
}

/// Verification work measured during one re-resolution pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VerifyStats {
    /// Cache entries whose roots were re-resolved.
    pub entries: u64,
    /// Individual roots resolved.
    pub roots: u64,
}

/// Verify one entry against current content.
pub fn verify_entry(entry: &CacheEntry, resolver: &dyn RootResolver) -> EntryStatus {
    let mut stats = VerifyStats::default();
    verify_entry_counted(entry, resolver, &mut stats)
}

/// [`verify_entry`] with measured verification work. Every entry whose witness recomputes is
/// counted once, and every root actually resolved is counted, so reuse telemetry reflects real
/// verification effort (fail-closed early exits still count the work performed up to the exit).
pub fn verify_entry_counted(
    entry: &CacheEntry,
    resolver: &dyn RootResolver,
    stats: &mut VerifyStats,
) -> EntryStatus {
    if !entry.key.completeness_witness.recomputes() {
        return EntryStatus::Unverifiable;
    }
    stats.entries += 1;
    let mut status = EntryStatus::Reusable;
    for root in entry.key.all_roots() {
        if !entry.key.completeness_witness.checked_roots.contains(root) {
            return EntryStatus::Unverifiable;
        }
        stats.roots += 1;
        match resolver.resolve(root) {
            RootResolution::Unchanged => {}
            RootResolution::Changed => status = EntryStatus::Invalidated,
            RootResolution::Unresolvable => return EntryStatus::Unverifiable,
        }
    }
    status
}

/// Current (relative path -> content digest) listing plus locked toolchain
/// identity, used to re-resolve recorded roots.
#[derive(Debug)]
pub struct SnapshotRoots {
    files: BTreeMap<String, String>,
    toolchain: String,
    /// Memoized `scope_digest` results for this listing (prefix -> hex).
    scope_digest_memo: RefCell<HashMap<String, String>>,
}

impl Clone for SnapshotRoots {
    fn clone(&self) -> Self {
        Self {
            files: self.files.clone(),
            toolchain: self.toolchain.clone(),
            // Fresh memo: cheap to recompute; avoids sharing RefCell across clones.
            scope_digest_memo: RefCell::new(HashMap::new()),
        }
    }
}

impl SnapshotRoots {
    /// Build from an opened snapshot's indexed path records. A path may still
    /// carry superseded blob records, so the newest record wins (hash breaks
    /// mtime ties) and the mapping does not depend on hash-map iteration order.
    pub fn from_snapshot(snapshot: &Snapshot) -> Self {
        let mut newest: BTreeMap<String, (u128, String)> = BTreeMap::new();
        for (hash, record) in snapshot.path_records() {
            let candidate = (record.mtime_nanos, hash.to_hex());
            newest
                .entry(record.path.clone())
                .and_modify(|current| {
                    if candidate > *current {
                        *current = candidate.clone();
                    }
                })
                .or_insert(candidate);
        }
        let files = newest
            .into_iter()
            .map(|(path, (_, hash))| (path, hash))
            .collect();
        Self {
            files,
            toolchain: toolchain_root_string(),
            scope_digest_memo: RefCell::new(HashMap::new()),
        }
    }

    /// Exact dependency root for one indexed file.
    pub fn file_root(&self, rel_path: &str) -> Option<CacheRoot> {
        let hash = self.files.get(rel_path)?;
        CacheRoot::file(rel_path, hash).ok()
    }

    /// Anti-dependency root over every indexed file under `prefix`.
    pub fn scope_root(&self, prefix: &str) -> Result<CacheRoot, WitnessCacheError> {
        CacheRoot::scope(prefix, &self.scope_digest(prefix))
    }

    pub fn toolchain_root(&self) -> Result<CacheRoot, WitnessCacheError> {
        CacheRoot::new(self.toolchain.clone())
    }

    /// Digest of the sorted (path, hash) listing under `prefix`. Streams the same
    /// bytes as `canonical_json` of `[{"path":...,"root":...},...]` into Sha256 —
    /// no intermediate JSON `Value` array tree. Memoized per prefix on this listing.
    fn scope_digest(&self, prefix: &str) -> String {
        if let Some(hit) = self.scope_digest_memo.borrow().get(prefix) {
            return hit.clone();
        }
        let digest = scope_digest_streaming(&self.files, prefix);
        self.scope_digest_memo
            .borrow_mut()
            .insert(prefix.to_owned(), digest.clone());
        digest
    }
}

/// Stream-canonical JSON array of `{"path","root"}` objects (keys sorted) into
/// Sha256. Byte-identical to `sha256_hex(canonical_json(&Value::Array(...)))`.
fn scope_digest_streaming(files: &BTreeMap<String, String>, prefix: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"[");
    let mut first = true;
    for (path, hash) in files.iter().filter(|(path, _)| path.starts_with(prefix)) {
        if !first {
            hasher.update(b",");
        }
        first = false;
        // Object keys sorted alphabetically: "path" then "root".
        hasher.update(b"{\"path\":");
        hasher.update(
            serde_json::to_string(path.as_str())
                .unwrap_or_else(|_| "\"\"".into())
                .as_bytes(),
        );
        hasher.update(b",\"root\":");
        hasher.update(
            serde_json::to_string(hash.as_str())
                .unwrap_or_else(|_| "\"\"".into())
                .as_bytes(),
        );
        hasher.update(b"}");
    }
    hasher.update(b"]");
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push(char::from_digit(u32::from(b >> 4), 16).unwrap());
        out.push(char::from_digit(u32::from(b & 0x0f), 16).unwrap());
    }
    out
}

impl RootResolver for SnapshotRoots {
    fn resolve(&self, root: &CacheRoot) -> RootResolution {
        let raw = root.as_str();
        // Root identities are unprefixed: `file/`, `scope/`, or `toolchain/`.
        let rest = raw;
        let toolchain = self.toolchain.as_str();
        if let Some(rest) = rest.strip_prefix("file/") {
            let Some((path, recorded)) = rest.rsplit_once('@') else {
                return RootResolution::Unresolvable;
            };
            return match self.files.get(path) {
                Some(current) if current == recorded => RootResolution::Unchanged,
                // A changed or removed dependency file both change the answer.
                _ => RootResolution::Changed,
            };
        }
        if let Some(rest) = rest.strip_prefix("scope/") {
            let Some((prefix, recorded)) = rest.rsplit_once('@') else {
                return RootResolution::Unresolvable;
            };
            return if self.scope_digest(prefix) == recorded {
                RootResolution::Unchanged
            } else {
                RootResolution::Changed
            };
        }
        if rest.starts_with("toolchain/") {
            return if rest == toolchain {
                RootResolution::Unchanged
            } else {
                RootResolution::Changed
            };
        }
        RootResolution::Unresolvable
    }
}

/// Locked parser/index identity. Query semantic changes must bump the query
/// contract version so old certificates cannot be reused.
pub fn toolchain_root_string() -> String {
    format!("toolchain/index-format-{FORMAT_VERSION}+query-contract-{QUERY_CACHE_CONTRACT_VERSION}")
}

/// Operator identity for the cached snapshot symbol query.
pub fn symbol_query_operator() -> OperatorIdentity {
    OperatorIdentity::new(
        SYMBOL_QUERY_OPERATOR,
        format!("index-format-{FORMAT_VERSION}+query-contract-{QUERY_CACHE_CONTRACT_VERSION}"),
    )
    .expect("operator identity fields are non-empty")
}

/// One def site of a cached answer.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AnswerSite {
    pub path: String,
    pub evidence_ref: String,
}

/// Cached query answer payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolAnswer {
    pub symbol: String,
    pub sites: Vec<AnswerSite>,
}

/// Where an answer came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerSource {
    CacheHit,
    Computed,
}

/// A cached-or-computed symbol answer. `None` answer means certified no matches.
#[derive(Debug, Clone, PartialEq)]
pub struct CachedAnswer {
    pub answer: Option<SymbolAnswer>,
    pub source: AnswerSource,
    pub key_hash_hex: String,
}

/// Witness cache: candidate entries bucketed by operator+parameters, accepted
/// only after every recorded root re-resolves to the same content.
#[derive(Debug, Default)]
pub struct WitnessCache {
    buckets: HashMap<String, Vec<CacheEntry>>,
    outputs: HashMap<String, String>,
    hits: u64,
    misses: u64,
    invalidations: u64,
    entries_verified: u64,
    roots_resolved: u64,
    deduplicated_inserts: u64,
    cutoff_passes: u64,
    cutoff_recomputed: u64,
    cutoff_saved: u64,
    /// Cached path listing for the last snapshot identity used with this cache.
    /// Invalidates automatically when store_root / snapshot_id / global_hash change
    /// (reopen / reindex). Avoids O(|paths|) rebuild on every cached_symbol_query.
    roots_cache: Option<(String, u64, u64, Arc<SnapshotRoots>)>,
}

impl WitnessCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Path listing for `snapshot`, cached by `(store_root, snapshot_id, global_hash)`.
    /// Rebuilds only when the opened snapshot identity changes.
    pub fn snapshot_roots(&mut self, snapshot: &Snapshot) -> Arc<SnapshotRoots> {
        let store = snapshot.store_root.to_string_lossy().into_owned();
        let sid = snapshot.entry.snapshot_id;
        let gh = snapshot.entry.global_hash;
        if let Some((s, id, h, roots)) = &self.roots_cache {
            if s == &store && *id == sid && *h == gh {
                return Arc::clone(roots);
            }
        }
        let roots = Arc::new(SnapshotRoots::from_snapshot(snapshot));
        self.roots_cache = Some((store, sid, gh, Arc::clone(&roots)));
        roots
    }

    pub fn hits(&self) -> u64 {
        self.hits
    }

    pub fn misses(&self) -> u64 {
        self.misses
    }

    /// Entries rejected because a recorded root changed or could not be verified.
    pub fn invalidations(&self) -> u64 {
        self.invalidations
    }

    /// Inserts that collapsed onto an existing row with an identical causal
    /// key digest (causal-key dedup, measured at insert time).
    pub fn deduplicated_inserts(&self) -> u64 {
        self.deduplicated_inserts
    }

    /// Measured verification/reuse telemetry: every
    /// counter reflects events that actually happened.
    pub fn reuse_report(&self) -> CacheReuseReport {
        CacheReuseReport {
            entries_verified: self.entries_verified,
            roots_resolved: self.roots_resolved,
            hits: self.hits,
            misses: self.misses,
            invalidations: self.invalidations,
            deduplicated_inserts: self.deduplicated_inserts,
            cutoff_passes: self.cutoff_passes,
            cutoff_recomputed: self.cutoff_recomputed,
            cutoff_saved: self.cutoff_saved,
        }
    }

    /// Record the measured savings of an equality-boundary cutoff pass into this cache's
    /// telemetry.
    pub fn record_cutoff_savings(&mut self, report: &CutoffReport) {
        self.cutoff_passes += 1;
        self.cutoff_recomputed += report.recomputed.len() as u64;
        self.cutoff_saved += report.cut_off.len() as u64;
    }

    pub fn entry_count(&self) -> usize {
        self.buckets.values().map(Vec::len).sum()
    }

    /// All stored entries, in bucket-insertion order per bucket.
    pub fn entries(&self) -> impl Iterator<Item = &CacheEntry> {
        self.buckets.values().flatten()
    }

    /// Look up a reusable entry for (operator, parameters) under current content. Invalidated /
    /// unverifiable candidates are **dropped** so buckets do not re-verify dead entries forever.
    pub fn lookup(
        &mut self,
        operator: &OperatorIdentity,
        canonical_parameters: &Value,
        resolver: &dyn RootResolver,
    ) -> Option<(String, Option<SymbolAnswer>)> {
        let bucket = operation_bucket(operator, canonical_parameters);
        let Some(candidates) = self.buckets.get(&bucket).cloned() else {
            self.misses += 1;
            return None;
        };
        let mut dead_keys: Vec<String> = Vec::new();
        let mut hit: Option<(String, Option<SymbolAnswer>)> = None;
        let mut verify_stats = VerifyStats::default();
        for entry in &candidates {
            match verify_entry_counted(entry, resolver, &mut verify_stats) {
                EntryStatus::Reusable => {
                    let payload = match entry.value() {
                        CacheValue::Hit { output_root } => {
                            let Some(raw) = self.outputs.get(output_root.as_str()) else {
                                // Output content is gone: treat as dead entry.
                                self.invalidations += 1;
                                dead_keys.push(entry.key_hash_hex());
                                continue;
                            };
                            match serde_json::from_str::<SymbolAnswer>(raw) {
                                Ok(answer) => Some(answer),
                                Err(_) => {
                                    self.invalidations += 1;
                                    dead_keys.push(entry.key_hash_hex());
                                    continue;
                                }
                            }
                        }
                        CacheValue::NoMatches => None,
                    };
                    self.hits += 1;
                    hit = Some((entry.key_hash_hex(), payload));
                    break;
                }
                EntryStatus::Invalidated | EntryStatus::Unverifiable => {
                    self.invalidations += 1;
                    dead_keys.push(entry.key_hash_hex());
                }
            }
        }
        self.entries_verified += verify_stats.entries;
        self.roots_resolved += verify_stats.roots;
        if !dead_keys.is_empty() {
            self.drop_entries(&bucket, &dead_keys);
        }
        if hit.is_some() {
            return hit;
        }
        self.misses += 1;
        None
    }

    /// Remove dead entries from a bucket and drop orphaned output payloads.
    fn drop_entries(&mut self, bucket: &str, dead_keys: &[String]) {
        let Some(slot) = self.buckets.get_mut(bucket) else {
            return;
        };
        let dead: std::collections::HashSet<&str> = dead_keys.iter().map(String::as_str).collect();
        let mut dropped_outputs: Vec<String> = Vec::new();
        slot.retain(|entry| {
            if dead.contains(entry.key_hash_hex().as_str()) {
                if let CacheValue::Hit { output_root } = entry.value() {
                    dropped_outputs.push(output_root.as_str().to_owned());
                }
                false
            } else {
                true
            }
        });
        if slot.is_empty() {
            self.buckets.remove(bucket);
        }
        for out in dropped_outputs {
            // Only remove output if no remaining entry references it.
            let still_used = self.buckets.values().flatten().any(|e| {
                matches!(e.value(), CacheValue::Hit { output_root } if output_root.as_str() == out)
            });
            if !still_used {
                self.outputs.remove(&out);
            }
        }
    }

    /// Store a positive answer under its minimum-dependency-set key.
    pub fn insert_positive(
        &mut self,
        key: CacheKey,
        answer: &SymbolAnswer,
    ) -> Result<String, WitnessCacheError> {
        let payload = zero_abi::canonical_json(
            &serde_json::to_value(answer).expect("answer is JSON-serializable"),
        );
        let output_root =
            CacheRoot::new(format!("out/{}", zero_abi::sha256_hex(payload.as_bytes())))?;
        self.outputs
            .insert(output_root.as_str().to_owned(), payload);
        let entry = CacheEntry::positive(key, output_root)?;
        Ok(self.store(entry))
    }

    /// Store a certified no-matches answer keyed by its searched scope roots.
    pub fn insert_negative(&mut self, key: CacheKey) -> Result<String, WitnessCacheError> {
        let entry = CacheEntry::negative(key)?;
        Ok(self.store(entry))
    }

    fn store(&mut self, entry: CacheEntry) -> String {
        let key_hash = entry.key_hash_hex();
        let bucket = entry.key.operation_bucket();
        let slot = self.buckets.entry(bucket).or_default();
        if let Some(existing) = slot.iter_mut().find(|e| e.key_hash_hex() == key_hash) {
            // Causal-key dedup: an identical CacheKey digest reuses ONE stored row -- no duplicate
            // rows for the same causal key. Measured, not claimed: the collapse increments
            // `deduplicated_inserts`.
            let replaced = std::mem::replace(existing, entry);
            self.deduplicated_inserts += 1;
            if let CacheValue::Hit { output_root } = replaced.value() {
                let still_used = self.buckets.values().flatten().any(|e| {
                    matches!(e.value(), CacheValue::Hit { output_root: other }
                        if other == output_root)
                });
                if !still_used {
                    self.outputs.remove(output_root.as_str());
                }
            }
        } else {
            slot.push(entry);
        }
        key_hash
    }
}

fn canonical_parameters(symbol: &str, scope: &str, budget: usize) -> Value {
    json!({"symbol": symbol, "scope": scope, "budget": budget})
}

fn blob_hash_of_ref(evidence_ref: &str) -> Option<&str> {
    let rest = evidence_ref.strip_prefix("z://blob/")?;
    Some(match rest.split_once('#') {
        Some((hash, _)) => hash,
        None => rest,
    })
}

/// Cached snapshot symbol query restricted to `scope` (a relative path prefix). A positive answer
/// is keyed by the exact files that supplied its evidence.
pub fn cached_symbol_query(
    snapshot: &Snapshot,
    cache: &mut WitnessCache,
    symbol: &str,
    scope: &str,
    budget: usize,
) -> Result<CachedAnswer> {
    // Amortize O(|paths|) listing build across hits on the same snapshot.
    let roots = cache.snapshot_roots(snapshot);
    let operator = symbol_query_operator();
    let params = canonical_parameters(symbol, scope, budget);

    if let Some((key_hash_hex, answer)) = cache.lookup(&operator, &params, roots.as_ref()) {
        return Ok(CachedAnswer {
            answer,
            source: AnswerSource::CacheHit,
            key_hash_hex,
        });
    }

    let capsule = snapshot.query(symbol, budget, false)?;
    let mut sites: Vec<AnswerSite> = Vec::new();
    let mut dependency_paths: BTreeMap<String, String> = BTreeMap::new();
    for m in &capsule.matches {
        if m.name != symbol {
            continue;
        }
        for def in &m.defs {
            let Some(hash) = blob_hash_of_ref(&def.evidence_ref) else {
                continue;
            };
            let Some(path) = snapshot.path_for_blob(hash).map(|r| r.path.clone()) else {
                continue;
            };
            if !path.starts_with(scope) {
                continue;
            }
            dependency_paths.insert(path.clone(), hash.to_owned());
            sites.push(AnswerSite {
                path,
                evidence_ref: def.evidence_ref.clone(),
            });
        }
    }
    sites.sort();
    sites.dedup();

    let toolchain_root = roots
        .toolchain_root()
        .map_err(|e| anyhow!("toolchain root: {e}"))?;

    if sites.is_empty() {
        // Certified absence: the searched scope is the anti-dependency.
        let scope_root = roots
            .scope_root(scope)
            .map_err(|e| anyhow!("scope root: {e}"))?;
        let witness = CompletenessWitness::over(vec![scope_root.clone(), toolchain_root.clone()])
            .map_err(|e| anyhow!("completeness witness: {e}"))?;
        let key = CacheKey::with_scope_roots(
            operator,
            params,
            Vec::new(),
            Vec::new(),
            vec![toolchain_root],
            witness,
            vec![scope_root],
        )
        .map_err(|e| anyhow!("negative cache key: {e}"))?;
        let key_hash_hex = cache
            .insert_negative(key)
            .map_err(|e| anyhow!("negative cache entry: {e}"))?;
        return Ok(CachedAnswer {
            answer: None,
            source: AnswerSource::Computed,
            key_hash_hex,
        });
    }

    let mut dependency_roots = Vec::with_capacity(dependency_paths.len());
    for (path, hash) in &dependency_paths {
        dependency_roots
            .push(CacheRoot::file(path, hash).map_err(|e| anyhow!("dependency root: {e}"))?);
    }
    let mut checked = dependency_roots.clone();
    checked.push(toolchain_root.clone());
    let witness =
        CompletenessWitness::over(checked).map_err(|e| anyhow!("completeness witness: {e}"))?;
    let key = CacheKey::new(
        operator,
        params,
        dependency_roots,
        Vec::new(),
        vec![toolchain_root],
        witness,
    )
    .map_err(|e| anyhow!("positive cache key: {e}"))?;
    let answer = SymbolAnswer {
        symbol: symbol.to_owned(),
        sites,
    };
    let key_hash_hex = cache
        .insert_positive(key, &answer)
        .map_err(|e| anyhow!("positive cache entry: {e}"))?;
    Ok(CachedAnswer {
        answer: Some(answer),
        source: AnswerSource::Computed,
        key_hash_hex,
    })
}
