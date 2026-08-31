//! Q99 causal-cache runtime.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use zero_abi::{Sha256Digest, canonical_json};

pub const RESIDENCY_CONTRACT_VERSION: u16 = 1;
/// Q99 guarantee: at most 1% of demanded knowledge is recomputed.
pub const Q99_RECOMPUTE_FRACTION_PPM: u64 = 10_000;
/// Slack floor: resident valid mass must stay at or above 99% of demanded.
pub const SLACK_RESIDENT_FRACTION_PPM: u64 = 990_000;
/// Central change threshold: >1% of demanded mass invalidated in a window
/// makes Q99 unavailable (impossibility reported, not averaged).
pub const Q99_CENTRAL_CHANGE_FRACTION_PPM: u64 = 10_000;

/// Fail-closed error for the Q99 runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResidencyError {
    InvalidDemandLedger(String),
    InvalidPlan(String),
    PlanRejected(String),
    SlackExceeded {
        resident_mass: u64,
        demanded_mass: u64,
        slack: i64,
    },
    RestorationRequired {
        valid_mass: u64,
        threshold: u64,
    },
    Q99Unavailable(String),
    InvalidLayerLedger(String),
    L3LossUndiscovered(String),
}

impl fmt::Display for ResidencyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDemandLedger(detail) => {
                write!(formatter, "invalid demand ledger: {detail}")
            }
            Self::InvalidPlan(detail) => write!(formatter, "invalid residency plan: {detail}"),
            Self::PlanRejected(detail) => write!(formatter, "residency plan rejected: {detail}"),
            Self::SlackExceeded {
                resident_mass,
                demanded_mass,
                slack,
            } => write!(
                formatter,
                "eviction slack exceeded: resident {resident_mass} vs demanded {demanded_mass}, slack {slack}"
            ),
            Self::RestorationRequired {
                valid_mass,
                threshold,
            } => write!(
                formatter,
                "valid mass {valid_mass} below restoration threshold {threshold}; Q99 unavailable until restored"
            ),
            Self::Q99Unavailable(detail) => write!(formatter, "Q99 unavailable: {detail}"),
            Self::InvalidLayerLedger(detail) => {
                write!(formatter, "invalid layer ledger: {detail}")
            }
            Self::L3LossUndiscovered(detail) => {
                write!(formatter, "undiscovered L3 loss: {detail}")
            }
        }
    }
}

impl Error for ResidencyError {}

// Demand-weight ledger.

/// The three cache layers. L3 is the provider/cold layer; losing L3 must not
/// destroy L2 validity (project amnesia).
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheLayerTier {
    L1,
    L2,
    L3,
}

impl CacheLayerTier {
    pub fn as_str(self) -> &'static str {
        match self {
            CacheLayerTier::L1 => "l1",
            CacheLayerTier::L2 => "l2",
            CacheLayerTier::L3 => "l3",
        }
    }
}

/// One demanded object: complete causal coordinate, declared demand weight,
/// window and tier attribution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DemandWeightedObject {
    pub object_root: Sha256Digest,
    pub demand_weight: u64,
    pub window_id: String,
    pub tier: CacheLayerTier,
}

impl DemandWeightedObject {
    pub fn new(
        object_root: Sha256Digest,
        demand_weight: u64,
        window_id: impl Into<String>,
        tier: CacheLayerTier,
    ) -> Result<Self, ResidencyError> {
        let object = Self {
            object_root,
            demand_weight,
            window_id: window_id.into(),
            tier,
        };
        object.validate()?;
        Ok(object)
    }

    pub fn validate(&self) -> Result<(), ResidencyError> {
        if self.window_id.is_empty() {
            return Err(ResidencyError::InvalidDemandLedger(
                "window_id must be nonempty".into(),
            ));
        }
        if self.demand_weight == 0 {
            return Err(ResidencyError::InvalidDemandLedger(
                "demand_weight must be nonzero".into(),
            ));
        }
        Ok(())
    }
}

/// Declared demand weights per window and tier. The report is rejected when
/// weights are absent or inconsistent with the window totals.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DemandWeightLedger {
    pub objects: Vec<DemandWeightedObject>,
}

impl DemandWeightLedger {
    pub fn new(objects: Vec<DemandWeightedObject>) -> Result<Self, ResidencyError> {
        let ledger = Self { objects };
        ledger.validate()?;
        Ok(ledger)
    }

