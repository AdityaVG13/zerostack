//! Cognitive-work graph contracts over GraphZero's existing truth lattice. Compiler-resolved
//! edges carry toolchain provenance. Reverse impact, temporal overlays, proof support, and
//! mechanical classification preserve incomplete and unknown states instead of upgrading them.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::atlas::LocusRank;
use crate::{ClosureClass, DecisionGap, FiberClass, TruthClass};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CognitiveNodeClass {
    SemanticDecision,
    Mechanical,
    RetryRepair,
    Verification,
    ModelCall,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompilerEdgeKind {
    Import,
    Export,
    Call,
    Extends,
    Implements,
    TypeReference,
    ConstructorDependency,
    Composition,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CompilerSemanticEdge {
    pub from: String,
    pub to: String,
    pub kind: CompilerEdgeKind,
    pub language: String,
    pub compiler_root: String,
    pub configuration_root: String,
    pub freshness_root: String,
    pub source_path: String,
    pub source_line: u32,
    pub source_column: u32,
}

impl CompilerSemanticEdge {
    pub fn validate(&self) -> Result<(), String> {
        if self.from.is_empty()
            || self.to.is_empty()
            || self.language.is_empty()
            || self.compiler_root.is_empty()
            || self.configuration_root.is_empty()
            || self.freshness_root.is_empty()
            || self.source_path.is_empty()
            || self.source_line == 0
            || self.source_column == 0
        {
            return Err("compiler semantic edge is missing identity or source provenance".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SemanticExtractionReport {
    pub language: String,
    pub compiler_root: String,
    pub configuration_root: String,
    pub freshness_root: String,
    pub indexed_files: u64,
    pub resolved_edges: u64,
    pub unresolved_sites: Vec<String>,
    pub fatal_diagnostics: Vec<String>,
}

impl SemanticExtractionReport {
    pub fn closure_class(&self) -> ClosureClass {
        if !self.fatal_diagnostics.is_empty() {
            ClosureClass::Incomplete
        } else if self.unresolved_sites.is_empty() {
            ClosureClass::Exact
        } else {
            ClosureClass::SoundOverapproximation
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CompilerImpact {
    pub changed: Vec<String>,
    pub impacted: Vec<String>,
    pub closure: ClosureClass,
    pub compiler_root: String,
    pub configuration_root: String,
    pub freshness_root: String,
}

pub fn compiler_reverse_impact(
    changed: impl IntoIterator<Item = String>,
    edges: &[CompilerSemanticEdge],
    report: &SemanticExtractionReport,
    bound: u32,
) -> Result<CompilerImpact, String> {
    if report.compiler_root.is_empty()
        || report.configuration_root.is_empty()
        || report.freshness_root.is_empty()
    {
        return Err(
            "semantic extraction report is missing compiler identity or freshness root".into(),
        );
    }
    if report.resolved_edges != edges.len() as u64 {
        return Err("semantic extraction report edge count does not match supplied edges".into());
    }
    let changed: BTreeSet<String> = changed.into_iter().collect();
    if changed.is_empty() || changed.iter().any(String::is_empty) {
        return Err("compiler impact requires nonempty changed symbols".into());
    }
    let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for edge in edges {
        edge.validate()?;
        if edge.compiler_root != report.compiler_root
            || edge.configuration_root != report.configuration_root
            || edge.language != report.language
        {
            return Err("compiler edge does not belong to the extraction report".into());
        }
        if edge.freshness_root != report.freshness_root {
            return Err("compiler edge freshness root does not match the extraction report".into());
        }
        dependents
            .entry(edge.to.as_str())
            .or_default()
            .push(edge.from.as_str());
    }
    let mut impacted = BTreeSet::new();
    let mut queue: VecDeque<String> = changed.iter().cloned().collect();
    let mut traversed: u64 = 0;
    let mut budget_exceeded = false;
    while let Some(symbol) = queue.pop_front() {
        if !impacted.insert(symbol.clone()) {
            continue;
        }
        if let Some(next) = dependents.get(symbol.as_str()) {
            for dependent in next {
                if traversed >= u64::from(bound) {
                    budget_exceeded = true;
                    break;
                }
                traversed += 1;
                queue.push_back((*dependent).to_owned());
            }
            if budget_exceeded {
                break;
            }
        }
    }
    let closure = if budget_exceeded || report.closure_class() == ClosureClass::Incomplete {
        ClosureClass::Incomplete
    } else {
        report.closure_class()
    };
    Ok(CompilerImpact {
        changed: changed.into_iter().collect(),
        impacted: impacted.into_iter().collect(),
        closure,
        compiler_root: report.compiler_root.clone(),
        configuration_root: report.configuration_root.clone(),
        freshness_root: report.freshness_root.clone(),
    })
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TemporalEdge {
    pub id: String,
    pub from: String,
    pub to: String,
    pub relation: String,
    pub provenance_root: String,
    pub valid_from_epoch: u64,
    pub valid_to_epoch: Option<u64>,
    pub supersedes: Option<String>,
}

impl TemporalEdge {
    pub fn live_at(&self, epoch: u64) -> bool {
        self.valid_from_epoch <= epoch && self.valid_to_epoch.is_none_or(|end| epoch < end)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeDelta {
    Upsert(TemporalEdge),
    Tombstone { edge_id: String, epoch: u64 },
}

pub fn fuse_edge_overlay(
    base: &[TemporalEdge],
    deltas: &[EdgeDelta],
) -> Result<Vec<TemporalEdge>, String> {
    let mut edges: std::collections::BTreeMap<String, TemporalEdge> = base
        .iter()
        .cloned()
        .map(|edge| (edge.id.clone(), edge))
        .collect();
    for delta in deltas {
        match delta {
            EdgeDelta::Upsert(edge) => {
                if edge.id.is_empty()
                    || edge.from.is_empty()
                    || edge.to.is_empty()
                    || edge.relation.is_empty()
                {
                    return Err("overlay edge is incomplete".into());
                }
                edges.insert(edge.id.clone(), edge.clone());
            }
            EdgeDelta::Tombstone { edge_id, epoch } => {
                let edge = edges
                    .get_mut(edge_id)
                    .ok_or("tombstone references an unknown edge")?;
                if *epoch < edge.valid_from_epoch {
                    return Err("tombstone precedes edge validity".into());
                }
                edge.valid_to_epoch = Some(*epoch);
            }
        }
    }
    Ok(edges.into_values().collect())
}

/// Finite obligation kinds a typed obligation may carry. Free-form kinds are
/// not part of the cognitive-work contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypedObligationKind {
    Decision,
    Execution,
    Verification,
    Restoration,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TypedObligation {
    pub id: String,
    pub kind: TypedObligationKind,
    pub protected_scope_root: String,
    pub required_evidence_kinds: Vec<String>,
}

impl TypedObligation {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() || self.protected_scope_root.is_empty() {
            return Err("typed obligation identity and protected scope are required".into());
        }
        if self.required_evidence_kinds.is_empty() {
            return Err("typed obligation requires at least one evidence kind".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProofSupportHyperedge {
    pub id: String,
    pub obligation_id: String,
    pub sources: Vec<String>,
    pub target: String,
    pub proof_root: String,
    pub verifier_contract_root: String,
    pub snapshot_root: String,
    pub provenance_root: String,
    pub valid_from_epoch: u64,
    pub valid_to_epoch: Option<u64>,
}

impl ProofSupportHyperedge {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty()
            || self.obligation_id.is_empty()
            || self.sources.is_empty()
            || self.sources.iter().any(String::is_empty)
            || self.target.is_empty()
            || self.proof_root.is_empty()
            || self.verifier_contract_root.is_empty()
            || self.snapshot_root.is_empty()
            || self.provenance_root.is_empty()
        {
            return Err(
                "proof-support hyperedge requires nonempty sources, target, and roots".into(),
            );
        }
        if self
            .valid_to_epoch
            .is_some_and(|end| end < self.valid_from_epoch)
        {
            return Err("proof-support validity interval is inverted".into());
        }
        Ok(())
    }

    /// Whether the support is live at `epoch` (its validity interval contains it).
    pub fn live_at(&self, epoch: u64) -> bool {
        self.valid_from_epoch <= epoch && self.valid_to_epoch.is_none_or(|end| epoch < end)
    }

    /// Whether the support is fresh: bound to `snapshot_root` and live at `epoch`.
    pub fn fresh_for(&self, snapshot_root: &str, epoch: u64) -> bool {
        self.snapshot_root == snapshot_root && self.live_at(epoch)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MechanicalGraphVerdict {
    Safe,
    Unsafe,
    Unknown,
}

/// Inputs to the trivalent mechanical-region classifier. `snapshot_root` is
/// the requested snapshot identity; a proof support only counts as fresh when
/// it binds to that root and is live at `epoch`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MechanicalRegionInput {
    pub truth: TruthClass,
    pub fiber: FiberClass,
    pub gaps: Vec<DecisionGap>,
    pub independently_verified: bool,
    pub loci: Vec<LocusRank>,
    pub impact_closure: ClosureClass,
    pub obligations: Vec<TypedObligation>,
    pub supports: Vec<ProofSupportHyperedge>,
    pub snapshot_root: String,
    pub epoch: u64,
}

/// Pure trivalent classifier, fail-closed `Unsafe` only for explicit semantic choice/conflict:
/// unresolved decision gaps. `Safe` only for exactly one rooted exact locus (with evidence
/// premises), exact/complete reverse impact, complete coverage and freshness, and every valid.
pub fn classify_mechanical_region(input: &MechanicalRegionInput) -> MechanicalGraphVerdict {
    if !input.gaps.is_empty() {
        return MechanicalGraphVerdict::Unsafe;
    }
    if input.snapshot_root.is_empty() || input.obligations.is_empty() {
        return MechanicalGraphVerdict::Unknown;
    }
    let all_obligations_discharged = input.obligations.iter().all(|obligation| {
        obligation.validate().is_ok()
            && input.supports.iter().any(|support| {
                support.obligation_id == obligation.id
                    && support.proof_root == obligation.protected_scope_root
                    && support.validate().is_ok()
                    && support.fresh_for(&input.snapshot_root, input.epoch)
            })
    });
    if !all_obligations_discharged {
        return MechanicalGraphVerdict::Unknown;
    }
    let unique_rooted_locus = input.loci.len() == 1
        && input.loci[0].truth.is_exact()
        && !input.loci[0].premises.is_empty();
    let complete_impact = input.impact_closure == ClosureClass::Exact;
    let complete_coverage = input.independently_verified
        && matches!(
            input.truth,
            TruthClass::CompilerExact | TruthClass::RuntimeObserved
        )
        && input.fiber.admissible_as_strict();
    if unique_rooted_locus && complete_impact && complete_coverage {
        MechanicalGraphVerdict::Safe
    } else {
        MechanicalGraphVerdict::Unknown
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InterruptProposal {
    pub obligation_id: String,
    pub unresolved_gaps: Vec<String>,
    pub nondominated_choices: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub continuation_ref: String,
}

impl InterruptProposal {
    pub fn required(&self) -> bool {
        !self.unresolved_gaps.is_empty() || self.nondominated_choices.len() != 1
    }
}
