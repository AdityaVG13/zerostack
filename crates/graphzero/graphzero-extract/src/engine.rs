//! Single-blob extraction engine. Pure function: BlobInput
//! → BlobFacts. Zero cross-blob state. Deterministic given identical blob bytes.

mod extract;
mod facts;
mod parse;

pub use facts::{extract_batch, extract_tier_a};