    pub fn validate(&self) -> Result<(), ResidencyError> {
        let mut seen = std::collections::BTreeSet::new();
        for object in &self.objects {
            object.validate()?;
            if !seen.insert((object.object_root, object.window_id.clone())) {
                return Err(ResidencyError::InvalidDemandLedger(format!(
                    "duplicate demand declaration for object {} in window {}",
                    object.object_root, object.window_id
                )));
            }
        }
        Ok(())
    }

    /// Demanded mass of one window, over all tiers.
    pub fn window_mass(&self, window_id: &str) -> u64 {
        self.objects
            .iter()
            .filter(|object| object.window_id == window_id)
            .fold(0_u64, |mass, object| {
                mass.saturating_add(object.demand_weight)
            })
    }

    /// Demanded mass of one window at one tier.
    pub fn tier_mass(&self, window_id: &str, tier: CacheLayerTier) -> u64 {
        self.objects
            .iter()
            .filter(|object| object.window_id == window_id && object.tier == tier)
            .fold(0_u64, |mass, object| {
                mass.saturating_add(object.demand_weight)
            })
    }

    /// All declared window ids, sorted.
    pub fn windows(&self) -> Vec<String> {
        let windows: std::collections::BTreeSet<String> = self
            .objects
            .iter()
            .map(|object| object.window_id.clone())
            .collect();
        windows.into_iter().collect()
    }
}

// Sliding-window Q99 + restoration threshold.

/// One observed demand event in the window.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DemandObservation {
    pub demanded_mass: u64,
    pub hit: bool,
}

impl DemandObservation {
    pub fn new(demanded_mass: u64, hit: bool) -> Result<Self, ResidencyError> {
        let observation = Self { demanded_mass, hit };
        if demanded_mass == 0 {
            return Err(ResidencyError::InvalidDemandLedger(
                "demanded_mass must be nonzero".into(),
            ));
        }
        Ok(observation)
    }
}

/// Sliding-window Q99 report: hit rate over demanded mass, with impossibility
/// reported as `unavailable` instead of averaged away.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Q99WindowReport {
    pub window_id: String,
    pub demanded_mass: u64,
    pub hit_mass: u64,
    /// PPM of demanded mass recomputed in this window (1_000_000 = 100%).
    pub recompute_ppm: u64,
    /// Q99 unavailable: central change exceeded 1% of demanded mass, or the
    /// restoration threshold was breached. Impossible states are reported,
    /// never averaged away.
    pub unavailable: bool,
    pub reasons: Vec<String>,
}

/// Sliding window of demand observations with Q99 accounting.
#[derive(Clone, Debug)]
pub struct Q99Window {
    window_id: String,
    observations: Vec<DemandObservation>,
    initial_valid_mass: u64,
}

impl Q99Window {
    pub fn new(window_id: impl Into<String>, initial_valid_mass: u64) -> Self {
        Self {
            window_id: window_id.into(),
            observations: Vec::new(),
            initial_valid_mass,
        }
    }

    pub fn window_id(&self) -> &str {
        &self.window_id
    }

    /// Observe one demand event (hit or miss) with its demanded mass.
    pub fn observe(&mut self, observation: DemandObservation) {
        self.observations.push(observation);
    }

    /// Total demanded mass in the window.
    pub fn demanded_mass(&self) -> u64 {
        self.observations.iter().fold(0_u64, |mass, observation| {
            mass.saturating_add(observation.demanded_mass)
        })
    }

    /// Mass of observations that hit.
    pub fn hit_mass(&self) -> u64 {
        self.observations
            .iter()
            .filter(|observation| observation.hit)
            .fold(0_u64, |mass, observation| {
                mass.saturating_add(observation.demanded_mass)
            })
    }

    /// Recomputed (missed) mass as PPM of demanded mass. Empty windows are
    /// unavailable (no evidence), never vacuously passing.
    pub fn recompute_ppm(&self) -> u64 {
        let demanded = self.demanded_mass();
        if demanded == 0 {
            return u64::MAX;
        }
        let missed = demanded.saturating_sub(self.hit_mass());
        ppm_of(missed, demanded)
    }

