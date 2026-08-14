//! Verified capability subsystem (ZS-CAP-001..006, ZS-METRIC-009/010).
//!
//! Capabilities are learned procedural assets accumulated from accepted
//! episodes. A capability becomes executable only when it carries the
//! 11-field core (exact scope, rooted preconditions, declared
//! reads/writes/effects, postcondition and verifier, successor-safety
//! obligation, fallback/rollback, dependency pins, freshness policy) and has
//! passed shadow-mode promotion behind the same baseline firewall. This is
//! deliberately last in the program: capability authority depends on every
//! preceding truth and authority layer.
//!
//! Fail-closed laws:
//! - CAP-001: a captured asset is NEVER authoritative until separately
//!   proved. No constructor or transition produces `Promoted` without
//!   passing through `Shadow`; `use_asset` returns `Executable` only for
//!   `Promoted` AND `Matched`.
//! - CAP-002: scope is exact (operation class AND input shape digest).
//!   Cross-project tasks never match (leakage kill condition); a changed or
//!   missing pinned dependency invalidates the asset and demands revocation.
//! - CAP-003: promotion is decided only by shadow trials with complete cost
//!   accounting (capture + maintenance + trial costs); any protected
//!   regression (negative transfer) or miss denies promotion. A `Promoted`
//!   asset still runs behind the baseline firewall and the verifier gate;
//!   promotion never grants execution authority by itself.
//! - CAP-004: failure syndromes are append-only (no delete API); a syndrome
//!   recorded for a `Promoted` (or `Shadow`) asset forces demotion.
//! - CAP-005: an expired or revoked asset can only ADD cost, never worsen a
//!   protected result. There is no execution path for a non-`Promoted`
//!   asset: `use_asset` returns `BaselineRequired` with the standing cost.
//! - METRIC-009: the lifetime-value ledger retires negative-value assets
//!   automatically once observed across a threshold of epochs.
//! - METRIC-010: per-epoch benefit is clamped to the certified
//!   unavoidable-work lower bound `(actual baseline cost - certified
//!   minimum)`; a claim above the bound fails closed and records the
//!   clamped benefit.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

pub const ASSET_CONTRACT_VERSION_V1: u16 = 1;
pub const ASSET_ID_MAX_BYTES_V1: usize = 128;
pub const ASSET_DIGEST_HEX_LEN_V1: usize = 64;

/// Fail-closed error for the verified capability subsystem.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssetErrorV1 {
    InvalidAsset(String),
    InvalidScope(String),
    InvalidPrecondition(String),
    InvalidDependency(String),
    InvalidFreshness(String),
    InvalidTrial(String),
    InvalidSyndrome(String),
    InvalidLedger(String),
    InvalidRevocation(String),
    InvalidTransition {
        from: AssetStateV1,
        to: AssetStateV1,
    },
    ScopeMismatch(String),
    ClampedBenefit {
        claimed: u64,
        allowed: u64,
    },
}

impl fmt::Display for AssetErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAsset(detail) => write!(formatter, "invalid capability asset: {detail}"),
            Self::InvalidScope(detail) => write!(formatter, "invalid capability scope: {detail}"),
            Self::InvalidPrecondition(detail) => {
                write!(formatter, "invalid rooted precondition: {detail}")
            }
            Self::InvalidDependency(detail) => {
                write!(formatter, "invalid dependency pin: {detail}")
            }
            Self::InvalidFreshness(detail) => {
                write!(formatter, "invalid freshness policy: {detail}")
            }
            Self::InvalidTrial(detail) => write!(formatter, "invalid shadow trial: {detail}"),
            Self::InvalidSyndrome(detail) => write!(formatter, "invalid failure syndrome: {detail}"),
            Self::InvalidLedger(detail) => write!(formatter, "invalid asset ledger: {detail}"),
            Self::InvalidRevocation(detail) => write!(formatter, "invalid revocation: {detail}"),
            Self::InvalidTransition { from, to } => {
                write!(formatter, "forbidden asset transition {from:?} -> {to:?}")
            }
            Self::ScopeMismatch(detail) => {
                write!(formatter, "certified lower bound scope mismatch: {detail}")
            }
            Self::ClampedBenefit { claimed, allowed } => write!(
                formatter,
                "benefit claim {claimed} exceeds certified lower-bound allowance {allowed}; clamped"
            ),
        }
    }
}

