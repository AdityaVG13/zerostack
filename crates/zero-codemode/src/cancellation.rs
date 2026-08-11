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
