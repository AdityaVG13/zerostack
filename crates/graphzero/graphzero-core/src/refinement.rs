//! Counterexample-guided dependency refinement.

use std::collections::{BTreeMap, BTreeSet};

use crate::invalidation::{ArtifactId, DependencyGraph, InvalidationCertificate};
use crate::omission::{OmissionImpact, OmissionKind, RecoveryTrigger};
use crate::truth::TruthClass;

/// An undeclared influence observed at runtime: `target` was observed to
/// depend on `source` although no declared edge recorded that influence.
#[derive(
    Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ObservedInfluence {
    pub source: ArtifactId,
    pub target: ArtifactId,
    /// Trace evidence (e.g. sandbox call record) that established the influence.
    pub evidence: String,
}

impl ObservedInfluence {
    /// Capture an influence from an omission impact. Only a `MissingDependencyEdge`
    /// omission that forces automatic recovery (impacts actions or invalidation) is a
    /// candidate for refinement. Advisory omissions return `None` -- they never mint graph edges.
    #[must_use]
    pub fn from_omission_impact(
        impact: &OmissionImpact,
        source: ArtifactId,
        target: ArtifactId,
    ) -> Option<Self> {
        if impact.kind == OmissionKind::MissingDependencyEdge
            && impact.trigger == RecoveryTrigger::ForceAutomaticRecovery
        {
            Some(Self {
                source,
                target,
                evidence: format!("omission:{}", impact.id.to_hex()),
            })
        } else {
            None
        }
    }
}

/// Provenance label of a dependency edge with respect to the refinement loop.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum EdgeProvenance {
    /// Edge present in the graph the loop was initialized with (static analysis).
    Declared,
    /// Edge added by this loop from an observed runtime influence.
    /// Maps to `TruthClass::RuntimeObserved`; never upgraded to `Declared`.
    RuntimeObserved,
}

impl EdgeProvenance {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::RuntimeObserved => "runtime_observed",
        }
    }

    /// Truth-class label of this provenance (truth.rs vocabulary).
    #[must_use]
    pub const fn truth_class(self) -> TruthClass {
        match self {
            Self::Declared => TruthClass::SoundOverapproximation,
            Self::RuntimeObserved => TruthClass::RuntimeObserved,
        }
    }
}

/// Append-only ledger of every counterexample that forced an edge add.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RetainedCounterexample {
    /// Monotonic sequence number (append order).
    pub sequence: u64,
    /// The observed influence that forced the edge add.
    pub influence: ObservedInfluence,
    /// The edge added to the dependency graph (source -> target).
    pub edge_added: (ArtifactId, ArtifactId),
    /// Provenance label of the added edge. Always `RuntimeObserved`; the loop
    /// never upgrades an observed edge to declared.
    pub edge_label: TruthClass,
    /// Certificates revoked: the certified upward closure of the edge source.
    pub invalidation: InvalidationCertificate,
}

/// Store of retained counterexamples plus the set of edges already
/// applied. Append-only for records; `applied` mirrors the graph edges
/// this loop added so duplicate observations are recognized without a graph re-scan.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RetainedCounterexampleStore {
    records: Vec<RetainedCounterexample>,
    applied: BTreeSet<(ArtifactId, ArtifactId)>,
    next_sequence: u64,
}

impl RetainedCounterexampleStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    #[must_use]
    pub fn records(&self) -> &[RetainedCounterexample] {
        &self.records
    }

    /// Edges already added by the loop (idempotence fast path / audit surface).
    #[must_use]
    pub fn applied_edges(&self) -> &BTreeSet<(ArtifactId, ArtifactId)> {
        &self.applied
    }

    #[must_use]
    pub fn contains(&self, input: ArtifactId, output: ArtifactId) -> bool {
        self.applied.contains(&(input, output))
    }

    /// Append a counterexample record (only called when an edge was added).
    fn push(
        &mut self,
        influence: ObservedInfluence,
        edge_added: (ArtifactId, ArtifactId),
        invalidation: InvalidationCertificate,
    ) -> RetainedCounterexample {
        let record = RetainedCounterexample {
            sequence: self.next_sequence,
            influence,
            edge_added,
            edge_label: TruthClass::RuntimeObserved,
            invalidation,
        };
        self.next_sequence += 1;
        self.applied.insert(edge_added);
        self.records.push(record.clone());
        record
    }
}

