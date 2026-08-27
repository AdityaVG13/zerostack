//! Raw-baseline project safepoints (fszero-d90z).

use sha2::{Digest, Sha256};

use super::exact_snapshot::{ExactSnapshot, NonsemanticExclusion};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafepointError {
    EmptySnapshot,
    IdentityMismatch { expected: String, actual: String },
}

impl std::fmt::Display for SafepointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptySnapshot => write!(
                f,
                "cannot safepoint empty snapshot set without explicit empty policy"
            ),
            Self::IdentityMismatch { expected, actual } => {
                write!(
                    f,
                    "safepoint identity mismatch: expected {expected}, actual {actual}"
                )
            }
        }
    }
}
impl std::error::Error for SafepointError {}

/// Exact project-side baseline safepoint. Does NOT claim external DB/process restore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawBaselineSafepoint {
    pub snapshot_root: String,
    pub overlay_root: Option<String>,
    pub journal_head: Option<String>,
    pub evidence_refs: Vec<String>,
    pub path_policy_digest: String,
    pub control_receipt_head: Option<String>,
    pub safepoint_id: String,
    /// Honest scope: this safepoint covers filesystem project state ONLY.
    /// External mutable state (DBs/processes/services) is out of scope.
    pub external_state_scope: &'static str,
    /// Declared metadata classes excluded from the bound snapshot identity.
    /// Mirrors the snapshot's declaration so the safepoint's scope statement
    /// is complete: what IS covered, and what is DECLARED excluded.
    pub nonsemantic_exclusions: Vec<NonsemanticExclusion>,
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

pub fn path_policy_digest() -> String {
    let mut h = Sha256::new();
    h.update(b"FSZERO-PATH-POLICY-V1\0reject-absolute\0reject-dotdot\0posix-relative");
    hex_encode(h.finalize().as_slice())
}

impl RawBaselineSafepoint {
    pub fn capture(
        snapshot: &ExactSnapshot,
        overlay_root: Option<&str>,
        journal_head: Option<&str>,
        evidence_refs: Vec<String>,
        control_receipt_head: Option<&str>,
    ) -> Self {
        let snapshot_root = snapshot.root_digest_hex().to_string();
        // The snapshot's declared exclusions become part of the safepoint's
        // scope declaration and identity (never silent).
        let nonsemantic_exclusions: Vec<NonsemanticExclusion> =
            snapshot.nonsemantic_exclusions().to_vec();
        let path_policy_digest = path_policy_digest();
        let mut h = Sha256::new();
        h.update(b"FSZERO-SAFEPOINT-V1\0");
        h.update(snapshot_root.as_bytes());
        h.update(&[0]);
        if let Some(o) = overlay_root {
            h.update(o.as_bytes());
        }
        h.update(&[0]);
        if let Some(j) = journal_head {
            h.update(j.as_bytes());
        }
        h.update(&[0]);
        for e in &evidence_refs {
            h.update(e.as_bytes());
            h.update(&[0]);
        }
        h.update(path_policy_digest.as_bytes());
        if let Some(c) = control_receipt_head {
            h.update(c.as_bytes());
        }
        for e in &nonsemantic_exclusions {
            h.update(e.tag());
        }
        let safepoint_id = hex_encode(h.finalize().as_slice());
        Self {
            snapshot_root,
            overlay_root: overlay_root.map(str::to_string),
            journal_head: journal_head.map(str::to_string),
            evidence_refs,
            path_policy_digest,
            control_receipt_head: control_receipt_head.map(str::to_string),
            safepoint_id,
            external_state_scope: "filesystem-project-state-only",
            nonsemantic_exclusions,
        }
    }

    pub fn assert_matches_snapshot(&self, snapshot: &ExactSnapshot) -> Result<(), SafepointError> {
        let actual = snapshot.root_digest_hex();
        if actual != self.snapshot_root {
            return Err(SafepointError::IdentityMismatch {
                expected: self.snapshot_root.clone(),
                actual: actual.to_string(),
            });
        }
        Ok(())
    }
}
