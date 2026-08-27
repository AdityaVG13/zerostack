//! Canonical ZeroStack store-root resolution and on-disk layout.
//!
//! One algorithm, three engines. Each engine contributes only its own
//! directory names and its own opt-in alias environment variables;
//! precedence, project namespacing, and path shapes are owned here, so two
//! engines can never disagree about where a logical object lives.
//!
//! Before this module existed, TokenZero, FSZero, and GraphZero each carried
//! their own resolver and had already drifted in ways that split a single
//! logical store: FSZero honored the environment pin ahead of a project-local
//! store and without any opt-in gate, TokenZero applied no project
//! namespacing at all (so unrelated projects sharing one pin collided on one
//! path), and the three disagreed on the project-key shape. The canonical
//! rules below are the reconciliation, with the majority behavior winning
//! except where only one engine was correct.

use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

/// Invalid engine-local file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineFileError {
    name: String,
}

impl std::fmt::Display for EngineFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "engine file name must be exactly one normal basename component: {:?}",
            self.name
        )
    }
}
impl std::error::Error for EngineFileError {}

fn validate_engine_file_name(name: &str) -> Result<(), EngineFileError> {
    let mut components = Path::new(name).components();
    let valid = matches!(components.next(), Some(Component::Normal(component)) if component == OsStr::new(name))
        && components.next().is_none()
        && !name.contains(['/', '\\'])
        && !matches!(name.as_bytes(), [drive, b':', ..] if drive.is_ascii_alphabetic());
    valid.then_some(()).ok_or_else(|| EngineFileError {
        name: name.to_owned(),
    })
}

/// Store-root pin variables, in precedence order. First non-empty wins.
pub const STORE_ROOT_ENVS: &[&str] = &["ZEROSTACK_STORE_ROOT", "ZERO_STACK_STORE_ROOT"];

/// Cross-engine shared-store opt-in. Engines additionally accept their own
/// alias (`TOKENZERO_SHARED_STORE` and friends).
pub const SHARED_STORE_OPT_IN_ENV: &str = "ZEROSTACK_SHARED_STORE";

/// Directory name of a project-local unified store.
pub const LOCAL_STORE_DIR: &str = ".zerostack";

/// Namespace directory used when the resolved store lives outside the project.
pub const PROJECTS_DIR: &str = "projects";

/// CAS directory name, relative to a resolved store root. Digest-addressed
/// objects are shared per store root and are never project-namespaced.
pub const BLOBS_DIR: &str = "blobs";

/// Hex characters retained from the sha256 project key.
pub const PROJECT_KEY_HEX_LEN: usize = 16;

/// Which engine is asking. Owns the engine namespace under `.zerostack/` and
/// the legacy directory recognized only for migration-safe compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Engine {
    TokenZero,
    FsZero,
    GraphZero,
}

impl Engine {
    /// Subdirectory under a resolved store root.
    pub const fn dir_name(self) -> &'static str {
        match self {
            Self::TokenZero => "tokenzero",
            Self::FsZero => "fszero",
            Self::GraphZero => "graphzero",
        }
    }

    /// Legacy per-repository directory, recognized only when it already exists.
    pub const fn legacy_dir_name(self) -> &'static str {
        match self {
            Self::TokenZero => ".tokenzero",
            Self::FsZero => ".fszero",
            Self::GraphZero => ".graphzero",
        }
    }

    /// Engine identity as it appears in the shared GC record protocol.
    pub const fn as_str(self) -> &'static str {
        self.dir_name()
    }
}

/// Everything resolution needs from the environment, captured so the pure
/// entry points read no process state and are therefore deterministic and
/// testable without mutating globals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreEnv {
    /// First non-empty value of [STORE_ROOT_ENVS]. An exported-but-empty
    /// variable is a shell artifact, not an instruction, so it is discarded
    /// here rather than resolving to the repository root.
    pub pin: Option<OsString>,
    /// True when [SHARED_STORE_OPT_IN_ENV] or an engine alias is truthy.
    pub shared_opt_in: bool,
}

impl StoreEnv {
    /// Read the process environment. `opt_in_aliases` lets each engine keep
    /// its own opt-in variable while precedence and truthiness live here.
    pub fn from_process(opt_in_aliases: &[&str]) -> Self {
        let pin = STORE_ROOT_ENVS
            .iter()
            .filter_map(std::env::var_os)
            .find(|v| !v.is_empty());
        let shared_opt_in = std::iter::once(&SHARED_STORE_OPT_IN_ENV)
            .chain(opt_in_aliases.iter())
            .any(|k| {
                std::env::var_os(k)
                    .map(|v| Self::is_truthy(&v))
                    .unwrap_or(false)
            });
        Self { pin, shared_opt_in }
    }

