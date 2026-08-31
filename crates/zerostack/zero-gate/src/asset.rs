//! Verified procedural capabilities accumulated from accepted episodes.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

pub const ASSET_CONTRACT_VERSION: u16 = 1;
pub const ASSET_ID_MAX_BYTES: usize = 128;
pub const ASSET_DIGEST_HEX_LEN: usize = 64;

/// Fail-closed error for the verified capability subsystem.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssetError {
    InvalidAsset(String),
    InvalidScope(String),
    InvalidPrecondition(String),
    InvalidDependency(String),
    InvalidFreshness(String),
    InvalidTrial(String),
    InvalidSyndrome(String),
    InvalidLedger(String),
    InvalidRevocation(String),
    InvalidTransition { from: AssetState, to: AssetState },
    ScopeMismatch(String),
    ClampedBenefit { claimed: u64, allowed: u64 },
}

impl fmt::Display for AssetError {
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
            Self::InvalidSyndrome(detail) => {
                write!(formatter, "invalid failure syndrome: {detail}")
            }
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

impl Error for AssetError {}

fn is_lower_hex_64(value: &str) -> bool {
    value.len() == ASSET_DIGEST_HEX_LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Exact task-scope descriptor. Scope equality is exact: operation class AND
/// input shape digest must both match.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub operation_class: String,
    pub input_shape_digest_hex: String,
}

impl Scope {
    pub fn new(
        operation_class: impl Into<String>,
        input_shape_digest_hex: impl Into<String>,
    ) -> Result<Self, AssetError> {
        let scope = Self {
            operation_class: operation_class.into(),
            input_shape_digest_hex: input_shape_digest_hex.into(),
        };
        scope.validate()?;
        Ok(scope)
    }

    pub fn validate(&self) -> Result<(), AssetError> {
        if self.operation_class.is_empty() {
            return Err(AssetError::InvalidScope(
                "operation_class must be nonempty".into(),
            ));
        }
        if !is_lower_hex_64(&self.input_shape_digest_hex) {
            return Err(AssetError::InvalidScope(format!(
                "input_shape_digest_hex must be {} lowercase hex characters",
                ASSET_DIGEST_HEX_LEN
            )));
        }
        Ok(())
    }
}

/// A precondition rooted at an exact project-root digest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RootedPrecondition {
    pub name: String,
    pub root_digest_hex: String,
}

impl RootedPrecondition {
    pub fn new(
        name: impl Into<String>,
        root_digest_hex: impl Into<String>,
    ) -> Result<Self, AssetError> {
        let precondition = Self {
            name: name.into(),
            root_digest_hex: root_digest_hex.into(),
        };
        precondition.validate()?;
        Ok(precondition)
    }

    pub fn validate(&self) -> Result<(), AssetError> {
        if self.name.is_empty() {
            return Err(AssetError::InvalidPrecondition(
                "name must be nonempty".into(),
            ));
        }
        if !is_lower_hex_64(&self.root_digest_hex) {
            return Err(AssetError::InvalidPrecondition(format!(
                "root_digest_hex must be {} lowercase hex characters",
                ASSET_DIGEST_HEX_LEN
            )));
        }
        Ok(())
    }
}

/// A dependency pin: the exact digest an asset was verified against.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyPin {
    pub name: String,
    pub digest_hex: String,
}

impl DependencyPin {
    pub fn new(name: impl Into<String>, digest_hex: impl Into<String>) -> Result<Self, AssetError> {
        let pin = Self {
            name: name.into(),
            digest_hex: digest_hex.into(),
        };
        pin.validate()?;
        Ok(pin)
    }

    pub fn validate(&self) -> Result<(), AssetError> {
        if self.name.is_empty() {
            return Err(AssetError::InvalidDependency(
                "name must be nonempty".into(),
            ));
        }
        if !is_lower_hex_64(&self.digest_hex) {
            return Err(AssetError::InvalidDependency(format!(
                "digest_hex must be {} lowercase hex characters",
                ASSET_DIGEST_HEX_LEN
            )));
        }
        Ok(())
    }
}

