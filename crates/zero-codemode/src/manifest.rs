//! Single-manifest harness location contract.
//!
//! A harness embedding ZeroStack must know four executables, two JavaScript
//! modules, a Node runtime, and where state lands. Hand-authoring those as
//! absolute paths in a shipped config bakes one developer's worktree into every
//! install. This module emits one versioned manifest instead, resolved with the
//! same precedence grammar as [crate::discovery]: no second discovery contract.
//!
//! Executables come from [crate::discovery]. Node and the JavaScript modules
//! extend that grammar with one extra rule, [Source::Explicit], for a direct
//! file pin, and are otherwise looked up under the same install roots.

use std::path::{Path, PathBuf};

use crate::discovery::{
    BIN_DIR, Candidate, DEV_TARGET_SUBDIR, DISCOVERY_SCHEMA, DiscoveryEnv, HarnessBinary, Source,
    candidates, resolve_all,
};

/// Versioned manifest schema, stable across surfaces.
pub const MANIFEST_SCHEMA: &str = "zerostack.locate.v1";

/// Module subdirectory of an install root.
pub const LIB_DIR: &str = "lib";

/// Journal subdirectory of a resolved engine store directory.
pub const JOURNAL_DIR: &str = "journal";

/// Direct file pin for the Node runtime.
pub const NODE_ENV: &str = "ZEROSTACK_NODE";

/// Direct file pin for the aggregate runtime module.
pub const RUNTIME_MODULE_ENV: &str = "ZEROSTACK_RUNTIME_MODULE";

/// Direct file pin for the substrate router module.
pub const SUBSTRATE_MODULE_ENV: &str = "ZEROSTACK_SUBSTRATE_MODULE";

/// Stable refusal reason for a per-shell runtime directory.
pub const EPHEMERAL_REASON: &str = "ephemeral_path";

/// Path fragments that identify a per-shell runtime directory.
///
/// An fnm multishell path is keyed by pid and timestamp and disappears with the
/// shell that made it, so pinning one produces an integration that dies silently
/// later. Such a candidate is refused with a reason instead of being resolved.
/// Broader ephemeral-runtime policy is owned by zerostack-f6qt.
pub const EPHEMERAL_MARKERS: &[&str] = &["fnm_multishells"];

/// A non-binary artifact a harness must also locate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessArtifact {
    /// Node runtime that hosts the aggregate JavaScript runtime.
    Node,
    /// Aggregate raw-worker runtime module.
    RuntimeModule,
    /// Substrate router module.
    SubstrateModule,
}

/// Every artifact a harness must resolve, in manifest order.
pub const HARNESS_ARTIFACTS: [HarnessArtifact; 3] = [
    HarnessArtifact::Node,
    HarnessArtifact::RuntimeModule,
    HarnessArtifact::SubstrateModule,
];

impl HarnessArtifact {
    /// File name looked for under an install root.
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::RuntimeModule => "raw-runtime.js",
            Self::SubstrateModule => "substrates.js",
        }
    }

    /// Stable manifest key.
    pub const fn manifest_key(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::RuntimeModule => "runtime_module",
            Self::SubstrateModule => "substrate_module",
        }
    }

    /// Environment variable carrying a direct file pin.
    pub const fn env_var(self) -> &'static str {
        match self {
            Self::Node => NODE_ENV,
            Self::RuntimeModule => RUNTIME_MODULE_ENV,
            Self::SubstrateModule => SUBSTRATE_MODULE_ENV,
        }
    }

    /// Subdirectory of an install root holding this artifact.
    pub const fn subdir(self) -> &'static str {
        match self {
            Self::Node => BIN_DIR,
            Self::RuntimeModule | Self::SubstrateModule => LIB_DIR,
        }
    }

    /// True when only an executable file is usable.
    pub const fn must_be_executable(self) -> bool {
        matches!(self, Self::Node)
    }
}

/// Direct file pins, captured once so resolution reads no process state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactEnv {
    node: Option<PathBuf>,
    runtime_module: Option<PathBuf>,
    substrate_module: Option<PathBuf>,
}

impl ArtifactEnv {
    /// Read the process environment.
    pub fn from_process() -> Self {
        Self {
            node: pin(NODE_ENV),
            runtime_module: pin(RUNTIME_MODULE_ENV),
            substrate_module: pin(SUBSTRATE_MODULE_ENV),
        }
    }