    /// Build from explicit values, applying the empty-is-unset rule.
    pub fn new(pin: Option<OsString>, shared_opt_in: bool) -> Self {
        Self {
            pin: pin.filter(|v| !v.is_empty()),
            shared_opt_in,
        }
    }

    /// `1`, `on`, `true`, or `yes`, trimmed and ASCII-lowercased. Anything
    /// else, including an unparsable value, is false.
    pub fn is_truthy(value: &OsStr) -> bool {
        let v = value.to_string_lossy().trim().to_ascii_lowercase();
        matches!(v.as_str(), "1" | "on" | "true" | "yes")
    }

    fn effective_pin(&self) -> Option<&OsStr> {
        self.pin
            .as_deref()
            .filter(|v: &&OsStr| !v.is_empty() && !v.to_string_lossy().trim().is_empty())
    }
}

/// How the store root was selected. Replaces three divergent per-engine
/// label vocabularies with one wire-stable set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreMode {
    /// The project-local `.zerostack` directory, existing or selected as the
    /// default for a new repository.
    LocalUnified,
    /// The pin was accepted and resolves inside the project root.
    PinnedInsideProject,
    /// The pin was accepted and lives outside the project root, so this
    /// engine's mutable data is namespaced by project key.
    SharedNamespaced,
    /// An existing legacy per-repository directory retained until explicit
    /// migration. New repositories never select this mode.
    Legacy,
}

impl StoreMode {
    /// Stable wire label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalUnified => "local_unified",
            Self::PinnedInsideProject => "pinned_inside_project",
            Self::SharedNamespaced => "shared_namespaced",
            Self::Legacy => "legacy",
        }
    }
}

/// Construction probes `.zerostack` and the requesting engine's legacy
/// directory, then normalizes paths. It never creates directories. Use
/// [ensure_layout] after resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedStore {
    repo_root: PathBuf,
    engine: Engine,
    mode: StoreMode,
    unified_root: Option<PathBuf>,
    engine_dir: PathBuf,
    project_key: Option<String>,
    pin_value: Option<PathBuf>,
}

impl ResolvedStore {
    /// The single resolution entry point.
    ///
    /// Order:
    /// 1. An existing real `<repo_root>/.zerostack` wins unconditionally.
    /// 2. Otherwise, an explicitly opted-in non-empty pin is selected.
    /// 3. Otherwise, an existing engine legacy directory remains selected so
    ///    authoritative data is never stranded or overwritten.
    /// 4. Otherwise, `<repo_root>/.zerostack` is the default for a new store.
    ///
    /// `repo_root` is normalized, so equivalent path spellings resolve
    /// identically and containment checks cannot be defeated by spelling.
    pub fn resolve(repo_root: &Path, engine: Engine, env: &StoreEnv) -> Self {
        let repo_root = absolutize(repo_root);
        let pin_value = env
            .effective_pin()
            .map(|p| resolve_pin_path(&repo_root, Path::new(p)));

        let local = repo_root.join(LOCAL_STORE_DIR);
        if is_real_dir(&local) {
            return Self {
                engine_dir: local.join(engine.dir_name()),
                unified_root: Some(local),
                mode: StoreMode::LocalUnified,
                project_key: None,
                repo_root,
                engine,
                pin_value,
            };
        }

        if env.shared_opt_in
            && let Some(store) = pin_value.clone()
            && !literal_tilde_root(&store)
        {
            let inside = store_is_under_project_root(&store, &repo_root);
            let (mode, project_key, engine_dir) = if inside {
                (
                    StoreMode::PinnedInsideProject,
                    None,
                    store.join(engine.dir_name()),
                )
            } else {
                let key = project_key(&repo_root);
                let dir = store.join(PROJECTS_DIR).join(&key).join(engine.dir_name());
                (StoreMode::SharedNamespaced, Some(key), dir)
            };
            return Self {
                unified_root: Some(store),
                mode,
                project_key,
                engine_dir,
                repo_root,
                engine,
                pin_value,
            };
        }

        let legacy = repo_root.join(engine.legacy_dir_name());
        if is_real_dir(&legacy) {
            return Self {
                engine_dir: legacy,
                unified_root: None,
                mode: StoreMode::Legacy,
                project_key: None,
                repo_root,
                engine,
                pin_value,
            };
        }

        Self {
            engine_dir: local.clone().join(engine.dir_name()),
            unified_root: Some(local),
            mode: StoreMode::LocalUnified,
            project_key: None,
            repo_root,
            engine,
            pin_value,
        }
    }