/// Freshness: an asset is valid for `valid_epochs` from its capture epoch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FreshnessPolicy {
    pub valid_epochs: u64,
    pub captured_epoch: u64,
}

impl FreshnessPolicy {
    pub fn new(valid_epochs: u64, captured_epoch: u64) -> Result<Self, AssetError> {
        let policy = Self {
            valid_epochs,
            captured_epoch,
        };
        policy.validate()?;
        Ok(policy)
    }

    pub fn validate(&self) -> Result<(), AssetError> {
        if self.valid_epochs == 0 {
            return Err(AssetError::InvalidFreshness(
                "valid_epochs must be >= 1".into(),
            ));
        }
        Ok(())
    }
}

/// Complete capture and per-epoch maintenance cost.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Cost {
    pub capture_units: u64,
    pub maintenance_units_per_epoch: u64,
}

/// Lifecycle state. `Captured` is NEVER authoritative; only `Promoted`
/// assets may execute, and only after passing through `Shadow`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetState {
    Captured,
    Shadow,
    Promoted,
    Demoted,
    Revoked,
    Expired,
}

/// The allowed state machine. There is NO path back to `Promoted` except via
/// `Shadow`; `Revoked` is reachable from any state (revocation trigger), and
/// `Demoted`/`Revoked`/`Expired` have no outgoing edges toward `Shadow` or `Promoted`.
pub fn allowed_asset_transition(from: AssetState, to: AssetState) -> bool {
    use AssetState::*;
    match (from, to) {
        (Captured, Shadow) => true,
        (Shadow, Promoted) | (Shadow, Demoted) => true,
        (Promoted, Demoted) | (Promoted, Revoked) | (Promoted, Expired) => true,
        (_, Revoked) => true,
        _ => false,
    }
}

/// A verified capability asset: the 11-field core (scope, preconditions, reads,
/// writes, effects, postconditions, verifier, successor relation, rollback,
/// dependencies, freshness) plus identity, capture cost, lifecycle state, and revocation reason.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityAsset {
    pub contract_version: u16,
    pub asset_id: String,
    pub project_id: String,
    pub scope: Scope,
    pub preconditions: Vec<RootedPrecondition>,
    pub reads: Vec<String>,
    pub writes: Vec<String>,
    pub effects: Vec<String>,
    pub postcondition: String,
    pub verifier_id: String,
    pub successor_relation: String,
    pub rollback: String,
    pub dependencies: Vec<DependencyPin>,
    pub freshness: FreshnessPolicy,
    pub capture_cost: Cost,
    pub state: AssetState,
    pub revocation_reason: Option<String>,
}

impl CapabilityAsset {
    /// Construct a NEW asset. It is born `Captured` and is NEVER
    /// authoritative; no constructor produces `Promoted` without shadow
    /// evaluation.
    #[allow(clippy::too_many_arguments)]
    pub fn captured(
        asset_id: impl Into<String>,
        project_id: impl Into<String>,
        scope: Scope,
        preconditions: Vec<RootedPrecondition>,
        reads: Vec<String>,
        writes: Vec<String>,
        effects: Vec<String>,
        postcondition: impl Into<String>,
        verifier_id: impl Into<String>,
        successor_relation: impl Into<String>,
        rollback: impl Into<String>,
        dependencies: Vec<DependencyPin>,
        freshness: FreshnessPolicy,
        capture_cost: Cost,
    ) -> Result<Self, AssetError> {
        let asset = Self {
            contract_version: ASSET_CONTRACT_VERSION,
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
            state: AssetState::Captured,
            revocation_reason: None,
        };
        asset.validate()?;
        Ok(asset)
    }