    /// The restoration threshold: `max(0, I0 - 0.01W)`. Valid mass must stay
    /// at or above this value; below it, Q99 is unavailable until restored.
    pub fn restoration_threshold(&self) -> u64 {
        let demanded = self.demanded_mass();
        let erosion = demanded / 100;
        self.initial_valid_mass.saturating_sub(erosion)
    }

    /// Whether the window's Q99 is unavailable. Impossibility events are
    /// reported, never averaged: central change exceeding 1% of demanded
    /// mass, or valid mass below the restoration threshold.
    pub fn q99_unavailable(&self, current_valid_mass: u64) -> Option<Vec<String>> {
        let mut reasons = Vec::new();
        let demanded = self.demanded_mass();
        if demanded == 0 {
            reasons.push("no_demand_observations".into());
        }
        // Central change >1% of demanded mass: recompute_ppm > 1%.
        if self.recompute_ppm() > Q99_CENTRAL_CHANGE_FRACTION_PPM {
            reasons.push(format!(
                "central_change_exceeds_one_percent:recompute_ppm={}",
                self.recompute_ppm()
            ));
        }
        let threshold = self.restoration_threshold();
        if current_valid_mass < threshold {
            reasons.push(format!(
                "restoration_required:valid_mass={current_valid_mass},threshold={threshold}"
            ));
        }
        if reasons.is_empty() {
            None
        } else {
            Some(reasons)
        }
    }

    /// The report. `unavailable` is a first-class state, never an average.
    pub fn report(&self, current_valid_mass: u64) -> Q99WindowReport {
        let unavailable = self.q99_unavailable(current_valid_mass);
        Q99WindowReport {
            window_id: self.window_id.clone(),
            demanded_mass: self.demanded_mass(),
            hit_mass: self.hit_mass(),
            recompute_ppm: self.recompute_ppm(),
            unavailable: unavailable.is_some(),
            reasons: unavailable.unwrap_or_default(),
        }
    }
}

/// One object in a residency plan.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResidencyPlanObject {
    pub object_root: String,
    pub size_bytes: u64,
    pub demand_weight: u64,
    pub valid: bool,
    pub resident: bool,
}

impl ResidencyPlanObject {
    pub fn new(
        object_root: impl Into<String>,
        size_bytes: u64,
        demand_weight: u64,
        valid: bool,
        resident: bool,
    ) -> Result<Self, ResidencyError> {
        let object = Self {
            object_root: object_root.into(),
            size_bytes,
            demand_weight,
            valid,
            resident,
        };
        if object.object_root.is_empty() {
            return Err(ResidencyError::InvalidPlan(
                "object_root must be nonempty".into(),
            ));
        }
        Ok(object)
    }
}

/// Optimizer proposal: which objects to keep resident at which tier.
/// Mirrors `causal_residency_plan.schema.json` exactly.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResidencyPlan {
    pub tier: String,
    pub capacity_bytes: u64,
    pub threshold: f64,
    pub demand_window_root: String,
    pub objects: Vec<ResidencyPlanObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optimizer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_root: Option<String>,
}

