//! Executable discovery for embedded harnesses.
//!
//! A harness that embeds ZeroStack needs the aggregate host and the three
//! engine delegate binaries. Writing those as absolute paths in a shipped
//! config bakes one developer's worktree into every install, so a config that
//! travels resolves nothing and a fresh machine fails at spawn time with a
//! bare ENOENT.
//!
//! Resolution order, highest precedence first:
//!
//! 1. `ZEROSTACK_HOME` — one directory holding every binary, in `bin/`.
//! 2. `ZEROSTACK_DEV_ROOT` — the documented dev-checkout override: a parent
//!    directory of sibling engine checkouts, each with `target/release/`.
//! 3. XDG data directory — `$XDG_DATA_HOME/zerostack/bin`, defaulting to
//!    `$HOME/.local/share/zerostack/bin`.
//! 4. Platform install directories.
//! 5. `PATH`.
//!
//! Every entry point is pure: environment state is captured once into
//! [DiscoveryEnv] and the filesystem is reached only through a caller-supplied
//! probe. Resolution therefore has no side effects and is testable without
//! mutating process globals or planting files.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Single-directory install pin. Highest precedence.
pub const HOME_ENV: &str = "ZEROSTACK_HOME";

/// Dev-checkout override: parent of sibling engine repositories.
pub const DEV_ROOT_ENV: &str = "ZEROSTACK_DEV_ROOT";

/// Binary subdirectory of an install root.
pub const BIN_DIR: &str = "bin";

/// Vendor subdirectory inside a shared data directory.
pub const DATA_SUBDIR: &str = "zerostack";

/// Profile subdirectory of a dev checkout's Cargo target directory.
pub const DEV_TARGET_SUBDIR: &str = "target/release";

/// A binary the aggregate CodeMode integration spawns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HarnessBinary {
    /// ZeroStack aggregate CodeMode sidecar.
    AggregateHost,
    /// FSZero raw-worker / CodeMode delegate.
    FsDelegate,
    /// GraphZero raw-worker / CodeMode delegate.
    GraphDelegate,
    /// TokenZero raw-worker / CodeMode delegate.
    TokenDelegate,
}

/// Every binary a harness must resolve, in stable order.
pub const HARNESS_BINARIES: [HarnessBinary; 4] = [
    HarnessBinary::AggregateHost,
    HarnessBinary::FsDelegate,
    HarnessBinary::GraphDelegate,
    HarnessBinary::TokenDelegate,
];

impl HarnessBinary {
    /// Executable file stem, identical on every platform.
    pub const fn file_stem(self) -> &'static str {
        match self {
            Self::AggregateHost => "zerostack-codemode-host",
            Self::FsDelegate => "fszero-codemode",
            Self::GraphDelegate => "graphzero-codemode",
            Self::TokenDelegate => "tokenzero-codemode",
        }
    }

    /// Stable config key, matching the harness `binaries` map.
    pub const fn config_key(self) -> &'static str {
        match self {
            Self::AggregateHost => "aggregate_host",
            Self::FsDelegate => "fs",
            Self::GraphDelegate => "graph",
            Self::TokenDelegate => "token",
        }
    }

    /// Owning repository directory name, used only by the dev-checkout override.
    pub const fn dev_repo_dir(self) -> &'static str {
        match self {
            Self::AggregateHost => "ZeroStack",
            Self::FsDelegate => "FSZero",
            Self::GraphDelegate => "GraphZero",
            Self::TokenDelegate => "TokenZero",
        }
    }

    /// Platform file name, `.exe`-suffixed on Windows.
    pub fn file_name(self) -> String {
        if cfg!(windows) {
            format!("{}.exe", self.file_stem())
        } else {
            self.file_stem().to_owned()
        }
    }
}

/// Which rule produced a candidate directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A direct file pin, used by artifacts that are not install-root binaries.
    Explicit,
    /// From [HOME_ENV].
    Home,
    /// From [DEV_ROOT_ENV].
    DevCheckout,
    /// From the XDG data directory.
    XdgData,
    /// A platform install directory.
    PlatformInstall,
    /// A `PATH` entry.
    Path,
}

impl Source {
    /// Stable wire label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Home => "zerostack_home",
            Self::DevCheckout => "dev_checkout",
            Self::XdgData => "xdg_data",
            Self::PlatformInstall => "platform_install",
            Self::Path => "path",
        }
    }
}

