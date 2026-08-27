//! Embeddable TokenZero store handle (tokenzero-lwt).
//!
//! `TokenZeroStore` is a non-global, in-process handle that owns a durable
//! recovery store and an optional shared CAS tier. It is intended for the
//! future single ZeroStack binary where each engine (FSZero, GraphZero,
//! TokenZero) holds its own isolated store handle while sharing the same
//! canonical CAS layout under a single store root.
//!
//! Store-root resolution delegates to the hub `zero_store` crate
//! (tokenzero-mivh): one algorithm, three engines. The handle captures a
//! `StoreEnv` at construction and derives its cache path, CAS host, store
//! root, and mode from the hub `ResolvedStore`, so the embedded handle and
//! the CLI/doctor facade can never select different files for the same root.
//!
//! The descriptor and contract produced by an embedded handle are derived
//! from the same RecoveryStore/SharedCas state used by the standalone CLI
//! and MCP sessions, so cross-mode behavior cannot drift.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;
use thiserror::Error;

use zero_store::{Engine, ResolvedStore, StoreEnv, StoreMode};

use tokenzero_core::ContentType;

use crate::shared_cas::{SharedCas, SharedCasError};
use crate::{RecoveryStore, ZeroRefBlob, ZeroRefError, ZeroRefFragment, parse_zeroref_v1_blob};

const DESCRIPTOR_SCHEMA_VERSION: &str = "tokenzero.recovery.capability.v1";
const DESCRIPTOR_VERSION: &str = "1.0.0";
static CAS_PUBLICATION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Engine-local opt-in alias passed to the hub store resolver, matching the
/// engine workspace facade so CLI and embedded handles never drift.
const ENGINE_OPT_IN_ALIASES: &[&str] = &["TOKENZERO_SHARED_STORE"];

/// Structured errors for the embeddable `TokenZeroStore` handle.
///
/// Portable full-hash blob refs never fall back to the legacy recovery tier.
/// Callers can distinguish missing objects from corruption, I/O, and policy
/// failures without inspecting free-form strings.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TokenZeroStoreError {
    /// Object is not present in the shared CAS (or no CAS is attached for a
    /// portable full-hash ref).
    #[error("object not found")]
    NotFound,
    /// Complete-object digest verification failed.
    #[error("corruption: object does not match expected hash")]
    Corruption,
    /// Underlying storage operation failed.
    #[error("io error: {0}")]
    Io(String),
    /// Policy denied access (e.g. not a regular file).
    #[error("policy violation")]
    Policy,
    /// Ref string is not a valid ZeroRef v1 portable blob ref.
    #[error("malformed ref")]
    Malformed,
    /// Fragment selector is invalid or out of range.
    #[error("fragment error: {0}")]
    Fragment(String),
    /// `#L` requested on non-UTF-8 content.
    #[error("non-utf8 line fragment")]
    NonUtf8Line,
    /// No shared CAS is attached for an operation that requires one.
    #[error("no shared CAS attached")]
    NoSharedCas,
    /// Payload exceeds the configured object size limit.
    #[error("payload {size} bytes exceeds limit {limit}")]
    PayloadTooLarge { size: u64, limit: u64 },
    /// Publishing was denied by filesystem permissions.
    #[error("shared CAS publish permission denied")]
    PublishPermission,
    /// The canonical publication subtree is blocked by a non-directory path.
    #[error("shared CAS publication path is not contained in a directory tree")]
    PublishContainment,
    /// The canonical destination conflicts with a non-file object.
    #[error("shared CAS publication destination conflicts with an existing object")]
    PublishConflict,
    /// Backward-compatible catch-all for publish failures that predate the
    /// structured categories above.
    #[error("shared CAS publish failed: {0}")]
    Publish(String),
    /// Durable cache directory could not be created.
    #[error("cannot create cache directory: {0}")]
    CacheDir(String),
    /// Legacy recovery expand did not find the ref.
    #[error("ref not found in recovery store")]
    LegacyNotFound,
}

impl From<SharedCasError> for TokenZeroStoreError {
    fn from(err: SharedCasError) -> Self {
        match err {
            SharedCasError::NotFound => Self::NotFound,
            SharedCasError::Corruption => Self::Corruption,
            SharedCasError::Io(e) => Self::Io(e.to_string()),
            SharedCasError::Policy => Self::Policy,
            SharedCasError::InvalidHash(_) => Self::Malformed,
            SharedCasError::Gc(_) => Self::Policy,
        }
    }
}

