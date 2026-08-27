//! Shared GraphZero primitive types.
//!
//! This crate breaks dependency cycles between graphzero-store and
//! graphzero-extract: extraction can emit content-addressed facts without
//! depending on the store implementation, while the store can consume those
//! facts in its default index path.

pub mod schema_version;

/// Logic-free compatibility re-export of the hub `zero-process` substrate.
///
/// GraphZero no longer forks process identity or verified child-tree lifecycle
/// code (previously `src/child_identity.rs`, a byte-level duplicate of
/// `zero-process`). The hub crate owns it; this module only re-exports the
/// exact public API the daemon and extraction surfaces consumed, so call sites
/// and persisted `stem.identity` records are unchanged.
pub mod child_identity {
    pub use zero_process::{
        ChildBinding, IDENTITY_FILE_NAME, IdentityError, ProcessIdentity, SignalOutcome,
        VerifiedChild, escalate_detached, peer_is_same_user,
    };
}

pub use schema_version::{
    AdmitOutcome, GRAPHZERO_STORE_SCHEMA_MAJOR, GRAPHZERO_STORE_SCHEMA_MINOR, SNAPSHOT_SCHEMA_FILE,
    SchemaVersionError, SchemaVersionRefuseReason, SchemaVersionStamp, SnapshotSchemaSegment,
    StoreSegmentKind, admit_current, admit_read,
};

/// Sha256 content hash of a blob, stored as 32 bytes.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn of(data: &[u8]) -> Self {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(data);
        Self(h.finalize().into())
    }

    pub fn to_hex(&self) -> String {
        fast_hex_32(&self.0)
    }

    pub fn from_hex(hex: &str) -> Option<Self> {
        if hex.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
            let s = std::str::from_utf8(chunk).ok()?;
            out[i] = u8::from_str_radix(s, 16).ok()?;
        }
        Some(Self(out))
    }
}

const HEX_LUT: &[u8; 512] = {
    const fn build() -> [u8; 512] {
        let mut t = [0u8; 512];
        let hex_chars = b"0123456789abcdef";
        let mut i = 0;
        while i < 256 {
            t[i * 2] = hex_chars[i >> 4];
            t[i * 2 + 1] = hex_chars[i & 0xf];
            i += 1;
        }
        t
    }
    &{ build() }
};

pub fn fast_hex_32(bytes: &[u8; 32]) -> String {
    fast_hex(bytes)
}

pub fn fast_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        let idx = (b as usize) * 2;
        out.push(HEX_LUT[idx] as char);
        out.push(HEX_LUT[idx + 1] as char);
    }
    out
}
