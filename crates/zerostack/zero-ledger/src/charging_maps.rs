//! ZS-METRIC-007: certified lower-bound ledger of disjoint charging maps.
//!
//! Six typed phases must carry every unavoidable cost claim: request
//! information, decisions, reasoning, verification, output, and effects. Each
//! phase has one [`ChargingMap`] whose entries are measured work units; the
//! map total is derived from its entries (checked integer sum, honest
//! exactness label), never caller-supplied.
//!
//! Invariants enforced here:
//!
//! - **Conservation within a map.** The map total is the checked sum of its
//!   entries by construction; a wire map is re-validated on decode.
//! - **Disjointness across maps.** [`ChargingMapSet::check_overlap`] rejects
//!   any work unit charged in two phases: double counting is a typed error,
//!   never silently merged.
//! - **Closure Gamma <= 1.** [`ChargingMapSet::check_closure`] compares the
//!   total attributed against a measured total. Attributed above measured is
//!   a non-conservation refusal; attributed below measured leaves the
//!   unclaimed residue *reported, not guessed* (nothing is split into a
//!   phase without evidence). Gamma = attributed / measured never exceeds 1
//!   under valid data, and the honest full-coverage endpoint is Gamma = 1.
//! - **Deterministic solving from measured receipts.** [`ChargingMapSet::solve`]
//!   groups [`CausalWorkReceipt`] charges into phases through an explicit
//!   total [`PhasePolicy`]. A receipt that fails validation, an empty receipt
//!   set, a work unit charged twice across receipts, or a policy that does
//!   not cover every causal class is a loud refusal -- the solver never
//!   guesses a split.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize, de};

use crate::causal_work::{
    CausalWorkClass, CausalWorkError, CausalWorkReceipt, CAUSAL_WORK_MAX_ID_BYTES,
};
use crate::resource_classes::{MeasurementSource, ResourceTotal};

/// The six phases of the certified lower-bound composition.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChargingPhase {
    /// Unavoidable request information (the request itself).
    RequestInfo,
    /// Unavoidable decisions.
    Decisions,
    /// Unavoidable reasoning.
    Reasoning,
    /// Unavoidable verification.
    Verification,
    /// Unavoidable visible output.
    Output,
    /// Unavoidable external effects.
    Effects,
}

impl ChargingPhase {
    /// Every phase, in canonical order.
    pub const ALL: [Self; 6] = [
        Self::RequestInfo,
        Self::Decisions,
        Self::Reasoning,
        Self::Verification,
        Self::Output,
        Self::Effects,
    ];

    /// Canonical lowercase phase string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RequestInfo => "request_info",
            Self::Decisions => "decisions",
            Self::Reasoning => "reasoning",
            Self::Verification => "verification",
            Self::Output => "output",
            Self::Effects => "effects",
        }
    }
}

impl fmt::Display for ChargingPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One measured work unit charged to one phase.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ChargingEntry {
    /// Identity of the measured work unit.
    pub work_unit_id: String,
    /// Measured amount charged.
    pub amount: u64,
    /// How the amount was obtained.
    pub source: MeasurementSource,
}

/// One phase's charging map.
///
/// Entries are kept sorted by work-unit id (canonical, deterministic), the
/// total is the checked entry sum, and the source label is derived. Wire
/// decoding re-validates every invariant.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ChargingMap {
    phase: ChargingPhase,
    entries: Vec<ChargingEntry>,
    total: u64,
    source: MeasurementSource,
}

impl ChargingMap {
    /// Builds one phase map.
    ///
    /// Refusals: an empty work-unit id, an id longer than
    /// [`CAUSAL_WORK_MAX_ID_BYTES`], a zero amount, a duplicate id within the
    /// map, and checked-sum overflow.
    pub fn build(phase: ChargingPhase, entries: Vec<ChargingEntry>) -> Result<Self, ChargingMapError> {
        let mut sorted = entries;
        for entry in &sorted {
            if entry.work_unit_id.is_empty() {
                return Err(ChargingMapError::EmptyWorkUnitId);
            }
            if entry.work_unit_id.len() > CAUSAL_WORK_MAX_ID_BYTES {
                return Err(ChargingMapError::WorkUnitIdTooLong {
                    len: entry.work_unit_id.len(),
                });
            }
            if entry.amount == 0 {
                return Err(ChargingMapError::ZeroAmount {
                    work_unit_id: entry.work_unit_id.clone(),
                });
            }
        }
        sorted.sort();
        for pair in sorted.windows(2) {
            if pair[0].work_unit_id == pair[1].work_unit_id {
                return Err(ChargingMapError::DuplicateWorkUnit {
                    work_unit_id: pair[0].work_unit_id.clone(),
                });
            }
        }
        let mut total = 0u64;
        for entry in &sorted {
            total = total
                .checked_add(entry.amount)
                .ok_or(ChargingMapError::CounterOverflow)?;
        }
        let source = MeasurementSource::derive(sorted.iter().map(|entry| entry.source));
        Ok(Self {
            phase,
            entries: sorted,
            total,
            source,
        })
    }