    pub fn validate(&self) -> Result<(), AssetError> {
        if self.contract_version != ASSET_CONTRACT_VERSION {
            return Err(AssetError::InvalidAsset(format!(
                "unsupported contract version {}",
                self.contract_version
            )));
        }
        if self.asset_id.is_empty() || self.asset_id.len() > ASSET_ID_MAX_BYTES {
            return Err(AssetError::InvalidAsset(format!(
                "asset_id must be nonempty and at most {} bytes",
                ASSET_ID_MAX_BYTES
            )));
        }
        if self.project_id.is_empty() {
            return Err(AssetError::InvalidAsset(
                "project_id must be nonempty (cross-project isolation key)".into(),
            ));
        }
        self.scope.validate()?;
        let mut precondition_names = std::collections::BTreeSet::new();
        for precondition in &self.preconditions {
            precondition.validate()?;
            if !precondition_names.insert(precondition.name.clone()) {
                return Err(AssetError::InvalidAsset(format!(
                    "duplicate precondition {}",
                    precondition.name
                )));
            }
        }
        for list in [&self.reads, &self.writes, &self.effects] {
            if list.iter().any(|entry| entry.is_empty()) {
                return Err(AssetError::InvalidAsset(
                    "reads/writes/effects entries must be nonempty".into(),
                ));
            }
        }
        if self.postcondition.is_empty() {
            return Err(AssetError::InvalidAsset(
                "postcondition must be nonempty".into(),
            ));
        }
        if self.verifier_id.is_empty() {
            return Err(AssetError::InvalidAsset(
                "verifier_id must be nonempty".into(),
            ));
        }
        if self.successor_relation.is_empty() {
            return Err(AssetError::InvalidAsset(
                "successor_relation must be nonempty (successor-safety obligation)".into(),
            ));
        }
        if self.rollback.is_empty() {
            return Err(AssetError::InvalidAsset(
                "rollback must be nonempty (fallback/rollback path)".into(),
            ));
        }
        let mut dependency_names = std::collections::BTreeSet::new();
        for pin in &self.dependencies {
            pin.validate()?;
            if !dependency_names.insert(pin.name.clone()) {
                return Err(AssetError::InvalidAsset(format!(
                    "duplicate dependency pin {}",
                    pin.name
                )));
            }
        }
        self.freshness.validate()?;
        Ok(())
    }

    fn transition(&mut self, to: AssetState) -> Result<(), AssetError> {
        let from = self.state;
        if !allowed_asset_transition(from, to) {
            return Err(AssetError::InvalidTransition { from, to });
        }
        self.state = to;
        Ok(())
    }

    /// Captured -> Shadow. Entering shadow evaluation.
    pub fn enter_shadow(&mut self) -> Result<(), AssetError> {
        self.transition(AssetState::Shadow)
    }

    /// Shadow -> Promoted, only after a passing promotion decision. Still
    /// runs behind the baseline firewall and verifier gate.
    pub fn promote(&mut self) -> Result<(), AssetError> {
        self.transition(AssetState::Promoted)
    }

    /// -> Demoted (syndrome, negative lifetime value, or decision).
    pub fn demote(&mut self) -> Result<(), AssetError> {
        self.transition(AssetState::Demoted)
    }

    /// Promoted -> Expired (freshness lapse).
    pub fn expire(&mut self) -> Result<(), AssetError> {
        self.transition(AssetState::Expired)
    }

    /// Any state -> Revoked on a revocation trigger. The trigger
    /// and reason are recorded on the asset.
    pub fn revoke_for(
        &mut self,
        trigger: &RevocationTrigger,
        reason: impl Into<String>,
    ) -> Result<(), AssetError> {
        let from = self.state;
        if !allowed_asset_transition(from, AssetState::Revoked) {
            return Err(AssetError::InvalidRevocation(format!(
                "cannot revoke from {from:?}"
            )));
        }
        self.state = AssetState::Revoked;
        self.revocation_reason = Some(format!("{trigger}: {}", reason.into()));
        Ok(())
    }
}

