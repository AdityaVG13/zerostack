//! Bounded exact raw pages/ranges and expansion receipts (fszero-ip9y).

use sha2::{Digest, Sha256};
use std::ops::Range;

/// Line-ending canonicalization declared in evidence identity (V6-F3).
///
/// Raw (byte-exact) evidence paths stay byte-exact: `Raw` pages digest the
/// exact window bytes. LINE-addressed evidence opts into `Lf` so the same
/// logical lines share one evidence identity across CRLF and LF checkouts;
/// the policy is part of the page identity and is never silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEndingPolicy {
    /// Byte-exact: no canonicalization (default; raw identity paths).
    Raw,
    /// Canonical LF: CRLF and lone CR are normalized to LF before digesting.
    Lf,
}

impl LineEndingPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Lf => "lf",
        }
    }
}

/// Normalize CRLF and lone CR to LF when `policy` is [`LineEndingPolicy::Lf`].
/// `Raw` returns the input unchanged (byte-exact).
pub fn canonicalize_line_endings(bytes: &[u8], policy: LineEndingPolicy) -> Vec<u8> {
    if policy == LineEndingPolicy::Raw || !bytes.contains(&b'\r') {
        return bytes.to_vec();
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                if bytes.get(i + 1) == Some(&b'\n') {
                    out.push(b'\n');
                    i += 2;
                } else {
                    // Lone CR is a line ending too (classic Mac); fold to LF.
                    out.push(b'\n');
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

/// Range digest for LINE-addressed evidence: canonicalized window bytes under
/// an explicit policy, in a policy-tagged domain so the identity declares the
/// canonicalization (never silent). Same logical lines across CRLF/LF
/// checkouts produce the same digest; raw byte digests stay distinct.
pub fn line_digest_hex(bytes: &[u8], policy: LineEndingPolicy) -> String {
    let canonical = canonicalize_line_endings(bytes, policy);
    let mut h = Sha256::new();
    h.update(b"FSZERO-EVIDENCE-LINES-V1\0");
    h.update(policy.as_str().as_bytes());
    h.update(&[0]);
    h.update(&canonical);
    hex_encode(h.finalize().as_slice())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidencePageError {
    OutOfBounds {
        start: u64,
        end: u64,
        len: u64,
    },
    EmptyRange,
    DigestMismatch {
        expected: String,
        actual: String,
    },
    StaleSource {
        expected_root: String,
        actual_root: String,
    },
}

impl std::fmt::Display for EvidencePageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutOfBounds { start, end, len } => {
                write!(f, "range [{start},{end}) out of bounds for len {len}")
            }
            Self::EmptyRange => write!(f, "empty evidence range refused"),
            Self::DigestMismatch { expected, actual } => {
                write!(
                    f,
                    "range digest mismatch: expected {expected}, actual {actual}"
                )
            }
            Self::StaleSource {
                expected_root,
                actual_root,
            } => write!(
                f,
                "stale evidence source: expected root {expected_root}, actual {actual_root}"
            ),
        }
    }
}
impl std::error::Error for EvidencePageError {}

/// Exact byte range within a source blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactRange {
    pub start: u64,
    pub end: u64,
}

impl ExactRange {
    pub fn new(start: u64, end: u64) -> Result<Self, EvidencePageError> {
        if end <= start {
            return Err(EvidencePageError::EmptyRange);
        }
        Ok(Self { start, end })
    }

    pub fn as_usize_range(&self, len: usize) -> Result<Range<usize>, EvidencePageError> {
        let end = self.end as usize;
        let start = self.start as usize;
        if self.end > len as u64 {
            return Err(EvidencePageError::OutOfBounds {
                start: self.start,
                end: self.end,
                len: len as u64,
            });
        }
        Ok(start..end)
    }
}

