//! Stable Node runtime resolution.
//!
//! A pinned fnm multishell path (`~/.local/state/fnm_multishells/<pid>_<ts>/bin/node`)
//! is keyed by shell pid and timestamp: it vanishes on reboot or in a new shell,
//! so a harness that recorded one dies silently later. Resolution here never
//! accepts such a path -- it is refused with a reason before it is probed -- and
//! never depends on shell state, so the same contract holds across sessions.
//!
//! Resolution order, highest precedence first:
//!
//! 1. [NODE_ENV] -- an explicit absolute file pin.
//! 2. `ZEROSTACK_HOME/bin/node` -- a runtime shipped with the install.
//! 3. Well-known stable locations: a version manager's *default alias* (stable
//!    across shells, unlike a multishell link), then user and system prefixes.
//! 4. `PATH`.
//!
//! Environment state is captured once into [NodeEnv] and the filesystem is
//! reached only through a caller-supplied probe, so resolution is pure and
//! testable without planting files or mutating process globals.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::discovery::{is_executable_file, BIN_DIR, HOME_ENV};

/// Direct file pin for the Node runtime. Highest precedence.
pub const NODE_ENV: &str = "ZEROSTACK_NODE";

/// Root of an fnm installation, holding the stable `aliases/default` link.
pub const FNM_DIR_ENV: &str = "FNM_DIR";

/// Stable alias subpath of an fnm installation.
pub const FNM_DEFAULT_ALIAS_SUBDIR: &str = "aliases/default";

/// Machine-readable node resolution report schema.
pub const NODE_SCHEMA: &str = "zerostack.node_resolution.v1";

/// Stable refusal reason for a per-shell runtime path.
pub const EPHEMERAL_REASON: &str = "ephemeral_path";

/// Path fragments that identify a per-shell, pid-keyed runtime directory.
pub const EPHEMERAL_MARKERS: &[&str] = &["fnm_multishells", "nvm_multishells"];

/// Which rule produced a node candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeSource {
    /// From [NODE_ENV].
    Explicit,
    /// From `ZEROSTACK_HOME/bin`.
    Home,
    /// A well-known stable install location.
    WellKnown,
    /// A `PATH` entry.
    Path,
}

impl NodeSource {
    /// Stable wire label, shared with the binary discovery grammar where the
    /// rule is the same.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Home => "zerostack_home",
            Self::WellKnown => "well_known",
            Self::Path => "path",
        }
    }
}

/// Rules the node contract applies, highest precedence first.
pub const NODE_ORDER: [NodeSource; 4] = [
    NodeSource::Explicit,
    NodeSource::Home,
    NodeSource::WellKnown,
    NodeSource::Path,
];

/// Platform file name of the Node executable.
pub fn node_file_name() -> &'static str {
    if cfg!(windows) {
        "node.exe"
    } else {
        "node"
    }
}

/// Environment state node resolution depends on, captured so entry points read
/// no process state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeEnv {
    explicit: Option<PathBuf>,
    zerostack_home: Option<PathBuf>,
    fnm_dir: Option<PathBuf>,
    user_home: Option<PathBuf>,
    path: Vec<PathBuf>,
    system_dirs: Vec<PathBuf>,
}

impl NodeEnv {
    /// Read the process environment.
    pub fn from_process() -> Self {
        Self {
            explicit: absolute_var(NODE_ENV),
            zerostack_home: absolute_var(HOME_ENV),
            fnm_dir: absolute_var(FNM_DIR_ENV),
            user_home: absolute_var("HOME").or_else(|| absolute_var("USERPROFILE")),
            path: std::env::var_os("PATH")
                .map(|value| std::env::split_paths(&value).collect())
                .unwrap_or_default(),
            system_dirs: default_system_dirs(),
        }
    }

