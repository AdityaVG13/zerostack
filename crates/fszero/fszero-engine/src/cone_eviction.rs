//! Cone-scoped eviction for session memos (fszero-ogub).

use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct ConeMemoEntry {
    pub key: String,
    pub path_prefix: String,
    pub cost: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ConeMemoStore {
    entries: BTreeMap<String, ConeMemoEntry>,
}

impl ConeMemoStore {
    pub fn insert(&mut self, entry: ConeMemoEntry) {
        self.entries.insert(entry.key.clone(), entry);
    }

    /// Evict all memos whose path_prefix is under cone (or equal).
    pub fn evict_cone(&mut self, cone: &str) -> usize {
        let before = self.entries.len();
        self.entries.retain(|_, e| {
            !(e.path_prefix == cone || e.path_prefix.starts_with(&format!("{cone}/")))
        });
        before - self.entries.len()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}