/// Revocation triggers: dependency, contract, verifier, or epoch
/// change.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RevocationTrigger {
    DependencyChange { name: String },
    ContractChange,
    VerifierChange,
    EpochChange,
}

impl fmt::Display for RevocationTrigger {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DependencyChange { name } => write!(formatter, "dependency_change:{name}"),
            Self::ContractChange => write!(formatter, "contract_change"),
            Self::VerifierChange => write!(formatter, "verifier_change"),
            Self::EpochChange => write!(formatter, "epoch_change"),
        }
    }
}

/// Task matching result. Only `Matched` may execute, and only for
/// a `Promoted` asset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MatchOutcome {
    Matched,
    OutOfScope,
    CrossProject,
    StaleDependency { name: String },
    ExpiredFreshness,
}

/// Exact task matching: project equality first (leakage kill), then exact
/// scope equality, then dependency digests, then freshness.
pub fn match_task(
    asset: &CapabilityAsset,
    task_scope: &Scope,
    project_id: &str,
    current_epoch: u64,
    current_dependency_digests: &[(String, String)],
) -> MatchOutcome {
    if asset.project_id != project_id {
        return MatchOutcome::CrossProject;
    }
    if &asset.scope != task_scope {
        return MatchOutcome::OutOfScope;
    }
    for pin in &asset.dependencies {
        let current = current_dependency_digests
            .iter()
            .find(|(name, _)| name == &pin.name);
        match current {
            Some((_, digest)) if digest == &pin.digest_hex => {}
            _ => {
                return MatchOutcome::StaleDependency {
                    name: pin.name.clone(),
                };
            }
        }
    }
    if asset
        .freshness
        .captured_epoch
        .saturating_add(asset.freshness.valid_epochs)
        <= current_epoch
    {
        return MatchOutcome::ExpiredFreshness;
    }
    MatchOutcome::Matched
}

/// One shadow trial comparing the baseline outcome to the shadow outcome
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowTrial {
    pub trial_id: String,
    pub asset_id: String,
    pub baseline_outcome_digest_hex: String,
    pub shadow_outcome_digest_hex: String,
    pub protected_regression: bool,
    pub strict_rescue: bool,
    pub cost_units: u64,
}

impl ShadowTrial {
    pub fn new(
        trial_id: impl Into<String>,
        asset_id: impl Into<String>,
        baseline_outcome_digest_hex: impl Into<String>,
        shadow_outcome_digest_hex: impl Into<String>,
        protected_regression: bool,
        strict_rescue: bool,
        cost_units: u64,
    ) -> Result<Self, AssetError> {
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

    pub fn validate(&self) -> Result<(), AssetError> {
        if self.trial_id.is_empty() || self.asset_id.is_empty() {
            return Err(AssetError::InvalidTrial(
                "trial_id and asset_id must be nonempty".into(),
            ));
        }
        if !is_lower_hex_64(&self.baseline_outcome_digest_hex)
            || !is_lower_hex_64(&self.shadow_outcome_digest_hex)
        {
            return Err(AssetError::InvalidTrial(format!(
                "outcome digests must be {} lowercase hex characters",
                ASSET_DIGEST_HEX_LEN
            )));
        }
        Ok(())
    }
}

/// Promotion report with COMPLETE cost accounting: capture cost,
/// maintenance so far, and all trial costs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionReport {
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
pub enum PromotionDecision {
    Promote {
        report: PromotionReport,
    },
    Deny {
        report: PromotionReport,
        reasons: Vec<String>,
    },
}

/// Shadow-mode promotion. Denies on: no trials, insufficient trials,
/// any protected regression (negative transfer -- demotion demanded), or any
/// miss. One shadow trial counts as one observed epoch for maintenance accounting.
pub fn evaluate_promotion(
    asset: &CapabilityAsset,
    trials: &[ShadowTrial],
    min_trials: u64,
) -> PromotionDecision {
    let observed: Vec<&ShadowTrial> = trials
        .iter()
        .filter(|trial| trial.asset_id == asset.asset_id)
        .collect();
    let trials_observed = observed.len() as u64;
    let misses = observed
        .iter()
        .filter(|trial| trial.baseline_outcome_digest_hex != trial.shadow_outcome_digest_hex)
        .count() as u64;
    let regressions = observed
        .iter()
        .filter(|trial| trial.protected_regression)
        .count() as u64;
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
    let report = PromotionReport {
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
        reasons.push(format!(
            "insufficient_trials:{trials_observed}:{min_trials}"
        ));
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
        PromotionDecision::Promote { report }
    } else {
        PromotionDecision::Deny { report, reasons }
    }
}

/// Use outcome (005). `Executable` exists ONLY for `Promoted` AND
/// `Matched`; every other combination adds cost and returns to baseline,
/// never worsening a protected result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UseOutcome {
    Executable,
    BaselineRequired { added_cost_units: u64 },
}