/// Non-global embeddable TokenZero store handle.
///
/// * Two handles with different workspace roots but different store roots are
///   fully isolated in their durable JSON stores and CAS attachment.
/// * Two handles pointing at the same ZeroStack store root share the same
///   canonical CAS layout (`<root>/blobs/sha256/...`).
/// * The capability descriptor is derived from the same RecoveryStore/SharedCas
///   state used by the standalone CLI/MCP session, so embedded and standalone
///   modes advertise identical contracts.
pub struct TokenZeroStore {
    /// Workspace root that this handle answers filesystem ops relative to.
    /// May be `None` for a store-only handle (e.g. a sibling-engine context).
    pub root: Option<PathBuf>,
    recovery: RecoveryStore,
    shared_cas: Option<SharedCas>,
    /// Mirrors the standalone session's durable-degraded flag: true when the
    /// durable store could not be opened and the handle fell back to in-memory.
    pub durable_degraded: bool,
    /// Unique temporary CAS directory for `in_memory()` handles. Cleaned on drop.
    cas_temp_dir: Option<PathBuf>,
    /// True only for an explicitly supplied CAS or the intentionally shared
    /// temporary CAS created by `in_memory()`. Ambient project-local CAS
    /// detection remains usable internally but is not advertised as shared.
    shared_cas_mode: bool,
    /// Hub-resolved store state for this handle. `None` for memory-only
    /// handles without a workspace root.
    resolved: Option<ResolvedStore>,
}

impl TokenZeroStore {
    /// Open an embedded handle using the same durable-store discovery as the
    /// CLI session, delegating to the hub `zero_store` resolver with the
    /// live process environment:
    /// `<root>/.zerostack/tokenzero/recovery-cache.json` for a project-local
    /// store, `<pin>/projects/<project-key>/tokenzero/recovery-cache.json`
    /// for an opted-in external pin, or `<root>/.tokenzero/recovery-cache.json`
    /// in legacy mode.
    ///
    /// A shared CAS is attached when the effective path follows the unified
    /// layout (local or pinned); legacy `.tokenzero` caches attach only when
    /// a sibling `blobs/` directory exists.
    ///
    /// If the durable store cannot be opened, the handle degrades to an
    /// in-memory store with `durable_degraded` set to `true`.
    pub fn open(root: impl AsRef<Path>) -> Self {
        let root_path = root.as_ref().to_path_buf();
        let env = StoreEnv::from_process(ENGINE_OPT_IN_ALIASES);
        Self::open_with_env(root_path, env)
    }

    /// Like [`Self::open`] with an explicit hub environment, so tests and
    /// sibling-engine callers stay deterministic without mutating process env.
    pub fn open_with_env(root: impl AsRef<Path>, env: StoreEnv) -> Self {
        let root_path = root.as_ref().to_path_buf();
        match Self::try_open_with_env(&root_path, env) {
            Ok(s) => s,
            Err(_e) => Self {
                root: Some(root_path),
                recovery: RecoveryStore::new(None),
                shared_cas: None,
                durable_degraded: true,
                cas_temp_dir: None,
                shared_cas_mode: false,
                resolved: None,
            },
        }
    }

    /// Open the handle, returning `Err` rather than degrading to in-memory.
    pub fn try_open(root: impl AsRef<Path>) -> Result<Self, TokenZeroStoreError> {
        let env = StoreEnv::from_process(ENGINE_OPT_IN_ALIASES);
        Self::try_open_with_env(root, env)
    }

    /// Like [`Self::try_open`] with an explicit hub environment.
    pub fn try_open_with_env(
        root: impl AsRef<Path>,
        env: StoreEnv,
    ) -> Result<Self, TokenZeroStoreError> {
        let root_path = root.as_ref().to_path_buf();
        let resolved = ResolvedStore::resolve(&root_path, Engine::TokenZero, &env);
        let cache_path = resolved
            .engine_file("recovery-cache.json")
            .map_err(|error| {
                TokenZeroStoreError::CacheDir(format!("invalid recovery cache file name: {error}"))
            })?;
        // Hub layout creation (unified root + engine dir, symlink / literal
        // tilde root rejection); report paths stay side-effect-free.
        zero_store::ensure_layout(&resolved).map_err(|error| {
            TokenZeroStoreError::CacheDir(format!("cannot prepare store layout: {error}"))
        })?;
        // Validate the durable target under the resolved containment root: the
        // CAS host for unified layouts (covers the namespaced
        // `projects/<key>/tokenzero` chain) and the engine directory itself in
        // legacy mode.
        let containment_root = match resolved.mode() {
            StoreMode::Legacy => resolved.engine_dir().to_path_buf(),
            _ => resolved.cas_host().to_path_buf(),
        };
        probe_durable_cache_target(&containment_root, &cache_path)?;
        let recovery = RecoveryStore::new(Some(cache_path.clone()));
        let shared_cas = Some(match resolved.mode() {
            StoreMode::Legacy => SharedCas::attach_for_cache_path(&cache_path),
            _ => SharedCas::new(resolved.cas_host().to_path_buf()),
        });
        Ok(Self {
            root: Some(root_path),
            recovery,
            shared_cas,
            durable_degraded: false,
            cas_temp_dir: None,
            shared_cas_mode: false,
            resolved: Some(resolved),
        })
    }

