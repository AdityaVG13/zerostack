//! Single-blob extraction engine (FR-001, FR-004, FR-005, FR-010, FR-015).
//!
//! Pure function: BlobInput → BlobFacts. Zero cross-blob state.
//! Deterministic given identical blob bytes (NFR-004).
//!
//! Layout:
//! - [`parse`] — grammar setup and tree-sitter parse
//! - [`extract`] — definition/call/import/implements query passes
//! - [`facts`] — BlobFacts assembly and public entry points

mod extract;
mod facts;
mod parse;

pub use facts::{extract_batch, extract_tier_a};

#[cfg(test)]
#[path = "../../../../tests/graphzero/unit/graphzero-extract/engine_tests.rs"]
mod tests;
