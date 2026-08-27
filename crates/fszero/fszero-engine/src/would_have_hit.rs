//! CacheZero would-have-hit ledger (fszero-r14c) — research instrumentation.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WouldHaveHit {
    pub determinant_digest: String,
    pub artifact_kind: String,
    pub would_hit: bool,
}

#[derive(Debug, Clone, Default)]
pub struct WouldHaveHitLedger {
    entries: Vec<WouldHaveHit>,
}

impl WouldHaveHitLedger {
    pub fn record(&mut self, entry: WouldHaveHit) {
        self.entries.push(entry);
    }

    pub fn hits(&self) -> usize {
        self.entries.iter().filter(|e| e.would_hit).count()
    }

    pub fn misses(&self) -> usize {
        self.entries.len().saturating_sub(self.hits())
    }

    pub fn by_kind(&self) -> BTreeMap<String, (usize, usize)> {
        let mut m = BTreeMap::new();
        for e in &self.entries {
            let slot = m.entry(e.artifact_kind.clone()).or_insert((0, 0));
            if e.would_hit {
                slot.0 += 1;
            } else {
                slot.1 += 1;
            }
        }
        m
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}
