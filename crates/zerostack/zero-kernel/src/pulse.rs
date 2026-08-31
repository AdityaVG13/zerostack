//! Hub Pulse recording around TokenEngine calls.

use std::path::PathBuf;
use std::sync::Arc;

use zero_abi::{
    CompressionRequest, CompressionResult, EngineError, EngineInvocation, ExpandOptions,
    ProjectionRequest, ProjectionResult, TokenAccounting, TokenEngine, ZeroHandle,
};

pub(crate) struct PulseRecordingTokens {
    inner: Arc<dyn TokenEngine>,
    store_root: PathBuf,
}

impl PulseRecordingTokens {
    pub(crate) fn wrap(
        inner: Arc<dyn TokenEngine>,
        pulse_root: impl Into<PathBuf>,
    ) -> Arc<dyn TokenEngine> {
        Arc::new(Self {
            inner,
            store_root: pulse_root.into(),
        })
    }

    fn record(
        &self,
        invocation: &EngineInvocation,
        tool: &str,
        accounting: &TokenAccounting,
    ) -> Result<(), EngineError> {
        zero_pulse::record_kernel_accounting(
            &self.store_root,
            &invocation.context.session_id,
            &invocation.context.cell_id,
            tool,
            accounting,
        )
        .map_err(|error| {
            EngineError::new(
                zero_abi::EngineErrorKind::Internal,
                format!("hub Pulse record failed: {error}"),
                true,
            )
        })
    }
}

impl TokenEngine for PulseRecordingTokens {
    fn measure(
        &self,
        invocation: &EngineInvocation,
        bytes: &[u8],
    ) -> Result<TokenAccounting, EngineError> {
        let accounting = self.inner.measure(invocation, bytes)?;
        self.record(invocation, "measure", &accounting)?;
        Ok(accounting)
    }

    fn certify(
        &self,
        invocation: &EngineInvocation,
        bytes: &[u8],
        claimed: &TokenAccounting,
    ) -> Result<zero_abi::CertifyResult, EngineError> {
        self.inner.certify(invocation, bytes, claimed)
    }

    fn project(
        &self,
        invocation: &EngineInvocation,
        request: ProjectionRequest,
    ) -> Result<ProjectionResult, EngineError> {
        let result = self.inner.project(invocation, request)?;
        self.record(invocation, "project", &result.accounting)?;
        Ok(result)
    }

    fn compress(
        &self,
        invocation: &EngineInvocation,
        request: CompressionRequest,
    ) -> Result<CompressionResult, EngineError> {
        let result = self.inner.compress(invocation, request)?;
        self.record(invocation, "compress", &result.accounting)?;
        Ok(result)
    }

    fn expand(
        &self,
        invocation: &EngineInvocation,
        handle: &ZeroHandle,
        options: ExpandOptions,
    ) -> Result<Vec<u8>, EngineError> {
        self.inner.expand(invocation, handle, options)
    }
}
