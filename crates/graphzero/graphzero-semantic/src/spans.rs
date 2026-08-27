//! Tier-A span selection for embed pipeline (FR-006).

use graphzero_extract::{BlobFacts, SymbolNode};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EmbedSpan {
    pub start: u32,
    pub end: u32,
    pub label: String,
}

pub fn select_embed_spans(facts: &BlobFacts) -> Vec<EmbedSpan> {
    facts.nodes.iter().map(symbol_span).collect()
}

fn symbol_span(node: &SymbolNode) -> EmbedSpan {
    EmbedSpan {
        start: node.span_start,
        end: node.span_end,
        label: node.name.clone(),
    }
}

#[cfg(test)]
#[path = "../../../../tests/graphzero/unit/graphzero-semantic/spans_tests.rs"]
mod tests;