    /// Create an in-memory store handle backed by a temporary shared CAS
    /// directory under the process temp dir. Useful for tests and ephemeral
    /// sibling-engine contexts that do not need a durable store.
    ///
    /// The temporary CAS directory is removed when the handle is dropped.
    pub fn in_memory() -> Self {
        let temp_dir = temp_cas_dir();
        let cas = SharedCas::new(temp_dir.clone());
        Self {
            root: None,
            recovery: RecoveryStore::new(None),
            shared_cas: Some(cas),
            durable_degraded: false,
            cas_temp_dir: Some(temp_dir),
            shared_cas_mode: true,
            resolved: None,
        }
    }

    /// Construct a handle with an explicit shared CAS. This is the path a
    /// sibling engine (FSZero, GraphZero) uses to hand off its CAS object so
    /// all engines in a single ZeroStack process publish/resolve to the same
    /// immutable object tier.
    ///
    /// If `root` is provided, a durable recovery cache is opened at the
    /// conventional TokenZero path resolved by the hub under that root. If
    /// directory creation fails, the handle is returned with an in-memory
    /// recovery store and `durable_degraded = true` rather than silently
    /// advertising durability. If `root` is `None`, the handle is memory-only
    /// for recovery metadata.
    pub fn with_shared_cas(root: Option<PathBuf>, shared_cas: SharedCas) -> Self {
        let env = StoreEnv::from_process(ENGINE_OPT_IN_ALIASES);
        Self::with_shared_cas_and_env(root, shared_cas, env)
    }

    /// Like [`Self::with_shared_cas`] with an explicit hub environment.
    pub fn with_shared_cas_and_env(
        root: Option<PathBuf>,
        shared_cas: SharedCas,
        env: StoreEnv,
    ) -> Self {
        let (recovery, durable_degraded, resolved) = match &root {
            Some(root_path) => {
                let resolved = ResolvedStore::resolve(root_path, Engine::TokenZero, &env);
                let cache_path =
                    match resolved
                        .engine_file("recovery-cache.json")
                        .map_err(|error| {
                            TokenZeroStoreError::CacheDir(format!(
                                "invalid recovery cache file name: {error}"
                            ))
                        }) {
                        Ok(path) => path,
                        Err(_) => {
                            return Self {
                                root,
                                recovery: RecoveryStore::new(None),
                                shared_cas: Some(shared_cas),
                                durable_degraded: true,
                                cas_temp_dir: None,
                                shared_cas_mode: true,
                                resolved: None,
                            };
                        }
                    };
                let containment_root = match resolved.mode() {
                    StoreMode::Legacy => resolved.engine_dir().to_path_buf(),
                    _ => resolved.cas_host().to_path_buf(),
                };
                // Hub layout creation; any failure degrades the handle to
                // in-memory recovery with `durable_degraded = true`, matching
                // the pre-existing degraded contract.
                let usable = match zero_store::ensure_layout(&resolved) {
                    Ok(()) => probe_durable_cache_target(&containment_root, &cache_path).is_ok(),
                    Err(_) => false,
                };
                if usable {
                    (RecoveryStore::new(Some(cache_path)), false, Some(resolved))
                } else {
                    (RecoveryStore::new(None), true, Some(resolved))
                }
            }
            None => (RecoveryStore::new(None), false, None),
        };
        Self {
            root,
            recovery,
            shared_cas: Some(shared_cas),
            durable_degraded,
            cas_temp_dir: None,
            shared_cas_mode: true,
            resolved,
        }
    }

