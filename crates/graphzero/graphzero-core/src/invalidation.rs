//! Certified incremental invalidation. Sound overapproximation may invalidate too much; it
//! must never invalidate too little for protected claims. Incremental recompute of the
//! upward dependency closure must agree with a full rebuild within the declared influence class.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use graphzero_types::ContentHash;

/// Identity of a derived or source artifact in the influence graph.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ArtifactId(pub ContentHash);

/// How precisely edges capture influence. ExactSupport is minimal; sound
/// overapprox may include extra edges (over-invalidate) but never drop needed ones.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum InfluenceClass {
    ExactSupport,
    SoundOverapproximation,
    Heuristic,
}

impl InfluenceClass {
    /// Whether agreement with full rebuild is required for protected claims.
    #[must_use]
    pub const fn protects_incremental_equivalence(self) -> bool {
        matches!(self, Self::ExactSupport | Self::SoundOverapproximation)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExactSupport => "exact_support",
            Self::SoundOverapproximation => "sound_overapproximation",
            Self::Heuristic => "heuristic",
        }
    }
}

/// Declared kind of a dependency edge. Optional metadata on top of the bare content-hash
/// edge: the kind does not change closure computation a kinded edge invalidates exactly like an
/// unkinded one -- it records the declared influence channel for consumers (witness/verifier layering).
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum DependencyKind {
    /// Declared build-system dependency (manifest/build inputs).
    Build,
    /// Declared schema/codegen dependency (generated artifacts, schema inputs).
    Schema,
    /// Declared effect channel (env, clocks, network, randomness policy).
    Effect,
    /// Declared data dependency (payload/content inputs).
    Data,
    /// Dependency only observable at runtime, not statically declared.
    RuntimeObserved,
}

/// Directed influence graph: input artifact -> derived artifacts that depend on it.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DependencyGraph {
    pub class: InfluenceClass,
    pub forward: BTreeMap<ArtifactId, BTreeSet<ArtifactId>>,
    /// Declared kinds for kinded edges, keyed (input, output). Absent entries
    /// are unkinded edges (legacy `add_dependency` callers).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub kinds: BTreeMap<(ArtifactId, ArtifactId), DependencyKind>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InvalidationError {
    Cycle(Vec<ArtifactId>),
    /// Influence class does not authorize a protected equivalence claim.
    UnprotectedInfluence(InfluenceClass),
    MissingProducer(ArtifactId),
    /// Incremental and full rebuild disagree for a protected producer.
    EquivalenceDivergence(ArtifactId),
}

impl std::fmt::Display for InvalidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cycle(nodes) => write!(f, "dependency cycle involving {} nodes", nodes.len()),
            Self::UnprotectedInfluence(c) => {
                write!(f, "influence class {} is not protected", c.as_str())
            }
            Self::MissingProducer(id) => {
                write!(f, "missing producer for {}", id.0.to_hex())
            }
            Self::EquivalenceDivergence(id) => {
                write!(f, "incremental/full divergence for {}", id.0.to_hex())
            }
        }
    }
}

impl std::error::Error for InvalidationError {}

/// Record of file/symbol/artifact closure consulted for a derived artifact.
/// TokenZero/CacheZero intersect journal deltas with this closure.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DependencyClosureRecord {
    pub artifact: ArtifactId,
    pub consulted: BTreeSet<ArtifactId>,
    pub influence: InfluenceClass,
    pub producer_digest: ContentHash,
}

/// Certificate that an invalidation set is a sound overapprox of the true dirty set.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InvalidationCertificate {
    pub influence: InfluenceClass,
    pub changed: BTreeSet<ArtifactId>,
    /// Upward closure -- may be a strict superset of the minimal dirty set.
    pub invalidated: BTreeSet<ArtifactId>,
    /// True when `invalidated` is exactly the upward closure of `changed`.
    pub is_upward_closure: bool,
}

impl DependencyGraph {
    #[must_use]
    pub fn new(class: InfluenceClass) -> Self {
        Self {
            class,
            forward: BTreeMap::new(),
            kinds: BTreeMap::new(),
        }
    }

    /// Record that `output` depends on `input` (edge input -> output).
    pub fn add_dependency(&mut self, input: ArtifactId, output: ArtifactId) {
        self.forward.entry(input).or_default().insert(output);
        self.forward.entry(output).or_default();
    }

