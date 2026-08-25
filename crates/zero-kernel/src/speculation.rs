//! Bounded in-process zero-miss speculation.
//!
//! Admission happens only for a finalized unconditional call permit. Capacity
//! refusal chooses ordinary execution before any work launches. Once admitted,
//! the real call must claim the exact rooted result; absence is an invariant
//! failure and never triggers duplicate execution.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use parking_lot::{Condvar, Mutex};
use serde_json::Value;
use zero_abi::{
    CancellationProbe, EngineError, EngineErrorKind, SpeculationAdmission, SpeculationLedger,
    SpeculationPermit, SpeculationState,
};

struct EntryState {
    state: SpeculationState,
    result: Option<Result<Value, EngineError>>,
}

struct Entry {
    permit: SpeculationPermit,
    state: Mutex<EntryState>,
    ready: Condvar,
    cancelled: Arc<AtomicBool>,
}

struct RuntimeInner {
    entries: Mutex<BTreeMap<String, Arc<Entry>>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    ledger: Mutex<SpeculationLedger>,
    inflight: AtomicU32,
    limit: u32,
}

#[derive(Clone)]
struct SpeculationCancellation(Arc<AtomicBool>);

impl CancellationProbe for SpeculationCancellation {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    fn atomic_flag(&self) -> Option<Arc<AtomicBool>> {
        Some(Arc::clone(&self.0))
    }
}

pub struct SpeculationRuntime {
    inner: Arc<RuntimeInner>,
}

impl SpeculationRuntime {
    pub fn new(limit: u32) -> Result<Self, String> {
        if limit == 0 {
            return Err("speculation inflight limit must be positive".into());
        }
        Ok(Self {
            inner: Arc::new(RuntimeInner {
                entries: Mutex::new(BTreeMap::new()),
                workers: Mutex::new(Vec::new()),
                ledger: Mutex::new(SpeculationLedger::default()),
                inflight: AtomicU32::new(0),
                limit,
            }),
        })
    }