    /// Access the underlying recovery store. This is an escape hatch; most
    /// callers should use the typed methods on this handle.
    pub fn recovery(&self) -> &RecoveryStore {
        &self.recovery
    }

    /// Mutable access to the underlying recovery store.
    pub fn recovery_mut(&mut self) -> &mut RecoveryStore {
        &mut self.recovery
    }

    /// Borrow the shared CAS attached to this handle, if any.
    pub fn shared_cas(&self) -> Option<&SharedCas> {
        self.shared_cas.as_ref()
    }

    /// Store root for this handle, derived from the hub resolution: the
    /// unified store root (project-local `.zerostack` or the resolved pin)
    /// when one resolves, otherwise the legacy engine directory. `None` for
    /// memory-only handles.
    pub fn store_root(&self) -> Option<PathBuf> {
        let resolved = self.resolved.as_ref()?;
        Some(match resolved.unified_root() {
            Some(root) => root.to_path_buf(),
            None => resolved.engine_dir().to_path_buf(),
        })
    }

    /// Persist a byte payload to the shared CAS and return a durable
    /// `tz://blob/<sha256>` portable ref. Honors `max_object_bytes` if
    /// supplied.
    ///
    /// Requires a shared CAS to be attached. Callers that need to publish
    /// without a durable project root should use [`Self::in_memory`] or
    /// [`Self::with_shared_cas`].
    pub fn put(
        &mut self,
        bytes: &[u8],
        max_object_bytes: Option<u64>,
    ) -> Result<String, TokenZeroStoreError> {
        if let Some(limit) = max_object_bytes
            && bytes.len() as u64 > limit
        {
            return Err(TokenZeroStoreError::PayloadTooLarge {
                size: bytes.len() as u64,
                limit,
            });
        }
        let cas = self
            .shared_cas
            .as_ref()
            .ok_or(TokenZeroStoreError::NoSharedCas)?;
        let _publication_guard = cas_publication_guard()?;
        validate_publication_target(cas, bytes)?;
        let hash = cas.publish(bytes).map_err(classify_publish_error)?;
        Ok(format!("tz://blob/{hash}"))
    }

    /// Expand a ref to its exact bytes.
    ///
    /// Portable full-hash `tz://blob/<sha256>` refs (and their `fz://blob/` /
    /// `gz://blob/` aliases), including optional `#B`/`#L` fragment selectors,
    /// are resolved from the shared CAS when one is attached:
    /// 1. The whole object is verified against the full hash.
    /// 2. Fragment selectors are applied only after verification.
    /// 3. Missing/corruption/I/O/policy failures are returned as typed errors.
    /// 4. Valid full-hash refs never fall back to the legacy recovery store.
    ///
    /// Non-portable / legacy refs fall back to the RecoveryStore expand path.
    pub fn expand(&mut self, r: &str) -> Result<Vec<u8>, TokenZeroStoreError> {
        // Classify the bare identity without the fragment so a valid full-hash
        // ref with a bad selector yields Fragment errors, not Malformed.
        let (bare, fragment) = r.split_once('#').map_or((r, None), |(b, f)| (b, Some(f)));

        match parse_zeroref_v1_blob(bare, None) {
            Ok(mut parsed) => {
                if let Some(frag) = fragment {
                    // Dedicated fragment taxonomy before any CAS I/O.
                    parsed.fragment = Some(parse_fragment_to_zeroref(frag)?);
                } else {
                    parsed.fragment = None;
                }
                self.expand_portable_full_hash(parsed)
            }
            // Legacy short refs and non-blob kinds may use the recovery tier
            // (which owns its own fragment handling for legacy IDs).
            Err(ZeroRefError::LegacyAmbiguity) | Err(ZeroRefError::Unsupported) => {
                self.expand_legacy(r)
            }
            Err(ZeroRefError::Malformed)
            | Err(ZeroRefError::Missing)
            | Err(ZeroRefError::Io)
            | Err(ZeroRefError::Corruption)
            | Err(ZeroRefError::Policy)
            | Err(ZeroRefError::IncompatibleVersion) => Err(TokenZeroStoreError::Malformed),
        }
    }

    fn expand_portable_full_hash(
        &mut self,
        parsed: ZeroRefBlob,
    ) -> Result<Vec<u8>, TokenZeroStoreError> {
        let cas = self
            .shared_cas
            .as_ref()
            .ok_or(TokenZeroStoreError::NotFound)?;

        // Whole-object verification first — never fragment before integrity.
        let bytes = cas.resolve(&parsed.hash)?;

        match parsed.fragment {
            None => Ok(bytes),
            Some(fragment) => apply_fragment_to_bytes(&bytes, &fragment),
        }
    }