    /// Record a dependency with a declared kind. Closure behavior
    /// is identical to [`Self::add_dependency`]; the kind only annotates the
    /// declared influence channel.
    pub fn add_dependency_kinded(
        &mut self,
        input: ArtifactId,
        output: ArtifactId,
        kind: DependencyKind,
    ) {
        self.add_dependency(input, output);
        self.kinds.insert((input, output), kind);
    }

    /// Declared kind for the edge `input -> output`, when one was declared.
    #[must_use]
    pub fn declared_kind(&self, input: ArtifactId, output: ArtifactId) -> Option<DependencyKind> {
        self.kinds.get(&(input, output)).copied()
    }

    /// Ensure an isolated node exists (source or sink with no edges yet).
    pub fn ensure_node(&mut self, id: ArtifactId) {
        self.forward.entry(id).or_default();
    }

    /// Sound overapprox dirty set: all artifacts reachable from `changed` via
    /// forward dependency edges, including `changed` itself.
    #[must_use]
    pub fn upward_closure(&self, changed: &BTreeSet<ArtifactId>) -> BTreeSet<ArtifactId> {
        let mut out = changed.clone();
        let mut q: VecDeque<_> = changed.iter().copied().collect();
        while let Some(x) = q.pop_front() {
            if let Some(next) = self.forward.get(&x) {
                for y in next {
                    if out.insert(*y) {
                        q.push_back(*y);
                    }
                }
            }
        }
        out
    }

    /// Certified invalidation: upward closure under the declared influence class.
    pub fn certify_invalidation(&self, changed: &BTreeSet<ArtifactId>) -> InvalidationCertificate {
        let invalidated = self.upward_closure(changed);
        InvalidationCertificate {
            influence: self.class,
            changed: changed.clone(),
            invalidated,
            is_upward_closure: true,
        }
    }

    /// Kahn topological order; errors on cycles.
    pub fn topological_order(&self) -> Result<Vec<ArtifactId>, InvalidationError> {
        let mut indegree: BTreeMap<ArtifactId, usize> =
            self.forward.keys().copied().map(|k| (k, 0)).collect();
        for targets in self.forward.values() {
            for target in targets {
                *indegree.entry(*target).or_default() += 1;
            }
        }
        let mut q: VecDeque<_> = indegree
            .iter()
            .filter_map(|(k, v)| (*v == 0).then_some(*k))
            .collect();
        let mut out = Vec::with_capacity(indegree.len());
        while let Some(x) = q.pop_front() {
            out.push(x);
            if let Some(targets) = self.forward.get(&x) {
                for y in targets {
                    if let Some(v) = indegree.get_mut(y) {
                        *v -= 1;
                        if *v == 0 {
                            q.push_back(*y);
                        }
                    }
                }
            }
        }
        if out.len() != indegree.len() {
            let cyclic = indegree
                .into_iter()
                .filter_map(|(k, v)| (v > 0).then_some(k))
                .collect();
            return Err(InvalidationError::Cycle(cyclic));
        }
        Ok(out)
    }
}

/// Producer functions map artifact id -> recompute from current state bytes.
pub type ProducerFn = Box<dyn Fn(&BTreeMap<ArtifactId, Vec<u8>>) -> Option<Vec<u8>>>;

/// Engine comparing full vs incremental recompute for protected influence classes.
pub struct RecomputeEngine {
    pub graph: DependencyGraph,
    pub producers: BTreeMap<ArtifactId, ProducerFn>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecomputeResult {
    pub state: BTreeMap<ArtifactId, Vec<u8>>,
    pub recomputed: BTreeSet<ArtifactId>,
}

/// Honest instrumentation for the equality-boundary early cutoff.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CutoffReport {
    /// Producers actually re-executed in this pass (producer returned a value).
    pub recomputed: BTreeSet<ArtifactId>,
    /// Dirty producers skipped because an upstream boundary artifact
    /// recomputed equal to its previous value -- the measured savings of
    /// this pass (these would have been recomputed unconditionally before the cutoff).
    pub cut_off: BTreeSet<ArtifactId>,
    /// Boundary artifacts: recomputed to a value byte-equal to the previous
    /// value; downstream propagation stopped at each of these.
    pub boundary_nodes: BTreeSet<ArtifactId>,
}

impl RecomputeEngine {
    #[must_use]
    pub fn new(graph: DependencyGraph) -> Self {
        Self {
            graph,
            producers: BTreeMap::new(),
        }
    }