    /// The phase this map charges.
    pub fn phase(&self) -> ChargingPhase {
        self.phase
    }

    /// The derived entry sum (conservation within the map).
    pub fn total(&self) -> u64 {
        self.total
    }

    /// The honest derived source label of the map total.
    pub fn source(&self) -> MeasurementSource {
        self.source
    }

    /// Entries in canonical (sorted) order.
    pub fn entries(&self) -> &[ChargingEntry] {
        &self.entries
    }
}

#[derive(Deserialize)]
struct ChargingMapWire {
    phase: ChargingPhase,
    entries: Vec<ChargingEntry>,
    total: u64,
    source: MeasurementSource,
}

impl<'de> Deserialize<'de> for ChargingMap {
    /// Wire decoding re-validates conservation, canonical order, and the
    /// derived label; a tampered map is refused.
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ChargingMapWire::deserialize(deserializer)?;
        let map = Self::build(wire.phase, wire.entries).map_err(de::Error::custom)?;
        if map.total != wire.total || map.source != wire.source {
            return Err(de::Error::custom(ChargingMapError::WireTotalsMismatch));
        }
        Ok(map)
    }
}

/// The complete six-phase lower-bound ledger.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ChargingMapSet {
    maps: BTreeMap<ChargingPhase, ChargingMap>,
}

impl ChargingMapSet {
    /// Builds a set from exactly one map per phase.
    pub fn new(maps: Vec<ChargingMap>) -> Result<Self, ChargingMapError> {
        let mut by_phase: BTreeMap<ChargingPhase, ChargingMap> = BTreeMap::new();
        for map in maps {
            if by_phase.insert(map.phase(), map.clone()).is_some() {
                return Err(ChargingMapError::DuplicatePhase(map.phase()));
            }
        }
        let mut missing = Vec::new();
        for phase in ChargingPhase::ALL {
            if !by_phase.contains_key(&phase) {
                missing.push(phase);
            }
        }
        if !missing.is_empty() {
            return Err(ChargingMapError::MissingPhases(missing));
        }
        Ok(Self { maps: by_phase })
    }

    /// The map for one phase.
    pub fn map(&self, phase: ChargingPhase) -> &ChargingMap {
        &self.maps[&phase]
    }

    /// All maps in canonical phase order.
    pub fn maps(&self) -> Vec<&ChargingMap> {
        ChargingPhase::ALL
            .iter()
            .map(|phase| &self.maps[phase])
            .collect()
    }

    /// Total attributed over every map, with the derived source label.
    pub fn total_attributed(&self) -> ResourceTotal {
        let mut amount = 0u128;
        let mut sources = Vec::new();
        for map in self.maps.values() {
            amount += u128::from(map.total());
            sources.push(map.source());
        }
        ResourceTotal::derived(amount, MeasurementSource::derive(sources))
    }

