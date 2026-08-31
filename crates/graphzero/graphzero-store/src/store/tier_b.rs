//! Tier-B merge helpers.

use std::collections::BTreeMap;

use crate::Tier;

use super::coverage::CoverageBitmap;
use super::indexer::{EdgeRecord, IndexData};

pub const TIER_B_CONFIDENCE_U8: u8 = 255;

/// Merge tier-B edges into index data; tier B wins on identical (src,dst,kind).
pub fn merge_tier_b_edges(
    data: &mut IndexData,
    incoming: &[EdgeRecord],
    touched_blob_indices: &[(usize, bool)],
) {
    let mut by_triple: BTreeMap<(String, String, u8), EdgeRecord> = BTreeMap::new();
    for e in std::mem::take(&mut data.edges) {
        let key = (e.src.clone(), e.dst.clone(), e.kind);
        by_triple.insert(key, e);
    }
    for e in incoming {
        by_triple.insert((e.src.clone(), e.dst.clone(), e.kind), e.clone());
    }
    data.edges = by_triple.into_values().collect();

    for (idx, set_b) in touched_blob_indices {
        if *set_b
            && let Some(hash) = data.blob_order.get(*idx)
            && let Some(meta) = data.blobs.get_mut(hash)
        {
            meta.tier_bits |= 0b010;
        }
    }
}

pub fn apply_tier_b_coverage(coverage: &mut CoverageBitmap, blob_idx: usize) {
    coverage.set(blob_idx, Tier::B, true);
}

pub fn is_tier_b_edge(e: &EdgeRecord) -> bool {
    e.confidence == TIER_B_CONFIDENCE_U8
}

pub fn tier_b_source_label(_e: &EdgeRecord) -> &'static str {
    "scip"
}