    /// Build explicitly, for tests and for harnesses that already parsed config.
    ///
    /// A relative pin would make resolution depend on the spawning working
    /// directory, so it is discarded rather than half-honored.
    pub fn new(
        node: Option<PathBuf>,
        runtime_module: Option<PathBuf>,
        substrate_module: Option<PathBuf>,
    ) -> Self {
        Self {
            node: node.filter(|path| path.is_absolute()),
            runtime_module: runtime_module.filter(|path| path.is_absolute()),
            substrate_module: substrate_module.filter(|path| path.is_absolute()),
        }
    }

    fn get(&self, artifact: HarnessArtifact) -> Option<&Path> {
        match artifact {
            HarnessArtifact::Node => self.node.as_deref(),
            HarnessArtifact::RuntimeModule => self.runtime_module.as_deref(),
            HarnessArtifact::SubstrateModule => self.substrate_module.as_deref(),
        }
    }
}

fn pin(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

/// Candidate paths for `artifact`, highest precedence first.
///
/// Pure: no filesystem access. The install roots and their order are the ones
/// [candidates] already established, reached through the aggregate host's own
/// candidate list so the two can never drift apart.
pub fn artifact_candidates(
    artifact: HarnessArtifact,
    env: &DiscoveryEnv,
    artifacts: &ArtifactEnv,
) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let push = |out: &mut Vec<Candidate>, source: Source, path: PathBuf| {
        if !out.iter().any(|candidate| candidate.path == path) {
            out.push(Candidate { source, path });
        }
    };

    if let Some(explicit) = artifacts.get(artifact) {
        push(&mut out, Source::Explicit, explicit.to_path_buf());
    }
    for candidate in candidates(HarnessBinary::AggregateHost, env) {
        let Some(root) = install_root(&candidate) else {
            continue;
        };
        push(
            &mut out,
            candidate.source,
            root.join(artifact.subdir()).join(artifact.file_name()),
        );
    }
    out
}

/// The install root implied by a probed aggregate-host path.
///
/// A dev checkout points at `<root>/<Repo>/target/release`, whose sibling
/// artifacts live beside the repository rather than beside the executable; every
/// other rule puts the executable directly in `<root>/bin`.
fn install_root(candidate: &Candidate) -> Option<PathBuf> {
    let dir = candidate.path.parent()?;
    if candidate.source == Source::DevCheckout {
        let depth = Path::new(DEV_TARGET_SUBDIR).components().count();
        return dir.ancestors().nth(depth).map(Path::to_path_buf);
    }
    if dir.file_name().is_some_and(|name| name == BIN_DIR) {
        return dir.parent().map(Path::to_path_buf);
    }
    Some(dir.to_path_buf())
}

/// A candidate rejected before it was probed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// Candidate that was refused.
    pub candidate: Candidate,
    /// Stable machine-readable reason.
    pub reason: &'static str,
}

/// Outcome of resolving one artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactOutcome {
    /// Accepted candidate, when one was found.
    pub resolved: Option<Candidate>,
    /// Candidates probed, in precedence order.
    pub probed: Vec<Candidate>,
    /// Candidates rejected before probing.
    pub refused: Vec<Refusal>,
}

/// True when `path` lives in a directory that will not outlive the shell.
pub fn is_ephemeral(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        EPHEMERAL_MARKERS.iter().any(|marker| name.contains(marker))
    })
}

/// Resolve `artifact`, refusing ephemeral candidates before probing them.
pub fn resolve_artifact(
    artifact: HarnessArtifact,
    env: &DiscoveryEnv,
    artifacts: &ArtifactEnv,
    probe: &dyn Fn(&Path) -> bool,
) -> ArtifactOutcome {
    let mut probed = Vec::new();
    let mut refused = Vec::new();
    for candidate in artifact_candidates(artifact, env, artifacts) {
        if is_ephemeral(&candidate.path) {
            refused.push(Refusal {
                candidate,
                reason: EPHEMERAL_REASON,
            });
            continue;
        }
        probed.push(candidate.clone());
        if probe(&candidate.path) {
            return ArtifactOutcome {
                resolved: Some(candidate),
                probed,
                refused,
            };
        }
    }
    ArtifactOutcome {
        resolved: None,
        probed,
        refused,
    }
}

/// Store locations a harness must share with the engines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StorePaths {
    /// Resolved engine store directory.
    pub store_root: Option<PathBuf>,
    /// Journal directory inside the store root.
    pub journal_dir: Option<PathBuf>,
}

impl StorePaths {
    /// Derive the journal directory from a resolved engine store directory.
    pub fn from_store_root(store_root: PathBuf) -> Self {
        Self {
            journal_dir: Some(store_root.join(JOURNAL_DIR)),
            store_root: Some(store_root),
        }
    }
}

