//! P5.1 semantic search tier — owned mmap index + leaf deterministic embedder.
//!
//! Default build uses a pinned deterministic bag-of-tokens embedder (no fastembed/ort).
//! Model weights for production MiniLM-class artifacts plug in behind the same manifest gate.

pub mod embed;
pub mod index;
pub mod manifest;
pub mod route;
pub mod shard;
pub mod spans;

pub use embed::{
    DeterministicEmbedder, SemanticVector, VectorDimensionError, cosine_similarity, cosine_top_k,
};
pub use index::{SemanticHit, SemanticIndex, SemanticRecord};
pub use manifest::{ManifestError, SemanticManifest, load_manifest, verify_model_bytes};
pub use route::{SnapRouteTrace, semantic_disabled};
pub use shard::{
    SEMANTIC_V1_MAGIC, SEMANTIC_VERSION, SemanticIntegrity, SemanticShardReader,
    SemanticShardWriter,
};
pub use spans::select_embed_spans;

pub const SEMANTIC_DIM: usize = 384;

const _: () = assert!(SEMANTIC_DIM == 384);
