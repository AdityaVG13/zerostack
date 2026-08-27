//! Journal-fed certificate freshness (fszero-m9ue / omega).
//!
//! A certificate is fresh when no unapplied journal mutation in its declared
//! range post-dates `journal_ordinal_at_assembly`. Cross-engine verifiers only
//! need this schema (JSON object); no FSZero-private types on the wire.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalFreshnessCertificate {
    pub schema: &'static str,
    pub segment_id: String,
    pub journal_ordinal_at_assembly: i64,
    /// Inclusive start / exclusive end of journal seq coverage.
    pub mutation_range_start: i64,
    pub mutation_range_end: i64,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FreshnessError {
    Stale {
        certificate_ordinal: i64,
        conflicting_seq: i64,
        conflicting_path: String,
    },
    TornJournal {
        expected_next: i64,
        found: i64,
    },
    Gap {
        after: i64,
        next: i64,
    },
    RangeEmpty,
}

impl std::fmt::Display for FreshnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stale {
                certificate_ordinal,
                conflicting_seq,
                conflicting_path,
            } => write!(
                f,
                "stale certificate: assembled at ordinal {certificate_ordinal}, conflicting mutation seq={conflicting_seq} path={conflicting_path}"
            ),
            Self::TornJournal {
                expected_next,
                found,
            } => write!(
                f,
                "torn journal: expected next seq {expected_next}, found {found}"
            ),
            Self::Gap { after, next } => {
                write!(f, "journal ordinal gap after {after}: next {next}")
            }
            Self::RangeEmpty => write!(f, "empty mutation range"),
        }
    }
}
impl std::error::Error for FreshnessError {}

/// One journal mutation row needed for freshness checks (engine-neutral).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalMutation {
    pub seq: i64,
    pub path: String,
}

pub const CERT_SCHEMA: &str = "fszero.journal-freshness/v1";

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

pub fn content_hash_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex_encode(h.finalize().as_slice())
}

impl JournalFreshnessCertificate {
    pub fn assemble(
        segment_id: impl Into<String>,
        journal_ordinal_at_assembly: i64,
        mutation_range_start: i64,
        mutation_range_end: i64,
        content: &[u8],
    ) -> Result<Self, FreshnessError> {
        if mutation_range_end < mutation_range_start {
            return Err(FreshnessError::RangeEmpty);
        }
        Ok(Self {
            schema: CERT_SCHEMA,
            segment_id: segment_id.into(),
            journal_ordinal_at_assembly,
            mutation_range_start,
            mutation_range_end,
            content_hash: content_hash_hex(content),
        })
    }

    /// Wire JSON without FSZero-private types (gz/tz can parse as generic JSON).
    pub fn to_wire_json(&self) -> String {
        format!(
            "{{\"schema\":\"{}\",\"segment_id\":\"{}\",\"journal_ordinal_at_assembly\":{},\"mutation_range_start\":{},\"mutation_range_end\":{},\"content_hash\":\"{}\"}}",
            self.schema,
            self.segment_id.replace('"', "\\\""),
            self.journal_ordinal_at_assembly,
            self.mutation_range_start,
            self.mutation_range_end,
            self.content_hash
        )
    }
}

/// Verify certificate against the current journal page for its range.
pub fn verify_freshness(
    cert: &JournalFreshnessCertificate,
    journal: &[JournalMutation],
) -> Result<(), FreshnessError> {
    let mut prev: Option<i64> = None;
    for m in journal {
        if m.seq < cert.mutation_range_start || m.seq >= cert.mutation_range_end {
            continue;
        }
        if let Some(p) = prev {
            if m.seq == p {
                return Err(FreshnessError::TornJournal {
                    expected_next: p + 1,
                    found: m.seq,
                });
            }
            if m.seq > p + 1 {
                return Err(FreshnessError::Gap {
                    after: p,
                    next: m.seq,
                });
            }
        }
        if m.seq > cert.journal_ordinal_at_assembly {
            return Err(FreshnessError::Stale {
                certificate_ordinal: cert.journal_ordinal_at_assembly,
                conflicting_seq: m.seq,
                conflicting_path: m.path.clone(),
            });
        }
        prev = Some(m.seq);
    }
    Ok(())
}
