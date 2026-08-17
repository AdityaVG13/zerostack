//! Session-level Q99/residency gate (V6-R4).
//!
//! Wires the zero-gate W4 residency contracts (ZS-CACHE-001/003/004/010/012)
//! into the zsx-core session execution path. One [`SessionResidencyGate`]
//! is installed per execution (one demand window per request); the
//! layer-validity ledger lives on the connector state for the whole session
//! so an L3/CAS loss in a later request can preserve the L2 validity that an
//! earlier request published.
//!
//! Honesty rules observed here:
//! - Window observations come only from measured (Exact) worker token
//!   accounting; estimates and conservative upper bounds never feed Q99
//!   (claims only from receipts, measured not claimed).
//! - Every Q99 figure is reported against its labeled denominator
//!   (`q99_demanded_mass:<N>`); no bare percentages are emitted anywhere.
//! - Impossibility is a reportable state: an empty window or a central
//!   change over 1% of demanded mass reports `unavailable` with reasons,
//!   never a vacuous pass or an average.
//! - The demand ledger is re-validated at report time; a demanded object
//!   whose weight is absent (zero-byte objects cannot declare a nonzero
//!   demand weight) rejects the whole report instead of silently omitting
//!   the object.
//! - Eviction decisions on the session cache consult [`EvictionSlack`]
//!   through [`SessionResidencyGate::guard_eviction`]; resident mass is the
//!   measured hit mass, demanded mass the measured window demand.
//!
//! Residual (documented, not faked): FSZero/GraphZero adapters return no
//! worker token accounting, so L1/L2 window observations stay empty
//! ("no_demand_observations") in production until engine-side accounting
//! exists; the L1/L2 demand ledger entries (verified blob byte mass) and
//! layer-validity publications are live today. Invalid-mass erosion tracking
//! (ZS-CACHE-005) needs GraphZero invalidation events, so the restoration
//! threshold starts inert (initial valid mass 0) and the report passes
//! `current_valid_mass = 0`. Engine-side eviction paths (FSZero CAS GC,
//! TokenZero action cache) are out of scope for the session hub and remain
//! unwired to the slack guard.

use std::collections::BTreeMap;

use serde::Serialize;
use zero_abi::raw_worker::{EngineIdentity, WorkerTokenAccounting, WorkerTokenCountKind};
use zero_abi::Sha256Digest;
use zero_gate::residency::{
    CacheLayerTier, DemandObservation, DemandWeightLedger, DemandWeightedObject,
    EvictionSlack, Q99WindowReport, Q99Window, ResidencyError,
};

/// Schema of the session Q99 report (a typed telemetry receipt, not prose).
pub const SESSION_Q99_REPORT_SCHEMA: &str = "zerostack.session_q99_report.v1";

/// All tiers in report order.
const TIERS: [CacheLayerTier; 3] = [
    CacheLayerTier::L1,
    CacheLayerTier::L2,
    CacheLayerTier::L3,
];

/// Tier attribution for connector dispatches (hub-side decision).
///
/// L1 = FSZero (session byte cache), L2 = GraphZero (structural validity
/// authority), L3 = TokenZero (provider/cold tokenizer surface). Engine
/// internal cache layers are a separate matter; this is the attribution the
/// session ledger uses for its observations.
pub fn tier_of_engine(engine: EngineIdentity) -> CacheLayerTier {
    match engine {
        EngineIdentity::FsZero => CacheLayerTier::L1,
        EngineIdentity::GraphZero => CacheLayerTier::L2,
        EngineIdentity::TokenZero => CacheLayerTier::L3,
    }
}

/// One tier's Q99 window inside the session report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TierQ99Report {
    pub tier: CacheLayerTier,
    pub window: Q99WindowReport,
    /// Labeled Q99 denominator: `q99_demanded_mass:<N>`. Every Q99 figure in
    /// this tier's window is only ever read against this denominator.
    pub denominator_label: String,
}

