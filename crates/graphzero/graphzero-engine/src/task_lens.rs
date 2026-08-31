//! Computes evidence and write closures for a candidate plan.
//! Returns assurance, source roots, tests, interfaces, and unresolved ambiguities.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use graphzero_core::atlas::{AddressAtlas, AtlasError, SnapLevel, TaskFingerprint};
use graphzero_core::decision::{
    ClosureClass, DecisionClosure, DecisionEvidence, DecisionGap, EvidenceKind,
};
use graphzero_core::graph::NodeId;
use graphzero_core::truth::TruthClass;
use graphzero_store::Snapshot;
use graphzero_types::ContentHash;
use serde::{Deserialize, Serialize};

use crate::blast::{
    BlastError, CoveringTest, PlannedEdit, SpeculativeBlastRequest, impact_before_edit,
};
use crate::rewrite_closure::{EditSite, PropagationPolicy, Relation, rewrite_closure};

pub const TASK_LENS_SCHEMA_VERSION: u32 = 1;
const TASK_LENS_BLAST_BUDGET: usize = 1024;

/// Minimum truth grade a candidate must meet to remain in scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GradeBar {
    /// No grade restriction; every candidate stays (identity-like filter).
    Any,
    /// Only `CompilerExact` / `LspExactScoped` candidates stay.
    Exact,
    /// Only candidates admissible in a strict sound fiber stay
    /// (`CompilerExact` / `LspExactScoped` / `SyntaxDerived` /
    /// `SoundOverapproximation`).
    StrictFiber,
}

impl GradeBar {
    #[must_use]
    pub fn admits(self, truth: TruthClass) -> bool {
        match self {
            GradeBar::Any => true,
            GradeBar::Exact => truth.is_exact(),
            GradeBar::StrictFiber => truth.admissible_in_strict_fiber(),
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Exact => "exact",
            Self::StrictFiber => "strict_fiber",
        }
    }
}

/// Engine-side task contract, vocabulary-aligned with the hub
/// `StructuredTaskContract` (task kind, acceptance criteria, protected
/// scope). The fingerprint drives the address-atlas scope lens.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskContract {
    pub task_kind: String,
    pub acceptance_criteria: Vec<String>,
    /// Protected-scope symbols the task must not disturb (hub vocabulary:
    /// `ProtectedScopeObligations`).
    pub protected_scope: Vec<String>,
    pub fingerprint: TaskFingerprint,
    /// Grade bar applied by the composed entry's grade filter.
    pub min_grade: GradeBar,
    /// Reverse-graph depth for the dependency-closure lens (1 = direct sites).
    pub dependency_depth: u32,
}

impl TaskContract {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        task_kind: impl Into<String>,
        acceptance_criteria: Vec<String>,
        protected_scope: Vec<String>,
        fingerprint: TaskFingerprint,
        min_grade: GradeBar,
        dependency_depth: u32,
    ) -> Self {
        Self {
            task_kind: task_kind.into(),
            acceptance_criteria,
            protected_scope,
            fingerprint,
            min_grade,
            dependency_depth,
        }
    }
}

/// Candidate plan: focus symbols plus planned edits (blast vocabulary).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidatePlan {
    pub focus_symbols: Vec<String>,
    pub planned_edits: Vec<PlannedEdit>,
}

/// One candidate in the working scope: a graph node, its symbol name, its
/// truth grade, and the stage that introduced it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeNode {
    pub node: NodeId,
    pub symbol: String,
    pub truth: TruthClass,
    pub source: String,
}

/// The working scope: an ordered, deduplicated candidate set plus an honest
/// provably-empty reason (when a stage proved emptiness).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LensScope {
    pub nodes: BTreeMap<NodeId, ScopeNode>,
    pub empty_reason: Option<String>,
}

impl LensScope {
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// Result of one lens stage: the narrowed scope plus a human-readable
/// attribution note and any ambiguity/coherence gaps the stage discovered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LensOutcome {
    pub scope: LensScope,
    pub note: String,
    pub gaps: Vec<DecisionGap>,
}

/// A single narrowing/expanding stage of the composed task lens.
pub trait TaskLens: Send + Sync {
    /// Stable stage name for receipt attribution.
    fn name(&self) -> &'static str;
    /// Apply the stage to the current scope.
    fn apply(&self, scope: LensScope, ctx: &LensContext<'_>) -> Result<LensOutcome, LensError>;
}

/// Read-only context handed to every stage.
pub struct LensContext<'a> {
    pub snapshot: &'a Snapshot,
    pub contract: &'a TaskContract,
    pub plan: &'a CandidatePlan,
}

