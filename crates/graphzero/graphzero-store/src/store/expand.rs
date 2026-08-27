//! `graphzero expand` resolution chain (FR-017, FR-018, ref-contract.md §6):
//! GraphZero blob store -> git object store (secondary OID key) ->
//! registered external stores (TokenZero recovery cache, when configured).
//! The first two steps make every ref resolvable standalone.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::blob_store::{BlobDigestMismatch, BlobStore};
use super::entity::{EntityId, lookup_entity_with_store};
use super::memory::load_fact;
use super::path_safety::read_queries_file;
use super::path_safety::validate_safe_id;
use super::query::{Snapshot, canonical_ref_for_loc};
use super::ref_index;
use super::refs::{CodeModeExecutionPart, Fragment, GzRef};
use super::zeroref::{ZeroFragment, select_fragment};

/// One step of the resolution trace, for structured errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceStep {
    pub store: &'static str,
    pub result: &'static str,
}

/// Typed expand failure class (graphzero-m3wx). Stable tokens for harnesses;
/// `reason` keeps the human/detail string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpandErrorKind {
    NotFound,
    WrongRoot,
    Expired,
    WorkerSkew,
    DigestMismatch,
    InvalidRef,
    Other,
}

impl ExpandErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::WrongRoot => "wrong_root",
            Self::Expired => "expired",
            Self::WorkerSkew => "worker_skew",
            Self::DigestMismatch => "digest_mismatch",
            Self::InvalidRef => "invalid_ref",
            Self::Other => "other",
        }
    }

    pub fn from_reason(reason: &str) -> Self {
        let head = reason.split(':').next().unwrap_or(reason).trim();
        match head {
            "not_found"
            | "query_not_found"
            | "snapshot_not_found"
            | "mem_not_found"
            | "entity_not_found"
            | "loc_not_found"
            | "codemode_execution_part_not_found"
            | "node_symbol_not_found"
            | "node_symbol_has_no_span" => Self::NotFound,
            "wrong_root" => Self::WrongRoot,
            "expired" => Self::Expired,
            "worker_skew" => Self::WorkerSkew,
            "digest_mismatch" => Self::DigestMismatch,
            "invalid_span"
            | "invalid_entity_id"
            | "invalid_execution_id"
            | "loc_canonical_invalid"
            | "execution_path_invalid"
            | "node_canonical_invalid" => Self::InvalidRef,
            _ if reason.starts_with("not_found") => Self::NotFound,
            _ if reason.starts_with("digest_mismatch") => Self::DigestMismatch,
            _ => Self::Other,
        }
    }
}

#[derive(Debug)]
pub struct ExpandError {
    pub reference: String,
    pub reason: String,
    pub trace: Vec<TraceStep>,
    pub kind: ExpandErrorKind,
}

impl ExpandError {
    pub fn new(reference: impl Into<String>, reason: impl Into<String>) -> Self {
        let reason = reason.into();
        let kind = ExpandErrorKind::from_reason(&reason);
        Self {
            reference: reference.into(),
            reason,
            trace: Vec::new(),
            kind,
        }
    }

    pub fn to_json(&self) -> String {
        let mut s = format!(
            "{{\"error\":\"{}\",\"kind\":\"{}\",\"ref\":\"{}\",\"trace\":[",
            json_escape(&self.reason),
            self.kind.as_str(),
            json_escape(&self.reference)
        );
        for (i, step) in self.trace.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&format!(
                "{{\"store\":\"{}\",\"result\":\"{}\"}}",
                step.store, step.result
            ));
        }
        s.push_str("]}");
        s
    }
}

impl std::fmt::Display for ExpandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Plain reason only — callers that need structured expand diagnostics
        // use `to_json()` / field access. Putting JSON here double-encodes when
        // DomainError / agent_error_json wrap the Display string.
        write!(f, "{}", self.reason)
    }
}

impl std::error::Error for ExpandError {}

fn expand_err(
    reference: impl Into<String>,
    reason: impl Into<String>,
    trace: Vec<TraceStep>,
) -> ExpandError {
    let reason = reason.into();
    ExpandError {
        kind: ExpandErrorKind::from_reason(&reason),
        reference: reference.into(),
        reason,
        trace,
    }
}

pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Resolved bytes plus which store satisfied the lookup.
#[derive(Debug)]
pub struct Resolution {
    pub bytes: Vec<u8>,
    pub source: &'static str,
}