impl Error for AssetErrorV1 {}

fn is_lower_hex_64(value: &str) -> bool {
    value.len() == ASSET_DIGEST_HEX_LEN_V1
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Exact task-scope descriptor. Scope equality is exact: operation class AND
/// input shape digest must both match (CAP-002).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeV1 {
    pub operation_class: String,
    pub input_shape_digest_hex: String,
}

impl ScopeV1 {
    pub fn new(
        operation_class: impl Into<String>,
        input_shape_digest_hex: impl Into<String>,
    ) -> Result<Self, AssetErrorV1> {
        let scope = Self {
            operation_class: operation_class.into(),
            input_shape_digest_hex: input_shape_digest_hex.into(),
        };
        scope.validate()?;
        Ok(scope)
    }

    pub fn validate(&self) -> Result<(), AssetErrorV1> {
        if self.operation_class.is_empty() {
            return Err(AssetErrorV1::InvalidScope(
                "operation_class must be nonempty".into(),
            ));
        }
        if !is_lower_hex_64(&self.input_shape_digest_hex) {
            return Err(AssetErrorV1::InvalidScope(format!(
                "input_shape_digest_hex must be {} lowercase hex characters",
                ASSET_DIGEST_HEX_LEN_V1
            )));
        }
        Ok(())
    }
}

/// A precondition rooted at an exact project-root digest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RootedPreconditionV1 {
    pub name: String,
    pub root_digest_hex: String,
}

impl RootedPreconditionV1 {
    pub fn new(
        name: impl Into<String>,
        root_digest_hex: impl Into<String>,
    ) -> Result<Self, AssetErrorV1> {
        let precondition = Self {
            name: name.into(),
            root_digest_hex: root_digest_hex.into(),
        };
        precondition.validate()?;
        Ok(precondition)
    }

    pub fn validate(&self) -> Result<(), AssetErrorV1> {
        if self.name.is_empty() {
            return Err(AssetErrorV1::InvalidPrecondition(
                "name must be nonempty".into(),
            ));
        }
        if !is_lower_hex_64(&self.root_digest_hex) {
            return Err(AssetErrorV1::InvalidPrecondition(format!(
                "root_digest_hex must be {} lowercase hex characters",
                ASSET_DIGEST_HEX_LEN_V1
            )));
        }
        Ok(())
    }
}

/// A dependency pin: the exact digest an asset was verified against.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyPinV1 {
    pub name: String,
    pub digest_hex: String,
}

impl DependencyPinV1 {
    pub fn new(name: impl Into<String>, digest_hex: impl Into<String>) -> Result<Self, AssetErrorV1> {
        let pin = Self {
            name: name.into(),
            digest_hex: digest_hex.into(),
        };
        pin.validate()?;
        Ok(pin)
    }

    pub fn validate(&self) -> Result<(), AssetErrorV1> {
        if self.name.is_empty() {
            return Err(AssetErrorV1::InvalidDependency(
                "name must be nonempty".into(),
            ));
        }
        if !is_lower_hex_64(&self.digest_hex) {
            return Err(AssetErrorV1::InvalidDependency(format!(
                "digest_hex must be {} lowercase hex characters",
                ASSET_DIGEST_HEX_LEN_V1
            )));
        }
        Ok(())
    }
}

/// Freshness: an asset is valid for `valid_epochs` from its capture epoch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FreshnessPolicyV1 {
    pub valid_epochs: u64,
    pub captured_epoch: u64,
}

impl FreshnessPolicyV1 {
    pub fn new(valid_epochs: u64, captured_epoch: u64) -> Result<Self, AssetErrorV1> {
        let policy = Self {
            valid_epochs,
            captured_epoch,
        };
        policy.validate()?;
        Ok(policy)
    }