/// Session Q99 report: per-tier windows, the measured demand closure, and
/// layer-validity accounting, all from receipts. `unavailable` is a
/// first-class state (empty windows, central change over 1%, rejected
/// closures are never averaged or hidden).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionQ99Report {
    pub schema: String,
    pub window_id: String,
    pub tiers: Vec<TierQ99Report>,
    /// Measured demanded-object closure of the window (labeled denominator
    /// per object: `object_root` + `demand_weight`).
    pub demand_ledger: DemandWeightLedger,
    /// L2-valid entries in the session layer-validity ledger.
    pub layer_valid_entries: usize,
    /// Measured resident (hit) mass across all tiers.
    pub resident_mass: u64,
    /// Measured demanded mass across all tiers.
    pub demanded_mass: u64,
    /// `floor = 0.99 * demanded_mass`: resident mass may not fall below this
    /// on an eviction (ZS-CACHE-012).
    pub eviction_floor_mass: u64,
    /// `sigma = W_R - 0.99W` in PPM of demanded mass; `None` when no demand
    /// was observed (no evidence to guard on).
    pub eviction_slack_ppm: Option<i64>,
    pub unavailable: bool,
    pub reasons: Vec<String>,
}

/// Per-execution residency gate over one demand window.
#[derive(Clone, Debug)]
pub struct SessionResidencyGate {
    window_id: String,
    windows: BTreeMap<CacheLayerTier, Q99Window>,
    /// Demanded weight per (object_root, tier), deduplicated across
    /// dispatches of the same window.
    demand: BTreeMap<(Sha256Digest, CacheLayerTier), u64>,
    /// Objects demanded in this window whose demand weight is absent
    /// (zero-byte objects cannot declare a nonzero weight). Their presence
    /// rejects the report: missing weights are reported, never silently
    /// omitted.
    unrecorded: Vec<Sha256Digest>,
}

impl SessionResidencyGate {
    /// Open one demand window. The restoration threshold starts with
    /// `initial_valid_mass = 0` (no invalid-mass measurement yet, see module
    /// docs); it stays inert until ZS-CACHE-005 tracking lands.
    pub fn new(window_id: impl Into<String>) -> Self {
        let window_id = window_id.into();
        let windows = TIERS
            .into_iter()
            .map(|tier| (tier, Q99Window::new(window_id.clone(), 0)))
            .collect();
        Self {
            window_id,
            windows,
            demand: BTreeMap::new(),
            unrecorded: Vec::new(),
        }
    }

    /// The demand window this gate observes. Consumed by unit tests and
    /// report correlation; the window id is also carried by every tier
    /// report.
    #[allow(dead_code)]
    pub fn window_id(&self) -> &str {
        &self.window_id
    }

    /// Observe one dispatch's measured token accounting at its tier. Only
    /// Exact accounting is evidence; estimates and conservative upper bounds
    /// never feed Q99. The mass split is exact: `cached_tokens` counts as hit
    /// mass, `raw - cached` as recomputed mass.
    pub fn observe_dispatch(
        &mut self,
        tier: CacheLayerTier,
        accounting: &WorkerTokenAccounting,
    ) -> Result<(), ResidencyError> {
        if accounting.count_kind != WorkerTokenCountKind::Exact {
            return Ok(());
        }
        let raw = accounting.raw_tokens;
        if raw == 0 {
            return Ok(());
        }
        let cached = accounting.cached_tokens.min(raw);
        let window = self
            .windows
            .get_mut(&tier)
            .ok_or_else(|| ResidencyError::InvalidLayerLedger(format!("no window for {tier:?}")))?;
        if cached > 0 {
            window.observe(DemandObservation::new(cached, true)?);
        }
        let missed = raw - cached;
        if missed > 0 {
            window.observe(DemandObservation::new(missed, false)?);
        }
        Ok(())
    }

    /// Record one verified demanded object in the window closure. A
    /// zero-byte object has no representable nonzero demand weight; it is
    /// recorded as a demanded-but-unweighted object, which rejects the
    /// report (missing weights fail closed) rather than silently vanishing.
    pub fn record_demand(
        &mut self,
        object_root: Sha256Digest,
        weight: u64,
        tier: CacheLayerTier,
    ) -> Result<(), ResidencyError> {
        if weight == 0 {
            if !self.unrecorded.contains(&object_root) {
                self.unrecorded.push(object_root);
            }
            return Ok(());
        }
        DemandWeightedObject::new(object_root, weight, self.window_id.clone(), tier)?;
        let key = (object_root, tier);
        let entry = self.demand.entry(key).or_insert(0);
        *entry = (*entry).saturating_add(weight);
        Ok(())
    }