impl CapabilityAsset {
    /// The ONLY execution admission point. A non-`Promoted` asset (Captured,
    /// Shadow, Demoted, Revoked, Expired) always adds its standing cost and falls
    /// back to the baseline; it can never execute and can never worsen a protected result.
    pub fn use_asset(&self, outcome: &MatchOutcome) -> UseOutcome {
        if self.state == AssetState::Promoted && *outcome == MatchOutcome::Matched {
            UseOutcome::Executable
        } else {
            let added_cost_units = self
                .capture_cost
                .capture_units
                .saturating_add(self.capture_cost.maintenance_units_per_epoch);
            UseOutcome::BaselineRequired { added_cost_units }
        }
    }
}

/// One recorded failure syndrome.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Syndrome {
    pub syndrome_id: String,
    pub asset_id: String,
    pub failure_class: String,
    pub observed_epoch: u64,
    pub detail: String,
}

impl Syndrome {
    pub fn new(
        syndrome_id: impl Into<String>,
        asset_id: impl Into<String>,
        failure_class: impl Into<String>,
        observed_epoch: u64,
        detail: impl Into<String>,
    ) -> Result<Self, AssetError> {
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

    pub fn validate(&self) -> Result<(), AssetError> {
        if self.syndrome_id.is_empty() || self.asset_id.is_empty() || self.failure_class.is_empty()
        {
            return Err(AssetError::InvalidSyndrome(
                "syndrome_id, asset_id, and failure_class must be nonempty".into(),
            ));
        }
        Ok(())
    }
}

/// Append-only failure syndrome store. There is no delete API.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SyndromeStore {
    pub syndromes: Vec<Syndrome>,
}

impl SyndromeStore {
    pub fn new() -> Self {
        Self {
            syndromes: Vec::new(),
        }
    }

    /// Append one syndrome. Duplicate ids are rejected; nothing is ever
    /// removed.
    pub fn record(&mut self, syndrome: Syndrome) -> Result<(), AssetError> {
        syndrome.validate()?;
        if self
            .syndromes
            .iter()
            .any(|existing| existing.syndrome_id == syndrome.syndrome_id)
        {
            return Err(AssetError::InvalidSyndrome(format!(
                "duplicate syndrome {}",
                syndrome.syndrome_id
            )));
        }
        self.syndromes.push(syndrome);
        Ok(())
    }

    /// All syndromes recorded for one asset, in recorded order.
    pub fn syndromes_for(&self, asset_id: &str) -> Vec<&Syndrome> {
        self.syndromes
            .iter()
            .filter(|syndrome| syndrome.asset_id == asset_id)
            .collect()
    }

