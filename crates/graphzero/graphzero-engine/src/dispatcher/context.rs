//! Engine context shared by every adapter calling the domain dispatcher.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// Which surface invoked the domain engine (telemetry only; semantics are identical).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AdapterKind {
    FastMcp,
    CodeMode,
    Cli,
    PrivateWorker,
}

impl AdapterKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FastMcp => "fastmcp",
            Self::CodeMode => "codemode",
            Self::Cli => "cli",
            Self::PrivateWorker => "private_worker",
        }
    }
}

/// Cloneable cancellation handle shared by a transport and in-flight domain work.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn from_arc(flag: Arc<AtomicBool>) -> Self {
        Self(flag)
    }

    pub fn as_arc(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.0)
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Transport-neutral execution context for one domain operation.
#[derive(Clone, Debug)]
pub struct EngineContext {
    pub repo_root: PathBuf,
    pub store_root: PathBuf,
    pub adapter: AdapterKind,
    /// Legacy preflight cancellation bit used by conformance adapters.
    pub cancelled: bool,
    /// Live cancellation handle used by transports while work is in flight.
    cancellation: CancellationToken,
    /// Hard wall deadline for this call.
    pub deadline: Option<Instant>,
}

impl EngineContext {
    pub fn for_paths(repo_root: PathBuf, store_root: PathBuf, adapter: AdapterKind) -> Self {
        Self {
            repo_root,
            store_root,
            adapter,
            cancelled: false,
            cancellation: CancellationToken::default(),
            deadline: None,
        }
    }

    pub fn from_snapshot(snapshot: &graphzero_store::Snapshot, adapter: AdapterKind) -> Self {
        Self {
            repo_root: snapshot
                .repo_root
                .clone()
                .unwrap_or_else(|| PathBuf::from(".")),
            store_root: snapshot.store_root.clone(),
            adapter,
            cancelled: false,
            cancellation: CancellationToken::default(),
            deadline: None,
        }
    }

    pub fn with_deadline(mut self, deadline: Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }

    pub fn with_cancellation_token(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = cancellation;
        self
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled || self.cancellation.is_cancelled()
    }

    pub fn repo(&self) -> &Path {
        &self.repo_root
    }

    pub fn store(&self) -> &Path {
        &self.store_root
    }

    pub fn check_point(&self, op: &str) -> Result<(), crate::operation_abi::DomainError> {
        use crate::operation_abi::{DomainError, DomainErrorKind};
        if self.is_cancelled() {
            return Err(DomainError::new(
                DomainErrorKind::Cancelled,
                format!("client cancelled during {op}"),
            )
            .with_op(op));
        }
        if let Some(deadline) = self.deadline
            && Instant::now() >= deadline
        {
            return Err(DomainError::new(
                DomainErrorKind::DeadlineExceeded,
                format!("deadline exceeded during {op}; retry with a smaller budget"),
            )
            .with_op(op)
            .with_retryable(true));
        }
        Ok(())
    }

    pub fn check_preflight(&self, op: &str) -> Result<(), crate::operation_abi::DomainError> {
        use crate::operation_abi::{DomainError, DomainErrorKind};
        if self.is_cancelled() {
            return Err(
                DomainError::new(DomainErrorKind::Cancelled, "client cancelled").with_op(op),
            );
        }
        if let Some(deadline) = self.deadline
            && Instant::now() >= deadline
        {
            return Err(DomainError::new(
                DomainErrorKind::DeadlineExceeded,
                format!(
                    "deadline exceeded before {op} started; nothing executed — retry with a longer deadline or smaller budget"
                ),
            )
            .with_op(op)
            .with_retryable(true));
        }
        Ok(())
    }
}