/// Stable failure classes at the external adapter boundary, aligned with the
/// ZeroRef v1 error registry (docs/adr/002-zeroref-v1.md §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalResolveError {
    /// Object not present in this store. The only outcome that lets the
    /// resolution chain continue to the next tier.
    NotFound,
    /// This adapter does not serve the requested ref/scheme. Terminal.
    Unsupported(String),
    /// Underlying I/O failed. Terminal: absence and failure stay distinct.
    Io(String),
    /// Request or stored object violates the exact-identity contract. Terminal.
    Malformed(String),
    /// Returned bytes did not hash to the requested identity. Terminal.
    DigestMismatch { expected: String, actual: String },
    /// Denied by storage policy (e.g. shared root not opted in). Terminal.
    PolicyDenied(String),
}

impl ExternalResolveError {
    /// Stable class token shared with the ZeroRef v1 registry.
    pub fn class(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Unsupported(_) => "unsupported",
            Self::Io(_) => "io",
            Self::Malformed(_) => "malformed",
            Self::DigestMismatch { .. } => "digest_mismatch",
            Self::PolicyDenied(_) => "policy_denied",
        }
    }

    /// Human detail without the class token (Display prepends the class).
    pub fn detail(&self) -> String {
        match self {
            Self::NotFound => "object not present".to_string(),
            Self::Unsupported(msg)
            | Self::Io(msg)
            | Self::Malformed(msg)
            | Self::PolicyDenied(msg) => msg.clone(),
            Self::DigestMismatch { expected, actual } => {
                format!("expected {expected}, got {actual}")
            }
        }
    }
}

impl std::fmt::Display for ExternalResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.class(), self.detail())
    }
}

impl std::error::Error for ExternalResolveError {}

/// Exact-identity blob request at the adapter boundary. Constructing one
/// requires the full lowercase 64-hex SHA-256; prefix resolution is not
/// expressible against external stores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobRequest<'a> {
    sha256: &'a str,
}

impl<'a> BlobRequest<'a> {
    pub fn exact(sha256: &'a str) -> Result<Self, ExternalResolveError> {
        let full_lower_hex = sha256.len() == 64
            && sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !full_lower_hex {
            return Err(ExternalResolveError::Malformed(
                "blob request requires the full lowercase 64-hex sha256".to_string(),
            ));
        }
        Ok(Self { sha256 })
    }

    pub fn sha256(&self) -> &str {
        self.sha256
    }
}

/// Pluggable external store (ref-contract.md §8, ZeroRef v1 §6/§7).
///
/// Contract for implementers:
/// - **Identity:** requests carry a validated full 64-hex SHA-256. Return the
///   complete object bytes for exactly that identity or a typed error. The
///   resolver re-verifies the digest after every hit, so wrong bytes are
///   rejected — but returning them is still a contract violation.
/// - **Errors:** return `NotFound` only for genuine absence; the chain falls
///   through to the next tier on `NotFound` alone. `Io`, `Malformed`,
///   `DigestMismatch`, `PolicyDenied`, and `Unsupported` are terminal so
///   failures and corruption are never masked by a lower tier.
/// - **Ownership/concurrency:** adapters are owned by the resolver
///   (`Box<dyn ExternalStore>`), must be `Send + Sync`, and may be called
///   concurrently from embedded hosts; keep them stateless or internally
///   synchronized. No process-global handles.
/// - **Trust:** never leak blob contents in error messages; identify the
///   store by its short `name()` label, not by unrelated filesystem paths.
pub trait ExternalStore: Send + Sync {
    fn name(&self) -> &'static str;
    fn get(&self, request: &BlobRequest<'_>) -> Result<Vec<u8>, ExternalResolveError>;
}

/// Narrow adapter exposing a local GraphZero [`BlobStore`] root through the
/// [`ExternalStore`] boundary, so embedded and CLI callers can chain legacy
/// store roots without duplicating ref parsing.
pub struct LocalBlobStoreAdapter {
    pub root: PathBuf,
    pub label: &'static str,
}

impl ExternalStore for LocalBlobStoreAdapter {
    fn name(&self) -> &'static str {
        self.label
    }

    fn get(&self, request: &BlobRequest<'_>) -> Result<Vec<u8>, ExternalResolveError> {
        let store = BlobStore::open(&self.root)
            .map_err(|e| ExternalResolveError::Io(format!("open blob store: {e}")))?;
        match store.get_hex(request.sha256()) {
            Ok(Some(bytes)) => Ok(bytes),
            Ok(None) => Err(ExternalResolveError::NotFound),
            Err(e) => Err(ExternalResolveError::Io(format!("read blob: {e}"))),
        }
    }
}

/// Content-addressed directory adapter: looks up `<dir>/<sha256>` (or the
/// TokenZero recovery-cache spelling `<dir>/<sha256>.txt`) by exact identity.
#[cfg(feature = "tokenzero")]
pub struct DirStore {
    pub dir: PathBuf,
    pub label: &'static str,
}

