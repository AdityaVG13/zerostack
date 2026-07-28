#![forbid(unsafe_code)]

//! Pure, synchronous proof-carrying decision gate.
//!
//! A compressed commit cannot be constructed directly or from an expansion:
//! ~~~compile_fail
//! use zero_gate::{CompressedCommit, DecisionGate, NextBudget};
//! let _forged = CompressedCommit { payload: b"forged" };
//! let expansion = DecisionGate::Expand(NextBudget::new_for_doctest(8));
//! let _ = expansion.commit(b"unproven");
//! ~~~
//!
//! Linear gates and proofs cannot be replayed:
//! ~~~compile_fail
//! use zero_abi::raw_worker::EffectClass;
//! use zero_gate::{decide, verify_task_acceptance, DecisionGate, GateInput, GateState, TaskAcceptanceClaims, TaskAcceptanceError, TaskAcceptanceVerifier};
//! struct Trusted;
//! impl TaskAcceptanceVerifier for Trusted { fn verify(&self, _: &TaskAcceptanceClaims) -> Result<(), TaskAcceptanceError> { Ok(()) } }
//! let receipt = verify_task_acceptance(&Trusted, TaskAcceptanceClaims::new(7, [1; 32], [2; 32])).unwrap();
//! let gate = decide(GateState::new(8).unwrap(), GateInput { effect_class: EffectClass::Irreversible, required_budget: 8, verified_evidence: None, task_receipt: Some(receipt) }).unwrap().1;
//! if let DecisionGate::TaskVerified(receipt) = gate {
//!     let _first = receipt.commit(b"once");
//!     let _replay = receipt.commit(b"twice");
//! }
//! ~~~

use serde::{Deserialize, Serialize};
use std::fmt;
use zero_abi::raw_worker::EffectClass;
use zero_cert::VerifiedEvidence;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NextBudget { budget: u128, round: u32 }
impl NextBudget {
    pub fn budget(self) -> u128 { self.budget }
    pub fn round(self) -> u32 { self.round }
    #[doc(hidden)]
    pub fn new_for_doctest(budget: u128) -> Self { Self { budget, round: 0 } }
}