    /// Overlap checker: rejects any work unit charged in two phases.
    ///
    /// Double counting is a typed refusal; overlapping amounts are never
    /// silently merged or split.
    pub fn check_overlap(&self) -> Result<(), ChargingMapError> {
        let mut owner: BTreeMap<&str, ChargingPhase> = BTreeMap::new();
        for map in self.maps.values() {
            for entry in &map.entries {
                if let Some(first) = owner.insert(&entry.work_unit_id, map.phase()) {
                    return Err(ChargingMapError::OverlappingCharge {
                        work_unit_id: entry.work_unit_id.clone(),
                        first,
                        second: map.phase(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Closure check against a measured total: Gamma = attributed / measured
    /// never exceeds 1 under valid data.
    ///
    /// Conservation is enforced exactly: attributed above the measured total
    /// is `NonConservation`. Attributed below the measured total is honest
    /// partial coverage: the unclaimed residue is reported and never
    /// attributed without evidence. A zero measured total with zero
    /// attribution has no denominator and is refused.
    pub fn check_closure(&self, measured_total: u64) -> Result<ClosureReport, ChargingMapError> {
        let attributed = self.total_attributed().amount();
        if attributed > u128::from(measured_total) {
            return Err(ChargingMapError::NonConservation {
                attributed,
                measured: measured_total,
            });
        }
        if measured_total == 0 {
            return Err(ChargingMapError::ZeroMeasuredTotal);
        }
        let (gamma_num, gamma_den) = reduce(attributed as u64, measured_total);
        Ok(ClosureReport {
            attributed,
            measured: measured_total,
            unclaimed: u128::from(measured_total) - attributed,
            gamma: (gamma_num, gamma_den),
            full: gamma_num == gamma_den,
        })
    }

    /// Deterministically solves the charging maps from measured receipts.
    ///
    /// Every receipt is validated, and every charge is attributed through the
    /// total policy to exactly one phase. Refusals: an empty receipt set, a
    /// receipt that fails validation, and a work unit charged in more than
    /// one receipt (double classification across windows). The same inputs
    /// always yield the same maps.
    pub fn solve(
        policy: &PhasePolicy,
        receipts: &[CausalWorkReceipt],
    ) -> Result<Self, ChargingMapError> {
        if receipts.is_empty() {
            return Err(ChargingMapError::EmptyReceiptSet);
        }
        let mut by_phase: BTreeMap<ChargingPhase, Vec<ChargingEntry>> = BTreeMap::new();
        for receipt in receipts {
            receipt.validate().map_err(ChargingMapError::InvalidReceipt)?;
            for charge in &receipt.charges {
                let phase = policy.phase_for(charge.class);
                by_phase
                    .entry(phase)
                    .or_default()
                    .push(ChargingEntry {
                        work_unit_id: charge.work_unit_id.to_hex(),
                        amount: charge.amount,
                        source: MeasurementSource::Exact,
                    });
            }
        }
        let mut maps = Vec::with_capacity(ChargingPhase::ALL.len());
        for phase in ChargingPhase::ALL {
            let entries = by_phase.remove(&phase).unwrap_or_default();
            maps.push(ChargingMap::build(phase, entries)?);
        }
        Self::new(maps)
    }
}

#[derive(Deserialize)]
struct ChargingMapSetWire {
    maps: Vec<ChargingMap>,
}

impl<'de> Deserialize<'de> for ChargingMapSet {
    /// Wire decoding re-validates exactly-one-map-per-phase.
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ChargingMapSetWire::deserialize(deserializer)?;
        Self::new(wire.maps).map_err(de::Error::custom)
    }
}

/// Closure report: attributed vs measured with the exact reduced Gamma.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClosureReport {
    /// Total attributed over every map.
    pub attributed: u128,
    /// Measured total the attribution is closed against.
    pub measured: u64,
    /// Measured minus attributed: reported, never guessed into a phase.
    pub unclaimed: u128,
    /// Reduced Gamma = attributed / measured, always <= 1 under valid data.
    pub gamma: (u64, u64),
    /// Whether Gamma is exactly 1 (the honest full-coverage endpoint).
    pub full: bool,
}

/// An explicit, total mapping from causal classes to charging phases.
///
/// The policy must cover every [`CausalWorkClass`] exactly once, so the
/// solver never has to guess where an unmapped class belongs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhasePolicy {
    assignments: BTreeMap<CausalWorkClass, ChargingPhase>,
}

impl PhasePolicy {
    /// Builds a total policy, refusing a class assigned twice or left
    /// unassigned.
    pub fn new(assignments: &[(CausalWorkClass, ChargingPhase)]) -> Result<Self, ChargingMapError> {
        let mut map = BTreeMap::new();
        for (class, phase) in assignments {
            if map.insert(*class, *phase).is_some() {
                return Err(ChargingMapError::PolicyConflict { class: *class });
            }
        }
        for class in CausalWorkClass::ALL {
            if !map.contains_key(&class) {
                return Err(ChargingMapError::IncompletePolicy(class));
            }
        }
        Ok(Self { assignments: map })
    }

    /// The phase a class is assigned to. Total, so always defined.
    pub fn phase_for(&self, class: CausalWorkClass) -> ChargingPhase {
        self.assignments[&class]
    }
}

/// Typed failures of the lower-bound ledger. None recoverable by rewriting
/// attribution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChargingMapError {
    /// An entry carried an empty work-unit id.
    EmptyWorkUnitId,
    /// A work-unit id exceeded the wire size limit.
    WorkUnitIdTooLong {
        /// Rejected length.
        len: usize,
    },
    /// A zero-amount charge is not measured work.
    ZeroAmount {
        /// Work unit with the zero amount.
        work_unit_id: String,
    },
    /// The same work unit appeared twice in one map.
    DuplicateWorkUnit {
        /// Work unit charged twice.
        work_unit_id: String,
    },
    /// The map total would overflow u64.
    CounterOverflow,
    /// A wire map carried totals or labels that disagree with its entries.
    WireTotalsMismatch,
    /// Two maps charged the same phase.
    DuplicatePhase(ChargingPhase),
    /// The set is missing at least one phase.
    MissingPhases(Vec<ChargingPhase>),
    /// A work unit was charged in two phases: double counting.
    OverlappingCharge {
        /// Work unit charged twice.
        work_unit_id: String,
        /// First phase that charged it.
        first: ChargingPhase,
        /// Second phase that charged it.
        second: ChargingPhase,
    },
    /// Attributed total exceeds the measured total: conservation violated.
    NonConservation {
        /// Total attributed over every map.
        attributed: u128,
        /// Measured total.
        measured: u64,
    },
    /// A zero measured total has no closure denominator.
    ZeroMeasuredTotal,
    /// The receipt set was empty: nothing measured, nothing to solve.
    EmptyReceiptSet,
    /// A receipt failed validation.
    InvalidReceipt(CausalWorkError),
    /// A policy assigned one class to two phases.
    PolicyConflict {
        /// The conflicting class.
        class: CausalWorkClass,
    },
    /// A policy left a causal class unassigned.
    IncompletePolicy(CausalWorkClass),
}

impl fmt::Display for ChargingMapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyWorkUnitId => f.write_str("a charging entry carried an empty work-unit id"),
            Self::WorkUnitIdTooLong { len } => write!(
                f,
                "work-unit id of {len} bytes exceeds the {CAUSAL_WORK_MAX_ID_BYTES}-byte limit"
            ),
            Self::ZeroAmount { work_unit_id } => {
                write!(f, "work unit {work_unit_id} was charged a zero amount")
            }
            Self::DuplicateWorkUnit { work_unit_id } => {
                write!(f, "work unit {work_unit_id} is charged twice in one map")
            }
            Self::CounterOverflow => f.write_str("charging map total would overflow u64"),
            Self::WireTotalsMismatch => {
                f.write_str("wire map totals disagree with its entries")
            }
            Self::DuplicatePhase(phase) => write!(f, "phase {phase} has more than one map"),
            Self::MissingPhases(phases) => {
                write!(f, "missing phases: {:?}", phases.iter().map(|p| p.as_str()).collect::<Vec<_>>())
            }
            Self::OverlappingCharge {
                work_unit_id,
                first,
                second,
            } => write!(
                f,
                "work unit {work_unit_id} is charged in both {first} and {second}: double counting"
            ),
            Self::NonConservation {
                attributed,
                measured,
            } => write!(
                f,
                "attributed {attributed} exceeds measured {measured}: conservation violated"
            ),
            Self::ZeroMeasuredTotal => {
                f.write_str("a zero measured total has no closure denominator")
            }
            Self::EmptyReceiptSet => f.write_str("no measured receipts were provided"),
            Self::InvalidReceipt(error) => write!(f, "invalid causal-work receipt: {error}"),
            Self::PolicyConflict { class } => {
                write!(f, "policy assigns class {:?} to two phases", class.as_str())
            }
            Self::IncompletePolicy(class) => {
                write!(f, "policy leaves class {:?} unassigned", class.as_str())
            }
        }
    }
}

impl Error for ChargingMapError {}

fn reduce(num: u64, den: u64) -> (u64, u64) {
    let mut a = num;
    let mut b = den;
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    (num / a, den / a)
}

