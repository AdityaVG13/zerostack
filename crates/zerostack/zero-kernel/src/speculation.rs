//! Bounded in-process zero-miss speculation. Admission happens only for a finalized unconditional
//! call permit carrying exact prepared identity. Capacity refusal chooses ordinary execution before
//! any work launches.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use serde_json::Value;
use zero_abi::{
    CancellationProbe, EngineError, EngineErrorKind, SpeculationAdmission, SpeculationLedger,
    SpeculationPermit, SpeculationState,
};

use crate::PreparedCell;

/// Typed outcomes for zero-miss speculation — covers the five observable contracts the runtime
/// guarantees without rerunning work * `Ordinary` — execution proceeded without speculation (plan
/// marked the node ordinary or host chose not to speculate). * `SpeculativeWin` — speculative.
#[derive(Debug)]
pub enum SpeculationOutcome {
    Ordinary,
    SpeculativeWin(Value),
    SpeculativeDomainError(EngineError),
    Cancelled(EngineError),
    CapacityRefusal,
}

/// Typed claim outcome when a permit was admitted. Splits the speculative
/// execution result without overlapping the admission decision.
#[derive(Debug)]
pub enum SpeculationClaimOutcome {
    Hit(Value),
    DomainError(EngineError),
    Cancelled(EngineError),
    InvariantFailure(EngineError),
}

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

    /// Current admitted inflight count — minimal API for host integration.
    pub fn inflight(&self) -> u32 {
        self.inner.inflight.load(Ordering::Acquire)
    }

    /// Whether any speculative work is still admitted.
    pub fn is_empty(&self) -> bool {
        self.inner.entries.lock().is_empty() && self.inner.workers.lock().is_empty()
    }

    /// Check whether a permit is currently admitted (exact claim exists).
    pub fn is_admitted(&self, permit: &SpeculationPermit) -> bool {
        permit
            .claim_key()
            .map(|key| self.inner.entries.lock().contains_key(&key))
            .unwrap_or(false)
    }

    /// Admit one finalized unconditional permit. Validates the permit (unconditional, certified-pure,
    /// cancellation-bound, positive budgets) and enforces the bounded capacity *before* any thread is
    /// spawned.
    pub fn admit<F>(
        &self,
        permit: SpeculationPermit,
        work: F,
    ) -> Result<SpeculationAdmission, String>
    where
        F: FnOnce(Arc<dyn CancellationProbe>) -> Result<Value, EngineError> + Send + 'static,
    {
        self.admit_inner(permit, None, work)
    }

    /// Admit one finalized unconditional permit bound to an exact `PreparedCell` identity. In addition
    /// to `admit` validation the permit's binding and finalized source root must match the sealed
    /// prepared cell exactly; any drift is rejected before launch with no worker created.
    pub fn admit_prepared<F>(
        &self,
        permit: SpeculationPermit,
        prepared: &PreparedCell,
        work: F,
    ) -> Result<SpeculationAdmission, String>
    where
        F: FnOnce(Arc<dyn CancellationProbe>) -> Result<Value, EngineError> + Send + 'static,
    {
        self.admit_inner(permit, Some(prepared), work)
    }

    fn admit_inner<F>(
        &self,
        permit: SpeculationPermit,
        prepared: Option<&PreparedCell>,
        work: F,
    ) -> Result<SpeculationAdmission, String>
    where
        F: FnOnce(Arc<dyn CancellationProbe>) -> Result<Value, EngineError> + Send + 'static,
    {
        // Finalized unconditional permit is mandatory — no prediction path.
        permit.validate()?;
        if let Some(prepared) = prepared {
            // Exact prepared identity: binding and finalized source root must be
            // byte-exact with the sealed cell. No drift, no re-dispatch.
            if &permit.binding != prepared.binding() {
                return Err("speculation permit binding does not match prepared identity".into());
            }
            if permit.proof.finalized_source_root != prepared.digest() {
                return Err(
                    "speculation permit finalized source does not match prepared identity".into(),
                );
            }
        }
        let key = permit.claim_key()?;
        let entry = Arc::new(Entry {
            permit,
            state: Mutex::new(EntryState {
                state: SpeculationState::Pending,
                result: None,
            }),
            ready: Condvar::new(),
            cancelled: Arc::new(AtomicBool::new(false)),
        });
        let mut entries = self.inner.entries.lock();
        if entries.contains_key(&key) {
            return Err("duplicate speculative claim key".into());
        }
        // Capacity refusal BEFORE launch — preserves ordinary execution.
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
        entries.insert(key, Arc::clone(&entry));
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
            // Entry state transitions: Pending -> Running -> Ready/Failed/Cancelled.
            // No duplicate commit: result is stored exactly once and taken on claim.
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
            {
                let mut state = worker_entry.state.lock();
                let cancelled = worker_entry.cancelled.load(Ordering::Acquire);
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
        // Admission becomes visible atomically with worker registration. end_turn
        // cannot snapshot the entry before its JoinHandle is owned.
        drop(entries);
        Ok(SpeculationAdmission::Speculated)
    }

    /// Claim the exact rooted result for an admitted permit. Waits at most `wait`; on timeout the
    /// worker is cancelled but remains joined via `end_turn`/Drop. A missing claim is an invariant
    /// failure and never triggers duplicate execution.
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
        // Prevent duplicate committed result: already-claimed entry is terminal.
        if state.state == SpeculationState::Claimed {
            return Err(EngineError::new(
                EngineErrorKind::Internal,
                "speculative claim already consumed",
                false,
            ));
        }
        let deadline = Instant::now()
            .checked_add(wait)
            .unwrap_or_else(Instant::now);
        while matches!(
            state.state,
            SpeculationState::Pending | SpeculationState::Running
        ) {
            if entry.cancelled.load(Ordering::Acquire) {
                return Err(EngineError::new(
                    EngineErrorKind::Cancelled,
                    "speculative call was cancelled",
                    false,
                ));
            }
            let now = Instant::now();
            if now >= deadline {
                entry.cancelled.store(true, Ordering::Release);
                return Err(EngineError::new(
                    EngineErrorKind::Deadline,
                    "speculative claim exceeded its bounded wait",
                    false,
                ));
            }
            let timeout = entry.ready.wait_for(&mut state, deadline - now);
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
        // Second check after wake: claimed or cancelled while waiting.
        if state.state == SpeculationState::Claimed {
            return Err(EngineError::new(
                EngineErrorKind::Internal,
                "speculative claim already consumed",
                false,
            ));
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
                Some(Err(error)) => {
                    state.state = SpeculationState::Claimed;
                    Err(error)
                }
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

    /// Typed claim outcome for host integration — maps the same invariants as `claim` onto the five
    /// typed contracts: speculative win, speculative domain error, cancellation, ordinary (no
    /// admission), and capacity refusal (handled at admission).
    pub fn claim_outcome(
        &self,
        permit: &SpeculationPermit,
        wait: Duration,
    ) -> SpeculationClaimOutcome {
        match self.claim(permit, wait) {
            Ok(value) => SpeculationClaimOutcome::Hit(value),
            Err(error) if error.kind == EngineErrorKind::Cancelled => {
                SpeculationClaimOutcome::Cancelled(error)
            }
            Err(error) if error.kind == EngineErrorKind::Deadline => {
                SpeculationClaimOutcome::Cancelled(error)
            }
            Err(error) if error.kind == EngineErrorKind::Internal => {
                SpeculationClaimOutcome::InvariantFailure(error)
            }
            Err(error) => SpeculationClaimOutcome::DomainError(error),
        }
    }

    pub fn ledger(&self) -> SpeculationLedger {
        self.inner.ledger.lock().clone()
    }

    /// End of turn: cancel any pending/running work, convert any unclaimed
    /// Ready into Cancelled (wasted_ready), then join or drain every admitted
    /// worker. No worker is leaked. Returns the validated ledger.
    pub fn end_turn(&self) -> Result<SpeculationLedger, String> {
        let entries: Vec<Arc<Entry>> = self.inner.entries.lock().values().cloned().collect();
        for entry in entries {
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
        let mut join_failed = false;
        for worker in self.inner.workers.lock().drain(..) {
            join_failed |= worker.join().is_err();
        }
        self.inner.entries.lock().clear();
        let ledger = self.ledger();
        ledger.validate()?;
        if join_failed {
            return Err("speculation worker join failed".into());
        }
        Ok(ledger)
    }
}

impl Drop for SpeculationRuntime {
    fn drop(&mut self) {
        let _ = self.end_turn();
    }
}