#[derive(Debug)]
pub struct PolicySufficiencyWitness<'certificate, 'payload> { evidence: VerifiedEvidence<'certificate, 'payload> }
impl<'certificate, 'payload> PolicySufficiencyWitness<'certificate, 'payload> {
    pub fn evidence(&self) -> &VerifiedEvidence<'certificate, 'payload> { &self.evidence }
    pub fn commit<'commit>(self, payload: &'commit [u8]) -> CompressedCommit<'certificate, 'payload, 'commit> {
        CompressedCommit { proof: CommitProof::Certified(self), payload }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskAcceptanceClaims {
    pub task_id: u64,
    pub result_digest: [u8; 32],
    pub verifier_identity: [u8; 32],
}
impl TaskAcceptanceClaims {
    pub const fn new(task_id: u64, result_digest: [u8; 32], verifier_identity: [u8; 32]) -> Self { Self { task_id, result_digest, verifier_identity } }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskAcceptanceError { Rejected }
impl fmt::Display for TaskAcceptanceError { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{self:?}") } }
impl std::error::Error for TaskAcceptanceError {}
pub trait TaskAcceptanceVerifier { fn verify(&self, claims: &TaskAcceptanceClaims) -> Result<(), TaskAcceptanceError>; }
/// Opaque linear proof minted only by an injected verifier.
/// ~~~compile_fail
/// use zero_gate::{TaskAcceptanceClaims, TaskAcceptanceReceipt};
/// let _ = TaskAcceptanceReceipt { claims: TaskAcceptanceClaims::new(7, [1; 32], [2; 32]) };
/// ~~~
#[derive(Debug)]
pub struct TaskAcceptanceReceipt { claims: TaskAcceptanceClaims }
impl TaskAcceptanceReceipt {
    pub fn claims(&self) -> &TaskAcceptanceClaims { &self.claims }
    pub fn task_id(&self) -> u64 { self.claims.task_id }
    pub fn result_digest(&self) -> &[u8; 32] { &self.claims.result_digest }
    pub fn verifier_identity(&self) -> &[u8; 32] { &self.claims.verifier_identity }
    pub fn commit<'certificate, 'payload, 'commit>(self, payload: &'commit [u8]) -> CompressedCommit<'certificate, 'payload, 'commit> { CompressedCommit { proof: CommitProof::TaskVerified(self), payload } }
}
pub fn verify_task_acceptance<V: TaskAcceptanceVerifier + ?Sized>(verifier: &V, claims: TaskAcceptanceClaims) -> Result<TaskAcceptanceReceipt, TaskAcceptanceError> {
    verifier.verify(&claims)?;
    Ok(TaskAcceptanceReceipt { claims })
}

#[derive(Debug)]
pub enum DecisionGate<'certificate, 'payload> {
    Certified(PolicySufficiencyWitness<'certificate, 'payload>),
    TaskVerified(TaskAcceptanceReceipt),
    Expand(NextBudget),
    RawFallback,
}

#[derive(Debug)]
enum CommitProof<'certificate, 'payload> {
    Certified(PolicySufficiencyWitness<'certificate, 'payload>),
    TaskVerified(TaskAcceptanceReceipt),
}

#[derive(Debug)]
pub struct CompressedCommit<'certificate, 'payload, 'commit> { proof: CommitProof<'certificate, 'payload>, payload: &'commit [u8] }
impl CompressedCommit<'_, '_, '_> {
    pub fn payload(&self) -> &[u8] { self.payload }
    pub fn is_task_verified(&self) -> bool { matches!(self.proof, CommitProof::TaskVerified(_)) }
    pub fn task_id(&self) -> Option<u64> {
        match &self.proof { CommitProof::TaskVerified(r) => Some(r.task_id()), CommitProof::Certified(_) => None }
    }
    pub fn evidence(&self) -> Option<&VerifiedEvidence<'_, '_>> {
        match &self.proof { CommitProof::Certified(w) => Some(&w.evidence), CommitProof::TaskVerified(_) => None }
    }
}

#[derive(Debug)]
pub struct GateInput<'certificate, 'payload> {
    pub effect_class: EffectClass,
    pub required_budget: u128,
    pub verified_evidence: Option<VerifiedEvidence<'certificate, 'payload>>,
    pub task_receipt: Option<TaskAcceptanceReceipt>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum GatePhase { Active, Terminal }

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
        if initial_budget == 0 { return Err(GateError::ZeroInitialBudget); }
        Ok(Self { initial_budget, current_budget: initial_budget, cumulative_visible_cost: initial_budget, rounds: 1, phase: GatePhase::Active })
    }
    pub fn initial_budget(self) -> u128 { self.initial_budget }
    pub fn current_budget(self) -> u128 { self.current_budget }
    pub fn cumulative_visible_cost(self) -> u128 { self.cumulative_visible_cost }
    pub fn rounds(self) -> u32 { self.rounds }
    pub fn phase(self) -> GatePhase { self.phase }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum GateError { ZeroInitialBudget, TerminalState, ConflictingProofs, BudgetOverflow, RoundOverflow, ZeroHindsightBudget, BoundOverflow }