#[cfg(feature = "tokenzero")]
impl ExternalStore for DirStore {
    fn name(&self) -> &'static str {
        self.label
    }

    fn get(&self, request: &BlobRequest<'_>) -> Result<Vec<u8>, ExternalResolveError> {
        for candidate in [
            self.dir.join(request.sha256()),
            self.dir.join(format!("{}.txt", request.sha256())),
        ] {
            match std::fs::read(&candidate) {
                Ok(bytes) => return Ok(bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(ExternalResolveError::Io(format!("read cache entry: {e}"))),
            }
        }
        Err(ExternalResolveError::NotFound)
    }
}

pub struct ExpandResolver {
    blob_store: BlobStore,
    store_root: PathBuf,
    repo_root: Option<PathBuf>,
    externals: Vec<Box<dyn ExternalStore>>,
    /// When set, ref-index hits whose store root is outside this set yield
    /// `wrong_root` instead of following the foreign root (graphzero-m3wx).
    authorized_roots: Option<Vec<PathBuf>>,
    /// Session-bound contract digest expected by the caller (router / harness).
    expected_contract_digest: Option<String>,
    /// Digest this owner process actually serves.
    actual_contract_digest: Option<String>,
    /// Session-bound worker revision expected by the caller.
    expected_worker_revision: Option<String>,
    /// Revision this owner process actually serves.
    actual_worker_revision: Option<String>,
}

impl ExpandResolver {
    /// `store_root` is `.graphzero`; `repo_root` enables the git fallback.
    pub fn new(store_root: &Path, repo_root: Option<&Path>) -> Result<Self> {
        let mut resolver = Self {
            blob_store: BlobStore::open(store_root)?,
            store_root: store_root.to_path_buf(),
            repo_root: repo_root.map(|p| p.to_path_buf()),
            externals: Vec::new(),
            authorized_roots: None,
            expected_contract_digest: None,
            actual_contract_digest: None,
            expected_worker_revision: None,
            actual_worker_revision: None,
        };
        // Project-local canonical CAS (ZeroRef v1 §7) is always chained after
        // the legacy flat store; a cross-project shared CAS joins only under
        // the explicit shared-store opt-in.
        resolver
            .externals
            .push(Box::new(super::shared_cas::SharedCas::open_labeled(
                store_root,
                "cas-local",
            )));
        if super::zerostack_store::shared_store_opt_in_from_env()
            && let Some(root) = super::zerostack_store::STORE_ROOT_ENVS
                .iter()
                .find_map(std::env::var_os)
        {
            resolver
                .externals
                .push(Box::new(super::shared_cas::SharedCas::open_labeled(
                    PathBuf::from(root),
                    "cas-shared",
                )));
        }
        #[cfg(feature = "tokenzero")]
        {
            if let Ok(dir) = std::env::var("GRAPHZERO_TOKENZERO_CACHE") {
                let dir = PathBuf::from(dir);
                if dir.is_dir() {
                    resolver.externals.push(Box::new(DirStore {
                        dir,
                        label: "tokenzero",
                    }));
                }
            }
        }
        Ok(resolver)
    }