    fn expand_legacy(&mut self, r: &str) -> Result<Vec<u8>, TokenZeroStoreError> {
        let result = self.recovery.expand(r, Some("raw"), None, None, None, None);
        if result.found {
            Ok(result.content.into_bytes())
        } else {
            // Surface structured recovery reasons when they match the CAS taxonomy.
            match result.reason.as_str() {
                "shared-cas-missing" | "ref-not-found" => Err(TokenZeroStoreError::NotFound),
                "shared-cas-corruption" => Err(TokenZeroStoreError::Corruption),
                "shared-cas-io" => Err(TokenZeroStoreError::Io(result.reason)),
                "shared-cas-policy" => Err(TokenZeroStoreError::Policy),
                reason if reason.starts_with("fragment-") || reason.starts_with("window-") => {
                    Err(TokenZeroStoreError::Fragment(reason.to_string()))
                }
                _ => Err(TokenZeroStoreError::LegacyNotFound),
            }
        }
    }

    /// Probe whether the attached shared CAS can actually accept writes.
    ///
    /// Attachment alone is not writability: a read-only mount or permission
    /// failure must report `shared_cas_writable = false`.
    pub fn cas_writable(&self) -> bool {
        self.shared_cas_mode && self.shared_cas.as_ref().is_some_and(probe_cas_writable)
    }

    fn durable_usable(&self) -> bool {
        if self.durable_degraded {
            return false;
        }
        let Some(cache) = self.recovery.persistence_path.as_deref() else {
            return false;
        };
        let Some(resolved) = self.resolved.as_ref() else {
            return false;
        };
        let containment_root = match resolved.mode() {
            StoreMode::Legacy => resolved.engine_dir(),
            _ => resolved.cas_host(),
        };
        probe_durable_cache_target(containment_root, cache).is_ok()
    }

    /// Classify the effective store layout from the hub `StoreMode`.
    /// Preserves the historical `unified`/`legacy`/`memory` vocabulary while
    /// the exact hub mode is available via [`Self::store_mode`].
    pub fn effective_root_mode(&self) -> &'static str {
        match self.resolved.as_ref().map(|resolved| resolved.mode()) {
            Some(StoreMode::LocalUnified)
            | Some(StoreMode::PinnedInsideProject)
            | Some(StoreMode::SharedNamespaced) => "unified",
            Some(StoreMode::Legacy) => "legacy",
            None => "memory",
        }
    }

    /// Exact hub store-mode wire label, additive telemetry. `None` for
    /// memory-only handles.
    pub fn store_mode(&self) -> Option<&'static str> {
        self.resolved
            .as_ref()
            .map(|resolved| resolved.mode().as_str())
    }

    /// Hub project key when the store is shared-namespaced; `None` otherwise.
    pub fn store_project_key(&self) -> Option<&str> {
        self.resolved
            .as_ref()
            .and_then(|resolved| resolved.project_key())
    }

    /// ZeroRef v1 capability descriptor for this handle. Static fields come
    /// from RecoveryStore/SharedCas constants; the `shared_cas` section is
    /// probed live so a caller can distinguish local-only, shared, and
    /// degraded states before routing any payload.
    pub fn capability_descriptor(&self) -> Value {
        let cas_attached = self.shared_cas_mode && self.shared_cas.is_some();
        let cas_writable = self.cas_writable();
        serde_json::json!({
            "schema_version": DESCRIPTOR_SCHEMA_VERSION,
            "descriptor_version": DESCRIPTOR_VERSION,
            "engine": "tokenzero",
            "zeroref": {
                "version": "v1",
                "enabled": true,
                "shared_cas": cas_attached,
                "shared_cas_writable": cas_writable,
                "blob_ref_expand": true,
                "ref_schemes": ["tz://", "fz://", "gz://"],
                "fragment_selectors": ["#B", "#L"],
                "features": [
                    "shared-content-addressable-storage",
                    "blob-ref-expand",
                    "fragment-selectors"
                ]
            },
            "recovery": {
                "durable": self.durable_usable(),
                "durable_degraded": self.durable_degraded,
                "persistent_path": self
                    .recovery
                    .persistence_path
                    .as_ref()
                    .map(|p| redact_path_identity(p)),
                "store_root": self.store_root().as_ref().map(|p| redact_path_identity(p))
            }
        })
    }

    /// Store health, root, and CAS summary for telemetry and the single-binary
    /// router. Does not leak absolute private paths — only non-reversible
    /// path identities and structural mode labels.
    pub fn root_report(&self) -> Value {
        let store_root = self
            .store_root()
            .map(|p| redact_path_identity(&p))
            .unwrap_or_else(|| "memory".to_string());
        let store_db = self
            .recovery
            .persistence_path
            .as_ref()
            .map(|p| redact_path_identity(p))
            .unwrap_or_else(|| "memory".to_string());
        let workspace_root = self
            .root
            .as_ref()
            .map(|p| redact_path_identity(p))
            .unwrap_or_else(|| "(none)".to_string());
        let effective_root_mode = self.effective_root_mode();
        let cas_attached = self.shared_cas_mode && self.shared_cas.is_some();
        let cas_writable = self.cas_writable();
        let cap = self.capability_descriptor();
        serde_json::json!({
            "workspace_root": workspace_root,
            "store_root": store_root,
            "store_db": store_db,
            "durable_degraded": self.durable_degraded,
            "effective_root_mode": effective_root_mode,
            "store_mode": self.store_mode(),
            "store_project_key": self.store_project_key(),
            "store_health": {
                "durable": self.durable_usable(),
                "cas_attached": cas_attached,
                "cas_writable": cas_writable,
            },
            "capabilities": cap,
            "last_integrity_error": null,
        })
    }

    /// Publish the capability descriptor through the store mode that this
    /// handle actually represents. Explicit and in-memory shared-CAS handles
    /// publish to that CAS; ambient/default isolated handles retain the
    /// recovery-store publication path. Best-effort; never blocks.
    pub fn publish_capabilities(&mut self) {
        let descriptor = self.capability_descriptor().to_string();
        if self.shared_cas_mode {
            let _ = self.put(descriptor.as_bytes(), None);
        } else {
            let _ = self
                .recovery
                .store_blob(&descriptor, ContentType::JsonConfig);
            // Deferred CAS (zerostack-5u7): put_blob no longer publishes to
            // CAS during staging. TokenZeroStore::expand for full-hash refs
            // resolves from CAS only (no inline fallback), so publish_pending_cas
            // is required here to keep the capability descriptor resolvable.
            let _ = self.recovery.publish_pending_cas();
        }
    }
}