    /// [Self::resolve] against the live process environment.
    pub fn resolve_from_process(repo_root: &Path, engine: Engine, opt_in_aliases: &[&str]) -> Self {
        Self::resolve(repo_root, engine, &StoreEnv::from_process(opt_in_aliases))
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    pub fn engine(&self) -> Engine {
        self.engine
    }

    pub fn mode(&self) -> StoreMode {
        self.mode
    }

    /// True when a pin was present, whether or not it was honored.
    pub fn pin_set(&self) -> bool {
        self.pin_value.is_some()
    }

    /// The pin as it would resolve, even in modes that ignore it. Reporting
    /// needs this to explain why a pin had no effect.
    pub fn pin_value(&self) -> Option<&Path> {
        self.pin_value.as_deref()
    }

    /// The store root itself, never the engine subdirectory. `None` only while
    /// preserving an existing legacy directory before migration.
    pub fn unified_root(&self) -> Option<&Path> {
        self.unified_root.as_deref()
    }

    /// Where this engine keeps its data for this repository.
    pub fn engine_dir(&self) -> &Path {
        &self.engine_dir
    }

    /// Project key, present only in [StoreMode::SharedNamespaced].
    pub fn project_key(&self) -> Option<&str> {
        self.project_key.as_deref()
    }

    /// The directory to hand to a CAS handle: the parent of `blobs/` and of
    /// `gc/`, which is also the scope of the store coordination lock.
    ///
    /// Digest-addressed objects are immutable and self-verifying, so they are
    /// shared per store root and deliberately not project-namespaced; only
    /// mutable engine state needs a project key. In legacy mode there is no
    /// shared root, so the engine's own directory hosts them.
    pub fn cas_host(&self) -> &Path {
        match &self.unified_root {
            Some(root) => root,
            None => &self.engine_dir,
        }
    }

    /// The CAS object directory itself, for inspection and reporting. Do not
    /// pass this to a CAS handle; pass [Self::cas_host].
    pub fn blobs_dir(&self) -> PathBuf {
        self.cas_host().join(BLOBS_DIR)
    }

    /// The GC namespace (`gc/`) beside [Self::blobs_dir].
    pub fn gc_dir(&self) -> PathBuf {
        self.cas_host().join(crate::gc_lock::GC_DIR)
    }

    /// A named file inside [Self::engine_dir].
    pub fn engine_file(&self, name: &str) -> Result<PathBuf, EngineFileError> {
        validate_engine_file_name(name)?;
        Ok(self.engine_dir().join(name))
    }

    /// Machine-readable account of how this root was chosen.
    pub fn report(&self, env: &StoreEnv) -> StoreResolutionReport {
        let mut warnings = Vec::new();
        if self.pin_value().is_some_and(literal_tilde_root) {
            warnings.push(
                "configured store root rejected: pin starts with a literal '~' path component"
                    .to_string(),
            );
        } else {
            if self.mode == StoreMode::Legacy {
                warnings.push(format!(
                    "existing legacy store {} is still authoritative; migrate it into {LOCAL_STORE_DIR}/{} before deleting it",
                    self.engine_dir.display(),
                    self.engine.dir_name(),
                ));
            }
            if self.pin_set() && !env.shared_opt_in {
                warnings.push(format!(
                    "store root pin ignored: set {SHARED_STORE_OPT_IN_ENV} or the engine alias to opt in"
                ));
            } else if self.mode == StoreMode::LocalUnified && self.pin_set() {
                warnings.push(format!(
                    "store root pin ignored: project-local {LOCAL_STORE_DIR} takes precedence"
                ));
            }
        }
        if local_marker_is_symlink(&self.repo_root) {
            warnings.push(format!(
                "{LOCAL_STORE_DIR} is a symlink and was refused: a project-local store marker must be a real directory"
            ));
        }
        if env.shared_opt_in && !self.pin_set() {
            warnings.push(format!(
                "shared store opt-in set but no store root pinned: set {}",
                STORE_ROOT_ENVS[0]
            ));
        }
        StoreResolutionReport {
            schema_version: STORE_RESOLUTION_SCHEMA.to_string(),
            repo_root: self.repo_root.clone(),
            engine: self.engine,
            engine_dir: self.engine_dir.clone(),
            unified_root: self.unified_root.clone(),
            cas_host: self.cas_host().to_path_buf(),
            mode: self.mode,
            project_key: self.project_key.clone(),
            shared_store_opt_in: env.shared_opt_in,
            pin_value: self.pin_value.clone(),
            warnings,
        }
    }
}

/// Schema identity for [StoreResolutionReport], shared by every engine's
/// doctor output so the three become comparable.
pub const STORE_RESOLUTION_SCHEMA: &str = "zerostack.store_resolution";

/// Resolution outcome in reportable form. Engines render their own JSON from
/// these fields, which keeps `serde` out of this crate's dependency set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreResolutionReport {
    pub schema_version: String,
    pub repo_root: PathBuf,
    pub engine: Engine,
    pub engine_dir: PathBuf,
    pub unified_root: Option<PathBuf>,
    pub cas_host: PathBuf,
    pub mode: StoreMode,
    pub project_key: Option<String>,
    pub shared_store_opt_in: bool,
    pub pin_value: Option<PathBuf>,
    /// Human-readable advisories, such as a pin that was ignored.
    pub warnings: Vec<String>,
}

