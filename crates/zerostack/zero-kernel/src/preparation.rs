use std::collections::BTreeSet;

use zero_abi::{
    CapsulePublication, CapsuleState, ExecDag, FinalizedSpeculationPlan, SOURCE_BYTE_LIMIT,
    SpeculationBinding, SpeculationCandidate, WorkCapsule, compile_finalized_speculation_plan,
    sha256_hex,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedCell {
    source: String,
    digest: String,
    binding: SpeculationBinding,
    capsule: WorkCapsule,
    publication: CapsulePublication,
}

impl PreparedCell {
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn binding(&self) -> &SpeculationBinding {
        &self.binding
    }
    pub fn capsule(&self) -> &WorkCapsule {
        &self.capsule
    }
    pub fn publication(&self) -> &CapsulePublication {
        &self.publication
    }

    /// Reject any coordinate drift away from the binding this cell was
    /// finalized against. A prepared cell may only be launched under the
    /// exact capsule, state, contract, and epoch coordinates it sealed.
    pub fn validate_binding(&self, expected: &SpeculationBinding) -> Result<(), String> {
        if expected.capsule_root != self.binding.capsule_root
            || expected.state_root != self.binding.state_root
            || expected.contract_root != self.binding.contract_root
            || expected.epoch != self.binding.epoch
        {
            return Err("prepared cell binding drifted from its finalized coordinates".into());
        }
        Ok(())
    }

    pub fn compile_speculation_plan(
        &self,
        dag: &ExecDag,
        verifier_root: String,
        unconditional_nodes: &BTreeSet<String>,
        candidates: Vec<SpeculationCandidate>,
    ) -> Result<FinalizedSpeculationPlan, String> {
        compile_finalized_speculation_plan(
            self.digest.clone(),
            self.binding.clone(),
            dag,
            verifier_root,
            unconditional_nodes,
            candidates,
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct CellPreparation {
    source: String,
}

impl CellPreparation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, delta: &str) -> Result<(), String> {
        let length = self
            .source
            .len()
            .checked_add(delta.len())
            .ok_or("prepared cell length overflow")?;
        if length > SOURCE_BYTE_LIMIT {
            return Err(format!("prepared cell exceeds {SOURCE_BYTE_LIMIT} bytes"));
        }
        self.source.push_str(delta);
        Ok(())
    }

    /// Seal the collected source into a prepared cell bound to one exact
    /// capsule, publication, and speculation binding. Every coordinate must
    /// agree: the capsule must be a Draft carrying the finalized source as
    /// its task root, its canonical root must be the published capsule root
    /// and the binding capsule root, and the binding must carry canonical
    /// roots with a positive epoch.
    pub fn finish(
        self,
        binding: SpeculationBinding,
        capsule: WorkCapsule,
        publication: CapsulePublication,
    ) -> Result<PreparedCell, String> {
        if self.source.is_empty() {
            return Err("prepared cell must not be empty".into());
        }
        binding
            .validate()
            .map_err(|error| format!("prepared cell binding is invalid: {error}"))?;
        if binding.epoch == 0 {
            return Err("prepared cell binding requires a positive epoch".into());
        }
        capsule
            .validate()
            .map_err(|error| format!("prepared cell capsule is invalid: {error}"))?;
        if capsule.state != CapsuleState::Draft {
            return Err("prepared cell capsule must be in Draft state".into());
        }
        publication
            .validate()
            .map_err(|error| format!("prepared cell publication is invalid: {error}"))?;
        let digest = sha256_hex(self.source.as_bytes());
        if capsule.roots.task != digest {
            return Err("prepared cell task root must equal the finalized source digest".into());
        }
        let capsule_root = capsule
            .root()
            .map_err(|error| format!("prepared cell capsule root is unavailable: {error}"))?;
        if capsule_root != publication.capsule_root || capsule_root != binding.capsule_root {
            return Err(
                "prepared cell binding, capsule, and publication disagree on the capsule root"
                    .into(),
            );
        }
        Ok(PreparedCell {
            source: self.source,
            digest,
            binding,
            capsule,
            publication,
        })
    }
}
