//! Exact finite Dominance-Complete Recovery (DCR) controller.
//!
//! A recoverable object is not enough: before any irreversible effect, the hub
//! computes `Complete`, `Conflict`, or `Unknown`. Only an opaque `Complete`
//! certificate identifies a model-accessible effect that is baseline-dominant
//! in every world of an exact or sound-overapproximated fiber. `Conflict`
//! selects a sound evidence query using exact rational dynamic programming.
//! `Unknown` requires the frozen raw-baseline route.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use zero_abi::{
    canonical_json,
    robust_snap::{
        ProtectedEffectSet, ProtectedEffectV1, WorldFiberDescriptor, ROBUST_SNAP_MAX_ASSUMPTIONS,
        ROBUST_SNAP_MAX_ASSUMPTION_BYTES, ROBUST_SNAP_MAX_EFFECTS, ROBUST_SNAP_MAX_WORLDS,
        ROBUST_SNAP_MODEL_VERSION,
    },
    sha256, DigestV1,
};
use zero_cert::{CompletenessWitness, Query, VerifiedEvidence};

pub const DCR_CONTRACT_VERSION_V1: u16 = 1;
pub const DCR_SCHEMA_VERSION_V1: &str = "racc-r-dcr-certificate/v1";
pub const DCR_PROBLEM_SCHEMA_VERSION_V1: &str = "zerostack.dcr.problem.v1";
pub const DCR_SCHEMA_SHA256_V1: &str =
    "bd41d160b90dfd153cce4bf6154d5b8d5e2b65b63d5b4f60ab421b43485db160";
pub const DCR_MAX_WORLDS_V1: usize = ROBUST_SNAP_MAX_WORLDS;
pub const DCR_MAX_HYPERGRAPH_STATES_V1: usize = 65_536;
pub const DCR_MAX_QUERIES_V1: usize = 32;
pub const DCR_MAX_OUTCOMES_PER_QUERY_V1: usize = 16;
pub const DCR_MAX_QUERY_TRACE_V1: usize = 32;
pub const DCR_MAX_CANONICAL_BYTES_V1: usize = 1_048_576;
pub const DCR_MAX_DP_STATES_V1: usize = 65_536;

