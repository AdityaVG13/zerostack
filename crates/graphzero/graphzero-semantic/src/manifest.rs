//! Model artifact pin (FR-007).

use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct SemanticManifest {
    pub model_artifact_sha256: String,
    pub dimension: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ManifestError {
    SemanticModelHashMismatch,
    Io(String),
    Parse(String),
}

pub fn load_manifest(path: &Path) -> Result<SemanticManifest, ManifestError> {
    let raw = fs::read_to_string(path).map_err(|e| ManifestError::Io(e.to_string()))?;
    serde_json::from_str(&raw).map_err(|e| ManifestError::Parse(e.to_string()))
}

pub fn verify_model_bytes(manifest: &SemanticManifest, bytes: &[u8]) -> Result<(), ManifestError> {
    let got = hex_sha256(bytes);
    if got != manifest.model_artifact_sha256 {
        return Err(ManifestError::SemanticModelHashMismatch);
    }
    Ok(())
}

pub fn hex_sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    graphzero_store::fast_hex(h.finalize().as_slice())
}

#[cfg(test)]
#[path = "../../../../tests/graphzero/unit/graphzero-semantic/manifest_tests.rs"]
mod tests;