/// Manifest content that is reported rather than resolved from the layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestFacts {
    /// Aggregate host package version.
    pub host_version: String,
    /// Aggregate host wire protocol identifier.
    pub protocol: String,
    /// Store locations, absent when the layout could not be resolved.
    pub store: StorePaths,
}

/// Resolution rules the manifest reports, highest precedence first.
pub fn manifest_order() -> Vec<&'static str> {
    vec![
        Source::Explicit.as_str(),
        Source::Home.as_str(),
        Source::DevCheckout.as_str(),
        Source::XdgData.as_str(),
        Source::PlatformInstall.as_str(),
        Source::Path.as_str(),
    ]
}

/// Emit the full harness manifest as canonical JSON.
///
/// `executable` admits spawnable executables (the four binaries and Node);
/// `readable` admits JavaScript modules. Both are injected, so the manifest is
/// testable against a synthetic layout with no filesystem access.
pub fn locate_manifest(
    env: &DiscoveryEnv,
    artifacts: &ArtifactEnv,
    facts: &ManifestFacts,
    executable: &dyn Fn(&Path) -> bool,
    readable: &dyn Fn(&Path) -> bool,
) -> serde_json::Value {
    let mut aggregate_host = serde_json::Value::Null;
    let mut binaries = serde_json::Map::new();
    for (binary, outcome) in resolve_all(env, executable) {
        let entry = match outcome {
            Ok(resolved) => serde_json::json!({
                "resolved": true,
                "source": resolved.source.as_str(),
                "path": resolved.path.to_string_lossy(),
            }),
            Err(error) => serde_json::json!({
                "resolved": false,
                "probed": probed_json(&error.probed),
            }),
        };
        if binary == HarnessBinary::AggregateHost {
            aggregate_host = entry;
        } else {
            binaries.insert(binary.config_key().to_owned(), entry);
        }
    }

    let mut manifest = serde_json::Map::new();
    manifest.insert("schema".into(), MANIFEST_SCHEMA.into());
    manifest.insert("order".into(), manifest_order().into());
    manifest.insert("aggregate_host".into(), aggregate_host);
    manifest.insert(
        "binaries".into(),
        serde_json::Value::Object(binaries.clone()),
    );
    for artifact in HARNESS_ARTIFACTS {
        let probe: &dyn Fn(&Path) -> bool = if artifact.must_be_executable() {
            executable
        } else {
            readable
        };
        let outcome = resolve_artifact(artifact, env, artifacts, probe);
        manifest.insert(artifact.manifest_key().to_owned(), artifact_json(&outcome));
    }
    manifest.insert(
        "store_root".into(),
        path_json(facts.store.store_root.as_deref()),
    );
    manifest.insert(
        "journal_dir".into(),
        path_json(facts.store.journal_dir.as_deref()),
    );
    manifest.insert(
        "versions".into(),
        serde_json::json!({
            "host": facts.host_version,
            "protocol": facts.protocol,
            "manifest_schema": MANIFEST_SCHEMA,
            "discovery_schema": DISCOVERY_SCHEMA,
        }),
    );
    // Capabilities are the delegate surfaces a plan can actually reach on this
    // install: an unresolved engine is not a capability.
    let capabilities: Vec<String> = binaries
        .iter()
        .filter(|(_, entry)| entry["resolved"] == serde_json::Value::Bool(true))
        .map(|(key, _)| key.clone())
        .collect();
    manifest.insert("capabilities".into(), capabilities.into());
    serde_json::Value::Object(manifest)
}

/// True for an existing regular file, executable or not.
pub fn is_readable_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
}

fn artifact_json(outcome: &ArtifactOutcome) -> serde_json::Value {
    let mut entry = serde_json::Map::new();
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
            entry.insert("probed".into(), probed_json(&outcome.probed));
        }
    }
    if !outcome.refused.is_empty() {
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
    }
    serde_json::Value::Object(entry)
}

fn probed_json(probed: &[Candidate]) -> serde_json::Value {
    probed
        .iter()
        .map(|candidate| {
            serde_json::json!({
                "source": candidate.source.as_str(),
                "path": candidate.path.to_string_lossy(),
            })
        })
        .collect::<Vec<_>>()
        .into()
}

fn path_json(path: Option<&Path>) -> serde_json::Value {
    match path {
        Some(path) => path.to_string_lossy().into_owned().into(),
        None => serde_json::Value::Null,
    }
}