    /// Record a syndrome and auto-demote the asset when it is `Promoted` or
    /// `Shadow` (a syndrome on a promoted asset forces demotion).
    pub fn record_for(
        &mut self,
        syndrome: Syndrome,
        asset: &mut CapabilityAsset,
    ) -> Result<(), AssetError> {
        self.record(syndrome)?;
        if asset.state == AssetState::Promoted || asset.state == AssetState::Shadow {
            asset.demote()?;
        }
        Ok(())
    }
}

/// Certified unavoidable-work lower bound: an epoch's benefit
/// may never exceed `actual baseline cost - certified minimum`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CertifiedLowerBound {
    pub scope: Scope,
    pub minimum_units: u64,
    pub certificate_digest_hex: String,
}

impl CertifiedLowerBound {
    pub fn new(
        scope: Scope,
        minimum_units: u64,
        certificate_digest_hex: impl Into<String>,
    ) -> Result<Self, AssetError> {
        let bound = Self {
            scope,
            minimum_units,
            certificate_digest_hex: certificate_digest_hex.into(),
        };
        bound.validate()?;
        Ok(bound)
    }

    pub fn validate(&self) -> Result<(), AssetError> {
        self.scope.validate()?;
        if !is_lower_hex_64(&self.certificate_digest_hex) {
            return Err(AssetError::InvalidLedger(format!(
                "certificate_digest_hex must be {} lowercase hex characters",
                ASSET_DIGEST_HEX_LEN
            )));
        }
        Ok(())
    }
}

/// One ledger entry. Capture cost is recorded exactly once (via
/// [`AssetValueLedger::record_capture`]); epoch entries carry benefit and
/// maintenance only.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerEntry {
    pub asset_id: String,
    pub epoch: u64,
    pub benefit_units: u64,
    pub maintenance_units: u64,
    pub capture_units: u64,
}

/// Capability asset lifetime-value ledger (010).
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssetValueLedger {
    pub entries: Vec<LedgerEntry>,
}

impl AssetValueLedger {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Record the one-time capture cost at the capture epoch.
    pub fn record_capture(&mut self, asset: &CapabilityAsset) -> Result<(), AssetError> {
        self.entries.push(LedgerEntry {
            asset_id: asset.asset_id.clone(),
            epoch: asset.freshness.captured_epoch,
            benefit_units: 0,
            maintenance_units: 0,
            capture_units: asset.capture_cost.capture_units,
        });
        Ok(())
    }