/// Create the directories implied by a resolved store. Separate from
pub fn ensure_layout(resolved: &ResolvedStore) -> std::io::Result<()> {
    if resolved.pin_value().is_some_and(literal_tilde_root) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "configured store root must not start with a literal '~' path component",
        ));
    }
    if local_marker_is_symlink(resolved.repo_root()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "{LOCAL_STORE_DIR} is a symlink; a project-local store marker must be a real directory"
            ),
        ));
    }
    if let Some(root) = resolved.unified_root() {
        std::fs::create_dir_all(root)?;
    }
    std::fs::create_dir_all(resolved.engine_dir())?;
    std::fs::create_dir_all(resolved.blobs_dir())?;
    std::fs::create_dir_all(resolved.gc_dir())?;
    Ok(())
}

/// A symlinked `.zerostack` is never followed. Resolution still selects the
/// project-local path for a new store, and [ensure_layout] then fails closed
/// before creating engine data through the symlink.
fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_dir())
        .unwrap_or(false)
}
/// True when a `.zerostack` path exists as a symlink. Layout creation refuses
/// this path before writing any engine state.
fn local_marker_is_symlink(repo_root: &Path) -> bool {
    std::fs::symlink_metadata(repo_root.join(LOCAL_STORE_DIR))
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// Stable project key: sha256 over the absolutized root path string, first
/// [PROJECT_KEY_HEX_LEN] lowercase hex characters, no prefix.
///
/// The key carries no prefix because the namespace already lives in the
/// [PROJECTS_DIR] path component; a prefix would only make one engine's shard
/// directory unpredictable from another engine.
pub fn project_key(repo_root: &Path) -> String {
    let abs = absolutize(repo_root);
    let mut h = Sha256::new();
    h.update(abs.to_string_lossy().as_bytes());
    let digest: [u8; 32] = h.finalize().into();
    let full = digest.iter().fold(String::with_capacity(64), |mut out, b| {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
        out
    });
    full[..PROJECT_KEY_HEX_LEN].to_string()
}

/// Absolutize a path for identity purposes: canonicalize when it exists,
/// otherwise make it absolute against the current directory and remove `.`
/// and `..` textually.
///
/// The textual fallback is what makes the key stable for a root that does not
/// exist yet: `./repo` and `<cwd>/repo` must hash identically, or two engines
/// disagree about a project's shard the moment one of them runs before the
/// directory is created.
pub fn absolutize(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    lexical_normalize(&absolute)
}

/// True when `store` is inside `root`. Both sides are absolutized first, so
/// containment cannot be defeated by a relative or non-canonical spelling.
pub fn store_is_under_project_root(store: &Path, root: &Path) -> bool {
    absolutize(store).starts_with(absolutize(root))
}

/// Resolve a pin against the repository root: absolute as-is, relative joined.
fn literal_tilde_root(path: &Path) -> bool {
    matches!(path.components().next(), Some(Component::Normal(component)) if component == OsStr::new("~"))
}

fn resolve_pin_path(repo_root: &Path, pin: &Path) -> PathBuf {
    if literal_tilde_root(pin) {
        return pin.to_path_buf();
    }
    let joined = if pin.is_absolute() {
        pin.to_path_buf()
    } else {
        repo_root.join(pin)
    };
    absolutize(&joined)
}

/// Remove `.` and `..` without touching the filesystem. Used only for paths
/// that do not exist, where `canonicalize` cannot help.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::ParentDir) | None => out.push(Component::ParentDir.as_os_str()),
                Some(Component::RootDir | Component::Prefix(_)) => {}
                Some(Component::CurDir) => unreachable!("CurDir is never retained"),
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
}