    /// Restrict ref-index follow to these store roots (plus this resolver's root).
    pub fn with_authorized_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.authorized_roots = Some(roots);
        self
    }

    /// Bind expected vs actual worker contract identity for `worker_skew` checks.
    pub fn with_worker_identity(
        mut self,
        expected_contract_digest: impl Into<String>,
        actual_contract_digest: impl Into<String>,
        expected_worker_revision: impl Into<String>,
        actual_worker_revision: impl Into<String>,
    ) -> Self {
        self.expected_contract_digest = Some(expected_contract_digest.into());
        self.actual_contract_digest = Some(actual_contract_digest.into());
        self.expected_worker_revision = Some(expected_worker_revision.into());
        self.actual_worker_revision = Some(actual_worker_revision.into());
        self
    }

    pub fn register_external(&mut self, store: Box<dyn ExternalStore>) {
        self.externals.push(store);
    }

    /// Resolve a whole blob by content-sha256 (or legacy prefix) through the
    /// documented tier order: (1) local GraphZero blob store, (2) git object
    /// store via the secondary OID key, (3) registered external stores in
    /// registration order — project-local canonical CAS, then the shared CAS
    /// under explicit opt-in, then any host-registered adapters —
    /// (4) per-user ref-index.
    ///
    /// Digest verification (INV-001, ZeroRef v1 §5) gates every full-hash hit
    /// before bytes reach fragment or presentation code. Corruption and I/O
    /// failure are terminal: the chain falls through to the next tier only on
    /// an explicit miss, so a lower tier can never mask a corrupt object.
    pub fn resolve_blob(&self, hash_hex: &str, reference: &str) -> Result<Resolution, ExpandError> {
        let mut trace = Vec::new();

        // 1. GraphZero blob store.
        match self.blob_store.get_hex(hash_hex) {
            Ok(Some(bytes)) => {
                return digest_gate(bytes, "graphzero", hash_hex, reference, &mut trace);
            }
            Ok(None) => trace.push(TraceStep {
                store: "graphzero",
                result: "miss",
            }),
            // Corruption is terminal (INV-001): a local blob that fails its
            // own content verification must not fall through to a lower tier
            // where a stale or attacker-controlled copy could mask it.
            Err(e) => {
                if let Some(mismatch) = e.downcast_ref::<BlobDigestMismatch>() {
                    trace.push(TraceStep {
                        store: "graphzero",
                        result: "digest_mismatch",
                    });
                    return Err(expand_err(
                        reference.to_string(),
                        format!(
                            "digest_mismatch: store 'graphzero' returned bytes hashing to {}",
                            mismatch.actual
                        ),
                        trace,
                    ));
                }
                trace.push(TraceStep {
                    store: "graphzero",
                    result: "error",
                });
            }
        }

        // 2. Git object store via the secondary OID key.
        match self.git_lookup(hash_hex) {
            Ok(Some(bytes)) => {
                return Ok(Resolution {
                    bytes,
                    source: "git",
                });
            }
            Ok(None) => trace.push(TraceStep {
                store: "git",
                result: "miss",
            }),
            Err(reason) => {
                trace.push(TraceStep {
                    store: "git",
                    result: "digest_mismatch",
                });
                return Err(expand_err(reference.to_string(), reason, trace));
            }
        }

        // 3. Registered external stores: exact identity only, fallible.
        if !self.externals.is_empty() {
            match BlobRequest::exact(hash_hex) {
                Ok(request) => {
                    for ext in &self.externals {
                        match ext.get(&request) {
                            Ok(bytes) => {
                                return digest_gate(
                                    bytes,
                                    ext.name(),
                                    hash_hex,
                                    reference,
                                    &mut trace,
                                );
                            }
                            Err(ExternalResolveError::NotFound) => trace.push(TraceStep {
                                store: ext.name(),
                                result: "miss",
                            }),
                            Err(err) => {
                                trace.push(TraceStep {
                                    store: ext.name(),
                                    result: err.class(),
                                });
                                return Err(expand_err(
                                    reference.to_string(),
                                    format!(
                                        "{}: external store '{}': {}",
                                        err.class(),
                                        ext.name(),
                                        err.detail()
                                    ),
                                    trace,
                                ));
                            }
                        }
                    }
                }
                // Legacy prefix refs cannot address exact-hash adapters.
                Err(_) => trace.push(TraceStep {
                    store: "external",
                    result: "skipped_prefix",
                }),
            }
        } else {
            trace.push(TraceStep {
                store: "tokenzero",
                result: "miss",
            });
        }

        match self.resolve_blob_from_ref_index(hash_hex, reference) {
            Ok(Some(bytes)) => {
                return digest_gate(bytes, "ref-index", hash_hex, reference, &mut trace);
            }
            Ok(None) => {
                trace.push(TraceStep {
                    store: "ref-index",
                    result: "miss",
                });
            }
            Err(err) => return Err(err),
        }

        Err(expand_err(
            reference.to_string(),
            "not_found: tried graphzero, git, tokenzero, ref-index".to_string(),
            trace,
        ))
    }

    fn git_lookup(&self, content_hash_hex: &str) -> Result<Option<Vec<u8>>, String> {
        let Some(repo_root) = self.repo_root.as_ref() else {
            return Ok(None);
        };
        let Some(oid_hex) = self.blob_store.git_oid_for(content_hash_hex).ok().flatten() else {
            return Ok(None);
        };
        let Ok(repo) = git2::Repository::discover(repo_root) else {
            return Ok(None);
        };
        let Ok(oid) = git2::Oid::from_str(&oid_hex) else {
            return Ok(None);
        };
        let Ok(blob) = repo.find_blob(oid) else {
            return Ok(None);
        };
        let bytes = blob.content().to_vec();
        // Content identity must verify (INV-001): a stale or corrupt OID
        // mapping for a full-hash request is terminal, never masked.
        if content_hash_hex.len() == 64 {
            let got = crate::ContentHash::of(&bytes).to_hex();
            if got != content_hash_hex {
                return Err(format!(
                    "digest_mismatch: store 'git' returned bytes hashing to {got}"
                ));
            }
        }
        Ok(Some(bytes))
    }

    /// Resolve any parsed `gz://` ref to its exact bytes.
    pub fn resolve(&self, gz: &GzRef, reference: &str) -> Result<Resolution, ExpandError> {
        self.check_worker_skew(reference)?;
        match gz {
            GzRef::Blob { hash, fragment } => self.resolve_blob_fragment(hash, fragment, reference),
            GzRef::Node { id } | GzRef::Edge { id } => self.resolve_node_or_edge(id, reference),
            GzRef::Query { id } => self.resolve_query(id, reference),
            GzRef::Snap { id } => self.resolve_snap(id, reference),
            GzRef::Loc { id } => self.resolve_loc(*id, reference),
            GzRef::Mem { id } => self.resolve_mem(id, reference),
            GzRef::Entity { id } => self.resolve_entity(id, reference),
            GzRef::CodeModeExecution { id, part } => self.resolve_codemode(id, part, reference),
        }
    }

    fn check_worker_skew(&self, reference: &str) -> Result<(), ExpandError> {
        if let (Some(expected), Some(actual)) = (
            self.expected_contract_digest.as_deref(),
            self.actual_contract_digest.as_deref(),
        ) {
            if expected != actual {
                return Err(self.worker_skew(
                    reference,
                    &format!("contract digest mismatch: expected {expected}, got {actual}"),
                ));
            }
        }
        if let (Some(expected), Some(actual)) = (
            self.expected_worker_revision.as_deref(),
            self.actual_worker_revision.as_deref(),
        ) {
            if expected != actual {
                return Err(self.worker_skew(
                    reference,
                    &format!("worker revision mismatch: expected {expected}, got {actual}"),
                ));
            }
        }
        Ok(())
    }

    fn resolve_blob_fragment(
        &self,
        hash: &str,
        fragment: &Fragment,
        reference: &str,
    ) -> Result<Resolution, ExpandError> {
        let resolution = self.resolve_blob(hash, reference)?;
        let bytes = apply_fragment(&resolution.bytes, fragment).map_err(|reason| {
            expand_err(
                reference.to_string(),
                reason,
                vec![TraceStep {
                    store: resolution.source,
                    result: "hit",
                }],
            )
        })?;
        Ok(Resolution {
            bytes,
            source: resolution.source,
        })
    }

    /// Node/edge ids carry their decl/evidence ref inline as
    /// `<hash>@B<start>-<end>` (see capsule emission); bare ids are either blob
    /// hashes or symbol names.
    ///
    /// Symbol-named node refs (`gz://node/<symbol>`) are minted by orient / snap /
    /// blast whenever an edge carries no inline evidence span. They must resolve
    /// on a later call from the durable snapshot, not only inside the minting
    /// session, so a bare non-hash id is looked up through the locate index.
    fn resolve_node_or_edge(&self, id: &str, reference: &str) -> Result<Resolution, ExpandError> {
        let Some((hash, span)) = id.split_once("@B") else {
            if !is_blob_hash_id(id) {
                return self.resolve_symbol_node(id, reference);
            }
            return self.resolve_blob(id, reference);
        };
        let Some((s, e)) = span.split_once('-') else {
            return Err(self.bad_ref(reference, "invalid_span"));
        };
        let (Ok(start), Ok(end)) = (s.parse::<u64>(), e.parse::<u64>()) else {
            return Err(self.bad_ref(reference, "invalid_span"));
        };
        self.resolve_blob_fragment(hash, &Fragment::Bytes { start, end }, reference)
    }

    /// Resolve `gz://node/<symbol>` through the durable locate index.
    ///
    /// The locate index is rebuilt from the published snapshot, so this survives
    /// process exit and does not depend on any in-process session registry.
    fn resolve_symbol_node(&self, name: &str, reference: &str) -> Result<Resolution, ExpandError> {
        let snapshot = Snapshot::open(&self.store_root, self.repo_root.as_deref())
            .map_err(|_| self.bad_ref(reference, "snapshot_open_failed"))?;
        let index = snapshot
            .locate_index()
            .map_err(|_| self.bad_ref(reference, "locate_index_unavailable"))?;
        let loc_id = index
            .symbol_to_loc
            .get(name)
            .copied()
            .ok_or_else(|| self.bad_ref(reference, "node_symbol_not_found"))?;
        let canonical = index
            .entry(loc_id)
            .map(|entry| entry.canonical_ref.clone())
            .ok_or_else(|| self.bad_ref(reference, "node_symbol_not_found"))?;
        // A symbol whose canonical ref is itself the same bare node ref has no
        // defining span in the snapshot; report that rather than recursing.
        if canonical == reference || canonical == format!("gz://node/{name}") {
            return Err(self.bad_ref(reference, "node_symbol_has_no_span"));
        }
        let inner = GzRef::parse(&canonical)
            .map_err(|_| self.bad_ref(reference, "node_canonical_invalid"))?;
        self.resolve(&inner, reference)
    }

    fn resolve_query(&self, id: &str, reference: &str) -> Result<Resolution, ExpandError> {
        self.check_query_lease(id, reference)?;
        match read_queries_file(&self.store_root, &format!("{id}.json")) {
            Ok(bytes) => Ok(Resolution {
                bytes,
                source: "graphzero",
            }),
            Err(_) => {
                // Owner-routed recovery: BlobStore prefix and/or sha256 sidecar →
                // SharedCas (graphzero-m3wx). Prefer this over opaque handle rewrite.
                if let Some(res) = self.resolve_query_from_owner_cas(id, reference) {
                    return Ok(res);
                }
                self.resolve_query_from_ref_index(id, reference)
                    .map(|bytes| Resolution {
                        bytes,
                        source: "ref-index",
                    })
            }
        }
    }

    /// Optional `queries/<id>.expires_at` sidecar (unix epoch milliseconds).
    /// Absent sidecar means no automatic expiry (graphzero-m3wx / ADR 011).
    fn check_query_lease(&self, id: &str, reference: &str) -> Result<(), ExpandError> {
        let path = self
            .store_root
            .join("queries")
            .join(format!("{id}.expires_at"));
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Ok(());
        };
        let Ok(expires_at_ms) = raw.trim().parse::<u64>() else {
            return Ok(());
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if now_ms > expires_at_ms {
            return Err(self.expired(
                reference,
                &format!("query lease elapsed at {expires_at_ms}ms (now {now_ms}ms)"),
            ));
        }
        Ok(())
    }

    /// Recover a query spill when `queries/<id>.json` is gone but dual-write
    /// bytes (and optional `<id>.sha256` pointer) remain under this store root.
    fn resolve_query_from_owner_cas(&self, id: &str, reference: &str) -> Option<Resolution> {
        // 1. Legacy flat BlobStore by 16-hex query-id prefix.
        if let Ok(Some(bytes)) = self.blob_store.get_hex(id) {
            return Some(Resolution {
                bytes,
                source: "graphzero-blob",
            });
        }
        // 2. Sidecar full digest → resolve_blob (flat → cas-local → shared).
        let sidecar = self.store_root.join("queries").join(format!("{id}.sha256"));
        let full = std::fs::read_to_string(&sidecar).ok()?;
        let full = full.trim();
        if full.len() != 64 || !full.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        match self.resolve_blob(full, reference) {
            Ok(res) => Some(Resolution {
                bytes: res.bytes,
                source: if res.source == "graphzero" {
                    "graphzero-blob"
                } else {
                    res.source
                },
            }),
            Err(_) => None,
        }
    }

    fn resolve_snap(&self, id: &str, reference: &str) -> Result<Resolution, ExpandError> {
        match read_queries_file(&self.store_root, &format!("snap_{id}.json")) {
            Ok(bytes) => Ok(Resolution {
                bytes,
                source: "graphzero",
            }),
            Err(_) => {
                // Snap spills share the query dual-write path when persisted via
                // persist_query_json; recover via owner CAS before failing.
                if let Some(res) = self.resolve_query_from_owner_cas(id, reference) {
                    return Ok(res);
                }
                Err(self.bad_ref(reference, "snapshot_not_found"))
            }
        }
    }

    fn resolve_loc(&self, id: u32, reference: &str) -> Result<Resolution, ExpandError> {
        let snapshot = Snapshot::open(&self.store_root, self.repo_root.as_deref())
            .map_err(|_| self.bad_ref(reference, "snapshot_open_failed"))?;
        let canonical = canonical_ref_for_loc(&snapshot, id)
            .map_err(|_| self.bad_ref(reference, "loc_not_found"))?;
        let inner = GzRef::parse(&canonical)
            .map_err(|_| self.bad_ref(reference, "loc_canonical_invalid"))?;
        self.resolve(&inner, reference)
    }

    fn resolve_mem(&self, id: &str, reference: &str) -> Result<Resolution, ExpandError> {
        let fact = load_fact(&self.store_root, id)
            .map_err(|_| self.bad_ref(reference, "mem_not_found"))?;
        self.json_resolution(&fact, reference)
    }

    fn resolve_entity(&self, id: &str, reference: &str) -> Result<Resolution, ExpandError> {
        let entity_id =
            EntityId::parse(id).map_err(|_| self.bad_ref(reference, "invalid_entity_id"))?;
        let record = lookup_entity_with_store(&self.store_root, &entity_id)
            .map_err(|e| self.bad_ref(reference, &e.to_string()))?
            .ok_or_else(|| self.bad_ref(reference, "entity_not_found"))?;
        self.json_resolution(&record, reference)
    }

    fn resolve_codemode(
        &self,
        id: &str,
        part: &CodeModeExecutionPart,
        reference: &str,
    ) -> Result<Resolution, ExpandError> {
        match self.resolve_codemode_execution(id, part, reference) {
            Ok(bytes) => Ok(Resolution {
                bytes,
                source: "graphzero",
            }),
            Err(_) => self
                .resolve_codemode_execution_from_ref_index(id, part, reference)
                .map(|bytes| Resolution {
                    bytes,
                    source: "ref-index",
                }),
        }
    }

    fn json_resolution<T: serde::Serialize>(
        &self,
        value: &T,
        reference: &str,
    ) -> Result<Resolution, ExpandError> {
        let bytes = serde_json::to_vec_pretty(value)
            .map_err(|e| expand_err(reference.to_string(), e.to_string(), Vec::new()))?;
        Ok(Resolution {
            bytes,
            source: "graphzero",
        })
    }

    fn resolve_codemode_execution(
        &self,
        id: &str,
        part: &CodeModeExecutionPart,
        reference: &str,
    ) -> Result<Vec<u8>, ExpandError> {
        validate_safe_id(id, reference)
            .map_err(|_| self.bad_ref(reference, "invalid_execution_id"))?;
        let dir = self.store_root.join("codemode").join("execution").join(id);
        let canonical_dir = dir
            .parent()
            .and_then(|p| p.canonicalize().ok())
            .filter(|c| c.starts_with(self.store_root.canonicalize().unwrap_or_default()));
        if canonical_dir.is_none() {
            return Err(self.bad_ref(reference, "execution_path_invalid"));
        }
        read_codemode_execution_file(&self.store_root, id, part, reference)
            .map_err(|_| self.bad_ref(reference, "codemode_execution_part_not_found"))
    }

    fn resolve_blob_from_ref_index(
        &self,
        hash_hex: &str,
        reference: &str,
    ) -> Result<Option<Vec<u8>>, ExpandError> {
        let Some(indexed_root) = ref_index::lookup_store(&format!("gz://blob/{hash_hex}")) else {
            return Ok(None);
        };
        if indexed_root == self.store_root {
            return Ok(None);
        }
        self.ensure_indexed_root_authorized(reference, &indexed_root)?;
        Ok(BlobStore::open(&indexed_root)
            .ok()
            .and_then(|store| store.get_hex(hash_hex).ok().flatten()))
    }

    fn resolve_query_from_ref_index(
        &self,
        id: &str,
        reference: &str,
    ) -> Result<Vec<u8>, ExpandError> {
        let canonical = format!("gz://query/{id}");
        let indexed_root = ref_index::lookup_store(reference)
            .or_else(|| ref_index::lookup_store(&canonical))
            .ok_or_else(|| self.index_miss(reference, "query_not_found"))?;
        if indexed_root == self.store_root {
            return Err(self.index_miss(reference, "query_not_found"));
        }
        self.ensure_indexed_root_authorized(reference, &indexed_root)?;
        read_queries_file(&indexed_root, &format!("{id}.json"))
            .map_err(|_| self.index_miss(reference, "query_not_found"))
    }

    fn resolve_codemode_execution_from_ref_index(
        &self,
        id: &str,
        part: &CodeModeExecutionPart,
        reference: &str,
    ) -> Result<Vec<u8>, ExpandError> {
        let canonical = if matches!(part, CodeModeExecutionPart::Execution) {
            format!("gz://codemode/execution/{id}")
        } else {
            format!("gz://codemode/execution/{id}/{}", part.as_str())
        };
        let indexed_root = ref_index::lookup_store(reference)
            .or_else(|| ref_index::lookup_store(&canonical))
            .ok_or_else(|| self.index_miss(reference, "codemode_execution_part_not_found"))?;
        if indexed_root == self.store_root {
            return Err(self.index_miss(reference, "codemode_execution_part_not_found"));
        }
        self.ensure_indexed_root_authorized(reference, &indexed_root)?;
        read_codemode_execution_file(&indexed_root, id, part, reference)
            .map_err(|_| self.index_miss(reference, "codemode_execution_part_not_found"))
    }

    fn root_authorized(&self, indexed_root: &Path) -> bool {
        let Some(allowed) = self.authorized_roots.as_ref() else {
            return true;
        };
        let indexed = canonicalize_loose(indexed_root);
        let self_root = canonicalize_loose(&self.store_root);
        if indexed == self_root {
            return true;
        }
        allowed
            .iter()
            .any(|root| canonicalize_loose(root) == indexed)
    }

    fn ensure_indexed_root_authorized(
        &self,
        reference: &str,
        indexed_root: &Path,
    ) -> Result<(), ExpandError> {
        if self.root_authorized(indexed_root) {
            return Ok(());
        }
        Err(self.wrong_root(
            reference,
            &format!(
                "indexed store {} is not authorized for this session",
                indexed_root.display()
            ),
        ))
    }

    fn index_miss(&self, reference: &str, reason: &str) -> ExpandError {
        expand_err(
            reference.to_string(),
            format!("{reason}: tried current-root store, ref-index"),
            vec![
                TraceStep {
                    store: "graphzero",
                    result: "miss",
                },
                TraceStep {
                    store: "ref-index",
                    result: "miss",
                },
            ],
        )
    }

    fn bad_ref(&self, reference: &str, reason: &str) -> ExpandError {
        expand_err(reference.to_string(), reason.to_string(), Vec::new())
    }

    fn wrong_root(&self, reference: &str, detail: &str) -> ExpandError {
        expand_err(
            reference.to_string(),
            format!("wrong_root: {detail}"),
            vec![TraceStep {
                store: "ref-index",
                result: "wrong_root",
            }],
        )
    }

    fn expired(&self, reference: &str, detail: &str) -> ExpandError {
        expand_err(
            reference.to_string(),
            format!("expired: {detail}"),
            vec![TraceStep {
                store: "graphzero",
                result: "expired",
            }],
        )
    }

    fn worker_skew(&self, reference: &str, detail: &str) -> ExpandError {
        expand_err(
            reference.to_string(),
            format!("worker_skew: {detail}"),
            vec![TraceStep {
                store: "graphzero",
                result: "worker_skew",
            }],
        )
    }
}