/// One receipt line per stage: what the stage narrowed, and how.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LensReceiptEntry {
    pub lens: String,
    pub before: usize,
    pub after: usize,
    pub removed: Vec<String>,
    pub note: String,
}

/// Full receipt of a composed lens run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LensReceipt {
    pub stages: Vec<LensReceiptEntry>,
    pub final_count: usize,
    pub provably_empty: Option<String>,
}

/// Source roots: the final scope candidates with their grade attribution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRoot {
    pub symbol: String,
    pub truth: TruthClass,
    pub source: String,
}

#[derive(Debug)]
pub enum LensError {
    /// Source lens applied to a non-empty scope (composition incoherence).
    SourceLensNotFirst(&'static str),
    /// Empty lens composition has no source stage.
    EmptyComposition,
    Atlas(AtlasError),
    Blast(BlastError),
    Store(String),
}

impl std::fmt::Display for LensError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceLensNotFirst(name) => write!(
                f,
                "source lens {name} applied to a non-empty scope: source lenses must be first"
            ),
            Self::EmptyComposition => write!(f, "empty lens composition has no source stage"),
            Self::Atlas(e) => write!(f, "atlas: {e}"),
            Self::Blast(e) => write!(f, "blast: {e}"),
            Self::Store(s) => write!(f, "store: {s}"),
        }
    }
}

impl std::error::Error for LensError {}

impl From<BlastError> for LensError {
    fn from(e: BlastError) -> Self {
        Self::Blast(e)
    }
}

/// Deterministic node identity for a scope candidate: the content hash of its
/// symbol name (same construction the atlas fixtures use).
fn node_id_for(symbol: &str) -> NodeId {
    NodeId(ContentHash::of(symbol.as_bytes()))
}

// Stages

/// Source stage: resolve the task fingerprint through the address atlas into
/// the initial candidate scope. Must be the first stage (fail loud otherwise).
pub struct TaskScopeLens {
    atlas: AddressAtlas,
    /// node -> symbol inverse of the atlas index, needed to render source roots.
    symbols: BTreeMap<NodeId, String>,
}

impl TaskScopeLens {
    #[must_use]
    pub fn new(atlas: AddressAtlas, symbols: BTreeMap<NodeId, String>) -> Self {
        Self { atlas, symbols }
    }

    fn symbol_for(&self, node: NodeId) -> String {
        self.symbols
            .get(&node)
            .cloned()
            .unwrap_or_else(|| format!("<node:{:?}>", node.0))
    }
}

impl TaskLens for TaskScopeLens {
    fn name(&self) -> &'static str {
        "task_scope"
    }

    fn apply(&self, scope: LensScope, ctx: &LensContext<'_>) -> Result<LensOutcome, LensError> {
        if !scope.nodes.is_empty() {
            return Err(LensError::SourceLensNotFirst(self.name()));
        }
        let fp = &ctx.contract.fingerprint;
        if fp.tokens.is_empty() {
            return Err(LensError::Atlas(AtlasError::EmptyFingerprint));
        }
        let (level, ranks) = self.atlas.resolve(fp).map_err(LensError::Atlas)?;
        let mut out = LensScope::default();
        for rank in &ranks {
            out.nodes.insert(
                rank.node,
                ScopeNode {
                    node: rank.node,
                    symbol: self.symbol_for(rank.node),
                    truth: rank.truth,
                    source: self.name().to_string(),
                },
            );
        }
        let note = match level {
            SnapLevel::S0 => format!("S0: unique exact locus ({:?})", ranks[0].node.0),
            SnapLevel::S1 => format!("S1: {} candidate(s), premises required", ranks.len()),
            SnapLevel::S2 => format!(
                "S2: broad candidate set ({}), premises required",
                ranks.len()
            ),
            SnapLevel::Unknown => "Unknown: no calibrated locus".to_string(),
        };
        let mut gaps = Vec::new();
        if out.nodes.is_empty() {
            out.empty_reason = Some(format!(
                "atlas has no calibrated locus for fingerprint tokens {:?} (SnapLevel::Unknown)",
                fp.tokens
            ));
            gaps.push(DecisionGap {
                kind: EvidenceKind::UnresolvedGap,
                reason: out.empty_reason.clone().expect("set above"),
            });
        } else if level != SnapLevel::S0 {
            gaps.push(DecisionGap {
                kind: EvidenceKind::UnresolvedGap,
                reason: format!("locus ambiguity: {note}"),
            });
        }
        Ok(LensOutcome {
            scope: out,
            note,
            gaps,
        })
    }
}

/// Lawful unit of composition: applying it changes nothing.
pub struct IdentityLens;

