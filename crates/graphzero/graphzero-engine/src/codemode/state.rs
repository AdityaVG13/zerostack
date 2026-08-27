//! Execution state tracking.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use graphzero_store::Snapshot as StoreSnapshot;

use crate::dispatcher::{AdapterKind, CancellationToken, EngineContext};

use super::CodeModeHostOps;
use super::errors::{cancelled_error, policy_error};
use super::types::{CodeModeError, CodeModeLimits, StepRecord};

/// Counts EngineContext constructions on the CodeMode execution path (o2uq.9 kill test).
static CODEMODE_CONTEXT_BUILDS: AtomicU64 = AtomicU64::new(0);

pub fn codemode_context_build_count() -> u64 {
    CODEMODE_CONTEXT_BUILDS.load(Ordering::SeqCst)
}

pub fn reset_codemode_context_build_count_for_tests() {
    CODEMODE_CONTEXT_BUILDS.store(0, Ordering::SeqCst);
}

pub(crate) struct ExecutionState<'a> {
    pub(crate) snapshot: &'a StoreSnapshot,
    refreshed_snapshot: Option<StoreSnapshot>,
    pub(crate) host: Option<&'a dyn CodeModeHostOps>,
    pub(crate) steps: Vec<StepRecord>,
    pub(crate) refs: Vec<String>,
    pub(crate) logical_ops: u64,
    pub(crate) physical_ops: u64,
    pub(crate) batched_ops: u64,
    pub(crate) cache_hits: u64,
    pub(crate) cache_misses: u64,
    pub(crate) store_writes: u64,
    pub(crate) parallel_groups: u64,
    pub(crate) bytes_materialized: usize,
    pub(crate) seen_queries: BTreeSet<String>,
    /// Hashes of texts explicitly materialized via expand: these are judgment
    /// payloads the model asked to see, so ref-first return rewriting must
    /// never fold them back into a ref.
    pub(crate) materialized_texts: BTreeSet<u64>,
    pub(crate) limits: CodeModeLimits,
    /// Warm engine context shared across all domain ops in this plan (o2uq.9).
    engine_ctx: Option<EngineContext>,
    count_context_build: bool,
    cancellation: CancellationToken,
    deadline: Option<Instant>,
}

impl<'a> ExecutionState<'a> {
    pub(crate) fn new(snapshot: &'a StoreSnapshot, host: Option<&'a dyn CodeModeHostOps>) -> Self {
        Self {
            snapshot,
            refreshed_snapshot: None,
            host,
            steps: Vec::new(),
            refs: Vec::new(),
            logical_ops: 0,
            physical_ops: 0,
            batched_ops: 0,
            cache_hits: 0,
            cache_misses: 0,
            store_writes: 0,
            parallel_groups: 0,
            bytes_materialized: 0,
            seen_queries: BTreeSet::new(),
            materialized_texts: BTreeSet::new(),
            limits: CodeModeLimits::default(),
            engine_ctx: None,
            count_context_build: true,
            cancellation: CancellationToken::default(),
            deadline: None,
        }
    }

    pub(crate) fn new_parallel(snapshot: &'a StoreSnapshot) -> Self {
        let mut state = Self::new(snapshot, None);
        state.count_context_build = false;
        state
    }

    pub(crate) fn set_control(
        &mut self,
        cancellation: CancellationToken,
        deadline: Option<Instant>,
    ) {
        self.cancellation = cancellation;
        self.deadline = deadline;
    }

    pub(crate) fn cancellation_requested(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    /// Wall time consumed so far, derived from the deadline the plan was given
    /// so no second clock can disagree with the one that fired.
    fn elapsed_ms(&self) -> u64 {
        let budget = u64::try_from(self.limits.max_wall_ms).unwrap_or(u64::MAX);
        let Some(deadline) = self.deadline else {
            return 0;
        };
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        budget.saturating_sub(remaining)
    }

    /// Index posture at the moment time ran out. A cold index needs a different
    /// remedy (index first) than a plan that is simply too large.
    fn index_state(&self) -> &'static str {
        if self.snapshot.entry.snapshot_id == 0 {
            "cold"
        } else {
            "warm"
        }
    }