/// True when a bare node/edge id is a content hash (full or prefix) rather than
/// a symbol name. Blob ids are lowercase hex; symbol names are not.
fn is_blob_hash_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

fn canonicalize_loose(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn read_codemode_execution_file(
    store_root: &Path,
    id: &str,
    part: &CodeModeExecutionPart,
    reference: &str,
) -> Result<Vec<u8>, ExpandError> {
    validate_safe_id(id, reference).map_err(|_| {
        expand_err(
            reference.to_string(),
            "invalid_execution_id".to_string(),
            Vec::new(),
        )
    })?;
    let dir = store_root.join("codemode").join("execution").join(id);
    let canonical_store = store_root
        .canonicalize()
        .unwrap_or_else(|_| store_root.to_path_buf());
    let canonical_dir = dir
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .filter(|c| c.starts_with(&canonical_store));
    if canonical_dir.is_none() {
        return Err(expand_err(
            reference.to_string(),
            "execution_path_invalid".to_string(),
            Vec::new(),
        ));
    }
    std::fs::read(dir.join(part.as_str())).map_err(|_| {
        expand_err(
            reference.to_string(),
            "codemode_execution_part_not_found".to_string(),
            Vec::new(),
        )
    })
}

/// Digest-gate a full-hash hit before bytes reach fragment or presentation
/// code (INV-001, ZeroRef v1 §5). Legacy prefix requests pass through: their
/// tiers verify by construction (content-addressed filenames) or reject at
/// parse time in ZeroRef v1.
fn digest_gate(
    bytes: Vec<u8>,
    source: &'static str,
    hash_hex: &str,
    reference: &str,
    trace: &mut Vec<TraceStep>,
) -> Result<Resolution, ExpandError> {
    if hash_hex.len() == 64 {
        let got = crate::ContentHash::of(&bytes).to_hex();
        if got != hash_hex {
            trace.push(TraceStep {
                store: source,
                result: "digest_mismatch",
            });
            return Err(expand_err(
                reference.to_string(),
                format!("digest_mismatch: store '{source}' returned bytes hashing to {got}"),
                std::mem::take(trace),
            ));
        }
    }
    Ok(Resolution { bytes, source })
}

/// Slice resolved bytes per fragment. Every expansion surface (CLI, embedded,
/// MCP/CodeMode, store APIs) funnels through the shared ZeroRef v1 selector
/// (docs/adr/002-zeroref-v1.md §3/§4): byte spans are zero-based half-open,
/// line spans one-based inclusive with exact newline retention, bounds error
/// instead of clamping, and `#L` over non-UTF-8 content is a typed error.
/// Errors are class-prefixed strings from the v1 registry.
pub fn apply_fragment(bytes: &[u8], fragment: &Fragment) -> Result<Vec<u8>, String> {
    let (zero_fragment, label) = match *fragment {
        Fragment::None => (ZeroFragment::None, String::new()),
        Fragment::Bytes { start, end } => (
            ZeroFragment::Bytes { start, end },
            format!("#B{start}-{end}"),
        ),
        Fragment::Lines { start, end } => (
            ZeroFragment::Lines { start, end },
            format!("#L{start}-{end}"),
        ),
    };
    select_fragment(bytes, &zero_fragment, &label)
        .map(<[u8]>::to_vec)
        .map_err(|e| e.to_string())
}