    pub fn register_producer<F>(&mut self, id: ArtifactId, f: F)
    where
        F: Fn(&BTreeMap<ArtifactId, Vec<u8>>) -> Option<Vec<u8>> + 'static,
    {
        self.graph.ensure_node(id);
        self.producers.insert(id, Box::new(f));
    }

    /// Full rebuild: recompute every producer to a fixed point. Uses data availability (producer
    /// returns Some) rather than only influence edges, so a missing influence edge still yields a
    /// correct full rebuild and can be compared against an under-invalidating incremental pass.
    pub fn full_recompute(
        &self,
        inputs: &BTreeMap<ArtifactId, Vec<u8>>,
    ) -> Result<RecomputeResult, InvalidationError> {
        let _ = self.graph.topological_order()?; // fail closed on cycles
        let mut state = inputs.clone();
        let mut recomputed = BTreeSet::new();
        let mut pending: BTreeSet<ArtifactId> = self.producers.keys().copied().collect();
        // Use influence topological order first, then retry unresolved artifacts.
        let order = self.graph.topological_order().unwrap_or_default();
        let mut schedule: Vec<ArtifactId> = order
            .into_iter()
            .filter(|id| pending.contains(id))
            .collect();
        for id in self.producers.keys() {
            if !schedule.contains(id) {
                schedule.push(*id);
            }
        }
        let mut guard = 0usize;
        while !pending.is_empty() {
            guard += 1;
            if guard > self.producers.len() + 2 {
                let stuck = pending.iter().next().copied().unwrap();
                return Err(InvalidationError::MissingProducer(stuck));
            }
            let mut progressed = false;
            for id in &schedule {
                if !pending.contains(id) {
                    continue;
                }
                let prod = self
                    .producers
                    .get(id)
                    .ok_or(InvalidationError::MissingProducer(*id))?;
                if let Some(value) = prod(&state) {
                    state.insert(*id, value);
                    recomputed.insert(*id);
                    pending.remove(id);
                    progressed = true;
                }
            }
            if !progressed {
                let stuck = pending.iter().next().copied().unwrap();
                return Err(InvalidationError::MissingProducer(stuck));
            }
        }
        Ok(RecomputeResult { state, recomputed })
    }

    /// Incremental: update changed inputs, recompute only the upward producer closure.
    pub fn incremental_recompute(
        &self,
        old_state: &BTreeMap<ArtifactId, Vec<u8>>,
        changed_inputs: &BTreeMap<ArtifactId, Vec<u8>>,
    ) -> Result<RecomputeResult, InvalidationError> {
        Ok(self
            .incremental_recompute_with_report(old_state, changed_inputs)?
            .0)
    }