    pub fn validate(&self) -> Result<(), AssetErrorV1> {
        if self.valid_epochs == 0 {
            return Err(AssetErrorV1::InvalidFreshness(
                "valid_epochs must be >= 1".into(),
            ));
        }
        Ok(())
    }
}

/// Capture and maintenance cost (complete cost accounting, CAP-003).
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CostV1 {
    pub capture_units: u64,
    pub maintenance_units_per_epoch: u64,
}

/// Lifecycle state. `Captured` is NEVER authoritative; only `Promoted`
/// assets may execute, and only after passing through `Shadow` (CAP-001).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetStateV1 {
    Captured,
    Shadow,
    Promoted,
    Demoted,
    Revoked,
    Expired,
}

/// The allowed state machine. There is NO path back to `Promoted` except via
/// `Shadow`; `Revoked` is reachable from any state (revocation trigger), and
/// `Demoted`/`Revoked`/`Expired` have no outgoing edges toward `Shadow` or
/// `Promoted`.
pub fn allowed_asset_transition(from: AssetStateV1, to: AssetStateV1) -> bool {
    use AssetStateV1::*;
    match (from, to) {
        (Captured, Shadow) => true,
        (Shadow, Promoted) | (Shadow, Demoted) => true,
        (Promoted, Demoted) | (Promoted, Revoked) | (Promoted, Expired) => true,
        (_, Revoked) => true,
        _ => false,
    }
}

/// A verified capability asset: the 11-field core (scope, preconditions,
/// reads, writes, effects, postconditions, verifier, successor relation,
/// rollback, dependencies, freshness) plus identity, capture cost, lifecycle
/// state, and revocation reason.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityAssetV1 {
    pub contract_version: u16,
    pub asset_id: String,
    pub project_id: String,
    pub scope: ScopeV1,
    pub preconditions: Vec<RootedPreconditionV1>,
    pub reads: Vec<String>,
    pub writes: Vec<String>,
    pub effects: Vec<String>,
    pub postcondition: String,
    pub verifier_id: String,
    pub successor_relation: String,
    pub rollback: String,
    pub dependencies: Vec<DependencyPinV1>,
    pub freshness: FreshnessPolicyV1,
    pub capture_cost: CostV1,
    pub state: AssetStateV1,
    pub revocation_reason: Option<String>,
}

impl CapabilityAssetV1 {
    /// Construct a NEW asset. It is born `Captured` and is NEVER
    /// authoritative; no constructor produces `Promoted` without shadow
    /// evaluation.
    #[allow(clippy::too_many_arguments)]
    pub fn captured(
        asset_id: impl Into<String>,
        project_id: impl Into<String>,
        scope: ScopeV1,
        preconditions: Vec<RootedPreconditionV1>,
        reads: Vec<String>,
        writes: Vec<String>,
        effects: Vec<String>,
        postcondition: impl Into<String>,
        verifier_id: impl Into<String>,
        successor_relation: impl Into<String>,
        rollback: impl Into<String>,
        dependencies: Vec<DependencyPinV1>,
        freshness: FreshnessPolicyV1,
        capture_cost: CostV1,
    ) -> Result<Self, AssetErrorV1> {
        let asset = Self {
            contract_version: ASSET_CONTRACT_VERSION_V1,
            asset_id: asset_id.into(),
            project_id: project_id.into(),
            scope,
            preconditions,
            reads,
            writes,
            effects,
            postcondition: postcondition.into(),
            verifier_id: verifier_id.into(),
            successor_relation: successor_relation.into(),
            rollback: rollback.into(),
            dependencies,
            freshness,
            capture_cost,
            state: AssetStateV1::Captured,
            revocation_reason: None,
        };
        asset.validate()?;
        Ok(asset)
    }

