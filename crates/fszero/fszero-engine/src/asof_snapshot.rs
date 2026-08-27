//! AS-OF world snapshots for volume mounts (fszero-dke0 / 1tp).
//!
//! Time-travel reads over a journal of put/delete mutations. Snapshot refs are
//! content digests (`fz://blob/<hex>`), not inline bytes. External volume
//! mounts (TokenZero) consume the wire form without an FSZero process.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsofError {
    UnknownPath(String),
    OrdinalOutOfRange { as_of: i64, max: i64 },
    DeletedAtOrdinal { path: String, as_of: i64 },
}

impl std::fmt::Display for AsofError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownPath(p) => write!(f, "unknown path: {p}"),
            Self::OrdinalOutOfRange { as_of, max } => {
                write!(f, "as_of {as_of} out of range (max {max})")
            }
            Self::DeletedAtOrdinal { path, as_of } => {
                write!(f, "path {path} deleted at or before ordinal {as_of}")
            }
        }
    }
}
impl std::error::Error for AsofError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsofMutation {
    Put {
        seq: i64,
        path: String,
        bytes: Vec<u8>,
    },
    Delete {
        seq: i64,
        path: String,
    },
}

impl AsofMutation {
    pub fn seq(&self) -> i64 {
        match self {
            Self::Put { seq, .. } | Self::Delete { seq, .. } => *seq,
        }
    }
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

pub fn blob_ref(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("fz://blob/{}", hex_encode(h.finalize().as_slice()))
}

/// Journal-backed AS-OF store (in-memory model for exact temporal reads).
#[derive(Debug, Clone, Default)]
pub struct AsofJournal {
    mutations: Vec<AsofMutation>,
}

impl AsofJournal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, m: AsofMutation) {
        self.mutations.push(m);
        self.mutations.sort_by_key(|m| m.seq());
    }

    pub fn max_seq(&self) -> i64 {
        self.mutations.iter().map(|m| m.seq()).max().unwrap_or(0)
    }

    /// Exact bytes at ordinal `as_of` (inclusive of mutations with seq <= as_of).
    pub fn read_as_of(&self, path: &str, as_of: i64) -> Result<Vec<u8>, AsofError> {
        if as_of < 0 {
            return Err(AsofError::OrdinalOutOfRange {
                as_of,
                max: self.max_seq(),
            });
        }
        let mut present: Option<Vec<u8>> = None;
        let mut seen = false;
        for m in &self.mutations {
            if m.seq() > as_of {
                break;
            }
            match m {
                AsofMutation::Put { path: p, bytes, .. } if p == path => {
                    present = Some(bytes.clone());
                    seen = true;
                }
                AsofMutation::Delete { path: p, .. } if p == path => {
                    present = None;
                    seen = true;
                }
                _ => {}
            }
        }
        if !seen {
            return Err(AsofError::UnknownPath(path.to_string()));
        }
        present.ok_or_else(|| AsofError::DeletedAtOrdinal {
            path: path.to_string(),
            as_of,
        })
    }

    /// AS-OF read returning an fz://blob ref (expandable by any engine).
    pub fn snapshot_ref(&self, path: &str, as_of: i64) -> Result<String, AsofError> {
        let bytes = self.read_as_of(path, as_of)?;
        Ok(blob_ref(&bytes))
    }

    /// Full tree as of ordinal: path -> bytes.
    pub fn tree_as_of(&self, as_of: i64) -> BTreeMap<String, Vec<u8>> {
        let mut tree = BTreeMap::new();
        for m in &self.mutations {
            if m.seq() > as_of {
                break;
            }
            match m {
                AsofMutation::Put { path, bytes, .. } => {
                    tree.insert(path.clone(), bytes.clone());
                }
                AsofMutation::Delete { path, .. } => {
                    tree.remove(path);
                }
            }
        }
        tree
    }

    /// Fork at ordinal k, apply alternative mutation(s) on the fork only.
    pub fn counterfactual_fork(
        &self,
        at: i64,
        alt: AsofMutation,
    ) -> (BTreeMap<String, Vec<u8>>, BTreeMap<String, Vec<u8>>) {
        let trunk = self.tree_as_of(self.max_seq());
        let mut fork_journal = AsofJournal::new();
        for m in &self.mutations {
            if m.seq() <= at {
                fork_journal.apply(m.clone());
            }
        }
        fork_journal.apply(alt);
        let fork = fork_journal.tree_as_of(fork_journal.max_seq());
        (trunk, fork)
    }
}
