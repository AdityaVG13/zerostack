//! Hermetic memoization of deterministic ops (bead
//! zerostack-racc-caching-output-vz89.6).
//!
//! An unchanged deterministic op (format / parse / index build) should cost a
//! ref lookup plus a gap check, never a re-execution. The key is
//! `id(F_version, id(inputs...))` expressed in the shared cache-entry shape
//! (docs/contracts/cache-entry-v1.md): operator id + locked version, canonical
//! parameters, the *minimum exact* input roots, the explicit environment inputs
//! and the toolchain root. Nothing implicit enters the key, so a hit is
//! provably equivalent to a re-run: same function version + same input refs
//! => same output root.
//!
//! Hermeticity is enforced, not assumed: a request must name a versioned tool
//! and at least one content root, every root must be a `fz://blob/<sha256>`
//! ref, and the environment is whatever the caller declares -- there is no
//! ambient read of `std::env`, no clock and no RNG in the key or the entry.
//!
//! Invalidation follows vz89.5: a candidate hit is verified against the
//! *current* CAS (every input root, the toolchain root, the witness root and
//! the cached output root) before it is reused. Missing evidence is never
//! treated as unchanged, and the gap is cone-scoped to the one key.

use super::cas::{CasError, CasStore};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Wire discriminator for persisted memo entries.
pub const OP_MEMO_SCHEMA: &str = "op-memo/v1";
/// Locked memo-layer version; bump on any change to key derivation.
pub const OP_MEMO_KEY_VERSION: &str = "1";

const MEMO_DIR: &str = "op-memo";
const INDEX_FILE: &str = "index.jsonl";

/// The deterministic ops we fully control today. Build/test stay out until
/// receipts can certify environment closure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoOp {
    Format,
    Parse,
    Index,
}

impl MemoOp {
    /// Stable operator id carried in the cache-entry key.
    pub fn operator_id(self) -> &'static str {
        match self {
            MemoOp::Format => "fszero.op.format",
            MemoOp::Parse => "fszero.op.parse",
            MemoOp::Index => "fszero.op.index",
        }
    }
}

/// Typed memo-layer failures. A refusal is never reported as a hit.
#[derive(Debug)]
pub enum MemoError {
    /// Underlying content store failure.
    Cas(CasError),
    /// Index file could not be read or appended.
    Io {
        context: String,
        source: std::io::Error,
    },
    /// A root is not a `fz://blob/<sha256>` ref.
    MalformedRoot(String),
    /// The request cannot be memoized without hidden inputs.
    NonHermetic { detail: String },
    /// Persisted index line is not a decodable record.
    CorruptIndex { line: usize, detail: String },
    /// The operator itself failed; nothing is cached.
    Compute { detail: String },
}

impl fmt::Display for MemoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemoError::Cas(e) => write!(f, "cas: {e}"),
            MemoError::Io { context, source } => write!(f, "io: {context}: {source}"),
            MemoError::MalformedRoot(r) => write!(f, "malformed content root: {r}"),
            MemoError::NonHermetic { detail } => write!(f, "not hermetic: {detail}"),
            MemoError::CorruptIndex { line, detail } => {
                write!(f, "corrupt memo index at line {line}: {detail}")
            }
            MemoError::Compute { detail } => write!(f, "operator failed: {detail}"),
        }
    }
}

impl std::error::Error for MemoError {}

impl From<CasError> for MemoError {
    fn from(e: CasError) -> Self {
        MemoError::Cas(e)
    }
}

impl MemoError {
    /// Stable class for telemetry and typed refusals.
    pub fn class(&self) -> &'static str {
        match self {
            MemoError::Cas(_) => "cas",
            MemoError::Io { .. } => "io",
            MemoError::MalformedRoot(_) => "malformed_root",
            MemoError::NonHermetic { .. } => "non_hermetic",
            MemoError::CorruptIndex { .. } => "corrupt_index",
            MemoError::Compute { .. } => "compute_failed",
        }
    }
}

/// The versioned tool that implements the op. Both fields enter the key, so a
/// tool upgrade is a miss rather than a stale hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolIdentity {
    pub name: String,
    pub version: String,
}

impl ToolIdentity {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
        }
    }
}

/// A fully declared deterministic op invocation. Everything the operator may
/// observe must appear here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoRequest {
    pub op: MemoOp,
    pub tool: ToolIdentity,
    /// Canonical, order-independent operator parameters.
    pub parameters: BTreeMap<String, String>,
    /// Minimum exact input content roots (`fz://blob/<sha256>`).
    pub input_roots: Vec<String>,
    /// Explicit environment inputs; the ambient process env is never read.
    pub env: BTreeMap<String, String>,
}

impl MemoRequest {
    pub fn new(op: MemoOp, tool: ToolIdentity, input_roots: Vec<String>) -> Self {
        Self {
            op,
            tool,
            parameters: BTreeMap::new(),
            input_roots,
            env: BTreeMap::new(),
        }
    }

    pub fn with_parameter(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.parameters.insert(key.into(), value.into());
        self
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }
}

