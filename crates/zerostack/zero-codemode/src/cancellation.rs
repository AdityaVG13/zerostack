use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Per-request cancellation shared by the interpreter and native adapters.
#[derive(Debug, Clone, Default)]
pub struct CancellationSignal(Arc<AtomicBool>);

impl CancellationSignal {
    pub fn new() -> Self {
        Self::default()
    }

    /// Share an existing atomic flag as this signal. The interpreter and
    /// every adapter observe exactly the flag the caller already holds, so
    /// one caller-owned flag cancels the whole request (host runtime,
    /// connector admission, and adapter calls alike). The call may set the
    /// shared flag when it tears down after a failure, so callers should use
    /// a per-call flag.
    pub fn from_atomic(flag: Arc<AtomicBool>) -> Self {
        Self(flag)
    }

    pub fn as_atomic(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.0)
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
