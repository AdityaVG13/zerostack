//! AST indexing — file walk, parsing, and index lifecycle.

mod adapter;
mod index;
mod parse;
mod types;
pub mod walk;

#[cfg(feature = "fszero-ast-sgrep")]
pub use adapter::is_structural_file_key;
#[cfg(any(test, feature = "search-eval"))]
pub use adapter::{extract_structural, is_structural_path};
pub use walk::relative_file_key;