/// Outcome of ingesting one observed influence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefinementOutcome {
    /// A new true edge was added and its dependent cone revoked.
    EdgeAdded { retained: RetainedCounterexample },
    /// The influence is already represented by an edge in the graph.
    /// Idempotent no-op: no edge add, no revocation, no retained record.
    AlreadyRepresented {
        source: ArtifactId,
        target: ArtifactId,
    },
}

/// Report of a fair-exercise fixed-point refinement pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FixedPointReport {
    /// Number of sweeps that added at least one edge. Under fair exercise of
    /// K missing true edges this is <= K (each adding sweep consumes at least
    /// one new edge).
    pub iterations: usize,
    /// Total edges added by the loop so far (cumulative across passes).
    pub edges_added: usize,
    /// True when a final full sweep added nothing -- fixed point reached.
    pub fixed_point: bool,
}

/// Counterexample-guided refinement loop over a [`DependencyGraph`].
#[derive(Clone, Debug)]
pub struct RefinementLoop {
    /// The graph being refined. Edges added here are labeled
    /// `RuntimeObserved`; pre-existing edges keep their declared status.
    pub graph: DependencyGraph,
    store: RetainedCounterexampleStore,
    edge_labels: BTreeMap<(ArtifactId, ArtifactId), TruthClass>,
}

impl RefinementLoop {
    #[must_use]
    pub fn new(graph: DependencyGraph) -> Self {
        Self {
            graph,
            store: RetainedCounterexampleStore::new(),
            edge_labels: BTreeMap::new(),
        }
    }

    /// Ingest one observed influence: ADD -> REVOKE -> RETAIN, idempotently. When the influence's edge
    /// is already present in the graph -- whether added by a prior counterexample or declared upfront
    /// -- the observation is a no-op: no second edge, no second revocation, no second retained record.
    pub fn observe(&mut self, influence: ObservedInfluence) -> RefinementOutcome {
        let already = self
            .graph
            .forward
            .get(&influence.source)
            .map_or(false, |targets| targets.contains(&influence.target));
        if already {
            return RefinementOutcome::AlreadyRepresented {
                source: influence.source,
                target: influence.target,
            };
        }
        let source = influence.source;
        let target = influence.target;
        // ADD: true dependency edge, labeled RuntimeObserved, never upgraded.
        self.graph.add_dependency(source, target);
        self.edge_labels
            .insert((source, target), TruthClass::RuntimeObserved);
        // REVOKE: certificates in the upward closure of the new edge's source.
        // The edge is added first so the closure includes the newly discovered
        // dependents (the whole point of the counterexample).
        let changed: BTreeSet<ArtifactId> = [source].into_iter().collect();
        let certificate = self.graph.certify_invalidation(&changed);
        // RETAIN: append-only counterexample record.
        let retained = self.store.push(influence, (source, target), certificate);
        RefinementOutcome::EdgeAdded { retained }
    }

    /// Sweep `trace` repeatedly until a full pass adds no edge (fixed point).
    pub fn refine_to_fixed_point(&mut self, trace: &[ObservedInfluence]) -> FixedPointReport {
        let mut iterations = 0usize;
        loop {
            let before = self.store.len();
            for obs in trace {
                let _ = self.observe(obs.clone());
            }
            let added = self.store.len() - before;
            if added == 0 {
                return FixedPointReport {
                    iterations,
                    edges_added: self.store.len(),
                    fixed_point: true,
                };
            }
            iterations += 1;
        }
    }

    /// Provenance label of the edge `input -> output`, when it exists. Edges added by this loop
    /// are `RuntimeObserved`; edges present before refinement are `Declared`. The label is never
    /// upgraded: an observed edge stays `RuntimeObserved` even across duplicate observations.
    #[must_use]
    pub fn provenance_of(&self, input: ArtifactId, output: ArtifactId) -> Option<EdgeProvenance> {
        if !self
            .graph
            .forward
            .get(&input)
            .map_or(false, |targets| targets.contains(&output))
        {
            return None;
        }
        Some(match self.edge_labels.get(&(input, output)) {
            Some(label) if *label == TruthClass::RuntimeObserved => EdgeProvenance::RuntimeObserved,
            _ => EdgeProvenance::Declared,
        })
    }

    /// Access the retained-counterexample store.
    #[must_use]
    pub fn retained(&self) -> &RetainedCounterexampleStore {
        &self.store
    }

    /// Number of true edges added so far by this loop.
    #[must_use]
    pub fn edges_added(&self) -> usize {
        self.store.len()
    }
}
