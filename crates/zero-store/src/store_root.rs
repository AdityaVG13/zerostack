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

/// Which engine is asking. Owns the engine subdirectory name and the legacy
/// per-repository directory, so no engine can spell either inconsistently.
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

    /// Legacy per-repository directory, used when no unified store resolves.
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
    /// A project-local `.zerostack` directory exists and takes precedence.
    LocalUnified,
    /// The pin was accepted and resolves inside the project root.
    PinnedInsideProject,
    /// The pin was accepted and lives outside the project root, so this
    /// engine's mutable data is namespaced by project key.
    SharedNamespaced,
    /// No unified store: the engine uses its legacy per-repository directory.
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

/// A resolved store root plus every path an engine derives from it.
///
/// Construction performs only the `.zerostack` existence probe and path
/// normalization; it never creates directories. Use [ensure_layout] for that,
/// so resolution stays free of side effects and safe to call from reporting
/// and diagnostic paths.
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
    /// 1. `<repo_root>/.zerostack`, when it is a directory, wins
    ///    unconditionally. A project-local marker is an explicit per-repository
    ///    declaration, whereas the pin is ambient process state, so a stray
    ///    variable in an agent harness must not relocate one engine's store.
    /// 2. Otherwise, when opted in and the pin is non-empty: the pin, used
    ///    as-is when absolute and joined to `repo_root` when relative.
    ///    Existence is deliberately not required, so a store can be pinned
    ///    before it is created.
    /// 3. Otherwise the engine's legacy per-repository directory.
    ///
    /// `repo_root` is normalized, so `R` and `R/../<basename>` resolve
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

        if env.shared_opt_in {
            if let Some(store) = pin_value.clone() {
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
        }

        Self {
            engine_dir: repo_root.join(engine.legacy_dir_name()),
            unified_root: None,
            mode: StoreMode::Legacy,
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

    /// The store root itself, never the engine subdirectory. `None` in
    /// [StoreMode::Legacy].
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

    /// A named file inside [Self::engine_dir].
    pub fn engine_file(&self, name: &str) -> Result<PathBuf, EngineFileError> {
        validate_engine_file_name(name)?;
        Ok(self.engine_dir().join(name))
    }

    /// Machine-readable account of how this root was chosen.
    pub fn report(&self, env: &StoreEnv) -> StoreResolutionReport {
        let mut warnings = Vec::new();
        match self.mode {
            StoreMode::Legacy if self.pin_set() && !env.shared_opt_in => warnings.push(format!(
                "store root pin ignored: set {SHARED_STORE_OPT_IN_ENV} or the engine alias to opt in"
            )),
            StoreMode::LocalUnified if self.pin_set() => warnings.push(format!(
                "store root pin ignored: project-local {LOCAL_STORE_DIR} takes precedence"
            )),
            _ => {}
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
pub const STORE_RESOLUTION_SCHEMA: &str = "zerostack.store_resolution.v1";

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
/// resolution so resolution never has side effects.
pub fn ensure_layout(resolved: &ResolvedStore) -> std::io::Result<()> {
    if resolved.pin_value().is_some_and(literal_tilde_root) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "configured store root must not start with a literal '~' path component",
        ));
    }
    if let Some(root) = resolved.unified_root() {
        std::fs::create_dir_all(root)?;
    }
    if local_marker_is_symlink(resolved.repo_root()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "{LOCAL_STORE_DIR} is a symlink; a project-local store marker must be a real directory"
            ),
        ));
    }
    std::fs::create_dir_all(resolved.engine_dir())
}

/// True only for a real directory: a symlinked `.zerostack` is refused.
///
/// `Path::is_dir` follows symlinks, so a symlink dropped into a repository
/// would silently redirect every engine's store — including publishes and
/// collections — to a root the repository never declared. The marker is a
/// security-relevant declaration, so the policy is fail-closed: a symlinked
/// marker is not a local unified store, and resolution falls through to the pin
/// or legacy path instead of following the link.
fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_dir())
        .unwrap_or(false)
}