impl TaskLens for IdentityLens {
    fn name(&self) -> &'static str {
        "identity"
    }

    fn apply(&self, scope: LensScope, _ctx: &LensContext<'_>) -> Result<LensOutcome, LensError> {
        Ok(LensOutcome {
            scope,
            note: "identity lens: lawful no-op, scope unchanged".to_string(),
            gaps: Vec::new(),
        })
    }
}

/// Intersection stage: keep only candidates on the explicit allowlist. When
/// the intersection is provably empty (disjoint scopes), the reason names
/// both sets instead of silently returning empty.
pub struct ScopeFilterLens {
    pub allow: BTreeSet<String>,
}

impl ScopeFilterLens {
    #[must_use]
    pub fn new(allow: impl IntoIterator<Item = String>) -> Self {
        Self {
            allow: allow.into_iter().collect(),
        }
    }
}

impl TaskLens for ScopeFilterLens {
    fn name(&self) -> &'static str {
        "scope_filter"
    }

    fn apply(&self, scope: LensScope, _ctx: &LensContext<'_>) -> Result<LensOutcome, LensError> {
        if scope.nodes.is_empty() {
            return Ok(LensOutcome {
                scope,
                note: "empty scope: filter is a lawful no-op".to_string(),
                gaps: Vec::new(),
            });
        }
        let mut out = scope.clone();
        let removed: Vec<String> = scope
            .nodes
            .values()
            .filter(|n| !self.allow.contains(&n.symbol))
            .map(|n| n.symbol.clone())
            .collect();
        for n in scope.nodes.values() {
            if !self.allow.contains(&n.symbol) {
                out.nodes.remove(&n.node);
            }
        }
        let mut gaps = Vec::new();
        let note = if out.nodes.is_empty() {
            let symbols: Vec<String> = scope.nodes.values().map(|n| n.symbol.clone()).collect();
            let allowed: Vec<String> = self.allow.iter().cloned().collect();
            out.empty_reason = Some(format!(
                "disjoint scopes: candidate scope {{{}}} intersect allowlist {{{}}} is provably empty",
                symbols.join(", "),
                allowed.join(", ")
            ));
            gaps.push(DecisionGap {
                kind: EvidenceKind::UnresolvedGap,
                reason: out.empty_reason.clone().expect("set above"),
            });
            format!(
                "narrowed {} -> 0: no candidate in allowlist",
                scope.nodes.len()
            )
        } else {
            format!(
                "narrowed {} -> {} (dropped {})",
                scope.nodes.len(),
                out.nodes.len(),
                removed.len()
            )
        };
        Ok(LensOutcome {
            scope: out,
            note,
            gaps,
        })
    }
}

/// Expansion stage: dependency closure of the scope over the indexed reverse graph, reusing
/// `rewrite_closure` (same relation walk, same HIT grammar).
pub struct DependencyClosureLens {
    pub max_depth: u32,
    /// symbol -> truth for symbols the address atlas knows (grade attribution).
    pub known_truth: BTreeMap<String, TruthClass>,
}

impl DependencyClosureLens {
    #[must_use]
    pub fn new(max_depth: u32, known_truth: BTreeMap<String, TruthClass>) -> Self {
        Self {
            max_depth,
            known_truth,
        }
    }
}

impl TaskLens for DependencyClosureLens {
    fn name(&self) -> &'static str {
        "dependency_closure"
    }

    fn apply(&self, scope: LensScope, ctx: &LensContext<'_>) -> Result<LensOutcome, LensError> {
        if scope.nodes.is_empty() {
            return Ok(LensOutcome {
                scope,
                note: "empty scope: closure is a lawful no-op".to_string(),
                gaps: Vec::new(),
            });
        }
        let policy = PropagationPolicy {
            relations: vec![Relation::Calls, Relation::Refs, Relation::Imports],
            max_depth: self.max_depth.max(1),
        };
        let mut out = scope.clone();
        let mut added_unknown = 0usize;
        let mut added_known = 0usize;
        let mut gaps = Vec::new();
        for node in scope.nodes.values() {
            match rewrite_closure(ctx.snapshot, &node.symbol, &policy) {
                Ok(closure) => {
                    for site in &closure.sites {
                        let id = node_id_for(&site.symbol);
                        if out.nodes.contains_key(&id) {
                            continue;
                        }
                        let truth = self
                            .known_truth
                            .get(&site.symbol)
                            .copied()
                            .unwrap_or(TruthClass::Unknown);
                        if truth == TruthClass::Unknown {
                            added_unknown += 1;
                        } else {
                            added_known += 1;
                        }
                        out.nodes.insert(
                            id,
                            ScopeNode {
                                node: id,
                                symbol: site.symbol.clone(),
                                truth,
                                source: self.name().to_string(),
                            },
                        );
                    }
                }
                Err(e) => gaps.push(DecisionGap {
                    kind: EvidenceKind::UnresolvedGap,
                    reason: format!("dependency closure for {}: {e}", node.symbol),
                }),
            }
        }
        let note = format!(
            "expanded {} -> {} (depth <= {}; +{added_known} known-truth, +{added_unknown} unknown-truth)",
            scope.nodes.len(),
            out.nodes.len(),
            self.max_depth,
        );
        Ok(LensOutcome {
            scope: out,
            note,
            gaps,
        })
    }
}

