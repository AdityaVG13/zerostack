//! Dense per-blob per-tier bitmap.
//!
//! Layout v1: one `u64` per tier per blob.  Bit 0 = `indexed`.
//! Remaining 63 bits are reserved for future categories.

use graphzero_store::{BlobId, Tier};
use std::collections::HashMap;

/// Bit position for the "indexed" category within a tier word.
pub const CATEGORY_INDEXED: u32 = 0;

/// Versioned bitmap layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitmap {
    /// blob_id -> [tier_a_word, tier_b_word, tier_c_word]
    inner: HashMap<BlobId, [u64; 3]>,
}

impl Default for Bitmap {
    fn default() -> Self {
        Self::new()
    }
}

impl Bitmap {
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            inner: HashMap::with_capacity(cap),
        }
    }

    /// Set a category bit for a blob at a tier.
    pub fn set(&mut self, blob_id: &BlobId, tier: Tier, category: u32) {
        let mask = category_mask(category);
        let word = self.inner.entry(blob_id.clone()).or_insert([0; 3]);
        let idx = tier_index(tier);
        word[idx] |= mask;
    }

    /// Clear a category bit for a blob at a tier.
    pub fn clear(&mut self, blob_id: &BlobId, tier: Tier, category: u32) {
        let mask = category_mask(category);
        if let Some(word) = self.inner.get_mut(blob_id) {
            let idx = tier_index(tier);
            word[idx] &= !mask;
        }
    }

    /// Check whether a category bit is set.
    pub fn get(&self, blob_id: &BlobId, tier: Tier, category: u32) -> bool {
        let mask = category_mask(category);
        self.inner
            .get(blob_id)
            .map(|word| {
                let idx = tier_index(tier);
                (word[idx] & mask) != 0
            })
            .unwrap_or(false)
    }

    /// Returns true if the `indexed` bit is set for the given blob at the tier.
    pub fn is_indexed(&self, blob_id: &BlobId, tier: Tier) -> bool {
        self.get(blob_id, tier, CATEGORY_INDEXED)
    }

    /// Mark a blob as indexed at a tier (sets the `indexed` bit).
    pub fn mark_indexed(&mut self, blob_id: &BlobId, tier: Tier) {
        self.set(blob_id, tier, CATEGORY_INDEXED);
    }

    /// Remove a blob entirely from the bitmap.
    pub fn remove(&mut self, blob_id: &BlobId) {
        self.inner.remove(blob_id);
    }

    /// All tracked blob ids.
    pub fn blob_ids(&self) -> impl Iterator<Item = &BlobId> {
        self.inner.keys()
    }

    /// Total number of tracked blobs.
    pub fn total_blobs(&self) -> usize {
        self.inner.len()
    }

    /// Number of blobs indexed at the given tier.
    pub fn indexed_count(&self, tier: Tier) -> usize {
        let idx = tier_index(tier);
        self.inner
            .values()
            .filter(|word| (word[idx] & (1u64 << CATEGORY_INDEXED)) != 0)
            .count()
    }

    /// Exact coverage percentage for a tier.
    pub fn coverage_pct(&self, tier: Tier) -> f64 {
        let total = self.total_blobs();
        if total == 0 {
            0.0
        } else {
            let indexed = self.indexed_count(tier);
            (indexed as f64 / total as f64) * 100.0
        }
    }

    /// Tier A placeholder categories (bits 1..=7 reserved for SCIP/LSP).
    pub fn tier_b_reserved_bits() -> &'static [u32] {
        &[1, 2, 3, 4, 5, 6, 7]
    }

    /// Tier C placeholder categories (bits 8..=15 reserved for git empirical).
    pub fn tier_c_reserved_bits() -> &'static [u32] {
        &[8, 9, 10, 11, 12, 13, 14, 15]
    }

    /// Merge another bitmap into this one (OR operation).
    pub fn merge(&mut self, other: &Bitmap) {
        for (blob_id, other_words) in &other.inner {
            let word = self.inner.entry(blob_id.clone()).or_insert([0; 3]);
            for i in 0..3 {
                word[i] |= other_words[i];
            }
        }
    }
}

fn tier_index(tier: Tier) -> usize {
    match tier {
        Tier::A => 0,
        Tier::B => 1,
        Tier::C => 2,
    }
}

fn category_mask(category: u32) -> u64 {
    1u64.checked_shl(category)
        .unwrap_or_else(|| panic!("bitmap category {category} is out of range; expected 0..64"))
}

#[cfg(test)]
#[path = "../../../../tests/graphzero/unit/graphzero-coverage/bitmap_tests.rs"]
mod tests;