    /// Build explicitly, for tests and for harnesses that already parsed config.
    ///
    /// A relative pin or install root would make resolution depend on the
    /// spawning working directory, so it is discarded rather than half-honored.
    pub fn new(
        explicit: Option<PathBuf>,
        zerostack_home: Option<PathBuf>,
        fnm_dir: Option<PathBuf>,
        user_home: Option<PathBuf>,
        path: Vec<PathBuf>,
        system_dirs: Vec<PathBuf>,
    ) -> Self {
        Self {
            explicit: absolute(explicit),
            zerostack_home: absolute(zerostack_home),
            fnm_dir: absolute(fnm_dir),
            user_home: absolute(user_home),
            path,
            system_dirs,
        }
    }
}

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
fn default_system_dirs() -> Vec<PathBuf> {
    ["PROGRAMFILES", "PROGRAMDATA"]
        .iter()
        .filter_map(|key| absolute_var(key))
        .map(|parent| parent.join("nodejs"))
        .collect()
}

#[cfg(not(windows))]
fn default_system_dirs() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
    ]
}

/// One probed node location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeCandidate {
    /// Rule that produced this path.
    pub source: NodeSource,
    /// Absolute path to the candidate runtime.
    pub path: PathBuf,
}

/// A candidate rejected before it was probed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRefusal {
    /// Candidate that was refused.
    pub candidate: NodeCandidate,
    /// Stable machine-readable reason.
    pub reason: &'static str,
}

/// True when `path` lives in a directory that will not outlive the shell that
/// created it.
pub fn is_ephemeral(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        EPHEMERAL_MARKERS.iter().any(|marker| name.contains(marker))
    })
}

/// Candidate node paths, highest precedence first.
///
/// Pure: no filesystem access. Ephemeral paths are still listed here so the
/// caller can report exactly which pin was refused and why.
pub fn node_candidates(env: &NodeEnv) -> Vec<NodeCandidate> {
    let file_name = node_file_name();
    let mut out: Vec<NodeCandidate> = Vec::new();
    let push = |out: &mut Vec<NodeCandidate>, source: NodeSource, path: PathBuf| {
        if !out.iter().any(|candidate| candidate.path == path) {
            out.push(NodeCandidate { source, path });
        }
    };

    if let Some(explicit) = &env.explicit {
        push(&mut out, NodeSource::Explicit, explicit.clone());
    }
    if let Some(home) = &env.zerostack_home {
        push(
            &mut out,
            NodeSource::Home,
            home.join(BIN_DIR).join(file_name),
        );
    }
    for dir in well_known_dirs(env) {
        push(&mut out, NodeSource::WellKnown, dir.join(file_name));
    }
    for dir in env.path.clone() {
        // A blank PATH entry means "current directory" to some shells. Honoring
        // that would make resolution depend on the spawning cwd.
        if dir.is_absolute() {
            push(&mut out, NodeSource::Path, dir.join(file_name));
        }
    }
    out
}

/// Stable install directories, highest precedence first.
///
/// The fnm entry is the `default` alias, which points at an installed version
/// and survives reboots; the per-shell multishell links are never consulted.
fn well_known_dirs(env: &NodeEnv) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let fnm_dir = env.fnm_dir.clone().or_else(|| {
        env.user_home
            .as_ref()
            .map(|home| home.join(".local").join("share").join("fnm"))
    });
    if let Some(fnm_dir) = fnm_dir {
        dirs.push(fnm_dir.join(FNM_DEFAULT_ALIAS_SUBDIR).join(BIN_DIR));
    }
    if let Some(home) = &env.user_home {
        dirs.push(home.join(".volta").join(BIN_DIR));
        dirs.push(home.join(".local").join(BIN_DIR));
    }
    dirs.extend(env.system_dirs.iter().cloned());
    dirs
}

/// Outcome of resolving the Node runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeOutcome {
    /// Accepted candidate, when one was found.
    pub resolved: Option<NodeCandidate>,
    /// Candidates probed, in precedence order.
    pub probed: Vec<NodeCandidate>,
    /// Candidates rejected before probing.
    pub refused: Vec<NodeRefusal>,
}