/// Environment state resolution depends on, captured so entry points read no
/// process state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscoveryEnv {
    home: Option<PathBuf>,
    dev_root: Option<PathBuf>,
    xdg_data_home: Option<PathBuf>,
    user_home: Option<PathBuf>,
    path: Vec<PathBuf>,
    platform_dirs: Vec<PathBuf>,
}

impl DiscoveryEnv {
    /// Read the process environment.
    pub fn from_process() -> Self {
        Self {
            home: absolute_var(HOME_ENV),
            dev_root: absolute_var(DEV_ROOT_ENV),
            xdg_data_home: absolute_var("XDG_DATA_HOME"),
            user_home: absolute_var("HOME").or_else(|| absolute_var("USERPROFILE")),
            path: std::env::var_os("PATH")
                .map(|value| std::env::split_paths(&value).collect())
                .unwrap_or_default(),
            platform_dirs: default_platform_dirs(),
        }
    }

    /// Build explicitly, for tests and for harnesses that already parsed config.
    pub fn new(
        home: Option<PathBuf>,
        dev_root: Option<PathBuf>,
        xdg_data_home: Option<PathBuf>,
        user_home: Option<PathBuf>,
        path: Vec<PathBuf>,
        platform_dirs: Vec<PathBuf>,
    ) -> Self {
        Self {
            home: absolute(home),
            dev_root: absolute(dev_root),
            xdg_data_home: absolute(xdg_data_home),
            user_home: absolute(user_home),
            path,
            platform_dirs,
        }
    }
}

/// An exported-but-empty variable is a shell artifact, not an instruction, and
/// a relative install root would silently depend on the spawning cwd. Both are
/// discarded rather than half-honored.
fn absolute_var(name: &str) -> Option<PathBuf> {
    absolute(std::env::var_os(name).map(PathBuf::from))
}

fn absolute(path: Option<PathBuf>) -> Option<PathBuf> {
    path.filter(|path| path.is_absolute() && !is_blank(path.as_os_str()))
}

fn is_blank(value: &OsStr) -> bool {
    value.is_empty() || value.to_string_lossy().trim().is_empty()
}

#[cfg(windows)]
fn default_platform_dirs() -> Vec<PathBuf> {
    ["LOCALAPPDATA", "PROGRAMFILES", "PROGRAMDATA"]
        .iter()
        .filter_map(|key| absolute_var(key))
        .map(|parent| parent.join("ZeroStack").join(BIN_DIR))
        .collect()
}

#[cfg(not(windows))]
fn default_platform_dirs() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/usr/local/lib/zerostack").join(BIN_DIR),
        PathBuf::from("/opt/zerostack").join(BIN_DIR),
        PathBuf::from("/usr/lib/zerostack").join(BIN_DIR),
    ]
}

/// One candidate location, in precedence order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Rule that produced this path.
    pub source: Source,
    /// Absolute path probed for the binary.
    pub path: PathBuf,
}

/// Candidate paths for `binary`, highest precedence first.
///
/// Pure: no filesystem access. Callers probe the returned paths in order.
pub fn candidates(binary: HarnessBinary, env: &DiscoveryEnv) -> Vec<Candidate> {
    let file_name = binary.file_name();
    let mut out: Vec<Candidate> = Vec::new();
    let push = |out: &mut Vec<Candidate>, source: Source, path: PathBuf| {
        if !out.iter().any(|c| c.path == path) {
            out.push(Candidate { source, path });
        }
    };

    if let Some(home) = &env.home {
        push(&mut out, Source::Home, home.join(BIN_DIR).join(&file_name));
    }
    if let Some(dev_root) = &env.dev_root {
        push(
            &mut out,
            Source::DevCheckout,
            dev_root
                .join(binary.dev_repo_dir())
                .join(DEV_TARGET_SUBDIR)
                .join(&file_name),
        );
    }
    for data_home in xdg_data_homes(env) {
        push(
            &mut out,
            Source::XdgData,
            data_home.join(DATA_SUBDIR).join(BIN_DIR).join(&file_name),
        );
    }
    for dir in env.platform_dirs.clone() {
        push(&mut out, Source::PlatformInstall, dir.join(&file_name));
    }
    for dir in env.path.clone() {
        // A blank PATH entry means "current directory" to some shells. Honoring
        // that would make resolution depend on the spawning cwd.
        if dir.is_absolute() {
            push(&mut out, Source::Path, dir.join(&file_name));
        }
    }
    out
}

