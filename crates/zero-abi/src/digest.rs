//! Deterministic contract digest helpers.
//!
//! These hashes are Wire identity (canonical JSON + SHA-256). They are **not**
//! a C-25 semantic-mutation checker: changing ProtocolLimits defaults,
//! ApprovalGrant shape, or EngineIdentity aliases can leave the digest
//! unchanged if the hashed field names / version string stay put. Do not
//! Promote C-25 from the existing version/key-order pins.

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::schema::canonical_json;

/// SHA-256 of arbitrary bytes.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// SHA-256 of arbitrary bytes as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = sha256(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push(char::from(HEX[usize::from(b >> 4)]));
        out.push(char::from(HEX[usize::from(b & 0x0f)]));
    }
    out
}

/// Digest of a contract manifest: canonical JSON encoding, then SHA-256 hex.
///
/// Engines build their manifest Value from their own registry (name, aliases,
/// error kinds, normalized input/output schemas, semantics fields) and hash it
/// through this single implementation so two engines can never disagree on
/// encoding.
pub fn contract_digest_hex(manifest: &Value) -> String {
    sha256_hex(canonical_json(manifest).as_bytes())
}

/// Raw digest bytes of a contract manifest (SHA-256 over canonical JSON).
pub fn contract_digest(manifest: &Value) -> [u8; 32] {
    sha256(canonical_json(manifest).as_bytes())
}

#[cfg(test)]
#[path = "../../../tests/rust/zero-abi/unit/digest.rs"]
mod tests;