    pub fn validate(&self) -> Result<(), AssetErrorV1> {
        if self.contract_version != ASSET_CONTRACT_VERSION_V1 {
            return Err(AssetErrorV1::InvalidAsset(format!(
                "unsupported contract version {}",
                self.contract_version
            )));
        }
        if self.asset_id.is_empty() || self.asset_id.len() > ASSET_ID_MAX_BYTES_V1 {
            return Err(AssetErrorV1::InvalidAsset(format!(
                "asset_id must be nonempty and at most {} bytes",
                ASSET_ID_MAX_BYTES_V1
            )));
        }
        if self.project_id.is_empty() {
            return Err(AssetErrorV1::InvalidAsset(
                "project_id must be nonempty (cross-project isolation key)".into(),
            ));
        }
        self.scope.validate()?;
        let mut precondition_names = std::collections::BTreeSet::new();
        for precondition in &self.preconditions {
            precondition.validate()?;
            if !precondition_names.insert(precondition.name.clone()) {
                return Err(AssetErrorV1::InvalidAsset(format!(
                    "duplicate precondition {}",
                    precondition.name
                )));
            }
        }
        for list in [&self.reads, &self.writes, &self.effects] {
            if list.iter().any(|entry| entry.is_empty()) {
                return Err(AssetErrorV1::InvalidAsset(
                    "reads/writes/effects entries must be nonempty".into(),
                ));
            }
        }
        if self.postcondition.is_empty() {
            return Err(AssetErrorV1::InvalidAsset(
                "postcondition must be nonempty".into(),
            ));
        }
        if self.verifier_id.is_empty() {
            return Err(AssetErrorV1::InvalidAsset(
                "verifier_id must be nonempty".into(),
            ));
        }
        if self.successor_relation.is_empty() {
            return Err(AssetErrorV1::InvalidAsset(
                "successor_relation must be nonempty (successor-safety obligation)".into(),
            ));
        }
        if self.rollback.is_empty() {
            return Err(AssetErrorV1::InvalidAsset(
                "rollback must be nonempty (fallback/rollback path)".into(),
            ));
        }
        let mut dependency_names = std::collections::BTreeSet::new();
        for pin in &self.dependencies {
            pin.validate()?;
            if !dependency_names.insert(pin.name.clone()) {
                return Err(AssetErrorV1::InvalidAsset(format!(
                    "duplicate dependency pin {}",
                    pin.name
                )));
            }
        }
        self.freshness.validate()?;
        Ok(())
    }

    fn transition(&mut self, to: AssetStateV1) -> Result<(), AssetErrorV1> {
        let from = self.state;
        if !allowed_asset_transition(from, to) {
            return Err(AssetErrorV1::InvalidTransition { from, to });
        }
        self.state = to;
        Ok(())
    }

    /// Captured -> Shadow. Entering shadow evaluation.
    pub fn enter_shadow(&mut self) -> Result<(), AssetErrorV1> {
        self.transition(AssetStateV1::Shadow)
    }

    /// Shadow -> Promoted, only after a passing promotion decision. Still
    /// runs behind the baseline firewall and verifier gate (CAP-003).
    pub fn promote(&mut self) -> Result<(), AssetErrorV1> {
        self.transition(AssetStateV1::Promoted)
    }

    /// -> Demoted (syndrome, negative lifetime value, or decision).
    pub fn demote(&mut self) -> Result<(), AssetErrorV1> {
        self.transition(AssetStateV1::Demoted)
    }

    /// Promoted -> Expired (freshness lapse).
    pub fn expire(&mut self) -> Result<(), AssetErrorV1> {
        self.transition(AssetStateV1::Expired)
    }

    /// Any state -> Revoked on a revocation trigger (CAP-005). The trigger
    /// and reason are recorded on the asset.
    pub fn revoke_for(
        &mut self,
        trigger: &RevocationTriggerV1,
        reason: impl Into<String>,
    ) -> Result<(), AssetErrorV1> {
        let from = self.state;
        if !allowed_asset_transition(from, AssetStateV1::Revoked) {
            return Err(AssetErrorV1::InvalidRevocation(format!(
                "cannot revoke from {from:?}"
            )));
        }
        self.state = AssetStateV1::Revoked;
        self.revocation_reason = Some(format!("{trigger}: {}", reason.into()));
        Ok(())
    }
}