const PROBLEM_DOMAIN_V1: &[u8] = b"zerostack.dcr.problem.v1\0";
const FIBER_DOMAIN_V1: &[u8] = b"zerostack.dcr.world_fiber.v1\0";
const SURFACE_DOMAIN_V1: &[u8] = b"zerostack.dcr.accessible_surface.v1\0";
const EFFECT_CLASS_DOMAIN_V1: &[u8] = b"zerostack.dcr.common_effect_class.v1\0";
const HYPEREDGE_DOMAIN_V1: &[u8] = b"zerostack.dcr.conflict_hyperedge.v1\0";
const CLAIM_DOMAIN_V1: &[u8] = b"zerostack.dcr.claim.v1\0";
const COMPLETE_DOMAIN_V1: &[u8] = b"zerostack.dcr.complete_certificate.v1\0";
const CONFLICT_DOMAIN_V1: &[u8] = b"zerostack.dcr.conflict_decision.v1\0";
const UNKNOWN_DOMAIN_V1: &[u8] = b"zerostack.dcr.unknown_decision.v1\0";
const VERIFIER_DOMAIN_V1: &[u8] = b"zerostack.dcr.verifier_identity.v1\0";
const OBSERVATION_DOMAIN_V1: &[u8] = b"zerostack.dcr.query_observation.v1\0";
const CONTRACT_DOMAIN_V1: &[u8] = b"zerostack.dcr.contract.v1\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFiberStatusV1 {
    Exact,
    SoundOverapproximation,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum DcrFiberStatusV1 {
    #[serde(rename = "exact")]
    Exact,
    #[serde(rename = "sound-overapproximation")]
    SoundOverapproximation,
    #[serde(rename = "unknown")]
    Unknown,
    #[serde(rename = "conflict")]
    Conflict,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorldRecoveryBudgetV1 {
    pub world_id: DigestV1,
    pub probability_weight: u64,
    pub raw_baseline_cost_units: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryQueryOutcomeV1 {
    pub outcome_digest: DigestV1,
    pub worlds: Vec<DigestV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryQueryV1 {
    pub query_digest: DigestV1,
    pub native_cost_units: u64,
    pub evidence_route_digest: DigestV1,
    pub outcomes: Vec<RecoveryQueryOutcomeV1>,
}

/// Canonical finite DCR problem. It is data, not authority. The controller only
/// accepts its exact bytes through successful `zero-cert` build/test evidence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DominanceRecoveryProblemV1 {
    schema_version: String,
    project_root: DigestV1,
    task_identity: DigestV1,
    fiber: WorldFiberDescriptor,
    fiber_status: SourceFiberStatusV1,
    baseline_identity: DigestV1,
    reasoning_contract_digest: DigestV1,
    decision_view_digest: DigestV1,
    protected_effects: Vec<ProtectedEffectSet>,
    accessible_effect_surface: Vec<ProtectedEffectV1>,
    world_budgets: Vec<WorldRecoveryBudgetV1>,
    queries: Vec<RecoveryQueryV1>,
    coverage_certificate: DigestV1,
    recovery_query_trace: Vec<DigestV1>,
    verifier_route: DigestV1,
    fallback_safepoint: DigestV1,
}

impl DominanceRecoveryProblemV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        project_root: DigestV1,
        task_identity: DigestV1,
        fiber: WorldFiberDescriptor,
        fiber_status: SourceFiberStatusV1,
        baseline_identity: DigestV1,
        reasoning_contract_digest: DigestV1,
        decision_view_digest: DigestV1,
        protected_effects: Vec<ProtectedEffectSet>,
        accessible_effect_surface: Vec<ProtectedEffectV1>,
        world_budgets: Vec<WorldRecoveryBudgetV1>,
        queries: Vec<RecoveryQueryV1>,
        coverage_certificate: DigestV1,
        recovery_query_trace: Vec<DigestV1>,
        verifier_route: DigestV1,
        fallback_safepoint: DigestV1,
    ) -> Result<Self, DcrErrorV1> {
        let problem = Self {
            schema_version: DCR_PROBLEM_SCHEMA_VERSION_V1.into(),
            project_root,
            task_identity,
            fiber,
            fiber_status,
            baseline_identity,
            reasoning_contract_digest,
            decision_view_digest,
            protected_effects,
            accessible_effect_surface,
            world_budgets,
            queries,
            coverage_certificate,
            recovery_query_trace,
            verifier_route,
            fallback_safepoint,
        };
        problem.validate()?;
        Ok(problem)
    }

    pub fn validate(&self) -> Result<(), DcrErrorV1> {
        if self.schema_version != DCR_PROBLEM_SCHEMA_VERSION_V1 {
            return Err(dcr_error(
                DcrFailureCodeV1::SchemaVersionMismatch,
                "DCR problem schema version is not v1",
            ));
        }
        require_nonzero(
            "DCR problem identity",
            &[
                self.project_root,
                self.task_identity,
                self.baseline_identity,
                self.reasoning_contract_digest,
                self.decision_view_digest,
                self.coverage_certificate,
                self.verifier_route,
                self.fallback_safepoint,
                self.fiber.assembly_manifest_digest,
                self.fiber.source_image_digest,
                self.fiber.task_fingerprint,
            ],
        )?;
        if self.fiber.model_version != ROBUST_SNAP_MODEL_VERSION {
            return Err(dcr_error(
                DcrFailureCodeV1::UnsupportedFiber,
                "world fiber does not use the frozen finite Robust Snap model",
            ));
        }
        if self.fiber.task_fingerprint != self.task_identity {
            return Err(dcr_error(
                DcrFailureCodeV1::IdentityMismatch,
                "world fiber task fingerprint differs from the DCR task identity",
            ));
        }
        if self.fiber.worlds.is_empty() || self.fiber.worlds.len() > DCR_MAX_WORLDS_V1 {
            return Err(dcr_error(
                DcrFailureCodeV1::BoundExceeded,
                "world fiber is empty or exceeds the exact finite DCR bound",
            ));
        }
        require_strict_order("fiber.worlds", &self.fiber.worlds)?;
        if self.fiber.assumptions.is_empty()
            || self.fiber.assumptions.len() > ROBUST_SNAP_MAX_ASSUMPTIONS
            || self.fiber.assumptions.iter().any(|assumption| {
                assumption.is_empty() || assumption.len() > ROBUST_SNAP_MAX_ASSUMPTION_BYTES
            })
        {
            return Err(dcr_error(
                DcrFailureCodeV1::UnsupportedFiber,
                "world-fiber assumptions are empty or exceed Robust Snap bounds",
            ));
        }
        require_strict_order("fiber.assumptions", &self.fiber.assumptions)?;

        if self.protected_effects.len() != self.fiber.worlds.len() {
            return Err(dcr_error(
                DcrFailureCodeV1::IncompleteCoverage,
                "every possible world must have one protected effect set",
            ));
        }
        let protected_worlds = self
            .protected_effects
            .iter()
            .map(|entry| entry.world_id)
            .collect::<Vec<_>>();
        require_strict_order("protected_effects", &protected_worlds)?;
        if protected_worlds != self.fiber.worlds {
            return Err(dcr_error(
                DcrFailureCodeV1::IncompleteCoverage,
                "protected effect sets do not cover the complete world fiber",
            ));
        }
        for entry in &self.protected_effects {
            if entry.effects.is_empty() || entry.effects.len() > ROBUST_SNAP_MAX_EFFECTS {
                return Err(dcr_error(
                    DcrFailureCodeV1::IncompleteCoverage,
                    "each world needs a bounded nonempty baseline-dominant effect set",
                ));
            }
            require_strict_order("protected_effects.effects", &entry.effects)?;
        }
        if self.accessible_effect_surface.len() > ROBUST_SNAP_MAX_EFFECTS {
            return Err(dcr_error(
                DcrFailureCodeV1::BoundExceeded,
                "model-accessible effect surface exceeds the Robust Snap effect bound",
            ));
        }
        require_strict_order("accessible_effect_surface", &self.accessible_effect_surface)?;

        if self.world_budgets.len() != self.fiber.worlds.len() {
            return Err(dcr_error(
                DcrFailureCodeV1::InvalidCostModel,
                "world probability and raw-baseline cost records must cover the fiber",
            ));
        }
        let budget_worlds = self
            .world_budgets
            .iter()
            .map(|entry| entry.world_id)
            .collect::<Vec<_>>();
        require_strict_order("world_budgets", &budget_worlds)?;
        if budget_worlds != self.fiber.worlds
            || self
                .world_budgets
                .iter()
                .any(|entry| entry.probability_weight == 0 || entry.raw_baseline_cost_units == 0)
        {
            return Err(dcr_error(
                DcrFailureCodeV1::InvalidCostModel,
                "world weights and fully charged raw-baseline costs must be positive",
            ));
        }

        if self.queries.len() > DCR_MAX_QUERIES_V1 {
            return Err(dcr_error(
                DcrFailureCodeV1::BoundExceeded,
                "recovery query set exceeds its finite bound",
            ));
        }
        let query_ids = self
            .queries
            .iter()
            .map(|query| query.query_digest)
            .collect::<Vec<_>>();
        require_strict_order("queries", &query_ids)?;
        for query in &self.queries {
            self.validate_query(query)?;
        }
        if self.recovery_query_trace.len() > DCR_MAX_QUERY_TRACE_V1
            || self
                .recovery_query_trace
                .iter()
                .any(|digest| *digest == DigestV1::ZERO)
            || self
                .recovery_query_trace
                .iter()
                .collect::<BTreeSet<_>>()
                .len()
                != self.recovery_query_trace.len()
        {
            return Err(dcr_error(
                DcrFailureCodeV1::InvalidQueryTrace,
                "query trace is too long, zero-valued, or repeats an observation",
            ));
        }
        Ok(())
    }

    fn validate_query(&self, query: &RecoveryQueryV1) -> Result<(), DcrErrorV1> {
        require_nonzero(
            "recovery query",
            &[query.query_digest, query.evidence_route_digest],
        )?;
        if query.native_cost_units == 0
            || query.outcomes.len() < 2
            || query.outcomes.len() > DCR_MAX_OUTCOMES_PER_QUERY_V1
        {
            return Err(dcr_error(
                DcrFailureCodeV1::InvalidQuery,
                "query needs a positive native cost and a bounded discriminating partition",
            ));
        }
        let outcome_ids = query
            .outcomes
            .iter()
            .map(|outcome| outcome.outcome_digest)
            .collect::<Vec<_>>();
        require_strict_order("query.outcomes", &outcome_ids)?;
        let expected = self.fiber.worlds.iter().copied().collect::<BTreeSet<_>>();
        let mut observed = BTreeSet::new();
        for outcome in &query.outcomes {
            if outcome.outcome_digest == DigestV1::ZERO || outcome.worlds.is_empty() {
                return Err(dcr_error(
                    DcrFailureCodeV1::InvalidQuery,
                    "query outcomes require nonzero identities and nonempty world cells",
                ));
            }
            require_strict_order("query.outcome.worlds", &outcome.worlds)?;
            for world in &outcome.worlds {
                if !expected.contains(world) || !observed.insert(*world) {
                    return Err(dcr_error(
                        DcrFailureCodeV1::InvalidQuery,
                        "query outcomes must be a disjoint partition of the world fiber",
                    ));
                }
            }
        }
        if observed != expected {
            return Err(dcr_error(
                DcrFailureCodeV1::InvalidQuery,
                "query outcomes drop possible worlds",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, DcrErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, DcrErrorV1> {
        let problem: Self = decode_canonical(bytes)?;
        problem.validate()?;
        Ok(problem)
    }

    pub fn digest(&self) -> Result<DigestV1, DcrErrorV1> {
        Ok(domain_digest(PROBLEM_DOMAIN_V1, &self.canonical_bytes()?))
    }

    pub fn world_fiber_digest(&self) -> Result<DigestV1, DcrErrorV1> {
        Ok(domain_digest(
            FIBER_DOMAIN_V1,
            &canonical_bytes(&self.fiber)?,
        ))
    }

    pub fn accessible_effect_surface_digest(&self) -> Result<DigestV1, DcrErrorV1> {
        Ok(domain_digest(
            SURFACE_DOMAIN_V1,
            &canonical_bytes(&self.accessible_effect_surface)?,
        ))
    }

    /// Apply an exact query outcome to the finite problem. The returned value is
    /// still untrusted data and must be re-proved before the next controller step.
    #[allow(clippy::too_many_arguments)]
    pub fn condition_on(
        &self,
        query_digest: DigestV1,
        outcome_digest: DigestV1,
        observation_receipt: DigestV1,
        decision_view_digest: DigestV1,
        accessible_effect_surface: Vec<ProtectedEffectV1>,
        coverage_certificate: DigestV1,
        verifier_route: DigestV1,
        fallback_safepoint: DigestV1,
    ) -> Result<Self, DcrErrorV1> {
        require_nonzero(
            "conditioned recovery identity",
            &[
                query_digest,
                outcome_digest,
                observation_receipt,
                decision_view_digest,
                coverage_certificate,
                verifier_route,
                fallback_safepoint,
            ],
        )?;
        let query = self
            .queries
            .iter()
            .find(|query| query.query_digest == query_digest)
            .ok_or_else(|| dcr_error(DcrFailureCodeV1::InvalidQuery, "query is not admissible"))?;
        let outcome = query
            .outcomes
            .iter()
            .find(|outcome| outcome.outcome_digest == outcome_digest)
            .ok_or_else(|| {
                dcr_error(
                    DcrFailureCodeV1::InvalidQuery,
                    "observation is not an outcome of the selected query",
                )
            })?;
        let keep = outcome.worlds.iter().copied().collect::<BTreeSet<_>>();
        let mut conditioned = self.clone();
        conditioned
            .fiber
            .worlds
            .retain(|world| keep.contains(world));
        conditioned
            .protected_effects
            .retain(|entry| keep.contains(&entry.world_id));
        conditioned
            .world_budgets
            .retain(|entry| keep.contains(&entry.world_id));
        conditioned.queries = conditioned
            .queries
            .into_iter()
            .filter_map(|mut candidate| {
                for cell in &mut candidate.outcomes {
                    cell.worlds.retain(|world| keep.contains(world));
                }
                candidate.outcomes.retain(|cell| !cell.worlds.is_empty());
                (candidate.outcomes.len() >= 2).then_some(candidate)
            })
            .collect();
        conditioned.decision_view_digest = decision_view_digest;
        conditioned.accessible_effect_surface = accessible_effect_surface;
        conditioned.coverage_certificate = coverage_certificate;
        conditioned.verifier_route = verifier_route;
        conditioned.fallback_safepoint = fallback_safepoint;
        let observation_digest = digest_value(
            OBSERVATION_DOMAIN_V1,
            &json!({
                "evidence_route_digest": query.evidence_route_digest,
                "observation_receipt": observation_receipt,
                "outcome_digest": outcome_digest,
                "query_digest": query_digest,
            }),
        );
        conditioned.recovery_query_trace.push(observation_digest);
        conditioned.validate()?;
        Ok(conditioned)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExactRecoveryCostV1 {
    #[serde(with = "u128_decimal")]
    numerator: u128,
    #[serde(with = "u128_decimal")]
    denominator: u128,
}

mod u128_decimal {
    use serde::{de::Error as _, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &u128, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u128, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(D::Error::custom)
    }
}

impl ExactRecoveryCostV1 {
    pub const fn numerator(self) -> u128 {
        self.numerator
    }
    pub const fn denominator(self) -> u128 {
        self.denominator
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConflictHyperedgeV1 {
    worlds: Vec<DigestV1>,
    hyperedge_digest: DigestV1,
}

impl ConflictHyperedgeV1 {
    pub fn worlds(&self) -> &[DigestV1] {
        &self.worlds
    }
    pub const fn digest(&self) -> DigestV1 {
        self.hyperedge_digest
    }
}

/// External v1 certificate claim. This exactly matches the published schema;
/// deserializing it does not create execution authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DominanceCompleteRecoveryClaimV1 {
    schema_version: String,
    project_root: DigestV1,
    task_identity: DigestV1,
    world_fiber_digest: DigestV1,
    fiber_status: DcrFiberStatusV1,
    baseline_identity: DigestV1,
    reasoning_contract_digest: DigestV1,
    decision_view_digest: DigestV1,
    accessible_effect_surface_digest: DigestV1,
    common_baseline_dominant_effect_class: DigestV1,
    coverage_certificate: DigestV1,
    conflict_hyperedges: Vec<DigestV1>,
    recovery_query_trace: Vec<DigestV1>,
    verifier_route: DigestV1,
    fallback_safepoint: DigestV1,
}

impl DominanceCompleteRecoveryClaimV1 {
    fn from_complete(
        problem: &DominanceRecoveryProblemV1,
        common_effects: &[ProtectedEffectV1],
    ) -> Result<Self, DcrErrorV1> {
        if common_effects.is_empty() {
            return Err(dcr_error(
                DcrFailureCodeV1::NotDominanceComplete,
                "a Complete claim requires a nonempty common accessible effect class",
            ));
        }
        let fiber_status = match problem.fiber_status {
            SourceFiberStatusV1::Exact => DcrFiberStatusV1::Exact,
            SourceFiberStatusV1::SoundOverapproximation => DcrFiberStatusV1::SoundOverapproximation,
            SourceFiberStatusV1::Unknown => {
                return Err(dcr_error(
                    DcrFailureCodeV1::UnknownFiberCannotComplete,
                    "an unknown fiber cannot mint strict DCR authority",
                ));
            }
        };
        let claim = Self {
            schema_version: DCR_SCHEMA_VERSION_V1.into(),
            project_root: problem.project_root,
            task_identity: problem.task_identity,
            world_fiber_digest: problem.world_fiber_digest()?,
            fiber_status,
            baseline_identity: problem.baseline_identity,
            reasoning_contract_digest: problem.reasoning_contract_digest,
            decision_view_digest: problem.decision_view_digest,
            accessible_effect_surface_digest: problem.accessible_effect_surface_digest()?,
            common_baseline_dominant_effect_class: domain_digest(
                EFFECT_CLASS_DOMAIN_V1,
                &canonical_bytes(common_effects)?,
            ),
            coverage_certificate: problem.coverage_certificate,
            conflict_hyperedges: Vec::new(),
            recovery_query_trace: problem.recovery_query_trace.clone(),
            verifier_route: problem.verifier_route,
            fallback_safepoint: problem.fallback_safepoint,
        };
        claim.validate()?;
        Ok(claim)
    }

    pub fn validate(&self) -> Result<(), DcrErrorV1> {
        if self.schema_version != DCR_SCHEMA_VERSION_V1 {
            return Err(dcr_error(
                DcrFailureCodeV1::SchemaVersionMismatch,
                "DCR certificate claim schema version is not v1",
            ));
        }
        if !matches!(
            self.fiber_status,
            DcrFiberStatusV1::Exact | DcrFiberStatusV1::SoundOverapproximation
        ) {
            return Err(dcr_error(
                DcrFailureCodeV1::UnknownFiberCannotComplete,
                "Conflict and Unknown records cannot be replayed as Complete",
            ));
        }
        require_nonzero(
            "DCR complete claim",
            &[
                self.project_root,
                self.task_identity,
                self.world_fiber_digest,
                self.baseline_identity,
                self.reasoning_contract_digest,
                self.decision_view_digest,
                self.accessible_effect_surface_digest,
                self.common_baseline_dominant_effect_class,
                self.coverage_certificate,
                self.verifier_route,
                self.fallback_safepoint,
            ],
        )?;
        if !self.conflict_hyperedges.is_empty() {
            return Err(dcr_error(
                DcrFailureCodeV1::NotDominanceComplete,
                "Complete claim cannot carry unresolved conflict hyperedges",
            ));
        }
        if self.recovery_query_trace.len() > DCR_MAX_QUERY_TRACE_V1
            || self
                .recovery_query_trace
                .iter()
                .any(|digest| *digest == DigestV1::ZERO)
        {
            return Err(dcr_error(
                DcrFailureCodeV1::InvalidQueryTrace,
                "complete claim query trace is invalid",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, DcrErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, DcrErrorV1> {
        let claim: Self = decode_canonical(bytes)?;
        claim.validate()?;
        Ok(claim)
    }

    pub fn digest(&self) -> Result<DigestV1, DcrErrorV1> {
        Ok(domain_digest(CLAIM_DOMAIN_V1, &self.canonical_bytes()?))
    }

    pub const fn common_effect_class_digest(&self) -> DigestV1 {
        self.common_baseline_dominant_effect_class
    }
    pub const fn verifier_route(&self) -> DigestV1 {
        self.verifier_route
    }
}

/// Opaque controller authority. Only exact finite recomputation over verified
/// problem bytes can create this value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DominanceCompleteRecoveryCertificateV1 {
    contract_version: u16,
    problem: DominanceRecoveryProblemV1,
    claim: DominanceCompleteRecoveryClaimV1,
    common_effects: Vec<ProtectedEffectV1>,
    problem_digest: DigestV1,
    evidence_digest: DigestV1,
    verifier_identity_digest: DigestV1,
    certificate_digest: DigestV1,
}

impl DominanceCompleteRecoveryCertificateV1 {
    fn mint(
        problem: DominanceRecoveryProblemV1,
        common_effects: Vec<ProtectedEffectV1>,
        evidence_digest: DigestV1,
        verifier_identity_digest: DigestV1,
    ) -> Result<Self, DcrErrorV1> {
        let claim = DominanceCompleteRecoveryClaimV1::from_complete(&problem, &common_effects)?;
        let problem_digest = problem.digest()?;
        let certificate_digest = complete_certificate_digest(
            problem_digest,
            claim.digest()?,
            evidence_digest,
            verifier_identity_digest,
            &common_effects,
        )?;
        let certificate = Self {
            contract_version: DCR_CONTRACT_VERSION_V1,
            problem,
            claim,
            common_effects,
            problem_digest,
            evidence_digest,
            verifier_identity_digest,
            certificate_digest,
        };
        certificate.validate()?;
        Ok(certificate)
    }

    pub fn validate(&self) -> Result<(), DcrErrorV1> {
        if self.contract_version != DCR_CONTRACT_VERSION_V1 {
            return Err(dcr_error(
                DcrFailureCodeV1::SchemaVersionMismatch,
                "DCR certificate contract version is not v1",
            ));
        }
        self.problem.validate()?;
        self.claim.validate()?;
        require_nonzero(
            "DCR certificate proof binding",
            &[self.evidence_digest, self.verifier_identity_digest],
        )?;
        if self.problem.verifier_route != self.verifier_identity_digest
            || self.problem.digest()? != self.problem_digest
            || self.claim
                != DominanceCompleteRecoveryClaimV1::from_complete(
                    &self.problem,
                    &self.common_effects,
                )?
            || common_effects(&self.problem, full_mask(&self.problem))? != self.common_effects
            || complete_certificate_digest(
                self.problem_digest,
                self.claim.digest()?,
                self.evidence_digest,
                self.verifier_identity_digest,
                &self.common_effects,
            )? != self.certificate_digest
        {
            return Err(dcr_error(
                DcrFailureCodeV1::CertificateDigestMismatch,
                "DCR certificate does not replay against its problem, claim, and proof binding",
            ));
        }
        Ok(())
    }

    pub fn record(&self) -> DominanceCompleteRecoveryCertificateRecordV1 {
        DominanceCompleteRecoveryCertificateRecordV1 {
            contract_version: self.contract_version,
            problem: self.problem.clone(),
            claim: self.claim.clone(),
            common_effects: self.common_effects.clone(),
            problem_digest: self.problem_digest,
            evidence_digest: self.evidence_digest,
            verifier_identity_digest: self.verifier_identity_digest,
            certificate_digest: self.certificate_digest,
        }
    }

    pub const fn claim(&self) -> &DominanceCompleteRecoveryClaimV1 {
        &self.claim
    }
    pub fn common_effects(&self) -> &[ProtectedEffectV1] {
        &self.common_effects
    }
    pub const fn certificate_digest(&self) -> DigestV1 {
        self.certificate_digest
    }
}

/// Replay-validatable receipt form. It cannot authorize execution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DominanceCompleteRecoveryCertificateRecordV1 {
    pub contract_version: u16,
    pub problem: DominanceRecoveryProblemV1,
    pub claim: DominanceCompleteRecoveryClaimV1,
    pub common_effects: Vec<ProtectedEffectV1>,
    pub problem_digest: DigestV1,
    pub evidence_digest: DigestV1,
    pub verifier_identity_digest: DigestV1,
    pub certificate_digest: DigestV1,
}

impl DominanceCompleteRecoveryCertificateRecordV1 {
    pub fn validate(&self) -> Result<(), DcrErrorV1> {
        DominanceCompleteRecoveryCertificateV1 {
            contract_version: self.contract_version,
            problem: self.problem.clone(),
            claim: self.claim.clone(),
            common_effects: self.common_effects.clone(),
            problem_digest: self.problem_digest,
            evidence_digest: self.evidence_digest,
            verifier_identity_digest: self.verifier_identity_digest,
            certificate_digest: self.certificate_digest,
        }
        .validate()
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, DcrErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, DcrErrorV1> {
        let record: Self = decode_canonical(bytes)?;
        record.validate()?;
        Ok(record)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryConflictDecisionV1 {
    problem_digest: DigestV1,
    selected_query: RecoveryQueryV1,
    admissible_queries: Vec<DigestV1>,
    conflict_hyperedges: Vec<ConflictHyperedgeV1>,
    optimal_expected_cost: ExactRecoveryCostV1,
    raw_baseline_expected_cost: ExactRecoveryCostV1,
    fallback_safepoint: DigestV1,
    decision_digest: DigestV1,
}

impl RecoveryConflictDecisionV1 {
    pub const fn selected_query(&self) -> &RecoveryQueryV1 {
        &self.selected_query
    }
    pub fn conflict_hyperedges(&self) -> &[ConflictHyperedgeV1] {
        &self.conflict_hyperedges
    }
    pub const fn optimal_expected_cost(&self) -> ExactRecoveryCostV1 {
        self.optimal_expected_cost
    }
    pub const fn raw_baseline_expected_cost(&self) -> ExactRecoveryCostV1 {
        self.raw_baseline_expected_cost
    }
    pub const fn decision_digest(&self) -> DigestV1 {
        self.decision_digest
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryUnknownReasonV1 {
    FiberUnknown,
    NoSoundQuery,
    RawBaselineCheaperOrEqual,
    AnalysisBoundExceeded,
    CostArithmeticOverflow,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryUnknownDecisionV1 {
    problem_digest: DigestV1,
    reason: RecoveryUnknownReasonV1,
    raw_baseline_required: bool,
    conflict_hyperedges: Vec<ConflictHyperedgeV1>,
    raw_baseline_expected_cost: Option<ExactRecoveryCostV1>,
    fallback_safepoint: DigestV1,
    decision_digest: DigestV1,
}

impl RecoveryUnknownDecisionV1 {
    pub const fn problem_digest(&self) -> DigestV1 {
        self.problem_digest
    }
    pub const fn reason(&self) -> RecoveryUnknownReasonV1 {
        self.reason
    }
    pub const fn raw_baseline_required(&self) -> bool {
        self.raw_baseline_required
    }
    pub fn conflict_hyperedges(&self) -> &[ConflictHyperedgeV1] {
        &self.conflict_hyperedges
    }
    pub const fn fallback_safepoint(&self) -> DigestV1 {
        self.fallback_safepoint
    }
    pub const fn decision_digest(&self) -> DigestV1 {
        self.decision_digest
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "decision")]
pub enum RecoveryDecisionV1 {
    Complete(DominanceCompleteRecoveryCertificateV1),
    Conflict(RecoveryConflictDecisionV1),
    Unknown(RecoveryUnknownDecisionV1),
}

/// Automatic DCR trigger. `Conflict` requires the selected exact query before an
/// irreversible effect. `Unknown` always requires frozen raw-baseline fallback.
pub fn dominance_complete_recover_v1(
    problem: DominanceRecoveryProblemV1,
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<RecoveryDecisionV1, DcrErrorV1> {
    problem.validate()?;
    verify_problem_evidence(&problem, evidence)?;
    let problem_digest = problem.digest()?;
    let evidence_digest = DigestV1::from_bytes(
        evidence
            .certificate()
            .canonical_digest()
            .map_err(|error| json_error(error.to_string()))?,
    );
    let verifier_identity_digest = dcr_verifier_identity_v1(evidence);
    if verifier_identity_digest != problem.verifier_route {
        return Err(dcr_error(
            DcrFailureCodeV1::VerifierIdentityMismatch,
            "verified evidence route differs from the DCR problem route",
        ));
    }

    if problem.fiber_status == SourceFiberStatusV1::Unknown {
        return Ok(RecoveryDecisionV1::Unknown(unknown_decision(
            &problem,
            problem_digest,
            RecoveryUnknownReasonV1::FiberUnknown,
            Vec::new(),
            baseline_cost(&problem, full_mask(&problem)).ok(),
        )?));
    }

    let root = full_mask(&problem);
    let effect_masks = accessible_world_effect_masks(&problem);
    let common = common_effects(&problem, root)?;
    if !common.is_empty() {
        return Ok(RecoveryDecisionV1::Complete(
            DominanceCompleteRecoveryCertificateV1::mint(
                problem,
                common,
                evidence_digest,
                verifier_identity_digest,
            )?,
        ));
    }

    let raw_cost = match baseline_cost(&problem, root) {
        Ok(cost) => cost,
        Err(DpFailure::Overflow) => {
            return Ok(RecoveryDecisionV1::Unknown(unknown_decision(
                &problem,
                problem_digest,
                RecoveryUnknownReasonV1::CostArithmeticOverflow,
                Vec::new(),
                None,
            )?));
        }
        Err(DpFailure::StateBound) => {
            return Ok(RecoveryDecisionV1::Unknown(unknown_decision(
                &problem,
                problem_digest,
                RecoveryUnknownReasonV1::AnalysisBoundExceeded,
                Vec::new(),
                None,
            )?));
        }
    };
    let hyperedges = match conflict_hyperedges(&problem, problem_digest, &effect_masks) {
        Ok(hyperedges) => hyperedges,
        Err(error) if error.failure_code() == DcrFailureCodeV1::AnalysisBoundExceeded => {
            return Ok(RecoveryDecisionV1::Unknown(unknown_decision(
                &problem,
                problem_digest,
                RecoveryUnknownReasonV1::AnalysisBoundExceeded,
                Vec::new(),
                Some(raw_cost),
            )?));
        }
        Err(error) => return Err(error),
    };
    let admissible_queries = problem
        .queries
        .iter()
        .filter(|query| query_children(&problem, root, query).len() >= 2)
        .map(|query| query.query_digest)
        .collect::<Vec<_>>();
    if admissible_queries.is_empty() {
        return Ok(RecoveryDecisionV1::Unknown(unknown_decision(
            &problem,
            problem_digest,
            RecoveryUnknownReasonV1::NoSoundQuery,
            hyperedges,
            Some(raw_cost),
        )?));
    }

    let mut memo = BTreeMap::new();
    let root_plan = match solve(&problem, &effect_masks, root, &mut memo) {
        Ok(plan) => plan,
        Err(DpFailure::StateBound) => {
            return Ok(RecoveryDecisionV1::Unknown(unknown_decision(
                &problem,
                problem_digest,
                RecoveryUnknownReasonV1::AnalysisBoundExceeded,
                hyperedges,
                Some(raw_cost),
            )?));
        }
        Err(DpFailure::Overflow) => {
            return Ok(RecoveryDecisionV1::Unknown(unknown_decision(
                &problem,
                problem_digest,
                RecoveryUnknownReasonV1::CostArithmeticOverflow,
                hyperedges,
                Some(raw_cost),
            )?));
        }
    };
    let DpChoice::Query(index) = root_plan.choice else {
        return Ok(RecoveryDecisionV1::Unknown(unknown_decision(
            &problem,
            problem_digest,
            RecoveryUnknownReasonV1::RawBaselineCheaperOrEqual,
            hyperedges,
            Some(raw_cost),
        )?));
    };
    let selected_query = problem.queries[index].clone();
    let optimal_expected_cost = root_plan.cost.into_public();
    let raw_baseline_expected_cost = raw_cost.into_public();
    let decision_digest = digest_value(
        CONFLICT_DOMAIN_V1,
        &json!({
            "admissible_queries": admissible_queries,
            "conflict_hyperedges": hyperedges,
            "fallback_safepoint": problem.fallback_safepoint,
            "optimal_expected_cost": optimal_expected_cost,
            "problem_digest": problem_digest,
            "raw_baseline_expected_cost": raw_baseline_expected_cost,
            "selected_query": selected_query,
        }),
    );
    Ok(RecoveryDecisionV1::Conflict(RecoveryConflictDecisionV1 {
        problem_digest,
        selected_query,
        admissible_queries,
        conflict_hyperedges: hyperedges,
        optimal_expected_cost,
        raw_baseline_expected_cost,
        fallback_safepoint: problem.fallback_safepoint,
        decision_digest,
    }))
}

fn verify_problem_evidence(
    problem: &DominanceRecoveryProblemV1,
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<(), DcrErrorV1> {
    match (evidence.query(), &evidence.certificate().completeness) {
        (Query::BuildReceipt { .. }, CompletenessWitness::BuildReceipt { exit_code: 0, .. })
        | (Query::TestTrace { .. }, CompletenessWitness::TestTrace { exit_code: 0, .. }) => {}
        _ => {
            return Err(dcr_error(
                DcrFailureCodeV1::UnsupportedEvidenceClass,
                "DCR requires a successful verified build receipt or test trace",
            ));
        }
    }
    if evidence.payload() != problem.canonical_bytes()? {
        return Err(dcr_error(
            DcrFailureCodeV1::EvidencePayloadMismatch,
            "verified evidence payload is not the exact canonical DCR problem",
        ));
    }
    Ok(())
}

pub fn dcr_verifier_identity_v1(evidence: &VerifiedEvidence<'_, '_>) -> DigestV1 {
    let provenance = evidence.provenance();
    digest_value(
        VERIFIER_DOMAIN_V1,
        &json!({
            "index_id": provenance.index_id,
            "index_version": provenance.index_version,
            "operator_id": provenance.operator_id,
            "operator_version": provenance.operator_version,
            "parser_id": provenance.parser_id,
            "parser_version": provenance.parser_version,
        }),
    )
}

fn full_mask(problem: &DominanceRecoveryProblemV1) -> u64 {
    (1_u64 << problem.fiber.worlds.len()) - 1
}

fn accessible_world_effect_masks(problem: &DominanceRecoveryProblemV1) -> Vec<u64> {
    problem
        .protected_effects
        .iter()
        .map(|protected| {
            protected.effects.iter().fold(0_u64, |mask, effect| {
                problem
                    .accessible_effect_surface
                    .binary_search(effect)
                    .map_or(mask, |index| mask | (1_u64 << index))
            })
        })
        .collect()
}

fn common_effect_bits(effect_masks: &[u64], world_mask: u64, effect_count: usize) -> u64 {
    let mut common = if effect_count == u64::BITS as usize {
        u64::MAX
    } else {
        (1_u64 << effect_count) - 1
    };
    for (index, effects) in effect_masks.iter().enumerate() {
        if world_mask & (1_u64 << index) != 0 {
            common &= effects;
            if common == 0 {
                break;
            }
        }
    }
    common
}

fn common_effects(
    problem: &DominanceRecoveryProblemV1,
    mask: u64,
) -> Result<Vec<ProtectedEffectV1>, DcrErrorV1> {
    let effect_masks = accessible_world_effect_masks(problem);
    let bits = common_effect_bits(&effect_masks, mask, problem.accessible_effect_surface.len());
    Ok(problem
        .accessible_effect_surface
        .iter()
        .enumerate()
        .filter(|(index, _)| bits & (1_u64 << index) != 0)
        .map(|(_, effect)| effect.clone())
        .collect())
}

fn conflict_hyperedges(
    problem: &DominanceRecoveryProblemV1,
    problem_digest: DigestV1,
    effect_masks: &[u64],
) -> Result<Vec<ConflictHyperedgeV1>, DcrErrorV1> {
    let full = full_mask(problem);
    let mut explored = 0_usize;
    let mut minimal_masks = Vec::new();
    for mask in 1..=full {
        explored = explored.saturating_add(1);
        if explored > DCR_MAX_HYPERGRAPH_STATES_V1 {
            return Err(dcr_error(
                DcrFailureCodeV1::AnalysisBoundExceeded,
                "exact minimal-hyperedge enumeration exceeded its state bound",
            ));
        }
        if common_effect_bits(effect_masks, mask, problem.accessible_effect_surface.len()) != 0 {
            continue;
        }
        let mut remaining = mask;
        let mut minimal = true;
        while remaining != 0 {
            let index = remaining.trailing_zeros();
            let bit = 1_u64 << index;
            remaining &= !bit;
            if common_effect_bits(
                effect_masks,
                mask & !bit,
                problem.accessible_effect_surface.len(),
            ) == 0
            {
                minimal = false;
                break;
            }
        }
        if minimal {
            minimal_masks.push(mask);
        }
    }
    minimal_masks
        .into_iter()
        .map(|mask| {
            let worlds = problem
                .fiber
                .worlds
                .iter()
                .enumerate()
                .filter(|(index, _)| mask & (1_u64 << index) != 0)
                .map(|(_, world)| *world)
                .collect::<Vec<_>>();
            let hyperedge_digest = digest_value(
                HYPEREDGE_DOMAIN_V1,
                &json!({
                    "problem_digest": problem_digest,
                    "worlds": worlds,
                }),
            );
            Ok(ConflictHyperedgeV1 {
                worlds,
                hyperedge_digest,
            })
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Rational {
    numerator: u128,
    denominator: u128,
}

impl Rational {
    const ZERO: Self = Self {
        numerator: 0,
        denominator: 1,
    };

    fn new(numerator: u128, denominator: u128) -> Result<Self, DpFailure> {
        if denominator == 0 {
            return Err(DpFailure::Overflow);
        }
        let divisor = gcd(numerator, denominator);
        Ok(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    const fn integer(value: u64) -> Self {
        Self {
            numerator: value as u128,
            denominator: 1,
        }
    }

    fn add(self, other: Self) -> Result<Self, DpFailure> {
        let common = gcd(self.denominator, other.denominator);
        let left_scale = other.denominator / common;
        let right_scale = self.denominator / common;
        let numerator = self
            .numerator
            .checked_mul(left_scale)
            .and_then(|left| {
                other
                    .numerator
                    .checked_mul(right_scale)
                    .and_then(|right| left.checked_add(right))
            })
            .ok_or(DpFailure::Overflow)?;
        let denominator = self
            .denominator
            .checked_mul(left_scale)
            .ok_or(DpFailure::Overflow)?;
        Self::new(numerator, denominator)
    }

    fn mul_fraction(self, numerator: u128, denominator: u128) -> Result<Self, DpFailure> {
        let left_cancel = gcd(self.numerator, denominator);
        let right_cancel = gcd(numerator, self.denominator);
        let result_numerator = (self.numerator / left_cancel)
            .checked_mul(numerator / right_cancel)
            .ok_or(DpFailure::Overflow)?;
        let result_denominator = (self.denominator / right_cancel)
            .checked_mul(denominator / left_cancel)
            .ok_or(DpFailure::Overflow)?;
        Self::new(result_numerator, result_denominator)
    }

    fn compare(self, other: Self) -> Result<Ordering, DpFailure> {
        let common = gcd(self.denominator, other.denominator);
        let left = self
            .numerator
            .checked_mul(other.denominator / common)
            .ok_or(DpFailure::Overflow)?;
        let right = other
            .numerator
            .checked_mul(self.denominator / common)
            .ok_or(DpFailure::Overflow)?;
        Ok(left.cmp(&right))
    }

    const fn into_public(self) -> ExactRecoveryCostV1 {
        ExactRecoveryCostV1 {
            numerator: self.numerator,
            denominator: self.denominator,
        }
    }
}

const fn gcd(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    if left == 0 {
        1
    } else {
        left
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DpChoice {
    Complete,
    Baseline,
    Query(usize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DpNode {
    cost: Rational,
    choice: DpChoice,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DpFailure {
    StateBound,
    Overflow,
}

fn solve(
    problem: &DominanceRecoveryProblemV1,
    effect_masks: &[u64],
    mask: u64,
    memo: &mut BTreeMap<u64, DpNode>,
) -> Result<DpNode, DpFailure> {
    if let Some(node) = memo.get(&mask) {
        return Ok(*node);
    }
    if memo.len() >= DCR_MAX_DP_STATES_V1 {
        return Err(DpFailure::StateBound);
    }
    if common_effect_bits(effect_masks, mask, problem.accessible_effect_surface.len()) != 0 {
        let node = DpNode {
            cost: Rational::ZERO,
            choice: DpChoice::Complete,
        };
        memo.insert(mask, node);
        return Ok(node);
    }

    let mut best = DpNode {
        cost: baseline_cost(problem, mask)?,
        choice: DpChoice::Baseline,
    };
    let parent_weight = mask_weight(problem, mask)?;
    for (index, query) in problem.queries.iter().enumerate() {
        let children = query_children(problem, mask, query);
        if children.len() < 2 {
            continue;
        }
        let mut expected = Rational::integer(query.native_cost_units);
        for child in children {
            let child_node = solve(problem, effect_masks, child, memo)?;
            expected = expected.add(
                child_node
                    .cost
                    .mul_fraction(mask_weight(problem, child)?, parent_weight)?,
            )?;
        }
        if expected.compare(best.cost)? == Ordering::Less {
            best = DpNode {
                cost: expected,
                choice: DpChoice::Query(index),
            };
        }
    }
    memo.insert(mask, best);
    Ok(best)
}

fn query_children(
    problem: &DominanceRecoveryProblemV1,
    mask: u64,
    query: &RecoveryQueryV1,
) -> Vec<u64> {
    query
        .outcomes
        .iter()
        .filter_map(|outcome| {
            let mut child = 0_u64;
            for world in &outcome.worlds {
                if let Ok(index) = problem.fiber.worlds.binary_search(world) {
                    child |= 1_u64 << index;
                }
            }
            let child = child & mask;
            (child != 0 && child != mask).then_some(child)
        })
        .collect()
}

fn mask_weight(problem: &DominanceRecoveryProblemV1, mask: u64) -> Result<u128, DpFailure> {
    problem
        .world_budgets
        .iter()
        .enumerate()
        .filter(|(index, _)| mask & (1_u64 << index) != 0)
        .try_fold(0_u128, |sum, (_, budget)| {
            sum.checked_add(budget.probability_weight as u128)
                .ok_or(DpFailure::Overflow)
        })
}

fn baseline_cost(problem: &DominanceRecoveryProblemV1, mask: u64) -> Result<Rational, DpFailure> {
    let numerator = problem
        .world_budgets
        .iter()
        .enumerate()
        .filter(|(index, _)| mask & (1_u64 << index) != 0)
        .try_fold(0_u128, |sum, (_, budget)| {
            (budget.probability_weight as u128)
                .checked_mul(budget.raw_baseline_cost_units as u128)
                .and_then(|value| sum.checked_add(value))
                .ok_or(DpFailure::Overflow)
        })?;
    Rational::new(numerator, mask_weight(problem, mask)?)
}

fn unknown_decision(
    problem: &DominanceRecoveryProblemV1,
    problem_digest: DigestV1,
    reason: RecoveryUnknownReasonV1,
    conflict_hyperedges: Vec<ConflictHyperedgeV1>,
    raw_baseline_expected_cost: Option<Rational>,
) -> Result<RecoveryUnknownDecisionV1, DcrErrorV1> {
    let raw_baseline_expected_cost = raw_baseline_expected_cost.map(Rational::into_public);
    let decision_digest = digest_value(
        UNKNOWN_DOMAIN_V1,
        &json!({
            "conflict_hyperedges": conflict_hyperedges,
            "fallback_safepoint": problem.fallback_safepoint,
            "problem_digest": problem_digest,
            "raw_baseline_expected_cost": raw_baseline_expected_cost,
            "raw_baseline_required": true,
            "reason": reason,
        }),
    );
    Ok(RecoveryUnknownDecisionV1 {
        problem_digest,
        reason,
        raw_baseline_required: true,
        conflict_hyperedges,
        raw_baseline_expected_cost,
        fallback_safepoint: problem.fallback_safepoint,
        decision_digest,
    })
}

fn complete_certificate_digest(
    problem_digest: DigestV1,
    claim_digest: DigestV1,
    evidence_digest: DigestV1,
    verifier_identity_digest: DigestV1,
    common_effects: &[ProtectedEffectV1],
) -> Result<DigestV1, DcrErrorV1> {
    Ok(digest_value(
        COMPLETE_DOMAIN_V1,
        &json!({
            "claim_digest": claim_digest,
            "common_effects": common_effects,
            "contract_version": DCR_CONTRACT_VERSION_V1,
            "evidence_digest": evidence_digest,
            "problem_digest": problem_digest,
            "verifier_identity_digest": verifier_identity_digest,
        }),
    ))
}

pub fn dcr_contract_manifest_v1() -> Value {
    json!({
        "canonical_encoding": "sorted_key_json_no_whitespace",
        "complete_authority": "opaque_verified_problem_plus_exact_finite_intersection",
        "conflict_hypergraph": "all_minimal_empty_intersections_over_accessible_baseline_dominant_effects",
        "contract_version": DCR_CONTRACT_VERSION_V1,
        "decision_outcomes": ["complete", "conflict", "unknown"],
        "dp": {
            "arithmetic": "checked_exact_reduced_u128_rational",
            "cost_encoding": "unsigned_decimal_strings",
            "objective": "minimum_expected_native_cost_with_fully_charged_raw_baseline_terminal",
            "state_bound": DCR_MAX_DP_STATES_V1,
            "tie_policy": "prefer_raw_baseline_then_lexicographically_first_query",
        },
        "finite_bounds": {
            "canonical_bytes": DCR_MAX_CANONICAL_BYTES_V1,
            "hypergraph_states": DCR_MAX_HYPERGRAPH_STATES_V1,
            "outcomes_per_query": DCR_MAX_OUTCOMES_PER_QUERY_V1,
            "queries": DCR_MAX_QUERIES_V1,
            "query_trace": DCR_MAX_QUERY_TRACE_V1,
            "worlds": DCR_MAX_WORLDS_V1,
        },
        "fiber_policy": {
            "exact": "eligible_for_complete",
            "sound_overapproximation": "eligible_for_complete_with_extra_recovery_allowed",
            "unknown": "raw_baseline_required",
            "underapproximation": "not_representable",
        },
        "proof_carrier": "zero_cert::VerifiedEvidence_successful_build_or_test_exact_problem_payload",
        "published_schema_sha256": DCR_SCHEMA_SHA256_V1,
        "schema_version": DCR_SCHEMA_VERSION_V1,
    })
}

pub fn dcr_contract_digest_v1() -> DigestV1 {
    digest_value(CONTRACT_DOMAIN_V1, &dcr_contract_manifest_v1())
}

fn canonical_bytes<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, DcrErrorV1> {
    let value = serde_json::to_value(value).map_err(|error| json_error(error.to_string()))?;
    let bytes = canonical_json(&value).into_bytes();
    if bytes.len() > DCR_MAX_CANONICAL_BYTES_V1 {
        return Err(dcr_error(
            DcrFailureCodeV1::CanonicalPayloadTooLarge,
            "DCR canonical payload exceeds its byte bound",
        ));
    }
    Ok(bytes)
}

fn decode_canonical<T>(bytes: &[u8]) -> Result<T, DcrErrorV1>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    if bytes.len() > DCR_MAX_CANONICAL_BYTES_V1 {
        return Err(dcr_error(
            DcrFailureCodeV1::CanonicalPayloadTooLarge,
            "DCR canonical payload exceeds its byte bound",
        ));
    }
    let decoded = serde_json::from_slice(bytes).map_err(|error| json_error(error.to_string()))?;
    if canonical_bytes(&decoded)? != bytes {
        return Err(dcr_error(
            DcrFailureCodeV1::NonCanonicalEncoding,
            "DCR bytes are not canonical sorted-key JSON",
        ));
    }
    Ok(decoded)
}

fn digest_value(domain: &[u8], value: &Value) -> DigestV1 {
    let canonical = canonical_json(value);
    let mut bytes = Vec::with_capacity(domain.len() + canonical.len());
    bytes.extend_from_slice(domain);
    bytes.extend_from_slice(canonical.as_bytes());
    DigestV1::from_bytes(sha256(&bytes))
}

fn domain_digest(domain: &[u8], bytes: &[u8]) -> DigestV1 {
    let mut bound = Vec::with_capacity(domain.len() + bytes.len());
    bound.extend_from_slice(domain);
    bound.extend_from_slice(bytes);
    DigestV1::from_bytes(sha256(&bound))
}

fn require_nonzero(label: &'static str, values: &[DigestV1]) -> Result<(), DcrErrorV1> {
    if values.iter().any(|value| *value == DigestV1::ZERO) {
        Err(dcr_error(
            DcrFailureCodeV1::ZeroDigest,
            format!("{label} contains a zero digest"),
        ))
    } else {
        Ok(())
    }
}

fn require_strict_order<T: Ord>(label: &'static str, values: &[T]) -> Result<(), DcrErrorV1> {
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        Err(dcr_error(
            DcrFailureCodeV1::NonCanonicalOrder,
            format!("{label} must be unique and strictly sorted"),
        ))
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DcrFailureCodeV1 {
    SchemaVersionMismatch,
    UnsupportedFiber,
    IdentityMismatch,
    BoundExceeded,
    AnalysisBoundExceeded,
    IncompleteCoverage,
    InvalidCostModel,
    InvalidQuery,
    InvalidQueryTrace,
    NonCanonicalOrder,
    ZeroDigest,
    CanonicalPayloadTooLarge,
    NonCanonicalEncoding,
    UnsupportedEvidenceClass,
    EvidencePayloadMismatch,
    VerifierIdentityMismatch,
    UnknownFiberCannotComplete,
    NotDominanceComplete,
    CertificateDigestMismatch,
    Json,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DcrErrorV1 {
    code: DcrFailureCodeV1,
    detail: String,
}

impl DcrErrorV1 {
    pub const fn failure_code(&self) -> DcrFailureCodeV1 {
        self.code
    }
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for DcrErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "DCR failed ({:?}): {}", self.code, self.detail)
    }
}

impl Error for DcrErrorV1 {}

fn dcr_error(code: DcrFailureCodeV1, detail: impl Into<String>) -> DcrErrorV1 {
    DcrErrorV1 {
        code,
        detail: detail.into(),
    }
}

fn json_error(detail: String) -> DcrErrorV1 {
    dcr_error(DcrFailureCodeV1::Json, detail)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;
    use zero_abi::robust_snap::ProtectedEffectClassV1;
    use zero_cert::{
        verify, EvidenceCertificate, ObjectId, OperatorLock, Provenance, Resolver, SpanRef, TestId,
    };

    fn d(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn effect(byte: u8) -> ProtectedEffectV1 {
        ProtectedEffectV1 {
            effect_digest: d(byte),
            effect_class: ProtectedEffectClassV1::ReversibleMutation,
        }
    }

    fn route() -> DigestV1 {
        digest_value(
            VERIFIER_DOMAIN_V1,
            &json!({
                "index_id": "dcr-index",
                "index_version": "1",
                "operator_id": "dcr-verifier",
                "operator_version": "1",
                "parser_id": "dcr-parser",
                "parser_version": "1",
            }),
        )
    }

    fn fiber() -> WorldFiberDescriptor {
        WorldFiberDescriptor {
            model_version: ROBUST_SNAP_MODEL_VERSION.into(),
            assembly_manifest_digest: d(40),
            source_image_digest: d(41),
            task_fingerprint: d(42),
            assumptions: vec!["finite exact test fiber".into()],
            worlds: vec![d(1), d(2), d(3)],
        }
    }

    fn query(id: u8, cost: u64, cells: Vec<(u8, Vec<DigestV1>)>) -> RecoveryQueryV1 {
        RecoveryQueryV1 {
            query_digest: d(id),
            native_cost_units: cost,
            evidence_route_digest: d(id + 1),
            outcomes: cells
                .into_iter()
                .map(|(outcome, worlds)| RecoveryQueryOutcomeV1 {
                    outcome_digest: d(outcome),
                    worlds,
                })
                .collect(),
        }
    }

    fn conflict_problem(
        status: SourceFiberStatusV1,
        query_cost: u64,
        raw_cost: u64,
    ) -> DominanceRecoveryProblemV1 {
        DominanceRecoveryProblemV1::new(
            d(50),
            d(42),
            fiber(),
            status,
            d(51),
            d(52),
            d(53),
            vec![
                ProtectedEffectSet {
                    world_id: d(1),
                    effects: vec![effect(10), effect(11)],
                },
                ProtectedEffectSet {
                    world_id: d(2),
                    effects: vec![effect(11), effect(12)],
                },
                ProtectedEffectSet {
                    world_id: d(3),
                    effects: vec![effect(10), effect(12)],
                },
            ],
            vec![effect(10), effect(11), effect(12)],
            vec![
                WorldRecoveryBudgetV1 {
                    world_id: d(1),
                    probability_weight: 1,
                    raw_baseline_cost_units: raw_cost,
                },
                WorldRecoveryBudgetV1 {
                    world_id: d(2),
                    probability_weight: 1,
                    raw_baseline_cost_units: raw_cost,
                },
                WorldRecoveryBudgetV1 {
                    world_id: d(3),
                    probability_weight: 1,
                    raw_baseline_cost_units: raw_cost,
                },
            ],
            vec![
                query(
                    60,
                    query_cost,
                    vec![(61, vec![d(1)]), (62, vec![d(2), d(3)])],
                ),
                query(
                    70,
                    query_cost + 2,
                    vec![(71, vec![d(1), d(2)]), (72, vec![d(3)])],
                ),
            ],
            d(54),
            Vec::new(),
            route(),
            d(55),
        )
        .unwrap()
    }

    fn complete_problem(status: SourceFiberStatusV1) -> DominanceRecoveryProblemV1 {
        let mut problem = conflict_problem(status, 5, 100);
        for protected in &mut problem.protected_effects {
            protected.effects = vec![effect(10), effect(11), effect(12)];
        }
        problem.queries.clear();
        problem.validate().unwrap();
        problem
    }

    struct TestResolver {
        bytes: Vec<u8>,
    }

    impl Resolver for TestResolver {
        fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]> {
            (object_id.0 == sha256(&self.bytes)).then_some(self.bytes.as_slice())
        }
        fn trusted_operator_version<'a>(&'a self, operator_id: &str) -> Option<&'a str> {
            (operator_id == "dcr-verifier").then_some("1")
        }
        fn trusted_parser_version<'a>(&'a self, parser_id: &str) -> Option<&'a str> {
            (parser_id == "dcr-parser").then_some("1")
        }
        fn trusted_index_version<'a>(&'a self, index_id: &str) -> Option<&'a str> {
            (index_id == "dcr-index").then_some("1")
        }
    }

    fn test_certificate(
        problem: &DominanceRecoveryProblemV1,
    ) -> (EvidenceCertificate<'static>, TestResolver) {
        let bytes = problem.canonical_bytes().unwrap();
        let digest = sha256(&bytes);
        let span = SpanRef {
            object_id: ObjectId(digest),
            object_digest: digest,
            byte_start: 0,
            byte_len: bytes.len() as u64,
            span_digest: digest,
        };
        (
            EvidenceCertificate {
                query: Query::TestTrace { test: TestId(7) },
                spans: vec![span],
                payload: Cow::Owned(bytes.clone()),
                provenance: Provenance {
                    parser_id: "dcr-parser".into(),
                    parser_version: "1".into(),
                    index_id: "dcr-index".into(),
                    index_version: "1".into(),
                    operator_id: "dcr-verifier".into(),
                    operator_version: "1".into(),
                },
                completeness: CompletenessWitness::TestTrace {
                    operator: OperatorLock {
                        operator_id: "dcr-verifier".into(),
                        operator_version: "1".into(),
                    },
                    test: TestId(7),
                    exit_code: 0,
                    trace_digest: digest,
                },
                input_token_cost: 0,
                backend_work_units: 1,
            },
            TestResolver { bytes },
        )
    }

    fn recover(problem: DominanceRecoveryProblemV1) -> RecoveryDecisionV1 {
        let (certificate, resolver) = test_certificate(&problem);
        let evidence = verify(&certificate, &resolver).unwrap();
        dominance_complete_recover_v1(problem, &evidence).unwrap()
    }

    fn hex(digest: DigestV1) -> String {
        digest.to_hex()
    }

    #[test]
    fn contract_digest_is_stable() {
        assert_eq!(
            hex(dcr_contract_digest_v1()),
            "a0a7a6951757472cdd1c5730ada5901a8c0f9a982c26b88338941dc34f5cf967"
        );
        assert_eq!(
            DigestV1::from_bytes(sha256(include_bytes!(
                "../../../conformance/schemas/dominance-complete-recovery-v1.schema.json"
            )))
            .to_hex(),
            DCR_SCHEMA_SHA256_V1
        );
        assert_eq!(
            serde_json::to_value(ExactRecoveryCostV1 {
                numerator: u128::MAX,
                denominator: u128::MAX - 1,
            })
            .unwrap(),
            json!({
                "denominator": (u128::MAX - 1).to_string(),
                "numerator": u128::MAX.to_string(),
            })
        );
    }

    #[test]
    fn exact_and_sound_overapproximation_mint_opaque_complete_authority() {
        for status in [
            SourceFiberStatusV1::Exact,
            SourceFiberStatusV1::SoundOverapproximation,
        ] {
            let RecoveryDecisionV1::Complete(certificate) = recover(complete_problem(status))
            else {
                panic!("complete finite intersection must mint DCR authority");
            };
            certificate.validate().unwrap();
            assert_eq!(
                certificate.common_effects(),
                &[effect(10), effect(11), effect(12)]
            );
            let record = certificate.record();
            let bytes = record.canonical_bytes().unwrap();
            assert_eq!(
                DominanceCompleteRecoveryCertificateRecordV1::from_canonical_bytes(&bytes).unwrap(),
                record
            );
        }
    }

    #[test]
    fn three_way_conflict_is_not_laundered_into_pairwise_completeness() {
        let RecoveryDecisionV1::Conflict(decision) =
            recover(conflict_problem(SourceFiberStatusV1::Exact, 5, 100))
        else {
            panic!("unresolved three-world ambiguity must trigger a query");
        };
        assert_eq!(decision.selected_query().query_digest, d(60));
        assert_eq!(decision.optimal_expected_cost().numerator(), 5);
        assert_eq!(decision.optimal_expected_cost().denominator(), 1);
        assert_eq!(decision.conflict_hyperedges().len(), 1);
        assert_eq!(
            decision.conflict_hyperedges()[0].worlds(),
            &[d(1), d(2), d(3)]
        );
        for pair in [[d(1), d(2)], [d(1), d(3)], [d(2), d(3)]] {
            assert!(!decision
                .conflict_hyperedges()
                .iter()
                .any(|edge| edge.worlds() == pair));
        }
    }

    #[test]
    fn oversized_exact_analysis_deoptimizes_instead_of_hanging_or_truncating() {
        let worlds = (1..=17).map(d).collect::<Vec<_>>();
        let protected_effects = worlds
            .iter()
            .enumerate()
            .map(|(index, world)| ProtectedEffectSet {
                world_id: *world,
                effects: vec![effect((index + 20) as u8)],
            })
            .collect::<Vec<_>>();
        let accessible = (20..=36).map(effect).collect::<Vec<_>>();
        let world_budgets = worlds
            .iter()
            .map(|world| WorldRecoveryBudgetV1 {
                world_id: *world,
                probability_weight: 1,
                raw_baseline_cost_units: 100,
            })
            .collect::<Vec<_>>();
        let problem = DominanceRecoveryProblemV1::new(
            d(50),
            d(42),
            WorldFiberDescriptor {
                model_version: ROBUST_SNAP_MODEL_VERSION.into(),
                assembly_manifest_digest: d(40),
                source_image_digest: d(41),
                task_fingerprint: d(42),
                assumptions: vec!["bounded large fiber".into()],
                worlds,
            },
            SourceFiberStatusV1::Exact,
            d(51),
            d(52),
            d(53),
            protected_effects,
            accessible,
            world_budgets,
            Vec::new(),
            d(54),
            Vec::new(),
            route(),
            d(55),
        )
        .unwrap();
        let RecoveryDecisionV1::Unknown(unknown) = recover(problem) else {
            panic!("bounded analysis exhaustion must deoptimize");
        };
        assert_eq!(
            unknown.reason(),
            RecoveryUnknownReasonV1::AnalysisBoundExceeded
        );
        assert!(unknown.raw_baseline_required());
    }

    #[test]
    fn unknown_fiber_and_nonbeneficial_query_require_raw_baseline() {
        let RecoveryDecisionV1::Unknown(unknown) =
            recover(conflict_problem(SourceFiberStatusV1::Unknown, 1, 100))
        else {
            panic!("unknown fiber must not complete");
        };
        assert_eq!(unknown.reason(), RecoveryUnknownReasonV1::FiberUnknown);
        assert!(unknown.raw_baseline_required());

        let RecoveryDecisionV1::Unknown(unknown) =
            recover(conflict_problem(SourceFiberStatusV1::Exact, 100, 5))
        else {
            panic!("raw baseline must win an equal-or-cheaper exact DP comparison");
        };
        assert_eq!(
            unknown.reason(),
            RecoveryUnknownReasonV1::RawBaselineCheaperOrEqual
        );
        assert!(unknown.raw_baseline_required());
    }

    #[test]
    fn exact_query_conditioning_reaches_complete_without_full_reconstruction() {
        let problem = conflict_problem(SourceFiberStatusV1::Exact, 5, 100);
        let RecoveryDecisionV1::Conflict(decision) = recover(problem.clone()) else {
            panic!("initial ambiguity must query");
        };
        let conditioned = problem
            .condition_on(
                decision.selected_query().query_digest,
                d(62),
                d(80),
                d(81),
                vec![effect(10), effect(11), effect(12)],
                d(82),
                route(),
                d(83),
            )
            .unwrap();
        assert_eq!(conditioned.fiber.worlds, vec![d(2), d(3)]);
        let RecoveryDecisionV1::Complete(certificate) = recover(conditioned) else {
            panic!("one deciding observation must reach a complete leaf");
        };
        assert_eq!(certificate.common_effects(), &[effect(12)]);
        assert_eq!(certificate.claim.fallback_safepoint, d(83));
        assert_eq!(certificate.claim.decision_view_digest, d(81));
        assert_eq!(certificate.claim.recovery_query_trace.len(), 1);
    }

    #[test]
    fn invalid_query_partition_and_tampered_record_fail_closed() {
        let mut invalid = conflict_problem(SourceFiberStatusV1::Exact, 5, 100);
        invalid.queries[0].outcomes[1].worlds.pop();
        assert_eq!(
            invalid.validate().unwrap_err().failure_code(),
            DcrFailureCodeV1::InvalidQuery
        );

        let RecoveryDecisionV1::Complete(certificate) =
            recover(complete_problem(SourceFiberStatusV1::Exact))
        else {
            panic!("complete fixture must mint authority");
        };
        let mut record = certificate.record();
        record.common_effects.pop();
        assert_eq!(
            record.validate().unwrap_err().failure_code(),
            DcrFailureCodeV1::CertificateDigestMismatch
        );
    }

    #[test]
    fn claim_shape_matches_external_schema_and_canonical_bytes_are_strict() {
        let RecoveryDecisionV1::Complete(certificate) =
            recover(complete_problem(SourceFiberStatusV1::Exact))
        else {
            panic!("complete fixture must mint authority");
        };
        let claim = certificate.claim();
        let object = serde_json::to_value(claim)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let expected = [
            "accessible_effect_surface_digest",
            "baseline_identity",
            "common_baseline_dominant_effect_class",
            "conflict_hyperedges",
            "coverage_certificate",
            "decision_view_digest",
            "fallback_safepoint",
            "fiber_status",
            "project_root",
            "reasoning_contract_digest",
            "recovery_query_trace",
            "schema_version",
            "task_identity",
            "verifier_route",
            "world_fiber_digest",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
        assert_eq!(object, expected);
        let value = serde_json::to_value(claim).unwrap();
        assert_eq!(value["fiber_status"], "exact");
        assert_eq!(
            value["common_baseline_dominant_effect_class"]
                .as_str()
                .unwrap()
                .len(),
            64
        );
        let mut bytes = claim.canonical_bytes().unwrap();
        bytes.push(b'\n');
        assert_eq!(
            DominanceCompleteRecoveryClaimV1::from_canonical_bytes(&bytes)
                .unwrap_err()
                .failure_code(),
            DcrFailureCodeV1::NonCanonicalEncoding
        );
    }
}
