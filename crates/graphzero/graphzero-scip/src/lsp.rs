//! Optional LSP delegation (FR-008, FR-009). Walking skeleton uses ignored tests.

use crate::types::TierBEdge;

pub trait LspAdapter {
    fn definition(&self, symbol: &str) -> anyhow::Result<Option<TierBEdge>>;
    fn references(&self, symbol: &str) -> anyhow::Result<Vec<TierBEdge>>;
}

/// No-op adapter for CI default lane.
pub struct DisabledLsp;

impl LspAdapter for DisabledLsp {
    fn definition(&self, _symbol: &str) -> anyhow::Result<Option<TierBEdge>> {
        Ok(None)
    }

    fn references(&self, _symbol: &str) -> anyhow::Result<Vec<TierBEdge>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
#[path = "../../../../tests/graphzero/unit/graphzero-scip/lsp_tests.rs"]
mod tests;
