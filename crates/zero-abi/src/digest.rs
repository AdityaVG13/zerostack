//! Deterministic contract digest helpers.

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
    let digest = sha256(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push(char::from_digit(u32::from(b >> 4), 16).unwrap());
        out.push(char::from_digit(u32::from(b & 0x0f), 16).unwrap());
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
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn digest_is_stable_across_key_order() {
        let a = json!({ "engine": "x", "ops": [{ "name": "read", "cost": "cheap" }] });
        let b = json!({ "ops": [{ "cost": "cheap", "name": "read" }], "engine": "x" });
        assert_eq!(contract_digest_hex(&a), contract_digest_hex(&b));
    }

    #[test]
    fn digest_changes_on_content_change() {
        let a = json!({ "engine": "x", "version": "1.0.0" });
        let b = json!({ "engine": "x", "version": "1.0.1" });
        assert_ne!(contract_digest_hex(&a), contract_digest_hex(&b));
    }

    #[test]
    fn sha256_hex_known_vector() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
