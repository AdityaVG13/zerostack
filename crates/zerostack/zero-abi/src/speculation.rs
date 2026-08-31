//! Rooted contracts for zero-miss speculative execution. Zero never predicts whether a partially
//! generated call will happen. It may prelaunch work only after the finalized source has compiled
//! to an exact execution DAG proving that the call is unconditional.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ExecDag, ExecNode, ExecNodeKind, canonical_json, sha256_hex};

pub const SPECULATION_CONTRACT: &str = "zerostack.speculation.claim";
pub const DEFAULT_SPECULATION_LIMIT: u32 = 16;

fn valid_root(root: &str) -> bool {
    root.len() == 64
        && root
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SpeculationBinding {
    pub capsule_root: String,
    pub state_root: String,
    pub contract_root: String,
    pub epoch: u64,
}

impl SpeculationBinding {
    pub fn validate(&self) -> Result<(), String> {
        for root in [&self.capsule_root, &self.state_root, &self.contract_root] {
            if !valid_root(root) {
                return Err("speculation binding carries an invalid root".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FinalizedCallProof {
    pub finalized_source_root: String,
    pub execution_dag_root: String,
    pub node_root: String,
    pub verifier_root: String,
    pub exact_input_roots: Vec<String>,
    pub unconditional: bool,
}

impl FinalizedCallProof {
    pub fn validate(&self) -> Result<(), String> {
        for root in [
            &self.finalized_source_root,
            &self.execution_dag_root,
            &self.node_root,
            &self.verifier_root,
        ] {
            if !valid_root(root) {
                return Err("finalized call proof carries an invalid root".into());
            }
        }
        if self.exact_input_roots.is_empty()
            || self.exact_input_roots.iter().any(|root| !valid_root(root))
        {
            return Err("finalized call proof requires exact input roots".into());
        }
        if !self.unconditional {
            return Err("speculation requires an unconditional finalized DAG node".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SpeculativeOperation {
    Read,
    Find,
    PureExtension { operation_id: String },
}

impl SpeculativeOperation {
    pub fn from_zero_method(method: &str) -> Option<Self> {
        match method {
            "read" => Some(Self::Read),
            "find" => Some(Self::Find),
            "edit" | "apply" | "run" | "state" => None,
            _ => None,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if let Self::PureExtension { operation_id } = self
            && (operation_id.is_empty()
                || !operation_id.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'.' | b'_' | b'-')
                }))
        {
            return Err("pure extension id must match lowercase [a-z0-9_.-]".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SpeculationPermit {
    pub node_id: String,
    pub operation: SpeculativeOperation,
    pub arguments: Value,
    pub binding: SpeculationBinding,
    pub proof: FinalizedCallProof,
    pub occurrence: u32,
    pub certified_pure: bool,
    pub cancellation_bound: bool,
    pub work_budget: u64,
    pub provider_token_budget: u64,
}

impl SpeculationPermit {
    pub fn validate(&self) -> Result<(), String> {
        self.operation.validate()?;
        self.binding.validate()?;
        self.proof.validate()?;
        if self.node_id.is_empty() {
            return Err("speculation permit requires a DAG node id".into());
        }
        if self.occurrence == 0 {
            return Err("speculation occurrence must be positive".into());
        }
        if !self.certified_pure {
            return Err("speculation requires a certified-pure operation".into());
        }
        if !self.cancellation_bound {
            return Err("speculation requires bounded cancellation".into());
        }
        if self.work_budget == 0 {
            return Err("speculation work budget must be positive".into());
        }
        Ok(())
    }

    pub fn claim_key(&self) -> Result<String, String> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(|error| error.to_string())?;
        let mut preimage = SPECULATION_CONTRACT.as_bytes().to_vec();
        preimage.push(0);
        preimage.extend_from_slice(canonical_json(&value).as_bytes());
        Ok(sha256_hex(&preimage))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SpeculationCandidate {
    pub node_id: String,
    pub operation: SpeculativeOperation,
    pub arguments: Value,
    pub exact_input_roots: Vec<String>,
    pub occurrence: u32,
    pub certified_pure: bool,
    pub cancellation_bound: bool,
    pub work_budget: u64,
    pub provider_token_budget: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FinalizedSpeculationPlan {
    pub finalized_source_root: String,
    pub execution_dag_root: String,
    pub verifier_root: String,
    pub binding: SpeculationBinding,
    pub speculative: Vec<SpeculationPermit>,
    pub ordinary_node_ids: Vec<String>,
}

impl FinalizedSpeculationPlan {
    pub fn validate(&self) -> Result<(), String> {
        self.binding.validate()?;
        for root in [
            &self.finalized_source_root,
            &self.execution_dag_root,
            &self.verifier_root,
        ] {
            if !valid_root(root) {
                return Err("finalized speculation plan carries an invalid root".into());
            }
        }
        let mut nodes = BTreeSet::new();
        for permit in &self.speculative {
            permit.validate()?;
            if permit.binding != self.binding
                || permit.proof.finalized_source_root != self.finalized_source_root
                || permit.proof.execution_dag_root != self.execution_dag_root // ubs:ignore — public content identity, not a secret
                || permit.proof.verifier_root != self.verifier_root
                || !nodes.insert(permit.node_id.as_str())
            {
                return Err("speculation permit disagrees with its finalized plan".into());
            }
        }
        if self
            .ordinary_node_ids
            .iter()
            .any(|node| node.is_empty() || !nodes.insert(node.as_str()))
        {
            return Err("finalized speculation plan has duplicate or empty nodes".into());
        }
        Ok(())
    }
}

pub fn compile_finalized_speculation_plan(
    finalized_source_root: String,
    binding: SpeculationBinding,
    dag: &ExecDag,
    verifier_root: String,
    unconditional_nodes: &BTreeSet<String>,
    candidates: Vec<SpeculationCandidate>,
) -> Result<FinalizedSpeculationPlan, String> {
    if !valid_root(&finalized_source_root) || !valid_root(&verifier_root) {
        return Err("speculation compiler requires finalized source and verifier roots".into());
    }
    binding.validate()?;
    dag.validate().map_err(|error| error.to_string())?;
    let execution_dag_root = dag.plan_digest().map_err(|error| error.to_string())?;
    let mut seen = BTreeSet::new();
    let mut speculative = Vec::new();
    let mut ordinary_node_ids = Vec::new();
    for candidate in candidates {
        if candidate.node_id.is_empty() || !seen.insert(candidate.node_id.clone()) {
            return Err("speculation candidates require unique nonempty node ids".into());
        }
        candidate.operation.validate()?;
        if candidate.occurrence == 0
            || candidate.exact_input_roots.is_empty()
            || candidate
                .exact_input_roots
                .iter()
                .any(|root| !valid_root(root))
        {
            return Err("speculation candidate identity or input roots are invalid".into());
        }
        let node = dag
            .nodes
            .iter()
            .find(|node| node.id == candidate.node_id)
            .ok_or_else(|| {
                format!(
                    "speculation candidate {} is absent from DAG",
                    candidate.node_id
                )
            })?;
        let eligible = node.kind == ExecNodeKind::Op
            && unconditional_nodes.contains(&candidate.node_id)
            && candidate.certified_pure
            && candidate.cancellation_bound
            && candidate.work_budget > 0;
        if !eligible {
            ordinary_node_ids.push(candidate.node_id);
            continue;
        }
        let node_root = speculation_node_root(node, &candidate)?;
        speculative.push(SpeculationPermit {
            node_id: candidate.node_id,
            operation: candidate.operation,
            arguments: candidate.arguments,
            binding: binding.clone(),
            proof: FinalizedCallProof {
                finalized_source_root: finalized_source_root.clone(),
                execution_dag_root: execution_dag_root.clone(),
                node_root,
                verifier_root: verifier_root.clone(),
                exact_input_roots: candidate.exact_input_roots,
                unconditional: true,
            },
            occurrence: candidate.occurrence,
            certified_pure: true,
            cancellation_bound: true,
            work_budget: candidate.work_budget,
            provider_token_budget: candidate.provider_token_budget,
        });
    }
    speculative.sort_by(|left, right| left.node_id.cmp(&right.node_id));
    ordinary_node_ids.sort();
    let plan = FinalizedSpeculationPlan {
        finalized_source_root,
        execution_dag_root,
        verifier_root,
        binding,
        speculative,
        ordinary_node_ids,
    };
    plan.validate()?;
    Ok(plan)
}

fn speculation_node_root(
    node: &ExecNode,
    candidate: &SpeculationCandidate,
) -> Result<String, String> {
    let value = serde_json::json!({
        "node": node,
        "operation": candidate.operation,
        "arguments": candidate.arguments,
        "exactInputRoots": candidate.exact_input_roots,
        "occurrence": candidate.occurrence,
        "providerTokenBudget": candidate.provider_token_budget,
    });
    Ok(sha256_hex(canonical_json(&value).as_bytes()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeculationAdmission {
    Ordinary,
    Speculated,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeculationState {
    Pending,
    Running,
    Ready,
    Claimed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SpeculationLedger {
    pub ordinary_admissions: u64,
    pub dispatched: u64,
    pub claim_hits: u64,
    pub claim_invariant_failures: u64,
    pub cancelled: u64,
    pub failed: u64,
    pub wasted_ready: u64,
    pub work_units_dispatched: u64,
    pub work_units_claimed: u64,
    pub provider_tokens_dispatched: u64,
    pub provider_tokens_claimed: u64,
    pub provider_tokens_wasted_upper_bound: u64,
}

impl SpeculationLedger {
    pub fn validate(&self) -> Result<(), String> {
        if self.claim_invariant_failures != 0 {
            return Err("speculation ledger records an exact-claim invariant failure".into());
        }
        if self.claim_hits > self.dispatched
            || self.cancelled > self.dispatched
            || self.failed > self.dispatched
            || self.wasted_ready > self.dispatched
            || self.work_units_claimed > self.work_units_dispatched
            || self.provider_tokens_claimed > self.provider_tokens_dispatched
            || self.provider_tokens_wasted_upper_bound > self.provider_tokens_dispatched
        {
            return Err("speculation ledger violates conservation".into());
        }
        Ok(())
    }
}
