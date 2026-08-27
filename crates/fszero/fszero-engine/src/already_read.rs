//! Session already-read tracker for expand dedup (fszero-8qwq).

use std::collections::BTreeSet;

#[derive(Debug, Clone, Default)]
pub struct AlreadyReadTracker {
    seen: BTreeSet<String>,
}

impl AlreadyReadTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mark(&mut self, key: impl Into<String>) {
        self.seen.insert(key.into());
    }

    pub fn contains(&self, key: &str) -> bool {
        self.seen.contains(key)
    }

    pub fn filter_new<'a>(&self, keys: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
        keys.into_iter()
            .filter(|k| !self.seen.contains(*k))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }
}
