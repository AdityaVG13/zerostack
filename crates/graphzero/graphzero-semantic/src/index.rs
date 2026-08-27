//! In-memory semantic index keyed by content hash (FR-005).

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use graphzero_store::ContentHash;

use crate::embed::{DeterministicEmbedder, SemanticVector};
use crate::spans::EmbedSpan;

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticRecord {
    pub blob_hash: ContentHash,
    pub span: EmbedSpan,
    pub vector: SemanticVector,
}

#[derive(Clone, Debug, Default)]
pub struct SemanticIndex {
    records: Vec<SemanticRecord>,
}

impl SemanticIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert_blob(
        &mut self,
        blob_hash: ContentHash,
        content: &[u8],
        spans: &[EmbedSpan],
        embedder: &DeterministicEmbedder,
    ) {
        self.records.retain(|r| r.blob_hash != blob_hash);
        for span in spans {
            let text = std::str::from_utf8(
                content
                    .get(span.start as usize..span.end as usize)
                    .unwrap_or(&[]),
            )
            .unwrap_or("");
            self.records.push(SemanticRecord {
                blob_hash,
                span: span.clone(),
                vector: embedder.embed_text(text),
            });
        }
    }

    pub fn remove_blob(&mut self, blob_hash: ContentHash) {
        self.records.retain(|r| r.blob_hash != blob_hash);
    }

    pub fn records(&self) -> &[SemanticRecord] {
        &self.records
    }

    pub fn semantic_tier_percent(&self, total_text_blobs: usize) -> f64 {
        if total_text_blobs == 0 {
            return 100.0;
        }
        let indexed_blobs: std::collections::BTreeSet<_> =
            self.records.iter().map(|r| r.blob_hash).collect();
        (indexed_blobs.len() as f64 / total_text_blobs as f64) * 100.0
    }

    pub fn query_top_k(
        &self,
        query_text: &str,
        embedder: &DeterministicEmbedder,
        k: usize,
    ) -> Vec<SemanticHit> {
        if k == 0 {
            return Vec::new();
        }
        let q = embedder.embed_text(query_text);
        // Bounded min-heap top-k: O(N log k) instead of O(N log N) full sort.
        let mut heap: BinaryHeap<HeapHit> = BinaryHeap::new();
        for r in &self.records {
            let score = crate::embed::cosine_similarity(q.as_slice(), r.vector.as_slice());
            let hit = HeapHit {
                score,
                blob_hash: r.blob_hash,
                span: r.span.clone(),
            };
            if heap.len() < k {
                heap.push(hit);
            } else if hit.score > heap.peek().map(|w| w.score).unwrap_or(f32::NEG_INFINITY) {
                heap.pop();
                heap.push(hit);
            }
        }
        let mut hits: Vec<SemanticHit> = heap
            .into_iter()
            .map(|h| SemanticHit {
                blob_hash: h.blob_hash,
                span: h.span,
                score: h.score,
            })
            .collect();
        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));
        hits
    }
}

/// Min-heap by score (BinaryHeap is max-heap; Ord is reversed).
#[derive(Clone, Debug)]
struct HeapHit {
    score: f32,
    blob_hash: ContentHash,
    span: EmbedSpan,
}

impl PartialEq for HeapHit {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score
    }
}
impl Eq for HeapHit {}
impl PartialOrd for HeapHit {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for HeapHit {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .score
            .partial_cmp(&self.score)
            .unwrap_or(Ordering::Equal)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticHit {
    pub blob_hash: ContentHash,
    pub span: EmbedSpan,
    pub score: f32,
}

#[cfg(test)]
#[path = "../../../../tests/graphzero/unit/graphzero-semantic/index_tests.rs"]
mod tests;
