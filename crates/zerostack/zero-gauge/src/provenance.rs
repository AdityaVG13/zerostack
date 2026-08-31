//! Deterministic provenance for measurement reports. Measurement-only, off the authority path.
//! Every report's provenance root is the SHA-256 hex over its canonical sorted-key JSON rendering.
//! The same logical report always yields the same root; any byte difference changes the root.

#![forbid(unsafe_code)]

use serde::Serialize;
use std::error::Error;
use std::fmt;

/// Render a value as deterministic sorted-key canonical JSON and hash it. Uses
/// the hub's canonical JSON contract (`zero_abi::canonical_json`) and
/// `zero_abi::sha256_hex`. This is the only provenance derivation allowed for savings reports.
pub fn provenance_root(value: &impl Serialize) -> String {
    let canonical = zero_abi::canonical_json(
        &serde_json::to_value(value).expect("report serializes by construction"),
    );
    zero_abi::sha256_hex(canonical.as_bytes())
}

/// Canonical rendering of a value (sorted keys, stable).
pub fn canonical_render(value: &impl Serialize) -> String {
    zero_abi::canonical_json(
        &serde_json::to_value(value).expect("value serializes by construction"),
    )
}

/// Typed provenance failure (reserved for tamper detection; construction
/// itself cannot fail because serialization is infallible for the report
/// shapes used here).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProvenanceError {
    RootMismatch { expected: String, actual: String },
}

impl fmt::Display for ProvenanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootMismatch { expected, actual } => write!(
                f,
                "provenance root mismatch: expected {expected}, actual {actual}"
            ),
        }
    }
}

impl Error for ProvenanceError {}
