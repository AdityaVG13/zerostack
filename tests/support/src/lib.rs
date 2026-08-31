//! Shared, test-only fixtures for ZeroStack integration targets.
//! Production crates must not depend on this package. Keep only
//! fixtures with multiple real consumers; one-off builders belong beside their test.

#![deny(unsafe_code)]

use std::fmt::Write as _;
use std::path::Path;
use std::path::PathBuf;
#[cfg(feature = "kernel")]
use std::sync::Arc;

pub mod env;
pub use env::{ScopedEnvVars, lock_env};

pub mod process;
pub use process::assert_completes_within;
#[cfg(unix)]
pub use process::make_fifo;

#[cfg(feature = "git")]
pub mod git;
#[cfg(feature = "git")]
pub use git::git_commit_all;

#[cfg(feature = "graph")]
pub mod graph;
#[cfg(feature = "graph")]
pub use graph::{
    BasicGraphFixture, ReserveFixture, basic_indexed_repo, reserve_indexed_fixture,
    write_alpha_beta_repo,
};

#[cfg(feature = "kernel")]
pub use zero_pulse::{PulseEvent, default_ledger_path};

#[cfg(feature = "kernel")]
use zero_abi::{
    CancellationProbe, CapsuleEventRoots, EngineCallContext, EngineInvocation, KernelBudget,
    KernelLedger, ZERO_KERNEL_PROTOCOL, ZeroHandle, ZeroKernelEvent, ZeroKernelOutcome,
};

/// Hermetic temp dir used as both workspace root and engine store root.
pub struct TempWorkspace {
    dir: tempfile::TempDir,
}

impl TempWorkspace {
    pub fn new(prefix: &str) -> std::io::Result<Self> {
        Ok(Self {
            dir: tempfile::Builder::new().prefix(prefix).tempdir()?,
        })
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn store(&self) -> &Path {
        self.dir.path()
    }

    pub fn path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.dir.path().join(relative)
    }

    pub fn create_dir_all(&self, relative: impl AsRef<Path>) -> std::io::Result<PathBuf> {
        let path = self.path(relative);
        std::fs::create_dir_all(&path)?;
        Ok(path)
    }

    pub fn write(
        &self,
        relative: impl AsRef<Path>,
        content: impl AsRef<[u8]>,
    ) -> std::io::Result<PathBuf> {
        let path = self.path(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
        Ok(path)
    }
}

#[cfg(feature = "kernel")]
struct NoopCancel;

#[cfg(feature = "kernel")]
impl CancellationProbe for NoopCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// Call-scoped invocation bound to `root` (workspace and project).
#[cfg(feature = "kernel")]
pub fn test_invocation(root: &Path, session_id: &str, cell_id: &str) -> EngineInvocation {
    EngineInvocation {
        context: EngineCallContext {
            workspace_root: PathBuf::from(root),
            project_root: PathBuf::from(root),
            session_id: session_id.into(),
            cell_id: cell_id.into(),
            trace_id: format!("{session_id}-{cell_id}"),
            deadline_unix_ms: u64::MAX,
            budget: KernelBudget {
                wall_ms: 1_000,
                cpu_ms: 1_000,
                memory_bytes: 1024 * 1024,
                call_limit: 8,
                task_limit: 2,
                output_byte_limit: 4096,
            },
        },
        cancellation: Arc::new(NoopCancel),
    }
}

/// Valid lowercase 64-hex root: repeats `hex` 64 times.
pub fn root64(hex: char) -> String {
    assert!(
        hex.is_ascii_hexdigit() && !hex.is_ascii_uppercase(),
        "root64 expects lowercase hex digit, got {hex}"
    );
    std::iter::repeat_n(hex, 64).collect()
}

/// Independent SHA-256 hex oracle shared by storage tests.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = <sha2::Sha256 as sha2::Digest>::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut hex, "{byte:02x}").unwrap();
    }
    hex
}

/// Canonical [`CapsuleEventRoots`] fixture used across zero-kernel and
/// zero-store event tests.
#[cfg(feature = "kernel")]
pub fn capsule_roots() -> CapsuleEventRoots {
    CapsuleEventRoots {
        capsule_root: root64('1'),
        capsule_object: ZeroHandle::from_digest(&root64('a')).unwrap(),
        provider_root: root64('2'),
        cache_root: root64('3'),
        speculation_root: root64('4'),
        effect_root: root64('5'),
        quality_root: root64('6'),
        occurrence_root: root64('7'),
    }
}

/// Completed, capsule-rooted [`ZeroKernelEvent`] over `visible` bytes.
/// `model_visible_digest` is `blake3(visible)` hex, matching publish oracles.
#[cfg(feature = "kernel")]
pub fn capsule_event(visible: &[u8]) -> ZeroKernelEvent {
    ZeroKernelEvent {
        protocol: ZERO_KERNEL_PROTOCOL.into(),
        session_id: "session".into(),
        cell_id: "cell".into(),
        source_digest: "source".into(),
        contract_digest: "contract".into(),
        policy_digest: "policy".into(),
        state_root_before: None,
        state_root_after: None,
        input_handles: vec![],
        output_handles: vec![],
        outcome: ZeroKernelOutcome::Completed,
        ledger: KernelLedger::default(),
        model_visible_digest: blake3::hash(visible).to_hex().to_string(),
        turn: None,
        capsule: Some(capsule_roots()),
    }
}