fn xdg_data_homes(env: &DiscoveryEnv) -> Vec<PathBuf> {
    if let Some(explicit) = &env.xdg_data_home {
        return vec![explicit.clone()];
    }
    env.user_home
        .iter()
        .map(|home| home.join(".local").join("share"))
        .collect()
}

/// A resolved executable and the rule that found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// Which binary was resolved.
    pub binary: HarnessBinary,
    /// Rule that produced the accepted path.
    pub source: Source,
    /// Absolute path to the executable.
    pub path: PathBuf,
}

/// Nothing on the search path is executable.
///
/// Carries every probed candidate, so the harness reports why discovery failed
/// instead of surfacing a bare ENOENT from spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryError {
    /// Binary that could not be resolved.
    pub binary: HarnessBinary,
    /// Every candidate probed, in precedence order.
    pub probed: Vec<Candidate>,
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cannot locate executable {}: probed {} candidate(s)",
            self.binary.file_stem(),
            self.probed.len()
        )?;
        for candidate in &self.probed {
            write!(
                f,
                "\n  [{}] {}",
                candidate.source.as_str(),
                candidate.path.display()
            )?;
        }
        if self.probed.is_empty() {
            write!(
                f,
                "\n  no candidates: set {HOME_ENV} to the install root, or {DEV_ROOT_ENV} to the parent of the engine checkouts"
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for DiscoveryError {}

/// Resolve `binary` against `env`, accepting the first candidate `probe` admits.
pub fn resolve_with(
    binary: HarnessBinary,
    env: &DiscoveryEnv,
    probe: &dyn Fn(&Path) -> bool,
) -> Result<Resolved, DiscoveryError> {
    let probed = candidates(binary, env);
    for candidate in &probed {
        if probe(&candidate.path) {
            return Ok(Resolved {
                binary,
                source: candidate.source,
                path: candidate.path.clone(),
            });
        }
    }
    Err(DiscoveryError { binary, probed })
}

/// Resolve `binary` against `env`, probing the real filesystem.
pub fn resolve(binary: HarnessBinary, env: &DiscoveryEnv) -> Result<Resolved, DiscoveryError> {
    resolve_with(binary, env, &is_executable_file)
}

/// True for an existing regular file carrying an execute bit.
///
/// A directory named like the binary, or a non-executable file, is not a
/// spawnable target and must not shadow a later candidate.
pub fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Resolve every harness binary, reporting per-binary outcomes.
///
/// A missing private engine is not fatal to the others, so this never
/// short-circuits: the caller decides which delegates it requires.
pub fn resolve_all(
    env: &DiscoveryEnv,
    probe: &dyn Fn(&Path) -> bool,
) -> Vec<(HarnessBinary, Result<Resolved, DiscoveryError>)> {
    HARNESS_BINARIES
        .iter()
        .map(|binary| (*binary, resolve_with(*binary, env, probe)))
        .collect()
}

/// Machine-readable discovery report schema, stable across surfaces.
pub const DISCOVERY_SCHEMA: &str = "zerostack.binary_discovery.v1";

/// Render [resolve_all] as a canonical JSON report.
pub fn locate_report(env: &DiscoveryEnv, probe: &dyn Fn(&Path) -> bool) -> serde_json::Value {
    let mut binaries = serde_json::Map::new();
    for (binary, outcome) in resolve_all(env, probe) {
        let entry = match outcome {
            Ok(resolved) => serde_json::json!({
                "resolved": true,
                "source": resolved.source.as_str(),
                "path": resolved.path.to_string_lossy(),
            }),
            Err(error) => serde_json::json!({
                "resolved": false,
                "probed": error
                    .probed
                    .iter()
                    .map(|candidate| serde_json::json!({
                        "source": candidate.source.as_str(),
                        "path": candidate.path.to_string_lossy(),
                    }))
                    .collect::<Vec<_>>(),
            }),
        };
        binaries.insert(binary.config_key().to_owned(), entry);
    }
    serde_json::json!({
        "schema": DISCOVERY_SCHEMA,
        "order": [
            Source::Home.as_str(),
            Source::DevCheckout.as_str(),
            Source::XdgData.as_str(),
            Source::PlatformInstall.as_str(),
            Source::Path.as_str(),
        ],
        "binaries": binaries,
    })
}
