//! Provider-locked token identity and analytical token-accounting models.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

pub mod adaptive_eval;
pub mod bounds;
pub mod hundredfold;
pub mod observation;
pub mod pair;
pub mod provenance;
pub mod report;
pub mod solver;
pub mod theorems;

/// Exact provider, model, and tokenizer revision identity used for certification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderLock {
    pub provider: String,
    pub model: String,
    /// Lowercase SHA-256 digest of the tokenizer revision manifest.
    pub tokenizer_revision_digest: String,
}
