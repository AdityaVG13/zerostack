use std::path::{Path, PathBuf};
use std::sync::Arc;

use fszero_kernel::ZeroFileEngine;
use graphzero_kernel::ZeroStructuralEngine;
use tokenzero_kernel::ZeroTokenEngine;
use zero_abi::{GUEST_METHODS, KernelBudget, KernelContext};

use crate::{HostError, ZeroKernel};

impl ZeroKernel {
    /// Build the only canonical engine composition. Domain engines are linked
    /// as typed Rust libraries; no operation registry or transport is involved.
    pub fn canonical(
        project_root: impl AsRef<Path>,
        store_root: impl Into<PathBuf>,
        session_id: impl Into<String>,
        budget: KernelBudget,
    ) -> Result<Self, HostError> {
        let project_root = std::fs::canonicalize(project_root.as_ref()).map_err(|error| {
            HostError::InvalidRequest(format!("canonicalize project root: {error}"))
        })?;
        let store_root = store_root.into();
        let contract_digest = direct_contract_digest();
        let files = Arc::new(
            ZeroFileEngine::open(&project_root, &store_root, &contract_digest)
                .map_err(HostError::Engine)?,
        );
        let structural = Arc::new(
            ZeroStructuralEngine::open(&project_root, store_root.join("graph"), &store_root)
                .map_err(HostError::Engine)?,
        );
        let tokens = Arc::new(ZeroTokenEngine::open(&store_root, None));
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