    /// Eviction decision on the session cache: consult [`EvictionSlack`]
    /// against the measured masses of this window. Rejects any eviction that
    /// would push resident mass below 99% of demanded mass, and any guard
    /// call without observed demand (fail closed).
    ///
    /// No session-owned eviction event exists yet (engine-side eviction
    /// paths are FSZero/TokenZero territory), so this is the consulted
    /// decision surface for harnesses and the unit tests; the report carries
    /// the masses any such decision needs.
    #[allow(dead_code)]
    pub fn guard_eviction(&self, evict_weight: u64) -> Result<(), ResidencyError> {
        let (resident_mass, demanded_mass) = self.masses();
        let slack = EvictionSlack::new(resident_mass, demanded_mass)?;
        slack.guard_eviction(evict_weight)
    }

    /// Finalize the window into a report. Re-validates the demand closure
    /// (duplicates, zero weights, absent weights) and the per-tier Q99
    /// windows. `layer_valid_entries` is the live count from the session
    /// layer-validity ledger.
    pub fn report(
        &self,
        layer_valid_entries: usize,
    ) -> Result<SessionQ99Report, ResidencyError> {
        if let Some(object_root) = self.unrecorded.first() {
            return Err(ResidencyError::InvalidDemandLedger(format!(
                "demand weight absent for zero-byte object {object_root}"
            )));
        }
        let objects = self
            .demand
            .iter()
            .map(|((object_root, tier), weight)| {
                DemandWeightedObject::new(*object_root, *weight, self.window_id.clone(), *tier)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let demand_ledger = DemandWeightLedger::new(objects)?;
        let mut tiers = Vec::new();
        let mut unavailable = false;
        let mut reasons = Vec::new();
        for tier in TIERS {
            let window = self
                .windows
                .get(&tier)
                .ok_or_else(|| ResidencyError::InvalidLayerLedger(format!("no window for {tier:?}")))?;
            // current_valid_mass = 0: invalid-mass erosion is not measured
            // yet (see module docs), so the restoration threshold stays
            // inert and only central change can make the window unavailable.
            let window_report = window.report(0);
            if window_report.unavailable {
                unavailable = true;
            }
            for reason in &window_report.reasons {
                reasons.push(format!("{}:{reason}", tier.as_str()));
            }
            tiers.push(TierQ99Report {
                tier,
                window: window_report,
                denominator_label: format!("q99_demanded_mass:{}", window.demanded_mass()),
            });
        }
        let (resident_mass, demanded_mass) = self.masses();
        let eviction_floor_mass = demanded_mass * 99 / 100;
        let eviction_slack_ppm = if demanded_mass > 0 {
            Some(EvictionSlack::new(resident_mass, demanded_mass)?.slack_ppm())
        } else {
            None
        };
        Ok(SessionQ99Report {
            schema: SESSION_Q99_REPORT_SCHEMA.into(),
            window_id: self.window_id.clone(),
            tiers,
            demand_ledger,
            layer_valid_entries,
            resident_mass,
            demanded_mass,
            eviction_floor_mass,
            eviction_slack_ppm,
            unavailable,
            reasons,
        })
    }

    fn masses(&self) -> (u64, u64) {
        self.windows.values().fold((0, 0), |(resident, demanded), window| {
            (
                resident.saturating_add(window.hit_mass()),
                demanded.saturating_add(window.demanded_mass()),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_window_report_is_unavailable_never_numeric() {
        let gate = SessionResidencyGate::new("empty-q99");
        let report = gate.report(0).expect("empty report is a first-class state");
        assert!(report.unavailable, "empty windows must be unavailable");
        assert_eq!(report.demanded_mass, 0);
        assert!(report.eviction_slack_ppm.is_none());
        assert!(
            report
                .reasons
                .iter()
                .any(|reason| reason.contains("no_demand_observations")),
            "reasons={:?}",
            report.reasons
        );
        for tier in &report.tiers {
            assert!(tier.window.unavailable);
            assert_eq!(tier.window.demanded_mass, 0);
            assert_eq!(tier.denominator_label, "q99_demanded_mass:0");
            assert!(
                tier.window
                    .reasons
                    .iter()
                    .any(|reason| reason == "no_demand_observations"),
                "tier={:?} reasons={:?}",
                tier.tier,
                tier.window.reasons
            );
            let dumped = serde_json::to_string(&tier.window).expect("serialize");
            assert!(
                !dumped.contains('%'),
                "empty window must not emit a bare percentage: {dumped}"
            );
        }
    }
}