/// No stable Node runtime is available.
///
/// Carries every probed and refused candidate, so a harness explains the
/// failure instead of surfacing a bare ENOENT from spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeError {
    /// Every candidate probed, in precedence order.
    pub probed: Vec<NodeCandidate>,
    /// Every candidate refused before probing.
    pub refused: Vec<NodeRefusal>,
}

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cannot locate a stable node runtime: probed {} candidate(s), refused {}",
            self.probed.len(),
            self.refused.len()
        )?;
        for candidate in &self.probed {
            write!(
                f,
                "
  [{}] {}",
                candidate.source.as_str(),
                candidate.path.display()
            )?;
        }
        for refusal in &self.refused {
            write!(
                f,
                "
  [{}] {} refused: {}",
                refusal.candidate.source.as_str(),
                refusal.candidate.path.display(),
                refusal.reason
            )?;
        }
        write!(
            f,
            "
  set {NODE_ENV} to an absolute path to a node executable that outlives the shell (not an fnm multishell path)"
        )
    }
}

impl std::error::Error for NodeError {}

impl NodeOutcome {
    /// Require a resolved runtime, turning an empty outcome into a diagnostic.
    pub fn require(self) -> Result<NodeCandidate, NodeError> {
        match self.resolved {
            Some(candidate) => Ok(candidate),
            None => Err(NodeError {
                probed: self.probed,
                refused: self.refused,
            }),
        }
    }
}

/// Resolve the Node runtime, accepting the first candidate `probe` admits.
///
/// An ephemeral candidate is refused before probing, at every precedence level:
/// an explicit pin at a multishell path is a stale pin, not an instruction.
pub fn resolve_node_with(env: &NodeEnv, probe: &dyn Fn(&Path) -> bool) -> NodeOutcome {
    let mut probed = Vec::new();
    let mut refused = Vec::new();
    for candidate in node_candidates(env) {
        if is_ephemeral(&candidate.path) {
            refused.push(NodeRefusal {
                candidate,
                reason: EPHEMERAL_REASON,
            });
            continue;
        }
        probed.push(candidate.clone());
        if probe(&candidate.path) {
            return NodeOutcome {
                resolved: Some(candidate),
                probed,
                refused,
            };
        }
    }
    NodeOutcome {
        resolved: None,
        probed,
        refused,
    }
}

/// Resolve the Node runtime, probing the real filesystem.
pub fn resolve_node(env: &NodeEnv) -> NodeOutcome {
    resolve_node_with(env, &is_executable_file)
}

/// Render node resolution as a canonical JSON report.
pub fn node_report(env: &NodeEnv, probe: &dyn Fn(&Path) -> bool) -> serde_json::Value {
    let outcome = resolve_node_with(env, probe);
    let mut entry = serde_json::Map::new();
    entry.insert("schema".into(), NODE_SCHEMA.into());
    entry.insert(
        "order".into(),
        NODE_ORDER
            .iter()
            .map(|source| source.as_str())
            .collect::<Vec<_>>()
            .into(),
    );
    match &outcome.resolved {
        Some(resolved) => {
            entry.insert("resolved".into(), true.into());
            entry.insert("source".into(), resolved.source.as_str().into());
            entry.insert(
                "path".into(),
                resolved.path.to_string_lossy().into_owned().into(),
            );
        }
        None => {
            entry.insert("resolved".into(), false.into());
        }
    }
    entry.insert(
        "probed".into(),
        outcome
            .probed
            .iter()
            .map(|candidate| {
                serde_json::json!({
                    "source": candidate.source.as_str(),
                    "path": candidate.path.to_string_lossy(),
                })
            })
            .collect::<Vec<_>>()
            .into(),
    );
    entry.insert(
        "refused".into(),
        outcome
            .refused
            .iter()
            .map(|refusal| {
                serde_json::json!({
                    "source": refusal.candidate.source.as_str(),
                    "path": refusal.candidate.path.to_string_lossy(),
                    "reason": refusal.reason,
                })
            })
            .collect::<Vec<_>>()
            .into(),
    );
    serde_json::Value::Object(entry)
}