impl Drop for TokenZeroStore {
    fn drop(&mut self) {
        if let Some(dir) = self.cas_temp_dir.take() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// Unique probe filename helper.
fn unique_probe_name(kind: &str) -> String {
    format!(
        ".{kind}-{}-{}",
        std::process::id(),
        crate::shared_cas::unique_suffix()
    )
}

fn reject_symlinks_below(root: &Path, path: &Path) -> Result<(), TokenZeroStoreError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| TokenZeroStoreError::PublishContainment)?;
    let mut candidate = root.to_path_buf();
    for component in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(component) = component {
            candidate.push(component.as_os_str());
        }
        match std::fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(TokenZeroStoreError::PublishContainment);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(TokenZeroStoreError::Io(error.to_string())),
        }
    }
    Ok(())
}

fn probe_durable_cache_target(
    containment_root: &Path,
    cache_path: &Path,
) -> Result<(), TokenZeroStoreError> {
    if cache_path.file_name().and_then(|name| name.to_str()) != Some("recovery-cache.json") {
        return Err(TokenZeroStoreError::CacheDir(
            "noncanonical recovery cache filename".to_string(),
        ));
    }
    let parent = cache_path.parent().ok_or_else(|| {
        TokenZeroStoreError::CacheDir("invalid cache path: no parent directory".to_string())
    })?;
    reject_symlinks_below(containment_root, parent).map_err(|error| {
        TokenZeroStoreError::CacheDir(format!("cache parent is noncanonical: {error}"))
    })?;
    let canonical_root = std::fs::canonicalize(containment_root).map_err(|error| {
        TokenZeroStoreError::CacheDir(format!("containment root is unavailable: {error}"))
    })?;
    let canonical_parent = std::fs::canonicalize(parent).map_err(|error| {
        TokenZeroStoreError::CacheDir(format!("cache parent is unavailable: {error}"))
    })?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err(TokenZeroStoreError::CacheDir(
            "cache parent escapes canonical containment root".to_string(),
        ));
    }
    match std::fs::symlink_metadata(cache_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(TokenZeroStoreError::CacheDir(
                "cache target is not a canonical regular file".to_string(),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(TokenZeroStoreError::CacheDir(format!(
                "cache target is unavailable: {error}"
            )));
        }
    }
    let probe = parent.join(unique_probe_name("recovery-cache-write-probe"));
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)?;
        file.write_all(b"tokenzero durable cache probe")?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        std::fs::remove_file(&probe)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&probe);
    }
    result.map_err(|error| {
        TokenZeroStoreError::CacheDir(format!("cache sibling is not fully writable: {error}"))
    })
}