impl fmt::Display for GateError { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{self:?}") } }
impl std::error::Error for GateError {}

pub fn decide<'certificate, 'payload>(mut state: GateState, input: GateInput<'certificate, 'payload>) -> Result<(GateState, DecisionGate<'certificate, 'payload>), GateError> {
    if state.phase == GatePhase::Terminal { return Err(GateError::TerminalState); }
    if input.verified_evidence.is_some() && input.task_receipt.is_some() { return Err(GateError::ConflictingProofs); }
    if input.effect_class == EffectClass::Irreversible && input.verified_evidence.is_none() && input.task_receipt.is_none() {
        state.phase = GatePhase::Terminal;
        return Ok((state, DecisionGate::RawFallback));
    }
    if let Some(receipt) = input.task_receipt {
        state.phase = GatePhase::Terminal;
        return Ok((state, DecisionGate::TaskVerified(receipt)));
    }
    if let Some(evidence) = input.verified_evidence {
        state.phase = GatePhase::Terminal;
        return Ok((state, DecisionGate::Certified(PolicySufficiencyWitness { evidence })));
    }
    if input.required_budget <= state.current_budget {
        state.phase = GatePhase::Terminal;
        return Ok((state, DecisionGate::RawFallback));
    }
    let budget = state.current_budget.checked_mul(2).ok_or(GateError::BudgetOverflow)?;
    let cumulative_visible_cost = state.cumulative_visible_cost.checked_add(budget).ok_or(GateError::BudgetOverflow)?;
    let rounds = state.rounds.checked_add(1).ok_or(GateError::RoundOverflow)?;
    state.current_budget = budget;
    state.cumulative_visible_cost = cumulative_visible_cost;
    state.rounds = rounds;
    Ok((state, DecisionGate::Expand(NextBudget { budget, round: rounds - 1 })))
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct T10Bound { pub actual_cost: u128, pub strict_upper_bound: u128, pub expansion_exponent: u32, pub holds: bool }

/// Checks exactly: visible_cost + q*rounds < 4*K_H + q*(1+ceil(log2(K_H/b0))).
pub fn check_t10_bound(state: GateState, hindsight_budget: u128, per_round_overhead: u128) -> Result<T10Bound, GateError> {
    if hindsight_budget == 0 { return Err(GateError::ZeroHindsightBudget); }
    let exponent = ceil_log2_ratio(hindsight_budget, state.initial_budget)?;
    let actual_overhead = per_round_overhead.checked_mul(u128::from(state.rounds)).ok_or(GateError::BoundOverflow)?;
    let actual_cost = state.cumulative_visible_cost.checked_add(actual_overhead).ok_or(GateError::BoundOverflow)?;
    let rhs_rounds = u128::from(exponent).checked_add(1).ok_or(GateError::BoundOverflow)?;
    let rhs_base = hindsight_budget.checked_mul(4).ok_or(GateError::BoundOverflow)?;
    let rhs_overhead = per_round_overhead.checked_mul(rhs_rounds).ok_or(GateError::BoundOverflow)?;
    let strict_upper_bound = rhs_base.checked_add(rhs_overhead).ok_or(GateError::BoundOverflow)?;
    Ok(T10Bound { actual_cost, strict_upper_bound, expansion_exponent: exponent, holds: actual_cost < strict_upper_bound })
}

pub fn ceil_log2_ratio(numerator: u128, denominator: u128) -> Result<u32, GateError> {
    if denominator == 0 { return Err(GateError::ZeroInitialBudget); }
    if numerator == 0 || numerator <= denominator { return Ok(0); }
    let quotient = numerator / denominator;
    let rounded = quotient.checked_add(u128::from(numerator % denominator != 0)).ok_or(GateError::BoundOverflow)?;
    Ok(if rounded <= 1 { 0 } else { 128 - (rounded - 1).leading_zeros() })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input(effect_class: EffectClass, required_budget: u128) -> GateInput<'static, 'static> {
        GateInput { effect_class, required_budget, verified_evidence: None, task_receipt: None }
    }
    #[test]
    fn fixed_transition_table() {
        let s = GateState::new(4).unwrap();
        let (terminal, gate) = decide(s, input(EffectClass::ReadOnly, 4)).unwrap();
        assert!(matches!(gate, DecisionGate::RawFallback));
        assert_eq!(decide(terminal, input(EffectClass::ReadOnly, 5)).unwrap_err(), GateError::TerminalState);
        let (expanded, gate) = decide(s, input(EffectClass::ReversibleMutation, 5)).unwrap();
        assert!(matches!(gate, DecisionGate::Expand(NextBudget { budget: 8, .. })));
        assert_eq!(expanded.cumulative_visible_cost(), 12);
        let (_, gate) = decide(s, input(EffectClass::Irreversible, u128::MAX)).unwrap();
        assert!(matches!(gate, DecisionGate::RawFallback));
    }
    struct Accept;
    impl TaskAcceptanceVerifier for Accept {
        fn verify(&self, _: &TaskAcceptanceClaims) -> Result<(), TaskAcceptanceError> { Ok(()) }
    }
    struct Reject;
    impl TaskAcceptanceVerifier for Reject {
        fn verify(&self, _: &TaskAcceptanceClaims) -> Result<(), TaskAcceptanceError> { Err(TaskAcceptanceError::Rejected) }
    }
    fn claims() -> TaskAcceptanceClaims { TaskAcceptanceClaims::new(7, [1; 32], [2; 32]) }

    #[test]
    fn trusted_task_acceptance_accepts_and_rejects() {
        assert_eq!(verify_task_acceptance(&Reject, claims()).unwrap_err(), TaskAcceptanceError::Rejected);
        let receipt = verify_task_acceptance(&Accept, claims()).unwrap();
        assert_eq!(receipt.task_id(), 7);
        assert_eq!(receipt.result_digest(), &[1; 32]);
        assert_eq!(receipt.verifier_identity(), &[2; 32]);
    }

    #[test]
    fn irreversible_without_proof_is_immediate_terminal_fallback() {
        for required_budget in [1, u128::MAX] {
            let (state, gate) = decide(GateState::new(4).unwrap(), input(EffectClass::Irreversible, required_budget)).unwrap();
            assert_eq!(state.phase(), GatePhase::Terminal);
            assert!(matches!(gate, DecisionGate::RawFallback));
        }
    }

    #[test]
    fn irreversible_with_valid_task_proof_is_legal() {
        let receipt = verify_task_acceptance(&Accept, claims()).unwrap();
        let gate_input = GateInput { effect_class: EffectClass::Irreversible, required_budget: u128::MAX, verified_evidence: None, task_receipt: Some(receipt) };
        let (state, gate) = decide(GateState::new(4).unwrap(), gate_input).unwrap();
        assert_eq!(state.phase(), GatePhase::Terminal);
        let DecisionGate::TaskVerified(receipt) = gate else { panic!("expected task proof"); };
        let commit = receipt.commit(b"accepted");
        assert_eq!(commit.payload(), b"accepted");
        assert_eq!(commit.task_id(), Some(7));
        assert!(commit.evidence().is_none());
    }

    #[test]
    fn geometric_and_nonmonotone_demands_obey_bound() {
        for demands in [[2, 3, 5, 9, 17], [33, 3, 65, 2, 129], [1025, 17, 2049, 1, 4097]] {
            let mut state = GateState::new(2).unwrap();
            let mut high = 2;
            for demand in demands {
                high = high.max(demand);
                while demand > state.current_budget() { (state, _) = decide(state, input(EffectClass::ReadOnly, demand)).unwrap(); }
            }
            assert!(check_t10_bound(state, high, 7).unwrap().holds);
        }
    }
    #[test]
    fn edge_errors_are_typed() {
        assert_eq!(GateState::new(0), Err(GateError::ZeroInitialBudget));
        let state = GateState { initial_budget: 1, current_budget: u128::MAX - 1, cumulative_visible_cost: 1, rounds: 1, phase: GatePhase::Active };
        assert_eq!(decide(state, input(EffectClass::ReadOnly, u128::MAX)).unwrap_err(), GateError::BudgetOverflow);
        assert_eq!(ceil_log2_ratio(1, 0), Err(GateError::ZeroInitialBudget));
        assert_eq!(check_t10_bound(GateState::new(1).unwrap(), 0, 0), Err(GateError::ZeroHindsightBudget));
    }
}