    /// Record one epoch's benefit, clamped to the certified unavoidable-work lower bound.
    /// A claim above `actual baseline cost certified minimum` fails closed with `ClampedBenefit` and
    /// the clamped benefit is what the ledger records.
    pub fn record_benefit(
        &mut self,
        asset: &CapabilityAsset,
        epoch: u64,
        claimed_benefit_units: u64,
        actual_baseline_cost_units: u64,
        certified: &CertifiedLowerBound,
    ) -> Result<u64, AssetError> {
        if certified.scope != asset.scope {
            return Err(AssetError::ScopeMismatch(
                "certified lower bound scope must equal the asset scope".into(),
            ));
        }
        let allowed = actual_baseline_cost_units.saturating_sub(certified.minimum_units);
        let clamped = claimed_benefit_units.min(allowed);
        self.entries.push(LedgerEntry {
            asset_id: asset.asset_id.clone(),
            epoch,
            benefit_units: clamped,
            maintenance_units: asset.capture_cost.maintenance_units_per_epoch,
            capture_units: 0,
        });
        if claimed_benefit_units > allowed {
            Err(AssetError::ClampedBenefit {
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
    /// . Sorted for determinism.
    pub fn retire_negative(&self, threshold_epochs: u64) -> Vec<String> {
        let mut per_asset: std::collections::BTreeMap<
            String,
            (std::collections::BTreeSet<u64>, i128),
        > = std::collections::BTreeMap::new();
        for entry in &self.entries {
            let slot = per_asset.entry(entry.asset_id.clone()).or_default();
            slot.0.insert(entry.epoch);
            slot.1 += entry.benefit_units as i128
                - entry.maintenance_units as i128
                - entry.capture_units as i128;
        }
        per_asset
            .into_iter()
            .filter(|(_, (epochs, value))| epochs.len() as u64 >= threshold_epochs && *value < 0)
            .map(|(asset_id, _)| asset_id)
            .collect()
    }

    /// Apply retirement: transition `Shadow`/`Promoted` assets with negative
    /// lifetime value to `Demoted` automatically. Returns the
    /// ids actually transitioned.
    pub fn apply_retirement(
        &mut self,
        assets: &mut Vec<CapabilityAsset>,
        threshold_epochs: u64,
    ) -> Vec<String> {
        let retire = self.retire_negative(threshold_epochs);
        let mut retired = Vec::new();
        for asset in assets.iter_mut() {
            if retire.contains(&asset.asset_id)
                && (asset.state == AssetState::Promoted || asset.state == AssetState::Shadow)
                && asset.demote().is_ok()
            {
                retired.push(asset.asset_id.clone());
            }
        }
        retired
    }
}

/// Hub-side CAS read gate: a verified capability asset names the exact content it
/// authorizes, and reads through the gate are refused fail-closed BEFORE any object lookup.
#[derive(Clone, Debug)]
pub struct CasCapabilityGate {
    asset: CapabilityAsset,
    caller_project_id: String,
    current_epoch: u64,
}

impl CasCapabilityGate {
    pub fn new(
        asset: CapabilityAsset,
        caller_project_id: impl Into<String>,
        current_epoch: u64,
    ) -> Result<Self, AssetError> {
        let caller_project_id = caller_project_id.into();
        if caller_project_id.is_empty() {
            return Err(AssetError::InvalidAsset(
                "caller project id must be nonempty (cross-project isolation key)".into(),
            ));
        }
        Ok(Self {
            asset,
            caller_project_id,
            current_epoch,
        })
    }

    pub fn asset(&self) -> &CapabilityAsset {
        &self.asset
    }

    pub fn caller_project_id(&self) -> &str {
        &self.caller_project_id
    }

    pub const fn current_epoch(&self) -> u64 {
        self.current_epoch
    }
}

impl zero_store::CasReadGate for CasCapabilityGate {
    fn authorize_read(&self, sha256: &str) -> Result<(), zero_store::CasError> {
        // Project equality first (leakage kill): a caller may guess
        // another project's root hash, but the gate refuses before lookup.
        if self.caller_project_id != self.asset.project_id {
            return Err(zero_store::CasError::PolicyDenied(format!(
                "capability gate: cross-project read refused (caller '{}', asset '{}')",
                self.caller_project_id, self.asset.project_id
            )));
        }
        // Only a promoted asset is authoritative for reads.
        if self.asset.state != AssetState::Promoted {
            return Err(zero_store::CasError::PolicyDenied(format!(
                "capability gate: asset state {:?} is not promoted",
                self.asset.state
            )));
        }
        // Exact content binding: the asset authorizes exactly the content
        // hash in its scope; mismatched content is refused fail-loud.
        if sha256 != self.asset.scope.input_shape_digest_hex {
            return Err(zero_store::CasError::PolicyDenied(format!(
                "capability gate: content {} not authorized (asset scope {})",
                &sha256[..sha256.len().min(16)],
                &self.asset.scope.input_shape_digest_hex[..16]
            )));
        }
        // Freshness: a stale capability is refused even when the state
        // machine still says Promoted.
        if self
            .asset
            .freshness
            .captured_epoch
            .saturating_add(self.asset.freshness.valid_epochs)
            <= self.current_epoch
        {
            return Err(zero_store::CasError::PolicyDenied(format!(
                "capability gate: stale capability (captured {}, valid {} epochs, epoch {})",
                self.asset.freshness.captured_epoch,
                self.asset.freshness.valid_epochs,
                self.current_epoch
            )));
        }
        Ok(())
    }
}