fn publication_hash_prefix(hash: &str) -> Result<&str, TokenZeroStoreError> {
    hash.get(..2).ok_or(TokenZeroStoreError::Malformed)
}

fn prepare_canonical_prefix(
    cas: &SharedCas,
    prefix: &str,
) -> Result<(PathBuf, [bool; 3]), TokenZeroStoreError> {
    match std::fs::symlink_metadata(cas.root()) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(TokenZeroStoreError::PublishContainment);
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(cas.root()).map_err(classify_io_publish_error)?;
        }
        Err(error) => return Err(classify_io_publish_error(error)),
    }
    let metadata = std::fs::symlink_metadata(cas.root()).map_err(classify_io_publish_error)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(TokenZeroStoreError::PublishContainment);
    }
    let mut canonical_parent = std::fs::canonicalize(cas.root())
        .map_err(|error| TokenZeroStoreError::Io(error.to_string()))?;
    let blobs = cas.root().join("blobs");
    let sha256 = blobs.join("sha256");
    let prefix_dir = sha256.join(prefix);
    let existed = [blobs.exists(), sha256.exists(), prefix_dir.exists()];
    for child in [&blobs, &sha256, &prefix_dir] {
        match std::fs::symlink_metadata(child) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(TokenZeroStoreError::PublishContainment);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::create_dir(child) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(classify_io_publish_error(error)),
                }
            }
            Err(error) => return Err(classify_io_publish_error(error)),
        }
        let metadata = std::fs::symlink_metadata(child).map_err(classify_io_publish_error)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(TokenZeroStoreError::PublishContainment);
        }
        let canonical_child = std::fs::canonicalize(child)
            .map_err(|error| TokenZeroStoreError::Io(error.to_string()))?;
        if canonical_child.parent() != Some(canonical_parent.as_path()) {
            return Err(TokenZeroStoreError::PublishContainment);
        }
        canonical_parent = canonical_child;
    }
    Ok((prefix_dir, existed))
}

fn validate_publication_target(cas: &SharedCas, bytes: &[u8]) -> Result<(), TokenZeroStoreError> {
    let hash = crate::shared_cas::content_sha256_hex(bytes);
    let prefix = publication_hash_prefix(&hash)?;
    let target = cas
        .root()
        .join("blobs")
        .join("sha256")
        .join(prefix)
        .join(&hash);
    let _ = prepare_canonical_prefix(cas, prefix)?;
    match std::fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(TokenZeroStoreError::PublishConflict)
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(classify_io_publish_error(error)),
    }
}

fn classify_io_publish_error(error: std::io::Error) -> TokenZeroStoreError {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => TokenZeroStoreError::PublishPermission,
        std::io::ErrorKind::NotADirectory | std::io::ErrorKind::IsADirectory => {
            TokenZeroStoreError::PublishContainment
        }
        std::io::ErrorKind::AlreadyExists => TokenZeroStoreError::PublishConflict,
        _ => TokenZeroStoreError::Io(error.to_string()),
    }
}

fn classify_publish_error(error: SharedCasError) -> TokenZeroStoreError {
    match error {
        SharedCasError::Io(error) => classify_io_publish_error(error),
        SharedCasError::Policy => TokenZeroStoreError::PublishConflict,
        other => other.into(),
    }
}

/// Non-reversible path identity for telemetry. Never emits absolute/private
/// path bytes or reversible encodings.
fn redact_path_identity(path: &Path) -> String {
    // Hash OS bytes when available so distinct paths stay distinct without
    // embedding the original string.
    #[cfg(unix)]
    let digest = {
        use std::os::unix::ffi::OsStrExt;
        crate::shared_cas::content_sha256_hex(path.as_os_str().as_bytes())
    };
    #[cfg(not(unix))]
    let digest = crate::shared_cas::content_sha256_hex(path.to_string_lossy().as_bytes());
    format!("path:{}", &digest[..16])
}

