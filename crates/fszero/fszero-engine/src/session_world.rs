//! Session-scope worlds: fork/replay/diff agent session state (fszero-g7c).
//!
//! Builds on mutation-journal ordinals. Does not claim model-policy quality;
//! only exact journaled session bytes and three-way diffs.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub turn: u64,
    pub journal_ordinal: i64,
    /// Logical session keys (store keys / seen-set markers) -> content digests.
    pub state: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionWorld {
    pub id: String,
    pub base_turn: u64,
    pub snapshots: Vec<SessionSnapshot>,
    pub parent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDiff {
    pub only_left: BTreeMap<String, String>,
    pub only_right: BTreeMap<String, String>,
    pub changed: BTreeMap<String, (String, String)>,
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

pub fn digest_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex_encode(h.finalize().as_slice())
}

impl SessionWorld {
    pub fn trunk(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            base_turn: 0,
            snapshots: vec![SessionSnapshot {
                turn: 0,
                journal_ordinal: 0,
                state: BTreeMap::new(),
            }],
            parent: None,
        }
    }

    pub fn latest(&self) -> &SessionSnapshot {
        self.snapshots
            .last()
            .expect("session world always has base")
    }

    pub fn record_turn(&mut self, journal_ordinal: i64, puts: BTreeMap<String, Vec<u8>>) {
        let mut state = self.latest().state.clone();
        for (k, v) in puts {
            state.insert(k, digest_hex(&v));
        }
        let turn = self.latest().turn + 1;
        self.snapshots.push(SessionSnapshot {
            turn,
            journal_ordinal,
            state,
        });
    }

    /// Fork from a turn boundary (inclusive snapshot).
    pub fn fork_at(&self, id: impl Into<String>, turn: u64) -> Option<Self> {
        let snap = self.snapshots.iter().find(|s| s.turn == turn)?.clone();
        Some(Self {
            id: id.into(),
            base_turn: turn,
            snapshots: vec![snap],
            parent: Some(self.id.clone()),
        })
    }

    pub fn three_way_diff(
        base: &SessionSnapshot,
        left: &SessionSnapshot,
        right: &SessionSnapshot,
    ) -> SessionDiff {
        let mut only_left = BTreeMap::new();
        let mut only_right = BTreeMap::new();
        let mut changed = BTreeMap::new();
        let mut keys: std::collections::BTreeSet<_> = base.state.keys().cloned().collect();
        keys.extend(left.state.keys().cloned());
        keys.extend(right.state.keys().cloned());
        for k in keys {
            let b = base.state.get(&k);
            let l = left.state.get(&k);
            let r = right.state.get(&k);
            match (l, r) {
                (Some(lv), Some(rv)) if lv != rv => {
                    changed.insert(k, (lv.clone(), rv.clone()));
                }
                (Some(lv), None) if b != Some(lv) || b.is_none() => {
                    only_left.insert(k, lv.clone());
                }
                (None, Some(rv)) if b != Some(rv) || b.is_none() => {
                    only_right.insert(k, rv.clone());
                }
                (Some(_), None) => {
                    only_left.insert(k.clone(), l.cloned().unwrap());
                }
                (None, Some(_)) => {
                    only_right.insert(k.clone(), r.cloned().unwrap());
                }
                _ => {}
            }
        }
        SessionDiff {
            only_left,
            only_right,
            changed,
        }
    }
}
