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

pub mod deoptimization;
pub mod durable_publication;
pub mod invalidation;
pub mod q99;
pub mod quality;
pub mod recovery;
pub mod semantic_cut;
pub mod transaction;
pub mod two_phase;
pub use deoptimization::*;
pub use durable_publication::*;
pub use invalidation::*;
pub use q99::*;
pub use quality::*;
pub use recovery::*;
pub use semantic_cut::*;
pub use transaction::*;
pub use two_phase::*;

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

pub fn decide<'certificate, 'payload>(
    mut state: GateState,
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
        .checked_add(u128::from(numerator % denominator != 0))
        .ok_or(GateError::BoundOverflow)?;
    Ok(if rounded <= 1 {
        0
    } else {
        128 - (rounded - 1).leading_zeros()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input(effect_class: EffectClass, required_budget: u128) -> GateInput<'static, 'static> {
        GateInput {
            effect_class,
            required_budget,
            verified_evidence: None,
            task_receipt: None,
        }
    }
    #[test]
    fn fixed_transition_table() {
        let s = GateState::new(4).unwrap();
        let (terminal, gate) = decide(s, input(EffectClass::ReadOnly, 4)).unwrap();
        assert!(matches!(gate, DecisionGate::RawFallback));
        assert_eq!(
            decide(terminal, input(EffectClass::ReadOnly, 5)).unwrap_err(),
            GateError::TerminalState
        );
        let (expanded, gate) = decide(s, input(EffectClass::ReversibleMutation, 5)).unwrap();
        assert!(matches!(
            gate,
            DecisionGate::Expand(NextBudget { budget: 8, .. })
        ));
        assert_eq!(expanded.cumulative_visible_cost(), 12);
        let (_, gate) = decide(s, input(EffectClass::Irreversible, u128::MAX)).unwrap();
        assert!(matches!(gate, DecisionGate::RawFallback));
    }
    struct Accept;
    impl TaskAcceptanceVerifier for Accept {
        fn verify_run(&self, _: &TaskRunEvidence) -> Result<(), TaskVerifierError> {
            Ok(())
        }
    }
    struct Reject;
    impl TaskAcceptanceVerifier for Reject {
        fn verify_run(&self, _: &TaskRunEvidence) -> Result<(), TaskVerifierError> {
            Err(TaskVerifierError::UntrustedRunEvidence)
        }
    }
    fn run(exit_code: i32, observed: Vec<[u8; 32]>, cost: u64) -> TaskRunEvidence {
        TaskRunEvidence::new(
            7,
            CommandId(11),
            [2; 32],
            exit_code,
            vec![[3; 32]],
            observed,
            [4; 32],
            cost,
        )
    }
    fn attempt(evidence: TaskRunEvidence) -> SandboxAttempt {
        begin_task_attempt(EffectClass::ReversibleMutation, evidence).unwrap()
    }

    #[test]
    fn objective_verifier_mints_complete_passing_receipt_and_commit() {
        let verified = verify_task_acceptance(&Accept, attempt(run(0, vec![[3; 32]], 9))).unwrap();
        let receipt = verified.receipt();
        assert_eq!(receipt.task_id(), 7);
        assert_eq!(receipt.verifier(), CommandId(11));
        assert_eq!(receipt.verifier_environment_digest(), &[2; 32]);
        assert_eq!(receipt.outcome(), TaskOutcome::Passed);
        assert_eq!(receipt.exit_code(), 0);
        assert_eq!(receipt.expected_artifact_digests(), &[[3; 32]]);
        assert_eq!(receipt.observed_artifact_digests(), &[[3; 32]]);
        assert_eq!(receipt.journal_id(), &[4; 32]);
        assert_eq!(receipt.attempt_cost(), 9);
        let gate_input = GateInput {
            effect_class: EffectClass::ReversibleMutation,
            required_budget: 4,
            verified_evidence: None,
            task_receipt: Some(verified.into_receipt()),
        };
        let (_, gate) = decide(GateState::new(4).unwrap(), gate_input).unwrap();
        let DecisionGate::TaskVerified(receipt) = gate else {
            panic!("expected task receipt")
        };
        assert_eq!(receipt.task_id(), 7);
    }

    #[test]
    fn verifier_failures_and_missing_receipts_rollback_with_cost() {
        let rejected =
            verify_task_acceptance(&Reject, attempt(run(0, vec![[3; 32]], 7))).unwrap_err();
        assert_eq!(
            rejected.reason(),
            TaskAcceptanceError::VerifierRejected(TaskVerifierError::UntrustedRunEvidence)
        );
        assert_eq!(rejected.rollback().attempt_cost(), 7);
        let nonzero =
            verify_task_acceptance(&Accept, attempt(run(2, vec![[3; 32]], 8))).unwrap_err();
        assert_eq!(
            nonzero.reason(),
            TaskAcceptanceError::NonZeroOutcome { exit_code: 2 }
        );
        assert_eq!(
            nonzero.rollback().reason(),
            RollbackReason::VerificationFailed(TaskAcceptanceError::NonZeroOutcome {
                exit_code: 2
            })
        );
        let mismatch =
            verify_task_acceptance(&Accept, attempt(run(0, vec![[9; 32]], 10))).unwrap_err();
        assert!(matches!(
            mismatch.reason(),
            TaskAcceptanceError::ArtifactMismatch { index: 0, .. }
        ));
        assert_eq!(mismatch.rollback().attempt_cost(), 10);
        let missing = attempt(run(0, vec![[3; 32]], 11)).rollback_missing_receipt();
        assert_eq!(missing.reason(), RollbackReason::MissingReceipt);
        assert_eq!(missing.attempt_cost(), 11);
    }

    #[test]
    fn irreversible_speculation_is_typed_rejection_even_with_receipt() {
        assert_eq!(
            begin_task_attempt(EffectClass::Irreversible, run(0, vec![[3; 32]], 1)).unwrap_err(),
            SpeculationError::IrreversibleEffect
        );
        let receipt = verify_task_acceptance(&Accept, attempt(run(0, vec![[3; 32]], 1)))
            .unwrap()
            .into_receipt();
        let gate_input = GateInput {
            effect_class: EffectClass::Irreversible,
            required_budget: u128::MAX,
            verified_evidence: None,
            task_receipt: Some(receipt),
        };
        assert_eq!(
            decide(GateState::new(4).unwrap(), gate_input).unwrap_err(),
            GateError::IrreversibleSpeculation
        );
    }

    #[test]
    fn attempts_are_nonzero_cost_and_artifacts_are_bounded() {
        assert_eq!(
            begin_task_attempt(EffectClass::ReadOnly, run(0, vec![[3; 32]], 0)).unwrap_err(),
            SpeculationError::ZeroAttemptCost
        );
        let too_many = vec![[3; 32]; MAX_TASK_ARTIFACTS + 1];
        assert_eq!(
            begin_task_attempt(
                EffectClass::ReadOnly,
                TaskRunEvidence::new(
                    7,
                    CommandId(1),
                    [2; 32],
                    0,
                    too_many.clone(),
                    too_many,
                    [4; 32],
                    1
                )
            )
            .unwrap_err(),
            SpeculationError::TooManyArtifacts {
                count: MAX_TASK_ARTIFACTS + 1,
                maximum: MAX_TASK_ARTIFACTS
            }
        );
    }

    #[test]
    fn irreversible_without_proof_is_immediate_terminal_fallback() {
        for required_budget in [1, u128::MAX] {
            let (state, gate) = decide(
                GateState::new(4).unwrap(),
                input(EffectClass::Irreversible, required_budget),
            )
            .unwrap();
            assert_eq!(state.phase(), GatePhase::Terminal);
            assert!(matches!(gate, DecisionGate::RawFallback));
        }
    }

    #[test]
    fn geometric_and_nonmonotone_demands_obey_bound() {
        for demands in [
            [2, 3, 5, 9, 17],
            [33, 3, 65, 2, 129],
            [1025, 17, 2049, 1, 4097],
        ] {
            let mut state = GateState::new(2).unwrap();
            let mut high = 2;
            for demand in demands {
                high = high.max(demand);
                while demand > state.current_budget() {
                    (state, _) = decide(state, input(EffectClass::ReadOnly, demand)).unwrap();
                }
            }
            assert!(check_t10_bound(state, high, 7).unwrap().holds);
        }
    }
    #[test]
    fn edge_errors_are_typed() {
        assert_eq!(GateState::new(0), Err(GateError::ZeroInitialBudget));
        let state = GateState {
            initial_budget: 1,
            current_budget: u128::MAX - 1,
            cumulative_visible_cost: 1,
            rounds: 1,
            phase: GatePhase::Active,
        };
        assert_eq!(
            decide(state, input(EffectClass::ReadOnly, u128::MAX)).unwrap_err(),
            GateError::BudgetOverflow
        );
        assert_eq!(ceil_log2_ratio(1, 0), Err(GateError::ZeroInitialBudget));
        assert_eq!(
            check_t10_bound(GateState::new(1).unwrap(), 0, 0),
            Err(GateError::ZeroHindsightBudget)
        );
    }
}