/// A persisted memo entry: the key identity plus the roots a reuse depends on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoEntry {
    pub schema: String,
    pub key_hash: String,
    pub operator: String,
    pub output_root: String,
    pub dependency_roots: Vec<String>,
    pub toolchain_root: String,
    pub witness_root: String,
}

/// Outcome of a memoized invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoOutcome {
    pub entry: MemoEntry,
    /// True when the key already had a verified output root.
    pub reused: bool,
}

impl MemoOutcome {
    /// Output bytes ref of the op.
    pub fn output_root(&self) -> &str {
        &self.entry.output_root
    }
}

/// Durable memo table: CAS blobs for outputs, append-only index for keys.
pub struct OpMemoStore {
    cas: CasStore,
    index_path: PathBuf,
    entries: BTreeMap<String, MemoEntry>,
}

fn blob_ref(hash: &str) -> String {
    format!("fz://blob/{hash}")
}

fn hash_of_ref(root: &str) -> Result<&str, MemoError> {
    super::cas::full_blob_hash(root).ok_or_else(|| MemoError::MalformedRoot(root.to_string()))
}

impl OpMemoStore {
    /// Open (or create) the memo layer under a ZeroStack store root and replay
    /// the persisted index so hits survive process restarts.
    pub fn open(store_root: &Path) -> Result<Self, MemoError> {
        let dir = store_root.join(MEMO_DIR);
        fs::create_dir_all(&dir).map_err(|source| MemoError::Io {
            context: format!("create {}", dir.display()),
            source,
        })?;
        let mut store = Self {
            cas: CasStore::for_store_root(store_root),
            index_path: dir.join(INDEX_FILE),
            entries: BTreeMap::new(),
        };
        store.replay()?;
        Ok(store)
    }