/// One RACC-R evidence page: owner-scoped exact bytes with range digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidencePage {
    pub source_root_digest: String,
    pub path: String,
    pub range: ExactRange,
    pub range_digest_hex: String,
    pub bytes: Vec<u8>,
    /// Canonicalization applied before this page's range digest was derived.
    /// `Raw` = byte-exact; `Lf` = CRLF/CR normalized to LF. Declared in the
    /// identity; verification recomputes under the same policy (V6-F3).
    pub line_ending_policy: LineEndingPolicy,
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

pub fn range_digest_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(b"FSZERO-EVIDENCE-RANGE-V1\0");
    h.update(bytes);
    hex_encode(h.finalize().as_slice())
}

impl EvidencePage {
    /// Extract a bounded page from full file bytes under a known snapshot root.
    /// Byte-exact: the range digest covers the raw window bytes
    /// ([`LineEndingPolicy::Raw`], the default identity for raw paths).
    pub fn extract(
        source_root_digest: &str,
        path: &str,
        full: &[u8],
        range: ExactRange,
    ) -> Result<Self, EvidencePageError> {
        Self::extract_with_policy(source_root_digest, path, full, range, LineEndingPolicy::Raw)
    }

    /// Shared extraction: derives the range digest under `policy`.
    fn extract_with_policy(
        source_root_digest: &str,
        path: &str,
        full: &[u8],
        range: ExactRange,
        policy: LineEndingPolicy,
    ) -> Result<Self, EvidencePageError> {
        let slice = &full[range.as_usize_range(full.len())?];
        let range_digest_hex = match policy {
            LineEndingPolicy::Raw => range_digest_hex(slice),
            LineEndingPolicy::Lf => line_digest_hex(slice, policy),
        };
        Ok(Self {
            source_root_digest: source_root_digest.to_string(),
            path: path.to_string(),
            range,
            range_digest_hex,
            bytes: slice.to_vec(),
            line_ending_policy: policy,
        })
    }

    /// Extract a LINE-addressed page: the range digest is derived over the
    /// canonicalized (line-normalized) window bytes under an explicit policy.
    /// `range` and `bytes` stay byte-exact raw; only the digest uses the
    /// canonical form, and the policy is declared in the page identity
    /// (V6-F3). Same logical lines across CRLF/LF checkouts share one
    /// evidence identity; raw byte digests remain distinct.
    pub fn extract_line_addressed(
        source_root_digest: &str,
        path: &str,
        full: &[u8],
        range: ExactRange,
        policy: LineEndingPolicy,
    ) -> Result<Self, EvidencePageError> {
        let slice = &full[range.as_usize_range(full.len())?];
        let range_digest_hex = line_digest_hex(slice, policy);
        Ok(Self {
            source_root_digest: source_root_digest.to_string(),
            path: path.to_string(),
            range,
            range_digest_hex,
            bytes: slice.to_vec(),
            line_ending_policy: policy,
        })
    }

    /// Re-expand only when the source root still matches and range digest holds.
    pub fn verify_against_source(
        &self,
        actual_root: &str,
        full: &[u8],
    ) -> Result<(), EvidencePageError> {
        if actual_root != self.source_root_digest {
            return Err(EvidencePageError::StaleSource {
                expected_root: self.source_root_digest.clone(),
                actual_root: actual_root.to_string(),
            });
        }
        let slice = &full[self.range.as_usize_range(full.len())?];
        let actual = match self.line_ending_policy {
            LineEndingPolicy::Raw => range_digest_hex(slice),
            LineEndingPolicy::Lf => line_digest_hex(slice, LineEndingPolicy::Lf),
        };
        if actual != self.range_digest_hex {
            return Err(EvidencePageError::DigestMismatch {
                expected: self.range_digest_hex.clone(),
                actual,
            });
        }
        if slice != self.bytes.as_slice() {
            return Err(EvidencePageError::DigestMismatch {
                expected: self.range_digest_hex.clone(),
                actual: range_digest_hex(slice),
            });
        }
        Ok(())
    }
}