/// Revocation triggers (CAP-005): dependency, contract, verifier, or epoch
/// change.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RevocationTriggerV1 {
    DependencyChange { name: String },
    ContractChange,
    VerifierChange,
    EpochChange,
}

impl fmt::Display for RevocationTriggerV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DependencyChange { name } => write!(formatter, "dependency_change:{name}"),
            Self::ContractChange => write!(formatter, "contract_change"),
            Self::VerifierChange => write!(formatter, "verifier_change"),
            Self::EpochChange => write!(formatter, "epoch_change"),
        }
    }
}

/// Task matching result (CAP-002). Only `Matched` may execute, and only for
/// a `Promoted` asset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MatchOutcomeV1 {
    Matched,
    OutOfScope,
    CrossProject,
    StaleDependency { name: String },
    ExpiredFreshness,
}

/// Exact task matching: project equality first (leakage kill), then exact
/// scope equality, then dependency digests, then freshness.
pub fn match_task(
    asset: &CapabilityAssetV1,
    task_scope: &ScopeV1,
    project_id: &str,
    current_epoch: u64,
    current_dependency_digests: &[(String, String)],
) -> MatchOutcomeV1 {
    if asset.project_id != project_id {
        return MatchOutcomeV1::CrossProject;
    }
    if &asset.scope != task_scope {
        return MatchOutcomeV1::OutOfScope;
    }
    for pin in &asset.dependencies {
        let current = current_dependency_digests
            .iter()
            .find(|(name, _)| name == &pin.name);
        match current {
            Some((_, digest)) if digest == &pin.digest_hex => {}
            _ => return MatchOutcomeV1::StaleDependency { name: pin.name.clone() },
        }
    }
    if asset
        .freshness
        .captured_epoch
        .saturating_add(asset.freshness.valid_epochs)
        <= current_epoch
    {
        return MatchOutcomeV1::ExpiredFreshness;
    }
    MatchOutcomeV1::Matched
}

/// One shadow trial comparing the baseline outcome to the shadow outcome
/// (CAP-003).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowTrialV1 {
    pub trial_id: String,
    pub asset_id: String,
    pub baseline_outcome_digest_hex: String,
    pub shadow_outcome_digest_hex: String,
    pub protected_regression: bool,
    pub strict_rescue: bool,
    pub cost_units: u64,
}

impl ShadowTrialV1 {
    pub fn new(
        trial_id: impl Into<String>,
        asset_id: impl Into<String>,
        baseline_outcome_digest_hex: impl Into<String>,
        shadow_outcome_digest_hex: impl Into<String>,
        protected_regression: bool,
        strict_rescue: bool,
        cost_units: u64,
    ) -> Result<Self, AssetErrorV1> {
        let trial = Self {
            trial_id: trial_id.into(),
            asset_id: asset_id.into(),
            baseline_outcome_digest_hex: baseline_outcome_digest_hex.into(),
            shadow_outcome_digest_hex: shadow_outcome_digest_hex.into(),
            protected_regression,
            strict_rescue,
            cost_units,
        };
        trial.validate()?;
        Ok(trial)
    }

    pub fn validate(&self) -> Result<(), AssetErrorV1> {
        if self.trial_id.is_empty() || self.asset_id.is_empty() {
            return Err(AssetErrorV1::InvalidTrial(
                "trial_id and asset_id must be nonempty".into(),
            ));
        }
        if !is_lower_hex_64(&self.baseline_outcome_digest_hex)
            || !is_lower_hex_64(&self.shadow_outcome_digest_hex)
        {
            return Err(AssetErrorV1::InvalidTrial(format!(
                "outcome digests must be {} lowercase hex characters",
                ASSET_DIGEST_HEX_LEN_V1
            )));
        }
        Ok(())
    }
}

/// Promotion report with COMPLETE cost accounting (CAP-003): capture cost,
/// maintenance so far, and all trial costs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionReportV1 {
    pub trials_observed: u64,
    pub misses: u64,
    pub regressions: u64,
    pub strict_rescues: u64,
    pub complete_cost_units: u64,
    pub min_trials: u64,
}

