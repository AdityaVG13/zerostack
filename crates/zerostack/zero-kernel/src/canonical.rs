use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use fs4::{FileExt, TryLockError};

use zero_abi::{GUEST_METHODS, KernelBudget, KernelContext};
use zero_fs::ZeroFileEngine;
use zero_graph::ZeroStructuralEngine;
use zero_token::ZeroTokenEngine;

use crate::{HostError, ZeroKernel};

const ACTIVATION_LOCK_FILE: &str = ".zero-kernel-activation.lock";
const ACTIVATION_LOCK_BACKOFF: Duration = Duration::from_millis(10);

struct ActivationLock(File);

impl ActivationLock {
    fn acquire(store_root: &Path, deadline: Duration) -> Result<Self, HostError> {
        std::fs::create_dir_all(store_root).map_err(|error| {
            HostError::Event(format!("create activation lock directory: {error}"))
        })?;
        let path = store_root.join(ACTIVATION_LOCK_FILE);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| {
                HostError::Event(format!("open activation lock {}: {error}", path.display()))
            })?;
        let started = Instant::now();
        loop {
            match FileExt::try_lock(&file) {
                Ok(()) => return Ok(Self(file)),
                Err(TryLockError::WouldBlock) if started.elapsed() < deadline => {
                    thread::sleep(
                        ACTIVATION_LOCK_BACKOFF.min(deadline.saturating_sub(started.elapsed())),
                    );
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(HostError::Event(format!(
                        "activation lock {} remained held for {deadline:?}",
                        path.display()
                    )));
                }
                Err(TryLockError::Error(error)) => {
                    return Err(HostError::Event(format!(
                        "acquire activation lock {}: {error}",
                        path.display()
                    )));
                }
            }
        }
    }
}

impl Drop for ActivationLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

impl ZeroKernel {
    /// Build the only canonical engine composition. Domain engines are linked
    /// as typed Rust libraries; no operation registry or transport is involved.
    pub fn canonical(
        project_root: impl AsRef<Path>,
        store_root: impl Into<PathBuf>,
        session_id: impl Into<String>,
        budget: KernelBudget,
    ) -> Result<Self, HostError> {
        Self::canonical_with_tokenizer(project_root, store_root, session_id, budget, None)
    }

    /// Build ZeroKernel with a tokenizer identity supplied by the embedding host.
    /// Recognized bundled tokenizers certify counts; unknown models remain estimators.
    pub fn canonical_with_tokenizer(
        project_root: impl AsRef<Path>,
        store_root: impl Into<PathBuf>,
        session_id: impl Into<String>,
        budget: KernelBudget,
        tokenizer_model: Option<String>,
    ) -> Result<Self, HostError> {
        let project_root = std::fs::canonicalize(project_root.as_ref()).map_err(|error| {
            HostError::InvalidRequest(format!("canonicalize project root: {error}"))
        })?;
        let store_root = store_root.into();
        // Independent processes may activate the same project. Serialize construction
        // while event and transaction logs are scanned; the advisory lock releases
        // on drop or process death.
        let _activation_lock = ActivationLock::acquire(
            &store_root,
            Duration::from_millis(budget.wall_ms.clamp(1, 30_000)),
        )?;
        let contract_digest = direct_contract_digest();
        let files = Arc::new(
            ZeroFileEngine::open(&project_root, &store_root, &contract_digest)
                .map_err(HostError::Engine)?,
        );
        let structural = Arc::new(
            ZeroStructuralEngine::open(&project_root, store_root.join("graph"), &store_root)
                .map_err(HostError::Engine)?,
        );
        let tokens = crate::pulse::PulseRecordingTokens::wrap(
            Arc::new(ZeroTokenEngine::open(&store_root, tokenizer_model)),
            &store_root,
        );
        Self::new(
            KernelContext {
                workspace_root: project_root.clone(),
                project_root,
                session_id: session_id.into(),
                expected_state_root: None,
                contract_digest,
            },
            budget,
            files,
            structural,
            tokens,
            store_root,
        )
    }
}

pub fn direct_contract_digest() -> String {
    let bytes = GUEST_METHODS.join("\n");
    blake3::hash(bytes.as_bytes()).to_hex().to_string()
}
