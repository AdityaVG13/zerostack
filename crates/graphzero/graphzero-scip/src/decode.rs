use anyhow::{Context, Result, bail};
use protobuf::Message;

use scip::types as scip_types;

use crate::types::ScipDecoded;

/// Decode SCIP index bytes (FR-001).
pub fn decode_scip_bytes(bytes: &[u8]) -> Result<(scip_types::Index, ScipDecoded)> {
    let index = scip_types::Index::parse_from_bytes(bytes).context("decode SCIP Index protobuf")?;
    let mut symbol_count = 0usize;
    let mut relationship_count = 0usize;
    for doc in &index.documents {
        add_count(&mut symbol_count, doc.symbols.len(), "SCIP symbol count")?;
        add_count(
            &mut relationship_count,
            doc.occurrences.len(),
            "SCIP occurrence relationship count",
        )?;
        for sym in &doc.symbols {
            add_count(
                &mut relationship_count,
                sym.relationships.len(),
                "SCIP symbol relationship count",
            )?;
        }
    }
    let summary = ScipDecoded {
        symbol_count,
        relationship_count,
        document_count: index.documents.len(),
    };
    Ok((index, summary))
}

fn add_count(total: &mut usize, addend: usize, label: &str) -> Result<()> {
    let Some(next) = total.checked_add(addend) else {
        bail!("{label} overflow while decoding SCIP index");
    };
    *total = next;
    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/graphzero/unit/graphzero-scip/decode_tests.rs"]
mod tests;
