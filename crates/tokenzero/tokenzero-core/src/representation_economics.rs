//! Representation resource vectors and exact model-facing decision surfaces.
//!
//! Selection preserves semantic and tokenizer identity. Pareto pruning,
//! segmentation, overfetch accounting, and rendering never claim provider
//! routing or semantic authority.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use zero_abi::work_capsule::SemanticInterrupt;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepresentationKind {
    RawBytes,
    ChunkManifest,
    CompressedBytes,
    SyntaxTree,
    SemanticIr,
    ClaimBundle,
    ProofBundle,
    Delta,
    ModelRendering,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RepresentationResources {
    pub stored_bytes: u64,
    pub wire_bytes: u64,
    pub source_tokens: u64,
    pub visible_tokens: u64,
    pub expansion_work: u64,
    pub verification_work: u64,
    pub latency_micros: u64,
    pub metadata_bytes: u64,
}

impl RepresentationResources {
    pub fn dominates_or_equal(&self, other: &Self) -> bool {
        self.stored_bytes <= other.stored_bytes
            && self.wire_bytes <= other.wire_bytes
            && self.source_tokens <= other.source_tokens
            && self.visible_tokens <= other.visible_tokens
            && self.expansion_work <= other.expansion_work
            && self.verification_work <= other.verification_work
            && self.latency_micros <= other.latency_micros
            && self.metadata_bytes <= other.metadata_bytes
    }

