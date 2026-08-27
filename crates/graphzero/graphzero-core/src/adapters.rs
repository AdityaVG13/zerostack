//! Dynamic-domain adapters (RACC row ZS-GRAPH-010).
//!
//! The adapter surface for non-static domains (runtime traces, external
//! systems) feeding the dependency graph through the refinement loop. The
//! [`DomainAdapter`] contract:
//!
//! - the adapter DECLARES its domain and the truth class of the edges it may
//!   emit: [`TruthClass::RuntimeObserved`] at best -- never a declared/static
//!   class ([`TruthClass::CompilerExact`], `LspExactScoped`,
//!   `SyntaxDerived`, `SoundOverapproximation` are rejected by the
//!   contract);
//! - it emits [`ObservedInfluence`]-compatible records, ingested via
//!   [`ingest_adapter`] which bridges to `RefinementLoop::observe` -- the
//!   loop labels every added edge `RuntimeObserved` and never upgrades it;
//! - it carries an honesty label ([`DomainAdapter::coverage_label`]): what
//!   the adapter can and cannot see. A missing or empty label is a contract
//!   violation, as is a missing domain.
//!
//! [`ReplayTraceAdapter`] is the one concrete reference adapter: a
//! deterministic replay of a recorded `Vec<ObservedInfluence>`, demonstrating
//! the loop adapter -> observed influences -> refinement loop -> edges labeled
//! `RuntimeObserved` -> derivability predicate answers that upgrade from
//! Unknown to Derivable-with-runtime-grade (see its test).

use std::fmt;

use crate::refinement::{ObservedInfluence, RefinementLoop, RefinementOutcome};
use crate::truth::TruthClass;

/// Whether `truth` is an admissible claim class for a dynamic-domain adapter:
/// [`TruthClass::RuntimeObserved`] at best, weaker empirical classes tolerated,
/// static/exact classes never.
#[must_use]
pub const fn is_dynamic_truth_class(truth: TruthClass) -> bool {
    matches!(
        truth,
        TruthClass::RuntimeObserved
            | TruthClass::Historical
            | TruthClass::Heuristic
            | TruthClass::Unknown
    )
}

/// Contract violations of a [`DomainAdapter`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdapterContractError {
    /// The adapter declared no domain.
    MissingDomain,
    /// The adapter carries no honesty label for its coverage.
    MissingCoverageLabel,
    /// The adapter claims a static/exact truth class for its emitted edges.
    /// Dynamic domains are [`TruthClass::RuntimeObserved`] at best.
    ClaimsStaticTruth(TruthClass),
}

impl fmt::Display for AdapterContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingDomain => write!(f, "adapter declared no domain"),
            Self::MissingCoverageLabel => {
                write!(f, "adapter carries no coverage honesty label")
            }
            Self::ClaimsStaticTruth(truth) => write!(
                f,
                "adapter claims static truth class {} for a dynamic domain",
                truth.as_str()
            ),
        }
    }
}

impl std::error::Error for AdapterContractError {}

/// Contract for adapters feeding non-static domains into the dependency graph.
///
/// Implementations are deterministic: the same adapter state yields the same
/// influence record sequence. The contract is enforced by
/// [`DomainAdapter::validate`], which [`ingest_adapter`] runs before emitting
/// any influence.
pub trait DomainAdapter {
    /// The declared domain this adapter observes (e.g. `runtime-trace:exec:2`).
    /// Never empty.
    fn domain(&self) -> &str;

    /// Honesty label: what the adapter can and cannot see (e.g. "traces only
    /// the main process; forked children and network egress are not
    /// observed"). Never empty.
    fn coverage_label(&self) -> &str;

    /// Strongest truth class the adapter claims for its emitted edges:
    /// [`TruthClass::RuntimeObserved`] at best, never a declared/static class.
    fn max_truth_class(&self) -> TruthClass;

    /// The recorded influences this adapter can replay, in deterministic
    /// order. Records are [`ObservedInfluence`]-compatible by construction.
    fn influences(&self) -> Vec<ObservedInfluence>;

    /// Enforce the adapter contract. Cheap; run by [`ingest_adapter`] before
    /// any influence is emitted.
    fn validate(&self) -> Result<(), AdapterContractError> {
        if self.domain().is_empty() {
            return Err(AdapterContractError::MissingDomain);
        }
        if self.coverage_label().is_empty() {
            return Err(AdapterContractError::MissingCoverageLabel);
        }
        if !is_dynamic_truth_class(self.max_truth_class()) {
            return Err(AdapterContractError::ClaimsStaticTruth(
                self.max_truth_class(),
            ));
        }
        Ok(())
    }
}

/// Measured outcome of one [`ingest_adapter`] pass.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdapterIngestReport {
    pub domain: String,
    /// Influences replayed by the adapter.
    pub influences_emitted: usize,
    /// Influences that added a new true edge to the graph.
    pub edges_added: usize,
    /// Influences already represented by an existing edge (idempotent no-ops).
    pub already_represented: usize,
}

/// Bridge: adapter -> `RefinementLoop::observe`.
///
/// Enforces the adapter contract first, then feeds every replayed influence
/// into the refinement loop. Every edge the loop adds is labeled
/// [`TruthClass::RuntimeObserved`] -- the loop never upgrades an observed
/// edge to declared -- so the adapter's `max_truth_class` promise is kept by
/// construction.
pub fn ingest_adapter(
    loop_: &mut RefinementLoop,
    adapter: &dyn DomainAdapter,
) -> Result<AdapterIngestReport, AdapterContractError> {
    adapter.validate()?;
    let influences = adapter.influences();
    let mut edges_added = 0usize;
    let mut already_represented = 0usize;
    for influence in &influences {
        match loop_.observe(influence.clone()) {
            RefinementOutcome::EdgeAdded { .. } => edges_added += 1,
            RefinementOutcome::AlreadyRepresented { .. } => already_represented += 1,
        }
    }
    Ok(AdapterIngestReport {
        domain: adapter.domain().to_owned(),
        influences_emitted: influences.len(),
        edges_added,
        already_represented,
    })
}

/// Reference adapter: deterministic replay of a recorded influence trace.
///
/// Demonstrates the full loop: adapter -> observed influences -> refinement
/// loop -> edges labeled `RuntimeObserved` -> derivability predicate answers
/// that upgrade from Unknown to Derivable-with-runtime-grade.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ReplayTraceAdapter {
    domain: String,
    coverage_label: String,
    trace: Vec<ObservedInfluence>,
}

impl ReplayTraceAdapter {
    /// Adapter over a recorded trace. `domain` and `coverage_label` are the
    /// declared domain and honesty label; empty ones are contract violations
    /// caught by [`DomainAdapter::validate`].
    #[must_use]
    pub fn new(
        domain: impl Into<String>,
        coverage_label: impl Into<String>,
        trace: Vec<ObservedInfluence>,
    ) -> Self {
        Self {
            domain: domain.into(),
            coverage_label: coverage_label.into(),
            trace,
        }
    }

    /// The recorded trace, in order.
    #[must_use]
    pub fn trace(&self) -> &[ObservedInfluence] {
        &self.trace
    }
}

impl DomainAdapter for ReplayTraceAdapter {
    fn domain(&self) -> &str {
        &self.domain
    }

    fn coverage_label(&self) -> &str {
        &self.coverage_label
    }

    fn max_truth_class(&self) -> TruthClass {
        // The reference adapter is a trace replay: RuntimeObserved at best,
        // never declared/static.
        TruthClass::RuntimeObserved
    }

    fn influences(&self) -> Vec<ObservedInfluence> {
        self.trace.clone()
    }
}
