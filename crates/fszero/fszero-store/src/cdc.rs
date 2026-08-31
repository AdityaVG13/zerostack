//! Deterministic content-defined chunking for rendered and semantic segments. Boundaries depend on
//! nearby bytes rather than absolute offsets, so small edits normally invalidate only the chunks
//! around the edit.

use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub const CDC_MIN_BYTES: usize = 512;
pub const CDC_AVG_BYTES: usize = 2_048;
pub const CDC_MAX_BYTES: usize = 8_192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CdcChunk {
    pub start: usize,
    pub end: usize,
    pub digest: [u8; 32],
}

impl CdcChunk {
    pub fn len(self) -> usize {
        self.end - self.start
    }
    pub fn bytes<'a>(self, input: &'a [u8]) -> &'a [u8] {
        &input[self.start..self.end]
    }
    pub fn digest_hex(self) -> String {
        self.digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedChunks {
    pub chunks: Vec<CdcChunk>,
    /// First unprocessed byte when `max_chunks` bounded the probe.
    pub truncated_at: Option<usize>,
}

/// A stable per-byte gear value. SplitMix64 gives a well-distributed table
/// without storing or initializing a 256-entry allocation.
const fn gear(byte: u8) -> u64 {
    let mut z = (byte as u64).wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[inline]
fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

const HASH_WINDOW: usize = 64;

fn window_hash(input: &[u8], end: usize) -> u64 {
    let start = end.saturating_sub(HASH_WINDOW);
    input[start..end]
        .iter()
        .fold(0u64, |hash, byte| hash.rotate_left(1) ^ gear(*byte))
}

/// Chunk the complete input. Empty input has no chunks.
pub fn content_defined_chunks(input: &[u8]) -> Vec<CdcChunk> {
    content_defined_chunks_bounded(input, usize::MAX).chunks
}

/// Chunk at most `max_chunks`, explicitly reporting unprocessed input. This is
/// used by bounded probes and indexers; it never disguises a partial result as a
/// complete platform result.
pub fn content_defined_chunks_bounded(input: &[u8], max_chunks: usize) -> BoundedChunks {
    if input.is_empty() || max_chunks == 0 {
        return BoundedChunks {
            chunks: Vec::new(),
            truncated_at: (!input.is_empty()).then_some(0),
        };
    }

    let mut chunks = Vec::with_capacity((input.len() / CDC_AVG_BYTES + 1).min(max_chunks));
    let mut start = 0;
    while start < input.len() && chunks.len() < max_chunks {
        let min_end = (start + CDC_MIN_BYTES).min(input.len());
        let max_end = (start + CDC_MAX_BYTES).min(input.len());
        let normal_end = (start + CDC_AVG_BYTES).min(max_end);
        let mut end = min_end;
        let mut hash = window_hash(input, end);

        while end < max_end {
            // Prefer fewer early cuts and more late cuts around the target.
            let mask = if end < normal_end { 0x0fff } else { 0x03ff };
            if hash & mask == 0 {
                break;
            }
            hash = hash.rotate_left(1) ^ gear(input[end]);
            if end >= HASH_WINDOW {
                hash ^= gear(input[end - HASH_WINDOW]).rotate_left(HASH_WINDOW as u32);
            }
            end += 1;
        }
        let bytes = &input[start..end];
        chunks.push(CdcChunk {
            start,
            end,
            digest: digest(bytes),
        });
        start = end;
    }

    BoundedChunks {
        chunks,
        truncated_at: (start < input.len()).then_some(start),
    }
}

/// Old-image chunks that are not reused by the new image. Every returned span
/// is therefore a complete old chunk; no consumer invalidates a partial chunk.
pub fn chunk_invalidations(before: &[u8], after: &[u8]) -> Vec<CdcChunk> {
    let old = content_defined_chunks(before);
    let mut available = HashMap::<[u8; 32], usize>::new();
    for chunk in content_defined_chunks(after) {
        *available.entry(chunk.digest).or_default() += 1;
    }
    old.into_iter()
        .filter(|chunk| {
            let count = available.entry(chunk.digest).or_default();
            if *count == 0 {
                true
            } else {
                *count -= 1;
                false
            }
        })
        .collect()
}

/// UTF-8-safe slices aligned to CDC boundaries. A boundary inside a scalar is
/// advanced to the next scalar boundary, preserving complete input coverage.
pub fn content_defined_text_chunks(text: &str, max_chunks: usize) -> Vec<&str> {
    if text.is_empty() || max_chunks == 0 {
        return Vec::new();
    }
    let bytes = text.as_bytes();
    let raw = content_defined_chunks_bounded(bytes, max_chunks);
    let mut out = Vec::with_capacity(raw.chunks.len());
    let mut start = 0;
    for (index, chunk) in raw.chunks.iter().enumerate() {
        let mut end = chunk.end;
        while end < bytes.len() && !text.is_char_boundary(end) {
            end += 1;
        }
        if index + 1 == max_chunks && raw.truncated_at.is_some() {
            // Preserve the existing indexer's explicit bounded-prefix behavior.
            end = chunk.end;
            while end > start && !text.is_char_boundary(end) {
                end -= 1;
            }
        }
        if end > start {
            out.push(&text[start..end]);
            start = end;
        }
    }
    out
}
