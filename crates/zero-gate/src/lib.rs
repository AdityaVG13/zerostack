#![forbid(unsafe_code)]

//! Pure proof-carrying policy and two-phase execution gate.
//!
//! Passing task receipts and two-phase permits are privacy-preserving linear capabilities.
//! Candidate bytes and staged effects are released only by `CommitReceipt::publish` after G8/G9.
//!
//! A sandbox attempt cannot commit without a passing receipt:
//! ~~~compile_fail
//! use zero_abi::raw_worker::EffectClass;
//! use zero_cert::CommandId;
//! use zero_gate::{begin_task_attempt, TaskRunEvidence};
//! let evidence = TaskRunEvidence::new(7, CommandId(1), [2; 32], 0, vec![[3; 32]], vec![[3; 32]], [4; 32], 5);
//! let attempt = begin_task_attempt(EffectClass::ReversibleMutation, evidence).unwrap();
//! let _ = attempt.commit(b"forged");
//! ~~~

use serde::{Deserialize, Serialize};
use std::fmt;
use zero_abi::raw_worker::EffectClass;
use zero_cert::{CommandId, VerifiedEvidence};

pub mod aggregate;
pub mod deoptimization;
pub mod durable_publication;
pub mod evidence;
pub mod invalidation;
pub mod program;
pub mod q99;
pub mod quality;
pub mod real_gc;
pub mod recovery;
pub mod reinvestment;
pub mod semantic_cut;
pub mod transaction;
pub mod two_phase;
pub mod verdict;
pub use aggregate::*;
pub use deoptimization::*;
pub use durable_publication::*;
pub use evidence::*;
pub use invalidation::*;
pub use program::*;
pub use q99::*;
pub use quality::*;
pub use real_gc::*;
pub use recovery::*;
pub use reinvestment::*;
pub use semantic_cut::*;
pub use transaction::*;
pub use two_phase::*;
pub use verdict::*;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NextBudget {
    budget: u128,
    round: u32,
}
impl NextBudget {
    pub fn budget(self) -> u128 {
        self.budget
    }
    pub fn round(self) -> u32 {
        self.round
    }
    #[doc(hidden)]
    pub fn new_for_doctest(budget: u128) -> Self {
        Self { budget, round: 0 }
    }
}