/// Grade stage: drop candidates that fail the grade bar. A provably-empty
/// outcome names the bar and the truth classes seen, never a silent empty.
pub struct GradeFilterLens {
    pub bar: GradeBar,
}

impl GradeFilterLens {
    #[must_use]
    pub const fn new(bar: GradeBar) -> Self {
        Self { bar }
    }
}

impl TaskLens for GradeFilterLens {
    fn name(&self) -> &'static str {
        "grade_filter"
    }

    fn apply(&self, scope: LensScope, _ctx: &LensContext<'_>) -> Result<LensOutcome, LensError> {
        if scope.nodes.is_empty() {
            return Ok(LensOutcome {
                scope,
                note: "empty scope: grade filter is a lawful no-op".to_string(),
                gaps: Vec::new(),
            });
        }
        let mut out = LensScope {
            empty_reason: None,
            nodes: BTreeMap::new(),
        };
        let mut removed = Vec::new();
        for (id, node) in &scope.nodes {
            if self.bar.admits(node.truth) {
                out.nodes.insert(*id, node.clone());
            } else {
                removed.push(format!("{}:{}", node.symbol, node.truth.as_str()));
            }
        }
        let mut gaps = Vec::new();
        let note = if out.nodes.is_empty() {
            out.empty_reason = Some(format!(
                "grade bar {} admits none of the {} candidate(s) (truth classes: {})",
                self.bar.as_str(),
                scope.nodes.len(),
                removed.join(", ")
            ));
            gaps.push(DecisionGap {
                kind: EvidenceKind::UnresolvedGap,
                reason: out.empty_reason.clone().expect("set above"),
            });
            format!(
                "narrowed {} -> 0: grade bar {} admits no candidate",
                scope.nodes.len(),
                self.bar.as_str()
            )
        } else {
            format!(
                "narrowed {} -> {} by grade bar {} (dropped {})",
                scope.nodes.len(),
                out.nodes.len(),
                self.bar.as_str(),
                removed.len()
            )
        };
        Ok(LensOutcome {
            scope: out,
            note,
            gaps,
        })
    }
}

// Composed entry point

/// Explicit, ordered lens composition. Stages apply left to right; each
/// contribution lands in the receipt.
pub struct ComposedTaskLens {
    lenses: Vec<Arc<dyn TaskLens>>,
}

impl std::fmt::Debug for ComposedTaskLens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.lenses.iter().map(|l| l.name()).collect();
        write!(f, "ComposedTaskLens({names:?})")
    }
}

impl ComposedTaskLens {
    /// Build a composition. Rejects the empty composition (no source stage)
    /// loudly.
    pub fn new(lenses: Vec<Arc<dyn TaskLens>>) -> Result<Self, LensError> {
        if lenses.is_empty() {
            return Err(LensError::EmptyComposition);
        }
        Ok(Self { lenses })
    }
}

/// The composed entry point's full answer, one tuple per RACC:
/// demanded evidence closure, write closure, assurance grade, source roots,
/// tests, interfaces, unresolved ambiguities -- plus the lens receipt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskLensReport {
    pub schema_version: u32,
    pub contract_task_kind: String,
    pub grade_bar: GradeBar,
    pub receipt: LensReceipt,
    pub demanded_evidence_closure: DecisionClosure,
    pub assurance_grade: ClosureClass,
    pub write_closure: Vec<EditSite>,
    pub write_closure_roots: Vec<String>,
    pub write_closure_unresolved_sites: usize,
    pub source_roots: Vec<SourceRoot>,
    pub tests: Vec<CoveringTest>,
    pub interfaces: Vec<String>,
    pub unresolved_ambiguities: Vec<DecisionGap>,
    /// Honest provably-empty reason, when a stage proved the scope empty.
    pub provably_empty: Option<String>,
}