impl ResidencyPlan {
    pub fn validate(&self) -> Result<(), ResidencyError> {
        if self.tier.is_empty() {
            return Err(ResidencyError::InvalidPlan("tier must be nonempty".into()));
        }
        if self.demand_window_root.is_empty() {
            return Err(ResidencyError::InvalidPlan(
                "demand_window_root must be nonempty".into(),
            ));
        }
        if !(0.0..=1.0).contains(&self.threshold) {
            return Err(ResidencyError::InvalidPlan(
                "threshold must be within [0, 1]".into(),
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for object in &self.objects {
            if !seen.insert(object.object_root.clone()) {
                return Err(ResidencyError::InvalidPlan(format!(
                    "duplicate object {}",
                    object.object_root
                )));
            }
        }
        Ok(())
    }

    /// Bytes consumed by resident objects.
    pub fn resident_bytes(&self) -> u64 {
        self.objects
            .iter()
            .filter(|object| object.resident)
            .fold(0_u64, |bytes, object| {
                bytes.saturating_add(object.size_bytes)
            })
    }

    /// Demand weight of resident + valid objects.
    pub fn resident_valid_weight(&self) -> u64 {
        self.objects
            .iter()
            .filter(|object| object.resident && object.valid)
            .fold(0_u64, |mass, object| {
                mass.saturating_add(object.demand_weight)
            })
    }

    /// Total demanded weight in the plan (all objects, resident or not).
    pub fn total_demand_weight(&self) -> u64 {
        self.objects.iter().fold(0_u64, |mass, object| {
            mass.saturating_add(object.demand_weight)
        })
    }

    /// The optimizer's own root claim (the plan root) must equal the
    /// canonical digest of the plan's content when the optimizer supplied
    /// one; a mismatched proposal root fails closed.
    pub fn proposal_root_matches(&self) -> Result<(), ResidencyError> {
        let Some(claimed) = self.proposal_root.as_deref() else {
            return Ok(());
        };
        let mut value = serde_json::to_value(self)
            .map_err(|error| ResidencyError::InvalidPlan(error.to_string()))?;
        value
            .as_object_mut()
            .map(|object| object.remove("proposal_root"));
        let digest = zero_abi::sha256_hex(canonical_json(&value).as_bytes());
        if digest != claimed {
            return Err(ResidencyError::InvalidPlan(
                "proposal_root does not match plan content".into(),
            ));
        }
        Ok(())
    }
}

/// Independent capacity + threshold checker. The optimizer proposes; the
/// checker authorizes. A plan just below the threshold fails, just above
/// passes (adversarial pair).
#[derive(Clone, Copy, Debug)]
pub struct ResidencyThresholdChecker;

impl ResidencyThresholdChecker {
    /// Authorize a plan: resident valid weight must cover at least
    /// `threshold` of total demanded weight, and resident bytes must fit
    /// capacity. Both conditions must hold; anything else fails closed.
    pub fn authorize(plan: &ResidencyPlan) -> Result<(), ResidencyError> {
        plan.validate()?;
        plan.proposal_root_matches()?;
        if plan.resident_bytes() > plan.capacity_bytes {
            return Err(ResidencyError::PlanRejected(format!(
                "resident bytes {} exceed capacity {}",
                plan.resident_bytes(),
                plan.capacity_bytes
            )));
        }
        let demanded = plan.total_demand_weight();
        if demanded == 0 {
            return Err(ResidencyError::PlanRejected(
                "plan declares no demanded weight".into(),
            ));
        }
        let resident = plan.resident_valid_weight();
        let covered = ppm_of(resident, demanded);
        let required_ppm = (plan.threshold * 1_000_000.0) as u64;
        if covered < required_ppm {
            return Err(ResidencyError::PlanRejected(format!(
                "resident valid weight {resident} covers {covered}ppm of demanded {demanded}, below required {required_ppm}ppm"
            )));
        }
        Ok(())
    }
}

// Eviction slack guard.

/// Eviction slack: `sigma = W_R - 0.99W`. An eviction that would push
/// resident mass below 99% of demanded mass is rejected.
#[derive(Clone, Copy, Debug)]
pub struct EvictionSlack {
    resident_mass: u64,
    demanded_mass: u64,
}

impl EvictionSlack {
    pub fn new(resident_mass: u64, demanded_mass: u64) -> Result<Self, ResidencyError> {
        if demanded_mass == 0 {
            return Err(ResidencyError::InvalidDemandLedger(
                "demanded_mass must be nonzero".into(),
            ));
        }
        Ok(Self {
            resident_mass,
            demanded_mass,
        })
    }

    /// `sigma = W_R - 0.99W` in PPM of demanded mass (can be negative).
    pub fn slack_ppm(&self) -> i64 {
        let floor = ppm_of(self.demanded_mass * 99 / 100, self.demanded_mass);
        // resident_mass as ppm of demanded, minus the 99% floor.
        let resident_ppm = ppm_of(self.resident_mass, self.demanded_mass);
        resident_ppm as i64 - floor as i64
    }

    /// Guard one eviction decision: evicting `evict_weight` must keep
    /// resident mass at or above 99% of demanded mass.
    pub fn guard_eviction(&self, evict_weight: u64) -> Result<(), ResidencyError> {
        let floor = self.demanded_mass * 99 / 100;
        let after = self.resident_mass.saturating_sub(evict_weight);
        if after < floor {
            return Err(ResidencyError::SlackExceeded {
                resident_mass: self.resident_mass,
                demanded_mass: self.demanded_mass,
                slack: self.slack_ppm(),
            });
        }
        Ok(())
    }
}

// L1/L2/L3 layer validity.

/// One cache entry's per-layer validity. L3 loss (provider miss) preserves
/// the entry's causal identity and L2 validity; recovery is
/// fetch/rematerialize, never rediscovery.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LayerValidityEntry {
    pub object_root: Sha256Digest,
    pub l1_valid: bool,
    pub l2_valid: bool,
    pub l3_valid: bool,
    /// When L3 is lost, the L2 copy must be refetched/rematerialized before
    /// use; the causal identity is never re-derived (no rediscovery).
    pub l2_needs_refetch: bool,
}

impl LayerValidityEntry {
    pub fn new(object_root: Sha256Digest) -> Self {
        Self {
            object_root,
            l1_valid: false,
            l2_valid: false,
            l3_valid: false,
            l2_needs_refetch: false,
        }
    }

    pub fn validate(&self) -> Result<(), ResidencyError> {
        if self.l2_needs_refetch && !self.l2_valid {
            return Err(ResidencyError::InvalidLayerLedger(
                "an L2 copy marked needs-refetch must be L2-valid (identity kept)".into(),
            ));
        }
        Ok(())
    }
}

/// Layer validity ledger. The law: a provider (L3) miss never becomes project
/// amnesia -- L2 validity records survive, marked for refetch, and tombstones
/// never delete them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LayerValidityLedger {
    entries: std::collections::BTreeMap<Sha256Digest, LayerValidityEntry>,
}

impl LayerValidityLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn entry(&self, object_root: Sha256Digest) -> Option<&LayerValidityEntry> {
        self.entries.get(&object_root)
    }