#[derive(Debug)]
pub struct PolicySufficiencyWitness<'certificate, 'payload> {
    evidence: VerifiedEvidence<'certificate, 'payload>,
}
impl<'certificate, 'payload> PolicySufficiencyWitness<'certificate, 'payload> {
    pub fn evidence(&self) -> &VerifiedEvidence<'certificate, 'payload> {
        &self.evidence
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskOutcome {
    Passed,
}

pub const MAX_TASK_ARTIFACTS: usize = 64;

/// Actual, bounded verifier-run evidence. This is input, never a proof.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskRunEvidence {
    task_id: u64,
    verifier: CommandId,
    verifier_environment_digest: [u8; 32],
    exit_code: i32,
    expected_artifact_digests: Vec<[u8; 32]>,
    observed_artifact_digests: Vec<[u8; 32]>,
    journal_id: [u8; 32],
    attempt_cost: u64,
}
impl TaskRunEvidence {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        task_id: u64,
        verifier: CommandId,
        verifier_environment_digest: [u8; 32],
        exit_code: i32,
        expected_artifact_digests: Vec<[u8; 32]>,
        observed_artifact_digests: Vec<[u8; 32]>,
        journal_id: [u8; 32],
        attempt_cost: u64,
    ) -> Self {
        Self {
            task_id,
            verifier,
            verifier_environment_digest,
            exit_code,
            expected_artifact_digests,
            observed_artifact_digests,
            journal_id,
            attempt_cost,
        }
    }
    pub fn task_id(&self) -> u64 {
        self.task_id
    }
    pub fn verifier(&self) -> CommandId {
        self.verifier
    }
    pub fn verifier_environment_digest(&self) -> &[u8; 32] {
        &self.verifier_environment_digest
    }
    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }
    pub fn expected_artifact_digests(&self) -> &[[u8; 32]] {
        &self.expected_artifact_digests
    }
    pub fn observed_artifact_digests(&self) -> &[[u8; 32]] {
        &self.observed_artifact_digests
    }
    pub fn journal_id(&self) -> &[u8; 32] {
        &self.journal_id
    }
    pub fn attempt_cost(&self) -> u64 {
        self.attempt_cost
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpeculationError {
    IrreversibleEffect,
    ZeroAttemptCost,
    TooManyArtifacts { count: usize, maximum: usize },
}
impl fmt::Display for SpeculationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for SpeculationError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskVerifierError {
    UntrustedRunEvidence,
}

pub trait TaskAcceptanceVerifier {
    fn verify_run(&self, evidence: &TaskRunEvidence) -> Result<(), TaskVerifierError>;
}

/// Active journal-scoped attempt. It has no commit operation.
#[derive(Debug)]
pub struct SandboxAttempt {
    evidence: TaskRunEvidence,
}

pub fn begin_task_attempt(
    effect_class: EffectClass,
    evidence: TaskRunEvidence,
) -> Result<SandboxAttempt, SpeculationError> {
    if effect_class == EffectClass::Irreversible {
        return Err(SpeculationError::IrreversibleEffect);
    }
    if evidence.attempt_cost == 0 {
        return Err(SpeculationError::ZeroAttemptCost);
    }
    let count = evidence
        .expected_artifact_digests
        .len()
        .max(evidence.observed_artifact_digests.len());
    if count > MAX_TASK_ARTIFACTS {
        return Err(SpeculationError::TooManyArtifacts {
            count,
            maximum: MAX_TASK_ARTIFACTS,
        });
    }
    Ok(SandboxAttempt { evidence })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskAcceptanceError {
    VerifierRejected(TaskVerifierError),
    NonZeroOutcome {
        exit_code: i32,
    },
    ArtifactCountMismatch {
        expected: usize,
        observed: usize,
    },
    ArtifactMismatch {
        index: usize,
        expected: [u8; 32],
        observed: [u8; 32],
    },
}
impl fmt::Display for TaskAcceptanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for TaskAcceptanceError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RollbackReason {
    MissingReceipt,
    VerificationFailed(TaskAcceptanceError),
}

#[derive(Debug)]
pub struct RawRollback {
    task_id: u64,
    journal_id: [u8; 32],
    attempt_cost: u64,
    reason: RollbackReason,
}
impl RawRollback {
    pub fn task_id(&self) -> u64 {
        self.task_id
    }
    pub fn journal_id(&self) -> &[u8; 32] {
        &self.journal_id
    }
    pub fn attempt_cost(&self) -> u64 {
        self.attempt_cost
    }
    pub fn reason(&self) -> RollbackReason {
        self.reason
    }
}
impl SandboxAttempt {
    pub fn rollback_missing_receipt(self) -> RawRollback {
        RawRollback {
            task_id: self.evidence.task_id,
            journal_id: self.evidence.journal_id,
            attempt_cost: self.evidence.attempt_cost,
            reason: RollbackReason::MissingReceipt,
        }
    }
}

#[derive(Debug)]
pub struct TaskAcceptanceFailure {
    reason: TaskAcceptanceError,
    rollback: RawRollback,
}
impl TaskAcceptanceFailure {
    pub fn reason(&self) -> TaskAcceptanceError {
        self.reason
    }
    pub fn rollback(&self) -> &RawRollback {
        &self.rollback
    }
    pub fn into_rollback(self) -> RawRollback {
        self.rollback
    }
}

/// Opaque linear objective proof. It is neither Clone nor Deserialize and all fields are private.
///
/// ~~~compile_fail
/// use zero_cert::CommandId;
/// use zero_gate::{TaskAcceptanceReceipt, TaskOutcome};
/// let _ = TaskAcceptanceReceipt { task_id: 7, verifier: CommandId(1), verifier_environment_digest: [0; 32], outcome: TaskOutcome::Passed, exit_code: 0, expected_artifact_digests: vec![], observed_artifact_digests: vec![], journal_id: [0; 32], attempt_cost: 1 };
/// ~~~
#[derive(Debug)]
pub struct TaskAcceptanceReceipt {
    task_id: u64,
    verifier: CommandId,
    verifier_environment_digest: [u8; 32],
    outcome: TaskOutcome,
    exit_code: i32,
    expected_artifact_digests: Vec<[u8; 32]>,
    observed_artifact_digests: Vec<[u8; 32]>,
    journal_id: [u8; 32],
    attempt_cost: u64,
}
impl TaskAcceptanceReceipt {
    pub fn task_id(&self) -> u64 {
        self.task_id
    }
    pub fn verifier(&self) -> CommandId {
        self.verifier
    }
    pub fn verifier_environment_digest(&self) -> &[u8; 32] {
        &self.verifier_environment_digest
    }
    pub fn outcome(&self) -> TaskOutcome {
        self.outcome
    }
    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }
    pub fn expected_artifact_digests(&self) -> &[[u8; 32]] {
        &self.expected_artifact_digests
    }
    pub fn observed_artifact_digests(&self) -> &[[u8; 32]] {
        &self.observed_artifact_digests
    }
    pub fn journal_id(&self) -> &[u8; 32] {
        &self.journal_id
    }
    pub fn attempt_cost(&self) -> u64 {
        self.attempt_cost
    }
}

#[derive(Debug)]
pub struct VerifiedTaskAttempt {
    receipt: TaskAcceptanceReceipt,
}
impl VerifiedTaskAttempt {
    pub fn receipt(&self) -> &TaskAcceptanceReceipt {
        &self.receipt
    }
    pub fn into_receipt(self) -> TaskAcceptanceReceipt {
        self.receipt
    }
}

#[allow(clippy::result_large_err)]
pub fn verify_task_acceptance<V: TaskAcceptanceVerifier + ?Sized>(
    verifier: &V,
    attempt: SandboxAttempt,
) -> Result<VerifiedTaskAttempt, TaskAcceptanceFailure> {
    let evidence = attempt.evidence;
    let reason = if evidence.exit_code != 0 {
        Some(TaskAcceptanceError::NonZeroOutcome {
            exit_code: evidence.exit_code,
        })
    } else if evidence.expected_artifact_digests.len() != evidence.observed_artifact_digests.len() {
        Some(TaskAcceptanceError::ArtifactCountMismatch {
            expected: evidence.expected_artifact_digests.len(),
            observed: evidence.observed_artifact_digests.len(),
        })
    } else if let Some((index, (expected, observed))) = evidence
        .expected_artifact_digests
        .iter()
        .zip(&evidence.observed_artifact_digests)
        .enumerate()
        .find(|(_, (expected, observed))| expected != observed)
    {
        Some(TaskAcceptanceError::ArtifactMismatch {
            index,
            expected: *expected,
            observed: *observed,
        })
    } else {
        verifier
            .verify_run(&evidence)
            .err()
            .map(TaskAcceptanceError::VerifierRejected)
    };
    if let Some(reason) = reason {
        let rollback = RawRollback {
            task_id: evidence.task_id,
            journal_id: evidence.journal_id,
            attempt_cost: evidence.attempt_cost,
            reason: RollbackReason::VerificationFailed(reason),
        };
        return Err(TaskAcceptanceFailure { reason, rollback });
    }
    Ok(VerifiedTaskAttempt {
        receipt: TaskAcceptanceReceipt {
            task_id: evidence.task_id,
            verifier: evidence.verifier,
            verifier_environment_digest: evidence.verifier_environment_digest,
            outcome: TaskOutcome::Passed,
            exit_code: evidence.exit_code,
            expected_artifact_digests: evidence.expected_artifact_digests,
            observed_artifact_digests: evidence.observed_artifact_digests,
            journal_id: evidence.journal_id,
            attempt_cost: evidence.attempt_cost,
        },
    })
}

#[derive(Debug)]
pub enum DecisionGate<'certificate, 'payload> {
    Certified(PolicySufficiencyWitness<'certificate, 'payload>),
    TaskVerified(TaskAcceptanceReceipt),
    Expand(NextBudget),
    RawFallback,
}

#[derive(Debug)]
pub struct GateInput<'certificate, 'payload> {
    pub effect_class: EffectClass,
    pub required_budget: u128,
    pub verified_evidence: Option<VerifiedEvidence<'certificate, 'payload>>,
    pub task_receipt: Option<TaskAcceptanceReceipt>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum GatePhase {
    Active,
    Terminal,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GateState {
    initial_budget: u128,
    current_budget: u128,
    cumulative_visible_cost: u128,
    rounds: u32,
    phase: GatePhase,
}
impl GateState {
    pub fn new(initial_budget: u128) -> Result<Self, GateError> {
        if initial_budget == 0 {
            return Err(GateError::ZeroInitialBudget);
        }
        Ok(Self {
            initial_budget,
            current_budget: initial_budget,
            cumulative_visible_cost: initial_budget,
            rounds: 1,
            phase: GatePhase::Active,
        })
    }
    pub fn initial_budget(self) -> u128 {
        self.initial_budget
    }
    pub fn current_budget(self) -> u128 {
        self.current_budget
    }
    pub fn cumulative_visible_cost(self) -> u128 {
        self.cumulative_visible_cost
    }
    pub fn rounds(self) -> u32 {
        self.rounds
    }
    pub fn phase(self) -> GatePhase {
        self.phase
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum GateError {
    ZeroInitialBudget,
    TerminalState,
    ConflictingProofs,
    IrreversibleSpeculation,
    BudgetOverflow,
    RoundOverflow,
    ZeroHindsightBudget,
    BoundOverflow,
}
impl fmt::Display for GateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for GateError {}

#[allow(clippy::result_large_err)]
pub fn decide<'certificate, 'payload>(
    state: GateState,
    input: GateInput<'certificate, 'payload>,
) -> Result<(GateState, DecisionGate<'certificate, 'payload>), GateError> {
    if state.phase == GatePhase::Terminal {
        return Err(GateError::TerminalState);
    }
    let GateInput {
        effect_class,
        required_budget,
        verified_evidence,
        task_receipt,
    } = input;
    if let Some(decision) = proof_decision(effect_class, verified_evidence, task_receipt)? {
        return Ok(terminate(state, decision));
    }
    if required_budget <= state.current_budget {
        return Ok(terminate(state, DecisionGate::RawFallback));
    }
    expand(state)
}

fn proof_decision<'certificate, 'payload>(
    effect_class: EffectClass,
    verified_evidence: Option<VerifiedEvidence<'certificate, 'payload>>,
    task_receipt: Option<TaskAcceptanceReceipt>,
) -> Result<Option<DecisionGate<'certificate, 'payload>>, GateError> {
    match (effect_class, verified_evidence, task_receipt) {
        (_, Some(_), Some(_)) => Err(GateError::ConflictingProofs),
        (EffectClass::Irreversible, _, Some(_)) => Err(GateError::IrreversibleSpeculation),
        (EffectClass::Irreversible, None, None) => Ok(Some(DecisionGate::RawFallback)),
        (_, _, Some(receipt)) => Ok(Some(DecisionGate::TaskVerified(receipt))),
        (_, Some(evidence), None) => Ok(Some(DecisionGate::Certified(PolicySufficiencyWitness {
            evidence,
        }))),
        (_, None, None) => Ok(None),
    }
}

fn terminate<'certificate, 'payload>(
    mut state: GateState,
    decision: DecisionGate<'certificate, 'payload>,
) -> (GateState, DecisionGate<'certificate, 'payload>) {
    state.phase = GatePhase::Terminal;
    (state, decision)
}

fn expand<'certificate, 'payload>(
    mut state: GateState,
) -> Result<(GateState, DecisionGate<'certificate, 'payload>), GateError> {
    let budget = state
        .current_budget
        .checked_mul(2)
        .ok_or(GateError::BudgetOverflow)?;
    let cumulative_visible_cost = state
        .cumulative_visible_cost
        .checked_add(budget)
        .ok_or(GateError::BudgetOverflow)?;
    let rounds = state
        .rounds
        .checked_add(1)
        .ok_or(GateError::RoundOverflow)?;
    state.current_budget = budget;
    state.cumulative_visible_cost = cumulative_visible_cost;
    state.rounds = rounds;
    Ok((
        state,
        DecisionGate::Expand(NextBudget {
            budget,
            round: rounds - 1,
        }),
    ))
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct T10Bound {
    pub actual_cost: u128,
    pub strict_upper_bound: u128,
    pub expansion_exponent: u32,
    pub holds: bool,
}

/// Checks exactly: visible_cost + q*rounds < 4*K_H + q*(1+ceil(log2(K_H/b0))).
pub fn check_t10_bound(
    state: GateState,
    hindsight_budget: u128,
    per_round_overhead: u128,
) -> Result<T10Bound, GateError> {
    if hindsight_budget == 0 {
        return Err(GateError::ZeroHindsightBudget);
    }
    let exponent = ceil_log2_ratio(hindsight_budget, state.initial_budget)?;
    let actual_overhead = per_round_overhead
        .checked_mul(u128::from(state.rounds))
        .ok_or(GateError::BoundOverflow)?;
    let actual_cost = state
        .cumulative_visible_cost
        .checked_add(actual_overhead)
        .ok_or(GateError::BoundOverflow)?;
    let rhs_rounds = u128::from(exponent)
        .checked_add(1)
        .ok_or(GateError::BoundOverflow)?;
    let rhs_base = hindsight_budget
        .checked_mul(4)
        .ok_or(GateError::BoundOverflow)?;
    let rhs_overhead = per_round_overhead
        .checked_mul(rhs_rounds)
        .ok_or(GateError::BoundOverflow)?;
    let strict_upper_bound = rhs_base
        .checked_add(rhs_overhead)
        .ok_or(GateError::BoundOverflow)?;
    Ok(T10Bound {
        actual_cost,
        strict_upper_bound,
        expansion_exponent: exponent,
        holds: actual_cost < strict_upper_bound,
    })
}

pub fn ceil_log2_ratio(numerator: u128, denominator: u128) -> Result<u32, GateError> {
    if denominator == 0 {
        return Err(GateError::ZeroInitialBudget);
    }
    if numerator == 0 || numerator <= denominator {
        return Ok(0);
    }
    let quotient = numerator / denominator;
    let rounded = quotient
        .checked_add(u128::from(!numerator.is_multiple_of(denominator)))
        .ok_or(GateError::BoundOverflow)?;
    Ok(if rounded <= 1 {
        0
    } else {
        128 - (rounded - 1).leading_zeros()
    })
}

#[cfg(test)]
#[path = "../../../tests/rust/zero-gate/unit/lib.rs"]
mod tests;