impl ComposedTaskLens {
    /// Run the composition: `(task contract, candidate plan)` -> full report.
    pub fn run(
        &self,
        snapshot: &Snapshot,
        contract: &TaskContract,
        plan: &CandidatePlan,
    ) -> Result<TaskLensReport, LensError> {
        let ctx = LensContext {
            snapshot,
            contract,
            plan,
        };
        let mut scope = LensScope::empty();
        let mut stages = Vec::new();
        let mut gaps: Vec<DecisionGap> = Vec::new();
        for lens in &self.lenses {
            let before: BTreeMap<NodeId, String> = scope
                .nodes
                .iter()
                .map(|(id, n)| (*id, n.symbol.clone()))
                .collect();
            let outcome = lens.apply(scope, &ctx)?;
            let removed: Vec<String> = before
                .iter()
                .filter(|(id, _)| !outcome.scope.nodes.contains_key(id))
                .map(|(_, symbol)| symbol.clone())
                .collect();
            stages.push(LensReceiptEntry {
                lens: lens.name().to_string(),
                before: before.len(),
                after: outcome.scope.nodes.len(),
                removed,
                note: outcome.note.clone(),
            });
            gaps.extend(outcome.gaps);
            scope = outcome.scope;
        }

        let provably_empty = scope.empty_reason.clone();

        // Demanded evidence closure: one Definition per scope candidate.
        let mut evidence = Vec::new();
        let mut source_roots = Vec::new();
        for node in scope.nodes.values() {
            evidence.push(DecisionEvidence {
                kind: EvidenceKind::Definition,
                node: Some(node.node),
                truth: node.truth,
                digest: node.node.0,
            });
            source_roots.push(SourceRoot {
                symbol: node.symbol.clone(),
                truth: node.truth,
                source: node.source.clone(),
            });
        }
        if scope.nodes.is_empty() && provably_empty.is_none() {
            gaps.push(DecisionGap {
                kind: EvidenceKind::UnresolvedGap,
                reason: "no candidates in scope".to_string(),
            });
        }
        let task_digest = ContentHash::of(format!("task:{}", contract.task_kind).as_bytes());
        let demanded_evidence_closure = DecisionClosure::assemble(task_digest, evidence, gaps);
        let assurance_grade = demanded_evidence_closure.class;

        // Write closure: reuse rewrite_closure per scope candidate.
        let policy = PropagationPolicy {
            relations: vec![Relation::Calls, Relation::Refs, Relation::Imports],
            max_depth: contract.dependency_depth.max(1),
        };
        let mut write_closure = Vec::new();
        let mut write_closure_roots = Vec::new();
        let mut write_closure_unresolved_sites = 0usize;
        let mut seen_targets = BTreeSet::new();
        for node in scope.nodes.values() {
            if node.symbol.is_empty() {
                continue;
            }
            if let Ok(closure) = rewrite_closure(snapshot, &node.symbol, &policy) {
                write_closure_unresolved_sites += closure.unresolved_sites;
                write_closure_roots.push(node.symbol.clone());
                for site in closure.sites {
                    if seen_targets.insert(site.target.clone()) {
                        write_closure.push(site);
                    }
                }
            }
        }

        // Tests + interfaces: reuse the blast impact query on the plan (or the
        // scope candidates when the plan names no focus symbols).
        let focus_symbols = if plan.focus_symbols.is_empty() {
            scope.nodes.values().map(|n| n.symbol.clone()).collect()
        } else {
            plan.focus_symbols.clone()
        };
        let request = SpeculativeBlastRequest {
            world_ref: format!("task-lens://{}", contract.task_kind),
            focus_symbols,
            planned_edits: plan.planned_edits.clone(),
            world_envelope: None,
        };
        let (tests, interfaces) =
            match impact_before_edit(snapshot, request, TASK_LENS_BLAST_BUDGET) {
                Ok(report) => (report.impacted_tests, report.impacted_symbols),
                Err(_e) => (Vec::new(), Vec::new()),
            };

        let receipt = LensReceipt {
            stages,
            final_count: scope.nodes.len(),
            provably_empty: provably_empty.clone(),
        };
        Ok(TaskLensReport {
            schema_version: TASK_LENS_SCHEMA_VERSION,
            contract_task_kind: contract.task_kind.clone(),
            grade_bar: contract.min_grade,
            receipt,
            unresolved_ambiguities: demanded_evidence_closure.gaps.clone(),
            demanded_evidence_closure,
            assurance_grade,
            write_closure,
            write_closure_roots,
            write_closure_unresolved_sites,
            source_roots,
            tests,
            interfaces,
            provably_empty,
        })
    }
}