    pub fn entries(&self) -> impl Iterator<Item = &LayerValidityEntry> {
        self.entries.values()
    }

    /// Publish a verified L2 copy (identity proven by content, not
    /// rediscovery).
    pub fn publish_l2(&mut self, object_root: Sha256Digest) -> Result<(), ResidencyError> {
        let entry = self
            .entries
            .entry(object_root)
            .or_insert_with(|| LayerValidityEntry::new(object_root));
        entry.l2_valid = true;
        entry.l2_needs_refetch = false;
        entry.validate()
    }

    /// Declare an L3 loss for a set of entries. L2 validity is PRESERVED and marked
    /// for refetch/rematerialization; the causal identity is never re-derived. An
    /// entry that was never L2-valid fails closed (there is nothing to preserve).
    pub fn mark_l3_loss(&mut self, object_root: Sha256Digest) -> Result<(), ResidencyError> {
        let entry = self.entries.get_mut(&object_root).ok_or_else(|| {
            ResidencyError::L3LossUndiscovered(
                "L3 loss declared for an entry with no L2 validity record".into(),
            )
        })?;
        if !entry.l2_valid {
            return Err(ResidencyError::L3LossUndiscovered(format!(
                "entry {} has no L2 validity to preserve",
                entry.object_root
            )));
        }
        entry.l3_valid = false;
        entry.l2_needs_refetch = true;
        entry.validate()
    }

    /// Complete a refetch: the L2 copy is byte-identical again; the causal
    /// identity was never re-derived.
    pub fn complete_refetch(&mut self, object_root: Sha256Digest) -> Result<(), ResidencyError> {
        let entry = self.entries.get_mut(&object_root).ok_or_else(|| {
            ResidencyError::InvalidLayerLedger("refetch for unknown entry".into())
        })?;
        if !entry.l2_needs_refetch {
            return Err(ResidencyError::InvalidLayerLedger(
                "refetch completed for an entry not marked needs-refetch".into(),
            ));
        }
        entry.l2_needs_refetch = false;
        entry.l3_valid = true;
        entry.validate()
    }

    /// Tombstone: the entry is evicted but its L2 validity record is NEVER
    /// deleted (no project amnesia); it remains for causal accounting.
    pub fn tombstone(&mut self, object_root: Sha256Digest) -> Result<(), ResidencyError> {
        let entry = self.entries.get_mut(&object_root).ok_or_else(|| {
            ResidencyError::InvalidLayerLedger("tombstone for unknown entry".into())
        })?;
        entry.l1_valid = false;
        entry.l2_valid = false;
        entry.validate()
    }
}

/// PPM helper: `numerator / denominator * 1_000_000`, saturating.
fn ppm_of(numerator: u64, denominator: u64) -> u64 {
    if denominator == 0 {
        return u64::MAX;
    }
    let scaled = numerator.saturating_mul(1_000_000);
    scaled / denominator
}