/// Promotion decision. `Promote` never grants execution authority by itself:
/// the runtime still routes execution behind the baseline firewall and the
/// verifier gate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PromotionDecisionV1 {
    Promote { report: PromotionReportV1 },
    Deny { report: PromotionReportV1, reasons: Vec<String> },
}

/// Shadow-mode promotion (CAP-003). Denies on: no trials, insufficient
/// trials, any protected regression (negative transfer -- demotion demanded),
/// or any miss. One shadow trial counts as one observed epoch for
/// maintenance accounting.
pub fn evaluate_promotion(
    asset: &CapabilityAssetV1,
    trials: &[ShadowTrialV1],
    min_trials: u64,
) -> PromotionDecisionV1 {
    let observed: Vec<&ShadowTrialV1> = trials
        .iter()
        .filter(|trial| trial.asset_id == asset.asset_id)
        .collect();
    let trials_observed = observed.len() as u64;
    let misses = observed
        .iter()
        .filter(|trial| trial.baseline_outcome_digest_hex != trial.shadow_outcome_digest_hex)
        .count() as u64;
    let regressions = observed.iter().filter(|trial| trial.protected_regression).count() as u64;
    let strict_rescues = observed.iter().filter(|trial| trial.strict_rescue).count() as u64;
    let trial_cost: u64 = observed.iter().map(|trial| trial.cost_units).sum();
    let maintenance = asset
        .capture_cost
        .maintenance_units_per_epoch
        .saturating_mul(trials_observed);
    let complete_cost_units = asset
        .capture_cost
        .capture_units
        .saturating_add(maintenance)
        .saturating_add(trial_cost);
    let report = PromotionReportV1 {
        trials_observed,
        misses,
        regressions,
        strict_rescues,
        complete_cost_units,
        min_trials,
    };

    let mut reasons = Vec::new();
    if trials_observed == 0 {
        reasons.push("no_shadow_trials".into());
    } else if trials_observed < min_trials {
        reasons.push(format!("insufficient_trials:{trials_observed}:{min_trials}"));
    }
    if regressions > 0 {
        reasons.push(format!(
            "protected_regression:{regressions}:negative_transfer_demotion_demanded"
        ));
    }
    if misses > 0 {
        reasons.push(format!("shadow_miss:{misses}"));
    }

    if reasons.is_empty() {
        PromotionDecisionV1::Promote { report }
    } else {
        PromotionDecisionV1::Deny { report, reasons }
    }
}

/// Use outcome (CAP-001/005). `Executable` exists ONLY for `Promoted` AND
/// `Matched`; every other combination adds cost and returns to baseline,
/// never worsening a protected result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UseOutcomeV1 {
    Executable,
    BaselineRequired { added_cost_units: u64 },
}

impl CapabilityAssetV1 {
    /// The ONLY execution admission point. A non-`Promoted` asset (Captured,
    /// Shadow, Demoted, Revoked, Expired) always adds its standing cost and
    /// falls back to the baseline; it can never execute and can never worsen
    /// a protected result.
    pub fn use_asset(&self, outcome: &MatchOutcomeV1) -> UseOutcomeV1 {
        if self.state == AssetStateV1::Promoted && *outcome == MatchOutcomeV1::Matched {
            UseOutcomeV1::Executable
        } else {
            let added_cost_units = self
                .capture_cost
                .capture_units
                .saturating_add(self.capture_cost.maintenance_units_per_epoch);
            UseOutcomeV1::BaselineRequired { added_cost_units }
        }
    }
}

/// One recorded failure syndrome (CAP-004).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SyndromeV1 {
    pub syndrome_id: String,
    pub asset_id: String,
    pub failure_class: String,
    pub observed_epoch: u64,
    pub detail: String,
}

impl SyndromeV1 {
    pub fn new(
        syndrome_id: impl Into<String>,
        asset_id: impl Into<String>,
        failure_class: impl Into<String>,
        observed_epoch: u64,
        detail: impl Into<String>,
    ) -> Result<Self, AssetErrorV1> {
        let syndrome = Self {
            syndrome_id: syndrome_id.into(),
            asset_id: asset_id.into(),
            failure_class: failure_class.into(),
            observed_epoch,
            detail: detail.into(),
        };
        syndrome.validate()?;
        Ok(syndrome)
    }