    /// Most recent durable ref this run produced, if any, so the caller can
    /// recover partial work instead of re-running from scratch.
    fn resume_ref(&self) -> Option<String> {
        self.refs.last().cloned()
    }

    pub(crate) fn current_snapshot(&self) -> &StoreSnapshot {
        self.refreshed_snapshot.as_ref().unwrap_or(self.snapshot)
    }

    /// One EngineContext per plan session — fusion of multi-op CodeMode paths.
    pub(crate) fn engine_context(&mut self) -> &EngineContext {
        if self.engine_ctx.is_none() {
            if self.count_context_build {
                CODEMODE_CONTEXT_BUILDS.fetch_add(1, Ordering::SeqCst);
            }
            let cancellation = self.cancellation.clone();
            let deadline = self.deadline;
            let mut ctx =
                EngineContext::from_snapshot(self.current_snapshot(), AdapterKind::CodeMode)
                    .with_cancellation_token(cancellation);
            if let Some(deadline) = deadline {
                ctx = ctx.with_deadline(deadline);
            }
            self.engine_ctx = Some(ctx);
        }
        self.engine_ctx.as_ref().expect("engine_ctx just set")
    }

    pub(crate) fn refresh_snapshot(&mut self) -> Result<(), CodeModeError> {
        let store_root = self.current_snapshot().store_root.clone();
        let repo_root = self.current_snapshot().repo_root.clone();
        self.refreshed_snapshot = Some(
            StoreSnapshot::open(&store_root, repo_root.as_deref()).map_err(|error| {
                super::errors::substrate_error(error.to_string(), "graph.index")
            })?,
        );
        // Snapshot identity changed — drop warm context so the next op rebuilds once.
        self.engine_ctx = None;
        Ok(())
    }

    pub(crate) fn note_materialized(&mut self, text: &str) {
        self.materialized_texts.insert(Self::text_key(text));
    }

    pub(crate) fn is_materialized(&self, text: &str) -> bool {
        self.materialized_texts.contains(&Self::text_key(text))
    }

    fn text_key(text: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    }

    /// Cheap clones of control inputs for parallel JSON workers.
    pub(crate) fn control_handles(&self) -> (CancellationToken, Option<Instant>) {
        (self.cancellation.clone(), self.deadline)
    }

    pub(crate) fn guard_ops(&self, step: &str) -> Result<(), CodeModeError> {
        if self.cancellation.is_cancelled() {
            return Err(cancelled_error(format!("client cancelled during {step}")));
        }
        if let Some(deadline) = self.deadline
            && Instant::now() >= deadline
        {
            return Err(super::errors::deadline_error_with_context(
                step,
                self.elapsed_ms(),
                u64::try_from(self.limits.max_wall_ms).unwrap_or(u64::MAX),
                self.index_state(),
                self.resume_ref(),
            ));
        }
        if self.logical_ops > self.limits.max_logical_ops {
            return Err(policy_error(
                format!(
                    "logical op limit exceeded: {} > {}",
                    self.logical_ops, self.limits.max_logical_ops
                ),
                step,
            ));
        }
        if self.physical_ops > self.limits.max_physical_ops {
            return Err(policy_error(
                format!(
                    "physical op limit exceeded: {} > {}",
                    self.physical_ops, self.limits.max_physical_ops
                ),
                step,
            ));
        }
        if self.refs.len() > self.limits.max_refs_emitted {
            return Err(policy_error(
                format!(
                    "ref count limit exceeded: {} > {}",
                    self.refs.len(),
                    self.limits.max_refs_emitted
                ),
                step,
            ));
        }
        Ok(())
    }

    pub(crate) fn push_ref(&mut self, r: String) -> Result<(), CodeModeError> {
        self.refs.push(r);
        self.guard_ops("refs")
    }
}