    pub fn admit<F>(
        &self,
        permit: SpeculationPermit,
        work: F,
    ) -> Result<SpeculationAdmission, String>
    where
        F: FnOnce(Arc<dyn CancellationProbe>) -> Result<Value, EngineError> + Send + 'static,
    {
        permit.validate()?;
        let key = permit.claim_key()?;
        if self.inner.entries.lock().contains_key(&key) {
            return Err("duplicate speculative claim key".into());
        }
        let admitted = self
            .inner
            .inflight
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < self.inner.limit).then_some(current + 1)
            })
            .is_ok();
        if !admitted {
            let mut ledger = self.inner.ledger.lock();
            ledger.ordinary_admissions = ledger.ordinary_admissions.saturating_add(1);
            return Ok(SpeculationAdmission::Ordinary);
        }

        let entry = Arc::new(Entry {
            permit,
            state: Mutex::new(EntryState {
                state: SpeculationState::Pending,
                result: None,
            }),
            ready: Condvar::new(),
            cancelled: Arc::new(AtomicBool::new(false)),
        });
        self.inner.entries.lock().insert(key, Arc::clone(&entry));
        {
            let mut ledger = self.inner.ledger.lock();
            ledger.dispatched = ledger.dispatched.saturating_add(1);
            ledger.work_units_dispatched = ledger
                .work_units_dispatched
                .saturating_add(entry.permit.work_budget);
            ledger.provider_tokens_dispatched = ledger
                .provider_tokens_dispatched
                .saturating_add(entry.permit.provider_token_budget);
        }

        let inner = Arc::clone(&self.inner);
        let worker_entry = Arc::clone(&entry);
        let handle = std::thread::spawn(move || {
            worker_entry.state.lock().state = SpeculationState::Running;
            let cancellation: Arc<dyn CancellationProbe> =
                Arc::new(SpeculationCancellation(Arc::clone(&worker_entry.cancelled)));
            let result =
                catch_unwind(AssertUnwindSafe(|| work(cancellation))).unwrap_or_else(|_| {
                    Err(EngineError::new(
                        EngineErrorKind::Internal,
                        "speculative work panicked",
                        false,
                    ))
                });
            let cancelled = worker_entry.cancelled.load(Ordering::Acquire);
            {
                let mut state = worker_entry.state.lock();
                if cancelled {
                    state.state = SpeculationState::Cancelled;
                    state.result = None;
                    let mut ledger = inner.ledger.lock();
                    ledger.cancelled = ledger.cancelled.saturating_add(1);
                    ledger.provider_tokens_wasted_upper_bound = ledger
                        .provider_tokens_wasted_upper_bound
                        .saturating_add(worker_entry.permit.provider_token_budget);
                } else {
                    state.state = if result.is_ok() {
                        SpeculationState::Ready
                    } else {
                        SpeculationState::Failed
                    };
                    if result.is_err() {
                        let mut ledger = inner.ledger.lock();
                        ledger.failed = ledger.failed.saturating_add(1);
                    }
                    state.result = Some(result);
                }
                worker_entry.ready.notify_all();
            }
            inner.inflight.fetch_sub(1, Ordering::AcqRel);
        });
        self.inner.workers.lock().push(handle);
        Ok(SpeculationAdmission::Speculated)
    }

    pub fn claim(&self, permit: &SpeculationPermit, wait: Duration) -> Result<Value, EngineError> {
        let key = permit
            .claim_key()
            .map_err(|detail| EngineError::new(EngineErrorKind::InvalidInput, detail, false))?;
        let Some(entry) = self.inner.entries.lock().get(&key).cloned() else {
            let mut ledger = self.inner.ledger.lock();
            ledger.claim_invariant_failures = ledger.claim_invariant_failures.saturating_add(1);
            return Err(EngineError::new(
                EngineErrorKind::Internal,
                "admitted speculative call has no exact claim",
                false,
            ));
        };

        let mut state = entry.state.lock();
        if matches!(
            state.state,
            SpeculationState::Pending | SpeculationState::Running
        ) {
            let timeout = entry.ready.wait_for(&mut state, wait);
            if timeout.timed_out()
                && matches!(
                    state.state,
                    SpeculationState::Pending | SpeculationState::Running
                )
            {
                entry.cancelled.store(true, Ordering::Release);
                return Err(EngineError::new(
                    EngineErrorKind::Deadline,
                    "speculative claim exceeded its bounded wait",
                    false,
                ));
            }
        }
        match state.state {
            SpeculationState::Ready => match state.result.take() {
                Some(Ok(value)) => {
                    state.state = SpeculationState::Claimed;
                    let mut ledger = self.inner.ledger.lock();
                    ledger.claim_hits = ledger.claim_hits.saturating_add(1);
                    ledger.work_units_claimed = ledger
                        .work_units_claimed
                        .saturating_add(entry.permit.work_budget);
                    ledger.provider_tokens_claimed = ledger
                        .provider_tokens_claimed
                        .saturating_add(entry.permit.provider_token_budget);
                    Ok(value)
                }
                _ => Err(EngineError::new(
                    EngineErrorKind::Internal,
                    "ready speculation has no value",
                    false,
                )),
            },
            SpeculationState::Failed => match state.result.take() {
                Some(Err(error)) => Err(error),
                _ => Err(EngineError::new(
                    EngineErrorKind::Internal,
                    "failed speculation has no typed error",
                    false,
                )),
            },
            SpeculationState::Cancelled => Err(EngineError::new(
                EngineErrorKind::Cancelled,
                "speculative call was cancelled",
                false,
            )),
            other => Err(EngineError::new(
                EngineErrorKind::Internal,
                format!("speculative claim reached nonterminal state {other:?}"),
                false,
            )),
        }
    }

    pub fn ledger(&self) -> SpeculationLedger {
        self.inner.ledger.lock().clone()
    }

    pub fn end_turn(&self) -> Result<SpeculationLedger, String> {
        for entry in self.inner.entries.lock().values() {
            let mut state = entry.state.lock();
            match state.state {
                SpeculationState::Pending | SpeculationState::Running => {
                    entry.cancelled.store(true, Ordering::Release);
                }
                SpeculationState::Ready => {
                    entry.cancelled.store(true, Ordering::Release);
                    state.state = SpeculationState::Cancelled;
                    state.result = None;
                    let mut ledger = self.inner.ledger.lock();
                    ledger.cancelled = ledger.cancelled.saturating_add(1);
                    ledger.wasted_ready = ledger.wasted_ready.saturating_add(1);
                    ledger.provider_tokens_wasted_upper_bound = ledger
                        .provider_tokens_wasted_upper_bound
                        .saturating_add(entry.permit.provider_token_budget);
                }
                SpeculationState::Claimed
                | SpeculationState::Cancelled
                | SpeculationState::Failed => {}
            }
            entry.ready.notify_all();
        }
        for worker in self.inner.workers.lock().drain(..) {
            worker
                .join()
                .map_err(|_| "speculation worker join failed".to_string())?;
        }
        let ledger = self.ledger();
        ledger.validate()?;
        Ok(ledger)
    }
}

impl Drop for SpeculationRuntime {
    fn drop(&mut self) {
        let _ = self.end_turn();
    }
}