    pub fn validate(&self) -> Result<(), AssetErrorV1> {
        if self.syndrome_id.is_empty() || self.asset_id.is_empty() || self.failure_class.is_empty() {
            return Err(AssetErrorV1::InvalidSyndrome(
                "syndrome_id, asset_id, and failure_class must be nonempty".into(),
            ));
        }
        Ok(())
    }
}

/// Append-only failure syndrome store (CAP-004). There is no delete API.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SyndromeStoreV1 {
    pub syndromes: Vec<SyndromeV1>,
}

impl SyndromeStoreV1 {
    pub fn new() -> Self {
        Self {
            syndromes: Vec::new(),
        }
    }

    /// Append one syndrome. Duplicate ids are rejected; nothing is ever
    /// removed.
    pub fn record(&mut self, syndrome: SyndromeV1) -> Result<(), AssetErrorV1> {
        syndrome.validate()?;
        if self
            .syndromes
            .iter()
            .any(|existing| existing.syndrome_id == syndrome.syndrome_id)
        {
            return Err(AssetErrorV1::InvalidSyndrome(format!(
                "duplicate syndrome {}",
                syndrome.syndrome_id
            )));
        }
        self.syndromes.push(syndrome);
        Ok(())
    }

    /// All syndromes recorded for one asset, in recorded order.
    pub fn syndromes_for(&self, asset_id: &str) -> Vec<&SyndromeV1> {
        self.syndromes
            .iter()
            .filter(|syndrome| syndrome.asset_id == asset_id)
            .collect()
    }

    /// Record a syndrome and auto-demote the asset when it is `Promoted` or
    /// `Shadow` (CAP-004: a syndrome on a promoted asset forces demotion).
    pub fn record_for(
        &mut self,
        syndrome: SyndromeV1,
        asset: &mut CapabilityAssetV1,
    ) -> Result<(), AssetErrorV1> {
        self.record(syndrome)?;
        if asset.state == AssetStateV1::Promoted || asset.state == AssetStateV1::Shadow {
            asset.demote()?;
        }
        Ok(())
    }
}

/// Certified unavoidable-work lower bound (METRIC-010): an epoch's benefit
/// may never exceed `actual baseline cost - certified minimum`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CertifiedLowerBoundV1 {
    pub scope: ScopeV1,
    pub minimum_units: u64,
    pub certificate_digest_hex: String,
}

impl CertifiedLowerBoundV1 {
    pub fn new(
        scope: ScopeV1,
        minimum_units: u64,
        certificate_digest_hex: impl Into<String>,
    ) -> Result<Self, AssetErrorV1> {
        let bound = Self {
            scope,
            minimum_units,
            certificate_digest_hex: certificate_digest_hex.into(),
        };
        bound.validate()?;
        Ok(bound)
    }

    pub fn validate(&self) -> Result<(), AssetErrorV1> {
        self.scope.validate()?;
        if !is_lower_hex_64(&self.certificate_digest_hex) {
            return Err(AssetErrorV1::InvalidLedger(format!(
                "certificate_digest_hex must be {} lowercase hex characters",
                ASSET_DIGEST_HEX_LEN_V1
            )));
        }
        Ok(())
    }
}

/// One ledger entry. Capture cost is recorded exactly once (via
/// [`AssetValueLedgerV1::record_capture`]); epoch entries carry benefit and
/// maintenance only.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerEntryV1 {
    pub asset_id: String,
    pub epoch: u64,
    pub benefit_units: u64,
    pub maintenance_units: u64,
    pub capture_units: u64,
}

/// Capability asset lifetime-value ledger (METRIC-009/010).
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssetValueLedgerV1 {
    pub entries: Vec<LedgerEntryV1>,
}