/// True when a `.zerostack` marker exists but is a symlink, which [ResolvedStore]
/// refuses to adopt.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use tempfile::TempDir;

    #[cfg(unix)]
    #[test]
    fn lexical_normalize_keeps_absolute_paths_at_root() {
        assert_eq!(
            lexical_normalize(Path::new("/../../project")),
            Path::new("/project")
        );
        assert_eq!(
            lexical_normalize(Path::new("/project/../../..")),
            Path::new("/")
        );
    }

    #[test]
    fn lexical_normalize_collapses_interior_components() {
        assert_eq!(
            lexical_normalize(Path::new("alpha/beta/../gamma/./delta")),
            Path::new("alpha/gamma/delta")
        );
        assert_eq!(
            lexical_normalize(Path::new("../../alpha")),
            Path::new("../../alpha")
        );
    }

    #[cfg(unix)]
    #[test]
    fn lexical_normalize_equivalent_identity_paths_match() {
        assert_eq!(
            lexical_normalize(Path::new("/workspace/project/../project")),
            lexical_normalize(Path::new("/workspace/project"))
        );
    }

    const ENGINES: [Engine; 3] = [Engine::TokenZero, Engine::FsZero, Engine::GraphZero];

    fn env_of(pin: Option<&Path>, opt_in: bool) -> StoreEnv {
        StoreEnv::new(pin.map(OsString::from), opt_in)
    }

    fn env_raw(pin: Option<&str>, opt_in: bool) -> StoreEnv {
        StoreEnv::new(pin.map(OsString::from), opt_in)
    }

    /// Golden row 1: nothing set resolves to the legacy per-repo directory.
    #[test]
    fn no_store_resolves_legacy() {
        let repo = TempDir::new().unwrap();
        for engine in ENGINES {
            let r = ResolvedStore::resolve(repo.path(), engine, &StoreEnv::default());
            assert_eq!(r.mode(), StoreMode::Legacy);
            assert_eq!(r.engine_dir(), r.repo_root().join(engine.legacy_dir_name()));
            assert!(r.unified_root().is_none());
            assert!(r.project_key().is_none());
        }
    }

    /// Golden row 2: a project-local store wins with no environment at all.
    #[test]
    fn local_zerostack_resolves_unified() {
        let repo = TempDir::new().unwrap();
        std::fs::create_dir(repo.path().join(LOCAL_STORE_DIR)).unwrap();
        for engine in ENGINES {
            let r = ResolvedStore::resolve(repo.path(), engine, &StoreEnv::default());
            assert_eq!(r.mode(), StoreMode::LocalUnified);
            assert_eq!(
                r.engine_dir(),
                r.repo_root().join(LOCAL_STORE_DIR).join(engine.dir_name())
            );
        }
    }

    /// Golden rows 3 and 8: a pin without opt-in is ignored, absolute or
    /// relative. This is the FSZero divergence (zerostack-pi1).
    #[test]
    fn pin_without_opt_in_is_ignored() {
        let repo = TempDir::new().unwrap();
        let shared = TempDir::new().unwrap();
        for engine in ENGINES {
            let abs =
                ResolvedStore::resolve(repo.path(), engine, &env_of(Some(shared.path()), false));
            assert_eq!(abs.mode(), StoreMode::Legacy);
            assert_eq!(
                abs.engine_dir(),
                abs.repo_root().join(engine.legacy_dir_name())
            );
            assert!(abs.pin_set(), "pin is still reported, just not honored");

            let rel =
                ResolvedStore::resolve(repo.path(), engine, &env_raw(Some("sub-store"), false));
            assert_eq!(rel.mode(), StoreMode::Legacy);
        }
    }

    /// Golden row 4: an opted-in pin outside the project is namespaced.
    #[test]
    fn opted_in_external_pin_is_project_namespaced() {
        let repo = TempDir::new().unwrap();
        let shared = TempDir::new().unwrap();
        let key = project_key(repo.path());
        for engine in ENGINES {
            let r = ResolvedStore::resolve(repo.path(), engine, &env_of(Some(shared.path()), true));
            assert_eq!(r.mode(), StoreMode::SharedNamespaced);
            assert_eq!(r.project_key(), Some(key.as_str()));
            assert_eq!(
                r.engine_dir(),
                absolutize(shared.path())
                    .join(PROJECTS_DIR)
                    .join(&key)
                    .join(engine.dir_name())
            );
        }
    }

    /// Golden rows 5 and 6: a project-local store outranks any pin.
    #[test]
    fn local_store_outranks_pin_with_and_without_opt_in() {
        let repo = TempDir::new().unwrap();
        std::fs::create_dir(repo.path().join(LOCAL_STORE_DIR)).unwrap();
        let shared = TempDir::new().unwrap();
        for opt_in in [true, false] {
            for engine in ENGINES {
                let r = ResolvedStore::resolve(
                    repo.path(),
                    engine,
                    &env_of(Some(shared.path()), opt_in),
                );
                assert_eq!(r.mode(), StoreMode::LocalUnified);
                assert_eq!(
                    r.engine_dir(),
                    r.repo_root().join(LOCAL_STORE_DIR).join(engine.dir_name())
                );
            }
        }
    }

    /// Golden row 7: a pin inside the project is not namespaced, because
    /// there is exactly one project under it.
    #[test]
    fn opted_in_internal_pin_is_not_namespaced() {
        let repo = TempDir::new().unwrap();
        let r = ResolvedStore::resolve(
            repo.path(),
            Engine::FsZero,
            &env_raw(Some("sub-store"), true),
        );
        assert_eq!(r.mode(), StoreMode::PinnedInsideProject);
        assert!(r.project_key().is_none());
        assert_eq!(
            r.engine_dir(),
            r.repo_root()
                .join("sub-store")
                .join(Engine::FsZero.dir_name())
        );
    }

    /// Golden row 9: an exported-but-empty pin means unset, not the repo root.
    /// This is the FSZero empty-pin bug (fszero-44nj).
    #[test]
    fn empty_pin_is_unset() {
        let repo = TempDir::new().unwrap();
        for engine in ENGINES {
            let r = ResolvedStore::resolve(repo.path(), engine, &env_raw(Some(""), true));
            assert_eq!(r.mode(), StoreMode::Legacy);
            assert!(!r.pin_set());
            assert_eq!(r.engine_dir(), r.repo_root().join(engine.legacy_dir_name()));
            assert_ne!(r.engine_dir(), r.repo_root().join(engine.dir_name()));
        }
    }

    /// Golden row 10: a pin that does not exist still resolves. A store may
    /// be pinned before it is created.
    #[test]
    fn nonexistent_pin_resolves_without_error() {
        let repo = TempDir::new().unwrap();
        let shared = TempDir::new().unwrap();
        let missing = shared.path().join("does-not-exist").join("store");
        let r = ResolvedStore::resolve(
            repo.path(),
            Engine::GraphZero,
            &env_of(Some(&missing), true),
        );
        assert_eq!(r.mode(), StoreMode::SharedNamespaced);
        assert_eq!(
            r.engine_dir(),
            missing
                .join(PROJECTS_DIR)
                .join(project_key(repo.path()))
                .join("graphzero")
        );
    }

    /// Golden row 17: the anti-collision gate. Two distinct projects sharing
    /// one pin must never resolve to the same engine directory. This is what
    /// TokenZero gets wrong today (zerostack-ljx).
    #[test]
    fn distinct_projects_never_collide_under_one_pin() {
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        let shared = TempDir::new().unwrap();
        for engine in ENGINES {
            let ra = ResolvedStore::resolve(a.path(), engine, &env_of(Some(shared.path()), true));
            let rb = ResolvedStore::resolve(b.path(), engine, &env_of(Some(shared.path()), true));
            assert_ne!(ra.engine_dir(), rb.engine_dir());
            assert_ne!(ra.project_key(), rb.project_key());
        }
    }

    /// Golden row 18: all three engines must agree on the project key, or one
    /// cannot find another's shard. FSZero's `sid-` prefix broke this.
    #[test]
    fn all_engines_share_one_project_key() {
        let repo = TempDir::new().unwrap();
        let shared = TempDir::new().unwrap();
        let keys: Vec<String> = ENGINES
            .iter()
            .map(|e| {
                ResolvedStore::resolve(repo.path(), *e, &env_of(Some(shared.path()), true))
                    .project_key()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(keys[0], keys[1]);
        assert_eq!(keys[1], keys[2]);
        assert_eq!(keys[0].len(), PROJECT_KEY_HEX_LEN);
        assert!(
            keys[0]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
    }

    /// Golden rows 19 and 20: the CAS root is shared per store root and is
    /// never project-namespaced, so a blob published by one engine is found
    /// by the others at the same path.
    #[test]
    fn cas_is_shared_per_store_root() {
        let repo = TempDir::new().unwrap();
        let shared = TempDir::new().unwrap();
        for engine in ENGINES {
            let r = ResolvedStore::resolve(repo.path(), engine, &env_of(Some(shared.path()), true));
            assert_eq!(r.cas_host(), absolutize(shared.path()));
            assert_eq!(r.blobs_dir(), absolutize(shared.path()).join(BLOBS_DIR));
            assert!(!r.blobs_dir().to_string_lossy().contains(PROJECTS_DIR));
        }
        std::fs::create_dir(repo.path().join(LOCAL_STORE_DIR)).unwrap();
        for engine in ENGINES {
            let r = ResolvedStore::resolve(repo.path(), engine, &StoreEnv::default());
            assert_eq!(r.cas_host(), r.repo_root().join(LOCAL_STORE_DIR));
            assert_eq!(
                r.blobs_dir(),
                r.repo_root().join(LOCAL_STORE_DIR).join(BLOBS_DIR)
            );
        }
    }

    /// Golden row 21: a root that does not exist yet must hash the same
    /// relative and absolute, or engines disagree about its shard.
    #[test]
    fn project_key_is_spelling_stable_for_missing_roots() {
        let cwd = std::env::current_dir().unwrap();
        let missing = "zero-store-does-not-exist-xyz";
        assert_eq!(
            project_key(Path::new(missing)),
            project_key(&cwd.join(missing))
        );
        assert_eq!(
            project_key(Path::new("./a/../b")),
            project_key(&cwd.join("b"))
        );
    }

    /// Golden row 22: `R` and `R/../<basename>` resolve identically.
    #[test]
    fn resolution_is_spelling_stable() {
        let repo = TempDir::new().unwrap();
        let base = repo.path().file_name().unwrap();
        let indirect = repo.path().join("..").join(base);
        let direct = ResolvedStore::resolve(repo.path(), Engine::TokenZero, &StoreEnv::default());
        let round = ResolvedStore::resolve(&indirect, Engine::TokenZero, &StoreEnv::default());
        assert_eq!(direct, round);
    }

    /// Golden rows 15 and 16: truthiness parity across every engine alias.
    #[test]
    fn opt_in_truthiness_parity() {
        for truthy in ["1", "on", "true", "yes", " 1 ", "TRUE", "Yes"] {
            assert!(
                StoreEnv::is_truthy(OsStr::new(truthy)),
                "expected truthy: {truthy:?}"
            );
        }
        for falsy in ["0", "off", "false", "no", "", "maybe", "2"] {
            assert!(
                !StoreEnv::is_truthy(OsStr::new(falsy)),
                "expected falsy: {falsy:?}"
            );
        }
    }

    /// Golden row 23: an ignored pin must be explained, not silently dropped.
    #[test]
    fn ignored_pin_is_reported() {
        let repo = TempDir::new().unwrap();
        let shared = TempDir::new().unwrap();
        let env = env_of(Some(shared.path()), false);
        let report = ResolvedStore::resolve(repo.path(), Engine::TokenZero, &env).report(&env);
        assert_eq!(report.mode, StoreMode::Legacy);
        assert_eq!(report.schema_version, STORE_RESOLUTION_SCHEMA);
        assert!(
            report.warnings.iter().any(|w| w.contains("pin ignored")),
            "warnings: {:?}",
            report.warnings
        );

        std::fs::create_dir(repo.path().join(LOCAL_STORE_DIR)).unwrap();
        let env = env_of(Some(shared.path()), true);
        let report = ResolvedStore::resolve(repo.path(), Engine::TokenZero, &env).report(&env);
        assert_eq!(report.mode, StoreMode::LocalUnified);
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("takes precedence"))
        );
    }

    /// Opt-in with nothing pinned is a misconfiguration worth surfacing.
    #[test]
    fn opt_in_without_pin_is_reported() {
        let repo = TempDir::new().unwrap();
        let env = env_of(None, true);
        let report = ResolvedStore::resolve(repo.path(), Engine::FsZero, &env).report(&env);
        assert_eq!(report.mode, StoreMode::Legacy);
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("no store root pinned"))
        );
    }

    /// Golden row 25: `.zerostack` as a regular file is not a store.
    #[test]
    fn zerostack_file_is_not_a_store() {
        let repo = TempDir::new().unwrap();
        std::fs::write(repo.path().join(LOCAL_STORE_DIR), b"not a dir").unwrap();
        let r = ResolvedStore::resolve(repo.path(), Engine::FsZero, &StoreEnv::default());
        assert_eq!(r.mode(), StoreMode::Legacy);
    }

    /// Golden row 26: a lookalike directory name is not a store.
    #[test]
    fn lookalike_directory_is_not_a_store() {
        let repo = TempDir::new().unwrap();
        std::fs::create_dir(repo.path().join(".zerostack-old")).unwrap();
        let r = ResolvedStore::resolve(repo.path(), Engine::GraphZero, &StoreEnv::default());
        assert_eq!(r.mode(), StoreMode::Legacy);
    }

    #[test]
    fn engine_dir_names_are_distinct_and_stable() {
        assert_eq!(Engine::TokenZero.dir_name(), "tokenzero");
        assert_eq!(Engine::FsZero.dir_name(), "fszero");
        assert_eq!(Engine::GraphZero.dir_name(), "graphzero");
        assert_eq!(Engine::TokenZero.legacy_dir_name(), ".tokenzero");
        assert_eq!(Engine::FsZero.legacy_dir_name(), ".fszero");
        assert_eq!(Engine::GraphZero.legacy_dir_name(), ".graphzero");
    }

    #[test]
    fn mode_labels_are_stable() {
        assert_eq!(StoreMode::LocalUnified.as_str(), "local_unified");
        assert_eq!(
            StoreMode::PinnedInsideProject.as_str(),
            "pinned_inside_project"
        );
        assert_eq!(StoreMode::SharedNamespaced.as_str(), "shared_namespaced");
        assert_eq!(StoreMode::Legacy.as_str(), "legacy");
    }

    #[test]
    fn ensure_layout_creates_engine_dir_only() {
        let repo = TempDir::new().unwrap();
        let shared = TempDir::new().unwrap();
        let r = ResolvedStore::resolve(
            repo.path(),
            Engine::FsZero,
            &env_of(Some(shared.path()), true),
        );
        ensure_layout(&r).unwrap();
        assert!(r.engine_dir().is_dir());
        assert!(
            !r.blobs_dir().exists(),
            "CAS directories are created on publish"
        );
    }

    #[test]
    fn engine_file_lands_in_engine_dir() {
        let repo = TempDir::new().unwrap();
        std::fs::create_dir(repo.path().join(LOCAL_STORE_DIR)).unwrap();
        let r = ResolvedStore::resolve(repo.path(), Engine::FsZero, &StoreEnv::default());
        assert_eq!(
            r.engine_file("store.sqlite3").unwrap(),
            r.repo_root()
                .join(LOCAL_STORE_DIR)
                .join("fszero")
                .join("store.sqlite3")
        );
    }

    #[test]
    fn containment_is_spelling_independent() {
        let repo = TempDir::new().unwrap();
        let inside = repo.path().join("sub");
        std::fs::create_dir(&inside).unwrap();
        assert!(store_is_under_project_root(&inside, repo.path()));
        assert!(store_is_under_project_root(
            &repo.path().join("./sub"),
            repo.path()
        ));
        let outside = TempDir::new().unwrap();
        assert!(!store_is_under_project_root(outside.path(), repo.path()));
    }

    #[test]
    fn engine_file_accepts_normal_and_dot_prefixed_basenames() {
        assert!(validate_engine_file_name("store.sqlite3").is_ok());
        assert!(validate_engine_file_name(".state.json").is_ok());
    }

    #[test]
    fn engine_file_rejects_non_basename_paths() {
        for name in [
            "",
            ".",
            "..",
            "/absolute",
            "nested/file",
            "nested\\file",
            "C:\\absolute",
            "C:relative",
        ] {
            assert!(
                validate_engine_file_name(name).is_err(),
                "accepted {name:?}"
            );
        }
    }

    #[test]
    fn a_symlinked_local_marker_is_refused() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path().join("repo");
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, repo.join(LOCAL_STORE_DIR)).unwrap();

        let env = StoreEnv::new(None, false);
        let resolved = ResolvedStore::resolve(&repo, Engine::TokenZero, &env);
        assert_eq!(
            resolved.mode(),
            StoreMode::Legacy,
            "a symlinked marker must not be adopted as a local unified store"
        );
        assert!(resolved.unified_root().is_none());
        assert!(
            resolved
                .report(&env)
                .warnings
                .iter()
                .any(|w| w.contains("symlink"))
        );
        assert!(ensure_layout(&resolved).is_err());
    }

    #[test]
    fn a_real_local_marker_is_still_adopted() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(LOCAL_STORE_DIR)).unwrap();
        let env = StoreEnv::new(None, false);
        let resolved = ResolvedStore::resolve(dir.path(), Engine::TokenZero, &env);
        assert_eq!(resolved.mode(), StoreMode::LocalUnified);
        ensure_layout(&resolved).unwrap();
    }

    #[test]
    fn tilde_root_rejects_literal_first_component_without_creating_it() {
        for pin in ["~", "~/foo"] {
            let repo = TempDir::new().unwrap();
            let env = StoreEnv::new(Some(OsString::from(pin)), true);
            let resolved = ResolvedStore::resolve(repo.path(), Engine::TokenZero, &env);

            let error = ensure_layout(&resolved).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
            assert!(error.to_string().contains("literal '~'"));
            assert!(!repo.path().join("~").exists());
        }
    }

    #[test]
    fn tilde_root_allows_tilde_after_the_first_component() {
        for pin in ["safe/~", "safe~/store"] {
            let repo = TempDir::new().unwrap();
            let env = StoreEnv::new(Some(OsString::from(pin)), true);
            let resolved = ResolvedStore::resolve(repo.path(), Engine::GraphZero, &env);

            ensure_layout(&resolved).unwrap();
            assert!(repo.path().join(pin).is_dir());
        }
    }
}