/// Establish actual CAS writability by attempting a tiny create/write/delete
/// probe under the CAS root. Attachment is not sufficient.
fn cas_publication_guard() -> Result<std::sync::MutexGuard<'static, ()>, TokenZeroStoreError> {
    CAS_PUBLICATION_LOCK
        .lock()
        .map_err(|_| TokenZeroStoreError::Io("CAS publication lock poisoned".into()))
}

fn probe_cas_writable(cas: &SharedCas) -> bool {
    let Ok(_publication_guard) = cas_publication_guard() else {
        return false;
    };
    let prefix_seed = unique_probe_name("cas-prefix");
    let hash = crate::shared_cas::content_sha256_hex(prefix_seed.as_bytes());
    let Ok((prefix, existed)) = prepare_canonical_prefix(cas, &hash[..2]) else {
        return false;
    };
    let probe = prefix.join(unique_probe_name("cas-write-probe"));
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)?;
        file.write_all(b"tokenzero CAS write probe")?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        std::fs::remove_file(&probe)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&probe);
    }
    if !existed[2] {
        let _ = std::fs::remove_dir(&prefix);
    }
    if !existed[1] {
        let _ = std::fs::remove_dir(cas.root().join("blobs").join("sha256"));
    }
    if !existed[0] {
        let _ = std::fs::remove_dir(cas.root().join("blobs"));
    }
    result.is_ok()
}

/// Apply a verified whole-object fragment selector. Byte ranges are zero-based
/// half-open; line ranges are one-based inclusive with exact newline retention.
fn apply_fragment_to_bytes(
    bytes: &[u8],
    fragment: &ZeroRefFragment,
) -> Result<Vec<u8>, TokenZeroStoreError> {
    match fragment {
        ZeroRefFragment::Byte { start, end } => {
            if *end > bytes.len() {
                return Err(TokenZeroStoreError::Fragment(format!(
                    "fragment-out-of-range; start={start} end={end} len={}",
                    bytes.len()
                )));
            }
            if start > end {
                return Err(TokenZeroStoreError::Fragment(
                    "fragment-reversed".to_string(),
                ));
            }
            Ok(bytes[*start..*end].to_vec())
        }
        ZeroRefFragment::Line { start, end } => {
            let text = std::str::from_utf8(bytes).map_err(|_| TokenZeroStoreError::NonUtf8Line)?;
            let segments: Vec<&str> = text.split_inclusive('\n').collect();
            // Must match RecoveryStore::content_line_count: empty/0-byte blobs
            // have 0 lines, not one empty split_inclusive remainder.
            let line_count = crate::content_line_count(text);
            if *start == 0 {
                return Err(TokenZeroStoreError::Fragment(
                    "fragment-malformed".to_string(),
                ));
            }
            if start > end {
                return Err(TokenZeroStoreError::Fragment(
                    "fragment-reversed".to_string(),
                ));
            }
            // zzmd.1 (CC1-R1-001): clamp end-past-EOF exactly like
            // RecoveryStore::clamp_line_window — start past EOF stays a typed
            // error, but an overshot end returns the available suffix.
            if *start > line_count {
                return Err(TokenZeroStoreError::Fragment(format!(
                    "fragment-out-of-range; start={start} end={end} lines={line_count}"
                )));
            }
            let lo = start - 1;
            let hi = (*end).min(line_count);
            Ok(segments[lo..hi].concat().into_bytes())
        }
    }
}

/// Parse and validate a #B/#L fragment into a ZeroRefFragment.
/// Delegates to the single shared fragment parser (`parse_fragment_spec`)
/// so RecoveryStore and the embedded TokenZeroStore cannot diverge on the
/// fragment grammar (CC1-R4-005).
fn parse_fragment_to_zeroref(fragment: &str) -> Result<ZeroRefFragment, TokenZeroStoreError> {
    match crate::parse_fragment_spec(fragment) {
        Ok(crate::FragmentSpec::Byte { start, end }) => Ok(ZeroRefFragment::Byte { start, end }),
        Ok(crate::FragmentSpec::Line { start, end }) => Ok(ZeroRefFragment::Line { start, end }),
        Err(err) => Err(TokenZeroStoreError::Fragment(
            crate::fragment_error_reason(err).to_string(),
        )),
    }
}

fn temp_cas_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "tokenzero-in-memory-cas-{}",
        crate::shared_cas::unique_suffix()
    ))
}