impl AssetValueLedgerV1 {
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// Record the one-time capture cost at the capture epoch.
    pub fn record_capture(&mut self, asset: &CapabilityAssetV1) -> Result<(), AssetErrorV1> {
        self.entries.push(LedgerEntryV1 {
            asset_id: asset.asset_id.clone(),
            epoch: asset.freshness.captured_epoch,
            benefit_units: 0,
            maintenance_units: 0,
            capture_units: asset.capture_cost.capture_units,
        });
        Ok(())
    }

    /// Record one epoch's benefit, clamped to the certified unavoidable-work
    /// lower bound (METRIC-010). A claim above `actual baseline cost -
    /// certified minimum` fails closed with `ClampedBenefit` and the clamped
    /// benefit is what the ledger records. The certified bound's scope must
    /// equal the asset's scope.
    pub fn record_benefit(
        &mut self,
        asset: &CapabilityAssetV1,
        epoch: u64,
        claimed_benefit_units: u64,
        actual_baseline_cost_units: u64,
        certified: &CertifiedLowerBoundV1,
    ) -> Result<u64, AssetErrorV1> {
        if certified.scope != asset.scope {
            return Err(AssetErrorV1::ScopeMismatch(
                "certified lower bound scope must equal the asset scope".into(),
            ));
        }
        let allowed = actual_baseline_cost_units.saturating_sub(certified.minimum_units);
        let clamped = claimed_benefit_units.min(allowed);
        self.entries.push(LedgerEntryV1 {
            asset_id: asset.asset_id.clone(),
            epoch,
            benefit_units: clamped,
            maintenance_units: asset.capture_cost.maintenance_units_per_epoch,
            capture_units: 0,
        });
        if claimed_benefit_units > allowed {
            Err(AssetErrorV1::ClampedBenefit {
                claimed: claimed_benefit_units,
                allowed,
            })
        } else {
            Ok(clamped)
        }
    }

    /// Lifetime value of one asset: benefits minus maintenance minus the
    /// one-time capture cost, across all recorded epochs.
    pub fn lifetime_value(&self, asset_id: &str) -> i128 {
        self.entries
            .iter()
            .filter(|entry| entry.asset_id == asset_id)
            .fold(0i128, |total, entry| {
                total + entry.benefit_units as i128
                    - entry.maintenance_units as i128
                    - entry.capture_units as i128
            })
    }

    /// Assets whose cumulative lifetime value is negative after being
    /// observed across at least `threshold_epochs` distinct epochs
    /// (METRIC-009). Sorted for determinism.
    pub fn retire_negative(&self, threshold_epochs: u64) -> Vec<String> {
        let mut per_asset: std::collections::BTreeMap<String, (std::collections::BTreeSet<u64>, i128)> =
            std::collections::BTreeMap::new();
        for entry in &self.entries {
            let slot = per_asset.entry(entry.asset_id.clone()).or_default();
            slot.0.insert(entry.epoch);
            slot.1 += entry.benefit_units as i128 - entry.maintenance_units as i128 - entry.capture_units as i128;
        }
        per_asset
            .into_iter()
            .filter(|(_, (epochs, value))| {
                epochs.len() as u64 >= threshold_epochs && *value < 0
            })
            .map(|(asset_id, _)| asset_id)
            .collect()
    }

    /// Apply retirement: transition `Shadow`/`Promoted` assets with negative
    /// lifetime value to `Demoted` automatically (METRIC-009). Returns the
    /// ids actually transitioned.
    pub fn apply_retirement(
        &mut self,
        assets: &mut Vec<CapabilityAssetV1>,
        threshold_epochs: u64,
    ) -> Vec<String> {
        let retire = self.retire_negative(threshold_epochs);
        let mut retired = Vec::new();
        for asset in assets.iter_mut() {
            if retire.contains(&asset.asset_id)
                && (asset.state == AssetStateV1::Promoted || asset.state == AssetStateV1::Shadow)
                && asset.demote().is_ok()
            {
                retired.push(asset.asset_id.clone());
            }
        }
        retired
    }
}

#[cfg(test)]
#[path = "../../../tests/rust/zero-gate/unit/asset.rs"]
mod tests;
