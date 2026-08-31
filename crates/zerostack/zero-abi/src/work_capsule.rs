//! Provider-neutral Work Capsule, turn, interrupt, governor, and promotion
//! contracts. Every transition and promotion decision fails closed. Roots bind
//! semantics; FSZero remains the byte-storage authority and no contract selects a model.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::digest::sha256_hex;
use crate::schema::canonical_json;

const ROOT_HEX_LEN: usize = 64;

pub(crate) fn valid_root(root: &str) -> bool {
    root.len() == ROOT_HEX_LEN
        && root
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CapsuleRoots {
    pub project: String,
    pub task: String,
    pub protected_scope: String,
    pub obligations: String,
    pub evidence: String,
    pub policy: String,
    pub execution: String,
    pub verifier: String,
    pub fallback: String,
    pub ledger: String,
}

impl CapsuleRoots {
    pub fn validate(&self) -> Result<(), String> {
        for (name, root) in [
            ("project", &self.project),
            ("task", &self.task),
            ("protected_scope", &self.protected_scope),
            ("obligations", &self.obligations),
            ("evidence", &self.evidence),
            ("policy", &self.policy),
            ("execution", &self.execution),
            ("verifier", &self.verifier),
            ("fallback", &self.fallback),
            ("ledger", &self.ledger),
        ] {
            if !valid_root(root) {
                return Err(format!(
                    "capsule {name} root must be 64 lowercase hexadecimal characters"
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapsuleState {
    Draft,
    EvidenceComplete,
    PolicyComplete,
    InterruptRequired,
    Executable,
    ExecutedInSandbox,
    Verified,
    BudgetAccepted,
    Committed,
}

impl CapsuleState {
    pub const fn allows(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Draft, Self::EvidenceComplete)
                | (
                    Self::EvidenceComplete,
                    Self::PolicyComplete | Self::InterruptRequired
                )
                | (Self::InterruptRequired, Self::PolicyComplete)
                | (Self::PolicyComplete, Self::Executable)
                | (Self::Executable, Self::ExecutedInSandbox)
                | (Self::ExecutedInSandbox, Self::Verified)
                | (Self::Verified, Self::BudgetAccepted)
                | (Self::BudgetAccepted, Self::Committed)
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkCapsule {
    pub version: u32,
    pub roots: CapsuleRoots,
    pub state: CapsuleState,
    pub epoch: u64,
    pub provider_usage_budget: u64,
    pub complete_work_budget: u64,
}

impl WorkCapsule {
    pub fn validate(&self) -> Result<(), String> {
        if self.version == 0 {
            return Err("capsule version must be positive".into());
        }
        if self.epoch == 0 {
            return Err("capsule epoch must be positive".into());
        }
        self.roots.validate()
    }

    /// Deterministic Draft genesis. Identical full inputs (roots, epoch, and both budgets) always
    /// produce the same capsule and the same capsule root.
    pub fn draft(
        roots: CapsuleRoots,
        epoch: u64,
        provider_usage_budget: u64,
        complete_work_budget: u64,
    ) -> Result<Self, String> {
        let capsule = Self {
            version: 1,
            roots,
            state: CapsuleState::Draft,
            epoch,
            provider_usage_budget,
            complete_work_budget,
        };
        capsule.validate()?;
        Ok(capsule)
    }

    pub fn root(&self) -> Result<String, String> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(|error| error.to_string())?;
        Ok(sha256_hex(canonical_json(&value).as_bytes()))
    }

    /// Successor law. `next` is a legal successor of `self` only when both capsules validate (version
    /// nonzero, epoch positive), the epoch is nondecreasing, the state edge is a legal [`CapsuleState`]
    /// edge, and the immutable roots (project,, protected_scope, fallback) are unchanged.
    pub fn validate_successor(&self, next: &Self) -> Result<(), String> {
        self.validate()?;
        next.validate()?;
        if next.epoch < self.epoch {
            return Err("capsule successor epoch must not decrease".into());
        }
        if !self.state.allows(next.state) {
            return Err(format!(
                "illegal capsule transition {:?} -> {:?}",
                self.state, next.state
            ));
        }
        if next.roots.project != self.roots.project
            || next.roots.task != self.roots.task
            || next.roots.protected_scope != self.roots.protected_scope
            || next.roots.fallback != self.roots.fallback
        {
            return Err(
                "capsule successor must not mutate project, task, protected_scope, or fallback roots"
                    .into(),
            );
        }
        Ok(())
    }

    /// Advance to `next` under the successor law, bumping the epoch. On any
    /// law violation the capsule is left unchanged.
    pub fn advance(&mut self, next: CapsuleState) -> Result<(), String> {
        let mut successor = self.clone();
        successor.state = next;
        successor.epoch = successor
            .epoch
            .checked_add(1)
            .ok_or_else(|| "capsule epoch overflow".to_string())?;
        self.validate_successor(&successor)?;
        *self = successor;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnClass {
    SemanticDecision,
    Mechanical,
    RetryRepair,
    Verification,
    UserPreference,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TurnMetadata {
    pub class: TurnClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_count: Option<u32>,
}

impl TurnMetadata {
    pub const fn native() -> Self {
        Self {
            class: TurnClass::Mechanical,
            retry_count: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TurnRecord {
    pub sequence: u64,
    pub class: TurnClass,
    pub operation_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_count: Option<u32>,
    pub resource_ledger_root: String,
    pub trace_root: String,
}

impl TurnRecord {
    pub fn validate(&self) -> Result<(), String> {
        if self.sequence == 0
            || !valid_root(&self.resource_ledger_root)
            || !valid_root(&self.trace_root)
        {
            return Err("turn record has invalid sequence or roots".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MechanicalVerdict {
    Safe,
    Unsafe,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MechanicalEvidence {
    pub deterministic: bool,
    pub effects_verified: bool,
    pub bounded: bool,
    pub cancellable: bool,
    pub transactional: bool,
    pub proof_complete: bool,
    pub native_fallback_available: bool,
    pub has_unresolved_choice: bool,
}

impl MechanicalEvidence {
    pub const fn verdict(&self) -> MechanicalVerdict {
        if self.has_unresolved_choice {
            MechanicalVerdict::Unsafe
        } else if self.deterministic
            && self.effects_verified
            && self.bounded
            && self.cancellable
            && self.transactional
            && (self.proof_complete || self.native_fallback_available)
        {
            MechanicalVerdict::Safe
        } else {
            MechanicalVerdict::Unknown
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ZeroDominanceProof {
    pub capsule_root: String,
    pub ledger_root: String,
    pub baseline_output_root: String,
    pub zero_output_root: String,
    pub protected_regressions: u32,
    pub correctness_complete: bool,
    pub baseline_visible_tokens: u64,
    pub zero_visible_tokens: u64,
    pub baseline_complete_work: u64,
    pub zero_complete_work: u64,
}

impl ZeroDominanceProof {
    pub fn validate(&self) -> Result<(), String> {
        for root in [
            &self.capsule_root,
            &self.ledger_root,
            &self.baseline_output_root,
            &self.zero_output_root,
        ] {
            if !valid_root(root) {
                return Err("Zero dominance proof carries an invalid root".into());
            }
        }
        if self.baseline_visible_tokens == 0 || self.baseline_complete_work == 0 {
            return Err("Zero dominance proof requires positive baseline measurements".into());
        }
        Ok(())
    }

    pub fn permits_native_elision(&self) -> bool {
        self.validate().is_ok()
            && self.correctness_complete
            && self.baseline_output_root == self.zero_output_root
            && self.protected_regressions == 0
            && self.zero_visible_tokens < self.baseline_visible_tokens
            && self.zero_complete_work <= self.baseline_complete_work
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticInterruptKind {
    UserPreferenceRequired,
    ArchitectureChoice,
    UncoveredObservation,
    ProtectedTradeoff,
    NovelGenerationRequired,
    VerifierUnknown,
    PermissionRequired,
    NativeEscapeRequested,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SemanticInterrupt {
    pub id: String,
    pub kind: SemanticInterruptKind,
    pub capsule_root: String,
    pub obligation_root: String,
    pub decision_frontier_root: String,
    pub evidence_view_root: String,
    pub reasoning_contract_root: String,
    pub continuation_root: String,
    pub exact_handles: Vec<String>,
    pub budget_impact: u64,
}

impl SemanticInterrupt {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() {
            return Err("interrupt id must not be empty".into());
        }
        for root in [
            &self.capsule_root,
            &self.obligation_root,
            &self.decision_frontier_root,
            &self.evidence_view_root,
            &self.reasoning_contract_root,
            &self.continuation_root,
        ] {
            if !valid_root(root) {
                return Err("semantic interrupt carries an invalid root".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleAction {
    ContinueMechanical,
    EmitSemanticInterrupt,
    NativeEscape,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InterruptSchedule {
    pub action: ScheduleAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupt_id: Option<String>,
    pub reserved_native_budget: u64,
    pub reason: String,
}

pub fn schedule_next(
    mechanical: MechanicalVerdict,
    pending: &[SemanticInterrupt],
    available_budget: u64,
    reserved_native_budget: u64,
    native_strategy_available: bool,
    dominance: Option<&ZeroDominanceProof>,
) -> Result<InterruptSchedule, String> {
    let proof_elides_native = mechanical == MechanicalVerdict::Safe
        && pending.is_empty()
        && dominance.is_some_and(ZeroDominanceProof::permits_native_elision);
    if proof_elides_native {
        return Ok(InterruptSchedule {
            action: ScheduleAction::ContinueMechanical,
            interrupt_id: None,
            reserved_native_budget: 0,
            reason: "certified parity and lower token use make the native path dominated".into(),
        });
    }
    let effective_reserve = if native_strategy_available {
        reserved_native_budget
    } else {
        0
    };
    if effective_reserve > available_budget {
        return Err("native reserve exceeds available budget".into());
    }
    if let Some(interrupt) = pending.first() {
        interrupt.validate()?;
        let semantic_budget = available_budget - effective_reserve;
        if interrupt.budget_impact <= semantic_budget {
            return Ok(InterruptSchedule {
                action: ScheduleAction::EmitSemanticInterrupt,
                interrupt_id: Some(interrupt.id.clone()),
                reserved_native_budget: effective_reserve,
                reason: "pending semantic choice fits outside the native reserve".into(),
            });
        }
        if native_strategy_available {
            return Ok(InterruptSchedule {
                action: ScheduleAction::NativeEscape,
                interrupt_id: None,
                reserved_native_budget: effective_reserve,
                reason: "semantic interrupt would consume the native reserve".into(),
            });
        }
        return Err("semantic interrupt exceeds budget and no native strategy is available".into());
    }
    if !native_strategy_available {
        return Err(
            "native strategy may be omitted only when Zero proves parity and dominance".into(),
        );
    }
    let (action, reason) = match mechanical {
        MechanicalVerdict::Safe => (
            ScheduleAction::ContinueMechanical,
            "mechanical continuation is certified safe",
        ),
        MechanicalVerdict::Unsafe | MechanicalVerdict::Unknown => (
            ScheduleAction::NativeEscape,
            "mechanical continuation is not certified safe",
        ),
    };
    Ok(InterruptSchedule {
        action,
        interrupt_id: None,
        reserved_native_budget: effective_reserve,
        reason: reason.into(),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GovernorRegime {
    Direct,
    Reuse,
    Mechanical,
    Dialect,
    SemanticInterrupt,
    Review,
    Baseline,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GovernorInput {
    pub reuse_valid: bool,
    pub mechanical: MechanicalVerdict,
    pub dialect_verified: bool,
    pub semantic_choice_required: bool,
    pub saved_budget_available: bool,
    pub baseline_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zero_dominance: Option<ZeroDominanceProof>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GovernorDecision {
    pub regime: GovernorRegime,
    pub reason: String,
    pub baseline_reserved: bool,
    pub fallback_elided: bool,
}

pub fn choose_regime(input: &GovernorInput) -> GovernorDecision {
    let (regime, reason) = if input.semantic_choice_required {
        (
            GovernorRegime::SemanticInterrupt,
            "protected semantic choice requires escalation",
        )
    } else if input.reuse_valid {
        (GovernorRegime::Reuse, "exact reusable result remains valid")
    } else if input.mechanical == MechanicalVerdict::Safe {
        (
            GovernorRegime::Mechanical,
            "mechanical subgraph is certified safe",
        )
    } else if input.dialect_verified {
        (GovernorRegime::Dialect, "verified project operator applies")
    } else if input.saved_budget_available {
        (
            GovernorRegime::Review,
            "saved budget funds additional review",
        )
    } else if input.baseline_available {
        (
            GovernorRegime::Baseline,
            "optimized evidence is insufficient",
        )
    } else {
        (
            GovernorRegime::Direct,
            "direct execution is the only bounded route",
        )
    };
    let zero_route = matches!(
        regime,
        GovernorRegime::Reuse | GovernorRegime::Mechanical | GovernorRegime::Dialect
    );
    let fallback_elided = zero_route
        && input
            .zero_dominance
            .as_ref()
            .is_some_and(ZeroDominanceProof::permits_native_elision);
    GovernorDecision {
        regime,
        reason: if fallback_elided {
            "certified parity and lower token use make the baseline dominated".into()
        } else {
            reason.into()
        },
        baseline_reserved: input.baseline_available && !fallback_elided,
        fallback_elided,
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PromotionInputs {
    pub baseline_output_root: String,
    pub candidate_output_root: String,
    pub protected_regressions: u32,
    pub declared_resources: BTreeMap<String, u64>,
    pub observed_resources: BTreeMap<String, u64>,
    pub declared_model_calls: u64,
    pub observed_model_calls: u64,
    pub injected_faults: u32,
    pub contained_faults: u32,
    pub rollback_root_before: String,
    pub rollback_root_after: String,
}

impl PromotionInputs {
    pub fn evaluate(&self) -> Result<PromotionEvidence, String> {
        for root in [
            &self.baseline_output_root,
            &self.candidate_output_root,
            &self.rollback_root_before,
            &self.rollback_root_after,
        ] {
            if !valid_root(root) {
                return Err("promotion evidence carries an invalid root".into());
            }
        }
        if self.declared_resources.is_empty()
            || self
                .declared_resources
                .keys()
                .chain(self.observed_resources.keys())
                .any(String::is_empty)
        {
            return Err("promotion resource coordinates must be nonempty".into());
        }
        let model_calls_reconciled = self.declared_model_calls == self.observed_model_calls;
        Ok(PromotionEvidence {
            exact_parity: self.baseline_output_root == self.candidate_output_root,
            protected_regressions: self.protected_regressions,
            complete_resource_reconciled: self.declared_resources == self.observed_resources,
            hidden_model_calls: self
                .observed_model_calls
                .saturating_sub(self.declared_model_calls),
            model_calls_reconciled,
            fault_injection_passed: self.injected_faults > 0
                && self.injected_faults == self.contained_faults,
            rollback_passed: self.rollback_root_before == self.rollback_root_after,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PromotionEvidence {
    pub exact_parity: bool,
    pub protected_regressions: u32,
    pub complete_resource_reconciled: bool,
    pub hidden_model_calls: u64,
    pub model_calls_reconciled: bool,
    pub fault_injection_passed: bool,
    pub rollback_passed: bool,
}

impl PromotionEvidence {
    pub const fn permits_promotion(&self) -> bool {
        self.exact_parity
            && self.protected_regressions == 0
            && self.complete_resource_reconciled
            && self.hidden_model_calls == 0
            && self.model_calls_reconciled
            && self.fault_injection_passed
            && self.rollback_passed
    }
}