    pub fn strictly_better_than(&self, other: &Self) -> bool {
        self.dominates_or_equal(other) && *self != *other
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RepresentationRecord {
    pub representation_root: String,
    pub semantic_root: String,
    pub adapter_root: String,
    pub kind: RepresentationKind,
    pub exact: bool,
    pub resources: RepresentationResources,
}

pub fn pareto_frontier(records: &[RepresentationRecord]) -> Vec<RepresentationRecord> {
    let mut frontier: Vec<_> = records
        .iter()
        .filter(|candidate| {
            !records.iter().any(|other| {
                other.representation_root != candidate.representation_root
                    && other.semantic_root == candidate.semantic_root
                    && other.adapter_root == candidate.adapter_root
                    && (!candidate.exact || other.exact)
                    && other.resources.strictly_better_than(&candidate.resources)
            })
        })
        .cloned()
        .collect();
    frontier.sort_by(|left, right| left.representation_root.cmp(&right.representation_root));
    frontier
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SegmentCandidate {
    pub start: u32,
    pub end: u32,
    pub additive_cost: u64,
    pub boundary_kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SegmentationPlan {
    pub total_cost: u64,
    pub segments: Vec<SegmentCandidate>,
}

pub fn optimal_segmentation(
    length: u32,
    candidates: &[SegmentCandidate],
) -> Result<SegmentationPlan, String> {
    if length == 0 {
        return Err("segmentation length must be positive".into());
    }
    let n = length as usize;
    let mut best: Vec<Option<(u64, Option<usize>, Option<usize>)>> = vec![None; n + 1];
    best[0] = Some((0, None, None));
    for position in 0..n {
        let Some((cost, _, _)) = best[position] else {
            continue;
        };
        for (index, candidate) in candidates.iter().enumerate() {
            if candidate.start as usize != position
                || candidate.end <= candidate.start
                || candidate.end > length
            {
                continue;
            }
            let next = candidate.end as usize;
            let total = cost
                .checked_add(candidate.additive_cost)
                .ok_or("segmentation cost overflow")?;
            if best[next].is_none_or(|(current, _, _)| total < current) {
                best[next] = Some((total, Some(position), Some(index)));
            }
        }
    }
    let Some((total_cost, _, _)) = best[n] else {
        return Err("candidate boundaries do not cover the source".into());
    };
    let mut position = n;
    let mut segments = Vec::new();
    while position > 0 {
        let (_, previous, candidate) = best[position].ok_or("segmentation predecessor missing")?;
        let previous = previous.ok_or("segmentation predecessor missing")?;
        let candidate = candidate.ok_or("segmentation candidate missing")?;
        segments.push(candidates[candidate].clone());
        position = previous;
    }
    segments.reverse();
    Ok(SegmentationPlan {
        total_cost,
        segments,
    })
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TokenizerCost {
    pub tokenizer_root: String,
    pub tokens: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TokenizerWeight {
    pub tokenizer_root: String,
    pub weight_ppm: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MultiTokenizerSegmentCandidate {
    pub segment: SegmentCandidate,
    pub tokenizer_costs: Vec<TokenizerCost>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MultiTokenizerSegmentationPlan {
    pub total_cost: u64,
    pub segments: Vec<MultiTokenizerSegmentCandidate>,
}

pub fn optimal_multi_tokenizer_segmentation(
    length: u32,
    candidates: &[MultiTokenizerSegmentCandidate],
    weights: &[TokenizerWeight],
) -> Result<MultiTokenizerSegmentationPlan, String> {
    if weights.is_empty() {
        return Err("multi-tokenizer segmentation requires at least one tokenizer".into());
    }
    let mut weight_by_root = BTreeMap::new();
    for weight in weights {
        if weight.tokenizer_root.is_empty()
            || weight.weight_ppm == 0
            || weight_by_root
                .insert(weight.tokenizer_root.as_str(), weight.weight_ppm)
                .is_some()
        {
            return Err("tokenizer weights require unique roots and positive weights".into());
        }
    }
    let mut effective_costs = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let costs: BTreeMap<_, _> = candidate
            .tokenizer_costs
            .iter()
            .map(|cost| (cost.tokenizer_root.as_str(), cost.tokens))
            .collect();
        if costs.len() != candidate.tokenizer_costs.len()
            || costs.keys().copied().collect::<BTreeSet<_>>()
                != weight_by_root.keys().copied().collect::<BTreeSet<_>>()
        {
            return Err("every segment requires exactly one cost per tokenizer".into());
        }
        let weighted = weight_by_root
            .iter()
            .try_fold(0_u128, |total, (root, weight)| {
                let tokens = costs
                    .get(root)
                    .copied()
                    .ok_or("validated tokenizer cost disappeared")?;
                total
                    .checked_add(
                        u128::from(tokens)
                            .saturating_mul(u128::from(*weight))
                            .div_ceil(1_000_000),
                    )
                    .ok_or("multi-tokenizer cost overflow")
            })?;
        let weighted =
            u64::try_from(weighted).map_err(|_| "multi-tokenizer cost does not fit u64")?;
        effective_costs.push(
            candidate
                .segment
                .additive_cost
                .checked_add(weighted)
                .ok_or("multi-tokenizer cost overflow")?,
        );
    }
    if length == 0 {
        return Err("segmentation length must be positive".into());
    }
    let n = length as usize;
    let mut best: Vec<Option<(u64, Option<usize>, Option<usize>)>> = vec![None; n + 1];
    best[0] = Some((0, None, None));
    for position in 0..n {
        let Some((cost, _, _)) = best[position] else {
            continue;
        };
        for (index, candidate) in candidates.iter().enumerate() {
            let segment = &candidate.segment;
            if segment.start as usize != position
                || segment.end <= segment.start
                || segment.end > length
            {
                continue;
            }
            let next = segment.end as usize;
            let total = cost
                .checked_add(effective_costs[index])
                .ok_or("segmentation cost overflow")?;
            if best[next].is_none_or(|(current, _, _)| total < current) {
                best[next] = Some((total, Some(position), Some(index)));
            }
        }
    }
    let Some((total_cost, _, _)) = best[n] else {
        return Err("candidate boundaries do not cover the source".into());
    };
    let mut position = n;
    let mut segments = Vec::new();
    while position > 0 {
        let (_, previous, candidate) = best[position].ok_or("segmentation predecessor missing")?;
        let previous = previous.ok_or("segmentation predecessor missing")?;
        let candidate = candidate.ok_or("segmentation candidate missing")?;
        segments.push(candidates[candidate].clone());
        position = previous;
    }
    segments.reverse();
    Ok(MultiTokenizerSegmentationPlan {
        total_cost,
        segments,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OverfetchAccounting {
    pub evidence_bytes: u64,
    pub returned_bytes: u64,
    pub sufficient_bytes: Option<u64>,
}

impl OverfetchAccounting {
    pub fn overfetch_bytes(&self) -> Option<u64> {
        self.sufficient_bytes
            .map(|sufficient| self.returned_bytes.saturating_sub(sufficient))
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.returned_bytes > self.evidence_bytes {
            return Err("returned evidence exceeds available evidence".into());
        }
        if self
            .sufficient_bytes
            .is_some_and(|sufficient| sufficient > self.returned_bytes)
        {
            return Err("sufficient evidence exceeds returned evidence".into());
        }
        Ok(())
    }
}

pub fn cache_asset_has_positive_value(
    demand_rate: u64,
    saved_work: u64,
    storage_cost_rate: u64,
    invalidation_hazard: u64,
    maintenance_cost: u64,
) -> bool {
    u128::from(demand_rate) * u128::from(saved_work)
        > u128::from(storage_cost_rate)
            + u128::from(invalidation_hazard) * u128::from(maintenance_cost)
}

pub fn render_semantic_interrupt(interrupt: &SemanticInterrupt) -> Result<String, String> {
    interrupt.validate()?;
    serde_json::to_string(interrupt).map_err(|error| error.to_string())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CapsuleDecisionSurface {
    pub capsule_root: String,
    pub representation_frontier: Vec<RepresentationRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupt: Option<SemanticInterrupt>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RenderedDecisionSurface {
    pub root: String,
    pub json: String,
}

pub fn render_capsule_decision_surface(
    capsule_root: &str,
    candidates: &[RepresentationRecord],
    interrupt: Option<&SemanticInterrupt>,
) -> Result<RenderedDecisionSurface, String> {
    if capsule_root.is_empty() || candidates.is_empty() {
        return Err(
            "decision surface requires a capsule root and representation candidates".into(),
        );
    }
    if let Some(interrupt) = interrupt {
        interrupt.validate()?;
        if interrupt.capsule_root != capsule_root {
            return Err("decision surface interrupt belongs to another capsule".into());
        }
    }
    let surface = CapsuleDecisionSurface {
        capsule_root: capsule_root.into(),
        representation_frontier: pareto_frontier(candidates),
        interrupt: interrupt.cloned(),
    };
    let value = serde_json::to_value(&surface).map_err(|error| error.to_string())?;
    let json = zero_abi::canonical_json(&value);
    Ok(RenderedDecisionSurface {
        root: zero_abi::sha256_hex(json.as_bytes()),
        json,
    })
}
