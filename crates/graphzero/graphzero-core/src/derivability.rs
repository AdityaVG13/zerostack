//! Derivability over declared roots and producer edges.
//! Returns derivable, disproved under complete coverage, or unknown.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::grades::GradeName;
use crate::graph::CoverageClass;
use crate::invalidation::{ArtifactId, DependencyGraph};
use crate::refinement::{EdgeProvenance, RefinementLoop};
use crate::truth::TruthClass;

/// Why the predicate cannot answer.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UnknownReason {
    /// X is not a node of the dependency graph at all.
    ArtifactUnknown,
    /// No root -> X path exists, but the graph is known to be incomplete: it
    /// carries `runtime_observed_edges` refinement-added edges (runtime
    /// provenance only), so absence cannot be asserted even as a current-graph claim.
    RuntimeObservedRegion { runtime_observed_edges: usize },
}

/// Honest three-valued answer to a derivability query.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
pub enum DerivabilityAnswer {
    /// A declared root reaches X. `path` is the proof (root ->... -> X);
    /// `grade` labels the claim (weakest edge on the path); `truth` is the
    /// weakest edge truth class.
    Derivable {
        root: ArtifactId,
        path: Vec<ArtifactId>,
        grade: GradeName,
        truth: TruthClass,
    },
    /// No root -> X path in the current graph. `certified` is true only under
    /// declared Complete coverage of the edge set; otherwise this is a
    /// current-graph statement, never a claim about the true world.
    NotDerivable {
        certified: bool,
        coverage: CoverageClass,
    },
    /// The predicate cannot honestly answer Derivable or NotDerivable.
    Unknown { reason: UnknownReason },
}

impl DerivabilityAnswer {
    /// Whether the answer asserts derivability.
    #[must_use]
    pub const fn is_derivable(&self) -> bool {
        matches!(self, Self::Derivable { .. })
    }

    /// The claim grade of a derivable answer, when it is derivable.
    #[must_use]
    pub const fn grade(&self) -> Option<GradeName> {
        match self {
            Self::Derivable { grade, .. } => Some(*grade),
            _ => None,
        }
    }
}

/// Standalone derivability predicate over a dependency graph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DerivabilityPredicate {
    pub graph: DependencyGraph,
    /// Declared root inputs: derivation starts only from these.
    pub roots: BTreeSet<ArtifactId>,
    /// Edges known to be refinement-added (`RuntimeObserved`). Any such edge
    /// makes the edge set known-incomplete.
    pub runtime_edges: BTreeSet<(ArtifactId, ArtifactId)>,
    /// Declared completeness of the edge set for the derivability question.
    pub coverage: CoverageClass,
}

impl DerivabilityPredicate {
    /// Predicate over `graph` with declared root inputs and a declared coverage of the
    /// edge set. No edges are initially known runtime-observed use
    /// [`Self::mark_runtime_observed`] or [`Self::from_refinement_loop`] to record refinement provenance.
    #[must_use]
    pub fn new(
        graph: DependencyGraph,
        roots: BTreeSet<ArtifactId>,
        coverage: CoverageClass,
    ) -> Self {
        Self {
            graph,
            roots,
            runtime_edges: BTreeSet::new(),
            coverage,
        }
    }

    /// Record that the edge `input -> output` is refinement-added
    /// (`RuntimeObserved` provenance). This both grades paths through the edge
    /// and marks the edge set known-incomplete.
    pub fn mark_runtime_observed(&mut self, input: ArtifactId, output: ArtifactId) {
        self.runtime_edges.insert((input, output));
    }

    /// Predicate over a refined graph: snapshot of `loop_`'s graph plus every
    /// edge the loop labeled `RuntimeObserved`. The loop's public
    /// `provenance_of` is the only access used -- the loop's internal label map stays private.
    #[must_use]
    pub fn from_refinement_loop(
        loop_: &RefinementLoop,
        roots: BTreeSet<ArtifactId>,
        coverage: CoverageClass,
    ) -> Self {
        let mut runtime_edges = BTreeSet::new();
        for (input, targets) in &loop_.graph.forward {
            for output in targets {
                if loop_.provenance_of(*input, *output) == Some(EdgeProvenance::RuntimeObserved) {
                    runtime_edges.insert((*input, *output));
                }
            }
        }
        Self {
            graph: loop_.graph.clone(),
            roots,
            runtime_edges,
            coverage,
        }
    }

    /// Ask: is `x` derivable from the declared roots? BFS from every declared root (deterministic
    /// order), capturing a parent map so the answer carries a root -> X path as proof.
    #[must_use]
    pub fn derivability(&self, x: ArtifactId) -> DerivabilityAnswer {
        if !self.graph.forward.contains_key(&x) {
            return DerivabilityAnswer::Unknown {
                reason: UnknownReason::ArtifactUnknown,
            };
        }
        // BFS from all declared roots; parent map reconstructs the proof path.
        let mut parent: BTreeMap<ArtifactId, ArtifactId> = BTreeMap::new();
        let mut queue: VecDeque<ArtifactId> = VecDeque::new();
        let mut reached: BTreeSet<ArtifactId> = BTreeSet::new();
        for root in &self.roots {
            if reached.insert(*root) {
                queue.push_back(*root);
            }
        }
        while let Some(u) = queue.pop_front() {
            if u == x {
                break;
            }
            if let Some(targets) = self.graph.forward.get(&u) {
                for v in targets {
                    if reached.insert(*v) {
                        parent.insert(*v, u);
                        queue.push_back(*v);
                    }
                }
            }
        }
        if reached.contains(&x) {
            let mut path = vec![x];
            let mut current = x;
            while let Some(p) = parent.get(&current) {
                path.push(*p);
                current = *p;
            }
            path.reverse();
            // Weakest edge on the path labels the claim: any runtime-observed
            // edge => ObservedOnly (runtime grade), never a static grade.
            let any_runtime = path
                .windows(2)
                .any(|w| self.runtime_edges.contains(&(w[0], w[1])));
            let (grade, truth) = if any_runtime {
                (GradeName::ObservedOnly, TruthClass::RuntimeObserved)
            } else {
                (
                    GradeName::SoundOverapproximation,
                    TruthClass::SoundOverapproximation,
                )
            };
            return DerivabilityAnswer::Derivable {
                root: path[0],
                path,
                grade,
                truth,
            };
        }
        // No path. Fail closed on known-incomplete edge sets: a
        // runtime-observed region means the declared edge set was once found
        // incomplete, so "no path" cannot be asserted even as a current-graph absence claim.
        if !self.runtime_edges.is_empty() {
            return DerivabilityAnswer::Unknown {
                reason: UnknownReason::RuntimeObservedRegion {
                    runtime_observed_edges: self.runtime_edges.len(),
                },
            };
        }
        DerivabilityAnswer::NotDerivable {
            certified: self.coverage.permits_absence_certificate(),
            coverage: self.coverage,
        }
    }
}
