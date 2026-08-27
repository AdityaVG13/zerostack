//! Certified empty answers with scope roots (fszero-ojnv).
//!
//! Negative entries are not "misses": a put only happens after a complete
//! scoped scan returns zero hits. Invalidation is generation-keyed so an
//! index rebuild never reuses a stale empty certificate.

use std::collections::BTreeMap;

/// Wire kind for cache-entry-aligned negative answers.
pub const NO_MATCHES_KIND: &str = "no_matches";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegativeEntry {
    pub query: String,
    /// Anti-dependency scope: roots that were fully scanned for this empty answer.
    pub scope_roots: Vec<String>,
    pub certified_empty: bool,
    /// Index generation at certification time; mismatch => stale.
    pub index_generation: u64,
    /// Value kind discriminator (`no_matches`).
    pub value_kind: &'static str,
}

#[derive(Debug, Clone, Default)]
pub struct NegativeCache {
    by_query: BTreeMap<String, NegativeEntry>,
}

impl NegativeCache {
    pub fn put_empty(
        &mut self,
        query: impl Into<String>,
        scope_roots: Vec<String>,
        index_generation: u64,
    ) {
        let query = query.into();
        let mut roots = scope_roots;
        roots.sort();
        roots.dedup();
        self.by_query.insert(
            query.clone(),
            NegativeEntry {
                query,
                scope_roots: roots,
                certified_empty: true,
                index_generation,
                value_kind: NO_MATCHES_KIND,
            },
        );
    }

    pub fn lookup(&self, query: &str) -> Option<&NegativeEntry> {
        self.by_query.get(query)
    }

    /// Certified empty only when generation still matches and scope is non-empty.
    pub fn is_certified_empty(&self, query: &str, index_generation: u64) -> bool {
        match self.by_query.get(query) {
            Some(e) => {
                e.certified_empty
                    && e.index_generation == index_generation
                    && !e.scope_roots.is_empty()
                    && e.value_kind == NO_MATCHES_KIND
            }
            None => false,
        }
    }

    /// Drop entries whose generation no longer matches (index changed).
    pub fn invalidate_generation(&mut self, current_generation: u64) {
        self.by_query
            .retain(|_, e| e.index_generation == current_generation);
    }

    /// Drop every entry that listed any of the given roots (or a parent/child prefix).
    pub fn invalidate_scopes(&mut self, changed_roots: &[String]) {
        if changed_roots.is_empty() {
            return;
        }
        self.by_query.retain(|_, e| {
            !e.scope_roots.iter().any(|sr| {
                if sr == "." || sr == "/" {
                    return true;
                }
                changed_roots.iter().any(|cr| scope_overlaps(sr, cr))
            })
        });
    }

    pub fn clear(&mut self) {
        self.by_query.clear();
    }
}

/// cache-entry key fragment: real scope_roots (never hardcode empty when known).
pub fn scope_roots_for_key(scope_roots: &[String]) -> Vec<String> {
    let mut v: Vec<String> = scope_roots
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect();
    v.sort();
    v.dedup();
    v
}

fn scope_overlaps(a: &str, b: &str) -> bool {
    let a = a.trim_end_matches('/');
    let b = b.trim_end_matches('/');
    if a.is_empty() || b.is_empty() {
        return true;
    }
    a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
}
