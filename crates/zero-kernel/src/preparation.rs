use std::collections::BTreeSet;

use zero_abi::{
    ExecDag, FinalizedSpeculationPlan, SOURCE_BYTE_LIMIT, SpeculationBinding, SpeculationCandidate,
    compile_finalized_speculation_plan,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedCell {
    source: String,
    digest: String,
}

impl PreparedCell {
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn compile_speculation_plan(
        &self,
        binding: SpeculationBinding,
        dag: &ExecDag,
        verifier_root: String,
        unconditional_nodes: &BTreeSet<String>,
        candidates: Vec<SpeculationCandidate>,
    ) -> Result<FinalizedSpeculationPlan, String> {
        compile_finalized_speculation_plan(
            self.digest.clone(),
            binding,
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

    pub fn finish(self) -> Result<PreparedCell, String> {
        if self.source.is_empty() {
            return Err("prepared cell must not be empty".into());
        }
        let digest = blake3::hash(self.source.as_bytes()).to_hex().to_string();
        Ok(PreparedCell {
            source: self.source,
            digest,
        })
    }
}
