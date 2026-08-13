//! Frozen finite Robust Snap contract.
//!
//! This module checks abstract S0/S1 witnesses only. It does not compile, execute,
//! rank away, or authorize effects. Operational correspondence remains a separate
//! release gate.

use std::{collections::BTreeSet, error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{DigestV1, canonical_json, sha256};

pub const ROBUST_SNAP_MODEL_VERSION: &str = "zerostack.robust_snap.finite.v1";
pub const ROBUST_SNAP_CONTRACT_VERSION: u16 = 1;
pub const ROBUST_SNAP_MAX_WORLDS: usize = 32;
pub const ROBUST_SNAP_MAX_EFFECTS: usize = 64;
pub const ROBUST_SNAP_MAX_LEAVES: usize = 64;
pub const ROBUST_SNAP_MAX_EVIDENCE_DEPTH: usize = 16;
pub const ROBUST_SNAP_MAX_ASSUMPTIONS: usize = 32;
pub const ROBUST_SNAP_MAX_ASSUMPTION_BYTES: usize = 512;
const CERTIFICATE_DOMAIN_V1: &[u8] = b"zerostack.robust_snap.certificate.v1\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectedEffectClassV1 {
    ReadOnly,
    ReversibleMutation,
    ApprovalRequiredMutation,
    Irreversible,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectedEffectV1 {
    pub effect_digest: DigestV1,
    pub effect_class: ProtectedEffectClassV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorldFiberDescriptor {
    pub model_version: String,
    pub assembly_manifest_digest: DigestV1,
    pub source_image_digest: DigestV1,
    pub task_fingerprint: DigestV1,
    pub assumptions: Vec<String>,
    pub worlds: Vec<DigestV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectedEffectSet {
    pub world_id: DigestV1,
    pub effects: Vec<ProtectedEffectV1>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapLevel {
    Unknown,
    S0,
    S1,
}

impl SnapLevel {
    /// UNKNOWN is never accepted as a theorem result.
    pub const fn is_verified(self) -> bool {
        matches!(self, Self::S0 | Self::S1)
    }

    /// Abstract certificates never grant operational execution.
    pub const fn permits_operational_execution(self) -> bool {
        false
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceObservationV1 {
    pub evidence_id: DigestV1,
    pub outcome_digest: DigestV1,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceLeafV1 {
    pub path: Vec<EvidenceObservationV1>,
    pub admitted_worlds: Vec<DigestV1>,
    pub selected_effect: ProtectedEffectV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceDecisionTree {
    pub evidence_schema_digest: DigestV1,
    pub leaves: Vec<EvidenceLeafV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RobustSnapCertificate {
    pub model_version: String,
    pub fiber: WorldFiberDescriptor,
    pub protected_effects: Vec<ProtectedEffectSet>,
    pub first_turn_selectable: Vec<ProtectedEffectV1>,
    pub expressible_and_verifiable: Vec<ProtectedEffectV1>,
    pub snap_level: SnapLevel,
    pub selected_effect: Option<ProtectedEffectV1>,
    pub evidence_tree: Option<EvidenceDecisionTree>,
    pub certificate_digest: DigestV1,
}

impl RobustSnapCertificate {
    #[allow(clippy::too_many_arguments)]
    pub fn create_s0(
        fiber: WorldFiberDescriptor,
        protected_effects: Vec<ProtectedEffectSet>,
        first_turn_selectable: Vec<ProtectedEffectV1>,
        expressible_and_verifiable: Vec<ProtectedEffectV1>,
        selected_effect: ProtectedEffectV1,
    ) -> Result<Self, RobustSnapErrorV1> {
        Self::create(
            fiber,
            protected_effects,
            first_turn_selectable,
            expressible_and_verifiable,
            SnapLevel::S0,
            Some(selected_effect),
            None,
        )
    }

    pub fn create_s1(
        fiber: WorldFiberDescriptor,
        protected_effects: Vec<ProtectedEffectSet>,
        expressible_and_verifiable: Vec<ProtectedEffectV1>,
        evidence_tree: EvidenceDecisionTree,
    ) -> Result<Self, RobustSnapErrorV1> {
        Self::create(
            fiber,
            protected_effects,
            Vec::new(),
            expressible_and_verifiable,
            SnapLevel::S1,
            None,
            Some(evidence_tree),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn create(
        fiber: WorldFiberDescriptor,
        protected_effects: Vec<ProtectedEffectSet>,
        first_turn_selectable: Vec<ProtectedEffectV1>,
        expressible_and_verifiable: Vec<ProtectedEffectV1>,
        snap_level: SnapLevel,
        selected_effect: Option<ProtectedEffectV1>,
        evidence_tree: Option<EvidenceDecisionTree>,
    ) -> Result<Self, RobustSnapErrorV1> {
        let mut certificate = Self {
            model_version: ROBUST_SNAP_MODEL_VERSION.into(),
            fiber,
            protected_effects,
            first_turn_selectable,
            expressible_and_verifiable,
            snap_level,
            selected_effect,
            evidence_tree,
            certificate_digest: DigestV1::ZERO,
        };
        certificate.validate_semantics()?;
        certificate.certificate_digest = certificate.compute_digest()?;
        Ok(certificate)
    }

    pub fn validate(&self) -> Result<(), RobustSnapErrorV1> {
        self.validate_semantics()?;
        let actual = self.compute_digest()?;
        if actual != self.certificate_digest {
            return Err(RobustSnapErrorV1::CertificateDigestMismatch {
                expected: self.certificate_digest,
                actual,
            });
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, RobustSnapErrorV1> {
        self.validate()?;
        let value = serde_json::to_value(self)
            .map_err(|error| RobustSnapErrorV1::Json(error.to_string()))?;
        Ok(canonical_json(&value).into_bytes())
    }

    pub fn compute_digest(&self) -> Result<DigestV1, RobustSnapErrorV1> {
        let mut body = serde_json::to_value(self)
            .map_err(|error| RobustSnapErrorV1::Json(error.to_string()))?;
        body.as_object_mut()
            .ok_or_else(|| RobustSnapErrorV1::Json("certificate must be an object".into()))?
            .remove("certificate_digest");
        let canonical = canonical_json(&body);
        let mut bound = Vec::with_capacity(CERTIFICATE_DOMAIN_V1.len() + canonical.len());
        bound.extend_from_slice(CERTIFICATE_DOMAIN_V1);
        bound.extend_from_slice(canonical.as_bytes());
        Ok(DigestV1::from_bytes(sha256(&bound)))
    }

    pub fn common_s0_effects(&self) -> Result<Vec<ProtectedEffectV1>, RobustSnapErrorV1> {
        let worlds = self.fiber.worlds.iter().copied().collect::<BTreeSet<_>>();
        let mut common = self.common_for_worlds(&worlds)?;
        common.retain(|effect| self.first_turn_selectable.binary_search(effect).is_ok());
        common.retain(|effect| {
            self.expressible_and_verifiable
                .binary_search(effect)
                .is_ok()
        });
        Ok(common.into_iter().collect())
    }

    fn validate_semantics(&self) -> Result<(), RobustSnapErrorV1> {
        if self.model_version != ROBUST_SNAP_MODEL_VERSION
            || self.fiber.model_version != ROBUST_SNAP_MODEL_VERSION
        {
            return Err(RobustSnapErrorV1::UnsupportedModelVersion);
        }
        validate_fiber(&self.fiber)?;
        validate_effect_vector("first_turn_selectable", &self.first_turn_selectable)?;
        validate_effect_vector(
            "expressible_and_verifiable",
            &self.expressible_and_verifiable,
        )?;
        validate_protected_sets(&self.fiber, &self.protected_effects)?;
        match self.snap_level {
            SnapLevel::Unknown => Err(RobustSnapErrorV1::UnknownCannotPass),
            SnapLevel::S0 => self.validate_s0(),
            SnapLevel::S1 => self.validate_s1(),
        }
    }

    fn validate_s0(&self) -> Result<(), RobustSnapErrorV1> {
        if self.evidence_tree.is_some() {
            return Err(RobustSnapErrorV1::EvidenceTreeForbiddenAtS0);
        }
        let selected = self
            .selected_effect
            .as_ref()
            .ok_or(RobustSnapErrorV1::SelectedEffectRequiredAtS0)?;
        let common = self.common_s0_effects()?;
        if common.is_empty() {
            return Err(RobustSnapErrorV1::EmptyCommonProtectedEffectSet);
        }
        if common.binary_search(selected).is_err() {
            return Err(RobustSnapErrorV1::SelectedEffectNotCommon);
        }
        Ok(())
    }

    fn validate_s1(&self) -> Result<(), RobustSnapErrorV1> {
        if self.selected_effect.is_some() {
            return Err(RobustSnapErrorV1::GlobalSelectionForbiddenAtS1);
        }
        let tree = self
            .evidence_tree
            .as_ref()
            .ok_or(RobustSnapErrorV1::EvidenceTreeRequiredAtS1)?;
        if tree.leaves.is_empty() {
            return Err(RobustSnapErrorV1::EmptyDecisionTree);
        }
        if tree.leaves.len() > ROBUST_SNAP_MAX_LEAVES {
            return Err(RobustSnapErrorV1::BoundExceeded("evidence_tree.leaves"));
        }
        require_strict_order("evidence_tree.leaves", &tree.leaves)?;
        let fiber_worlds = self.fiber.worlds.iter().copied().collect::<BTreeSet<_>>();
        let mut covered = BTreeSet::new();
        for leaf in &tree.leaves {
            if leaf.path.is_empty() {
                return Err(RobustSnapErrorV1::EmptyEvidencePath);
            }
            if leaf.path.len() > ROBUST_SNAP_MAX_EVIDENCE_DEPTH {
                return Err(RobustSnapErrorV1::BoundExceeded("evidence_leaf.path"));
            }
            if leaf.admitted_worlds.is_empty() {
                return Err(RobustSnapErrorV1::EmptyEvidenceLeaf);
            }
            require_strict_order("evidence_leaf.admitted_worlds", &leaf.admitted_worlds)?;
            let leaf_worlds = leaf
                .admitted_worlds
                .iter()
                .copied()
                .collect::<BTreeSet<_>>();
            if !leaf_worlds.is_subset(&fiber_worlds) {
                return Err(RobustSnapErrorV1::LeafWorldOutsideFiber);
            }
            for world in &leaf_worlds {
                if !covered.insert(*world) {
                    return Err(RobustSnapErrorV1::WorldAppearsInMultipleLeaves);
                }
            }
            let mut common = self.common_for_worlds(&leaf_worlds)?;
            common.retain(|effect| {
                self.expressible_and_verifiable
                    .binary_search(effect)
                    .is_ok()
            });
            if !common.contains(&leaf.selected_effect) {
                return Err(RobustSnapErrorV1::LeafEffectNotProtected);
            }
        }
        if covered != fiber_worlds {
            return Err(RobustSnapErrorV1::EvidenceTreeDropsWorld);
        }
        Ok(())
    }

    fn common_for_worlds(
        &self,
        worlds: &BTreeSet<DigestV1>,
    ) -> Result<BTreeSet<ProtectedEffectV1>, RobustSnapErrorV1> {
        let mut iter = worlds.iter();
        let first = iter.next().ok_or(RobustSnapErrorV1::EmptyWorldFiber)?;
        let mut common = self
            .effects_for(*first)?
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        for world in iter {
            let effects = self
                .effects_for(*world)?
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>();
            common = common.intersection(&effects).cloned().collect();
        }
        Ok(common)
    }

    fn effects_for(&self, world: DigestV1) -> Result<&[ProtectedEffectV1], RobustSnapErrorV1> {
        self.protected_effects
            .binary_search_by_key(&world, |set| set.world_id)
            .ok()
            .map(|index| self.protected_effects[index].effects.as_slice())
            .ok_or(RobustSnapErrorV1::MissingProtectedEffectSet)
    }
}

/// Heuristics may reorder the complete fiber but cannot narrow it.
pub fn validate_heuristic_world_order(
    fiber: &WorldFiberDescriptor,
    ranked_worlds: &[DigestV1],
) -> Result<(), RobustSnapErrorV1> {
    if ranked_worlds.len() != fiber.worlds.len() {
        return Err(RobustSnapErrorV1::HeuristicDroppedWorld);
    }
    let expected = fiber.worlds.iter().copied().collect::<BTreeSet<_>>();
    let actual = ranked_worlds.iter().copied().collect::<BTreeSet<_>>();
    if actual.len() != ranked_worlds.len() || actual != expected {
        return Err(RobustSnapErrorV1::HeuristicDroppedWorld);
    }
    Ok(())
}

fn validate_fiber(fiber: &WorldFiberDescriptor) -> Result<(), RobustSnapErrorV1> {
    if fiber.worlds.is_empty() {
        return Err(RobustSnapErrorV1::EmptyWorldFiber);
    }
    if fiber.worlds.len() > ROBUST_SNAP_MAX_WORLDS {
        return Err(RobustSnapErrorV1::BoundExceeded("fiber.worlds"));
    }
    require_strict_order("fiber.worlds", &fiber.worlds)?;
    if fiber.assumptions.is_empty() || fiber.assumptions.len() > ROBUST_SNAP_MAX_ASSUMPTIONS {
        return Err(RobustSnapErrorV1::BoundExceeded("fiber.assumptions"));
    }
    require_strict_order("fiber.assumptions", &fiber.assumptions)?;
    for assumption in &fiber.assumptions {
        if assumption.is_empty() || assumption.len() > ROBUST_SNAP_MAX_ASSUMPTION_BYTES {
            return Err(RobustSnapErrorV1::InvalidAssumption);
        }
    }
    Ok(())
}

fn validate_effect_vector(
    field: &'static str,
    effects: &[ProtectedEffectV1],
) -> Result<(), RobustSnapErrorV1> {
    if effects.len() > ROBUST_SNAP_MAX_EFFECTS {
        return Err(RobustSnapErrorV1::BoundExceeded(field));
    }
    require_strict_order(field, effects)
}

fn validate_protected_sets(
    fiber: &WorldFiberDescriptor,
    sets: &[ProtectedEffectSet],
) -> Result<(), RobustSnapErrorV1> {
    if sets.len() != fiber.worlds.len() {
        return Err(RobustSnapErrorV1::ProtectedWorldSetMismatch);
    }
    let set_worlds = sets.iter().map(|set| set.world_id).collect::<Vec<_>>();
    require_strict_order("protected_effects", &set_worlds)?;
    if set_worlds != fiber.worlds {
        return Err(RobustSnapErrorV1::ProtectedWorldSetMismatch);
    }
    for set in sets {
        validate_effect_vector("protected_effects.effects", &set.effects)?;
    }
    Ok(())
}

fn require_strict_order<T: Ord>(
    field: &'static str,
    values: &[T],
) -> Result<(), RobustSnapErrorV1> {
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        Err(RobustSnapErrorV1::NonCanonicalOrder(field))
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RobustSnapFailureCodeV1 {
    UnsupportedModelVersion,
    UnknownCannotPass,
    EmptyWorldFiber,
    BoundExceeded,
    NonCanonicalOrder,
    InvalidAssumption,
    ProtectedWorldSetMismatch,
    MissingProtectedEffectSet,
    EmptyCommonProtectedEffectSet,
    SelectedEffectRequiredAtS0,
    SelectedEffectNotCommon,
    EvidenceTreeForbiddenAtS0,
    EvidenceTreeRequiredAtS1,
    GlobalSelectionForbiddenAtS1,
    EmptyDecisionTree,
    EmptyEvidencePath,
    EmptyEvidenceLeaf,
    LeafWorldOutsideFiber,
    WorldAppearsInMultipleLeaves,
    EvidenceTreeDropsWorld,
    LeafEffectNotProtected,
    HeuristicDroppedWorld,
    CertificateDigestMismatch,
    Json,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RobustSnapErrorV1 {
    UnsupportedModelVersion,
    UnknownCannotPass,
    EmptyWorldFiber,
    BoundExceeded(&'static str),
    NonCanonicalOrder(&'static str),
    InvalidAssumption,
    ProtectedWorldSetMismatch,
    MissingProtectedEffectSet,
    EmptyCommonProtectedEffectSet,
    SelectedEffectRequiredAtS0,
    SelectedEffectNotCommon,
    EvidenceTreeForbiddenAtS0,
    EvidenceTreeRequiredAtS1,
    GlobalSelectionForbiddenAtS1,
    EmptyDecisionTree,
    EmptyEvidencePath,
    EmptyEvidenceLeaf,
    LeafWorldOutsideFiber,
    WorldAppearsInMultipleLeaves,
    EvidenceTreeDropsWorld,
    LeafEffectNotProtected,
    HeuristicDroppedWorld,
    CertificateDigestMismatch {
        expected: DigestV1,
        actual: DigestV1,
    },
    Json(String),
}

impl RobustSnapErrorV1 {
    pub const fn code(&self) -> RobustSnapFailureCodeV1 {
        match self {
            Self::UnsupportedModelVersion => RobustSnapFailureCodeV1::UnsupportedModelVersion,
            Self::UnknownCannotPass => RobustSnapFailureCodeV1::UnknownCannotPass,
            Self::EmptyWorldFiber => RobustSnapFailureCodeV1::EmptyWorldFiber,
            Self::BoundExceeded(_) => RobustSnapFailureCodeV1::BoundExceeded,
            Self::NonCanonicalOrder(_) => RobustSnapFailureCodeV1::NonCanonicalOrder,
            Self::InvalidAssumption => RobustSnapFailureCodeV1::InvalidAssumption,
            Self::ProtectedWorldSetMismatch => RobustSnapFailureCodeV1::ProtectedWorldSetMismatch,
            Self::MissingProtectedEffectSet => RobustSnapFailureCodeV1::MissingProtectedEffectSet,
            Self::EmptyCommonProtectedEffectSet => {
                RobustSnapFailureCodeV1::EmptyCommonProtectedEffectSet
            }
            Self::SelectedEffectRequiredAtS0 => RobustSnapFailureCodeV1::SelectedEffectRequiredAtS0,
            Self::SelectedEffectNotCommon => RobustSnapFailureCodeV1::SelectedEffectNotCommon,
            Self::EvidenceTreeForbiddenAtS0 => RobustSnapFailureCodeV1::EvidenceTreeForbiddenAtS0,
            Self::EvidenceTreeRequiredAtS1 => RobustSnapFailureCodeV1::EvidenceTreeRequiredAtS1,
            Self::GlobalSelectionForbiddenAtS1 => {
                RobustSnapFailureCodeV1::GlobalSelectionForbiddenAtS1
            }
            Self::EmptyDecisionTree => RobustSnapFailureCodeV1::EmptyDecisionTree,
            Self::EmptyEvidencePath => RobustSnapFailureCodeV1::EmptyEvidencePath,
            Self::EmptyEvidenceLeaf => RobustSnapFailureCodeV1::EmptyEvidenceLeaf,
            Self::LeafWorldOutsideFiber => RobustSnapFailureCodeV1::LeafWorldOutsideFiber,
            Self::WorldAppearsInMultipleLeaves => {
                RobustSnapFailureCodeV1::WorldAppearsInMultipleLeaves
            }
            Self::EvidenceTreeDropsWorld => RobustSnapFailureCodeV1::EvidenceTreeDropsWorld,
            Self::LeafEffectNotProtected => RobustSnapFailureCodeV1::LeafEffectNotProtected,
            Self::HeuristicDroppedWorld => RobustSnapFailureCodeV1::HeuristicDroppedWorld,
            Self::CertificateDigestMismatch { .. } => {
                RobustSnapFailureCodeV1::CertificateDigestMismatch
            }
            Self::Json(_) => RobustSnapFailureCodeV1::Json,
        }
    }
}

impl fmt::Display for RobustSnapErrorV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Robust Snap verification failed: {:?}", self.code())
    }
}

impl Error for RobustSnapErrorV1 {}

pub fn robust_snap_contract_manifest_v1() -> Value {
    json!({
        "contract": "zerostack.robust_snap",
        "contract_version": ROBUST_SNAP_CONTRACT_VERSION,
        "model_version": ROBUST_SNAP_MODEL_VERSION,
        "finite_bounds": {
            "worlds": ROBUST_SNAP_MAX_WORLDS,
            "effects": ROBUST_SNAP_MAX_EFFECTS,
            "leaves": ROBUST_SNAP_MAX_LEAVES,
            "evidence_depth": ROBUST_SNAP_MAX_EVIDENCE_DEPTH,
            "assumptions": ROBUST_SNAP_MAX_ASSUMPTIONS,
            "assumption_bytes": ROBUST_SNAP_MAX_ASSUMPTION_BYTES
        },
        "effect_equality": "effect_digest_and_effect_class",
        "s0_law": "selected_in_first_turn_and_verifiable_intersection_of_every_world_protected_set",
        "s1_law": "nonempty_exhaustive_disjoint_leaves_each_select_common_protected_verifiable_effect",
        "heuristic_law": "ordering_may_change_but_world_set_must_be_identical",
        "unknown_is_verified": false,
        "abstract_certificate_grants_operational_execution": false,
        "digest": "sha256(domain || canonical_json_without_certificate_digest)"
    })
}

pub fn robust_snap_contract_digest_v1() -> DigestV1 {
    DigestV1::from_bytes(sha256(
        canonical_json(&robust_snap_contract_manifest_v1()).as_bytes(),
    ))
}

#[cfg(test)]
#[path = "../../../tests/rust/zero-abi/unit/robust_snap.rs"]
mod tests;