    fn replay(&mut self) -> Result<(), MemoError> {
        let text = match fs::read_to_string(&self.index_path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(MemoError::Io {
                    context: format!("read {}", self.index_path.display()),
                    source,
                });
            }
        };
        for (n, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: MemoEntry =
                serde_json::from_str(line).map_err(|e| MemoError::CorruptIndex {
                    line: n + 1,
                    detail: e.to_string(),
                })?;
            if entry.schema != OP_MEMO_SCHEMA {
                return Err(MemoError::CorruptIndex {
                    line: n + 1,
                    detail: format!("unknown schema {}", entry.schema),
                });
            }
            self.entries.insert(entry.key_hash.clone(), entry);
        }
        Ok(())
    }

    fn append(&mut self, entry: MemoEntry) -> Result<MemoEntry, MemoError> {
        let line = serde_json::to_string(&entry).map_err(|e| MemoError::CorruptIndex {
            line: 0,
            detail: e.to_string(),
        })?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.index_path)
            .map_err(|source| MemoError::Io {
                context: format!("open {}", self.index_path.display()),
                source,
            })?;
        writeln!(file, "{line}").map_err(|source| MemoError::Io {
            context: format!("append {}", self.index_path.display()),
            source,
        })?;
        self.entries.insert(entry.key_hash.clone(), entry.clone());
        Ok(entry)
    }

    /// Derive the cache-entry key for a request. Public so callers can log
    /// or compare identities without materializing anything.
    pub fn key_hash(&mut self, request: &MemoRequest) -> Result<String, MemoError> {
        Ok(self.key_material(request)?.0)
    }

    /// `(key_hash, sorted dependency roots, toolchain root, witness root)`.
    fn key_material(
        &mut self,
        request: &MemoRequest,
    ) -> Result<(String, Vec<String>, String, String), MemoError> {
        if request.tool.name.trim().is_empty() || request.tool.version.trim().is_empty() {
            return Err(MemoError::NonHermetic {
                detail: "tool name and version must both be pinned".to_string(),
            });
        }
        if request.input_roots.is_empty() {
            return Err(MemoError::NonHermetic {
                detail: "at least one input content root is required".to_string(),
            });
        }
        let mut dependency_roots = Vec::with_capacity(request.input_roots.len());
        for root in &request.input_roots {
            hash_of_ref(root)?;
            dependency_roots.push(root.clone());
        }
        dependency_roots.sort();
        dependency_roots.dedup();

        let toolchain_root = blob_ref(
            &self
                .cas
                .put(format!("{}@{}", request.tool.name, request.tool.version).as_bytes())?
                .hash,
        );
        let environment_roots: Vec<Value> = request
            .env
            .iter()
            .map(|(k, v)| json!({ "name": k, "value": v }))
            .collect();
        let witness_root = blob_ref(
            &self
                .cas
                .put(
                    zero_abi::canonical_json(&json!({
                        "checked_roots": dependency_roots.clone(),
                        "operator": format!("{}@{}", request.op.operator_id(), OP_MEMO_KEY_VERSION),
                        "environment": environment_roots.clone(),
                    }))
                    .as_bytes(),
                )?
                .hash,
        );

        let key = memo_cache_key(
            request,
            &dependency_roots,
            &toolchain_root,
            &witness_root,
            &environment_roots,
        );
        Ok((
            zero_abi::contract_digest_hex(&key),
            dependency_roots,
            toolchain_root,
            witness_root,
        ))
    }

    /// Look up a request without running it. Returns `None` on a key miss or
    /// on a CAS gap in any root the entry depends on.
    pub fn lookup(&mut self, request: &MemoRequest) -> Result<Option<MemoEntry>, MemoError> {
        let (key_hash, _, _, _) = self.key_material(request)?;
        let Some(entry) = self.entries.get(&key_hash).cloned() else {
            return Ok(None);
        };
        match self.verify_entry_roots(&entry)? {
            None => Ok(Some(entry)),
            Some(_cause) => Ok(None),
        }
    }

    /// Verify entry roots against current CAS. `Ok(None)` = verified;
    /// `Ok(Some(cause))` = gap attributed for Q99 miss accounting (fszero-2uhi).
    pub fn verify_entry_roots(
        &self,
        entry: &MemoEntry,
    ) -> Result<Option<super::recovery::CacheMissCause>, MemoError> {
        use super::recovery::CacheMissCause;
        for root in &entry.dependency_roots {
            if !self.cas.contains(hash_of_ref(root)?) {
                return Ok(Some(CacheMissCause::DependencyRootChanged));
            }
        }
        if !self.cas.contains(hash_of_ref(&entry.output_root)?) {
            // Output gap: treat as dependency-side (cached product of inputs).
            return Ok(Some(CacheMissCause::DependencyRootChanged));
        }
        if !self.cas.contains(hash_of_ref(&entry.toolchain_root)?)
            || !self.cas.contains(hash_of_ref(&entry.witness_root)?)
        {
            return Ok(Some(CacheMissCause::WitnessUnverifiable));
        }
        Ok(None)
    }

    /// Lookup that also records miss causes on a RecoveryStore when provided.
    pub fn lookup_with_metrics(
        &mut self,
        request: &MemoRequest,
        metrics: Option<&super::recovery::RecoveryStore>,
    ) -> Result<Option<MemoEntry>, MemoError> {
        let (key_hash, _, _, _) = self.key_material(request)?;
        let Some(entry) = self.entries.get(&key_hash).cloned() else {
            return Ok(None);
        };
        match self.verify_entry_roots(&entry)? {
            None => Ok(Some(entry)),
            Some(cause) => {
                if let Some(store) = metrics {
                    store.note_cache_miss_cause(cause);
                }
                Ok(None)
            }
        }
    }

    /// Run `compute` only when the key has no verified entry. On a hit the op
    /// costs a ref lookup plus the gap check, not a re-execution.
    pub fn memoize<F>(
        &mut self,
        request: &MemoRequest,
        compute: F,
    ) -> Result<MemoOutcome, MemoError>
    where
        F: FnOnce() -> Result<Vec<u8>, String>,
    {
        let (key_hash, dependency_roots, toolchain_root, witness_root) =
            self.key_material(request)?;
        if let Some(entry) = self.entries.get(&key_hash).cloned() {
            if self.verify_entry_roots(&entry)?.is_none() {
                return Ok(MemoOutcome {
                    entry,
                    reused: true,
                });
            }
            // Stale entry: fall through to recompute (cause counted by callers
            // via lookup_with_metrics when they hold RecoveryStore).
        }
        let output = compute().map_err(|detail| MemoError::Compute { detail })?;
        let output_root = blob_ref(&self.cas.put(&output)?.hash);
        let entry = self.append(MemoEntry {
            schema: OP_MEMO_SCHEMA.to_string(),
            key_hash,
            operator: format!("{}@{}", request.op.operator_id(), OP_MEMO_KEY_VERSION),
            output_root,
            dependency_roots,
            toolchain_root,
            witness_root,
        })?;
        Ok(MemoOutcome {
            entry,
            reused: false,
        })
    }

    /// Output bytes for an entry.
    pub fn output_bytes(&self, entry: &MemoEntry) -> Result<Vec<u8>, MemoError> {
        Ok(self.cas.get(hash_of_ref(&entry.output_root)?)?)
    }
}

/// Cache key object aligned with docs/contracts/cache-entry-v1.md. The
/// dependency set is the minimum exact one (declared input blobs only), and the
/// environment is the declared set -- never the ambient process environment.
fn memo_cache_key(
    request: &MemoRequest,
    dependency_roots: &[String],
    toolchain_root: &str,
    witness_root: &str,
    environment_roots: &[Value],
) -> Value {
    // fszero-ojnv: scope_roots mirror the declared input cone (never silent []).
    let scope_roots = super::negative_cache::scope_roots_for_key(dependency_roots);
    json!({
        "operator": { "id": request.op.operator_id(), "version": OP_MEMO_KEY_VERSION },
        "canonical_parameters": request.parameters,
        "minimum_dependency_roots": dependency_roots,
        "environment_roots": environment_roots,
        "toolchain_roots": [toolchain_root],
        "completeness_witness": { "proof_root": witness_root, "checked_roots": dependency_roots },
        "scope_roots": scope_roots,
    })
}