    /// Incremental recompute with equality-boundary early cutoff, returning a [`CutoffReport`] so
    /// savings are measured, not claimed. Algorithm: process the (unfiltered) upward invalidation
    /// closure in graph topological order.
    pub fn incremental_recompute_with_report(
        &self,
        old_state: &BTreeMap<ArtifactId, Vec<u8>>,
        changed_inputs: &BTreeMap<ArtifactId, Vec<u8>>,
    ) -> Result<(RecomputeResult, CutoffReport), InvalidationError> {
        let changed: BTreeSet<_> = changed_inputs.keys().copied().collect();
        let cert = self.graph.certify_invalidation(&changed);
        // Unfiltered upward closure: non-producer nodes inside it still carry
        // propagation (their dependents must recompute), they just never
        // recompute themselves.
        let dirty = cert.invalidated;
        let mut state = old_state.clone();
        for (k, v) in changed_inputs {
            state.insert(*k, v.clone());
        }
        // Recompute reached producers in graph topological order so dependents
        // always observe updated prerequisites within the same invalidation pass.
        let order = self.graph.topological_order()?;
        let mut preds: BTreeMap<ArtifactId, BTreeSet<ArtifactId>> = BTreeMap::new();
        for (u, targets) in &self.graph.forward {
            for v in targets {
                preds.entry(*v).or_default().insert(*u);
            }
        }
        let mut report = CutoffReport::default();
        let mut boundary: BTreeSet<ArtifactId> = BTreeSet::new();
        let mut tainted: BTreeSet<ArtifactId> = BTreeSet::new();
        let mut reached_ids: BTreeSet<ArtifactId> = BTreeSet::new();
        for id in order {
            if !dirty.contains(&id) {
                continue;
            }
            // A node is reached iff it is a changed input itself or at least one predecessor propagates:
            // predecessor is not a boundary, and either it is a non-producer that is dirty (its value may have
            // changed or it merely carries overapprox invalidation) or it is a producer that was itself.
            let reached = if changed.contains(&id) {
                true
            } else {
                preds.get(&id).is_some_and(|ps| {
                    ps.iter().any(|p| {
                        !boundary.contains(p)
                            && (if self.producers.contains_key(p) {
                                reached_ids.contains(p)
                            } else {
                                dirty.contains(p)
                            })
                    })
                })
            };
            if !reached {
                // Upstream equality boundary stopped propagation: this is the
                // measured saving of the pass.
                if self.producers.contains_key(&id) {
                    report.cut_off.insert(id);
                }
                continue;
            }
            // Taint flows through EVERY dirty node -- producer or not. A
            // non-producer node between a failed producer and its dependents
            // must still carry the taint downstream (fail-closed).
            let inputs_tainted = preds
                .get(&id)
                .is_some_and(|ps| ps.iter().any(|p| tainted.contains(p)));
            if inputs_tainted {
                tainted.insert(id);
            }
            if !self.producers.contains_key(&id) {
                continue; // plain input node: carries propagation, never recomputed
            }
            reached_ids.insert(id);
            let Some(prod) = self.producers.get(&id) else {
                // Producer registry inconsistency; fail closed: propagate.
                continue;
            };
            if let Some(value) = prod(&state) {
                state.insert(id, value.clone());
                report.recomputed.insert(id);
                // Content-hash equality: byte-equal to the previous value is the only trigger for a boundary. A
                // missing previous value (None) can never establish equality -> propagates.
                if !inputs_tainted && old_state.get(&id) == Some(&value) {
                    boundary.insert(id);
                    report.boundary_nodes.insert(id);
                }
            } else {
                // Producer cannot run: leave stale value and propagate
                // (fail-closed). Anything downstream recomputes from this
                // stale value, so equality below this point is untrustworthy.
                tainted.insert(id);
            }
        }
        Ok((
            RecomputeResult {
                state,
                recomputed: report.recomputed.clone(),
            },
            report,
        ))
    }

    /// Protected claim: incremental state equals full rebuild for all producer outputs.
    pub fn assert_incremental_equivalence(
        &self,
        baseline_inputs: &BTreeMap<ArtifactId, Vec<u8>>,
        changed_inputs: &BTreeMap<ArtifactId, Vec<u8>>,
    ) -> Result<(), InvalidationError> {
        if !self.graph.class.protects_incremental_equivalence() {
            return Err(InvalidationError::UnprotectedInfluence(self.graph.class));
        }
        let full_inputs = {
            let mut m = baseline_inputs.clone();
            for (k, v) in changed_inputs {
                m.insert(*k, v.clone());
            }
            m
        };
        let full = self.full_recompute(&full_inputs)?;
        let base = self.full_recompute(baseline_inputs)?;
        let incr = self.incremental_recompute(&base.state, changed_inputs)?;
        for id in self.producers.keys() {
            let a = full.state.get(id);
            let b = incr.state.get(id);
            if a != b {
                // Under-invalidation: incremental missed a producer that full rebuilt.
                // Fail closed -- never silently accept divergence on protected class.
                return Err(InvalidationError::EquivalenceDivergence(*id));
            }
        }
        // Soundness of overapprox: every producer whose output actually changed
        // must appear in the incremental recomputed set (no under-invalidation).
        for id in self.producers.keys() {
            let before = base.state.get(id);
            let after = full.state.get(id);
            if before != after && !incr.recomputed.contains(id) {
                return Err(InvalidationError::EquivalenceDivergence(*id));
            }
        }
        Ok(())
    }
}

/// Build a dependency-closure record for a derived artifact.
#[must_use]
pub fn record_dependency_closure(
    artifact: ArtifactId,
    consulted: BTreeSet<ArtifactId>,
    influence: InfluenceClass,
    producer_digest: ContentHash,
) -> DependencyClosureRecord {
    DependencyClosureRecord {
        artifact,
        consulted,
        influence,
        producer_digest,
    }
}

/// Intersect journal-changed paths with a recorded closure -> dirty derived set.
#[must_use]
pub fn dirty_from_closure(
    closures: &[DependencyClosureRecord],
    journal_changed: &BTreeSet<ArtifactId>,
) -> BTreeSet<ArtifactId> {
    let mut dirty = BTreeSet::new();
    for rec in closures {
        if rec.consulted.iter().any(|c| journal_changed.contains(c)) {
            dirty.insert(rec.artifact);
        }
    }
    dirty
}
