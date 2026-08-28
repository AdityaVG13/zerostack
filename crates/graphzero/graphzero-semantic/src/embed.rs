//! Owned inference micro-loop: deterministic embed + cosine top-k (FR-002, FR-003).

use sha2::{Digest, Sha256};
use std::cmp::Ordering;

use crate::SEMANTIC_DIM;

const L2_NORMALIZE_MIN_NORM: f64 = 1.0e-6;

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticVector {
    dims: Vec<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VectorDimensionError {
    pub expected: usize,
    pub actual: usize,
}

impl std::fmt::Display for VectorDimensionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "semantic vector dimension mismatch: expected {}, got {}",
            self.expected, self.actual
        )
    }
}

impl std::error::Error for VectorDimensionError {}

impl SemanticVector {
    pub fn new(dims: Vec<f32>) -> Result<Self, VectorDimensionError> {
        if dims.len() != SEMANTIC_DIM {
            return Err(VectorDimensionError {
                expected: SEMANTIC_DIM,
                actual: dims.len(),
            });
        }
        Ok(Self { dims })
    }

    pub fn as_slice(&self) -> &[f32] {
        &self.dims
    }
}

/// Deterministic embedder for walking skeleton / CI (pinned pseudo-model).
#[derive(Clone, Copy, Debug, Default)]
pub struct DeterministicEmbedder;

impl DeterministicEmbedder {
    pub fn embed_text(&self, text: &str) -> SemanticVector {
        let mut acc = vec![0f32; SEMANTIC_DIM];
        for token in tokenize(text) {
            let mut h = Sha256::new();
            h.update(b"graphzero-semantic-v1:");
            h.update(token.as_bytes());
            let digest: [u8; 32] = h.finalize().into();
            for (i, chunk) in digest.chunks_exact(4).enumerate() {
                let idx = i % SEMANTIC_DIM;
                let v = f32::from_le_bytes(chunk.try_into().unwrap());
                acc[idx] += v;
            }
        }
        l2_normalize(&mut acc);
        SemanticVector { dims: acc }
    }
}

fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            cur.push(ch.to_ascii_lowercase());
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn l2_normalize(v: &mut [f32]) {
    let mut sum = 0f64;
    for x in v.iter() {
        sum += f64::from(*x) * f64::from(*x);
    }
    let norm = sum.sqrt();
    if norm <= L2_NORMALIZE_MIN_NORM {
        v.fill(0.0);
        return;
    }
    for x in v.iter_mut() {
        *x = (*x as f64 / norm) as f32;
    }
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let mut dot = 0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += f64::from(*x) * f64::from(*y);
    }
    dot as f32
}

#[derive(Clone, Debug, PartialEq)]
pub struct TopKHit {
    pub id: String,
    pub score: f32,
}

pub fn cosine_top_k(query: &[f32], candidates: &[(String, Vec<f32>)], k: usize) -> Vec<TopKHit> {
    let mut scored: Vec<TopKHit> = candidates
        .iter()
        .map(|(id, vec)| TopKHit {
            id: id.clone(),
            score: cosine_similarity(query, vec),
        })
        .collect();
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    scored.truncate(k);
    scored
}
